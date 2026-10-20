//! The symmetric connection table: at most one live link per peer **instance**,
//! established on first need and reused (FR-002), usable in both directions
//! whichever end initiated it (FR-001).
//!
//! # Simultaneous connect (FR-027, D23)
//!
//! Both ends may initiate at once. Every inbound request names its initiator
//! (see [`super::identity`]), and the table keeps the link whose **initiator
//! has the lower [`PeerId`]**, deciding at accept — before `OP_HELLO`, so the
//! loser carries no context. Both ends evaluate the same rule over the same two
//! names, so they agree without exchanging anything further.
//!
//! On an inbound request from `X`, with `me` the local instance:
//!
//! | local state for `X` | action |
//! |---|---|
//! | nothing | accept |
//! | draining, no live link | reject [`Draining`](RejectReason::Draining) (FR-023) |
//! | our connect pending, `X < me` | close ours, accept theirs |
//! | our connect pending, `me < X` | reject [`Duplicate`](RejectReason::Duplicate) |
//! | live link initiated by `me`, `me < X` | reject `Duplicate` — the race's loser |
//! | live link initiated by `me`, `X < me` | **newer wins**: tear the old link down, accept |
//! | live link initiated by `X` | **newer wins**: tear the old link down, accept |
//!
//! The two "newer wins" rows cannot arise from a race: an end only connects
//! when it holds neither a link nor an attempt, so a request from an end that
//! already owns or out-ranks the live link means that end has lost it —
//! usually a restart, or a teardown whose `Disconnected` has not arrived yet.
//! Rejecting would lock that peer out until the stale link was noticed, and an
//! idle RC link to a dead peer may never be. The old link goes to ERROR first,
//! so FR-017 holds: no write can land through it.
//!
//! **Known gap.** A peer that crashed without its connection being torn down,
//! restarted, and out-ranks nobody (`me < X`, link initiated by `me`) is
//! indistinguishable from the race's loser and is rejected `Duplicate` until
//! this end notices the stale link — at the latest when a request over it hits
//! its deadline (FR-016). A clean process exit tears its connections down, so
//! this needs a node crash.
//!
//! # Teardown
//!
//! A link leaves service only through ERROR. Whether by [`begin_teardown`], a
//! peer's disconnect, or newer-wins, the table forces the link to ERROR, moves
//! it to the peer's draining set and reports [`TableEvent::Down`]. The caller
//! then drains each draining link until every outstanding `wr_id` is
//! accounted for ([`drain`](ConnectionTable::drain)), reclaims, and calls
//! [`finish_teardown`], which refuses while any slab still holds state. While a peer has
//! draining links and no live one, neither end may reconnect: [`ensure`]
//! refuses and an inbound request is rejected `Draining` (FR-023's "fresh
//! connection only after reclaim"). Newer-wins is the one exception — the old
//! link is already in ERROR and the new one carries none of its contexts.
//!
//! [`begin_teardown`]: ConnectionTable::begin_teardown
//! [`finish_teardown`]: ConnectionTable::finish_teardown
//! [`ensure`]: ConnectionTable::ensure

// Consumed by the endpoint from T016 onward.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use super::identity::{
    decode_identity, decode_reject, encode_identity, encode_reject, RejectReason, MAX_IDENTITY_LEN,
};
use super::slab::{drain, DrainError, Slab};
use super::transport::{
    Completion, Link, LinkId, QpCaps, RdmaError, RequestId, Transport, TransportEvent,
};
use super::PeerId;

/// Whether a peer's link is ready to carry work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Established; [`ConnectionTable::live`] returns it. Data messages still
    /// wait for the peer's `OP_HELLO` (T076).
    Ready,
    /// A connect or accept is in flight; [`TableEvent::Up`] follows, or
    /// [`TableEvent::Failed`].
    Connecting,
}

/// Why [`ConnectionTable::ensure`] refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableError {
    /// The local instance's own name: a peer is another instance.
    SelfConnect,
    /// A teardown is in progress; no fresh connection until it finishes.
    Draining(PeerId),
    /// `finish_teardown` while a draining link still holds state.
    Undrained {
        peer: PeerId,
        outstanding: usize,
    },
    Transport(RdmaError),
}

impl fmt::Display for TableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TableError::SelfConnect => write!(f, "cannot connect to the local instance"),
            TableError::Draining(p) => write!(f, "connection to {p} is still being torn down"),
            TableError::Undrained { peer, outstanding } => write!(
                f,
                "{outstanding} work requests to {peer} are still outstanding; drain first"
            ),
            TableError::Transport(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TableError {}

/// Why an outbound connect did not produce a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectFailure {
    /// No listener, unreachable, or the route could not be resolved.
    Unreachable(String),
    /// Refused, with the peer's reason if it gave a certus one.
    Rejected(Option<RejectReason>),
    /// The acceptor answers to a different name than the one dialed. Each
    /// instance must be named by the same string everywhere, or the tie-break
    /// and FR-002's reuse both silently fail (D16 has no name resolution).
    NameMismatch {
        dialed: PeerId,
        answered: Option<PeerId>,
    },
    /// The peer went away before the connection was established.
    Disconnected,
}

