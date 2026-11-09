//! Per-connection message state: the capability exchange, message credits in
//! each direction, and the receive-buffer partition (FR-025, FR-026, D20, D22).
//!
//! A [`Session`] is created when a link comes up and dies with it, so a rebuilt
//! connection re-learns everything. It is pure bookkeeping — it never posts —
//! so every rule here is testable without a transport.
//!
//! # Bring-up
//!
//! 1. The link comes up with one **bootstrap** message credit: enough for
//!    `OP_HELLO`, which is itself a request.
//! 2. Both ends send `OP_HELLO` unsolicited and answer the peer's with
//!    `OP_HELLO_REPLY`. Nobody decides anything: each end computes the version
//!    from the two advertised ranges with a commutative rule
//!    ([`negotiate`]), so both reach the same verdict.
//! 3. On the peer's `OP_HELLO` the outbound credit limit becomes
//!    `min(local message_credits, peer's quota)`. Data requests wait until the
//!    peer's `OP_HELLO` **and** our own `OP_HELLO_REPLY` have both arrived —
//!    the first gives the version and limits, the second returns the bootstrap
//!    credit.
//!
//! # The receive partition
//!
//! Each end posts `reserve + quota` receives: a **reply reserve** of one per
//! own message credit (each request draws exactly one reply, FR-007) and an
//! **inbound quota** it advertises as the peer's `max_outstanding_requests`.
//! A peer that exceeds the quota is a protocol fault. The two counts are
//! tracked separately so a peer saturating its quota cannot take a buffer a
//! reply this end awaits will need (SC-016).

use crate::message::{Hello, MAX_ENTRIES, VERSION_MAX, VERSION_MIN};

/// The outcome of negotiating two advertised version ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Negotiated {
    /// The version both ends will speak: `min(a.max, b.max)`.
    Version(u16),
    /// The ranges do not intersect; the connection fails at establishment.
    Incompatible,
}

/// The version rule: `min(a.max, b.max)`, failing below `max(a.min, b.min)`.
/// Commutative by construction — `min` and `max` are — so both ends compute
/// the same verdict without an arbitrator (SC-014). A malformed range
/// (`min > max`) never intersects anything.
pub(crate) fn negotiate(a: (u16, u16), b: (u16, u16)) -> Negotiated {
    let v = a.1.min(b.1);
    if a.0 > a.1 || b.0 > b.1 || v < a.0.max(b.0) || v == 0 {
        return Negotiated::Incompatible;
    }
    Negotiated::Version(v)
}

/// Where a connection's capability exchange stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Our `OP_HELLO` is out (or about to be); the peer's is awaited.
    Greeting,
    /// Both `OP_HELLO`s seen and our reply returned: data may flow.
    Ready,
}

/// What this end advertises and is configured with, fixed per endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// Requests this end may have outstanding against a peer, before the
    /// peer's quota lowers it; also the size of the reply reserve.
    pub(crate) message_credits: u32,
    /// Requests the peer may have outstanding against this end.
    pub(crate) inbound_quota: u32,
}

impl Limits {
    /// Receives each connection posts: the reply reserve plus the quota.
    pub(crate) fn receives(&self) -> u32 {
        self.message_credits + self.inbound_quota
    }

    /// The `OP_HELLO` this end sends.
    pub(crate) fn hello(&self) -> Hello {
        Hello {
            max_entries: MAX_ENTRIES,
            max_outstanding_requests: self.inbound_quota,
            version_min: VERSION_MIN,
            version_max: VERSION_MAX,
        }
    }
}

/// Why the peer's `OP_HELLO` fails the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HelloError {
    /// No version in common: `IncompatiblePeer`, naming both ranges.
    Incompatible {
        ours: (u16, u16),
        theirs: (u16, u16),
    },
    /// A quota of zero would let this end send nothing, ever.
    ZeroQuota,
    /// A peer that cannot receive even one entry cannot be fetched from.
    ZeroMaxEntries,
    /// A second `OP_HELLO` on one connection.
    Duplicate,
}

