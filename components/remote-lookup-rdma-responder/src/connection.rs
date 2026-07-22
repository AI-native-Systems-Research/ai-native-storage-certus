//! Inbound connection table, the CM seam, and the per-peer state machine.
//!
//! # The CM seam
//!
//! [`CmListener`] + [`CmConnection`] are the accept-side analog of the
//! initiator's `RdmaTransport`/`RdmaConn`: a seam that lets all
//! hardware-independent logic (the [`ConnectionTable`], its
//! `Active → Draining → Dead` state machine, and telemetry) be unit-tested and
//! benchmarked without an RDMA NIC. [`MockCmSeam`] drives it in tests; the
//! production `rdma_cm` listener (real `bind`/`listen`/`epoll`/`accept`) is a
//! hardware follow-up.
//!
//! # Teardown-before-reclaim
//!
//! [`ConnectionTable::disconnect`] transitions a peer's queue pair into the
//! ERROR state (via [`CmConnection::to_error`]) **before** the caller emits
//! `DisconnectAck`, so late one-sided writes are NAKed and cannot land in slots
//! that are about to be reclaimed. `to_error` is asserted (fail-stop on a fatal
//! HCA fault); destroying the QP is best-effort cleanup performed when the entry
//! is dropped.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use component_core::channel::Receiver;
use interfaces::{PeerId, ResponderCommand};

use crate::telemetry::TelemetryCollector;

/// One accepted inbound connection's queue pair, behind a seam so the table is
/// testable without RDMA hardware. The real implementation wraps an
/// `rdma_cm_id` + RC queue pair and destroys them on `Drop`.
pub trait CmConnection: Send {
    /// Transition the RC queue pair into the ERROR state so late one-sided
    /// writes are NAKed.
    ///
    /// This is the load-bearing safety step of teardown-before-reclaim and is
    /// **asserted**: the transition is always legal from any QP state and fails
    /// only on a fatal HCA/programming fault, so an implementation must
    /// fail-stop (panic) rather than return on failure. It MUST be ordered
    /// before the caller emits `DisconnectAck`.
    fn to_error(&self);
}

/// An event surfaced by a [`CmListener`] to the accept loop.
pub enum CmEvent {
    /// A serving peer is connecting in; `private_data` carries its zyre UUID
    /// (absent/malformed → an unidentified connection).
    ConnectRequest {
        /// Raw connect `private_data` bytes, if any.
        private_data: Option<Vec<u8>>,
        /// The accepted connection's queue pair.
        conn: Box<dyn CmConnection>,
    },
    /// A control command arrived from `remote-lookup`.
    Command(ResponderCommand),
    /// The listener was asked to stop (stop signalled or command channel closed).
    Stop,
}

/// The accept loop's single wait point.
///
/// [`next_events`](CmListener::next_events) blocks until at least one event is
/// available from `{cm channel, command inbox, stop}` and returns a batch. On
/// hardware this is an `epoll` over the `rdma_cm` fd plus the command/stop
/// eventfds; in tests it is [`MockCmSeam`].
pub trait CmListener: Send {
    /// Block until one or more events are ready, then return them.
    fn next_events(&self) -> Vec<CmEvent>;
}

/// Per-peer connection lifecycle. Absence from the table means *disconnected*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// A healthy, accepted connection.
    Active,
    /// Teardown in progress; new connects for this peer are refused.
    Draining,
    /// Torn down; the queue pair is in ERROR and about to be destroyed.
    Dead,
}

struct ConnectionEntry {
    state: ConnState,
    conn: Box<dyn CmConnection>,
}

/// Outcome of [`ConnectionTable::accept`].
#[derive(Debug, PartialEq, Eq)]
pub enum AcceptOutcome {
    /// The connection was accepted; emit `ConnectionEstablished { node }`.
    Established(Option<PeerId>),
    /// The connection was refused because the peer is `Draining` (teardown must
    /// not be raced by new work); no event is emitted.
    Refused,
}

/// A table of inbound connections keyed by [`PeerId`], plus a side-list of
/// unidentified connections (absent/malformed `private_data`) that are
/// reclaimable only via `shutdown`.
pub struct ConnectionTable {
    identified: HashMap<PeerId, ConnectionEntry>,
    unidentified: Vec<ConnectionEntry>,
    telemetry: Arc<TelemetryCollector>,
}

