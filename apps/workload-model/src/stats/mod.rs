//! What a workload *is*, computed from a reference stream and nothing else.
//!
//! Every statistic here is a property of the stream: no capacity parameter, no
//! replacement policy, no cache model (spec FR-034, FR-034a). A consumer reads
//! the reuse-distance CDF and derives whatever its own capacity would buy; this
//! crate does not do that arithmetic for it, and deliberately publishes no
//! hit-rate-at-a-capacity figure of any kind.
//!
//! # One definition, two sources
//!
//! These accumulators consume [`Ref`] — the minimum a statistic needs — rather
//! than a plan record, because the same statistics are computed over generated
//! plans (`certus-workload report`) and over real traces (`certus-trace fit`).
//! Two implementations would drift, and a `validate` comparing a fitted model
//! against the trace it was fitted from would then be comparing two different
//! definitions of reuse distance: a failure that looks like a success.
//!
//! # How warmup is handled, and why it is not simply dropped
//!
//! FR-045 excludes warmup operations from steady-state statistics. Dropping the
//! *references* outright would be wrong, and wrong in a way that inflates the
//! headline number: a key fetched during warmup and used in the measured window
//! would look like a first touch, so the compulsory-miss floor would count a
//! reference that warmup had already paid for.
//!
//! So warmup references are pushed all the way through the machinery — they
//! prime the seen-set and they occupy reuse distance, because a consumer really
//! did see them — but they contribute **no samples** to any published
//! distribution, and are counted separately. A key that warmup fetched is
//! therefore not a compulsory miss, which is the whole point of warming.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::keys::{CacheKey, SessionId};
use crate::plan::{flags, PlanEvent};

pub mod divergence;
pub mod floor;
pub mod hist;
pub mod length;
pub mod reuse_distance;
pub mod sharing;
pub mod trunk;
pub mod unique;
pub mod wss;

pub(crate) mod report;
mod text;
pub use report::{
    IntendedSharing, Provenance, Report, Statistics, WarmupCounts, Warning, WarningKind,
    NOTHING_RE_READ_FLOOR, SHARING_NOT_REALISED_OCCUPANCY,
};

/// One block reference: everything the statistics need and nothing else.
///
/// A real trace supplies the same six fields — session identity, position within
/// the request's block list as `depth`, the block id as `key` — so a statistic
/// cannot accidentally come to depend on something only a generated plan has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ref {
    /// The block referenced.
    pub key: CacheKey,
    /// Payload bytes.
    pub size: u32,
    /// Position in the prefix: trie depth for a plan, list index for a trace.
    pub depth: u32,
    /// Who asked. For a fan-out child this is the reader, not the minter.
    pub session: SessionId,
    /// First reference of a request.
    pub request_start: bool,
    /// Inside the warmup window (spec FR-045).
    pub warmup: bool,
}

impl From<&PlanEvent> for Ref {
    fn from(e: &PlanEvent) -> Ref {
        Ref {
            key: e.key,
            size: e.size,
            depth: e.depth,
            session: e.session_id,
            request_start: e.has(flags::REQUEST_START),
            warmup: e.has(flags::WARMUP),
        }
    }
}

/// A multiply-xor hasher for keys that are already hashes.
///
/// [`CacheKey`] is blake3 output and [`SessionId`] a dense counter, so the
/// avalanche work SipHash does is work already done. This is not a
/// micro-optimisation: the key table is touched once per event, and SC-004 puts
/// 10^7 events through it inside a minute on one core.
#[derive(Default)]
pub struct FastHasher(u64);

impl Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(u64::from(*b));
        }
    }

    fn write_u64(&mut self, n: u64) {
        // splitmix64's finaliser: enough diffusion for a table, no keying.
        let mut x = self.0 ^ n;
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        self.0 = x ^ (x >> 31);
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}

/// A [`HashMap`] over already-hashed keys.
pub type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

/// A [`std::collections::HashSet`] over already-hashed keys.
pub type FastSet<K> = std::collections::HashSet<K, BuildHasherDefault<FastHasher>>;

