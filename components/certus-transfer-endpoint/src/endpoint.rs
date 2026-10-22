//! The endpoint thread: one actor owns the connection table, and the
//! interface's `&self` methods reach it as [`Command`]s.
//!
//! The table is single-threaded by design (T010), so nothing outside this
//! thread touches it. Methods that need an answer — `stats`, `disconnect`,
//! shutdown — carry a one-shot reply channel and wait on it; `push_async`
//! returns once its command is queued, and its callback fires on this thread.
//!
//! # Pushes
//!
//! Each push is resolved **lazily, as send credits allow**: an entry is looked
//! up (and so pinned) only when a credit is free to post its write at once, so
//! pins held never exceed the credits in flight (SC-004) however long the
//! request, and nothing is pinned while a connection is still being made. Every
//! way a push can end — completion, write failure, peer going down, disconnect,
//! shutdown — releases each pin exactly once (see `serve`).
//!
//! # Panics abort
//!
//! The framework's actor catches a panic in a handler and keeps looping. For
//! this component that is the safety net Principle XI forbids: a panic here
//! means a pin, slab or credit invariant broke, and carrying on with that table
//! risks a silent double release. So every handler path runs under
//! [`fatal_on_panic`], which logs and aborts the process.
//!
//! # Shutdown drains before it drops
//!
//! Dropping a table that still holds state panics (T013), because destroying
//! a queue pair discards its completions and every pin with them. Shutdown
//! therefore tears down and drains every peer first, and only then lets the
//! table go. A peer whose drain stalls is reported and its links are
//! deliberately leaked rather than destroyed with state still held.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use component_core::actor::ActorHandler;
use interfaces::{
    CacheKey, EntryOutcome, FetchCompletion, ILogger, LandingRegionHandle, RemoteRegion,
    TransferEndpointError, TransferStats,
};

use crate::bounce::Readers;
use crate::connection::slab::{TrackedRecv, TrackedSend};
use crate::connection::table::{ConnectionTable, ReapError, TableEvent};
use crate::connection::transport::{
    Access, Completed, Completion, CompletionError, Region, SendOp,
};
use crate::connection::PeerId;
use crate::message::{self, Header, Kind};
use crate::msgpool::{MsgBuf, MsgPool};
use crate::serve::{
    resolve, BlockRead, Finish, Hold, Map, MissCause, Pin, Pool, Push, PushWork, Resolved,
};
use crate::session::{HelloError, Limits, Phase, Session, Violation};

/// What a work request's slab slot carries until its completion is reaped.
pub(crate) enum Work {
    /// A push's RDMA write, holding the entry's pin.
    Push(PushWork),
    /// A message being sent from this buffer.
    Send(MsgBuf),
    /// A receive posted into this buffer.
    Recv(MsgBuf),
    /// A receive a test posted only to hold a slot.
    #[cfg(test)]
    Held,
}

pub(crate) type WorkState = Work;

/// How long a drain may go without a completion before it is a stuck device.
/// After ERROR every queued request completes promptly, so this is generous.
pub(crate) const DRAIN_STALL: Duration = Duration::from_secs(5);

/// Completions reaped per link per poll.
const REAP_BATCH: usize = 64;

/// A request to the endpoint thread.
pub(crate) enum Command {
    Stats(mpsc::Sender<TransferStats>),
    /// Tear `peer` down and drain it; replies once no write from it can land.
    Disconnect(PeerId, mpsc::Sender<Result<(), String>>),
    /// Tear down and drain every peer, then drop the table. The last command.
    Shutdown(mpsc::Sender<Result<(), String>>),
    /// Register `[base, base + len)` for peers to write into (T020). The
    /// address travels as an integer: the precondition on
    /// `register_landing_region` makes it the caller's to keep valid.
    Register {
        base: usize,
        len: usize,
        reply: mpsc::Sender<Result<LandingRegionHandle, String>>,
    },
    /// Ask `peer` for `items`, written into the caller's landing slots, and
    /// report one outcome per item (T033). The id is the caller's.
    Fetch {
        id: u64,
        peer: PeerId,
        items: Vec<(CacheKey, RemoteRegion)>,
        on_complete: FetchCompletion,
    },
    /// The legacy push (T017): write each entry the source holds into the
    /// peer's named region, then report one outcome per entry.
    Push {
        peer: PeerId,
        items: Vec<(CacheKey, RemoteRegion)>,
        on_complete: FetchCompletion,
    },
    /// Start connecting to `peer`. Test-only.
    #[cfg(test)]
    Connect(PeerId, mpsc::Sender<Result<(), String>>),
    /// Whether `peer` has an established link. Test-only.
    #[cfg(test)]
    IsUp(PeerId, mpsc::Sender<bool>),
    /// Post one receive on `peer`'s link and keep it held, so a test can tell
    /// a shutdown that drains from one that does not. Test-only.
    #[cfg(test)]
    HoldRecv(PeerId, mpsc::Sender<Result<(), String>>),
    /// Post no reply reserve on links brought up from now on: FR-026's fault.
    /// Test-only.
    #[cfg(test)]
    NoReplyReserve(mpsc::Sender<()>),
    /// The local pool's address and rkey, so a test can push into it. Test-only.
    #[cfg(test)]
    PoolKey(mpsc::Sender<(u64, u32)>),
    /// Send `bytes` to `peer` as one message, bypassing every protocol rule —
    /// a hostile or buggy peer, for tests. With `spend` it first takes a
    /// message credit, as a genuine request would, so the reply is expected.
    #[cfg(test)]
    Inject {
        peer: PeerId,
        bytes: Vec<u8>,
        spend: bool,
        reply: mpsc::Sender<Result<(), String>>,
    },
    /// `peer`'s session: (negotiated version, peer max_entries, outbound
    /// (now, peak), inbound (now, peak)). Test-only.
    #[cfg(test)]
    SessionOf(PeerId, mpsc::Sender<Option<SessionView>>),
}

thread_local! {
    /// Set on an endpoint thread for its whole life, so a blocking call made
    /// *from* that thread — a completion callback calling `push` — is caught
    /// instead of waiting forever on itself.
    static ON_ENDPOINT_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the caller is running on an endpoint thread.
pub(crate) fn on_endpoint_thread() -> bool {
    ON_ENDPOINT_THREAD.with(|f| f.get())
}

/// Run `f`, aborting the process if it panics. See the module docs.
pub(crate) fn fatal_on_panic<R>(what: &str, f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("<non-string panic>");
            eprintln!(
                "certus-transfer-endpoint: fatal: panic in {what}: {msg}; aborting rather than \
                 continue with a possibly corrupt connection table"
            );
            std::process::abort();
        }
    }
}

/// Why `peer`'s `OP_HELLO` makes a connection impossible, for the fetches it
/// fails with `IncompatiblePeer`.
fn incompatibility(peer: &PeerId, e: &HelloError) -> String {
    match e {
        HelloError::Incompatible { ours, theirs } => format!(
            "{peer} speaks protocol versions {}..={}, this end {}..={}: none in common",
            theirs.0, theirs.1, ours.0, ours.1
        ),
        HelloError::ZeroQuota => format!("{peer} advertised an inbound quota of 0"),
        HelloError::ZeroMaxEntries => format!("{peer} advertised a max_entries of 0"),
        HelloError::Duplicate => unreachable!("a duplicate OP_HELLO is answered, not fatal"),
    }
}

/// Settle one push write's completion: release its pin, or return its bounce
/// buffer, and record its entry's outcome. Returns whether it was a genuine failure (not a flush), which means
/// the link is now in ERROR and must be torn down. Message work is not settled
/// here; see [`EndpointActor::on_message_work`].
fn settle(
    pushes: &mut BTreeMap<u64, Push>,
    map: &Map,
    readers: &mut Option<Readers>,
    counters: &mut crate::telemetry::Counters,
    c: &Completion,
    work: Work,
) -> bool {
    match work {
        Work::Send(_) | Work::Recv(_) => unreachable!("message work is settled by the actor"),
        Work::Push(pw) => {
            let hit = match pw.hold {
                Hold::Pin(pin) => {
                    pin.release(&**map);
                    EntryOutcome::HitMemoryTier
                }
                Hold::Buffer(buf) => {
                    readers
                        .as_mut()
                        .expect("a buffer implies a block source")
                        .release(buf);
                    counters.bounce_buffers_in_use -= 1;
                    EntryOutcome::HitBlockTier
                }
            };
            let push = pushes
                .get_mut(&pw.push)
                .expect("a write completed for a push already finished");
            push.in_flight -= 1;
            // Only a successful completion is a hit: a failed or flushed
            // write may or may not have landed, and is reported as not.
            push.outcomes[pw.index] = Some(if c.result.is_ok() {
                hit
            } else {
                EntryOutcome::Failed
            });
            matches!(c.result, Err(CompletionError::Failed { .. }))
        }
        #[cfg(test)]
        Work::Held => false,
    }
}