impl ConnectionTable {
    /// Create an empty table recording metrics into `telemetry` (a no-op unless
    /// the `telemetry` feature is on).
    pub fn new(telemetry: Arc<TelemetryCollector>) -> Self {
        Self {
            identified: HashMap::new(),
            unidentified: Vec::new(),
            telemetry,
        }
    }

    /// Accept an inbound connect (FR-005/FR-006/FR-007).
    ///
    /// A valid zyre UUID in `private_data` keys an `Active` entry and yields
    /// `Established(Some(peer))`; absent/malformed data yields
    /// `Established(None)` (unidentified). A connect for a peer currently
    /// `Draining` is [`Refused`](AcceptOutcome::Refused) so teardown is not
    /// raced. A second connect for an already-`Active` peer replaces its queue
    /// pair without corrupting the entry's state (a reconnect).
    pub fn accept(
        &mut self,
        private_data: Option<Vec<u8>>,
        conn: Box<dyn CmConnection>,
    ) -> AcceptOutcome {
        match parse_peer_id(private_data.as_deref()) {
            Some(peer) => {
                if matches!(
                    self.identified.get(&peer),
                    Some(entry) if entry.state == ConnState::Draining
                ) {
                    // Refuse: teardown in progress; drop `conn` (destroys the QP).
                    return AcceptOutcome::Refused;
                }
                self.identified.insert(
                    peer.clone(),
                    ConnectionEntry {
                        state: ConnState::Active,
                        conn,
                    },
                );
                self.telemetry.record_connection_accepted();
                self.telemetry.record_connection_identified();
                AcceptOutcome::Established(Some(peer))
            }
            None => {
                self.unidentified.push(ConnectionEntry {
                    state: ConnState::Active,
                    conn,
                });
                self.telemetry.record_connection_accepted();
                self.telemetry.record_connection_unidentified();
                AcceptOutcome::Established(None)
            }
        }
    }

    /// Tear down the connection to `node` (FR-008), then the caller emits
    /// `DisconnectAck`.
    ///
    /// Transitions `Active → Draining`, drives the queue pair to ERROR (ordered
    /// **before** the ack), then `→ Dead` and drops the entry (destroying the
    /// QP). Idempotent: a `node` with no live connection is a no-op — the caller
    /// still acks (an unconditional guarantee that late writes can no longer
    /// land). Returns `true` if a live connection was torn down.
    pub fn disconnect(&mut self, node: &PeerId) -> bool {
        let torn_down = if let Some(mut entry) = self.identified.remove(node) {
            entry.state = ConnState::Draining;
            // QP → ERROR *before* the ack (asserted inside `to_error`).
            entry.conn.to_error();
            entry.state = ConnState::Dead;
            // `entry` (and its `conn`) dropped here: best-effort QP destroy.
            true
        } else {
            false
        };
        // Every Disconnect yields exactly one DisconnectAck; count each ack.
        self.telemetry.record_teardown();
        torn_down
    }

    /// Tear down every remaining connection (identified and unidentified) — the
    /// listener shutdown path. Each queue pair is driven to ERROR then dropped.
    pub fn teardown_all(&mut self) {
        for (_, mut entry) in self.identified.drain() {
            entry.state = ConnState::Draining;
            entry.conn.to_error();
        }
        for mut entry in self.unidentified.drain(..) {
            entry.state = ConnState::Draining;
            entry.conn.to_error();
        }
    }

    /// Number of identified (`PeerId`-keyed) connections currently tracked.
    pub fn identified_len(&self) -> usize {
        self.identified.len()
    }

    /// Number of unidentified (`node: None`) connections currently tracked.
    pub fn unidentified_len(&self) -> usize {
        self.unidentified.len()
    }

    #[cfg(test)]
    fn state_of(&self, node: &PeerId) -> Option<ConnState> {
        self.identified.get(node).map(|e| e.state)
    }

    #[cfg(test)]
    fn force_state(&mut self, node: &PeerId, state: ConnState) {
        if let Some(e) = self.identified.get_mut(node) {
            e.state = state;
        }
    }
}

/// Parse a zyre UUID from connect `private_data` into a [`PeerId`].
///
/// Returns `None` for absent, empty, non-UTF-8, or blank data.
fn parse_peer_id(private_data: Option<&[u8]>) -> Option<PeerId> {
    let bytes = private_data?;
    let s = std::str::from_utf8(bytes).ok()?;
    let s = s.trim_matches('\0').trim();
    if s.is_empty() {
        None
    } else {
        Some(PeerId::new(s))
    }
}

