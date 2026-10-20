//! The endpoint thread: one actor owns the connection table, and the
//! interface's `&self` methods reach it as [`Command`]s.
//!
//! The table is single-threaded by design (T010), so nothing outside this
//! thread touches it. Methods that need an answer — `stats`, `disconnect`,
//! shutdown — carry a one-shot reply channel and wait on it; `fetch` and
//! `push_async` will return once their command is queued (T017, T033).
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

use std::sync::mpsc;
use std::time::Duration;

use component_core::actor::ActorHandler;
use interfaces::{ILogger, TransferStats};

use crate::connection::table::{ConnectionTable, TableEvent};
use crate::connection::PeerId;

/// What the endpoint thread does for each work request's state. Grows with the
/// serve and fetch paths; nothing posts tracked work yet.
pub(crate) type WorkState = ();

/// How long a drain may go without a completion before it is a stuck device.
/// After ERROR every queued request completes promptly, so this is generous.
pub(crate) const DRAIN_STALL: Duration = Duration::from_secs(5);

/// A request to the endpoint thread.
pub(crate) enum Command {
    Stats(mpsc::Sender<TransferStats>),
    /// Tear `peer` down and drain it; replies once no write from it can land.
    Disconnect(PeerId, mpsc::Sender<Result<(), String>>),
    /// Tear down and drain every peer, then drop the table. The last command.
    Shutdown(mpsc::Sender<Result<(), String>>),
    /// Start connecting to `peer`. Test-only until `fetch`/`push` (T017,
    /// T033) open connections on first need.
    #[cfg(test)]
    Connect(PeerId, mpsc::Sender<Result<(), String>>),
    /// Whether `peer` has an established link. Test-only.
    #[cfg(test)]
    IsUp(PeerId, mpsc::Sender<bool>),
    /// Post one receive on `peer`'s link and keep it held, so a test can tell
    /// a shutdown that drains from one that does not. Test-only.
    #[cfg(test)]
    HoldRecv(PeerId, mpsc::Sender<Result<(), String>>),
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

/// The actor's state: the table, until shutdown takes it.
pub(crate) struct EndpointActor {
    table: Option<ConnectionTable<WorkState>>,
    logger: Box<dyn ILogger + Send + Sync>,
    events: Vec<TableEvent>,
    /// Buffers behind `HoldRecv` receives. Declared after `table`, so they
    /// outlive any drain of it.
    #[cfg(test)]
    held: Vec<(Box<[u8; 64]>, crate::connection::transport::Region)>,
}

impl EndpointActor {
    pub(crate) fn new(
        table: ConnectionTable<WorkState>,
        logger: Box<dyn ILogger + Send + Sync>,
    ) -> Self {
        EndpointActor {
            table: Some(table),
            logger,
            events: Vec::new(),
            #[cfg(test)]
            held: Vec::new(),
        }
    }

    /// Tear `peer` down, drain it, and destroy its links. The FR-017 barrier:
    /// when this returns `Ok`, no write from `peer` can still land.
    fn teardown(table: &mut ConnectionTable<WorkState>, peer: &PeerId) -> Result<(), String> {
        table
            .begin_teardown(peer)
            .map_err(|e| format!("forcing {peer} to ERROR: {e}"))?;
        let mut released = Vec::new();
        table
            .drain(peer, &mut released, DRAIN_STALL)
            .map_err(|e| format!("draining {peer}: {e}"))?;
        // Nothing posts tracked work yet, so there is no state to release; the
        // serve path (T024) will release each pin here.
        table
            .finish_teardown(peer)
            .map_err(|e| format!("destroying {peer}'s links: {e}"))
    }

    /// Process connection events, logging what a caller does not see.
    fn pump(&mut self) -> bool {
        let Some(table) = self.table.as_mut() else {
            return false;
        };
        table.poll(&mut self.events);
        let did = !self.events.is_empty();
        for ev in self.events.drain(..) {
            match ev {
                TableEvent::Down(peer) => {
                    // The peer went away: its link is already in ERROR, so
                    // finish the barrier now rather than leave it draining.
                    if let Err(e) = Self::teardown(table, &peer) {
                        self.logger.error(&format!("certus-transfer-endpoint: {e}"));
                    }
                }
                TableEvent::Failed { peer, reason } => self.logger.warn(&format!(
                    "certus-transfer-endpoint: connect to {peer} failed: {reason:?}"
                )),
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
                    state: (),
                }],
            )
        }
        .map_err(|e| e.to_string())?;
        self.held.push((buf, region));
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let Some(mut table) = self.table.take() else {
            return Ok(());
        };
        let mut failures = Vec::new();
        for peer in table.peers() {
            if let Err(e) = Self::teardown(&mut table, &peer) {
                failures.push(e);
            }
        }
        if failures.is_empty() {
            drop(table);
            return Ok(());
        }
        // Some link still holds state. Destroying it would lose that state, so
        // the table is leaked instead — its queue pairs stay in ERROR, which
        // is what makes late writes impossible — and the failure is reported.
        std::mem::forget(table);
        Err(failures.join("; "))
    }
}

impl ActorHandler<Command> for EndpointActor {
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
                let r = match self.table.as_mut() {
                    Some(t) => Self::teardown(t, &peer),
                    None => Err("endpoint is shut down".into()),
                };
                let _ = reply.send(r);
            }
            Command::Shutdown(reply) => {
                let _ = reply.send(self.shutdown());
            }
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
        });
    }

    fn on_idle(&mut self) -> bool {
        fatal_on_panic("endpoint poll", || self.pump())
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
