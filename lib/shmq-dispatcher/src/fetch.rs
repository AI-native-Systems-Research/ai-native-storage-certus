//! The `FETCH` op: fetch a run of blocks from a named peer's transfer endpoint
//! into the local memory tier, paced in a byte-bounded window, and attribute
//! each fetched block's first serve as [`ServedBy::Fetched`].
//!
//! The contract is `components/certus-transfer-endpoint/specs/001-rdma-cache-transfer/
//! contracts/shmq-fetch.md`. In short:
//!
//! * **One op names the whole run** — the leading keys of a request that miss
//!   locally, in prefix order. The op never waits for the transfer: each key
//!   reads `PENDING` on Check until it is `RESIDENT` (fetched) or `MISS`.
//! * **The server paces it.** At most `window_bytes` of unpublished slots per
//!   run and `inflight_bytes` across all runs. A slot is reserved with
//!   `reserve_memory` (unpublished, as a store's), handed to
//!   `ITransferEndpoint::fetch`, and on completion either published
//!   (`copy_gpu_to_memory_completed`, which also schedules write-through) or
//!   released. Then the window advances.
//! * **The run stops at its first miss**: what follows is not a usable prefix.
//! * **Only a completion reclaims a slot.** The peer may still be writing it
//!   until then (FR-017), so these slots are never in `pending_stores`, which
//!   the stale-reservation reaper reclaims by age.
//!
//! Completions run on the endpoint's thread and must not block: each one only
//! settles its keys and issues the next window, under one short lock.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use interfaces::{EntryOutcome, FetchResult, IDispatcher, ITransferEndpoint, LocalSlot, ServedBy};

/// Server-side limits on fetching, all in bytes but the last.
#[derive(Debug, Clone, Copy)]
pub struct FetchLimits {
    /// Unpublished slots one run may hold at once.
    pub window_bytes: u64,
    /// Unpublished slots all runs together may hold at once.
    pub inflight_bytes: u64,
    /// Keys one FETCH may name: a sanity bound on one message.
    pub max_keys: usize,
    /// How long a fetched block stays marked for `Fetched` attribution if no
    /// lookup serves it, and no eviction is seen for it.
    pub mark_ttl: Duration,
}

impl Default for FetchLimits {
    fn default() -> Self {
        FetchLimits {
            window_bytes: 128 << 20,
            inflight_bytes: 1 << 30,
            max_keys: 65_536,
            mark_ttl: Duration::from_secs(30),
        }
    }
}

/// Cumulative fetch counters. Every one is counted where its event happens.
#[derive(Debug, Default)]
pub struct FetchCounters {
    pub requests: AtomicU64,
    pub keys_requested: AtomicU64,
    pub keys_started: AtomicU64,
    pub not_started: AtomicU64,
    pub hits: AtomicU64,
    pub bytes: AtomicU64,
    pub misses: AtomicU64,
    pub failed: AtomicU64,
    pub truncated: AtomicU64,
    pub window_waits: AtomicU64,
    pub lookup_hits_fetched: AtomicU64,
    pub fetched_unused: AtomicU64,
}

/// A copy of [`FetchCounters`], for metrics and tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FetchStats {
    pub requests: u64,
    pub keys_requested: u64,
    pub keys_started: u64,
    pub not_started: u64,
    pub hits: u64,
    pub bytes: u64,
    pub misses: u64,
    pub failed: u64,
    pub truncated: u64,
    pub window_waits: u64,
    pub lookup_hits_fetched: u64,
    pub fetched_unused: u64,
    /// Gauges: keys still in a run, fetched blocks still marked.
    pub keys_pending: u64,
    pub marks_live: u64,
}

/// One run: a peer and the keys still to fetch, in prefix order.
struct Run {
    peer: String,
    /// Not yet issued, in order.
    queued: VecDeque<(u64, u32)>,
    /// Issued: reserved and handed to the endpoint, awaiting completion.
    inflight_bytes: u64,
    inflight: usize,
    /// A miss (or a refused reservation) ended the run: what is queued will
    /// never be issued, and is dropped once the in-flight keys settle.
    cut: bool,
}