/// A test/bench CM listener that injects connect events and delivers commands
/// over an event-driven wait (no polling).
///
/// `next_events` returns any injected connects first, otherwise **blocks** on
/// the command inbox — a `send` on the command channel unparks it immediately,
/// so an enqueued `Disconnect` is serviced without waiting a poll cycle and
/// without being stuck behind a pending accept (SC-003). A raised stop flag or a
/// closed command channel yields [`CmEvent::Stop`].
/// A queued inbound connect: its `private_data` and accepted queue pair.
type PendingConnect = (Option<Vec<u8>>, Box<dyn CmConnection>);

pub struct MockCmSeam {
    command_rx: Receiver<ResponderCommand>,
    stop: Arc<AtomicBool>,
    pending: Mutex<VecDeque<PendingConnect>>,
}

impl MockCmSeam {
    /// Create a mock listener over `command_rx` and a shared `stop` flag.
    pub fn new(command_rx: Receiver<ResponderCommand>, stop: Arc<AtomicBool>) -> Self {
        Self {
            command_rx,
            stop,
            pending: Mutex::new(VecDeque::new()),
        }
    }

    /// Queue an inbound connect to be surfaced by the next `next_events` call.
    pub fn inject_connect(&self, private_data: Option<Vec<u8>>, conn: Box<dyn CmConnection>) {
        self.pending
            .lock()
            .expect("pending lock poisoned")
            .push_back((private_data, conn));
    }
}

impl CmListener for MockCmSeam {
    fn next_events(&self) -> Vec<CmEvent> {
        if self.stop.load(Ordering::Acquire) {
            return vec![CmEvent::Stop];
        }
        {
            let mut q = self.pending.lock().expect("pending lock poisoned");
            if !q.is_empty() {
                return q
                    .drain(..)
                    .map(|(private_data, conn)| CmEvent::ConnectRequest { private_data, conn })
                    .collect();
            }
        }
        // Event-driven wait: a send unparks this recv immediately.
        match self.command_rx.recv() {
            Ok(cmd) => vec![CmEvent::Command(cmd)],
            Err(_) => vec![CmEvent::Stop],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::channel::SpscChannel;

    /// A mock connection that logs its `to_error` call into a shared vec so
    /// tests can assert teardown ordering.
    struct MockCmConn {
        log: Arc<Mutex<Vec<String>>>,
        tag: &'static str,
    }

    impl CmConnection for MockCmConn {
        fn to_error(&self) {
            self.log
                .lock()
                .unwrap()
                .push(format!("qp_error:{}", self.tag));
        }
    }

    fn conn(log: &Arc<Mutex<Vec<String>>>, tag: &'static str) -> Box<dyn CmConnection> {
        Box::new(MockCmConn {
            log: Arc::clone(log),
            tag,
        })
    }

    fn table() -> ConnectionTable {
        ConnectionTable::new(Arc::new(TelemetryCollector::new()))
    }

    // --- parse_peer_id (FR-005/FR-006, SC-005) ---

    #[test]
    fn parse_peer_id_variants() {
        assert_eq!(
            parse_peer_id(Some(b"uuid-abc")),
            Some(PeerId::new("uuid-abc"))
        );
        assert_eq!(parse_peer_id(None), None);
        assert_eq!(parse_peer_id(Some(b"")), None);
        assert_eq!(parse_peer_id(Some(&[0xff, 0xfe])), None); // non-UTF8
        assert_eq!(parse_peer_id(Some(b"  ")), None); // blank
    }

    // --- accept / correlate (US2) ---

    #[test]
    fn accept_identified_and_unidentified() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut t = table();

        let out = t.accept(Some(b"peer-1".to_vec()), conn(&log, "a"));
        assert_eq!(out, AcceptOutcome::Established(Some(PeerId::new("peer-1"))));
        assert_eq!(t.identified_len(), 1);

        let out = t.accept(None, conn(&log, "b"));
        assert_eq!(out, AcceptOutcome::Established(None));
        assert_eq!(t.unidentified_len(), 1);
    }

    #[test]
    fn second_connect_for_active_peer_not_corrupted() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut t = table();
        let peer = PeerId::new("peer-1");

        t.accept(Some(b"peer-1".to_vec()), conn(&log, "first"));
        assert_eq!(t.state_of(&peer), Some(ConnState::Active));

        // A second connect for the same Active peer is a reconnect: still Active,
        // still exactly one entry.
        let out = t.accept(Some(b"peer-1".to_vec()), conn(&log, "second"));
        assert_eq!(out, AcceptOutcome::Established(Some(peer.clone())));
        assert_eq!(t.identified_len(), 1);
        assert_eq!(t.state_of(&peer), Some(ConnState::Active));
    }

