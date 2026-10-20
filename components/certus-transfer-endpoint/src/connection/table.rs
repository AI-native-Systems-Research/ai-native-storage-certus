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

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::time::Duration;

use super::credits::SendCredits;
use super::identity::{
    decode_identity, decode_reject, encode_identity, encode_reject, RejectReason, MAX_IDENTITY_LEN,
};
use super::slab::{
    drain, post_recv_tracked, post_send_tracked, DrainError, Slab, SlabError, TrackedPostError,
    TrackedRecv, TrackedSend,
};
use super::transport::{
    Completion, Link, LinkId, QpCaps, QpState, RdmaError, RequestId, Transport, TransportEvent,
};
use super::PeerId;
use crate::telemetry::{Counters, ExtraStats, Extras};
use interfaces::TransferStats;

/// Whether a peer's link is ready to carry work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Established; [`ConnectionTable::post_sends`] and friends accept work
    /// for it. Data messages still wait for the peer's `OP_HELLO` (T076).
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
    /// No established link to the peer, so nothing can be posted to it.
    NotLive(PeerId),
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
            TableError::NotLive(p) => write!(f, "no established connection to {p}"),
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
    /// The configured send-credit limit exceeds what the device granted this
    /// link, and was clamped to the grant. Reported once, when the link comes
    /// up, so a limit that is not in force never looks as if it were.
    CreditsClamped {
        peer: PeerId,
        configured: u32,
        granted: u32,
    },
}

/// A completion the reaper could not attribute; see [`SlabError`]. Every
/// variant is a defect, so the caller should fail loudly.
#[derive(Debug)]
pub enum ReapError<T> {
    NotLive(PeerId),
    Slab(SlabError<T>),
    Poll(RdmaError),
}

impl<T> fmt::Display for ReapError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReapError::NotLive(p) => write!(f, "no established connection to {p}"),
            ReapError::Slab(e) => write!(f, "{e}"),
            ReapError::Poll(e) => write!(f, "{e}"),
        }
    }
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
    credits: SendCredits,
}

/// A link in ERROR, with whatever its slab still holds.
struct Draining<T> {
    link: LinkId,
    slab: Slab<T>,
}

struct Entry<T> {
    /// Our outbound attempt, not yet established. It has no slab: nothing can
    /// be posted on a link before it is live.
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
    /// Configured send-credit limit per link, before clamping to the grant.
    send_credits: u32,
    entries: HashMap<PeerId, Entry<T>>,
    /// Which peer each live, pending or draining link belongs to.
    owners: HashMap<LinkId, PeerId>,
    scratch: Vec<TransportEvent>,
    counters: Counters,
    extras: Extras,
}