#[derive(Default)]
struct State {
    runs: HashMap<u64, Run>,
    next_run: u64,
    /// Every key in a run, issued or queued, and its run: what Check reads as
    /// pending and the guard against two runs for one key.
    keys: HashMap<u64, u64>,
    /// Unpublished bytes across all runs.
    inflight_bytes: u64,
    /// Runs that could not issue for want of `inflight_bytes`, oldest first.
    waiting: VecDeque<u64>,
    /// Fetched blocks not yet served: key → (peer, landed at).
    marks: HashMap<u64, (String, Instant)>,
}

struct Inner {
    dispatcher: Arc<dyn IDispatcher + Send + Sync>,
    endpoint: Arc<dyn ITransferEndpoint + Send + Sync>,
    limits: FetchLimits,
    state: Mutex<State>,
    counters: FetchCounters,
}

/// The fetch engine. Cloneable; every clone shares one state.
#[derive(Clone)]
pub struct Fetcher(Arc<Inner>);

impl std::fmt::Debug for Fetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetcher")
            .field("limits", &self.0.limits)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

/// Why a FETCH was refused whole.
#[derive(Debug, PartialEq, Eq)]
pub enum FetchRefused {
    TooMany,
}

impl Fetcher {
    pub fn new(
        dispatcher: Arc<dyn IDispatcher + Send + Sync>,
        endpoint: Arc<dyn ITransferEndpoint + Send + Sync>,
        limits: FetchLimits,
    ) -> Self {
        Fetcher(Arc::new(Inner {
            dispatcher,
            endpoint,
            limits,
            state: Mutex::new(State::default()),
            counters: FetchCounters::default(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Start a run of `keys` from `peer`. `pending` are keys a store already
    /// has in flight. Returns one `started` flag per key: the run is the keys
    /// answered `true`, and stops at the first key that is not (it is already
    /// resident or pending, so what follows would not be a usable prefix
    /// through it — and is not fetched either).
    ///
    /// # Errors
    ///
    /// [`FetchRefused::TooMany`] if `keys` exceeds the limit; nothing started.
    pub fn start(
        &self,
        peer: &str,
        keys: &[(u64, u32)],
        pending: &HashSet<u64>,
    ) -> Result<Vec<bool>, FetchRefused> {
        let c = &self.0.counters;
        if keys.len() > self.0.limits.max_keys {
            return Err(FetchRefused::TooMany);
        }
        c.requests.fetch_add(1, Ordering::Relaxed);
        c.keys_requested
            .fetch_add(keys.len() as u64, Ordering::Relaxed);
        // `check` may wait on a dispatch-map writer, so it runs outside the
        // lock; whether another run owns a key is settled under it, below.
        let mut candidates = Vec::new();
        for &(key, size) in keys {
            if pending.contains(&key) || self.0.dispatcher.check(key).unwrap_or(false) {
                break;
            }
            candidates.push((key, size));
        }
        let mut started = vec![false; keys.len()];
        let mut queued = VecDeque::new();
        let id = {
            let mut state = self.lock();
            let id = state.next_run;
            for (i, &(key, size)) in candidates.iter().enumerate() {
                if state.keys.contains_key(&key) {
                    break; // another run is fetching it
                }
                state.keys.insert(key, id);
                started[i] = true;
                queued.push_back((key, size));
            }
            let n = queued.len() as u64;
            c.keys_started.fetch_add(n, Ordering::Relaxed);
            c.not_started
                .fetch_add(keys.len() as u64 - n, Ordering::Relaxed);
            if queued.is_empty() {
                return Ok(started);
            }
            state.next_run += 1;
            state.runs.insert(
                id,
                Run {
                    peer: peer.to_string(),
                    queued,
                    inflight_bytes: 0,
                    inflight: 0,
                    cut: false,
                },
            );
            id
        };
        self.issue(id);
        Ok(started)
    }

    /// Issue as much of run `id` as its window and the global cap allow.
    fn issue(&self, id: u64) {
        let limits = self.0.limits;
        // Reserve under the lock, so two completions cannot both take the
        // same headroom; the endpoint call happens after it is released.
        let (peer, slots) = {
            let mut state = self.lock();
            let State {
                runs,
                inflight_bytes,
                waiting,
                keys,
                ..
            } = &mut *state;
            let Some(run) = runs.get_mut(&id) else {
                return;
            };
            let mut slots = Vec::new();
            while let Some(&(key, size)) = run.queued.front().filter(|_| !run.cut) {
                let size64 = u64::from(size);
                let fits_window =
                    run.inflight == 0 || run.inflight_bytes + size64 <= limits.window_bytes;
                if !fits_window {
                    break;
                }
                if *inflight_bytes > 0 && *inflight_bytes + size64 > limits.inflight_bytes {
                    if run.inflight == 0 && !waiting.contains(&id) {
                        waiting.push_back(id);
                        self.0.counters.window_waits.fetch_add(1, Ordering::Relaxed);
                    }
                    break;
                }
                // No deadline: a fetch takes only what is free now. A full
                // tier ends the run rather than evict the cache for a guess.
                let reserved = self
                    .0
                    .dispatcher
                    .reserve_memory(key, size, 0, Some(Instant::now()));
                let Ok(ptr) = reserved else {
                    run.cut = true;
                    break;
                };
                run.queued.pop_front();
                run.inflight += 1;
                run.inflight_bytes += size64;
                *inflight_bytes += size64;
                slots.push((
                    key,
                    LocalSlot {
                        addr: ptr as u64,
                        length: size,
                    },
                ));
            }
            if run.cut && !run.queued.is_empty() {
                Self::drop_queued(keys, run, &self.0.counters);
            }
            (run.peer.clone(), slots)
        };
        if !slots.is_empty() {
            let me = Arc::downgrade(&self.0);
            let issued = slots.clone();
            let cb = Box::new(move |r: FetchResult| {
                if let Some(inner) = Weak::upgrade(&me) {
                    Fetcher(inner).complete(id, &issued, r);
                }
            });
            if let Err(e) = self.0.endpoint.fetch(&peer, &slots, cb) {
                // Refused before anything was sent: nothing can land.
                self.complete(id, &slots, Err(e));
            }
        }
        self.finish_if_done(id);
    }

    /// The queued keys of a cut run never get fetched: they read MISS.
    fn drop_queued(keys: &mut HashMap<u64, u64>, run: &mut Run, c: &FetchCounters) {
        c.truncated
            .fetch_add(run.queued.len() as u64, Ordering::Relaxed);
        for (key, _) in run.queued.drain(..) {
            keys.remove(&key);
        }
    }

    /// A completion: publish hits, release the rest, cut the run at a miss,
    /// and issue the next window — of this run, or of one the cap held back.
    fn complete(&self, id: u64, slots: &[(u64, LocalSlot)], result: FetchResult) {
        let c = &self.0.counters;
        let outcomes: Vec<EntryOutcome> = match result {
            Ok(o) if o.len() == slots.len() => o,
            // Every slot of a failed fetch is to be treated as not landed.
            _ => vec![EntryOutcome::Failed; slots.len()],
        };
        let mut published = Vec::new();
        let mut cut = false;
        for (&(key, slot), outcome) in slots.iter().zip(&outcomes) {
            // A hit after a miss is not a usable prefix either, but it is a
            // valid entry: keep it, it may serve another request.
            let ok = outcome.is_hit()
                && self
                    .0
                    .dispatcher
                    .copy_gpu_to_memory_completed(key, slot.length)
                    .is_ok();
            if ok {
                c.hits.fetch_add(1, Ordering::Relaxed);
                c.bytes.fetch_add(u64::from(slot.length), Ordering::Relaxed);
                published.push(key);
            } else {
                let _ = self.0.dispatcher.release_memory(key);
                cut = true;
                match outcome {
                    EntryOutcome::Miss => c.misses.fetch_add(1, Ordering::Relaxed),
                    _ => c.failed.fetch_add(1, Ordering::Relaxed),
                };
            }
        }
        let next = {
            let mut state = self.lock();
            let State {
                runs,
                keys,
                inflight_bytes,
                marks,
                waiting,
                ..
            } = &mut *state;
            let bytes: u64 = slots.iter().map(|(_, s)| u64::from(s.length)).sum();
            *inflight_bytes -= bytes;
            for &(key, _) in slots {
                keys.remove(&key);
            }
            if let Some(run) = runs.get_mut(&id) {
                run.inflight -= slots.len();
                run.inflight_bytes -= bytes;
                for &key in &published {
                    marks.insert(key, (run.peer.clone(), Instant::now()));
                }
                if cut && !run.cut {
                    run.cut = true;
                    Self::drop_queued(keys, run, c);
                }
            }
            // Freed headroom goes to this run first, then to the oldest held
            // back.
            let mut next = vec![id];
            next.extend(waiting.drain(..));
            next
        };
        for run in next {
            self.issue(run);
        }
    }

    fn finish_if_done(&self, id: u64) {
        let mut state = self.lock();
        let done = state
            .runs
            .get(&id)
            .is_some_and(|r| r.inflight == 0 && (r.queued.is_empty() || r.cut));
        if done {
            state.runs.remove(&id);
        }
    }

    /// Keys of `keys` that are in a run now: Check reads them `PENDING`.
    #[must_use]
    pub fn pending(&self, keys: &[u64]) -> HashSet<u64> {
        let state = self.lock();
        keys.iter()
            .copied()
            .filter(|k| state.keys.contains_key(k))
            .collect()
    }

    /// Re-attribute a lookup's outcome: the first serve of a fetched block is
    /// `Fetched`, not the `Dram` the dispatcher sees. The mark is consumed.
    #[must_use]
    pub fn attribute(&self, key: u64, served_by: ServedBy) -> ServedBy {
        if served_by != ServedBy::Dram {
            return served_by;
        }
        let mut state = self.lock();
        match state.marks.remove(&key) {
            Some((_, at)) if at.elapsed() < self.0.limits.mark_ttl => {
                self.0
                    .counters
                    .lookup_hits_fetched
                    .fetch_add(1, Ordering::Relaxed);
                ServedBy::Fetched
            }
            Some(_) => {
                self.0
                    .counters
                    .fetched_unused
                    .fetch_add(1, Ordering::Relaxed);
                served_by
            }
            None => served_by,
        }
    }

    /// A fetched block left the memory tier before any lookup served it.
    pub fn evicted(&self, key: u64) {
        if self.lock().marks.remove(&key).is_some() {
            self.0
                .counters
                .fetched_unused
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Drop every mark, counting each as unused: the cache was emptied, so no
    /// marked block can be served. Returns how many were dropped.
    pub fn clear_marks(&self) -> usize {
        let mut state = self.lock();
        let n = state.marks.len();
        state.marks.clear();
        self.0
            .counters
            .fetched_unused
            .fetch_add(n as u64, Ordering::Relaxed);
        n
    }

    /// Drop marks older than the TTL, counting each as unused. Called by the
    /// reaper.
    #[must_use]
    pub fn expire_marks(&self) -> usize {
        let ttl = self.0.limits.mark_ttl;
        let mut state = self.lock();
        let before = state.marks.len();
        state.marks.retain(|_, (_, at)| at.elapsed() < ttl);
        let n = before - state.marks.len();
        self.0
            .counters
            .fetched_unused
            .fetch_add(n as u64, Ordering::Relaxed);
        n
    }

    #[must_use]
    pub fn stats(&self) -> FetchStats {
        let c = &self.0.counters;
        let ld = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let state = self.lock();
        FetchStats {
            requests: ld(&c.requests),
            keys_requested: ld(&c.keys_requested),
            keys_started: ld(&c.keys_started),
            not_started: ld(&c.not_started),
            hits: ld(&c.hits),
            bytes: ld(&c.bytes),
            misses: ld(&c.misses),
            failed: ld(&c.failed),
            truncated: ld(&c.truncated),
            window_waits: ld(&c.window_waits),
            lookup_hits_fetched: ld(&c.lookup_hits_fetched),
            fetched_unused: ld(&c.fetched_unused),
            keys_pending: state.keys.len() as u64,
            marks_live: state.marks.len() as u64,
        }
    }
}
