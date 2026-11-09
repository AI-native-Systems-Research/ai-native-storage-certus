//! Server role: answering peers' KEY_QUERYs and serving their RDMA_REQUESTs.
//!
//! On a KEY_QUERY the actor classifies each `(key, size)` against the
//! dispatch-map (memory / disk / not-available, FR-015) and whispers a
//! KEY_RESPONSE. On an RDMA_REQUEST it pins each value, delegates the write to
//! `IRemoteLookupRdmaInitiator::push`, and whispers per-key RDMA_STATUS.
//! Disk-resident keys are promoted to the memory tier first via
//! `IDispatcher::promote_to_memory_tier` (US4), batched once per request so the
//! promotion fans out across drives; a key that is still not memory-resident
//! afterwards reports `KeyNoLongerAvailable`.
//!
//! These are pure functions over the local receptacles — they compute the wire
//! response but do not touch zyre; the actor performs the whisper. A node acts
//! as a server for a peer's operation while simultaneously running its own
//! `batch_lookup`s as a client (the dual-role concurrency the mesh tests
//! exercise).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use interfaces::{
    CacheKey, Endpoint, IDispatchMap, IDispatcher, IRemoteLookupRdmaInitiator, LookupResult,
    PushStatus, RemoteRegion,
};

use crate::wire::{Avail, RdmaStatusCode, SlotDesc};

/// Receives a serve's per-key statuses once the RDMA writes have completed.
///
/// Boxed and `Send` because it is invoked from the initiator's connection thread,
/// not the thread that started the serve.
pub(crate) type ServeCompletion = Box<dyn FnOnce(Vec<(CacheKey, RdmaStatusCode)>) + Send>;

/// A batch of dispatch-map read pins, released together when this is dropped.
///
/// Pins have to outlive the *completion* of an RDMA write, not merely the call that
/// submits it — the NIC reads the pinned buffers asynchronously, and the pin is what
/// stops the memory tier evicting them out from under it. Since submission is
/// asynchronous, the set of paths that must release is large (success, per-key
/// failure, rejected submission, connection loss, teardown), and a missed release is
/// unrecoverable: `read_ref` carries no owner identity, so a leaked pin makes its
/// entry permanently unevictable and is indistinguishable from a live reader. There
/// is no leak detector to catch it.
///
/// Hence a guard rather than hand-rolled release loops: whoever owns the completion
/// callback owns this, and dropping the callback — however that happens — releases
/// every pin in it.
pub(crate) struct PinnedBatch {
    dispatch_map: Arc<dyn IDispatchMap + Send + Sync>,
    keys: Vec<CacheKey>,
    /// Shared with `serve_stats()`, so a stalled serve is visible from outside this
    /// crate. The guard is the only place that knows both when a pin was taken and
    /// when it was released, which is why the accounting lives here rather than at
    /// the call sites.
    counters: Arc<ServeCounters>,
    /// When this batch came into existence. The interval from here to `drop` is
    /// exactly the window in which these keys cannot be evicted.
    created: Instant,
}

impl PinnedBatch {
    /// An empty batch pinning nothing.
    fn new(
        dispatch_map: Arc<dyn IDispatchMap + Send + Sync>,
        counters: Arc<ServeCounters>,
    ) -> Self {
        Self {
            dispatch_map,
            keys: Vec::new(),
            counters,
            created: Instant::now(),
        }
    }