/// Count a finished request's outcomes.
fn count_outcomes(c: &mut crate::telemetry::Counters, outcomes: &[EntryOutcome]) {
    c.requests_completed += 1;
    for o in outcomes {
        match o {
            EntryOutcome::HitMemoryTier => c.hit_memory_tier += 1,
            EntryOutcome::HitBlockTier => c.hit_block_tier += 1,
            EntryOutcome::Miss => c.miss += 1,
            EntryOutcome::Failed => c.failed += 1,
        }
    }
}

/// A test's view of a session.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionView {
    pub(crate) version: Option<u16>,
    pub(crate) peer_max_entries: u32,
    pub(crate) outbound: (u32, u32),
    pub(crate) inbound: (u32, u32),
    pub(crate) credits_available: u32,
}

/// One connection's messaging: its session, and messages waiting for a send
/// credit or a buffer.
struct Conn {
    session: Session,
    /// Encoded messages not yet posted, in order. Replies go to the front:
    /// they spend no message credit, and waiting on one starves the peer.
    outbox: VecDeque<(MsgBuf, usize)>,
    /// Our requests on this connection, by the `context_id` we gave them —
    /// the **outbound** book: (fetch id, chunk index). Its ids are ours; the
    /// inbound book is the pushes with `Finish::Reply`, whose ids are the
    /// peer's. The same number can be live in both, and they are unrelated
    /// (FR-026, D22). One id per chunk message: nothing on the wire links the
    /// chunks of one fetch.
    outbound: BTreeMap<u64, (u64, usize)>,
    next_context: u64,
}

/// A fetch in progress: one caller's request, on one connection, split into
/// chunks of the peer's `max_entries` once its `OP_HELLO` says what that is.
struct Fetch {
    peer: PeerId,
    /// Not yet chunked: the exchange has not told us the peer's limit.
    items: Option<Vec<(CacheKey, RemoteRegion)>>,
    chunks: Vec<Chunk>,
    len: usize,
    /// One per item, filled from each chunk's reply at its recorded offset.
    outcomes: Vec<Option<EntryOutcome>>,
    /// The first chunk the peer refused, if any: the fetch then fails as a
    /// whole once every chunk has settled (a prefix is never half-filled).
    refused: Option<String>,
    on_complete: Option<FetchCompletion>,
}

/// One message's worth of a fetch.
struct Chunk {
    /// Where its entries start in the caller's item list.
    offset: usize,
    items: Vec<(CacheKey, RemoteRegion)>,
    state: ChunkState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkState {
    /// Waiting for a message credit.
    Unsent,
    /// On the wire: when it was handed to the transport, and the context id it
    /// went out under. The reply deadline runs from here: a chunk waiting
    /// for a credit or the exchange is queued, not stuck.
    Sent { at: Instant, ctx: u64 },
    /// Answered, or refused.
    Settled,
}

impl Fetch {
    /// Split into chunks of at most `max` entries, in request order — which
    /// is key order for a prefix, so a stall truncates it cleanly.
    fn chunk(&mut self, max: usize) {
        let items = self.items.take().expect("chunked once");
        let max = max.max(1);
        self.chunks = items
            .chunks(max)
            .enumerate()
            .map(|(i, c)| Chunk {
                offset: i * max,
                items: c.to_vec(),
                state: ChunkState::Unsent,
            })
            .collect();
    }

    /// Every chunk answered or refused. An empty fetch is settled at once.
    fn settled(&self) -> bool {
        self.items.is_none() && self.chunks.iter().all(|c| c.state == ChunkState::Settled)
    }

    /// The requested keys' range, for a deadline's diagnostic.
    fn keys(chunk: &Chunk) -> (CacheKey, CacheKey) {
        let ks = chunk.items.iter().map(|(k, _)| *k);
        (ks.clone().min().unwrap_or(0), ks.max().unwrap_or(0))
    }
}

/// Why a whole fetch fails before a usable reply: the `Err` its callback
/// receives. Every slot of the request is to be treated as not landed.
#[derive(Clone)]
enum Abort {
    /// The peer refused it with OP_ERROR.
    Refused(String),
    /// Its connection could not be made, or was torn down.
    Lost(String),
    /// This fetch's own reply deadline expired.
    Deadline(String),
    /// The capability exchange failed, so no request was ever issued.
    Incompatible(String),
}

impl Abort {
    fn error(self) -> TransferEndpointError {
        match self {
            Abort::Refused(m) => TransferEndpointError::Protocol(m),
            Abort::Lost(m) => TransferEndpointError::Transport(m),
            Abort::Deadline(m) => TransferEndpointError::Deadline(m),
            Abort::Incompatible(m) => TransferEndpointError::IncompatiblePeer(m),
        }
    }
}

/// Why a connection is being torn down, for what its fetches are told.
enum Cause {
    /// Lost, disconnected, or faulted: the fetches fail with `Transport`.
    Lost,
    /// The reply deadline of this fetch expired: it fails with `Deadline`,
    /// and its siblings as aborted by it.
    Deadline(u64, String),
    /// The capability exchange failed: every fetch fails `IncompatiblePeer`.
    Incompatible(String),
}

/// Why an inbound message tears its connection down. Every one is counted as
/// a protocol fault.
enum Fault {
    /// The peer broke the protocol in a way this end cannot answer: a bad
    /// reply, a request over quota, a message it could not read.
    Protocol(String),
    /// The peer's `OP_HELLO` makes a connection impossible: no version in
    /// common, or a zero quota or `max_entries`.
    Incompatible(String),
}

impl From<String> for Fault {
    fn from(why: String) -> Self {
        Fault::Protocol(why)
    }
}

impl From<&str> for Fault {
    fn from(why: &str) -> Self {
        Fault::Protocol(why.to_string())
    }
}

/// The actor's state: the table, until shutdown takes it.
pub(crate) struct EndpointActor {
    table: Option<ConnectionTable<WorkState>>,
    limits: Limits,
    /// Per-connection message state, by peer, for live links only.
    conns: BTreeMap<PeerId, Conn>,
    /// Message buffers. Dropped after the table, so after every posted
    /// receive and send into them has been drained.
    msgs: MsgPool,
    logger: Box<dyn ILogger + Send + Sync>,
    events: Vec<TableEvent>,
    map: Map,
    pool: Pool,
    /// The memory-tier pool's registration. Declared after `table`, and
    /// released only after it, so every write the pool backs has been drained.
    pool_region: Option<Region>,
    /// Landing regions, by handle id. Like the pool, declared after `table`
    /// and released only after it — or leaked with it if a drain stalls —
    /// since a peer may be writing into any of them until teardown.
    landing: BTreeMap<u64, Region>,
    next_landing: u64,
    /// Pushes in progress, in arrival order: local `push_async`s and the
    /// serving of peers' `OP_FETCH`s alike.
    pushes: BTreeMap<u64, Push>,
    next_push: u64,
    /// Fetches in progress, by the caller's `TransferContextId`.
    fetches: BTreeMap<u64, Fetch>,
    /// The fetch that last sent a chunk: the next turn starts after it.
    turn: u64,
    /// The block-tier reader threads, when a block source is bound (`spdk`).
    readers: Option<Readers>,
    /// Block-tier reads out, by job id: each entry's pin waits here until its
    /// read returns (FR-011) — the one place a pin is held outside the slab.
    reads: BTreeMap<u64, (u64, BlockRead)>,
    next_read: u64,
    /// The reply deadline, and what to do when it expires (FR-016).
    deadline: Option<Duration>,
    /// Buffers behind `HoldRecv` receives. Declared after `table`, so they
    /// outlive any drain of it.
    #[cfg(test)]
    held: Vec<(Box<[u8; 64]>, Region)>,
    /// Post only the inbound quota, no reply reserve (a fault, for tests).
    #[cfg(test)]
    pub(crate) no_reply_reserve: bool,
}

impl EndpointActor {
    /// Set the reply deadline; `None` disables it.
    pub(crate) fn with_deadline(mut self, deadline: Option<Duration>) -> Self {
        self.deadline = deadline;
        self
    }

