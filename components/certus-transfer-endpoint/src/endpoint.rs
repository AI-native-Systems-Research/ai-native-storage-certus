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
use std::time::Duration;

use component_core::actor::ActorHandler;
use interfaces::{
    CacheKey, EntryOutcome, FetchCompletion, ILogger, LandingRegionHandle, RemoteRegion,
    TransferStats,
};

use crate::connection::slab::{TrackedRecv, TrackedSend};
use crate::connection::table::{ConnectionTable, ReapError, TableEvent};
use crate::connection::transport::{
    Access, Completed, Completion, CompletionError, Region, SendOp,
};
use crate::connection::PeerId;
use crate::message::{self, Header, Kind};
use crate::msgpool::{MsgBuf, MsgPool};
use crate::serve::{resolve, Finish, Map, MissCause, Pool, Push, PushWork, Resolved};
use crate::session::{Limits, Phase, Session};

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

/// Settle one push write's completion: release its pin and record its entry's
/// outcome. Returns whether it was a genuine failure (not a flush), which means
/// the link is now in ERROR and must be torn down. Message work is not settled
/// here; see [`EndpointActor::on_message_work`].
fn settle(pushes: &mut BTreeMap<u64, Push>, map: &Map, c: &Completion, work: Work) -> bool {
    match work {
        Work::Send(_) | Work::Recv(_) => unreachable!("message work is settled by the actor"),
        Work::Push(pw) => {
            pw.pin.release(&**map);
            let push = pushes
                .get_mut(&pw.push)
                .expect("a write completed for a push already finished");
            push.in_flight -= 1;
            push.outcomes[pw.index] = Some(if c.result.is_ok() {
                EntryOutcome::HitMemoryTier
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
    /// Pushes in progress, in arrival order.
    pushes: BTreeMap<u64, Push>,
    next_push: u64,
    /// Buffers behind `HoldRecv` receives. Declared after `table`, so they
    /// outlive any drain of it.
    #[cfg(test)]
    held: Vec<(Box<[u8; 64]>, Region)>,
}

impl EndpointActor {
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
            #[cfg(test)]
            held: Vec::new(),
        }
    }

    /// Tear `peer` down, drain it, and destroy its links: the FR-017 barrier.
    /// When this returns `Ok`, no write from `peer` can still land. Every pin
    /// the drain returns is released, and every push to `peer` is finished.
    fn teardown(&mut self, peer: &PeerId) -> Result<(), String> {
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
                    settle(&mut self.pushes, &self.map, &c, work);
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
        Ok(())
    }

    /// End every push to `peer` that can make no further progress: entries
    /// never looked up become `Failed`. Their pins were never taken.
    fn fail_pushes(&mut self, peer: &PeerId) {
        for push in self.pushes.values_mut().filter(|p| p.peer == *peer) {
            for (i, _, _) in push.pending.drain(..) {
                push.outcomes[i] = Some(EntryOutcome::Failed);
            }
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
                    cb(outcomes);
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
            on_complete(vec![EntryOutcome::Failed; items.len()]);
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

    /// Resolve and post as far as credits allow, reap what has completed, and
    /// tear down any link a write failed on. Returns whether anything moved.
    fn advance(&mut self) -> bool {
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
            while queue.len() < avail {
                let Some((index, key, remote)) = push.pending.pop_front() else {
                    break;
                };
                did = true;
                match resolve(&*self.map, self.pool, index, key, remote) {
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
                                pin: w.pin,
                            }),
                        });
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
                        pw.pin.release(&*self.map);
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
                if settle(&mut self.pushes, &self.map, &c, work) {
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

        for (peer, c, work) in messages {
            if let Err(why) = self.on_message_work(&peer, c, work) {
                self.logger.error(&format!(
                    "certus-transfer-endpoint: {peer}: {why}; tearing the connection down"
                ));
                faulted.push(peer);
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
        for peer in faulted {
            if let Err(e) = self.teardown(&peer) {
                self.logger.error(&format!("certus-transfer-endpoint: {e}"));
            }
        }
        self.finish_done();
        did
    }

    // --- messaging (slice B: bring-up) ---------------------------------------

    /// A link just came up: post its receives (reserve + quota) and send our
    /// `OP_HELLO` on the bootstrap credit. Nothing else is sent until the
    /// exchange completes.
    fn bring_up(&mut self, peer: &PeerId) -> Result<(), String> {
        let table = self.table.as_mut().ok_or("shut down")?;
        let receives = self.limits.receives() as usize;
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
    fn on_message_work(&mut self, peer: &PeerId, c: Completion, work: Work) -> Result<(), String> {
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
            (Work::Recv(b), Err(_)) => {
                self.msgs.give(b);
                Err("a receive failed".into())
            }
            (Work::Recv(b), Ok(other)) => {
                self.msgs.give(b);
                Err(format!("a receive completed as {other:?}"))
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

    /// One inbound message. `Err` tears the connection down: the peer broke
    /// the protocol in a way this end cannot answer.
    fn on_message(&mut self, peer: &PeerId, msg: &[u8]) -> Result<(), String> {
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
                        self.queue_reply(peer, |out| message::encode_error(&e, out));
                        Ok(())
                    }
                    None => Err(format!("unprocessable reply: {}", why.what)),
                };
            }
        };
        let conn = self.conns.get_mut(peer).ok_or("no session")?;
        match h.kind {
            Kind::Hello => {
                let hello = message::decode_hello(&h, msg).map_err(|m| m.what.to_string())?;
                let v = conn
                    .session
                    .on_hello(&hello)
                    .map_err(|e| format!("{e:?}"))?;
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
                conn.session.on_hello_reply().map_err(|v| format!("{v:?}"))
            }
            Kind::Error => {
                let e = message::decode_error(&h, msg).map_err(|m| m.what.to_string())?;
                if let Some(t) = self.table.as_mut() {
                    t.counters_mut().protocol_faults += 1;
                }
                conn.session.on_reply().map_err(|v| format!("{v:?}"))?;
                // Nothing of ours is outstanding yet in slice B; slice C fails
                // the named context here.
                self.logger.warn(&format!(
                    "certus-transfer-endpoint: {peer} refused a request: {e:?}"
                ));
                Ok(())
            }
            Kind::FetchReply => Err("a fetch reply, but no fetch was sent".into()),
            Kind::Fetch => {
                if let Err(v) = conn.session.on_request() {
                    return Err(format!("{v:?}"));
                }
                let items = match message::decode_fetch(&h, msg) {
                    Ok(items) => items,
                    Err(why) => {
                        conn.session.replied();
                        let e = message::refusal(msg, why).expect("a request");
                        self.queue_reply(peer, |out| message::encode_error(&e, out));
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
        // Pushes whose peer the table never held (a refused connect) end here.
        let stranded: Vec<PeerId> = self.pushes.values().map(|p| p.peer.clone()).collect();
        for peer in stranded {
            self.fail_pushes(&peer);
        }
        let table = self.table.take().expect("checked above");
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
                let stats = self
                    .table
                    .as_ref()
                    .map(ConnectionTable::stats)
                    .unwrap_or_default();
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
            events || work
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