/// What the key table knew about a key at the moment it was referenced.
///
/// Handed to each statistic so that "have I seen this key before" is answered
/// once per event rather than once per statistic — both for speed and so that
/// the floor and the reuse-distance CDF cannot disagree about what a first
/// touch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyFacts {
    /// This reference's 1-based stream position.
    pub pos: u64,
    /// Stream position of the previous reference to this key, if any.
    ///
    /// Positions count **every** reference, warmup included, because a warmup
    /// reference really did occupy the consumer's capacity.
    pub prev_pos: Option<u64>,
    /// The size on record for this key — the one seen at its first touch.
    ///
    /// Byte-weighted statistics use this rather than the current reference's
    /// size, so that a stream which contradicts itself about a key's size
    /// produces a consistent total instead of a quietly drifting one. Under
    /// FR-011 the two always agree for a generated plan, since size is a pure
    /// function of key identity; a real trace disagreeing with itself is a real
    /// disagreement, not something to average away.
    pub entry_size: u32,
    /// First reference to this key anywhere in the stream — the compulsory-miss
    /// condition, and false for a key that warmup already fetched.
    pub first_touch: bool,
    /// First reference to this key within the measured window.
    pub first_steady_touch: bool,
    /// The session that referenced this key first.
    pub first_session: SessionId,
    /// This reference is the second distinct session to touch the key, so the
    /// key became observably shared just now.
    pub newly_shared: bool,
    /// The key is shared by at least two sessions as of this reference.
    pub shared: bool,
}

/// Per-key state. 32 bytes, so a few million distinct keys stay affordable.
#[derive(Debug, Clone, Copy)]
struct KeyEntry {
    last_pos: u64,
    first_session: SessionId,
    depth: u32,
    size: u32,
    shared: bool,
    steady_touched: bool,
}

/// The one table every statistic shares.
///
/// Owns the answer to "seen before?", "seen by whom?" and "how deep?" for every
/// distinct key in the stream. Memory is O(distinct keys), which is the only
/// unavoidable cost in the whole report — a reference stream's distinct-key set
/// is not summarisable.
#[derive(Debug, Default)]
pub struct KeyTable {
    keys: FastMap<CacheKey, KeyEntry>,
    pos: u64,
    steady_distinct: u64,
    steady_distinct_bytes: u128,
}

impl KeyTable {
    /// An empty table.
    pub fn new() -> KeyTable {
        KeyTable::default()
    }

    /// Record `r` at the next stream position and report what was already known.
    pub fn observe(&mut self, r: &Ref) -> KeyFacts {
        self.pos += 1;
        let pos = self.pos;
        let steady = !r.warmup;
        match self.keys.get_mut(&r.key) {
            Some(e) => {
                let prev = e.last_pos;
                e.last_pos = pos;
                let newly_shared = !e.shared && e.first_session != r.session;
                if newly_shared {
                    e.shared = true;
                }
                let first_steady_touch = steady && !e.steady_touched;
                if first_steady_touch {
                    e.steady_touched = true;
                    self.steady_distinct += 1;
                    self.steady_distinct_bytes += u128::from(e.size);
                }
                KeyFacts {
                    pos,
                    prev_pos: Some(prev),
                    entry_size: e.size,
                    first_touch: false,
                    first_steady_touch,
                    first_session: e.first_session,
                    newly_shared,
                    shared: e.shared,
                }
            }
            None => {
                self.keys.insert(
                    r.key,
                    KeyEntry {
                        last_pos: pos,
                        first_session: r.session,
                        depth: r.depth,
                        size: r.size,
                        shared: false,
                        steady_touched: steady,
                    },
                );
                if steady {
                    self.steady_distinct += 1;
                    self.steady_distinct_bytes += u128::from(r.size);
                }
                KeyFacts {
                    pos,
                    prev_pos: None,
                    entry_size: r.size,
                    first_touch: true,
                    first_steady_touch: steady,
                    first_session: r.session,
                    newly_shared: false,
                    shared: false,
                }
            }
        }
    }

    /// References observed so far, warmup included.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Distinct keys referenced in the measured window.
    pub fn steady_distinct_keys(&self) -> u64 {
        self.steady_distinct
    }

    /// Sum of entry sizes over the distinct keys of the measured window.
    ///
    /// The bytes a consumer would have to hold to keep the whole measured
    /// working set — distinct from total bytes *transferred*, which counts every
    /// reference.
    pub fn steady_distinct_bytes(&self) -> u128 {
        self.steady_distinct_bytes
    }

    /// Distinct keys anywhere in the stream, warmup included.
    pub fn distinct_keys(&self) -> u64 {
        self.keys.len() as u64
    }

    /// Every distinct key with its depth, size and observed sharing.
    ///
    /// Used by the run-wide aggregations that can only be computed once the
    /// whole stream is in — a key's sharing is not known until the last session
    /// that touches it has been seen.
    pub fn iter(&self) -> impl Iterator<Item = (CacheKey, u32, u32, bool)> + '_ {
        self.keys
            .iter()
            .map(|(k, e)| (*k, e.depth, e.size, e.shared))
    }
}