    /// Serve block-tier entries through `readers`; without them a block-tier
    /// answer is a miss (D14).
    pub(crate) fn with_readers(mut self, readers: Option<Readers>) -> Self {
        self.readers = readers;
        self
    }

    pub(crate) fn new(
        table: ConnectionTable<WorkState>,
        logger: Box<dyn ILogger + Send + Sync>,
        map: Map,
        pool_region: Region,
        limits: Limits,
    ) -> Self {
        EndpointActor {
            limits,
            conns: BTreeMap::new(),
            msgs: MsgPool::default(),
            pool: Pool {
                addr: pool_region.addr(),
                len: pool_region.len(),
            },
            table: Some(table),
            logger,
            events: Vec::new(),
            map,
            pool_region: Some(pool_region),
            landing: BTreeMap::new(),
            next_landing: 1,
            pushes: BTreeMap::new(),
            next_push: 0,
            fetches: BTreeMap::new(),
            turn: 0,
            readers: None,
            reads: BTreeMap::new(),
            next_read: 0,
            deadline: None,
            #[cfg(test)]
            held: Vec::new(),
            #[cfg(test)]
            no_reply_reserve: false,
        }
    }

    /// Tear `peer` down, drain it, and destroy its links: the FR-017 barrier.
    /// When this returns `Ok`, no write from `peer` can still land. Every pin
    /// the drain returns is released, and every push to `peer` is finished.
    fn teardown(&mut self, peer: &PeerId) -> Result<(), String> {
        self.teardown_for(peer, Cause::Lost)
    }

    /// [`teardown`](Self::teardown), naming why, for what its fetches are
    /// told.
    fn teardown_for(&mut self, peer: &PeerId, cause: Cause) -> Result<(), String> {
        let table = self.table.as_mut().ok_or("endpoint is shut down")?;
        let begun = table
            .begin_teardown(peer)
            .map_err(|e| format!("forcing {peer} to ERROR: {e}"));
        let mut released = Vec::new();
        let drained = table
            .drain(peer, &mut released, DRAIN_STALL)
            .map_err(|e| format!("draining {peer}: {e}"));
        // Settle what came back even if the drain stopped early: those pins
        // and buffers are ours to release whatever happens next.
        for (c, work) in released {
            match work {
                Work::Send(b) | Work::Recv(b) => self.msgs.give(b),
                work => {
                    let counters = table.counters_mut();
                    settle(
                        &mut self.pushes,
                        &self.map,
                        &mut self.readers,
                        counters,
                        &c,
                        work,
                    );
                }
            }
        }
        if let Some(conn) = self.conns.remove(peer) {
            for (b, _) in conn.outbox {
                self.msgs.give(b);
            }
        }
        begun?;
        drained?;
        table
            .finish_teardown(peer)
            .map_err(|e| format!("destroying {peer}'s links: {e}"))?;
        self.fail_pushes(peer);
        // Only now, behind the teardown barrier: every write the peer could
        // make into these fetches' landing slots is impossible (FR-017).
        self.fail_fetches(peer, &cause);
        Ok(())
    }

    /// End every fetch to `peer`: no reply can arrive for it. Called only
    /// once its link is destroyed, so its slots are safe to reclaim, and
    /// failed as `cause` says.
    fn fail_fetches(&mut self, peer: &PeerId, cause: &Cause) {
        let ids: Vec<u64> = self
            .fetches
            .iter()
            .filter(|(_, f)| f.peer == *peer)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let f = self.fetches.remove(&id).expect("listed above");
            let abort = match cause {
                Cause::Deadline(expired, why) if *expired == id => Abort::Deadline(why.clone()),
                Cause::Deadline(expired, _) => Abort::Lost(format!(
                    "aborted: the connection to {peer} was torn down after the reply \
                     deadline of fetch {expired} expired"
                )),
                Cause::Incompatible(why) => Abort::Incompatible(why.clone()),
                Cause::Lost => Abort::Lost(format!("the connection to {peer} was lost")),
            };
            self.abort_fetch(f, abort);
        }
    }

    /// Fire a fetch's callback once, every chunk having been answered, and
    /// count it: its outcomes, or — if the peer refused any chunk — `Protocol`
    /// for the whole fetch, since a fetch with a hole is not a usable prefix.
    fn complete_fetch(&mut self, mut f: Fetch) {
        debug_assert!(f.settled());
        if let Some(why) = f.refused.take() {
            return self.abort_fetch(f, Abort::Refused(why));
        }
        let outcomes: Vec<EntryOutcome> = f
            .outcomes
            .iter()
            .map(|o| o.expect("every chunk was answered"))
            .collect();
        if let Some(t) = self.table.as_mut() {
            count_outcomes(t.counters_mut(), &outcomes);
        }
        if let Some(cb) = f.on_complete.take() {
            cb(Ok(outcomes));
        }
    }

    /// Fire a fetch's callback once with why there is no usable reply. Every
    /// entry counts as failed.
    fn abort_fetch(&mut self, mut f: Fetch, why: Abort) {
        if let Some(t) = self.table.as_mut() {
            count_outcomes(t.counters_mut(), &vec![EntryOutcome::Failed; f.len]);
        }
        if let Some(cb) = f.on_complete.take() {
            cb(Err(why.error()));
        }
    }

    /// Fire the reply deadline of every sent fetch that has waited too long
    /// (T049). Detection is by time, not connection state: a peer that hangs
    /// with its connection open is invisible to the transport. Expiry follows
    /// the T070 order — log, force ERROR, drain, destroy, *then* fail every
    /// fetch on the connection — through the same barrier as `disconnect`.
    fn check_deadlines(&mut self) -> bool {
        let Some(limit) = self.deadline else {
            return false;
        };
        let now = Instant::now();
        let mut expired: BTreeMap<PeerId, (u64, String)> = BTreeMap::new();
        for (&id, f) in &self.fetches {
            if expired.contains_key(&f.peer) {
                continue;
            }
            let late = f
                .chunks
                .iter()
                .enumerate()
                .find_map(|(k, c)| match c.state {
                    ChunkState::Sent { at, ctx } if now.duration_since(at) >= limit => {
                        Some((k, c, ctx))
                    }
                    _ => None,
                });
            let Some((k, chunk, ctx)) = late else {
                continue;
            };
            let qp = self
                .table
                .as_mut()
                .and_then(|t| t.link_state(&f.peer))
                .map_or_else(|| "no link".to_string(), |s| format!("{s:?}"));
            let siblings = self.fetches.values().filter(|g| g.peer == f.peer).count() - 1;
            let (lo, hi) = Fetch::keys(chunk);
            let why = format!(
                "no reply from {peer} within {limit:?}: keys {lo}..={hi} ({n} entries), \
                 chunk {k} of {of} (context {ctx}), queue pair {qp}; tearing the connection \
                 down aborts {siblings} sibling fetch(es)",
                peer = f.peer,
                n = chunk.items.len(),
                of = f.chunks.len(),
            );
            expired.insert(f.peer.clone(), (id, why));
        }
        let did = !expired.is_empty();
        for (peer, (id, why)) in expired {
            self.logger
                .error(&format!("certus-transfer-endpoint: deadline: {why}"));
            if let Some(t) = self.table.as_mut() {
                t.counters_mut().deadline_expiries += 1;
            }
            if let Err(e) = self.teardown_for(&peer, Cause::Deadline(id, why)) {
                self.logger.error(&format!("certus-transfer-endpoint: {e}"));
            }
        }
        did
    }

    /// End every push to `peer` that can make no further progress: entries
    /// never looked up become `Failed`. Their pins were never taken.
    fn fail_pushes(&mut self, peer: &PeerId) {
        let readers = &mut self.readers;
        let counters = self.table.as_mut().map(|t| t.counters_mut());
        let mut freed = 0u32;
        for push in self.pushes.values_mut().filter(|p| p.peer == *peer) {
            // Reads still out finish on their own and are discarded then:
            // their pins and buffers are released when they return.
            push.dead = true;
            for (i, _, _) in push.pending.drain(..).chain(push.deferred.drain(..)) {
                push.outcomes[i] = Some(EntryOutcome::Failed);
            }
            for (i, buf, _) in push.ready.drain(..) {
                readers
                    .as_mut()
                    .expect("a buffer implies a block source")
                    .release(buf);
                freed += 1;
                push.outcomes[i] = Some(EntryOutcome::Failed);
            }
        }
        if let Some(c) = counters {
            c.bounce_buffers_in_use -= freed;
        }
        self.finish_done();
    }

