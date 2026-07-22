//! Per-`op_id` operation state machine and completion criteria.
//!
//! One [`Operation`] tracks a single `batch_lookup`: per-key status, cached peer
//! replies, phase, retry rounds, deadline, and the one-shot back to the blocked
//! caller. All state lives on the actor thread (research Decision 2). Landing
//! slots are reserved privately and published to dispatch-map only on RDMA
//! success (publish-on-success, research Decision 5). Single-flight coalescing
//! is enforced here via a per-key in-flight index.

use std::collections::HashMap;
use std::time::Instant;

use interfaces::{CacheKey, PeerId, RemoteLookupError};

use crate::actor::{BatchResult, OneShot};
use crate::wire::Avail;

/// Per-key progress within an [`Operation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyState {
    /// No peer is currently fetching this key.
    Unsatisfied,
    /// An RDMA_REQUEST is outstanding to some peer for this key.
    InProgress,
    /// The value has been RDMA-written and published locally.
    Satisfied,
}

/// Which phase of the lookup an [`Operation`] is in (FR-005/FR-010).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Fetching memory hits only; waiting for a quorum of replies or the
    /// Phase-1 timeout before falling back to disk.
    Phase1,
    /// Post-transition: disk holders are eligible (served via promotion).
    Phase2,
}

/// A private landing slot reserved in the local memory tier for one in-flight
/// key (publish-on-success): allocated via `IMemoryTier::insert`, advertised to
/// the serving `peer`, and published to dispatch-map only on RDMA success.
pub(crate) struct LandingSlot {
    /// Requester pool address the value must land at (the reserved pointer).
    pub addr: u64,
    /// Reserved length in bytes (== requested size).
    pub len: u32,
    /// The peer the slot was exposed to (its initiator writes here).
    pub peer: PeerId,
}

/// One peer's cached KEY_RESPONSE: its per-key availability, kept for Phase-2
/// disk re-scan (US4) and retry re-targeting (US5).
pub(crate) struct PeerReply {
    /// Per-key `(size, availability)` this peer reported.
    pub entries: Vec<(CacheKey, u32, Avail)>,
}

/// State for a single in-flight `batch_lookup`, keyed by `op_id`.
pub(crate) struct Operation {
    /// Requested `(key, size)` entries, in caller order (drives the positional
    /// result vector).
    pub entries: Vec<(CacheKey, u32)>,
    /// Per-key progress.
    pub state: HashMap<CacheKey, KeyState>,
    /// Reserved landing slots by key (present once an RDMA_REQUEST is sent).
    pub slots: HashMap<CacheKey, LandingSlot>,
    /// Cached replies by peer (for Phase-2 / retry).
    pub replies: HashMap<PeerId, PeerReply>,
    /// Peers already tried (and failed) per key, so retries pick alternates.
    pub tried: HashMap<CacheKey, Vec<PeerId>>,
    /// Finalize no later than this instant (FR-005, `op_deadline`).
    pub deadline: Instant,
    /// Transition to Phase 2 no later than this instant (FR-010, `phase1_timeout`).
    pub phase1_deadline: Instant,
    /// Current phase; Phase 2 admits disk holders (FR-010).
    pub phase: Phase,
    /// Group size snapshot at SHOUT time (Phase-1 quorum denominator).
    pub peers_expected: usize,
    /// Distinct peers that have replied so far.
    pub peers_replied: usize,
    /// One-shot back to the blocked caller; taken exactly once on finalize.
    pub done: Option<OneShot<BatchResult>>,
}

impl Operation {
    /// Create an operation with every key `Unsatisfied`.
    pub(crate) fn new(
        entries: Vec<(CacheKey, u32)>,
        deadline: Instant,
        phase1_deadline: Instant,
        peers_expected: usize,
        done: OneShot<BatchResult>,
    ) -> Self {
        let state = entries
            .iter()
            .map(|(k, _)| (*k, KeyState::Unsatisfied))
            .collect();
        Self {
            entries,
            state,
            slots: HashMap::new(),
            replies: HashMap::new(),
            tried: HashMap::new(),
            deadline,
            phase1_deadline,
            phase: Phase::Phase1,
            peers_expected,
            peers_replied: 0,
            done: Some(done),
        }
    }

    /// Whether a quorum of expected peers has replied (FR-010). With no expected
    /// peers the operation short-circuits elsewhere, so treat that as met.
    pub(crate) fn quorum_reached(&self, quorum_pct: u8) -> bool {
        self.peers_expected == 0
            || self.peers_replied * 100 >= self.peers_expected * quorum_pct as usize
    }

    /// Requested size for `key`, if it is part of this operation.
    pub(crate) fn size_of(&self, key: CacheKey) -> Option<u32> {
        self.entries
            .iter()
            .find_map(|(k, s)| (*k == key).then_some(*s))
    }

    /// Current state of `key` (defaults to `Unsatisfied` if not tracked).
    pub(crate) fn state_of(&self, key: CacheKey) -> KeyState {
        self.state
            .get(&key)
            .copied()
            .unwrap_or(KeyState::Unsatisfied)
    }

    /// Set `key`'s state.
    pub(crate) fn set_state(&mut self, key: CacheKey, s: KeyState) {
        if let Some(slot) = self.state.get_mut(&key) {
            *slot = s;
        }
    }

    /// Whether every key has reached `Satisfied`.
    pub(crate) fn all_satisfied(&self) -> bool {
        self.state.values().all(|s| *s == KeyState::Satisfied)
    }

    /// The positional result vector: `Ok(())` for satisfied keys, else
    /// `Err(NotFound)`, in the original request order.
    pub(crate) fn results(&self) -> BatchResult {
        self.entries
            .iter()
            .map(|(k, _)| {
                if self.state_of(*k) == KeyState::Satisfied {
                    Ok(())
                } else {
                    Err(RemoteLookupError::NotFound)
                }
            })
            .collect()
    }

    /// Record that `peer` was tried (and failed) for `key`.
    pub(crate) fn note_tried(&mut self, key: CacheKey, peer: &PeerId) {
        let v = self.tried.entry(key).or_default();
        if !v.iter().any(|p| p == peer) {
            v.push(peer.clone());
        }
    }

    /// Whether `peer` has already been tried for `key`.
    pub(crate) fn already_tried(&self, key: CacheKey, peer: &PeerId) -> bool {
        self.tried
            .get(&key)
            .is_some_and(|v| v.iter().any(|p| p == peer))
    }
}