/// Per-key state within one window.
#[derive(Debug, Clone, Copy)]
struct WinEntry {
    depth: u32,
    size: u32,
    /// Distinct sessions that referenced this key **within this window**.
    sessions: u32,
    /// The first session to reach it in this window. Kept so that "some *other*
    /// session has been here" is answerable exactly at one session's cost.
    first_session: SessionId,
}

/// The distinct-key state of one `run.wss_window`.
///
/// Three statistics are defined over a window rather than over the run — the
/// prefix-sharing depth histogram, trunk occupancy, and the working-set size —
/// and FR-009h makes that window a **request count** rather than a time span. All
/// three read the same table, so they cannot come to disagree about which
/// references were in the window.
///
/// The window is why sharing is a physical claim at all: counted over a whole run
/// instead, a configuration could "achieve" sharing merely by running longer.
#[derive(Debug, Default)]
pub struct WindowTable {
    keys: FastMap<CacheKey, WinEntry>,
    /// Distinct `(key, session)` pairs, mixed into one word.
    pairs: FastSet<u64>,
    /// The open request's references, held back so that "seen already" means
    /// seen in an *earlier* request — which is what the longest-common-prefix
    /// definition of realised sharing requires.
    current: Vec<Ref>,
    requests: u64,
    references: u64,
    bytes: u128,
}

impl WindowTable {
    /// An empty window.
    pub fn new() -> WindowTable {
        WindowTable::default()
    }

    /// Whether `key` was referenced by an **earlier** request in this window.
    pub fn seen_in_earlier_request(&self, key: CacheKey) -> bool {
        self.keys.contains_key(&key)
    }

    /// Whether an earlier request in this window from a **different** session
    /// referenced `key`.
    ///
    /// The distinction is the whole content of the prefix-sharing statistic.
    /// `shared_depth` means *inter*-session sharing: the length of the trunk a
    /// session walks in common with **other** sessions. A session's own later
    /// turns re-walk its entire path by construction (FR-014a), so counting them
    /// would report a multi-turn session's turn 2 as sharing its whole turn-1
    /// prefix — turning the statistic into a measure of multi-turn structure and
    /// inflating realised sharing far past anything `shared_depth` could mean.
    pub fn seen_in_earlier_request_by_other(&self, key: CacheKey, session: SessionId) -> bool {
        match self.keys.get(&key) {
            None => false,
            Some(e) => e.sessions >= 2 || e.first_session != session,
        }
    }

    /// Buffer one reference into the open request.
    pub fn observe(&mut self, r: &Ref) {
        self.current.push(*r);
    }

    /// Close the open request, folding its references into the window.
    pub fn end_request(&mut self) {
        if self.current.is_empty() {
            return;
        }
        self.requests += 1;
        for r in std::mem::take(&mut self.current) {
            self.references += 1;
            self.bytes += u128::from(r.size);
            let fresh_pair = self.pairs.insert(mix_pair(r.key, r.session));
            let e = self.keys.entry(r.key).or_insert(WinEntry {
                depth: r.depth,
                size: r.size,
                sessions: 0,
                first_session: r.session,
            });
            if fresh_pair {
                e.sessions += 1;
            }
        }
    }

    /// References buffered into the request that is still open.
    pub fn open_references(&self) -> usize {
        self.current.len()
    }

    /// Requests closed in this window.
    pub fn requests(&self) -> u64 {
        self.requests
    }

    /// References in this window.
    pub fn references(&self) -> u64 {
        self.references
    }

    /// Distinct keys in this window.
    pub fn distinct_keys(&self) -> u64 {
        self.keys.len() as u64
    }

    /// Summed entry size over the distinct keys of this window — the bytes a
    /// consumer would have to hold to keep the whole window resident.
    pub fn distinct_bytes(&self) -> u128 {
        self.keys.values().map(|e| u128::from(e.size)).sum()
    }

    /// Bytes referenced in this window, counting every reference.
    pub fn bytes(&self) -> u128 {
        self.bytes
    }

    /// Every distinct key as `(depth, distinct sessions)`.
    pub fn by_depth(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.keys.values().map(|e| (e.depth, e.sessions))
    }

    /// Empty the window, keeping the allocations for the next one.
    pub fn reset(&mut self) {
        self.keys.clear();
        self.pairs.clear();
        self.current.clear();
        self.requests = 0;
        self.references = 0;
        self.bytes = 0;
    }