    /// Fire the callback of every push that has finished, and count it.
    fn finish_done(&mut self) {
        let done: Vec<u64> = self
            .pushes
            .iter()
            .filter(|(_, p)| p.is_done())
            .map(|(id, _)| *id)
            .collect();
        for id in done {
            let mut push = self.pushes.remove(&id).expect("listed above");
            let peer = push.peer.clone();
            match push.finish() {
                (outcomes, Some(Finish::Callback(cb))) => {
                    if let Some(t) = self.table.as_mut() {
                        count_outcomes(t.counters_mut(), &outcomes);
                    }
                    // A push is always per entry: hits certainly landed, and
                    // everything else — flushed, refused, never sent — did not.
                    cb(Ok(outcomes));
                }
                (outcomes, Some(Finish::Reply { context_id })) => {
                    // Every write has completed — the ordering guarantee — so
                    // the reply can now say which entries landed (T034). The
                    // peer's request leaves service when the reply is queued.
                    if let Some(conn) = self.conns.get_mut(&peer) {
                        conn.session.replied();
                        let version = conn.session.version().unwrap_or(message::VERSION_MAX);
                        self.queue_reply(&peer, |out| {
                            message::encode_reply(version, context_id, &outcomes, out)
                        });
                    }
                }
                (_, None) => {}
            }
        }
    }

    fn start_push(
        &mut self,
        peer: PeerId,
        items: Vec<(CacheKey, RemoteRegion)>,
        on_complete: FetchCompletion,
    ) {
        let Some(table) = self.table.as_mut() else {
            // Shut down between queueing and handling: nothing was attempted.
            on_complete(Err(TransferEndpointError::NotInitialized(
                "endpoint is shut down".into(),
            )));
            return;
        };
        let c = table.counters_mut();
        c.requests_attempted += 1;
        c.entries_requested += items.len() as u64;
        let ensured = table.ensure(&peer);
        let id = self.next_push;
        self.next_push += 1;
        self.pushes.insert(
            id,
            Push::new(peer.clone(), items, Finish::Callback(on_complete)),
        );
        if let Err(e) = ensured {
            self.logger.error(&format!(
                "certus-transfer-endpoint: push to {peer} cannot connect: {e}"
            ));
            self.fail_pushes(&peer);
        }
    }

    fn start_fetch(
        &mut self,
        id: u64,
        peer: PeerId,
        items: Vec<(CacheKey, RemoteRegion)>,
        on_complete: FetchCompletion,
    ) {
        let len = items.len();
        let fetch = Fetch {
            peer: peer.clone(),
            items: Some(items),
            chunks: Vec::new(),
            len,
            outcomes: vec![None; len],
            refused: None,
            on_complete: Some(on_complete),
        };
        let Some(table) = self.table.as_mut() else {
            return self.abort_fetch(fetch, Abort::Lost("endpoint is shut down".into()));
        };
        let c = table.counters_mut();
        c.requests_attempted += 1;
        c.entries_requested += len as u64;
        if let Err(e) = table.ensure(&peer) {
            self.logger.error(&format!(
                "certus-transfer-endpoint: fetch from {peer} cannot connect: {e}"
            ));
            return self.abort_fetch(fetch, Abort::Lost(format!("cannot connect to {peer}: {e}")));
        }
        self.fetches.insert(id, fetch);
    }

    /// Send unsent chunks while their connections are ready and have message
    /// credits: a fetch is chunked against the peer's `max_entries` once its
    /// `OP_HELLO` is in, and its chunks go out in order, one per credit (FR-015
    /// — waiting, never failing). Fetches to one peer take turns, a chunk each
    /// per round, so one large fetch cannot hold every credit while a small
    /// one waits. Returns whether anything moved.
    fn send_fetches(&mut self) -> bool {
        let mut did = false;
        let mut empty = Vec::new();
        loop {
            let mut sent_this_round = false;
            // Each round starts after the fetch that last sent a chunk, so a
            // freed credit goes to the next fetch in turn, not the oldest.
            use std::ops::Bound::{Excluded, Unbounded};
            let order: Vec<u64> = self
                .fetches
                .range((Excluded(self.turn), Unbounded))
                .chain(self.fetches.range(..=self.turn))
                .map(|(&id, _)| id)
                .collect();
            for id in order {
                let f = self.fetches.get_mut(&id).expect("listed above");
                let Some(conn) = self.conns.get_mut(&f.peer) else {
                    continue;
                };
                if conn.session.phase() != Phase::Ready {
                    continue;
                }
                if f.items.is_some() {
                    f.chunk(conn.session.peer_max_entries() as usize);
                    if f.chunks.is_empty() {
                        empty.push(id);
                        continue;
                    }
                }
                let Some(k) = f.chunks.iter().position(|c| c.state == ChunkState::Unsent) else {
                    continue;
                };
                if !conn.session.try_send_request() {
                    continue; // no credit: wait, never fail (FR-015)
                }
                let Some(table) = self.table.as_mut() else {
                    return did;
                };
                if self.msgs.reserve(table.transport(), 1).is_err() {
                    // Unspent: nothing was sent.
                    let _ = conn.session.on_reply();
                    continue;
                }
                let ctx = conn.next_context;
                conn.next_context += 1;
                conn.outbound.insert(ctx, (id, k));
                let chunk = &mut f.chunks[k];
                chunk.state = ChunkState::Sent {
                    at: Instant::now(),
                    ctx,
                };
                let b = self.msgs.take().expect("reserved");
                let version = conn.session.version().expect("Ready");
                // SAFETY: just taken from the free list.
                let len = message::encode_fetch(version, ctx, &chunk.items, unsafe {
                    self.msgs.bytes_mut(b)
                });
                conn.outbox.push_back((b, len));
                table.counters_mut().chunks_sent += 1;
                self.turn = id;
                sent_this_round = true;
                did = true;
            }
            if !sent_this_round {
                break;
            }
        }
        // Nothing to ask for: answered at once, with no outcomes.
        for id in empty {
            let f = self.fetches.remove(&id).expect("listed above");
            self.complete_fetch(f);
        }
        did
    }