/// Why an inbound message breaks the session's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Violation {
    /// A data request before the peer's `OP_HELLO`, or before negotiation.
    BeforeHello,
    /// The peer exceeded the quota this end advertised.
    OverQuota,
    /// A reply this end was not waiting for — the credits say so.
    UnexpectedReply,
}

/// One connection's message state.
#[derive(Debug)]
pub(crate) struct Session {
    limits: Limits,
    phase: Phase,
    version: Option<u16>,
    /// The peer's `max_entries`: chunk against this, not the local value.
    peer_max_entries: u32,
    /// Outbound: requests this end may still send, and how many are out.
    out_limit: u32,
    out_in_flight: u32,
    out_peak: u32,
    /// Whether our `OP_HELLO`'s reply is still owed — the bootstrap credit.
    hello_reply_owed: bool,
    peer_hello_seen: bool,
    /// Inbound: peer requests being served, never above the quota.
    in_service: u32,
    in_peak: u32,
}

impl Session {
    /// A new session for a link that just came up: one bootstrap credit, for
    /// our `OP_HELLO`.
    pub(crate) fn new(limits: Limits) -> Self {
        Session {
            limits,
            phase: Phase::Greeting,
            version: None,
            peer_max_entries: 0,
            out_limit: 1,
            out_in_flight: 0,
            out_peak: 0,
            hello_reply_owed: false,
            peer_hello_seen: false,
            in_service: 0,
            in_peak: 0,
        }
    }

    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    pub(crate) fn version(&self) -> Option<u16> {
        self.version
    }

    pub(crate) fn peer_max_entries(&self) -> u32 {
        self.peer_max_entries
    }

    /// Requests out now, and the most there have been.
    pub(crate) fn outbound(&self) -> (u32, u32) {
        (self.out_in_flight, self.out_peak)
    }

    /// Peer requests in service now, and the most there have been.
    pub(crate) fn inbound(&self) -> (u32, u32) {
        (self.in_service, self.in_peak)
    }

    /// Take the bootstrap credit for our `OP_HELLO`. Called once, as the link
    /// comes up.
    pub(crate) fn send_hello(&mut self) -> Hello {
        assert!(
            !self.hello_reply_owed && self.out_in_flight == 0,
            "OP_HELLO sent twice"
        );
        self.take_out();
        self.hello_reply_owed = true;
        self.limits.hello()
    }

    fn take_out(&mut self) {
        self.out_in_flight += 1;
        self.out_peak = self.out_peak.max(self.out_in_flight);
    }

    /// The peer's `OP_HELLO`: negotiate, and adopt its limits. The caller
    /// answers it with `OP_HELLO_REPLY` whatever this returns, unless it is
    /// tearing the connection down.
    pub(crate) fn on_hello(&mut self, theirs: &Hello) -> Result<u16, HelloError> {
        if self.peer_hello_seen {
            return Err(HelloError::Duplicate);
        }
        self.peer_hello_seen = true;
        let ours = (VERSION_MIN, VERSION_MAX);
        let range = (theirs.version_min, theirs.version_max);
        let Negotiated::Version(v) = negotiate(ours, range) else {
            return Err(HelloError::Incompatible {
                ours,
                theirs: range,
            });
        };
        if theirs.max_outstanding_requests == 0 {
            return Err(HelloError::ZeroQuota);
        }
        if theirs.max_entries == 0 {
            return Err(HelloError::ZeroMaxEntries);
        }
        self.version = Some(v);
        self.peer_max_entries = theirs.max_entries.min(MAX_ENTRIES);
        self.out_limit = self
            .limits
            .message_credits
            .min(theirs.max_outstanding_requests);
        self.advance_phase();
        Ok(v)
    }

    /// Our `OP_HELLO`'s reply: the bootstrap credit comes back.
    pub(crate) fn on_hello_reply(&mut self) -> Result<(), Violation> {
        if !self.hello_reply_owed {
            return Err(Violation::UnexpectedReply);
        }
        self.hello_reply_owed = false;
        self.out_in_flight -= 1;
        self.advance_phase();
        Ok(())
    }

    fn advance_phase(&mut self) {
        if self.peer_hello_seen && !self.hello_reply_owed && self.version.is_some() {
            self.phase = Phase::Ready;
        }
    }