    /// Whether anything has been recorded.
    pub fn is_empty(&self) -> bool {
        self.requests == 0 && self.current.is_empty()
    }
}

/// Mix a key and a session into one word for distinct-pair counting.
///
/// Both inputs are already well-distributed, so one splitmix round over their
/// combination is enough. Collisions would undercount a pair; at 64 bits and the
/// millions of pairs a window holds, the expected undercount is far below the
/// resolution of any figure derived from it.
fn mix_pair(key: CacheKey, session: SessionId) -> u64 {
    let mut h = FastHasher::default();
    h.write_u64(key.0);
    h.write_u32(session.0);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(key: u64, session: u32, depth: u32) -> Ref {
        Ref {
            key: CacheKey(key),
            size: 1024,
            depth,
            session: SessionId(session),
            request_start: depth == 0,
            warmup: false,
        }
    }

    fn warm(key: u64, session: u32, depth: u32) -> Ref {
        Ref {
            warmup: true,
            ..r(key, session, depth)
        }
    }

    #[test]
    fn a_first_touch_is_reported_once() {
        let mut t = KeyTable::new();
        assert!(t.observe(&r(1, 0, 0)).first_touch);
        assert!(!t.observe(&r(1, 0, 0)).first_touch);
        assert_eq!(t.distinct_keys(), 1);
    }

    #[test]
    fn warmup_primes_the_seen_set_so_a_warmed_key_is_not_a_compulsory_miss() {
        // The FR-045 subtlety: excluding warmup from the *samples* must not
        // exclude it from the *history*, or warming would inflate the floor.
        let mut t = KeyTable::new();
        assert!(t.observe(&warm(7, 0, 0)).first_touch);
        let f = t.observe(&r(7, 0, 0));
        assert!(
            !f.first_touch,
            "a warmed key must not read as a first touch"
        );
        assert!(f.first_steady_touch, "but it is new to the measured window");
        assert_eq!(t.steady_distinct_keys(), 1);
    }

    #[test]
    fn previous_position_counts_warmup_references_too() {
        // A warmup reference occupied real capacity, so it sits inside the reuse
        // distance of whatever follows it.
        let mut t = KeyTable::new();
        t.observe(&r(1, 0, 0));
        t.observe(&warm(2, 0, 0));
        let f = t.observe(&r(1, 0, 0));
        assert_eq!(f.prev_pos, Some(1));
        assert_eq!(t.position(), 3);
    }

    #[test]
    fn sharing_is_established_by_the_second_distinct_session_and_names_the_first() {
        // Both sessions must be attributable at that instant: the first is
        // remembered on the key, which is what lets a one-pass sharing histogram
        // credit a session that has already retired.
        let mut t = KeyTable::new();
        let a = t.observe(&r(9, 1, 0));
        assert!(!a.shared && !a.newly_shared);
        let b = t.observe(&r(9, 2, 0));
        assert!(
            b.newly_shared,
            "second distinct session establishes sharing"
        );
        assert_eq!(b.first_session, SessionId(1));
        let c = t.observe(&r(9, 3, 0));
        assert!(c.shared && !c.newly_shared, "already shared, not newly so");
    }

    #[test]
    fn a_session_revisiting_its_own_key_does_not_make_it_shared() {
        let mut t = KeyTable::new();
        t.observe(&r(4, 1, 0));
        let f = t.observe(&r(4, 1, 0));
        assert!(!f.shared && !f.newly_shared);
    }

    #[test]
    fn distinct_bytes_counts_each_key_once_however_often_it_is_referenced() {
        let mut t = KeyTable::new();
        for _ in 0..5 {
            t.observe(&r(1, 0, 0));
        }
        t.observe(&r(2, 0, 0));
        assert_eq!(t.steady_distinct_bytes(), 2048);
        assert_eq!(t.steady_distinct_keys(), 2);
    }

    #[test]
    fn a_plan_event_converts_without_losing_what_a_statistic_needs() {
        let e = PlanEvent {
            t_ns: 5,
            key: CacheKey(3),
            size: 4096,
            request_id: 1,
            session_id: SessionId(2),
            depth: 6,
            turn: 1,
            node: 0,
            mix_index: 0,
            flags: flags::REQUEST_START | flags::WARMUP,
        };
        let got = Ref::from(&e);
        assert_eq!(got.key, CacheKey(3));
        assert_eq!(got.size, 4096);
        assert_eq!(got.depth, 6);
        assert_eq!(got.session, SessionId(2));
        assert!(got.request_start);
        assert!(got.warmup);
    }
}