/// Something the caller must act on, from [`ConnectionTable::poll`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableEvent {
    /// The peer's link is established.
    Up(PeerId),
    /// The peer's live link was forced to ERROR and is draining. Every context
    /// on it is aborted; drain it, reclaim, then
    /// [`finish_teardown`](ConnectionTable::finish_teardown).
    Down(PeerId),
    /// An outbound connect failed. Nothing is retried; the next
    /// [`ensure`](ConnectionTable::ensure) makes a fresh attempt.
    Failed {
        peer: PeerId,
        reason: ConnectFailure,
    },
    /// Our connect lost the simultaneous-connect tie-break (D23). Not a
    /// failure: the peer's own connect is the survivor, and arrives as `Up`.
    LostTieBreak(PeerId),
    /// An inbound connect was refused. Reported so a refusal is never silent;
    /// `peer` is `None` when the request named nobody parseable.
    RefusedInbound {
        peer: Option<PeerId>,
        reason: RejectReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Initiator {
    Local,
    Peer,
}

struct Live<T> {
    link: LinkId,
    initiator: Initiator,
    up: bool,
    slab: Slab<T>,
}

/// A link in ERROR, with whatever its slab still holds.
struct Draining<T> {
    link: LinkId,
    slab: Slab<T>,
}

struct Entry<T> {
    /// Our outbound attempt, not yet established. It has no slab: nothing can
    /// be posted on a link before it is live (see [`ConnectionTable::live`]).
    pending: Option<LinkId>,
    live: Option<Live<T>>,
    /// In ERROR, awaiting the caller's drain and `finish_teardown`.
    draining: Vec<Draining<T>>,
}

impl<T> Default for Entry<T> {
    fn default() -> Self {
        Entry {
            pending: None,
            live: None,
            draining: Vec::new(),
        }
    }
}

impl<T> Entry<T> {
    fn is_empty(&self) -> bool {
        self.pending.is_none() && self.live.is_none() && self.draining.is_empty()
    }
}

/// One local instance's connections, keyed by peer instance. `T` is the
/// per-work-request state each link's [`Slab`] holds until the request's
/// completion releases it.
pub struct ConnectionTable<T> {
    transport: Box<dyn Transport>,
    local: PeerId,
    identity: Vec<u8>,
    caps: QpCaps,
    entries: HashMap<PeerId, Entry<T>>,
    /// Which peer each live, pending or draining link belongs to.
    owners: HashMap<LinkId, PeerId>,
    scratch: Vec<TransportEvent>,
}

impl<T> ConnectionTable<T> {
    /// Listen as `local` on `transport`. Every link gets `caps`.
    pub fn new(
        mut transport: Box<dyn Transport>,
        local: PeerId,
        caps: QpCaps,
    ) -> Result<Self, TableError> {
        let identity =
            encode_identity(&local).ok_or(TableError::Transport(RdmaError::Invalid {
                op: "ConnectionTable::new",
                reason: "local instance name exceeds the connect private-data limit",
            }))?;
        debug_assert!(identity.len() <= MAX_IDENTITY_LEN + 4);
        transport.listen(&local).map_err(TableError::Transport)?;
        Ok(ConnectionTable {
            transport,
            local,
            identity,
            caps,
            entries: HashMap::new(),
            owners: HashMap::new(),
            scratch: Vec::new(),
        })
    }

    pub fn local(&self) -> &PeerId {
        &self.local
    }

    /// The transport, for registering memory.
    pub fn transport(&mut self) -> &mut dyn Transport {
        &mut *self.transport
    }

    /// Make sure `peer` is connected or connecting, starting a connect only if
    /// there is neither a link nor an attempt.
    pub fn ensure(&mut self, peer: &PeerId) -> Result<Readiness, TableError> {
        if *peer == self.local {
            return Err(TableError::SelfConnect);
        }
        if let Some(e) = self.entries.get(peer) {
            if let Some(live) = &e.live {
                return Ok(if live.up {
                    Readiness::Ready
                } else {
                    Readiness::Connecting
                });
            }
            if e.pending.is_some() {
                return Ok(Readiness::Connecting);
            }
            if !e.draining.is_empty() {
                return Err(TableError::Draining(peer.clone()));
            }
        }
        let link = self
            .transport
            .connect(peer, &self.identity, self.caps)
            .map_err(TableError::Transport)?;
        self.entries.entry(peer.clone()).or_default().pending = Some(link);
        self.owners.insert(link, peer.clone());
        Ok(Readiness::Connecting)
    }

    /// The peer's established link and its slab — the only way to post. Post
    /// through [`post_send_tracked`](super::slab::post_send_tracked) and
    /// [`post_recv_tracked`](super::slab::post_recv_tracked), and release
    /// state only through [`Slab::complete`].
    pub fn live(&mut self, peer: &PeerId) -> Option<(&mut dyn Link, &mut Slab<T>)> {
        let live = self.entries.get_mut(peer)?.live.as_mut().filter(|l| l.up)?;
        let link = self.transport.link(live.link)?;
        Some((link, &mut live.slab))
    }

    /// Whether `peer` has links awaiting drain and `finish_teardown`.
    pub fn is_draining(&self, peer: &PeerId) -> bool {
        self.entries
            .get(peer)
            .is_some_and(|e| !e.draining.is_empty())
    }

    /// Start tearing `peer` down: its live link goes to ERROR and joins the
    /// draining set, and a pending connect is abandoned. Returns whether there
    /// was a live link to drain. No `Down` is reported for a teardown the
    /// caller asked for.
    pub fn begin_teardown(&mut self, peer: &PeerId) -> Result<bool, RdmaError> {
        let Some(e) = self.entries.get_mut(peer) else {
            return Ok(false);
        };
        if let Some(link) = e.pending.take() {
            self.owners.remove(&link);
            self.transport.close(link);
        }
        let Some(live) = self.entries.get_mut(peer).and_then(|e| e.live.take()) else {
            self.forget_if_empty(peer);
            return Ok(false);
        };
        self.retire(peer, live)?;
        Ok(true)
    }

    /// Drain every draining link of `peer`, moving each completion and the
    /// state it releases onto `out` (FR-023's "drain and discard"; discarding
    /// is the caller's, after it has released what each state holds).
    pub fn drain(
        &mut self,
        peer: &PeerId,
        out: &mut Vec<(Completion, T)>,
        stall: Duration,
    ) -> Result<(), DrainError<T>> {
        let Some(e) = self.entries.get_mut(peer) else {
            return Ok(());
        };
        for d in &mut e.draining {
            let link = self
                .transport
                .link(d.link)
                .expect("draining link missing from its transport");
            drain(link, &mut d.slab, out, stall)?;
        }
        Ok(())
    }

    /// Destroy `peer`'s draining links. Refused while any of them still holds
    /// state: destroying a queue pair discards its completions, so every pin
    /// still held would be lost rather than released (Principle IX).
    pub fn finish_teardown(&mut self, peer: &PeerId) -> Result<(), TableError> {
        let Some(e) = self.entries.get_mut(peer) else {
            return Ok(());
        };
        let outstanding: usize = e.draining.iter().map(|d| d.slab.outstanding()).sum();
        if outstanding > 0 {
            return Err(TableError::Undrained {
                peer: peer.clone(),
                outstanding,
            });
        }
        for d in std::mem::take(&mut e.draining) {
            self.owners.remove(&d.link);
            self.transport.close(d.link);
        }
        self.forget_if_empty(peer);
        Ok(())
    }

    /// Process every pending connection event, without blocking.
    pub fn poll(&mut self, out: &mut Vec<TableEvent>) {
        let mut evs = std::mem::take(&mut self.scratch);
        self.transport.poll_events(&mut evs);
        for ev in evs.drain(..) {
            self.handle(ev, out);
        }
        self.scratch = evs;
    }

    /// A slab for `link`, sized to its granted send plus receive depth.
    fn slab_for(&mut self, link: LinkId) -> Slab<T> {
        let caps = self
            .transport
            .link(link)
            .expect("new link missing from its transport")
            .granted();
        Slab::with_capacity(caps.max_send_wr as usize + caps.max_recv_wr as usize)
    }

    /// Force `live` to ERROR and move it to `peer`'s draining set.
    fn retire(&mut self, peer: &PeerId, live: Live<T>) -> Result<(), RdmaError> {
        let result = self
            .transport
            .link(live.link)
            .expect("table link missing from its transport")
            .to_error();
        // Draining whether or not the transition reported success: the link is
        // out of service either way, and the caller must not reuse it.
        self.entries
            .get_mut(peer)
            .expect("retired link has no entry")
            .draining
            .push(Draining {
                link: live.link,
                slab: live.slab,
            });
        result
    }

    fn forget_if_empty(&mut self, peer: &PeerId) {
        if self.entries.get(peer).is_some_and(Entry::is_empty) {
            self.entries.remove(peer);
        }
    }

    /// Retire `peer`'s live link and report it `Down`. A failed ERROR
    /// transition is fatal: FR-017 cannot be honoured without it.
    fn take_down(&mut self, peer: &PeerId, out: &mut Vec<TableEvent>) {
        let Some(live) = self.entries.get_mut(peer).and_then(|e| e.live.take()) else {
            return;
        };
        if let Err(e) = self.retire(peer, live) {
            panic!("cannot force the link to {peer} to ERROR, so late writes stay possible: {e}");
        }
        out.push(TableEvent::Down(peer.clone()));
    }

    fn handle(&mut self, ev: TransportEvent, out: &mut Vec<TableEvent>) {
        match ev {
            TransportEvent::ConnectRequest {
                request,
                private_data,
            } => self.on_request(request, &private_data, out),
            TransportEvent::Established { link, private_data } => {
                self.on_established(link, &private_data, out)
            }
            TransportEvent::Rejected { link, private_data } => {
                let Some(peer) = self.take_pending(link) else {
                    return;
                };
                match decode_reject(&private_data) {
                    Some(RejectReason::Duplicate) => out.push(TableEvent::LostTieBreak(peer)),
                    reason => out.push(TableEvent::Failed {
                        peer,
                        reason: ConnectFailure::Rejected(reason),
                    }),
                }
            }
            TransportEvent::ConnectError { link, reason } => {
                if let Some(peer) = self.take_pending(link) {
                    out.push(TableEvent::Failed {
                        peer,
                        reason: ConnectFailure::Unreachable(reason),
                    });
                }
            }
            TransportEvent::Disconnected { link } => {
                let Some(peer) = self.owners.get(&link).cloned() else {
                    return;
                };
                if self.take_pending(link).is_some() {
                    out.push(TableEvent::Failed {
                        peer,
                        reason: ConnectFailure::Disconnected,
                    });
                } else if self.entries[&peer]
                    .live
                    .as_ref()
                    .is_some_and(|l| l.link == link)
                {
                    self.take_down(&peer, out);
                }
                // A draining link's disconnect changes nothing: it is in ERROR.
            }
        }
    }

    /// If `link` is a pending outbound attempt, abandon it and name its peer.
    /// Safe to close outright: a pending link has no slab, so nothing on it
    /// holds state.
    fn take_pending(&mut self, link: LinkId) -> Option<PeerId> {
        let peer = self.owners.get(&link)?.clone();
        let e = self.entries.get_mut(&peer)?;
        if e.pending != Some(link) {
            return None;
        }
        e.pending = None;
        self.owners.remove(&link);
        self.transport.close(link);
        self.forget_if_empty(&peer);
        Some(peer)
    }

    fn reject(
        &mut self,
        request: RequestId,
        peer: Option<PeerId>,
        reason: RejectReason,
        out: &mut Vec<TableEvent>,
    ) {
        // The initiator may already have abandoned the request (it lost the
        // race and closed its attempt), in which case there is no one to tell.
        let _ = self.transport.reject(request, &encode_reject(reason));
        out.push(TableEvent::RefusedInbound { peer, reason });
    }

    fn on_request(&mut self, request: RequestId, private_data: &[u8], out: &mut Vec<TableEvent>) {
        let peer = match decode_identity(private_data) {
            Some(p) if p != self.local => p,
            claimed => {
                return self.reject(request, claimed, RejectReason::MalformedIdentity, out);
            }
        };
        let peer_wins = peer < self.local;
        let (pending, live, draining) = match self.entries.get(&peer) {
            Some(e) => (
                e.pending,
                e.live.as_ref().map(|l| l.initiator),
                !e.draining.is_empty(),
            ),
            None => (None, None, false),
        };
        match live {
            None if draining => {
                return self.reject(request, Some(peer), RejectReason::Draining, out);
            }
            None if pending.is_some() && !peer_wins => {
                return self.reject(request, Some(peer), RejectReason::Duplicate, out);
            }
            None => {
                if let Some(link) = pending {
                    // We lose the race: our attempt goes, theirs is accepted.
                    self.take_pending(link);
                }
            }
            Some(Initiator::Local) if !peer_wins => {
                return self.reject(request, Some(peer), RejectReason::Duplicate, out);
            }
            // Newer wins: the peer has lost the live link.
            Some(_) => self.take_down(&peer, out),
        }
        match self.transport.accept(request, &self.identity, self.caps) {
            Ok(link) => {
                let slab = self.slab_for(link);
                self.entries.entry(peer.clone()).or_default().live = Some(Live {
                    link,
                    initiator: Initiator::Peer,
                    up: false,
                    slab,
                });
                self.owners.insert(link, peer);
            }
            // The initiator abandoned the request before we answered; nothing
            // was created and its end reports its own outcome.
            Err(_) => self.forget_if_empty(&peer),
        }
    }

    fn on_established(&mut self, link: LinkId, private_data: &[u8], out: &mut Vec<TableEvent>) {
        let Some(peer) = self.owners.get(&link).cloned() else {
            return;
        };
        let e = self
            .entries
            .get_mut(&peer)
            .expect("owned link has no entry");
        if let Some(live) = e.live.as_mut().filter(|l| l.link == link) {
            live.up = true;
            out.push(TableEvent::Up(peer));
            return;
        }
        if e.pending != Some(link) {
            return;
        }
        // Our connect completed: the acceptor must answer to the dialed name.
        let answered = decode_identity(private_data);
        if answered.as_ref() != Some(&peer) {
            self.take_pending(link);
            out.push(TableEvent::Failed {
                peer: peer.clone(),
                reason: ConnectFailure::NameMismatch {
                    dialed: peer,
                    answered,
                },
            });
            return;
        }
        let slab = self.slab_for(link);
        let e = self.entries.get_mut(&peer).unwrap();
        e.pending = None;
        e.live = Some(Live {
            link,
            initiator: Initiator::Local,
            up: true,
            slab,
        });
        out.push(TableEvent::Up(peer));
    }
}

impl<T> Drop for ConnectionTable<T> {
    /// Dropping the table destroys every queue pair, discarding their
    /// completions — so any state still held would never be released. That is
    /// a defect in the owner's shutdown, which must tear down and drain every
    /// peer first, and is reported rather than silently leaked (Principle XI).
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let held: usize = self
            .entries
            .values()
            .flat_map(|e| {
                e.live
                    .iter()
                    .map(|l| l.slab.outstanding())
                    .chain(e.draining.iter().map(|d| d.slab.outstanding()))
            })
            .sum();
        assert_eq!(
            held, 0,
            "connection table dropped with {held} work requests still holding state; \
             tear down and drain every peer first"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::mock::{MockFabric, MockTransport};
    use crate::connection::slab::{post_recv_tracked, post_send_tracked, TrackedRecv, TrackedSend};
    use crate::connection::transport::{Access, Completed, CompletionError, SendOp};

    const CAPS: QpCaps = QpCaps {
        max_send_wr: 8,
        max_recv_wr: 8,
    };

    fn peer(s: &str) -> PeerId {
        PeerId::parse(s).unwrap()
    }

    fn table(fabric: &MockFabric, name: &str) -> ConnectionTable<Tag> {
        ConnectionTable::new(
            Box::new(MockTransport::new(fabric.clone())),
            peer(name),
            CAPS,
        )
        .unwrap()
    }

    fn poll(t: &mut ConnectionTable<Tag>) -> Vec<TableEvent> {
        let mut out = Vec::new();
        t.poll(&mut out);
        out
    }

    /// Poll every table until nothing more happens. A round can process
    /// transport events without producing a table event (an accept does), so
    /// quiescence is two rounds in a row with nothing to report.
    fn settle(ts: &mut [&mut ConnectionTable<Tag>]) -> Vec<Vec<TableEvent>> {
        let mut all = vec![Vec::new(); ts.len()];
        let mut quiet = 0;
        while quiet < 2 {
            let mut any = false;
            for (i, t) in ts.iter_mut().enumerate() {
                let evs = poll(t);
                any |= !evs.is_empty();
                all[i].extend(evs);
            }
            quiet = if any { 0 } else { quiet + 1 };
        }
        all
    }

    /// Test state: what each work request was, so the release can be checked.
    type Tag = &'static str;

    /// Send `msg` from `a` to `b` over their links and check it arrives — so a
    /// test proves the two ends hold *the same* connection. Uses the tracked
    /// path, so every exchange also checks the slab hands each state back.
    fn exchange(a: &mut ConnectionTable<Tag>, b: &mut ConnectionTable<Tag>, msg: &[u8; 4]) {
        let (pa, pb) = (b.local().clone(), a.local().clone());
        let mut src = *msg;
        let mut dst = [0u8; 4];
        // SAFETY: `dst` and `src` outlive their regions and the completions
        // polled below.
        let rd = unsafe {
            b.transport()
                .register(dst.as_mut_ptr(), 4, Access::LOCAL_WRITE)
        }
        .unwrap();
        let rs = unsafe {
            a.transport()
                .register(src.as_mut_ptr(), 4, Access::LOCAL_WRITE)
        }
        .unwrap();
        {
            let (link, slab) = b.live(&pb).expect("b has a link to a");
            // SAFETY: as above.
            unsafe {
                post_recv_tracked(
                    link,
                    slab,
                    vec![TrackedRecv {
                        buf: rd.sge(rd.addr(), 4).unwrap(),
                        state: "recv",
                    }],
                )
            }
            .unwrap();
        }
        {
            let (link, slab) = a.live(&pa).expect("a has a link to b");
            // SAFETY: as above.
            unsafe {
                post_send_tracked(
                    link,
                    slab,
                    vec![TrackedSend {
                        op: SendOp::Send,
                        local: rs.sge(rs.addr(), 4).unwrap(),
                        state: "send",
                    }],
                )
            }
            .unwrap();
        }
        assert_eq!(
            reap(b, &pb),
            [(Ok(Completed::Recv { byte_len: 4 }), "recv")]
        );
        assert_eq!(reap(a, &pa), [(Ok(Completed::Send), "send")]);
        drop((rs, rd));
        assert_eq!(&dst, msg);
    }

    /// Poll `peer`'s live link and release each completion through its slab.
    fn reap(
        t: &mut ConnectionTable<Tag>,
        peer: &PeerId,
    ) -> Vec<(Result<Completed, CompletionError>, Tag)> {
        let (link, slab) = t.live(peer).unwrap();
        let mut c = Vec::new();
        link.poll(&mut c, 64).unwrap();
        c.iter()
            .map(|c| (c.result, slab.complete(c).unwrap()))
            .collect()
    }

    #[test]
    fn connect_establishes_both_ends_and_either_can_post() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut b = table(&f, "10.0.0.2:7000");
        let (pa, pb) = (a.local().clone(), b.local().clone());
        assert_eq!(a.ensure(&pb), Ok(Readiness::Connecting));
        let evs = settle(&mut [&mut a, &mut b]);
        assert_eq!(evs[0], [TableEvent::Up(pb.clone())]);
        assert_eq!(evs[1], [TableEvent::Up(pa.clone())]);
        // FR-001: the accepted end posts as freely as the initiating one.
        exchange(&mut a, &mut b, b"a->b");
        exchange(&mut b, &mut a, b"b->a");
    }

    #[test]
    fn ensure_reuses_the_link_and_inbound_counts_as_ours() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut b = table(&f, "10.0.0.2:7000");
        let (pa, pb) = (a.local().clone(), b.local().clone());
        a.ensure(&pb).unwrap();
        assert_eq!(
            a.ensure(&pb),
            Ok(Readiness::Connecting),
            "no second attempt"
        );
        settle(&mut [&mut a, &mut b]);
        assert_eq!(a.ensure(&pb), Ok(Readiness::Ready));
        // FR-002: b reuses the link a opened rather than dialing its own.
        assert_eq!(b.ensure(&pa), Ok(Readiness::Ready));
        assert_eq!(settle(&mut [&mut a, &mut b]), [vec![], vec![]]);
        assert_eq!(f.qp_count(), 2, "one queue pair at each end");
    }

    /// SC-009: NUMA siblings — same host, different ports — are separate peers
    /// with separate connections.
    #[test]
    fn same_host_siblings_get_separate_connections() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut s0 = table(&f, "10.0.0.2:7000");
        let mut s1 = table(&f, "10.0.0.2:7001");
        a.ensure(s0.local()).unwrap();
        a.ensure(&s1.local().clone()).unwrap();
        settle(&mut [&mut a, &mut s0, &mut s1]);
        exchange(&mut a, &mut s0, b"to-0");
        exchange(&mut a, &mut s1, b"to-1");
        assert_eq!(f.qp_count(), 4);
    }

    /// SC-015: both ends connect at once, under every poll order; exactly one
    /// connection survives and both ends hold the same one.
    #[test]
    fn simultaneous_connect_converges_on_one_link() {
        for order in 0..4 {
            let f = MockFabric::new();
            let mut lo = table(&f, "10.0.0.1:7000");
            let mut hi = table(&f, "10.0.0.2:7000");
            let (plo, phi) = (lo.local().clone(), hi.local().clone());
            lo.ensure(&phi).unwrap();
            hi.ensure(&plo).unwrap();
            let (mut elo, mut ehi) = (Vec::new(), Vec::new());
            match order {
                0 => {
                    elo.extend(poll(&mut lo));
                    ehi.extend(poll(&mut hi));
                }
                1 => {
                    ehi.extend(poll(&mut hi));
                    elo.extend(poll(&mut lo));
                }
                2 => {
                    ehi.extend(poll(&mut hi));
                    ehi.extend(poll(&mut hi));
                }
                _ => {
                    elo.extend(poll(&mut lo));
                    elo.extend(poll(&mut lo));
                }
            }
            let rest = settle(&mut [&mut lo, &mut hi]);
            elo.extend(rest[0].iter().cloned());
            ehi.extend(rest[1].iter().cloned());

            assert_eq!(lo.ensure(&phi), Ok(Readiness::Ready), "order {order}");
            assert_eq!(hi.ensure(&plo), Ok(Readiness::Ready), "order {order}");
            assert_eq!(f.qp_count(), 2, "order {order}: exactly one connection");
            assert_eq!(
                elo.iter()
                    .filter(|e| matches!(e, TableEvent::Up(_)))
                    .count(),
                1
            );
            assert_eq!(
                ehi.iter()
                    .filter(|e| matches!(e, TableEvent::Up(_)))
                    .count(),
                1
            );
            assert!(
                !elo.iter()
                    .chain(&ehi)
                    .any(|e| matches!(e, TableEvent::Failed { .. })),
                "order {order}: a race is not a failure: {elo:?} {ehi:?}"
            );
            exchange(&mut lo, &mut hi, b"same");
            exchange(&mut hi, &mut lo, b"link");
        }
    }

    /// A raw transport speaking the identity format, for requests a table
    /// would never send.
    fn raw(f: &MockFabric) -> MockTransport {
        MockTransport::new(f.clone())
    }

    #[test]
    fn peer_reconnecting_over_its_own_link_wins_and_old_link_is_retired_first() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.2:7000");
        let pb = peer("10.0.0.1:7000");
        // "b" connects; a accepts. Then b — having lost its end — connects again.
        let mut b1 = raw(&f);
        let id = encode_identity(&pb).unwrap();
        b1.connect(a.local(), &id, CAPS).unwrap();
        assert_eq!(settle(&mut [&mut a])[0], [TableEvent::Up(pb.clone())]);
        let mut b2 = raw(&f);
        b2.connect(a.local(), &id, CAPS).unwrap();
        let evs = settle(&mut [&mut a]).remove(0);
        assert_eq!(
            evs,
            [TableEvent::Down(pb.clone()), TableEvent::Up(pb.clone())]
        );
        assert!(a.is_draining(&pb), "the old link was retired, not dropped");
        assert_eq!(a.ensure(&pb), Ok(Readiness::Ready), "the new link serves");
        a.finish_teardown(&pb).unwrap();
        assert!(!a.is_draining(&pb));
        drop((b1, b2));
    }

    #[test]
    fn lower_peer_reconnecting_over_our_link_wins_too() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.2:7000");
        let mut b = table(&f, "10.0.0.1:7000");
        let pb = b.local().clone();
        a.ensure(&pb).unwrap(); // a initiates; b (lower) accepts
        settle(&mut [&mut a, &mut b]);
        // b loses its end without a's noticing, then reconnects.
        let mut b2 = raw(&f);
        b2.connect(a.local(), &encode_identity(&pb).unwrap(), CAPS)
            .unwrap();
        let evs = settle(&mut [&mut a]).remove(0);
        assert_eq!(
            evs,
            [TableEvent::Down(pb.clone()), TableEvent::Up(pb.clone())]
        );
        drop(b2);
    }

    #[test]
    fn higher_peer_reconnecting_over_our_link_is_a_duplicate() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut b = table(&f, "10.0.0.2:7000");
        let pb = b.local().clone();
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);
        let mut b2 = raw(&f);
        let l = b2
            .connect(a.local(), &encode_identity(&pb).unwrap(), CAPS)
            .unwrap();
        assert_eq!(
            poll(&mut a),
            [TableEvent::RefusedInbound {
                peer: Some(pb.clone()),
                reason: RejectReason::Duplicate
            }]
        );
        let mut evs = Vec::new();
        b2.poll_events(&mut evs);
        assert!(
            matches!(&evs[..], [TransportEvent::Rejected { link, private_data }]
            if *link == l && decode_reject(private_data) == Some(RejectReason::Duplicate))
        );
        assert_eq!(a.ensure(&pb), Ok(Readiness::Ready), "the original survives");
    }

    /// The race's other delivery order: the winner's `Duplicate` reject reaches
    /// the loser *before* the winner's own connect request does — which the
    /// mock never produces between two tables, but a real fabric can. The loser
    /// must report a lost tie-break, not a failure, and then accept the winner.
    #[test]
    fn reject_before_the_winners_request_is_a_lost_tie_break() {
        let f = MockFabric::new();
        let mut hi = table(&f, "10.0.0.2:7000");
        let plo = peer("10.0.0.1:7000");
        let mut lo = raw(&f);
        lo.listen(&plo).unwrap();
        hi.ensure(&plo).unwrap();

        let mut evs = Vec::new();
        lo.poll_events(&mut evs);
        let TransportEvent::ConnectRequest { request, .. } = evs[0] else {
            panic!("{evs:?}");
        };
        lo.reject(request, &encode_reject(RejectReason::Duplicate))
            .unwrap();
        lo.connect(hi.local(), &encode_identity(&plo).unwrap(), CAPS)
            .unwrap();

        assert_eq!(
            settle(&mut [&mut hi])[0],
            [
                TableEvent::LostTieBreak(plo.clone()),
                TableEvent::Up(plo.clone())
            ]
        );
        assert_eq!(hi.ensure(&plo), Ok(Readiness::Ready));
    }

    #[test]
    fn malformed_and_self_identities_are_refused() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut x = raw(&f);
        x.connect(a.local(), b"hello", CAPS).unwrap();
        x.connect(
            a.local(),
            &encode_identity(&a.local().clone()).unwrap(),
            CAPS,
        )
        .unwrap();
        assert_eq!(
            poll(&mut a),
            [
                TableEvent::RefusedInbound {
                    peer: None,
                    reason: RejectReason::MalformedIdentity
                },
                TableEvent::RefusedInbound {
                    peer: Some(a.local().clone()),
                    reason: RejectReason::MalformedIdentity
                },
            ]
        );
        assert_eq!(a.ensure(&a.local().clone()), Err(TableError::SelfConnect));
    }

    #[test]
    fn acceptor_answering_to_another_name_fails_the_link() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let dialed = peer("10.0.0.2:7000");
        let mut imposter = raw(&f);
        imposter.listen(&dialed).unwrap();
        a.ensure(&dialed).unwrap();
        let mut evs = Vec::new();
        imposter.poll_events(&mut evs);
        let TransportEvent::ConnectRequest { request, .. } = evs[0] else {
            panic!("{evs:?}");
        };
        imposter
            .accept(
                request,
                &encode_identity(&peer("node2:7000")).unwrap(),
                CAPS,
            )
            .unwrap();
        assert_eq!(
            poll(&mut a),
            [TableEvent::Failed {
                peer: dialed.clone(),
                reason: ConnectFailure::NameMismatch {
                    dialed: dialed.clone(),
                    answered: Some(peer("node2:7000"))
                }
            }]
        );
        assert!(a.live(&dialed).is_none());
    }

    #[test]
    fn teardown_blocks_reconnect_until_finished() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut b = table(&f, "10.0.0.2:7000");
        let (pa, pb) = (a.local().clone(), b.local().clone());
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);

        assert_eq!(a.begin_teardown(&pb), Ok(true));
        assert!(a.is_draining(&pb));
        assert!(a.live(&pb).is_none());
        assert_eq!(a.ensure(&pb), Err(TableError::Draining(pb.clone())));

        // b notices (a's link is in ERROR, but a has not disconnected): b's own
        // link is still live, so it would not redial. Force b to: its peer is
        // down from its view once a destroys the link.
        a.finish_teardown(&pb).unwrap();
        let evs = settle(&mut [&mut a, &mut b]);
        assert_eq!(evs[1], [TableEvent::Down(pa.clone())]);
        // Now b is draining and a is clear: a's redial is refused by b (FR-023
        // at the far end) until b finishes too.
        a.ensure(&pb).unwrap();
        let evs = settle(&mut [&mut a, &mut b]);
        assert_eq!(
            evs[0],
            [TableEvent::Failed {
                peer: pb.clone(),
                reason: ConnectFailure::Rejected(Some(RejectReason::Draining))
            }]
        );
        b.finish_teardown(&pa).unwrap();
        a.ensure(&pb).unwrap();
        let evs = settle(&mut [&mut a, &mut b]);
        assert_eq!(evs[0], [TableEvent::Up(pb.clone())]);
        exchange(&mut b, &mut a, b"back");
    }

    /// Principle IX across teardown: a write in flight when the link is
    /// forced to ERROR comes back through the drain exactly once, and the link
    /// cannot be destroyed while it is still held.
    #[test]
    fn teardown_drains_every_held_state_before_destroying_the_link() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut b = table(&f, "10.0.0.2:7000");
        let (pa, pb) = (a.local().clone(), b.local().clone());
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);

        let mut src = [9u8; 8];
        // SAFETY: `src` outlives the region and the drain below.
        let rs = unsafe {
            a.transport()
                .register(src.as_mut_ptr(), 8, Access::LOCAL_WRITE)
        }
        .unwrap();
        {
            let (link, slab) = a.live(&pb).unwrap();
            // Two sends with no receive posted at b: both stay queued (RNR).
            // SAFETY: as above.
            unsafe {
                post_send_tracked(
                    link,
                    slab,
                    ["pin-1", "pin-2"]
                        .into_iter()
                        .map(|state| TrackedSend {
                            op: SendOp::Send,
                            local: rs.sge(rs.addr(), 8).unwrap(),
                            state,
                        })
                        .collect(),
                )
            }
            .unwrap();
        }
        a.begin_teardown(&pb).unwrap();
        assert_eq!(
            a.finish_teardown(&pb),
            Err(TableError::Undrained {
                peer: pb.clone(),
                outstanding: 2
            }),
            "destroying now would lose both pins"
        );

        let mut released = Vec::new();
        a.drain(&pb, &mut released, Duration::from_secs(1)).unwrap();
        let mut tags: Vec<_> = released.iter().map(|(c, t)| (c.result, *t)).collect();
        tags.sort_by_key(|(_, t)| *t);
        assert_eq!(
            tags,
            [
                (Err(CompletionError::Flushed), "pin-1"),
                (Err(CompletionError::Flushed), "pin-2")
            ]
        );
        a.finish_teardown(&pb).unwrap();
        drop(rs);
        settle(&mut [&mut a, &mut b]);
        b.begin_teardown(&pa).unwrap();
        b.finish_teardown(&pa).unwrap();
    }

    #[test]
    #[should_panic(expected = "still holding state")]
    fn dropping_a_table_with_held_state_is_loud() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut b = table(&f, "10.0.0.2:7000");
        let pb = b.local().clone();
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);
        let mut buf = [0u8; 8];
        // SAFETY: the receive is never delivered; the table is dropped first.
        let r = unsafe {
            a.transport()
                .register(buf.as_mut_ptr(), 8, Access::LOCAL_WRITE)
        }
        .unwrap();
        let (link, slab) = a.live(&pb).unwrap();
        // SAFETY: as above.
        unsafe {
            post_recv_tracked(
                link,
                slab,
                vec![TrackedRecv {
                    buf: r.sge(r.addr(), 8).unwrap(),
                    state: "held",
                }],
            )
        }
        .unwrap();
        drop(r);
        drop(a);
    }

    #[test]
    fn connect_to_nobody_fails_without_retry() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let p = peer("10.0.0.9:1");
        a.ensure(&p).unwrap();
        assert!(matches!(
            &poll(&mut a)[..],
            [TableEvent::Failed { peer, reason: ConnectFailure::Unreachable(_) }] if *peer == p
        ));
        assert!(poll(&mut a).is_empty(), "no retry");
        assert_eq!(f.qp_count(), 0, "the failed attempt is closed");
        assert_eq!(a.ensure(&p), Ok(Readiness::Connecting), "next need retries");
    }

    #[test]
    fn local_name_too_long_for_private_data_is_refused() {
        let long = format!("{}:1", "h".repeat(MAX_IDENTITY_LEN));
        let r = ConnectionTable::<Tag>::new(
            Box::new(MockTransport::new(MockFabric::new())),
            peer(&long),
            CAPS,
        );
        assert!(matches!(
            r,
            Err(TableError::Transport(RdmaError::Invalid { .. }))
        ));
    }
}