    /// Message credits free for data requests now. Zero until `Ready`.
    pub(crate) fn credits_available(&self) -> u32 {
        if self.phase != Phase::Ready {
            return 0;
        }
        self.out_limit.saturating_sub(self.out_in_flight)
    }

    /// Spend a message credit on a data request. `false` — and nothing spent
    /// — when none is free: the request waits (FR-015, never fails).
    pub(crate) fn try_send_request(&mut self) -> bool {
        if self.credits_available() == 0 {
            return false;
        }
        self.take_out();
        true
    }

    /// A reply to a data request arrived: its credit comes back.
    pub(crate) fn on_reply(&mut self) -> Result<(), Violation> {
        // The bootstrap's reply is `on_hello_reply`; anything beyond what is
        // out (less the still-owed hello) is a reply nobody asked for.
        let data_out = self.out_in_flight - u32::from(self.hello_reply_owed);
        if data_out == 0 {
            return Err(Violation::UnexpectedReply);
        }
        self.out_in_flight -= 1;
        Ok(())
    }

    /// A data request arrived from the peer: it occupies one quota buffer
    /// until this end has replied.
    pub(crate) fn on_request(&mut self) -> Result<(), Violation> {
        if self.version.is_none() {
            return Err(Violation::BeforeHello);
        }
        if self.in_service >= self.limits.inbound_quota {
            return Err(Violation::OverQuota);
        }
        self.in_service += 1;
        self.in_peak = self.in_peak.max(self.in_service);
        Ok(())
    }

    /// This end replied to one of the peer's requests.
    pub(crate) fn replied(&mut self) {
        assert!(self.in_service > 0, "replied to a request never received");
        self.in_service -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        message_credits: 4,
        inbound_quota: 3,
    };

    fn hello(quota: u32, min: u16, max: u16) -> Hello {
        Hello {
            max_entries: 340,
            max_outstanding_requests: quota,
            version_min: min,
            version_max: max,
        }
    }