    /// Resolve and post as far as credits allow, reap what has completed, and
    /// tear down any link a write failed on. Returns whether anything moved.
    fn advance(&mut self) -> bool {
        let sent = self.send_fetches();
        let read = self.collect_reads();
        let Some(table) = self.table.as_mut() else {
            return false;
        };
        let region = self
            .pool_region
            .as_ref()
            .expect("pool lives as long as the table");
        let mut did = false;
        let mut faulted: Vec<PeerId> = Vec::new();

        for (&id, push) in self.pushes.iter_mut() {
            // Not connected, or the capability exchange is not finished: no
            // data before the peer's OP_HELLO (FR-025).
            if self
                .conns
                .get(&push.peer)
                .map_or(true, |c| c.session.phase() != Phase::Ready)
            {
                continue;
            }
            let Some(avail) = table.credits(&push.peer).map(|c| c.available() as usize) else {
                continue;
            };
            let mut queue = VecDeque::new();
            // Block-tier entries already read go first: their pins are gone,
            // but their buffers are held until their writes complete.
            while queue.len() < avail {
                let Some((index, buf, remote)) = push.ready.pop_front() else {
                    break;
                };
                did = true;
                let local = region
                    .sge(buf.ptr as u64, buf.len as usize)
                    .expect("a bounce buffer lies inside the registered pool");
                push.in_flight += 1;
                queue.push_back(TrackedSend {
                    op: SendOp::RdmaWrite {
                        remote_addr: remote.addr,
                        rkey: remote.rkey,
                    },
                    local,
                    state: Work::Push(PushWork {
                        push: id,
                        index,
                        hold: Hold::Buffer(buf),
                    }),
                });
            }
            // Deferred entries retry, in order, as readers free — but not while
            // the reserve is known full, which only a buffer coming back ends.
            // Each is looked up afresh: its old pin was released when it was
            // put back.
            let retry = self.readers.as_ref().map_or(0, Readers::can_read);
            let n = retry.min(push.deferred.len());
            for next in push.deferred.drain(..n).rev().collect::<Vec<_>>() {
                push.pending.push_front(next);
            }
            while queue.len() < avail {
                let Some((index, key, remote)) = push.pending.pop_front() else {
                    break;
                };
                did = true;
                let blocks = self.readers.is_some();
                match resolve(&*self.map, self.pool, blocks, index, key, remote) {
                    (Resolved::Failed, cause) => {
                        push.outcomes[index] = Some(EntryOutcome::Failed);
                        self.logger.warn(&format!(
                            "certus-transfer-endpoint: key {key} not served: {cause:?}"
                        ));
                    }
                    (Resolved::Miss, cause) => {
                        push.outcomes[index] = Some(EntryOutcome::Miss);
                        if let Some(c @ MissCause::OutsidePool) = cause {
                            self.logger.warn(&format!(
                                "certus-transfer-endpoint: key {key} not served: {c:?}"
                            ));
                        }
                    }
                    (Resolved::Write(w), _) => {
                        let local = region
                            .sge(w.local, w.len as usize)
                            .expect("resolve checked the entry lies in the pool");
                        push.in_flight += 1;
                        queue.push_back(TrackedSend {
                            op: w.op,
                            local,
                            state: Work::Push(PushWork {
                                push: id,
                                index: w.index,
                                hold: Hold::Pin(w.pin),
                            }),
                        });
                    }
                    (Resolved::Block(b), _) => {
                        let readers = self.readers.as_mut().expect("resolve saw a source");
                        let c = table.counters_mut();
                        if readers.can_read() == 0 {
                            // No reader, or no bounce space, now: hold no pin
                            // while waiting (SC-004). And stop looking up for
                            // this pass, or a long block-tier push would look
                            // up and release every entry each time round.
                            b.pin.release(&*self.map);
                            c.block_deferrals += 1;
                            push.deferred.push_back((b.index, b.key, b.region));
                            break;
                        }
                        let rid = self.next_read;
                        self.next_read += 1;
                        c.block_reads_attempted += 1;
                        let mut b = b;
                        b.frees_at = readers.frees();
                        readers.submit(rid, b.key, b.offset, b.region.length);
                        push.reading += 1;
                        self.reads.insert(rid, (id, b));
                    }
                }
            }
            if queue.is_empty() {
                continue;
            }
            // SAFETY: each write's local buffer is the memory-tier entry its
            // pin keeps resident until the completion releases that pin, and
            // the pool's registration outlives every drain of the table.
            if let Err(e) = unsafe { table.post_sends(&push.peer, &mut queue) } {
                self.logger.error(&format!(
                    "certus-transfer-endpoint: posting to {} failed: {e}",
                    push.peer
                ));
                // The unposted were never queued, so their pins are ours to
                // release here; the queued ones come back through the drain.
                for unposted in queue.drain(..) {
                    #[allow(irrefutable_let_patterns)] // `Held` exists only in tests
                    if let Work::Push(pw) = unposted.state {
                        match pw.hold {
                            Hold::Pin(pin) => pin.release(&*self.map),
                            Hold::Buffer(buf) => {
                                self.readers
                                    .as_mut()
                                    .expect("a buffer implies a block source")
                                    .release(buf);
                                table.counters_mut().bounce_buffers_in_use -= 1;
                            }
                        }
                        push.in_flight -= 1;
                        push.outcomes[pw.index] = Some(EntryOutcome::Failed);
                    }
                }
                faulted.push(push.peer.clone());
            }
        }

        // Every live connection has receives posted, so every one is reaped.
        let busy: Vec<PeerId> = self.conns.keys().cloned().collect();
        let mut reaped = Vec::new();
        let mut messages: Vec<(PeerId, Completion, Work)> = Vec::new();
        for peer in busy {
            match table.reap(&peer, &mut reaped, REAP_BATCH) {
                Ok(_) | Err(ReapError::NotLive(_)) => {}
                Err(e) => panic!("unattributable completion from {peer}: {e}"),
            }
            for (c, work) in reaped.drain(..) {
                did = true;
                if matches!(work, Work::Send(_) | Work::Recv(_)) {
                    messages.push((peer.clone(), c, work));
                    continue;
                }
                let counters = table.counters_mut();
                if settle(
                    &mut self.pushes,
                    &self.map,
                    &mut self.readers,
                    counters,
                    &c,
                    work,
                ) {
                    self.logger.error(&format!(
                        "certus-transfer-endpoint: write to {peer} failed ({}); \
                         tearing the connection down",
                        match c.result {
                            Err(e) => e.status_name(),
                            Ok(_) => "ok",
                        }
                    ));
                    faulted.push(peer.clone());
                }
            }
        }

        let mut incompatible: BTreeMap<PeerId, String> = BTreeMap::new();
        for (peer, c, work) in messages {
            if let Err(fault) = self.on_message_work(&peer, c, work) {
                if let Some(t) = self.table.as_mut() {
                    t.counters_mut().protocol_faults += 1;
                }
                let why = match fault {
                    Fault::Protocol(why) => {
                        faulted.push(peer.clone());
                        why
                    }
                    Fault::Incompatible(why) => {
                        incompatible.insert(peer.clone(), why.clone());
                        why
                    }
                };
                self.logger.error(&format!(
                    "certus-transfer-endpoint: {peer}: {why}; tearing the connection down"
                ));
            }
        }
        for peer in self.conns.keys().cloned().collect::<Vec<_>>() {
            match self.flush_outbox(&peer) {
                Ok(moved) => did |= moved,
                Err(why) => {
                    self.logger.error(&format!(
                        "certus-transfer-endpoint: {peer}: {why}; tearing the connection down"
                    ));
                    faulted.push(peer);
                }
            }
        }

        faulted.sort();
        faulted.dedup();
        faulted.retain(|p| !incompatible.contains_key(p));
        for peer in faulted {
            if let Err(e) = self.teardown(&peer) {
                self.logger.error(&format!("certus-transfer-endpoint: {e}"));
            }
        }
        for (peer, why) in incompatible {
            if let Err(e) = self.teardown_for(&peer, Cause::Incompatible(why)) {
                self.logger.error(&format!("certus-transfer-endpoint: {e}"));
            }
        }
        self.finish_done();
        did || sent || read
    }

    /// Take every finished block-tier read: release its pin — the data has
    /// been copied — and queue its write, or decide its outcome. Returns
    /// whether any came back.
    fn collect_reads(&mut self) -> bool {
        let Some(readers) = self.readers.as_mut() else {
            return false;
        };
        let mut done = Vec::new();
        while let Some(d) = readers.try_collect() {
            done.push(d);
        }
        let any = !done.is_empty();
        for (rid, result) in done {
            self.on_read_done(rid, result);
        }
        any
    }

    /// One block-tier read is back.
    fn on_read_done(
        &mut self,
        rid: u64,
        result: Result<interfaces::BounceBuffer, interfaces::BounceError>,
    ) {
        let (push_id, b) = self.reads.remove(&rid).expect("a read we handed out");
        // FR-011: the pin goes now, whatever the result. On success the data
        // is in the buffer, which protects it from here on.
        Pin::release(b.pin, &*self.map);
        let push = self
            .pushes
            .get_mut(&push_id)
            .expect("a push lives until its reads are back");
        push.reading -= 1;
        let readers = self.readers.as_mut().expect("reads imply readers");
        let Some(table) = self.table.as_mut() else {
            if let Ok(buf) = result {
                readers.release(buf);
            }
            return;
        };
        let c = table.counters_mut();
        match result {
            Ok(buf) if push.dead => {
                // Its connection is gone: nothing will write from it.
                c.block_reads_completed += 1;
                readers.release(buf);
                push.outcomes[b.index] = Some(EntryOutcome::Failed);
            }
            Ok(buf) => {
                c.block_reads_completed += 1;
                c.bounce_buffers_in_use += 1;
                c.bounce_buffers_peak = c.bounce_buffers_peak.max(c.bounce_buffers_in_use);
                push.ready.push_back((b.index, buf, b.region));
            }
            Err(e) if e.is_transient() && !push.dead => {
                // No bounce space now: put it back, holding no pin, to be
                // looked up again when a buffer frees.
                readers.mark_full(b.frees_at);
                c.block_deferrals += 1;
                push.deferred.push_back((b.index, b.key, b.region));
            }
            Err(e) => {
                if matches!(e, interfaces::BounceError::Io(_)) {
                    // The source freed the failed read's buffer: room again.
                    readers.reserve_freed();
                }
                c.block_reads_failed += u64::from(!e.is_transient());
                push.outcomes[b.index] = Some(EntryOutcome::Failed);
                if !e.is_transient() {
                    self.logger.error(&format!(
                        "certus-transfer-endpoint: block-tier key {} not served: {e}",
                        b.key
                    ));
                }
            }
        }
    }