    /// Take ownership of an already-held read pin on `key`.
    ///
    /// The caller must have obtained the pin (via `lookup` or `take_read`) and must
    /// not release it itself.
    fn adopt(&mut self, key: CacheKey) {
        self.keys.push(key);
        // One atomic per key. `pins_taken` was dropped: it measured 1.000x
        // `peer_served_keys` on every run, so it duplicated an already-published
        // counter for the cost of a second per-key atomic on the serve path.
        self.counters.pins_held.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for PinnedBatch {
    fn drop(&mut self) {
        let held = self.keys.len() as u64;
        for key in self.keys.drain(..) {
            // Errors are not actionable here: a failed release means the entry is
            // already gone, which is the outcome we wanted.
            let _ = self.dispatch_map.release_read(key);
        }
        if held == 0 {
            // A serve that found nothing servable still builds a batch. Such a batch
            // blocks no eviction, so recording its lifetime would pull the mean hold
            // time toward zero and hide the holds that matter.
            return;
        }
        self.counters.pins_held.fetch_sub(held, Ordering::Relaxed);
        let us = self.created.elapsed().as_micros() as u64;
        // Only the MAX is kept. A summed total cannot yield a mean hold: it accumulates
        // per BATCH while every key count available is per KEY, so their quotient is
        // batch-microseconds-per-key, not a duration. A real mean needs a batch counter
        // that nothing provides, and the summed form invites exactly that bad division.
        self.counters
            .pin_hold_us_max
            .fetch_max(us, Ordering::Relaxed);
    }
}

/// Reports a serve's statuses exactly once, and holds the read pins until it does.
///
/// The completion callback handed to the initiator owns one of these. If the
/// initiator instead *drops* that callback — a rejected submission, or teardown with
/// the batch still queued — `Drop` reports the pushed keys as unable-to-connect, so
/// the requester always hears an answer rather than waiting out its deadline.
struct ServeReport {
    /// Statuses decided before any RDMA (absent, size-mismatched, cold-and-lost).
    decided: Vec<(CacheKey, RdmaStatusCode)>,
    /// Keys handed to the initiator, in the order their statuses come back.
    pushed: Vec<CacheKey>,
    /// Taken when reported, so `Drop` knows whether it still owes an answer.
    on_done: Option<ServeCompletion>,
    /// Released once the report is made — after the NIC is done with the buffers.
    _pinned: PinnedBatch,
}

impl ServeReport {
    /// Report the outcome of a completed push.
    fn complete(&mut self, results: Vec<PushStatus>) {
        let Some(on_done) = self.on_done.take() else {
            return;
        };
        let mut out = std::mem::take(&mut self.decided);
        for (key, status) in self.pushed.iter().zip(results) {
            out.push((*key, map_push_status(status)));
        }
        on_done(out);
    }
}

impl Drop for ServeReport {
    fn drop(&mut self) {
        if let Some(on_done) = self.on_done.take() {
            let mut out = std::mem::take(&mut self.decided);
            for key in &self.pushed {
                out.push((*key, RdmaStatusCode::UnableToConnect));
            }
            on_done(out);
        }
    }
}

/// Classify each queried `(key, size)` against the local dispatch map (US2,
/// FR-015): memory-resident at the requested size ⇒ [`Avail::Memory`], on the
/// block/disk tier at the requested size ⇒ [`Avail::Disk`], otherwise
/// [`Avail::None`] (absent or size-mismatched — treated as a miss, see
/// `knowledge/size-mismatch-handling.md`).
///
/// Each `lookup` hit pins a read reference; this releases it immediately, since
/// classification does not serve data.
pub(crate) fn classify_query(
    dispatch_map: &Arc<dyn IDispatchMap + Send + Sync>,
    entries: &[(CacheKey, u32)],
) -> Vec<(CacheKey, u32, Avail)> {
    entries
        .iter()
        .map(|&(key, size)| {
            let avail = match dispatch_map.lookup(key) {
                Ok(LookupResult::MemoryTier { size: stored, .. }) => {
                    let _ = dispatch_map.release_read(key);
                    if stored == size {
                        Avail::Memory
                    } else {
                        Avail::None
                    }
                }
                Ok(LookupResult::BlockDevice { .. }) => {
                    let stored = dispatch_map.entry_size(key).ok();
                    let _ = dispatch_map.release_read(key);
                    if stored == Some(size) {
                        Avail::Disk
                    } else {
                        Avail::None
                    }
                }
                // NotExist / MismatchSize / error: no pin taken, nothing held.
                _ => Avail::None,
            };
            (key, size, avail)
        })
        .collect()
}

/// Per-slot outcome of the first classification pass in [`serve_rdma_request`].
enum Resolved {
    /// Memory-resident at the requested size; the read pin is still held.
    Servable,
    /// Not servable; no pin held.
    Failed,
    /// Block-tier resident; awaiting the batched promotion. Only recorded when a
    /// dispatcher receptacle is bound.
    Cold,
}

/// Serve a peer's RDMA_REQUEST (US3, FR-016): for each requested slot, pin the
/// value, delegate the one-sided write to `initiator.push_async`, and map the per-key
/// [`PushStatus`] to an [`RdmaStatusCode`]. `BlockDevice` keys are promoted to the
/// memory tier via `dispatcher.promote_to_memory_tier` and re-looked-up (US4,
/// FR-016/FR-017); a key that is still not memory-resident at the requested size
/// (evicted, size-mismatched, or promotion failed) reports
/// [`RdmaStatusCode::KeyNoLongerAvailable`].
///
/// **Returns as soon as the writes are submitted, not when they land.** `on_done`
/// receives the per-key statuses later, on the initiator's connection thread, and is
/// invoked exactly once however the push turns out. Read pins are owned by that
/// callback and released when it runs (or is dropped), because the NIC keeps reading
/// the pinned buffers after submission returns — releasing them here would be a
/// use-after-free with the NIC as the reader.
///
/// Promotion is **batched across the whole request**: every block-tier key in
/// `slots` goes through a single `promote_to_memory_tier` call, because that
/// method groups keys by target drive and reads them concurrently (one thread and
/// one exclusive SPDK channel per drive). Promoting key-by-key would confine each
/// read to the single drive owning that key and leave the others idle.
///
/// `requester_endpoint`/`rkey` come from the RDMA_REQUEST: the serving peer's
/// initiator connects to the requester's responder and writes into its pool.
/// Responder-side serve counters, shared between the worker thread and the
/// component's `serve_stats()` accessor.
///
/// Relaxed ordering throughout: these are monotonic observability counters read by a
/// polling scraper, so no reader depends on seeing them in step with each other or
/// with any other memory operation.
#[derive(Debug, Default)]
pub(crate) struct ServeCounters {
    pub served: AtomicU64,
    pub cold_promotions: AtomicU64,
    /// Read pins held on behalf of peers right now — a gauge, raised on `adopt` and
    /// lowered when the owning [`PinnedBatch`] drops. The only non-monotonic counter
    /// here, because what matters is the level, not the rate.
    pub pins_held: AtomicU64,
    /// Longest single batch lifetime, in microseconds.
    pub pin_hold_us_max: AtomicU64,
}

impl ServeCounters {
    pub(crate) fn snapshot(&self) -> interfaces::RemoteServeStats {
        interfaces::RemoteServeStats {
            peer_served_keys: self.served.load(Ordering::Relaxed),
            peer_triggered_promotions: self.cold_promotions.load(Ordering::Relaxed),
            peer_pins_held: self.pins_held.load(Ordering::Relaxed),
            peer_pin_hold_us_max: self.pin_hold_us_max.load(Ordering::Relaxed),
        }
    }
}

pub(crate) fn serve_rdma_request(
    dispatch_map: &Arc<dyn IDispatchMap + Send + Sync>,
    dispatcher: Option<&Arc<dyn IDispatcher + Send + Sync>>,
    initiator: &Arc<dyn IRemoteLookupRdmaInitiator + Send + Sync>,
    requester_endpoint: &Endpoint,
    rkey: u32,
    slots: &[SlotDesc],
    counters: &Arc<ServeCounters>,
    on_done: ServeCompletion,
) {
    let mut statuses: Vec<(CacheKey, RdmaStatusCode)> = Vec::with_capacity(slots.len());
    let mut pinned = PinnedBatch::new(Arc::clone(dispatch_map), Arc::clone(counters));
    let mut to_push: Vec<(CacheKey, RemoteRegion)> = Vec::new();

    // Pass 1: classify every slot with no SSD I/O. A memory hit at the requested
    // size keeps its read pin; every other outcome releases it immediately.
    // Block-tier keys are collected for one batched promotion below.
    let mut resolved: Vec<Resolved> = Vec::with_capacity(slots.len());
    let mut cold: Vec<CacheKey> = Vec::new();
    for slot in slots {
        let outcome = match dispatch_map.lookup(slot.key) {
            Ok(LookupResult::MemoryTier { size, .. }) if size == slot.length => Resolved::Servable,
            Ok(LookupResult::MemoryTier { .. }) => {
                let _ = dispatch_map.release_read(slot.key);
                Resolved::Failed
            }
            Ok(LookupResult::BlockDevice { .. }) => {
                let _ = dispatch_map.release_read(slot.key);
                // The size check happens after promotion, matching the memory-tier
                // path: `entry_size` is not consulted here.
                if dispatcher.is_some() {
                    cold.push(slot.key);
                    Resolved::Cold
                } else {
                    Resolved::Failed
                }
            }
            // NotExist / MismatchSize / error: no pin taken, nothing held.
            _ => Resolved::Failed,
        };
        resolved.push(outcome);
    }

    // Pass 2: one promotion for every block-tier key in the request, then
    // re-resolve those slots. Duplicate keys are collapsed so the dispatcher does
    // not evict-and-insert the same key twice within one call.
    if let Some(disp) = dispatcher {
        if !cold.is_empty() {
            cold.sort_unstable();
            cold.dedup();
            disp.promote_to_memory_tier(&cold);
            // The cold-serve signal: keys this node had to go to its OWN disk for in
            // order to answer a peer. Counted here, where the promotion is requested,
            // so it measures I/O undertaken rather than bytes confirmed delivered --
            // a promotion that then fails the re-lookup below still cost the read.
            //
            // Deduplicated already, so one request asking twice for the same key
            // counts once. First-touch by nature: the promotion leaves the entry in
            // DRAM, so the next request for it is served warm and adds nothing.
            counters
                .cold_promotions
                .fetch_add(cold.len() as u64, Ordering::Relaxed);

            for (slot, outcome) in slots.iter().zip(resolved.iter_mut()) {
                if !matches!(outcome, Resolved::Cold) {
                    continue;
                }
                *outcome = match dispatch_map.lookup(slot.key) {
                    Ok(LookupResult::MemoryTier { size, .. }) if size == slot.length => {
                        Resolved::Servable
                    }
                    Ok(LookupResult::MemoryTier { .. }) | Ok(LookupResult::BlockDevice { .. }) => {
                        let _ = dispatch_map.release_read(slot.key);
                        Resolved::Failed
                    }
                    _ => Resolved::Failed,
                };
            }
        }
    }

    // Pass 3: assemble the push batch and failure statuses in slot order.
    for (slot, outcome) in slots.iter().zip(&resolved) {
        match outcome {
            Resolved::Servable => {
                pinned.adopt(slot.key);
                to_push.push((
                    slot.key,
                    RemoteRegion {
                        addr: slot.addr,
                        rkey,
                        length: slot.length,
                    },
                ));
            }
            // `Cold` reaches here only with no dispatcher bound, in which case
            // pass 1 recorded `Failed` instead — folded in for totality.
            Resolved::Failed | Resolved::Cold => {
                statuses.push((slot.key, RdmaStatusCode::KeyNoLongerAvailable));
            }
        }
    }

    // Counted at the same stage as `cold_promotions`, deliberately: both are work
    // this node undertook for a peer, so their ratio is the fraction of peer-served
    // keys that needed a disk read. Counting one at completion and the other at
    // submission would make that ratio quietly wrong whenever a push failed.
    counters
        .served
        .fetch_add(to_push.len() as u64, Ordering::Relaxed);

    if to_push.is_empty() {
        // Nothing to write, so nothing to wait for — and `pinned` is empty.
        on_done(statuses);
        return;
    }

    let endpoint = format!("{}:{}", requester_endpoint.ip, requester_endpoint.port);
    let mut report = ServeReport {
        decided: statuses,
        pushed: to_push.iter().map(|(key, _)| *key).collect(),
        on_done: Some(on_done),
        _pinned: pinned,
    };

    // The callback owns the report, and therefore the pins: running it reports and
    // releases; dropping it (a rejected batch, or teardown) reports
    // unable-to-connect and releases just the same. Either way nothing is leaked and
    // the requester is never left waiting on its deadline.
    let submitted = initiator.push_async(
        &endpoint,
        &to_push,
        Box::new(move |results| report.complete(results)),
    );
    if submitted.is_err() {
        // `push_async` documents that an `Err` drops the callback without invoking
        // it, so `ServeReport::drop` has already reported the batch.
    }
}

/// Map an initiator [`PushStatus`] to the wire [`RdmaStatusCode`] (FR-016):
/// `KeyNotFound`/`SizeMismatch` both fold to `KeyNoLongerAvailable`.
fn map_push_status(status: PushStatus) -> RdmaStatusCode {
    match status {
        PushStatus::Success => RdmaStatusCode::Success,
        PushStatus::UnableToConnect => RdmaStatusCode::UnableToConnect,
        PushStatus::KeyNotFound | PushStatus::SizeMismatch => RdmaStatusCode::KeyNoLongerAvailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seams::{MockDispatchMap, MockDispatcher, MockInitiator, NodeWorld};

    const SIZE: u32 = 4096;

    /// Wire a single node's mocks over one shared world.
    #[allow(clippy::type_complexity)]
    fn node(
        world: &NodeWorld,
    ) -> (
        Arc<dyn IDispatchMap + Send + Sync>,
        Arc<dyn IDispatcher + Send + Sync>,
        Arc<dyn IRemoteLookupRdmaInitiator + Send + Sync>,
    ) {
        (
            Arc::new(MockDispatchMap::new(world.clone())),
            Arc::new(MockDispatcher::new(world.clone())),
            Arc::new(MockInitiator::new(world.clone())),
        )
    }

    fn slots(keys: &[CacheKey]) -> Vec<SlotDesc> {
        keys.iter()
            .enumerate()
            .map(|(i, &key)| SlotDesc {
                key,
                addr: 0x1000 + (i as u64 * SIZE as u64),
                length: SIZE,
            })
            .collect()
    }

    fn endpoint() -> Endpoint {
        Endpoint {
            ip: "127.0.0.1".into(),
            port: 4791,
        }
    }

    /// `peer_pins_held` must return to zero once a batch is released, and that is the
    /// point of the counter: this type's own documentation says a leaked pin makes its
    /// entry permanently unevictable, is indistinguishable from a live reader, and that
    /// **there is no leak detector to catch it**. A gauge that returns to zero between
    /// serves is that detector, so this asserts the property rather than merely that
    /// the number moves.
    ///
    /// The hold time is attributed per batch, not per key. One batch pinning three keys
    /// for one interval blocks eviction for that interval, not three times it, so
    /// multiplying by the key count would overstate the window.
    #[test]
    fn a_released_pin_batch_returns_the_gauge_to_zero_and_records_its_lifetime() {
        let world = NodeWorld::new(1 << 20);
        let (dm, _disp, _init) = node(&world);
        let counters = Arc::new(ServeCounters::default());

        {
            let mut batch = PinnedBatch::new(Arc::clone(&dm), Arc::clone(&counters));
            for key in 0..3u64 {
                batch.adopt(key);
            }
            assert_eq!(
                counters.snapshot().peer_pins_held,
                3,
                "the gauge must show pins while the batch is alive",
            );
            // Long enough that elapsed microseconds are unambiguously non-zero rather
            // than rounding to 0 on a fast machine.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let s = counters.snapshot();
        assert_eq!(
            s.peer_pins_held, 0,
            "a released batch must return the gauge to zero -- a non-zero resting level \
             is exactly the leak this counter exists to surface: {s:?}",
        );
        assert!(
            s.peer_pin_hold_us_max >= 1_000,
            "a batch held ~2 ms must record a max lifetime in that order, got {} us",
            s.peer_pin_hold_us_max,
        );
    }

    /// A serve that finds nothing servable still constructs a batch, and such a batch
    /// blocks no eviction. Recording its lifetime would drag the mean hold time toward
    /// zero and hide the long holds that matter, so an empty batch records nothing.
    #[test]
    fn an_empty_pin_batch_records_no_hold_time() {
        let world = NodeWorld::new(1 << 20);
        let (dm, _disp, _init) = node(&world);
        let counters = Arc::new(ServeCounters::default());

        {
            let _batch = PinnedBatch::new(Arc::clone(&dm), Arc::clone(&counters));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let s = counters.snapshot();
        assert_eq!(
            (s.peer_pins_held, s.peer_pin_hold_us_max),
            (0, 0),
            "an empty batch pinned nothing and blocked nothing, so it must not appear \
             in the hold-time statistics: {s:?}",
        );
    }

    /// Run a serve and return the statuses it reported.
    ///
    /// The mock initiator completes synchronously unless a test stages a serve
    /// delay, so the report has already landed by the time this returns; an
    /// unreported serve is a bug and panics rather than returning an empty result.
    fn serve(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        disp: Option<&Arc<dyn IDispatcher + Send + Sync>>,
        init: &Arc<dyn IRemoteLookupRdmaInitiator + Send + Sync>,
        rkey: u32,
        slots: &[SlotDesc],
    ) -> Vec<(CacheKey, RdmaStatusCode)> {
        serve_counted(dm, disp, init, rkey, slots).0
    }

    /// As `serve`, but also returns the serve counters this request moved.
    ///
    /// Separate rather than folded into `serve` so the existing call sites stay
    /// readable; the counters matter to two tests, not to the twenty that only care
    /// about statuses.
    fn serve_counted(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        disp: Option<&Arc<dyn IDispatcher + Send + Sync>>,
        init: &Arc<dyn IRemoteLookupRdmaInitiator + Send + Sync>,
        rkey: u32,
        slots: &[SlotDesc],
    ) -> (
        Vec<(CacheKey, RdmaStatusCode)>,
        interfaces::RemoteServeStats,
    ) {
        let reported = Arc::new(std::sync::Mutex::new(None));
        let sink = Arc::clone(&reported);
        let counters = Arc::new(ServeCounters::default());
        serve_rdma_request(
            dm,
            disp,
            init,
            &endpoint(),
            rkey,
            slots,
            &counters,
            Box::new(move |statuses| {
                *sink.lock().expect("sink poisoned") = Some(statuses);
            }),
        );
        // Bound to a local so the guard drops before `reported` does.
        let statuses = reported.lock().expect("sink poisoned").take();
        (
            statuses.expect("serve did not report any statuses"),
            counters.snapshot(),
        )
    }

    /// Every disk-resident key in one RDMA_REQUEST must be promoted by a *single*
    /// `promote_to_memory_tier` call. Promoting key-by-key confines each SSD read
    /// to the one drive owning that key, so the dispatcher's per-drive fan-out
    /// never engages.
    #[test]
    fn disk_keys_are_promoted_in_one_batched_call() {
        let world = NodeWorld::new(1 << 20);
        let keys: Vec<CacheKey> = (1..=8).collect();
        for (i, &key) in keys.iter().enumerate() {
            world.with_disk(key, SIZE, i as u64 * SIZE as u64);
        }
        let (dm, disp, init) = node(&world);

        let statuses = serve(&dm, Some(&disp), &init, 0x42, &slots(&keys));

        let calls = world.promote_calls();
        assert_eq!(
            calls.len(),
            1,
            "expected one batched promote, got {calls:?}"
        );
        assert_eq!(calls[0], keys, "batched call must carry every cold key");

        assert_eq!(statuses.len(), keys.len());
        for (key, code) in &statuses {
            assert_eq!(*code, RdmaStatusCode::Success, "key {key} not served");
        }
    }

    /// The cold-serve counter counts the node's OWN disk reads done for a peer.
    ///
    /// This is the instrument that replaced a per-key `REMOTE_SSD` attribution
    /// (withdrawn — see the dispatcher's spec 002): the useful question was never
    /// "which tier did my hit come from" but "how much cold work are peers causing
    /// here", and that is a responder-side aggregate.
    ///
    /// Asserted against `peer_served_keys` in the same request, because the number
    /// that matters is the *fraction*: 8 of 8 served keys needed a disk read here.
    #[test]
    fn cold_serve_counts_this_nodes_own_disk_reads_for_a_peer() {
        let world = NodeWorld::new(1 << 20);
        let keys: Vec<CacheKey> = (1..=8).collect();
        for (i, &key) in keys.iter().enumerate() {
            world.with_disk(key, SIZE, i as u64 * SIZE as u64);
        }
        let (dm, disp, init) = node(&world);

        let (statuses, stats) = serve_counted(&dm, Some(&disp), &init, 0x42, &slots(&keys));

        for (key, code) in &statuses {
            assert_eq!(*code, RdmaStatusCode::Success, "key {key} not served");
        }
        assert_eq!(
            stats.peer_triggered_promotions, 8,
            "every disk-only key served to a peer is one disk read this node did for it"
        );
        assert_eq!(
            stats.peer_served_keys, 8,
            "all eight were served, so the cold fraction is 8/8"
        );
    }

    /// A MIXED batch is what pins the counter to the cold keys specifically.
    ///
    /// Added after mutation testing found the hole: with an all-cold batch,
    /// `cold.len()` and `slots.len()` are the same number, so replacing one with the
    /// other passed both of the neighbouring tests. Only a batch where some keys are
    /// warm and some are cold can tell "keys I read from disk" from "keys I served".
    ///
    /// Nine keys, three of them disk-only: the counter must read 3, not 9.
    #[test]
    fn cold_serve_counts_only_the_cold_keys_in_a_mixed_batch() {
        let world = NodeWorld::new(1 << 20);
        let warm: Vec<CacheKey> = (1..=6).collect();
        let cold: Vec<CacheKey> = (7..=9).collect();
        for &key in &warm {
            world.with_memory(key, SIZE);
        }
        for (i, &key) in cold.iter().enumerate() {
            world.with_disk(key, SIZE, i as u64 * SIZE as u64);
        }
        let all: Vec<CacheKey> = warm.iter().chain(cold.iter()).copied().collect();
        let (dm, disp, init) = node(&world);

        let (statuses, stats) = serve_counted(&dm, Some(&disp), &init, 0x42, &slots(&all));

        for (key, code) in &statuses {
            assert_eq!(*code, RdmaStatusCode::Success, "key {key} not served");
        }
        assert_eq!(
            stats.peer_served_keys, 9,
            "all nine were served to the peer"
        );
        assert_eq!(
            stats.peer_triggered_promotions, 3,
            "only the three disk-only keys cost this node a read; counting all nine \
             would report three times the true cold-serve pressure"
        );
    }

    /// Serving from memory costs this node no disk read, and must count none.
    ///
    /// **This is the test that makes the counter mean anything.** Counting every
    /// served key as cold would satisfy the test above perfectly while measuring
    /// nothing but traffic — the same failure mode that made `certus_lookup_hits`
    /// useless for the remote question.
    #[test]
    fn serving_from_memory_costs_no_disk_read() {
        let world = NodeWorld::new(1 << 20);
        let keys: Vec<CacheKey> = (1..=4).collect();
        for &key in &keys {
            world.with_memory(key, SIZE);
        }
        let (dm, disp, init) = node(&world);

        let (statuses, stats) = serve_counted(&dm, Some(&disp), &init, 0x42, &slots(&keys));

        for (key, code) in &statuses {
            assert_eq!(*code, RdmaStatusCode::Success, "key {key} not served");
        }
        assert_eq!(
            stats.peer_served_keys, 4,
            "the peer was served, so the denominator moves"
        );
        assert_eq!(
            stats.peer_triggered_promotions, 0,
            "a warm serve reads no disk; counting it would make the signal meaningless"
        );
    }

    /// Memory-resident keys need no promotion at all — the batched call must not
    /// fire when nothing is cold.
    #[test]
    fn memory_resident_keys_trigger_no_promote() {
        let world = NodeWorld::new(1 << 20);
        let keys: Vec<CacheKey> = (1..=4).collect();
        for &key in &keys {
            world.with_memory(key, SIZE);
        }
        let (dm, disp, init) = node(&world);

        let statuses = serve(&dm, Some(&disp), &init, 0x42, &slots(&keys));

        assert!(world.promote_calls().is_empty());
        for (_, code) in &statuses {
            assert_eq!(*code, RdmaStatusCode::Success);
        }
    }

    /// A mixed request promotes only the cold keys, still in one call, and serves
    /// the memory-resident ones untouched.
    #[test]
    fn mixed_request_promotes_only_cold_keys_once() {
        let world = NodeWorld::new(1 << 20);
        world.with_memory(1, SIZE).with_memory(3, SIZE);
        world.with_disk(2, SIZE, 0).with_disk(4, SIZE, SIZE as u64);
        let (dm, disp, init) = node(&world);

        let statuses = serve(&dm, Some(&disp), &init, 0x42, &slots(&[1, 2, 3, 4]));

        let calls = world.promote_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], vec![2, 4], "only the disk-resident keys");

        assert_eq!(statuses.len(), 4);
        for (_, code) in &statuses {
            assert_eq!(*code, RdmaStatusCode::Success);
        }
    }

    /// Duplicate keys across slots collapse to one entry in the promote call, so
    /// the dispatcher does not evict-and-reinsert the same key within one batch.
    #[test]
    fn duplicate_cold_keys_are_deduped_in_the_promote_call() {
        let world = NodeWorld::new(1 << 20);
        world.with_disk(7, SIZE, 0);
        let (dm, disp, init) = node(&world);

        let _ = serve(&dm, Some(&disp), &init, 0x42, &slots(&[7, 7, 7]));

        let calls = world.promote_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], vec![7]);
    }

    /// With no dispatcher bound there is nothing to promote with, so disk-only
    /// keys report `KeyNoLongerAvailable` rather than being served.
    #[test]
    fn disk_keys_without_a_dispatcher_are_unavailable() {
        let world = NodeWorld::new(1 << 20);
        world.with_disk(1, SIZE, 0);
        let (dm, _disp, init) = node(&world);

        let statuses = serve(&dm, None, &init, 0x42, &slots(&[1]));

        assert!(world.promote_calls().is_empty());
        assert_eq!(statuses, vec![(1, RdmaStatusCode::KeyNoLongerAvailable)]);
    }

    /// A key whose promotion is scripted to fail stays unavailable, while its
    /// batch-mates are still served — the batched call must not be all-or-nothing.
    #[test]
    fn failed_promotion_does_not_sink_the_rest_of_the_batch() {
        let world = NodeWorld::new(1 << 20);
        world.with_disk(1, SIZE, 0).with_disk(2, SIZE, SIZE as u64);
        world.fail_promote(1);
        let (dm, disp, init) = node(&world);

        let statuses = serve(&dm, Some(&disp), &init, 0x42, &slots(&[1, 2]));

        assert_eq!(world.promote_calls(), vec![vec![1, 2]]);
        let by_key: std::collections::HashMap<_, _> = statuses.into_iter().collect();
        assert_eq!(by_key[&1], RdmaStatusCode::KeyNoLongerAvailable);
        assert_eq!(by_key[&2], RdmaStatusCode::Success);
    }
}