impl<T> ConnectionTable<T> {
    /// Listen as `local` on `transport`. Every link gets `caps`, and at most
    /// `send_credits` send-queue requests in flight (clamped to the grant).
    pub fn new(
        mut transport: Box<dyn Transport>,
        local: PeerId,
        caps: QpCaps,
        send_credits: u32,
    ) -> Result<Self, TableError> {
        if send_credits == 0 {
            // Nothing could ever be posted: every send would wait forever.
            return Err(TableError::Transport(RdmaError::Invalid {
                op: "ConnectionTable::new",
                reason: "send_credits must be at least 1",
            }));
        }
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
            send_credits,
            entries: HashMap::new(),
            owners: HashMap::new(),
            scratch: Vec::new(),
            counters: Counters::default(),
            extras: Extras::new(),
        })
    }

    pub fn local(&self) -> &PeerId {
        &self.local
    }

    /// A snapshot of the endpoint's counters. The send-queue gauges are read
    /// from the live links' credits now, so they cannot drift from what the
    /// credits enforce.
    pub fn stats(&self) -> TransferStats {
        let live = self
            .entries
            .values()
            .filter_map(|e| e.live.as_ref().filter(|l| l.up));
        let (in_flight, peak) = live.fold((0u32, 0u32), |(sum, peak), l| {
            (sum + l.credits.in_flight(), peak.max(l.credits.peak()))
        });
        self.counters.snapshot(in_flight, peak)
    }

    /// The always-counted tier, for the layers above that fill its other
    /// fields (see `crate::telemetry`).
    pub fn counters_mut(&mut self) -> &mut Counters {
        &mut self.counters
    }

    /// The `telemetry` tier: `None` when the feature is off.
    pub fn extra_stats(&self) -> Option<ExtraStats> {
        self.extras.snapshot()
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
        self.extras.connect_attempted();
        let link = self
            .transport
            .connect(peer, &self.identity, self.caps)
            .map_err(|e| {
                self.extras.connect_failed();
                TableError::Transport(e)
            })?;
        self.entries.entry(peer.clone()).or_default().pending = Some(link);
        self.owners.insert(link, peer.clone());
        Ok(Readiness::Connecting)
    }

    /// The peer's send credits, while it has an established link.
    pub fn credits(&self, peer: &PeerId) -> Option<&SendCredits> {
        let live = self.entries.get(peer)?.live.as_ref().filter(|l| l.up)?;
        Some(&live.credits)
    }

    /// The device-reported state of the peer's established link, for a
    /// deadline's diagnostic: whether the queue pair is still RTS, or already
    /// in ERROR, says which side stalled.
    pub fn link_state(&mut self, peer: &PeerId) -> Option<QpState> {
        let (link, _) = self.up_link(peer)?;
        link.state().ok()
    }

    fn up_link(&mut self, peer: &PeerId) -> Option<(&mut dyn Link, &mut Live<T>)> {
        let live = self.entries.get_mut(peer)?.live.as_mut().filter(|l| l.up)?;
        let link = self.transport.link(live.link)?;
        Some((link, live))
    }

    /// Post send-queue requests from the front of `queue` for as long as
    /// `peer`'s send credits allow, leaving the rest queued for the caller —
    /// FR-015's "chunk, don't fail" at this layer. Returns how many were
    /// posted; call again after [`reap`](Self::reap) returns credits.
    ///
    /// On a transport failure partway, the requests not queued go back on the
    /// **front** of `queue`, in their original order, and their credits are
    /// returned; the queued ones complete through `reap` as usual.
    ///
    /// # Safety
    ///
    /// As [`Link::post_send`]: every local buffer must stay valid until its
    /// completion is reaped or the link is forced to ERROR and drained.
    pub unsafe fn post_sends<'a>(
        &mut self,
        peer: &PeerId,
        queue: &mut VecDeque<TrackedSend<'a, T>>,
    ) -> Result<usize, TableError> {
        let Some((link, live)) = self.up_link(peer) else {
            return Err(TableError::NotLive(peer.clone()));
        };
        let n = live.credits.take(queue.len().min(u32::MAX as usize) as u32) as usize;
        if n == 0 {
            return Ok(0);
        }
        let batch: Vec<_> = queue.drain(..n).collect();
        let shape: Vec<_> = batch.iter().map(|r| (r.op, r.local)).collect();
        let result = post_send_tracked(link, &mut live.slab, batch);
        let posted = match result {
            Ok(()) => Ok(n),
            Err(TrackedPostError {
                accepted,
                error,
                unposted,
            }) => {
                live.credits.give_back(unposted.len() as u32);
                for ((op, local), state) in shape[accepted..].iter().zip(unposted).rev() {
                    queue.push_front(TrackedSend {
                        op: *op,
                        local: *local,
                        state,
                    });
                }
                Err(TableError::Transport(error))
            }
        };
        debug_assert_eq!(
            live.credits.in_flight() as usize,
            live.slab.send_outstanding()
        );
        posted
    }

    /// Post receive buffers on `peer`'s link. Receives hold no send credit;
    /// their bound is the receive partition (FR-026, T080).
    ///
    /// # Safety
    ///
    /// As [`Link::post_recv`].
    pub unsafe fn post_recvs<'a>(
        &mut self,
        peer: &PeerId,
        reqs: Vec<TrackedRecv<'a, T>>,
    ) -> Result<(), TrackedPostError<T>> {
        let Some((link, live)) = self.up_link(peer) else {
            return Err(TrackedPostError {
                accepted: 0,
                error: RdmaError::Invalid {
                    op: "post_recvs",
                    reason: "no established connection to the peer",
                },
                unposted: reqs.into_iter().map(|r| r.state).collect(),
            });
        };
        post_recv_tracked(link, &mut live.slab, reqs)
    }

    /// Poll `peer`'s link for up to `max` completions, releasing each one's
    /// state through the slab onto `out` and returning a send credit for each
    /// send-queue completion. This is the only way to poll a live link, so the
    /// credits and the slab cannot drift apart.
    ///
    /// Every completion polled is processed even if one fails to attribute, so
    /// none is lost; the first such failure is returned after.
    pub fn reap(
        &mut self,
        peer: &PeerId,
        out: &mut Vec<(Completion, T)>,
        max: usize,
    ) -> Result<usize, ReapError<T>> {
        let Some((link, live)) = self.up_link(peer) else {
            return Err(ReapError::NotLive(peer.clone()));
        };
        let mut batch = Vec::new();
        let n = link.poll(&mut batch, max).map_err(ReapError::Poll)?;
        let mut first_error = None;
        for c in batch {
            match live.slab.complete_kind(&c) {
                Ok((kind, state)) => {
                    if kind.is_send_queue() {
                        live.credits.give_back(1);
                    }
                    out.push((c, state));
                }
                Err(e) => {
                    if let SlabError::KindMismatch { posted, .. } = &e {
                        if posted.is_send_queue() {
                            live.credits.give_back(1);
                        }
                    }
                    first_error.get_or_insert(ReapError::Slab(e));
                }
            }
        }
        debug_assert_eq!(
            live.credits.in_flight() as usize,
            live.slab.send_outstanding()
        );
        first_error.map_or(Ok(n), Err)
    }

    /// Every peer the table holds anything for — a pending connect, a live
    /// link or a draining one. Shutdown tears each down.
    pub fn peers(&self) -> Vec<PeerId> {
        self.entries.keys().cloned().collect()
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

    /// Credits for a new link, and whether the configured limit was clamped.
    fn credits_for(&mut self, link: LinkId) -> (SendCredits, Option<u32>) {
        let granted = self
            .transport
            .link(link)
            .expect("new link missing from its transport")
            .granted()
            .max_send_wr;
        let clamped = (self.send_credits > granted).then_some(granted);
        (SendCredits::new(self.send_credits, granted), clamped)
    }

    /// Force `live` to ERROR and move it to `peer`'s draining set. Its send
    /// credits go with it: nothing posts on a draining link, and the slab
    /// alone accounts for what is still outstanding there.
    fn retire(&mut self, peer: &PeerId, live: Live<T>) -> Result<(), RdmaError> {
        self.counters.fold_peak(live.credits.peak());
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
                    Some(RejectReason::Duplicate) => {
                        self.counters.duplicate_connections_resolved += 1;
                        out.push(TableEvent::LostTieBreak(peer))
                    }
                    reason => self.failed(peer, ConnectFailure::Rejected(reason), out),
                }
            }
            TransportEvent::ConnectError { link, reason } => {
                if let Some(peer) = self.take_pending(link) {
                    self.failed(peer, ConnectFailure::Unreachable(reason), out);
                }
            }
            TransportEvent::Disconnected { link } => {
                let Some(peer) = self.owners.get(&link).cloned() else {
                    return;
                };
                if self.take_pending(link).is_some() {
                    self.failed(peer, ConnectFailure::Disconnected, out);
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

    fn failed(&mut self, peer: PeerId, reason: ConnectFailure, out: &mut Vec<TableEvent>) {
        self.extras.connect_failed();
        out.push(TableEvent::Failed { peer, reason });
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
                    // Counted here and on a Duplicate reject — the two ways an
                    // end discards its own attempt — so a race counts once
                    // across the pair, never at the winner too.
                    self.take_pending(link);
                    self.counters.duplicate_connections_resolved += 1;
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
                let (credits, _) = self.credits_for(link);
                self.entries.entry(peer.clone()).or_default().live = Some(Live {
                    link,
                    initiator: Initiator::Peer,
                    up: false,
                    slab,
                    credits,
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
            out.push(TableEvent::Up(peer.clone()));
            self.report_clamp(&peer, link, out);
            return;
        }
        if e.pending != Some(link) {
            return;
        }
        // Our connect completed: the acceptor must answer to the dialed name.
        let answered = decode_identity(private_data);
        if answered.as_ref() != Some(&peer) {
            self.take_pending(link);
            let reason = ConnectFailure::NameMismatch {
                dialed: peer.clone(),
                answered,
            };
            self.failed(peer, reason, out);
            return;
        }
        self.extras.connect_established();
        let slab = self.slab_for(link);
        let (credits, _) = self.credits_for(link);
        let e = self.entries.get_mut(&peer).unwrap();
        e.pending = None;
        e.live = Some(Live {
            link,
            initiator: Initiator::Local,
            up: true,
            slab,
            credits,
        });
        out.push(TableEvent::Up(peer.clone()));
        self.report_clamp(&peer, link, out);
    }

    fn report_clamp(&mut self, peer: &PeerId, link: LinkId, out: &mut Vec<TableEvent>) {
        if let (_, Some(granted)) = self.credits_for(link) {
            out.push(TableEvent::CreditsClamped {
                peer: peer.clone(),
                configured: self.send_credits,
                granted,
            });
        }
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
    use crate::connection::transport::{Access, Completed, CompletionError, Region, SendOp};

    const CAPS: QpCaps = QpCaps {
        max_send_wr: 8,
        max_recv_wr: 8,
    };

    fn peer(s: &str) -> PeerId {
        PeerId::parse(s).unwrap()
    }

    fn table(fabric: &MockFabric, name: &str) -> ConnectionTable<Tag> {
        table_with(fabric, name, CAPS.max_send_wr)
    }

    fn table_with(fabric: &MockFabric, name: &str, send_credits: u32) -> ConnectionTable<Tag> {
        ConnectionTable::new(
            Box::new(MockTransport::new(fabric.clone())),
            peer(name),
            CAPS,
            send_credits,
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
        // SAFETY: as above.
        unsafe {
            b.post_recvs(
                &pb,
                vec![TrackedRecv {
                    buf: rd.sge(rd.addr(), 4).unwrap(),
                    state: "recv",
                }],
            )
        }
        .expect("b has a link to a");
        let mut q = VecDeque::from([TrackedSend {
            op: SendOp::Send,
            local: rs.sge(rs.addr(), 4).unwrap(),
            state: "send",
        }]);
        // SAFETY: as above.
        assert_eq!(
            unsafe { a.post_sends(&pa, &mut q) },
            Ok(1),
            "a has a link to b"
        );
        assert_eq!(
            reap(b, &pb),
            [(Ok(Completed::Recv { byte_len: 4 }), "recv")]
        );
        assert_eq!(reap(a, &pa), [(Ok(Completed::Send), "send")]);
        drop((rs, rd));
        assert_eq!(&dst, msg);
    }

    /// Reap `peer`'s live link: each completion's result and released state.
    fn reap(
        t: &mut ConnectionTable<Tag>,
        peer: &PeerId,
    ) -> Vec<(Result<Completed, CompletionError>, Tag)> {
        let mut out = Vec::new();
        t.reap(peer, &mut out, 64).unwrap();
        out.into_iter().map(|(c, t)| (c.result, t)).collect()
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
        assert!(a.credits(&dialed).is_none(), "no link was kept");
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
        assert!(a.credits(&pb).is_none());
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
        // Two sends with no receive posted at b: both stay queued (RNR).
        let mut q: VecDeque<_> = ["pin-1", "pin-2"]
            .into_iter()
            .map(|state| TrackedSend {
                op: SendOp::Send,
                local: rs.sge(rs.addr(), 8).unwrap(),
                state,
            })
            .collect();
        // SAFETY: as above.
        assert_eq!(unsafe { a.post_sends(&pb, &mut q) }, Ok(2));
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
        // SAFETY: as above.
        unsafe {
            a.post_recvs(
                &pb,
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
            8,
        );
        assert!(matches!(
            r,
            Err(TableError::Transport(RdmaError::Invalid { .. }))
        ));
    }

    // --- send-queue credits (T014) -------------------------------------------

    /// Backing memory for [`writes`]: `src` at the writer, `land` at the target.
    struct Writes {
        src: Vec<u8>,
        land: Vec<u8>,
    }

    /// Register `bufs` and describe `n` one-byte writes, the i-th from
    /// `src[i]` to `land[i]`: (source region, landing region, (op, local addr)).
    fn writes(
        a: &mut ConnectionTable<Tag>,
        b: &mut ConnectionTable<Tag>,
        bufs: &mut Writes,
        n: usize,
    ) -> (Region, Region, Vec<(SendOp, u64)>) {
        // SAFETY: `bufs` outlives both regions and every completion in the
        // calling test.
        let rs = unsafe {
            a.transport()
                .register(bufs.src.as_mut_ptr(), bufs.src.len(), Access::LOCAL_WRITE)
        }
        .unwrap();
        let rl = unsafe {
            b.transport().register(
                bufs.land.as_mut_ptr(),
                bufs.land.len(),
                Access::LOCAL_WRITE | Access::REMOTE_WRITE,
            )
        }
        .unwrap();
        let ops = (0..n as u64)
            .map(|i| {
                (
                    SendOp::RdmaWrite {
                        remote_addr: rl.addr() + i,
                        rkey: rl.rkey(),
                    },
                    rs.addr() + i,
                )
            })
            .collect();
        (rs, rl, ops)
    }

    const TAGS: [&str; 12] = [
        "w0", "w1", "w2", "w3", "w4", "w5", "w6", "w7", "w8", "w9", "w10", "w11",
    ];

    /// SC-004's bound at this layer, and FR-015's chunking: a queue longer
    /// than the credit limit posts in chunks, never holds more than the limit
    /// in flight, and keeps request order.
    #[test]
    fn credits_bound_in_flight_and_chunk_a_long_queue() {
        let f = MockFabric::new();
        let mut a = table_with(&f, "10.0.0.1:7000", 3);
        let mut b = table(&f, "10.0.0.2:7000");
        let pb = b.local().clone();
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);
        let mut bufs = Writes {
            src: (0..12).collect(),
            land: vec![0; 12],
        };
        let (rs, rl, ops) = writes(&mut a, &mut b, &mut bufs, 12);
        let mut q: VecDeque<_> = ops
            .iter()
            .zip(TAGS)
            .map(|((op, addr), state)| TrackedSend {
                op: *op,
                local: rs.sge(*addr, 1).unwrap(),
                state,
            })
            .collect();

        let mut done = Vec::new();
        let mut rounds = 0;
        while !q.is_empty() || a.credits(&pb).unwrap().in_flight() > 0 {
            // SAFETY: the regions outlive the loop.
            let posted = unsafe { a.post_sends(&pb, &mut q) }.unwrap();
            assert!(posted <= 3);
            assert!(
                a.credits(&pb).unwrap().in_flight() <= 3,
                "never past the limit"
            );
            let mut out = Vec::new();
            a.reap(&pb, &mut out, 64).unwrap();
            done.extend(out.into_iter().map(|(_, t)| t));
            rounds += 1;
            assert!(rounds < 100, "no progress");
        }
        assert_eq!(done, TAGS, "every write completes, in order");
        assert_eq!(a.credits(&pb).unwrap().peak(), 3);
        assert!(rounds >= 4, "12 writes at 3 credits take at least 4 rounds");
        drop((rs, rl));
        assert_eq!(bufs.land, bufs.src);
    }

    /// A queue that cannot proceed — sends stalled on RNR — keeps its credits
    /// held, and the rest of the queue waits rather than failing.
    #[test]
    fn exhausted_credits_post_nothing_and_fail_nothing() {
        let f = MockFabric::new();
        let mut a = table_with(&f, "10.0.0.1:7000", 2);
        let mut b = table(&f, "10.0.0.2:7000");
        let pb = b.local().clone();
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);
        let mut src = [0u8; 8];
        // SAFETY: `src` outlives the region and the drain below.
        let rs = unsafe {
            a.transport()
                .register(src.as_mut_ptr(), 8, Access::LOCAL_WRITE)
        }
        .unwrap();
        let mut q: VecDeque<_> = TAGS[..5]
            .iter()
            .map(|&state| TrackedSend {
                op: SendOp::Send,
                local: rs.sge(rs.addr(), 8).unwrap(),
                state,
            })
            .collect();
        // SAFETY: as above.
        assert_eq!(unsafe { a.post_sends(&pb, &mut q) }, Ok(2));
        assert_eq!(
            unsafe { a.post_sends(&pb, &mut q) },
            Ok(0),
            "no credit, no post"
        );
        assert_eq!(q.len(), 3, "the rest waits");
        assert_eq!(q.front().unwrap().state, "w2", "in order");
        assert!(reap(&mut a, &pb).is_empty(), "stalled on RNR: nothing back");

        a.begin_teardown(&pb).unwrap();
        let mut out = Vec::new();
        a.drain(&pb, &mut out, Duration::from_secs(1)).unwrap();
        assert_eq!(out.len(), 2, "the two in flight drain as flushed");
        a.finish_teardown(&pb).unwrap();
        drop(rs);
    }

    /// A transport refusal partway returns the unqueued requests to the front
    /// of the queue, in order, and their credits with them. Through the table
    /// alone the device never refuses — credits are clamped to its depth — so
    /// the test fills device slots behind the table's back.
    #[test]
    fn transport_refusal_requeues_at_the_front_and_returns_credits() {
        let f = MockFabric::new();
        let mut a = table_with(&f, "10.0.0.1:7000", 4);
        let mut b = table(&f, "10.0.0.2:7000");
        let pb = b.local().clone();
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);
        let mut bufs = Writes {
            src: vec![1; 8],
            land: vec![0; 8],
        };
        let (rs, rl, ops) = writes(&mut a, &mut b, &mut bufs, 5);
        // Raw writes execute at once but hold their device slots until polled:
        // 7 of the 8, leaving room for exactly one more.
        let raw = a.entries[&pb].live.as_ref().unwrap().link;
        for k in 0..7 {
            // SAFETY: the regions outlive every completion below.
            unsafe {
                a.transport().link(raw).unwrap().post_send(&[
                    crate::connection::transport::SendRequest {
                        wr_id: u64::MAX - k,
                        op: ops[4].0,
                        local: rs.sge(ops[4].1, 1).unwrap(),
                        signaled: true,
                    },
                ])
            }
            .unwrap();
        }
        let mut q: VecDeque<_> = ops[..4]
            .iter()
            .zip(["x", "y", "z", "w"])
            .map(|((op, addr), state)| TrackedSend {
                op: *op,
                local: rs.sge(*addr, 1).unwrap(),
                state,
            })
            .collect();
        // Four credits, so all four are tried; the device takes x only.
        // SAFETY: as above.
        let r = unsafe { a.post_sends(&pb, &mut q) };
        assert!(matches!(r, Err(TableError::Transport(_))), "{r:?}");
        let order: Vec<_> = q.iter().map(|r| r.state).collect();
        assert_eq!(order, ["y", "z", "w"], "unqueued back in front, in order");
        assert_eq!(
            a.credits(&pb).unwrap().in_flight(),
            1,
            "their credits returned"
        );

        // Reaping processes every completion even though the raw ones are
        // unattributable: x comes back, and the error is still reported.
        let mut out = Vec::new();
        let e = a.reap(&pb, &mut out, 64);
        assert!(
            matches!(e, Err(ReapError::Slab(SlabError::UnknownWrId(_)))),
            "{e:?}"
        );
        assert_eq!(out.iter().map(|(_, t)| *t).collect::<Vec<_>>(), ["x"]);
        assert_eq!(a.credits(&pb).unwrap().in_flight(), 0);
        // SAFETY: as above.
        assert_eq!(
            unsafe { a.post_sends(&pb, &mut q) },
            Ok(3),
            "the rest go now"
        );
        let mut out = Vec::new();
        a.reap(&pb, &mut out, 64).unwrap();
        assert_eq!(
            out.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
            ["y", "z", "w"]
        );
        drop((rs, rl));
    }

    #[test]
    fn credits_clamped_to_the_grant_are_reported() {
        let f = MockFabric::new();
        let mut a = table_with(&f, "10.0.0.1:7000", 64);
        let mut b = table(&f, "10.0.0.2:7000");
        let (pa, pb) = (a.local().clone(), b.local().clone());
        a.ensure(&pb).unwrap();
        let evs = settle(&mut [&mut a, &mut b]);
        assert_eq!(
            evs[0],
            [
                TableEvent::Up(pb.clone()),
                TableEvent::CreditsClamped {
                    peer: pb.clone(),
                    configured: 64,
                    granted: CAPS.max_send_wr
                }
            ]
        );
        assert_eq!(
            evs[1],
            [TableEvent::Up(pa)],
            "b's limit was within the grant"
        );
        assert_eq!(a.credits(&pb).unwrap().limit(), CAPS.max_send_wr);
    }

    #[test]
    fn receives_hold_no_send_credit() {
        let f = MockFabric::new();
        let mut a = table_with(&f, "10.0.0.1:7000", 1);
        let mut b = table(&f, "10.0.0.2:7000");
        let pb = b.local().clone();
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);
        let mut buf = [0u8; 32];
        // SAFETY: `buf` outlives the region and the drain below.
        let r = unsafe {
            a.transport()
                .register(buf.as_mut_ptr(), 32, Access::LOCAL_WRITE)
        }
        .unwrap();
        let recvs = TAGS[..4]
            .iter()
            .enumerate()
            .map(|(i, &state)| TrackedRecv {
                buf: r.sge(r.addr() + 8 * i as u64, 8).unwrap(),
                state,
            })
            .collect();
        // SAFETY: as above.
        unsafe { a.post_recvs(&pb, recvs) }.unwrap();
        assert_eq!(a.credits(&pb).unwrap().in_flight(), 0);
        a.begin_teardown(&pb).unwrap();
        a.drain(&pb, &mut Vec::new(), Duration::from_secs(1))
            .unwrap();
        a.finish_teardown(&pb).unwrap();
        drop(r);
    }

    #[test]
    fn zero_credits_is_refused() {
        let r = ConnectionTable::<Tag>::new(
            Box::new(MockTransport::new(MockFabric::new())),
            peer("10.0.0.1:7000"),
            CAPS,
            0,
        );
        assert!(matches!(
            r,
            Err(TableError::Transport(RdmaError::Invalid { .. }))
        ));
    }

    #[test]
    fn posting_to_a_peer_without_a_link_is_refused() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let p = peer("10.0.0.9:1");
        let mut q: VecDeque<TrackedSend<'_, Tag>> = VecDeque::new();
        // SAFETY: nothing is posted.
        assert_eq!(
            unsafe { a.post_sends(&p, &mut q) },
            Err(TableError::NotLive(p.clone()))
        );
        assert!(matches!(
            a.reap(&p, &mut Vec::new(), 1),
            Err(ReapError::NotLive(_))
        ));
    }

    // --- counters (T015) ------------------------------------------------------

    /// A race counts once across the pair — at the end whose own attempt was
    /// discarded — under every poll order.
    #[test]
    fn a_race_counts_one_duplicate_across_the_pair() {
        for order in 0..2 {
            let f = MockFabric::new();
            let mut lo = table(&f, "10.0.0.1:7000");
            let mut hi = table(&f, "10.0.0.2:7000");
            let (plo, phi) = (lo.local().clone(), hi.local().clone());
            lo.ensure(&phi).unwrap();
            hi.ensure(&plo).unwrap();
            if order == 0 {
                settle(&mut [&mut lo, &mut hi]);
            } else {
                settle(&mut [&mut hi, &mut lo]);
            }
            let total = lo.stats().duplicate_connections_resolved
                + hi.stats().duplicate_connections_resolved;
            assert_eq!(total, 1, "order {order}");
        }
        // No race, no count.
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut b = table(&f, "10.0.0.2:7000");
        a.ensure(&b.local().clone()).unwrap();
        settle(&mut [&mut a, &mut b]);
        assert_eq!(a.stats().duplicate_connections_resolved, 0);
        assert_eq!(b.stats().duplicate_connections_resolved, 0);
    }

    /// The send-queue gauges come from the live credits, and the peak outlives
    /// the link that reached it.
    #[test]
    fn send_queue_gauges_track_credits_and_peak_survives_retirement() {
        let f = MockFabric::new();
        let mut a = table_with(&f, "10.0.0.1:7000", 4);
        let mut b = table(&f, "10.0.0.2:7000");
        let (pa, pb) = (a.local().clone(), b.local().clone());
        a.ensure(&pb).unwrap();
        settle(&mut [&mut a, &mut b]);
        let mut src = [0u8; 8];
        // SAFETY: `src` outlives the region and the drain below.
        let rs = unsafe {
            a.transport()
                .register(src.as_mut_ptr(), 8, Access::LOCAL_WRITE)
        }
        .unwrap();
        // Three sends stall on RNR (no receive at b), so they stay in flight.
        let mut q: VecDeque<_> = TAGS[..3]
            .iter()
            .map(|&state| TrackedSend {
                op: SendOp::Send,
                local: rs.sge(rs.addr(), 8).unwrap(),
                state,
            })
            .collect();
        // SAFETY: as above.
        assert_eq!(unsafe { a.post_sends(&pb, &mut q) }, Ok(3));
        let s = a.stats();
        assert_eq!(
            (s.send_queue_in_flight, s.send_queue_peak_in_flight),
            (3, 3)
        );

        a.begin_teardown(&pb).unwrap();
        a.drain(&pb, &mut Vec::new(), Duration::from_secs(1))
            .unwrap();
        a.finish_teardown(&pb).unwrap();
        let s = a.stats();
        assert_eq!(s.send_queue_in_flight, 0, "no live link");
        assert_eq!(s.send_queue_peak_in_flight, 3, "the peak survives the link");
        drop(rs);
        settle(&mut [&mut a, &mut b]);
        b.begin_teardown(&pa).unwrap();
        b.finish_teardown(&pa).unwrap();
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn extras_count_connect_attempts_and_outcomes() {
        let f = MockFabric::new();
        let mut a = table(&f, "10.0.0.1:7000");
        let mut b = table(&f, "10.0.0.2:7000");
        a.ensure(&b.local().clone()).unwrap();
        a.ensure(&peer("10.0.0.9:1")).unwrap();
        settle(&mut [&mut a, &mut b]);
        assert_eq!(
            a.extra_stats(),
            Some(ExtraStats {
                connects_attempted: 2,
                connects_established: 1,
                connects_failed: 1,
            })
        );
    }

    #[cfg(not(feature = "telemetry"))]
    #[test]
    fn extras_are_absent_not_zero_without_the_feature() {
        let f = MockFabric::new();
        let a = table(&f, "10.0.0.1:7000");
        assert_eq!(a.extra_stats(), None);
    }
}