    // --- messaging -------------------------------------------------------------

    /// A link just came up: post its receives (reserve + quota) and send our
    /// `OP_HELLO` on the bootstrap credit. Nothing else is sent until the
    /// exchange completes.
    fn bring_up(&mut self, peer: &PeerId) -> Result<(), String> {
        let table = self.table.as_mut().ok_or("shut down")?;
        let receives = self.limits.receives() as usize;
        // The fault a test asks for: no reply reserve, so replies compete with
        // inbound requests for the quota's buffers (FR-026's deadlock).
        #[cfg(test)]
        let receives = if self.no_reply_reserve {
            self.limits.inbound_quota as usize
        } else {
            receives
        };
        // Receives, plus one buffer to send OP_HELLO from.
        self.msgs.reserve(table.transport(), receives + 1)?;
        let bufs: Vec<MsgBuf> = (0..receives)
            .map(|_| self.msgs.take().expect("reserved"))
            .collect();
        let recvs: Vec<_> = bufs
            .iter()
            .map(|&b| TrackedRecv {
                buf: self.msgs.sge(b, message::MESSAGE_LEN),
                state: Work::Recv(b),
            })
            .collect();
        // SAFETY: each buffer is the pool's, in this receive's slab slot alone
        // until its completion, and the pool outlives every drain.
        if let Err(e) = unsafe { table.post_recvs(peer, recvs) } {
            for w in e.unposted {
                if let Work::Recv(b) = w {
                    self.msgs.give(b);
                }
            }
            return Err(format!("posting receives: {}", e.error));
        }
        let mut session = Session::new(self.limits);
        let hello = session.send_hello();
        let b = self.msgs.take().expect("reserved");
        // SAFETY: just taken from the free list, so in no posted request.
        let len = message::encode_hello(&hello, unsafe { self.msgs.bytes_mut(b) });
        self.conns.insert(
            peer.clone(),
            Conn {
                session,
                outbox: VecDeque::from([(b, len)]),
                outbound: BTreeMap::new(),
                next_context: 0,
            },
        );
        Ok(())
    }

    /// Queue a reply (it spends no message credit). Replies go to the front.
    fn queue_reply(&mut self, peer: &PeerId, encode: impl FnOnce(&mut [u8]) -> usize) {
        let Some(table) = self.table.as_mut() else {
            return;
        };
        if self.msgs.reserve(table.transport(), 1).is_err() {
            self.logger.error(&format!(
                "certus-transfer-endpoint: no buffer to reply to {peer}"
            ));
            return;
        }
        let b = self.msgs.take().expect("reserved");
        // SAFETY: just taken from the free list.
        let len = encode(unsafe { self.msgs.bytes_mut(b) });
        if let Some(conn) = self.conns.get_mut(peer) {
            conn.outbox.push_front((b, len));
        } else {
            self.msgs.give(b);
        }
    }

    /// Post queued messages as far as send credits allow.
    fn flush_outbox(&mut self, peer: &PeerId) -> Result<bool, String> {
        let Some(table) = self.table.as_mut() else {
            return Ok(false);
        };
        let Some(conn) = self.conns.get_mut(peer) else {
            return Ok(false);
        };
        if conn.outbox.is_empty() {
            return Ok(false);
        }
        let mut queue: VecDeque<_> = conn
            .outbox
            .drain(..)
            .map(|(b, len)| TrackedSend {
                op: SendOp::Send,
                local: self.msgs.sge(b, len),
                state: Work::Send(b),
            })
            .collect();
        // SAFETY: each send's buffer is in its slab slot alone until the
        // completion, and the pool outlives every drain.
        let posted = unsafe { table.post_sends(peer, &mut queue) };
        // Whatever was not posted goes back, in order, to the front; each
        // send's local buffer still carries its message's length.
        while let Some(s) = queue.pop_back() {
            let Work::Send(b) = s.state else {
                unreachable!("the outbox holds only sends")
            };
            conn.outbox.push_front((b, s.local.len() as usize));
        }
        match posted {
            Ok(n) => Ok(n > 0),
            Err(e) => Err(format!("posting messages: {e}")),
        }
    }

    /// A message completion: a send finished (its buffer comes back), or a
    /// receive delivered a message (handled, then the buffer re-posted).
    ///
    /// A receive that fails is a teardown, though the queue pair may still be
    /// up (a message longer than the buffer fails only the receive): with the
    /// message unread, this end cannot tell a request it owes a reply from a
    /// reply that returns a credit, so its credit accounting is in doubt.
    fn on_message_work(&mut self, peer: &PeerId, c: Completion, work: Work) -> Result<(), Fault> {
        match (work, c.result) {
            (Work::Send(b), _) => {
                self.msgs.give(b);
                Ok(())
            }
            (Work::Recv(b), Ok(Completed::Recv { byte_len })) => {
                // SAFETY: the receive completed, so the buffer is in no posted
                // request until it is re-posted below.
                let msg = unsafe { self.msgs.read(b, byte_len as usize) };
                let r = self.on_message(peer, &msg);
                self.repost(peer, b);
                r
            }
            (Work::Recv(b), Err(e)) => {
                self.msgs.give(b);
                Err(format!("a receive failed ({})", e.status_name()).into())
            }
            (Work::Recv(b), Ok(other)) => {
                self.msgs.give(b);
                Err(format!("a receive completed as {other:?}").into())
            }
            _ => unreachable!("only message work comes here"),
        }
    }

    fn repost(&mut self, peer: &PeerId, b: MsgBuf) {
        let Some(table) = self.table.as_mut() else {
            self.msgs.give(b);
            return;
        };
        let recv = vec![TrackedRecv {
            buf: self.msgs.sge(b, message::MESSAGE_LEN),
            state: Work::Recv(b),
        }];
        // SAFETY: as in `bring_up`.
        if let Err(e) = unsafe { table.post_recvs(peer, recv) } {
            for w in e.unposted {
                if let Work::Recv(b) = w {
                    self.msgs.give(b);
                }
            }
        }
    }

    /// Answer a request with `OP_ERROR`, in place of its reply, and count it:
    /// a refusal sent is a protocol fault, as one received is.
    fn refuse(&mut self, peer: &PeerId, e: message::ErrorReply) {
        if let Some(t) = self.table.as_mut() {
            t.counters_mut().protocol_faults += 1;
        }
        self.queue_reply(peer, |out| message::encode_error(&e, out));
    }

