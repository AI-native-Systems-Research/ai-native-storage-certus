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
use interfaces::{CacheKey, EntryOutcome, FetchCompletion, ILogger, RemoteRegion, TransferStats};

use crate::connection::slab::TrackedSend;
use crate::connection::table::{ConnectionTable, ReapError, TableEvent};
use crate::connection::transport::{Completion, CompletionError, Region};
use crate::connection::PeerId;
use crate::serve::{resolve, Map, MissCause, Pool, Push, PushWork, Resolved};

/// What a work request's slab slot carries until its completion is reaped.
pub(crate) enum Work {
    /// A push's RDMA write, holding the entry's pin.
    Push(PushWork),
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
    /// Post one send on `peer`'s link that the peer has no receive for, so it
    /// stalls on RNR and everything posted behind it stays in flight. Lets a
    /// test put writes genuinely in flight at teardown. Test-only.
    #[cfg(test)]
    Plug(PeerId, mpsc::Sender<Result<(), String>>),
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

/// Settle one reaped or drained completion: release its pin and record its
/// entry's outcome. Returns whether it was a genuine failure (not a flush),
/// which means the link is now in ERROR and must be torn down.
fn settle(pushes: &mut BTreeMap<u64, Push>, map: &Map, c: &Completion, work: Work) -> bool {
    match work {
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

/// The actor's state: the table, until shutdown takes it.
pub(crate) struct EndpointActor {
    table: Option<ConnectionTable<WorkState>>,
    logger: Box<dyn ILogger + Send + Sync>,
    events: Vec<TableEvent>,
    map: Map,
    pool: Pool,
    /// The memory-tier pool's registration. Declared after `table`, and
    /// released only after it, so every write the pool backs has been drained.
    pool_region: Option<Region>,
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
    ) -> Self {
        EndpointActor {
            pool: Pool {
                addr: pool_region.addr(),
                len: pool_region.len(),
            },
            table: Some(table),
            logger,
            events: Vec::new(),
            map,
            pool_region: Some(pool_region),
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
        // are ours to release whatever happens next.
        for (c, work) in released {
            settle(&mut self.pushes, &self.map, &c, work);
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
            let outcomes = push.finish();
            if let Some(t) = self.table.as_mut() {
                let c = t.counters_mut();
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
        self.pushes
            .insert(id, Push::new(peer.clone(), items, on_complete));
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
            let Some(avail) = table.credits(&push.peer).map(|c| c.available() as usize) else {
                continue; // not connected yet
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

        let busy: Vec<PeerId> = self
            .pushes
            .values()
            .filter(|p| p.in_flight > 0)
            .map(|p| p.peer.clone())
            .collect();
        let mut reaped = Vec::new();
        for peer in busy {
            match table.reap(&peer, &mut reaped, REAP_BATCH) {
                Ok(_) | Err(ReapError::NotLive(_)) => {}
                Err(e) => panic!("unattributable completion from {peer}: {e}"),
            }
            for (c, work) in reaped.drain(..) {
                did = true;
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

        faulted.dedup();
        for peer in faulted {
            if let Err(e) = self.teardown(&peer) {
                self.logger.error(&format!("certus-transfer-endpoint: {e}"));
            }
        }
        self.finish_done();
        did
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
                TableEvent::Up(peer) => self
                    .logger
                    .debug(&format!("certus-transfer-endpoint: connected to {peer}")),
                TableEvent::LostTieBreak(peer) => self.logger.debug(&format!(
                    "certus-transfer-endpoint: simultaneous connect with {peer} resolved"
                )),
            }
        }
        did
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
    fn plug(&mut self, peer: &PeerId) -> Result<(), String> {
        use crate::connection::transport::SendOp;
        let table = self.table.as_mut().ok_or("shut down")?;
        let region = self.pool_region.as_ref().ok_or("shut down")?;
        let mut q = VecDeque::from([TrackedSend {
            op: SendOp::Send,
            local: region.sge(region.addr(), 8).map_err(|e| e.to_string())?,
            state: Work::Held,
        }]);
        // SAFETY: the pool backs the send and outlives every drain.
        unsafe { table.post_sends(peer, &mut q) }.map_err(|e| e.to_string())?;
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
            return Ok(());
        }
        // Some link still holds state. Destroying it would lose that state, so
        // the table — and the pool registration its writes use — is leaked
        // instead; its queue pairs stay in ERROR, which is what makes late
        // writes impossible. The failure is reported.
        std::mem::forget(table);
        std::mem::forget(self.pool_region.take());
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
                let up = self
                    .table
                    .as_ref()
                    .is_some_and(|t| t.credits(&peer).is_some());
                let _ = reply.send(up);
            }
            #[cfg(test)]
            Command::Plug(peer, reply) => {
                let _ = reply.send(self.plug(&peer));
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