    /// SC-014: the rule is commutative over every pair of small ranges.
    #[test]
    fn negotiation_is_commutative() {
        let ranges: Vec<(u16, u16)> = (0..5)
            .flat_map(|lo| (0..5).map(move |hi| (lo, hi)))
            .collect();
        for &a in &ranges {
            for &b in &ranges {
                assert_eq!(negotiate(a, b), negotiate(b, a), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn negotiation_picks_the_lower_max_and_fails_without_overlap() {
        assert_eq!(negotiate((1, 3), (2, 5)), Negotiated::Version(3));
        assert_eq!(negotiate((1, 1), (1, 1)), Negotiated::Version(1));
        assert_eq!(negotiate((1, 2), (3, 4)), Negotiated::Incompatible);
        assert_eq!(
            negotiate((2, 1), (1, 2)),
            Negotiated::Incompatible,
            "malformed"
        );
        assert_eq!(
            negotiate((0, 0), (0, 0)),
            Negotiated::Incompatible,
            "no version 0"
        );
    }

    /// No data request leaves before both the peer's hello and our reply.
    #[test]
    fn data_waits_for_both_hellos() {
        let mut s = Session::new(LIMITS);
        assert_eq!(s.credits_available(), 0);
        s.send_hello();
        assert!(!s.try_send_request(), "the bootstrap credit is spent");
        s.on_hello(&hello(2, 1, 1)).unwrap();
        assert_eq!(s.phase(), Phase::Greeting, "our reply is still owed");
        assert_eq!(s.credits_available(), 0);
        s.on_hello_reply().unwrap();
        assert_eq!(s.phase(), Phase::Ready);
        // The other order reaches the same state.
        let mut t = Session::new(LIMITS);
        t.send_hello();
        t.on_hello_reply().unwrap();
        assert_eq!(
            t.phase(),
            Phase::Greeting,
            "the peer's hello is still awaited"
        );
        t.on_hello(&hello(2, 1, 1)).unwrap();
        assert_eq!(t.phase(), Phase::Ready);
    }

    /// The outbound limit is the lower of ours and the peer's quota (FR-026).
    #[test]
    fn outbound_credits_are_clamped_to_the_peers_quota_and_block_at_zero() {
        let mut s = Session::new(LIMITS);
        s.send_hello();
        s.on_hello(&hello(2, 1, 1)).unwrap();
        s.on_hello_reply().unwrap();
        assert_eq!(s.credits_available(), 2, "peer's quota 2 < our 4");
        assert!(s.try_send_request());
        assert!(s.try_send_request());
        assert!(!s.try_send_request(), "blocks, does not fail");
        s.on_reply().unwrap();
        assert!(s.try_send_request());
        assert_eq!(s.outbound(), (2, 2));
        let mut wide = Session::new(LIMITS);
        wide.send_hello();
        wide.on_hello(&hello(64, 1, 1)).unwrap();
        wide.on_hello_reply().unwrap();
        assert_eq!(wide.credits_available(), 4, "our 4 < peer's 64");
    }

    #[test]
    fn hello_failures_fail_the_connection() {
        let fresh = || {
            let mut s = Session::new(LIMITS);
            s.send_hello();
            s
        };
        assert_eq!(
            fresh().on_hello(&hello(2, 2, 3)),
            Err(HelloError::Incompatible {
                ours: (1, 1),
                theirs: (2, 3)
            })
        );
        assert_eq!(
            fresh().on_hello(&hello(0, 1, 1)),
            Err(HelloError::ZeroQuota)
        );
        let mut s = fresh();
        assert_eq!(
            s.on_hello(&Hello {
                max_entries: 0,
                ..hello(1, 1, 1)
            }),
            Err(HelloError::ZeroMaxEntries)
        );
        let mut d = fresh();
        d.on_hello(&hello(1, 1, 1)).unwrap();
        assert_eq!(d.on_hello(&hello(1, 1, 1)), Err(HelloError::Duplicate));
    }

    /// The peer's max_entries is what chunking must respect, not ours.
    #[test]
    fn the_peers_max_entries_is_kept() {
        let mut s = Session::new(LIMITS);
        s.send_hello();
        s.on_hello(&Hello {
            max_entries: 100,
            ..hello(1, 1, 1)
        })
        .unwrap();
        assert_eq!(s.peer_max_entries(), 100);
    }

    /// SC-016's bookkeeping: the peer saturating its quota is refused past it,
    /// and that consumes none of the reply reserve this end's requests need.
    #[test]
    fn a_saturated_quota_cannot_take_the_reply_reserve() {
        let mut s = Session::new(LIMITS);
        assert_eq!(s.on_request(), Err(Violation::BeforeHello));
        s.send_hello();
        s.on_hello(&hello(4, 1, 1)).unwrap();
        s.on_hello_reply().unwrap();
        for _ in 0..3 {
            s.on_request().unwrap();
        }
        assert_eq!(s.on_request(), Err(Violation::OverQuota), "quota is 3");
        // Our own requests still have their full reserve.
        for _ in 0..4 {
            assert!(s.try_send_request());
        }
        for _ in 0..4 {
            s.on_reply().unwrap();
        }
        s.replied();
        s.on_request().unwrap();
        assert_eq!(s.inbound(), (3, 3));
        assert_eq!(LIMITS.receives(), 7, "reserve 4 + quota 3 posted");
    }

    #[test]
    fn unexpected_replies_are_violations() {
        let mut s = Session::new(LIMITS);
        s.send_hello();
        assert_eq!(
            s.on_reply(),
            Err(Violation::UnexpectedReply),
            "only the hello is out"
        );
        s.on_hello_reply().unwrap();
        assert_eq!(s.on_hello_reply(), Err(Violation::UnexpectedReply));
        s.on_hello(&hello(2, 1, 1)).unwrap();
        assert_eq!(s.on_reply(), Err(Violation::UnexpectedReply), "nothing out");
    }

    #[test]
    fn our_hello_advertises_our_quota_and_versions() {
        assert_eq!(
            LIMITS.hello(),
            Hello {
                max_entries: 340,
                max_outstanding_requests: 3,
                version_min: 1,
                version_max: 1,
            }
        );
    }
}