    /// One inbound message. `Err` tears the connection down, and only when
    /// it must (FR-024): the peer may have written into our slots (a bad
    /// reply), our credit accounting is in doubt (a reply nobody asked for, a
    /// request over quota), or no connection was agreed (a failed capability
    /// exchange). A request this end merely cannot process is answered with
    /// `OP_ERROR` and the connection stays.
    fn on_message(&mut self, peer: &PeerId, msg: &[u8]) -> Result<(), Fault> {
        let version = self
            .conns
            .get(peer)
            .and_then(|c| c.session.version())
            .unwrap_or(0);
        let h = match Header::parse(msg, version) {
            Ok(h) => h,
            Err(why) => {
                // A request is answered; a reply is never answered, and a
                // bad one is a teardown (FR-024).
                return match message::refusal(msg, why) {
                    Some(e) => {
                        self.refuse(peer, e);
                        Ok(())
                    }
                    None => Err(format!("unprocessable reply: {}", why.what).into()),
                };
            }
        };
        let conn = self.conns.get_mut(peer).ok_or("no session")?;
        match h.kind {
            Kind::Hello => {
                let hello = message::decode_hello(&h, msg).map_err(|m| m.what.to_string())?;
                let v = match conn.session.on_hello(&hello) {
                    Ok(v) => v,
                    // The session already agreed stands; the request is
                    // answered, so the peer's credits still balance.
                    Err(HelloError::Duplicate) => {
                        self.refuse(
                            peer,
                            message::ErrorReply {
                                context_id: h.context_id,
                                offending_opcode: Kind::Hello.opcode(),
                                reason: message::Reason::Malformed,
                                max_supported_version: message::VERSION_MAX,
                            },
                        );
                        return Ok(());
                    }
                    Err(e) => return Err(Fault::Incompatible(incompatibility(peer, &e))),
                };
                if v < message::VERSION_MAX {
                    if let Some(t) = self.table.as_mut() {
                        t.counters_mut().version_downgrades += 1;
                    }
                }
                self.queue_reply(peer, message::encode_hello_reply);
                Ok(())
            }
            Kind::HelloReply => {
                message::decode_hello_reply(&h, msg).map_err(|m| m.what.to_string())?;
                conn.session
                    .on_hello_reply()
                    .map_err(|v| format!("{v:?}").into())
            }
            Kind::Error => {
                let e = message::decode_error(&h, msg).map_err(|m| m.what.to_string())?;
                // A refusal of one of our fetches fails that context alone; the
                // peer acted on nothing, so the connection stays (FR-024). It
                // is matched by context **and** kind: only an OP_ERROR whose
                // offending opcode is OP_FETCH answers a fetch of ours. An
                // OP_ERROR naming any other kind answers no request we made
                // — we send no other request but OP_HELLO, answered by
                // OP_HELLO_REPLY — so it is a bad reply and tears down.
                if e.offending_opcode != Kind::Fetch.opcode() {
                    return Err(format!("OP_ERROR for a request we never sent: {e:?}").into());
                }
                let (id, k) = conn
                    .outbound
                    .remove(&e.context_id)
                    .ok_or("OP_ERROR for no fetch of ours")?;
                conn.session.on_reply().map_err(|v| format!("{v:?}"))?;
                // Counted only once it is a refusal of ours: a bad one is
                // counted as the teardown it causes.
                if let Some(t) = self.table.as_mut() {
                    t.counters_mut().protocol_faults += 1;
                }
                self.logger.warn(&format!(
                    "certus-transfer-endpoint: {peer} refused chunk {k} of fetch {id}: {e:?}"
                ));
                if let Some(f) = self.fetches.get_mut(&id) {
                    // The rest of the fetch still settles — its other chunks
                    // may be writing into its slots — and then fails whole.
                    f.chunks[k].state = ChunkState::Settled;
                    f.refused
                        .get_or_insert(format!("{peer} refused chunk {k}: {e:?}"));
                    // Unsent chunks are not worth sending now.
                    for c in &mut f.chunks {
                        if c.state == ChunkState::Unsent {
                            c.state = ChunkState::Settled;
                        }
                    }
                    if f.settled() {
                        let f = self.fetches.remove(&id).expect("just found");
                        self.complete_fetch(f);
                    }
                }
                Ok(())
            }
            Kind::FetchReply => {
                let (id, k) = conn
                    .outbound
                    .remove(&h.context_id)
                    .ok_or("a fetch reply for no request of ours")?;
                let f = self
                    .fetches
                    .get_mut(&id)
                    .ok_or("a reply for a finished fetch")?;
                let want = f.chunks[k].items.len();
                let outcomes = match message::decode_reply(&h, msg) {
                    Ok(o) if o.len() == want => o,
                    // A bad reply is a teardown, and the fetch fails with it —
                    // after the teardown, so its slots are safe to reclaim.
                    Ok(o) => {
                        return Err(format!(
                            "{} outcomes for chunk {k} of fetch {id}, of {want} entries",
                            o.len()
                        )
                        .into())
                    }
                    Err(e) => return Err(e.what.into()),
                };
                conn.session.on_reply().map_err(|v| format!("{v:?}"))?;
                // Every write for a hit completed before this reply was sent
                // (the source's ordering guarantee), so each hit is safe for the
                // caller to commit and each miss or failure to abort (T035).
                // Placed by the chunk's offset: replies to one fetch's chunks
                // may come in any order (FR-013).
                let chunk = &mut f.chunks[k];
                chunk.state = ChunkState::Settled;
                for (i, o) in outcomes.into_iter().enumerate() {
                    f.outcomes[chunk.offset + i] = Some(o);
                }
                if f.settled() {
                    let f = self.fetches.remove(&id).expect("just found");
                    self.complete_fetch(f);
                }
                Ok(())
            }
            Kind::Fetch => {
                match conn.session.on_request() {
                    Ok(()) => {}
                    // Before any version is agreed: answered, since it took no
                    // credit this end counts and its reply lands in the peer's
                    // reserve. (A versioned one fails the header parse.)
                    Err(Violation::BeforeHello) => {
                        self.refuse(
                            peer,
                            message::ErrorReply {
                                context_id: h.context_id,
                                offending_opcode: Kind::Fetch.opcode(),
                                reason: message::Reason::UnsupportedVersion,
                                max_supported_version: message::VERSION_MAX,
                            },
                        );
                        return Ok(());
                    }
                    // Over quota: the peer's credits disagree with ours.
                    Err(v) => return Err(format!("{v:?}").into()),
                }
                let items = match message::decode_fetch(&h, msg) {
                    Ok(items) => items,
                    Err(why) => {
                        conn.session.replied();
                        let e = message::refusal(msg, why).expect("a request");
                        self.refuse(peer, e);
                        return Ok(());
                    }
                };
                // Serve it exactly as a push: the same lookup, pin, write and
                // release (Phase 3); it finishes by replying, once.
                let id = self.next_push;
                self.next_push += 1;
                self.pushes.insert(
                    id,
                    Push::new(
                        peer.clone(),
                        items,
                        Finish::Reply {
                            context_id: h.context_id,
                        },
                    ),
                );
                self.finish_done(); // an empty fetch is answered at once
                Ok(())
            }
        }
    }

    /// Whether `peer`'s capability exchange is done. Test-only.
    #[cfg(test)]
    pub(crate) fn is_ready(&self, peer: &PeerId) -> bool {
        self.conns
            .get(peer)
            .is_some_and(|c| c.session.phase() == Phase::Ready)
    }

    /// Process connection events, logging what a caller does not see.
    fn pump(&mut self) -> bool {
        let Some(table) = self.table.as_mut() else {
            return false;
        };
        table.poll(&mut self.events);
        let events = std::mem::take(&mut self.events);
        let did = !events.is_empty();
        for ev in events {
            match ev {
                TableEvent::Down(peer) => {
                    // The peer went away: its link is already in ERROR, so
                    // finish the barrier now rather than leave it draining.
                    if let Err(e) = self.teardown(&peer) {
                        self.logger.error(&format!("certus-transfer-endpoint: {e}"));
                    }
                }
                TableEvent::Failed { peer, reason } => {
                    self.logger.warn(&format!(
                        "certus-transfer-endpoint: connect to {peer} failed: {reason:?}"
                    ));
                    self.fail_pushes(&peer);
                    self.fail_fetches(&peer, &Cause::Lost);
                }
                TableEvent::RefusedInbound { peer, reason } => self.logger.warn(&format!(
                    "certus-transfer-endpoint: refused inbound connect from {peer:?}: {reason:?}"
                )),
                TableEvent::CreditsClamped {
                    peer,
                    configured,
                    granted,
                } => self.logger.warn(&format!(
                    "certus-transfer-endpoint: send credits for {peer} clamped from \
                     {configured} to the device's {granted}"
                )),
                TableEvent::Up(peer) => {
                    self.logger
                        .debug(&format!("certus-transfer-endpoint: connected to {peer}"));
                    if let Err(e) = self.bring_up(&peer) {
                        self.logger.error(&format!(
                            "certus-transfer-endpoint: bringing up {peer}: {e}"
                        ));
                        if let Err(e) = self.teardown(&peer) {
                            self.logger.error(&format!("certus-transfer-endpoint: {e}"));
                        }
                    }
                }
                TableEvent::LostTieBreak(peer) => self.logger.debug(&format!(
                    "certus-transfer-endpoint: simultaneous connect with {peer} resolved"
                )),
            }
        }
        did
    }