    // --- teardown before reclaim (US3, SC-002) ---

    #[test]
    fn disconnect_transitions_qp_to_error_before_ack() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut t = table();
        let peer = PeerId::new("peer-1");
        t.accept(Some(b"peer-1".to_vec()), conn(&log, "a"));

        let torn = t.disconnect(&peer);
        assert!(torn);
        // The caller (accept loop) emits the ack after disconnect() returns;
        // model that here and assert QP→ERROR was recorded strictly before it.
        log.lock().unwrap().push("ack".to_string());

        assert_eq!(&*log.lock().unwrap(), &["qp_error:a", "ack"]);
        // Entry is gone (Dead entries are removed).
        assert_eq!(t.state_of(&peer), None);
        assert_eq!(t.identified_len(), 0);
    }

    #[test]
    fn disconnect_unknown_is_idempotent_noop() {
        let mut t = table();
        // No connection for this peer: still a no-op success (caller still acks).
        let torn = t.disconnect(&PeerId::new("ghost"));
        assert!(!torn);
    }

    #[test]
    fn connect_refused_while_draining() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut t = table();
        let peer = PeerId::new("peer-1");
        t.accept(Some(b"peer-1".to_vec()), conn(&log, "a"));

        // Force the entry into Draining (models an in-flight teardown) and assert
        // a new connect for it is refused (FR-007).
        t.force_state(&peer, ConnState::Draining);
        let out = t.accept(Some(b"peer-1".to_vec()), conn(&log, "b"));
        assert_eq!(out, AcceptOutcome::Refused);
        // The existing entry is untouched.
        assert_eq!(t.state_of(&peer), Some(ConnState::Draining));
        assert_eq!(t.identified_len(), 1);
    }

    #[test]
    fn teardown_all_errors_every_qp() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut t = table();
        t.accept(Some(b"peer-1".to_vec()), conn(&log, "id1"));
        t.accept(None, conn(&log, "anon"));

        t.teardown_all();
        assert_eq!(t.identified_len(), 0);
        assert_eq!(t.unidentified_len(), 0);
        let l = log.lock().unwrap();
        assert!(l.contains(&"qp_error:id1".to_string()));
        assert!(l.contains(&"qp_error:anon".to_string()));
    }

    // --- MockCmSeam prompt command servicing (US4, SC-003, structural) ---

    #[test]
    fn seam_services_command_without_pending_connect() {
        let ch = SpscChannel::<ResponderCommand>::new(8);
        let (tx, rx) = ch.split().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let seam = MockCmSeam::new(rx, stop);

        // No connects injected. Enqueue a Disconnect; the seam returns it on an
        // event-driven wake — not behind a connection, not after a poll cycle.
        tx.send(ResponderCommand::Disconnect {
            node: PeerId::new("peer-1"),
        })
        .unwrap();
        let events = seam.next_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            CmEvent::Command(ResponderCommand::Disconnect { .. })
        ));
    }

    #[test]
    fn seam_returns_stop_when_flagged() {
        let ch = SpscChannel::<ResponderCommand>::new(8);
        let (_tx, rx) = ch.split().unwrap();
        let stop = Arc::new(AtomicBool::new(true));
        let seam = MockCmSeam::new(rx, stop);
        assert!(matches!(seam.next_events().as_slice(), [CmEvent::Stop]));
    }

    // --- telemetry wiring (US6; meaningful only with the feature) ---

    #[cfg(feature = "telemetry")]
    #[test]
    fn telemetry_counts_accept_and_teardown() {
        let tm = Arc::new(TelemetryCollector::new());
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut t = ConnectionTable::new(Arc::clone(&tm));

        t.accept(Some(b"peer-1".to_vec()), conn(&log, "a"));
        t.accept(None, conn(&log, "b"));
        t.disconnect(&PeerId::new("peer-1"));
        t.disconnect(&PeerId::new("ghost")); // idempotent, still an ack

        assert_eq!(tm.connections_accepted(), 2);
        assert_eq!(tm.connections_identified(), 1);
        assert_eq!(tm.connections_unidentified(), 1);
        assert_eq!(tm.teardowns(), 2);
    }
}