    /// Register a landing region, refusing a range that overlaps the pool or
    /// another landing region: one byte under two keys helps nobody.
    fn register(&mut self, base: usize, len: usize) -> Result<LandingRegionHandle, String> {
        let table = self.table.as_mut().ok_or("endpoint is shut down")?;
        if base == 0 || len == 0 {
            return Err("landing region is null or empty".into());
        }
        let start = base as u64;
        let end = start
            .checked_add(len as u64)
            .ok_or("landing region wraps the address space")?;
        let overlaps = |r: &Region| start < r.addr() + r.len() as u64 && r.addr() < end;
        if self.pool_region.as_ref().is_some_and(overlaps) {
            return Err(format!(
                "landing region 0x{start:x}+{len} overlaps the memory-tier pool"
            ));
        }
        if let Some((id, _)) = self.landing.iter().find(|(_, r)| overlaps(r)) {
            return Err(format!(
                "landing region 0x{start:x}+{len} overlaps landing region {id}"
            ));
        }
        // SAFETY: `register_landing_region`'s documented precondition — the
        // range stays allocated until shutdown returns and is not held as a
        // Rust reference while peers may write it. The region is dropped only
        // after the table, so after every write into it has been drained.
        let region = unsafe {
            table.transport().register(
                base as *mut u8,
                len,
                Access::LOCAL_WRITE | Access::REMOTE_WRITE,
            )
        }
        .map_err(|e| format!("registering the landing region: {e}"))?;
        let id = self.next_landing;
        self.next_landing += 1;
        let handle = LandingRegionHandle::new(id, region.addr(), region.len(), region.rkey());
        self.landing.insert(id, region);
        Ok(handle)
    }

    #[cfg(test)]
    fn hold_recv(&mut self, peer: &PeerId) -> Result<(), String> {
        use crate::connection::slab::TrackedRecv;
        use crate::connection::transport::Access;
        let table = self.table.as_mut().ok_or("shut down")?;
        let mut buf = Box::new([0u8; 64]);
        // SAFETY: `buf` is kept in `self.held`, declared after `table`, so it
        // outlives the region, the posted receive and any drain of it.
        let region = unsafe {
            table
                .transport()
                .register(buf.as_mut_ptr(), 64, Access::LOCAL_WRITE)
        }
        .map_err(|e| e.to_string())?;
        let sge = region.sge(region.addr(), 64).map_err(|e| e.to_string())?;
        // SAFETY: as above.
        unsafe {
            table.post_recvs(
                peer,
                vec![TrackedRecv {
                    buf: sge,
                    state: Work::Held,
                }],
            )
        }
        .map_err(|e| e.to_string())?;
        self.held.push((buf, region));
        Ok(())
    }

    #[cfg(test)]
    fn inject(&mut self, peer: &PeerId, bytes: &[u8], spend: bool) -> Result<(), String> {
        if spend
            && !self
                .conns
                .get_mut(peer)
                .ok_or("no session")?
                .session
                .try_send_request()
        {
            return Err("no message credit".into());
        }
        let table = self.table.as_mut().ok_or("shut down")?;
        self.msgs.reserve(table.transport(), 1)?;
        let b = self.msgs.take().expect("reserved");
        // SAFETY: just taken from the free list, so in no posted request.
        let buf = unsafe { self.msgs.bytes_mut(b) };
        buf[..bytes.len()].copy_from_slice(bytes);
        let conn = self.conns.get_mut(peer).ok_or("no session")?;
        conn.outbox.push_back((b, bytes.len()));
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let Some(peers) = self.table.as_ref().map(ConnectionTable::peers) else {
            return Ok(());
        };
        let mut failures = Vec::new();
        for peer in peers {
            if let Err(e) = self.teardown(&peer) {
                failures.push(e);
            }
        }
        // Wait for every block-tier read still out, however long the device
        // takes: each holds a pin, and a buffer the drive is still writing. Their
        // pushes are dead now, so each buffer is returned as it comes back.
        while let Some(readers) = self.readers.as_mut().filter(|r| r.busy() > 0) {
            let (rid, r) = readers.collect_blocking();
            self.on_read_done(rid, r);
        }
        // Pushes that were waiting only on those reads end now.
        self.finish_done();
        // Pushes and fetches whose peer the table never held (a refused
        // connect) end here.
        let stranded: Vec<PeerId> = self
            .pushes
            .values()
            .map(|p| p.peer.clone())
            .chain(self.fetches.values().map(|f| f.peer.clone()))
            .collect();
        for peer in stranded {
            self.fail_pushes(&peer);
            self.fail_fetches(&peer, &Cause::Lost);
        }
        let table = self.table.take().expect("checked above");
        if let Some(mut readers) = self.readers.take() {
            readers.stop();
        }
        if failures.is_empty() {
            drop(table);
            self.pool_region = None;
            self.landing.clear();
            self.msgs = MsgPool::default();
            return Ok(());
        }
        // Some link still holds state. Destroying it would lose that state, so
        // the table — and every registration its writes use, the pool and the
        // landing regions — is leaked instead; its queue pairs stay in ERROR,
        // which is what makes late writes impossible. The failure is reported.
        std::mem::forget(table);
        std::mem::forget(self.pool_region.take());
        std::mem::forget(std::mem::take(&mut self.landing));
        std::mem::forget(std::mem::take(&mut self.msgs));
        Err(failures.join("; "))
    }
}

impl ActorHandler<Command> for EndpointActor {
    fn on_start(&mut self) {
        ON_ENDPOINT_THREAD.with(|f| f.set(true));
    }

    fn handle(&mut self, cmd: Command) {
        fatal_on_panic("endpoint command", || match cmd {
            Command::Stats(reply) => {
                let mut stats = self
                    .table
                    .as_ref()
                    .map(ConnectionTable::stats)
                    .unwrap_or_default();
                // Read from the sessions now, like the send-queue gauges, so
                // they cannot drift from the message credits they describe.
                for c in self.conns.values() {
                    stats.requests_outstanding_outbound += c.session.outbound().0;
                    stats.requests_in_service_inbound += c.session.inbound().0;
                }
                let _ = reply.send(stats);
            }
            Command::Disconnect(peer, reply) => {
                let _ = reply.send(self.teardown(&peer));
            }
            Command::Shutdown(reply) => {
                let _ = reply.send(self.shutdown());
            }
            Command::Register { base, len, reply } => {
                let _ = reply.send(self.register(base, len));
            }
            Command::Push {
                peer,
                items,
                on_complete,
            } => self.start_push(peer, items, on_complete),
            Command::Fetch {
                id,
                peer,
                items,
                on_complete,
            } => self.start_fetch(id, peer, items, on_complete),
            #[cfg(test)]
            Command::Connect(peer, reply) => {
                let r = match self.table.as_mut() {
                    Some(t) => t.ensure(&peer).map(|_| ()).map_err(|e| e.to_string()),
                    None => Err("endpoint is shut down".into()),
                };
                let _ = reply.send(r);
            }
            #[cfg(test)]
            Command::HoldRecv(peer, reply) => {
                let _ = reply.send(self.hold_recv(&peer));
            }
            #[cfg(test)]
            Command::NoReplyReserve(reply) => {
                self.no_reply_reserve = true;
                let _ = reply.send(());
            }
            #[cfg(test)]
            Command::IsUp(peer, reply) => {
                let _ = reply.send(self.is_ready(&peer));
            }
            #[cfg(test)]
            Command::Inject {
                peer,
                bytes,
                spend,
                reply,
            } => {
                let _ = reply.send(self.inject(&peer, &bytes, spend));
            }
            #[cfg(test)]
            Command::SessionOf(peer, reply) => {
                let _ = reply.send(self.conns.get(&peer).map(|c| SessionView {
                    version: c.session.version(),
                    peer_max_entries: c.session.peer_max_entries(),
                    outbound: c.session.outbound(),
                    inbound: c.session.inbound(),
                    credits_available: c.session.credits_available(),
                }));
            }
            #[cfg(test)]
            Command::PoolKey(reply) => {
                let r = self.pool_region.as_ref().expect("running");
                let _ = reply.send((r.addr(), r.rkey()));
            }
        });
    }

    fn on_idle(&mut self) -> bool {
        fatal_on_panic("endpoint poll", || {
            let events = self.pump();
            let work = self.advance();
            let expired = self.check_deadlines();
            events || work || expired
        })
    }

    fn on_stop(&mut self) {
        // The channel closed without a Shutdown command. Same barrier, nobody
        // to tell but the log. (Not reached by dropping the component: the
        // framework's self-reference keeps it alive, so only `shutdown` or the
        // process exiting stops the thread.)
        fatal_on_panic("endpoint stop", || {
            if let Err(e) = self.shutdown() {
                self.logger.error(&format!(
                    "certus-transfer-endpoint: shutdown on drop left links undrained: {e}"
                ));
            }
        });
    }
}
