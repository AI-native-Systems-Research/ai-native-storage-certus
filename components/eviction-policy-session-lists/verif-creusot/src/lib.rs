#![recursion_limit = "16384"]
//! Creusot verification mirror for `components/eviction-policy-session-lists`.
//!
//! The shipped component keeps its per-pool state in `RwLock<Vec<Mutex<Pool>>>`
//! (src/lib.rs:80-87) and each `Pool` (src/session_list.rs:39-56) in an index-based arena
//! `Vec<Node>` plus a `Vec<u32>` free list, two `HashMap`s (`by_key`, `sessions`), a
//! `BTreeSet<(u64,u32)>` of current leaves, a `u64` clock and a `usize` count.
//!
//! Creusot cannot translate `Mutex`/`RwLock` and std's `HashMap`/`BTreeSet` carry no logical
//! insert/remove/get specs (known defeat KD-STD-CONTAINER-NO-SPECS), so this crate proves a
//! standalone, line-faithful **ghost mirror** of the pure logic:
//!   * `Node`    — field for field with `session_list::Node` (src/session_list.rs:24-37);
//!   * `Map`     — `HashMap<u64,u32>` modelled as an association list `Vec<(key,value)>` whose
//!                 keys are distinct (`map_distinct`). `map_find`/`map_insert`/`map_remove`
//!                 reproduce `get`/`insert`/`remove` exactly, with full functional contracts;
//!   * `Leaves`  — `BTreeSet<(u64,u32)>` modelled as a `Vec<(u64,u32)>` held in strictly
//!                 ascending lexicographic order (`leaves_sorted`), which is precisely the
//!                 BTreeSet's iteration order, so `iter().next()` is `e[0]` and
//!                 `iter().take(n)` is the length-`n` prefix;
//!   * `Pool`    — field for field with `session_list::Pool`;
//!   * `Pools`   — the `Vec<Mutex<Pool>>` + `announced` latch of `EvictionState`, minus the locks
//!                 (the locks are a trusted boundary; the routing and per-pool frame logic they
//!                 protect are proved here over the plain `Vec`);
//!   * `Log` / `LockTrace` — observation counters standing in for the `ILogger` receptacle and
//!                 for the lock-acquire/release discipline of `batch_touch`.
//!
//! Each `verify_<ID>` fn is a driver whose contract is the obligation of the unified-inventory
//! property `<ID>`; its `verify_<ID>__mutant` twin states a deliberately FALSE contract over the
//! identical precondition, which MUST fail (anti-vacuity: were the precondition unsatisfiable or
//! the postcondition trivial, the twin would prove too). A `refute_<ID>` fn states the NEGATION
//! of an obligation the code violates and PROVES it.
//!
//! Honest fidelity boundary (recorded per id in `../verif/creusot_advisory.yaml`): the *ancestor
//! walk* (following `parent` links to a chain head) is not a first-order SMT notion, so
//! acyclicity is carried by a fuel-bounded recursive logic function rather than by a reachability
//! predicate, and the mutual-link invariant `links_agree` is an assumed precondition wherever a
//! mutator needs it.

use creusot_std::prelude::*;

// ===========================================================================
// Mirror types
// ===========================================================================

/// Mirror of `session_list::Node` (src/session_list.rs:24-37), field for field.
pub struct Node {
    pub key: u64,
    pub session: u64,
    pub parent: Option<u32>,
    pub child: Option<u32>,
    pub stamp: u64,
    /// GHOST/AUDIT FIELD — NOT present in `session_list::Node`. It records the stamp the block was
    /// given at REGISTRATION, which the shipped code computes (session_list.rs:96) and then throws
    /// away the moment `touch` overwrites `stamp`. Nothing ever reads it to decide anything, so it
    /// cannot change behaviour; it exists only to carry the acyclicity measure. A block's parent
    /// was registered strictly earlier, so `birth` strictly DECREASES along `parent`
    /// (`chain_birth_decreases`), and a strictly decreasing measure bounded below is exactly the
    /// well-foundedness argument — the first-order content of `EPSL-INV-NO-CYCLES-IN-LINEAGE`. It
    /// also rules out a block being its own parent, or two blocks being each other's parent.
    pub birth: u64,
    pub active: bool,
}

/// Mirror of a `HashMap<u64, u32>` (`Pool::by_key`, `Pool::sessions`) as an association list
/// with distinct keys. `creusot-std` gives `HashMap` a `FMap` view but ships specs for iteration
/// only, so the mutators have no semantics; this list carries them explicitly.
pub struct Map {
    pub e: Vec<(u64, u32)>,
}

/// Mirror of `Pool::leaves: BTreeSet<(u64, u32)>` as a strictly ascending `Vec<(u64,u32)>`.
pub struct Leaves {
    pub e: Vec<(u64, u32)>,
}

/// Mirror of `session_list::Pool` (src/session_list.rs:39-56), field for field.
pub struct Pool {
    pub nodes: Vec<Node>,
    pub free: Vec<u32>,
    pub by_key: Map,
    pub sessions: Map,
    pub leaves: Leaves,
    pub clock: u64,
    pub len: usize,
}

/// Mirror of `EvictionState` (src/lib.rs:80-87) minus the `Mutex`/`RwLock` wrappers.
pub struct Pools {
    pub pools: Vec<Pool>,
    pub announced: bool,
}

/// Mirror of `interfaces::EvictionHandle` (pool id + arena index, no generation counter).
/// `Copy`, exactly like the shipped `EvictionHandle`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Handle {
    pub pool: u32,
    pub index: u32,
}

/// Mirror of `interfaces::EvictionPolicyError`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum PolicyError {
    InvalidPool(u32),
    InvalidHandle,
}

/// Observation counters for the `ILogger` receptacle (src/lib.rs:107-119, 131-136).
pub struct Log {
    pub info: usize,
    pub debug: usize,
    pub warn: usize,
}

/// Observation counters for lock discipline: `held` is the number of per-pool `Mutex` guards
/// alive right now, `peak` the maximum ever held at once, `writes`/`reads` the number of
/// `RwLock::write`/`read` acquisitions.
pub struct LockTrace {
    pub held: usize,
    pub peak: usize,
    /// Total number of per-pool `Mutex::lock` acquisitions (how many times the group refresh
    /// re-locked).
    pub acquires: usize,
    pub writes: usize,
    pub reads: usize,
}

// ===========================================================================
// Map (HashMap mirror) — logic model
// ===========================================================================

/// Keys of the association list are pairwise distinct — the defining property of a map.
#[logic]
pub fn map_distinct(m: Seq<(u64, u32)>) -> bool {
    pearlite! { forall<i: Int, j: Int> 0 <= i && i < j && j < m.len() ==> (m[i]).0 != (m[j]).0 }
}

/// `k` is bound at all.
#[logic]
pub fn map_mem(m: Seq<(u64, u32)>, k: u64) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < m.len() && (m[i]).0 == k }
}

/// `k` is bound to `v`.
#[logic]
pub fn map_has(m: Seq<(u64, u32)>, k: u64, v: u32) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < m.len() && (m[i]).0 == k && (m[i]).1 == v }
}

// ===========================================================================
// Leaves (BTreeSet mirror) — logic model
// ===========================================================================

/// Strict lexicographic order on `(stamp, index)` — exactly `BTreeSet<(u64,u32)>`'s order.
#[logic]
pub fn pair_lt(a: (u64, u32), b: (u64, u32)) -> bool {
    pearlite! { a.0@ < b.0@ || (a.0@ == b.0@ && a.1@ < b.1@) }
}

/// The mirror list is in strictly ascending order, so it is duplicate-free and its element `i`
/// is what `BTreeSet::iter()` yields at step `i`.
#[logic]
pub fn leaves_sorted(l: Seq<(u64, u32)>) -> bool {
    pearlite! { forall<i: Int, j: Int> 0 <= i && i < j && j < l.len() ==> pair_lt(l[i], l[j]) }
}

/// Set membership.
#[logic]
pub fn leaves_mem(l: Seq<(u64, u32)>, x: (u64, u32)) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < l.len() && l[i] == x }
}

/// The index part of some entry equals `idx` (i.e. slot `idx` is currently eviction-eligible).
#[logic]
pub fn leaves_has_idx(l: Seq<(u64, u32)>, idx: u32) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < l.len() && (l[i]).1 == idx }
}

/// `leaves_has_idx` with the position given as an `Int`.
#[logic]
pub fn leaves_has_idx_i(l: Seq<(u64, u32)>, idx: Int) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < l.len() && ((l[i]).1)@ == idx }
}

// ---- Int-indexed spellings. Pearlite has no `Int as u32` cast, so quantifying an arena
// ---- position as an `Int` needs these rather than building a `u32` value.

/// `map_has` with the value given as an `Int` arena position.
#[logic]
pub fn map_has_i(m: Seq<(u64, u32)>, k: u64, v: Int) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < m.len() && (m[i]).0 == k && ((m[i]).1)@ == v }
}

/// `leaves_mem` with the entry given as `(stamp, Int position)`.
#[logic]
pub fn leaves_mem_i(l: Seq<(u64, u32)>, s: u64, idx: Int) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < l.len() && (l[i]).0 == s && ((l[i]).1)@ == idx }
}

/// `o == Some(v)` with `v` an `Int` arena position.
#[logic]
pub fn opt_is(o: Option<u32>, v: Int) -> bool {
    pearlite! { match o { Some(x) => x@ == v, None => false } }
}

// ===========================================================================
// Map primitives — `HashMap::get` / `insert` / `remove` / `clear`
// ===========================================================================

/// Position of `k`'s binding, or `None`. Mirrors `HashMap::get`'s outcome.
#[ensures(match result {
    Some(i) => i@ < m.e@.len() && (m.e@[i@]).0 == k,
    None => forall<j: Int> 0 <= j && j < m.e@.len() ==> (m.e@[j]).0 != k })]
pub fn map_find(m: &Map, k: u64) -> Option<usize> {
    let mut i: usize = 0;
    let n = m.e.len();
    #[invariant(i@ <= n@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> (m.e@[j]).0 != k)]
    while i < n {
        if m.e[i].0 == k {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `HashMap::get`: the value bound to `k`, if any.
#[requires(map_distinct(m.e@))]
#[ensures(match result { Some(v) => map_has(m.e@, k, v), None => !map_mem(m.e@, k) })]
pub fn map_get(m: &Map, k: u64) -> Option<u32> {
    match map_find(m, k) {
        Some(i) => Some(m.e[i].1),
        None => None,
    }
}

/// `HashMap::insert`: bind `k` to `v`, replacing any existing binding in place.
#[requires(map_distinct(m.e@))]
#[ensures(map_distinct((^m).e@))]
#[ensures(map_has((^m).e@, k, v))]
#[ensures(map_mem((*m).e@, k) ==> (^m).e@.len() == (*m).e@.len())]
#[ensures(!map_mem((*m).e@, k) ==> (^m).e@.len() == (*m).e@.len() + 1)]
#[ensures((^m).e@.len() >= (*m).e@.len())]
#[ensures(forall<j: Int> 0 <= j && j < (*m).e@.len() && ((*m).e@[j]).0 != k
             ==> (^m).e@[j] == (*m).e@[j])]
#[ensures(forall<k2: u64> k2 != k ==> (map_mem((^m).e@, k2) == map_mem((*m).e@, k2)))]
#[ensures(forall<k2: u64, v2: u32> k2 != k
             ==> (map_has((^m).e@, k2, v2) == map_has((*m).e@, k2, v2)))]
// Goal-shaped preservation, for the `map_has_i` form every invariant is written in.
#[ensures(map_has_i((^m).e@, k, v@))]
#[ensures(forall<k2: u64, v2: Int> k2 != k && map_has_i((*m).e@, k2, v2)
             ==> map_has_i((^m).e@, k2, v2))]
#[ensures(forall<k2: u64, v2: Int> k2 != k && map_has_i((^m).e@, k2, v2)
             ==> map_has_i((*m).e@, k2, v2))]
// Positional provenance, existential-free. `idx_ok`, `by_key_entries_live` and
// `session_entries_ok` all quantify over a POSITION of the resulting list and then look up
// `nodes[list[k].1]`, so the only hypothesis they can consume is "the pair now at position j is the
// inserted pair, or the pair that was already at j". The `map_has_i` membership clauses above and a
// `forall<b: Int>` bound clause both make the solver invent a witness first, and it does not.
#[ensures((^m).e@.len() == (*m).e@.len() || (^m).e@.len() == (*m).e@.len() + 1)]
#[ensures(forall<j: Int> 0 <= j && j < (*m).e@.len() ==>
             (^m).e@[j] == (*m).e@[j] || (^m).e@[j] == (k, v))]
#[ensures((^m).e@.len() > (*m).e@.len() ==> (^m).e@[(*m).e@.len()] == (k, v))]
pub fn map_insert(m: &mut Map, k: u64, v: u32) {
    match map_find(m, k) {
        Some(i) => {
            m.e[i] = (k, v);
        }
        None => {
            m.e.push((k, v));
        }
    }
}

/// `HashMap::remove`: drop `k`'s binding. Implemented as "overwrite the hole with the last entry,
/// then pop" — a map has no order to preserve, so this is faithful and keeps the proof
/// positional.
#[requires(map_distinct(m.e@))]
#[ensures(map_distinct((^m).e@))]
#[ensures(!map_mem((^m).e@, k))]
#[ensures(map_mem((*m).e@, k) ==> (^m).e@.len() == (*m).e@.len() - 1)]
#[ensures(!map_mem((*m).e@, k) ==> (^m).e@ == (*m).e@)]
#[ensures((^m).e@.len() <= (*m).e@.len())]
#[ensures(forall<k2: u64> k2 != k ==> (map_mem((^m).e@, k2) == map_mem((*m).e@, k2)))]
#[ensures(forall<k2: u64, v2: u32> k2 != k
             ==> (map_has((^m).e@, k2, v2) == map_has((*m).e@, k2, v2)))]
// Goal-shaped preservation; plus: after removal `k` is bound to nothing at all.
#[ensures(forall<k2: u64, v2: Int> k2 != k && map_has_i((*m).e@, k2, v2)
             ==> map_has_i((^m).e@, k2, v2))]
#[ensures(forall<k2: u64, v2: Int> map_has_i((^m).e@, k2, v2) ==> map_has_i((*m).e@, k2, v2))]
#[ensures(forall<v2: Int> !map_has_i((^m).e@, k, v2))]
// value-bound preservation, so `idx_ok` survives a removal
#[ensures(forall<b: Int>
             (forall<j: Int> 0 <= j && j < (*m).e@.len() ==> (((*m).e@[j]).1)@ < b)
             ==> (forall<j: Int> 0 <= j && j < (^m).e@.len() ==> (((^m).e@[j]).1)@ < b))]
// Positional provenance, existential-free (see `map_insert`). `remove` fills the hole with the last
// entry, so the pair now at position j is either the pair that was at j or the pair that was last —
// nothing else can appear. This is what carries the position-indexed invariants across a removal.
#[ensures((^m).e@.len() == (*m).e@.len() || (^m).e@.len() == (*m).e@.len() - 1)]
#[ensures(forall<j: Int> 0 <= j && j < (^m).e@.len() ==>
             (^m).e@[j] == (*m).e@[j] || (^m).e@[j] == (*m).e@[(*m).e@.len() - 1])]
pub fn map_remove(m: &mut Map, k: u64) {
    match map_find(m, k) {
        Some(i) => {
            let n = m.e.len();
            let last = m.e[n - 1];
            m.e[i] = last;
            let _ = m.e.pop();
        }
        None => {}
    }
}

/// `HashMap::clear`.
#[ensures((^m).e@.len() == 0)]
#[ensures(map_distinct((^m).e@))]
#[ensures(forall<k: u64> !map_mem((^m).e@, k))]
pub fn map_clear(m: &mut Map) {
    m.e.clear();
}

// ===========================================================================
// Leaves primitives — `BTreeSet::insert` / `remove` / `iter().next()` / `iter().take(n)`
// ===========================================================================

/// The sorted insertion point for `x`: the number of entries strictly below it.
#[requires(leaves_sorted(l.e@))]
#[ensures(result@ <= l.e@.len())]
#[ensures(forall<j: Int> 0 <= j && j < result@ ==> pair_lt(l.e@[j], x))]
#[ensures(forall<j: Int> result@ <= j && j < l.e@.len() ==> !pair_lt(l.e@[j], x))]
pub fn leaves_pos(l: &Leaves, x: (u64, u32)) -> usize {
    let mut i: usize = 0;
    let n = l.e.len();
    #[invariant(i@ <= n@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> pair_lt(l.e@[j], x))]
    while i < n {
        let y = l.e[i];
        if y.0 < x.0 || (y.0 == x.0 && y.1 < x.1) {
            i += 1;
        } else {
            return i;
        }
    }
    n
}

/// `BTreeSet::insert(x)` — idempotent, keeps ascending order.
#[requires(leaves_sorted(l.e@))]
#[ensures(leaves_sorted((^l).e@))]
#[ensures(leaves_mem((^l).e@, x))]
#[ensures(forall<y: (u64, u32)> leaves_mem((*l).e@, y) ==> leaves_mem((^l).e@, y))]
#[ensures(forall<y: (u64, u32)> leaves_mem((^l).e@, y) ==> y == x || leaves_mem((*l).e@, y))]
#[ensures(leaves_mem((*l).e@, x) ==> (^l).e@ == (*l).e@)]
#[ensures(!leaves_mem((*l).e@, x) ==> (^l).e@.len() == (*l).e@.len() + 1)]
#[ensures((^l).e@.len() >= (*l).e@.len())]
// Every entry of the result is either `x` or one of the entries that were already there. This is
// the directly instantiable form of index-bound preservation: it carries `idx_ok` across an insert
// without the solver having to guess a bound.
#[ensures(forall<k: Int> 0 <= k && k < (^l).e@.len() ==>
             (^l).e@[k] == x || leaves_mem((*l).e@, (^l).e@[k]))]
#[ensures(forall<b: Int>
             (forall<j: Int> 0 <= j && j < (*l).e@.len() ==> (((*l).e@[j]).1)@ < b) && (x.1)@ < b
             ==> (forall<j: Int> 0 <= j && j < (^l).e@.len() ==> (((^l).e@[j]).1)@ < b))]
// Goal-shaped preservation: `leaf_in_set` is phrased with `leaves_mem_i`, so state it that way.
#[ensures(leaves_mem_i((^l).e@, x.0, (x.1)@))]
#[ensures(forall<s: u64, ix: Int> leaves_mem_i((*l).e@, s, ix) ==> leaves_mem_i((^l).e@, s, ix))]
// and nothing appears that was not there before, except `x` itself
#[ensures(forall<y: (u64, u32)> !leaves_mem((*l).e@, y) && y != x ==> !leaves_mem((^l).e@, y))]
#[ensures(forall<s: u64, ix: Int> !leaves_mem_i((*l).e@, s, ix)
             && !(s == x.0 && ix == (x.1)@) ==> !leaves_mem_i((^l).e@, s, ix))]
// Positional provenance, existential-free (see `map_insert`): a sorted insert shifts the tail right
// by one, so the entry now at position k is `x`, the entry that was at k, or the entry that was at
// k-1. `idx_ok` reads the candidate list by position, so it needs this and not `leaves_mem`.
#[ensures(forall<k: Int> 0 <= k && k < (^l).e@.len() ==>
             (^l).e@[k] == x
             || (k < (*l).e@.len() && (^l).e@[k] == (*l).e@[k])
             || (1 <= k && k - 1 < (*l).e@.len() && (^l).e@[k] == (*l).e@[k - 1]))]
pub fn leaves_insert(l: &mut Leaves, x: (u64, u32)) {
    let p = leaves_pos(l, x);
    if p < l.e.len() && l.e[p].0 == x.0 && l.e[p].1 == x.1 {
        return;
    }
    l.e.insert(p, x);
}

/// `BTreeSet::remove(x)`.
#[requires(leaves_sorted(l.e@))]
#[ensures(leaves_sorted((^l).e@))]
#[ensures(!leaves_mem((^l).e@, x))]
#[ensures(forall<y: (u64, u32)> leaves_mem((^l).e@, y) ==> leaves_mem((*l).e@, y))]
#[ensures(forall<y: (u64, u32)> leaves_mem((*l).e@, y) && y != x ==> leaves_mem((^l).e@, y))]
#[ensures(leaves_mem((*l).e@, x) ==> (^l).e@.len() == (*l).e@.len() - 1)]
#[ensures(!leaves_mem((*l).e@, x) ==> (^l).e@ == (*l).e@)]
#[ensures((^l).e@.len() <= (*l).e@.len())]
#[ensures(forall<k: Int> 0 <= k && k < (^l).e@.len() ==> leaves_mem((*l).e@, (^l).e@[k]))]
#[ensures(forall<b: Int>
             (forall<j: Int> 0 <= j && j < (*l).e@.len() ==> (((*l).e@[j]).1)@ < b)
             ==> (forall<j: Int> 0 <= j && j < (^l).e@.len() ==> (((^l).e@[j]).1)@ < b))]
// Goal-shaped preservation: every entry at a DIFFERENT position than the removed one survives,
// which is exactly how `leaf_in_set` discriminates candidates.
#[ensures(forall<s: u64, ix: Int> leaves_mem_i((*l).e@, s, ix) && ix != (x.1)@
             ==> leaves_mem_i((^l).e@, s, ix))]
#[ensures(forall<s: u64, ix: Int> leaves_mem_i((^l).e@, s, ix) ==> leaves_mem_i((*l).e@, s, ix))]
// Positional provenance, existential-free (see `map_insert`): a sorted removal shifts the tail left
// by one, so the entry now at position k is the entry that was at k or the one that was at k+1.
#[ensures(forall<k: Int> 0 <= k && k < (^l).e@.len() ==>
             (^l).e@[k] == (*l).e@[k] || (^l).e@[k] == (*l).e@[k + 1])]
pub fn leaves_remove(l: &mut Leaves, x: (u64, u32)) {
    let p = leaves_pos(l, x);
    let old = snapshot! { l.e@ };
    if p < l.e.len() && l.e[p].0 == x.0 && l.e[p].1 == x.1 {
        let _ = l.e.remove(p);
        proof_assert! { forall<j: Int> 0 <= j && j < p@ ==> l.e@[j] == old[j] };
        proof_assert! { forall<j: Int> p@ <= j && j < l.e@.len() ==> l.e@[j] == old[j + 1] };
        proof_assert! { forall<y: (u64, u32)> leaves_mem(*old, y) && y != x ==> leaves_mem(l.e@, y) };
    }
}

/// `BTreeSet::clear`.
#[ensures((^l).e@.len() == 0)]
#[ensures(leaves_sorted((^l).e@))]
#[ensures(forall<x: (u64, u32)> !leaves_mem((^l).e@, x))]
pub fn leaves_clear(l: &mut Leaves) {
    l.e.clear();
}

/// `BTreeSet::iter().next()` — the least entry, i.e. the oldest eligible leaf.
#[requires(leaves_sorted(l.e@))]
#[ensures(match result {
    Some(x) => l.e@.len() > 0 && x == l.e@[0]
               && (forall<j: Int> 0 <= j && j < l.e@.len() ==> x == l.e@[j] || pair_lt(x, l.e@[j])),
    None => l.e@.len() == 0 })]
pub fn leaves_first(l: &Leaves) -> Option<(u64, u32)> {
    if l.e.len() == 0 { None } else { Some(l.e[0]) }
}

// ===========================================================================
// Pool data-model invariants — one named logic predicate per inventory invariant id
// ===========================================================================

/// Every stored index is in range and the arena is addressable by `u32`.
/// (`EPSL-ARENA-SLOT-COUNT-ASSUMED-BELOW-U32-MAX` is the last conjunct: slots are addressed by
/// `u32` via `self.nodes.len() as u32` at session_list.rs:73, and the shipped code has no check,
/// so the bound is an explicit disclosed modelling assumption.)
#[logic]
pub fn idx_ok(p: &Pool) -> bool {
    pearlite! {
        // J3: relaxed by one (was `<= 4294967295`). D5 (D-RANGE-SC-002-d0657c) admits a
        // registration that appends at `nodes.len() == u32::MAX`, after which the arena holds
        // 2^32 slots (indices 0..=u32::MAX, all still u32-addressable).
        p.nodes@.len() <= 4294967296
        && (forall<i: Int> 0 <= i && i < p.nodes@.len() ==>
              match (p.nodes@[i]).parent { Some(x) => x@ < p.nodes@.len(), None => true })
        && (forall<i: Int> 0 <= i && i < p.nodes@.len() ==>
              match (p.nodes@[i]).child { Some(x) => x@ < p.nodes@.len(), None => true })
        && (forall<k: Int> 0 <= k && k < p.free@.len() ==> (p.free@[k])@ < p.nodes@.len())
        && (forall<k: Int> 0 <= k && k < p.leaves.e@.len() ==>
              ((p.leaves.e@[k]).1)@ < p.nodes@.len())
        && (forall<k: Int> 0 <= k && k < p.by_key.e@.len() ==>
              ((p.by_key.e@[k]).1)@ < p.nodes@.len())
        && (forall<k: Int> 0 <= k && k < p.sessions.e@.len() ==>
              ((p.sessions.e@[k]).1)@ < p.nodes@.len())
    }
}

/// `EPSL-INV-SIZE-ACCOUNTING-IS-EXACT` — the reported size equals the occupied slot count,
/// i.e. total slots minus spare slots (session_list.rs:239-243 asserts exactly this).
#[logic]
pub fn size_exact(p: &Pool) -> bool {
    pearlite! { p.nodes@.len() == p.len@ + p.free@.len() }
}

/// `EPSL-INV-FREE-LIST-IS-EXACTLY-THE-EMPTY-SLOTS` — the spare list holds every empty slot
/// exactly once and no occupied slot.
#[logic]
pub fn free_exact(p: &Pool) -> bool {
    pearlite! { free_all_inactive(p) && free_nodup(p) && free_covers_inactive(p) }
}

/// `EPSL-INV-NO-ORPHANED-BLOCKS` — the two directions of every link agree and both ends are live
/// (session_list.rs:284-297 asserts exactly this).
#[logic]
pub fn links_agree(p: &Pool) -> bool {
    pearlite! { parent_links_agree(p) && child_links_agree(p) }
}

/// `EPSL-INV-LINEAGE-NEVER-CROSSES-SESSIONS` — a block's parent and child are in its own session.
#[logic]
pub fn lineage_in_session(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len() && ((p.nodes@[i]).active) ==>
            (match (p.nodes@[i]).parent {
                Some(x) => (p.nodes@[x@]).session == (p.nodes@[i]).session, None => true })
            && (match (p.nodes@[i]).child {
                Some(x) => (p.nodes@[x@]).session == (p.nodes@[i]).session, None => true })
    }
}

/// `EPSL-INV-ELIGIBLE-SET-IS-EXACTLY-THE-CHILDLESS` — the candidate set is, at every moment,
/// exactly the live childless slots, each recorded once with its current recency
/// (session_list.rs:257-277 asserts exactly this).
#[logic]
pub fn leaf_set_exact(p: &Pool) -> bool {
    pearlite! { leaf_in_set(p) && set_only_leaves(p) }
}

/// `EPSL-INV-SESSION-POINTS-AT-ITS-OWN-END-OF-CHAIN` — every remembered session leaf is live,
/// belongs to that session and is childless (session_list.rs:308-313 asserts exactly this).
#[logic]
pub fn sessions_ok(p: &Pool) -> bool {
    pearlite! { map_distinct(p.sessions.e@) && session_entries_ok(p) }
}

/// `EPSL-INV-EXACTLY-ONE-LEAF-PER-SESSION` — a session holding blocks has exactly one childless
/// block, and it is the one `sessions` remembers.
/// `EPSL-INV-EXACTLY-ONE-LEAF-PER-SESSION`, the existence half on its own.
#[logic]
pub fn leaf_is_session_leaf(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len()
            && ((p.nodes@[i]).active) && (p.nodes@[i]).child == None ==>
              map_has_i(p.sessions.e@, (p.nodes@[i]).session, i)
    }
}

#[logic]
pub fn one_leaf_per_session(p: &Pool) -> bool {
    pearlite! { leaf_is_session_leaf(p) && leaf_sessions_unique(p) }
}

/// `EPSL-INV-ONE-TRACKED-BLOCK-PER-CACHE-KEY` — the key index holds exactly one entry per live
/// block, each naming a live block that really holds that key (session_list.rs:246-255).
#[logic]
pub fn by_key_ok(p: &Pool) -> bool {
    pearlite! {
        map_distinct(p.by_key.e@) && by_key_entries_live(p) && by_key_covers_live(p)
        && keys_unique(p)
    }
}

/// The clock dominates every live recency value — the code-level fact behind
/// `EPSL-INV-RECENCY-STRICTLY-ADVANCES` and what makes each new stamp fresh.
#[logic]
pub fn clock_dominates(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len() && ((p.nodes@[i]).active) ==>
            (p.nodes@[i]).stamp@ <= p.clock@
    }
}

/// `EPSL-INV-NO-CYCLES-IN-LINEAGE` — `birth` (the registration stamp) strictly decreases along
/// `parent` and is bounded above by the clock. Following parent links therefore strictly decreases
/// a natural number, so the walk to the head of a chain terminates and no block is its own
/// ancestor: a session's lineage is always a finite chain, never a ring.
#[logic]
pub fn chain_birth_decreases(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len() && ((p.nodes@[i]).active) ==>
            (p.nodes@[i]).birth@ <= p.clock@
            && (match (p.nodes@[i]).parent {
                  Some(x) => (p.nodes@[x@]).birth@ < (p.nodes@[i]).birth@,
                  None => true })
    }
}

/// `EPSL-INV-ACTIVE-STAMPS-ARE-DISTINCT` — no two live blocks share a recency value.
#[logic]
pub fn stamps_distinct(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < j && j < p.nodes@.len()
            && ((p.nodes@[i]).active) && ((p.nodes@[j]).active) ==>
              (p.nodes@[i]).stamp != (p.nodes@[j]).stamp
    }
}

/// The two cardinality equalities the structure maintains in lockstep. The key index holds
/// exactly one entry per live block (`EPSL-INV-ONE-TRACKED-BLOCK-PER-CACHE-KEY`;
/// session_list.rs:246-250 asserts `by_key.len() == len`), and the candidate set holds exactly one
/// entry per non-empty session (`EPSL-INV-EXACTLY-ONE-LEAF-PER-SESSION`: "the number of eviction
/// candidates always equals the number of sessions currently holding at least one block").
#[logic]
pub fn counts_bounded(p: &Pool) -> bool {
    pearlite! {
        p.by_key.e@.len() == p.len@
        && p.leaves.e@.len() == p.sessions.e@.len()
        && p.len@ <= p.nodes@.len()
    }
}

/// `EPSL-INV-ELIGIBLE-SET-IS-EXACTLY-THE-CHILDLESS`, first direction: every live childless block
/// is in the candidate set, recorded with its current recency.
#[logic]
pub fn leaf_in_set(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len()
            && ((p.nodes@[i]).active) && (p.nodes@[i]).child == None ==>
              leaves_mem_i(p.leaves.e@, (p.nodes@[i]).stamp, i)
    }
}

/// `EPSL-INV-ELIGIBLE-SET-IS-EXACTLY-THE-CHILDLESS`, second direction: nothing else is in it.
#[logic]
pub fn set_only_leaves(p: &Pool) -> bool {
    pearlite! {
        forall<k: Int> 0 <= k && k < p.leaves.e@.len() ==>
              ((p.nodes@[((p.leaves.e@[k]).1)@]).active)
              && (p.nodes@[((p.leaves.e@[k]).1)@]).child == None
              && (p.nodes@[((p.leaves.e@[k]).1)@]).stamp == (p.leaves.e@[k]).0
    }
}

/// `EPSL-INV-FREE-LIST-IS-EXACTLY-THE-EMPTY-SLOTS`, split into its three conjuncts.
#[logic]
pub fn free_all_inactive(p: &Pool) -> bool {
    pearlite! {
        forall<k: Int> 0 <= k && k < p.free@.len() ==> !((p.nodes@[(p.free@[k])@]).active)
    }
}

#[logic]
pub fn free_nodup(p: &Pool) -> bool {
    pearlite! {
        forall<j: Int, k: Int> 0 <= j && j < k && k < p.free@.len() ==> p.free@[j] != p.free@[k]
    }
}

/// "slot `i` is somewhere on the spare list". Named, rather than written inline in
/// `free_covers_inactive`, so that the ONE existential in the model sits behind a predicate symbol.
/// Behind a symbol, carrying coverage across a mutation is equality reasoning on `free_covers(..)`
/// terms; written inline, every caller has to re-find a witness. The existential itself is then
/// discharged only where a witness genuinely appears: inside `free_push` and `pool_alloc`.
#[logic]
pub fn free_covers(f: Seq<u32>, i: Int) -> bool {
    pearlite! { exists<k: Int> 0 <= k && k < f.len() && ((f[k])@ == i) }
}

#[logic]
pub fn free_covers_inactive(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len() && !((p.nodes@[i]).active) ==>
              free_covers(p.free@, i)
    }
}

/// `EPSL-INV-ONE-TRACKED-BLOCK-PER-CACHE-KEY`, split into its four conjuncts.
#[logic]
pub fn by_key_entries_live(p: &Pool) -> bool {
    pearlite! {
        forall<k: Int> 0 <= k && k < p.by_key.e@.len() ==>
              ((p.nodes@[((p.by_key.e@[k]).1)@]).active)
              && (p.nodes@[((p.by_key.e@[k]).1)@]).key == (p.by_key.e@[k]).0
    }
}

#[logic]
pub fn by_key_covers_live(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len() && ((p.nodes@[i]).active) ==>
              map_has_i(p.by_key.e@, (p.nodes@[i]).key, i)
    }
}

#[logic]
pub fn keys_unique(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < j && j < p.nodes@.len()
            && ((p.nodes@[i]).active) && ((p.nodes@[j]).active) ==>
              (p.nodes@[i]).key != (p.nodes@[j]).key
    }
}

/// `EPSL-INV-SESSION-POINTS-AT-ITS-OWN-END-OF-CHAIN`, second conjunct on its own.
#[logic]
pub fn session_entries_ok(p: &Pool) -> bool {
    pearlite! {
        forall<k: Int> 0 <= k && k < p.sessions.e@.len() ==>
              ((p.nodes@[((p.sessions.e@[k]).1)@]).active)
              && (p.nodes@[((p.sessions.e@[k]).1)@]).session == (p.sessions.e@[k]).0
              && (p.nodes@[((p.sessions.e@[k]).1)@]).child == None
    }
}

/// `EPSL-INV-EXACTLY-ONE-LEAF-PER-SESSION`, the uniqueness half on its own.
#[logic]
pub fn leaf_sessions_unique(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < j && j < p.nodes@.len()
            && ((p.nodes@[i]).active) && (p.nodes@[i]).child == None
            && ((p.nodes@[j]).active) && (p.nodes@[j]).child == None ==>
              (p.nodes@[i]).session != (p.nodes@[j]).session
    }
}

/// `EPSL-INV-NO-ORPHANED-BLOCKS`, split by direction.
#[logic]
pub fn parent_links_agree(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len() && ((p.nodes@[i]).active) ==>
            (match (p.nodes@[i]).parent {
                Some(x) => ((p.nodes@[x@]).active) && opt_is((p.nodes@[x@]).child, i),
                None => true })
    }
}

#[logic]
pub fn child_links_agree(p: &Pool) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < p.nodes@.len() && ((p.nodes@[i]).active) ==>
            (match (p.nodes@[i]).child {
                Some(x) => ((p.nodes@[x@]).active) && opt_is((p.nodes@[x@]).parent, i),
                None => true })
    }
}

/// The whole first-order data-model invariant. Everything `Pool::check_invariants`
/// (session_list.rs:232-314) asserts except the ancestor walk, which is not a first-order notion
/// and is carried separately by `chain_bounded`.
#[logic]
pub fn pool_inv(p: &Pool) -> bool {
    pearlite! {
        idx_ok(p) && size_exact(p) && free_exact(p) && links_agree(p) && lineage_in_session(p)
        && leaf_set_exact(p) && sessions_ok(p) && one_leaf_per_session(p) && by_key_ok(p)
        && clock_dominates(p) && stamps_distinct(p) && leaves_sorted(p.leaves.e@)
        && counts_bounded(p) && chain_birth_decreases(p)
    }
}

/// A freshly created pool: `Pool::default()` (session_list.rs:40).
#[logic]
pub fn pool_empty(p: &Pool) -> bool {
    pearlite! {
        p.nodes@.len() == 0 && p.free@.len() == 0 && p.by_key.e@.len() == 0
        && p.sessions.e@.len() == 0 && p.leaves.e@.len() == 0 && p.clock@ == 0 && p.len@ == 0
    }
}

// ===========================================================================
// Pool primitives — line-faithful to src/session_list.rs
// ===========================================================================

/// `Pool::default()` (session_list.rs:40-41).
#[ensures(pool_empty(&result))]
#[ensures(pool_inv(&result))]
pub fn pool_fresh() -> Pool {
    Pool {
        nodes: Vec::new(),
        free: Vec::new(),
        by_key: Map { e: Vec::new() },
        sessions: Map { e: Vec::new() },
        leaves: Leaves { e: Vec::new() },
        clock: 0,
        len: 0,
    }
}

/// `Pool::tick` (session_list.rs:62-65). The `u64` addition has no overflow guard, so the
/// no-overflow assumption (`EPSL-ACCESS-CLOCK-ASSUMED-NOT-TO-OVERFLOW`) is a precondition.
#[requires(p.clock@ < 18446744073709551615)]
#[ensures((^p).clock@ == (*p).clock@ + 1)]
#[ensures(result == (^p).clock)]
#[ensures(result@ > (*p).clock@)]
#[ensures((^p).nodes@ == (*p).nodes@)]
#[ensures((^p).free@ == (*p).free@)]
#[ensures((^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@)]
#[ensures((^p).leaves.e@ == (*p).leaves.e@)]
#[ensures((^p).len == (*p).len)]
pub fn pool_tick(p: &mut Pool) -> u64 {
    p.clock += 1;
    p.clock
}

/// `Pool::is_active` (session_list.rs:80-82) — the ONLY validation a handle ever gets
/// (`EPSL-HANDLE-VALIDATION-IS-OCCUPANCY-ONLY`): in range and occupied. Nothing ties the handle
/// to the key, session or registration it was issued for.
#[ensures(result == (index@ < p.nodes@.len() && ((p.nodes@[index@]).active)))]
pub fn pool_is_active(p: &Pool, index: u32) -> bool {
    let i = index as usize;
    if i < p.nodes.len() { p.nodes[i].active } else { false }
}

/// `Pool::len` (session_list.rs:215-217).
#[ensures(result == p.len)]
pub fn pool_len(p: &Pool) -> usize {
    p.len
}

/// `Pool::clear` (session_list.rs:220-228). Note it also restarts slot numbering and resets the
/// recency counter to zero — the root of two divergences.
#[ensures(pool_empty(&^p))]
#[ensures(pool_inv(&^p))]
pub fn pool_clear(p: &mut Pool) {
    p.nodes.clear();
    p.free.clear();
    map_clear(&mut p.by_key);
    map_clear(&mut p.sessions);
    leaves_clear(&mut p.leaves);
    p.clock = 0;
    p.len = 0;
}

/// `Pool::candidates` (session_list.rs:196-202): `leaves.iter().take(n).map(|(_,idx)| key)`.
#[requires(idx_ok(p))]
#[ensures(result@.len() <= n@)]
#[ensures(result@.len() <= p.leaves.e@.len())]
#[ensures(n@ <= p.leaves.e@.len() ==> result@.len() == n@)]
#[ensures(p.leaves.e@.len() <= n@ ==> result@.len() == p.leaves.e@.len())]
#[ensures(forall<j: Int> 0 <= j && j < result@.len() ==>
             result@[j] == (p.nodes@[(((p.leaves.e@[j]).1))@]).key)]
pub fn pool_candidates(p: &Pool, n: usize) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    let total = p.leaves.e.len();
    let lim = if n < total { n } else { total };
    let mut i: usize = 0;
    #[invariant(i@ <= lim@)]
    #[invariant(out@.len() == i@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==>
                   out@[j] == (p.nodes@[(((p.leaves.e@[j]).1))@]).key)]
    while i < lim {
        let idx = p.leaves.e[i].1;
        out.push(p.nodes[idx as usize].key);
        i += 1;
    }
    out
}

/// `Pool::alloc` (session_list.rs:68-77): reuse a spare slot, else append a new one.
#[requires(idx_ok(p))]
#[requires(free_exact(p))]
// J3: exactly D5 (D-RANGE-SC-002-d0657c) `pool.nodes.len() <= u32::MAX as usize` (was `< 4294967295`)
#[requires((*p).nodes@.len() <= 4294967295)]
#[ensures(result@ < (^p).nodes@.len())]
#[ensures((^p).nodes@[result@] == node)]
#[ensures(forall<i: Int> 0 <= i && i < (*p).nodes@.len() && i != result@ ==>
             (^p).nodes@[i] == (*p).nodes@[i])]
#[ensures((*p).free@.len() > 0 ==>
             result == (*p).free@[(*p).free@.len() - 1]
             && (^p).nodes@.len() == (*p).nodes@.len()
             && (^p).free@ == (*p).free@.subsequence(0, (*p).free@.len() - 1)
             && !(((*p).nodes@[result@]).active))]
#[ensures((*p).free@.len() == 0 ==>
             result@ == (*p).nodes@.len()
             && (^p).nodes@.len() == (*p).nodes@.len() + 1
             && (^p).free@ == (*p).free@)]
#[ensures((^p).nodes@.len() >= (*p).nodes@.len())]
#[ensures((^p).nodes@.len() <= (*p).nodes@.len() + 1)]
#[ensures((^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@)]
#[ensures((^p).leaves.e@ == (*p).leaves.e@)]
#[ensures((^p).clock == (*p).clock)]
#[ensures((^p).len == (*p).len)]
// Spare-list coverage in the shape `free_covers_inactive` consumes. Popping the last entry cannot
// un-cover any slot other than the one popped: a witness at any other position is below the new
// length. Stated here because deriving it from the `subsequence` clause above cost the caller a
// 29.0 s search that sat right at the budget edge and was the goal that starved inside `free_exact`.
#[ensures(forall<i: Int> free_covers((*p).free@, i) && i != result@ ==> free_covers((^p).free@, i))]
pub fn pool_alloc(p: &mut Pool, node: Node) -> u32 {
    match p.free.pop() {
        Some(idx) => {
            p.nodes[idx as usize] = node;
            idx
        }
        None => {
            let idx = p.nodes.len() as u32;
            p.nodes.push(node);
            idx
        }
    }
}

// ---------------------------------------------------------------------------
// Bodies are written ONCE as macros, so a second contract over the same code is a new proof of
// the same body, never a second implementation that could drift from it.
// ---------------------------------------------------------------------------

/// `Vec::push` on the spare list. The only reason this is not written inline is the WITNESS.
/// `free_covers_inactive` is the one existential invariant in the model
/// (`forall i inactive. exists k. free[k] == i`), and after a push the witness for the slot just
/// freed is `free@.len()` — a term no prover derives from the `push_back` axiom on its own. Stating
/// both halves here, in a one-line context, is what carries that invariant across an unlink.
#[ensures((^v)@ == (*v)@.push_back(x))]
#[ensures((^v)@.len() == (*v)@.len() + 1)]
#[ensures(free_covers((^v)@, x@))]
#[ensures(forall<i: Int> free_covers((*v)@, i) ==> free_covers((^v)@, i))]
pub fn free_push(v: &mut Vec<u32>, x: u32) {
    v.push(x);
}

macro_rules! touch_body {
    ($p:expr, $index:expr) => { touch_body!($p, $index, {}) };
    ($p:expr, $index:expr, $post:block) => {{
        let p = $p;
        let index = $index;
        if !pool_is_active(&*p, index) {
            return false;
        }
        let i = index as usize;
        let old_stamp = p.nodes[i].stamp;
        let is_leaf = p.nodes[i].child.is_none();
        let new_stamp = pool_tick(&mut *p);
        p.nodes[i].stamp = new_stamp;
        if is_leaf {
            leaves_remove(&mut p.leaves, (old_stamp, index));
            leaves_insert(&mut p.leaves, (new_stamp, index));
        }
        $post
        true
    }};
}

/// `Pool::touch` (session_list.rs:123-137): refresh the recency of slot `index`, re-keying its
/// candidate-set entry if it is a leaf. Returns `false` for an out-of-range or empty slot.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)]
#[ensures(result == (index@ < (*p).nodes@.len() && (((*p).nodes@[index@]).active)))]
#[ensures(!result ==> (^p).nodes@ == (*p).nodes@ && (^p).leaves.e@ == (*p).leaves.e@
                      && (^p).clock == (*p).clock && (^p).len == (*p).len
                      && (^p).free@ == (*p).free@ && (^p).by_key.e@ == (*p).by_key.e@
                      && (^p).sessions.e@ == (*p).sessions.e@)]
#[ensures(result ==> (^p).clock@ == (*p).clock@ + 1)]
#[ensures(result ==> ((^p).nodes@[index@]).stamp == (^p).clock)]
#[ensures(result ==> ((^p).nodes@[index@]).stamp@ > (*p).clock@)]
#[ensures(result ==> ((^p).nodes@[index@]).key == ((*p).nodes@[index@]).key
                  && ((^p).nodes@[index@]).session == ((*p).nodes@[index@]).session
                  && ((^p).nodes@[index@]).parent == ((*p).nodes@[index@]).parent
                  && ((^p).nodes@[index@]).child == ((*p).nodes@[index@]).child
                  && ((^p).nodes@[index@]).active)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != index@ ==>
             (^p).nodes@[j] == (*p).nodes@[j])]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
#[ensures((^p).free@ == (*p).free@)]
#[ensures((^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@)]
#[ensures((^p).len == (*p).len)]
#[ensures((^p).clock@ >= (*p).clock@)]
#[ensures(result && ((*p).nodes@[index@]).child != None ==> (^p).leaves.e@ == (*p).leaves.e@)]
#[ensures(result && ((*p).nodes@[index@]).child == None ==>
             leaves_mem((^p).leaves.e@, ((^p).clock, index))
             && (^p).leaves.e@.len() == (*p).leaves.e@.len()
             && !leaves_mem((^p).leaves.e@, (((*p).nodes@[index@]).stamp, index)))]
#[ensures(idx_ok(&^p))]
#[ensures(size_exact(&^p))]
#[ensures(free_all_inactive(&^p))]
#[ensures(free_nodup(&^p))]
#[ensures(free_covers_inactive(&^p))]
#[ensures(free_exact(&^p))]
#[ensures(parent_links_agree(&^p))]
#[ensures(child_links_agree(&^p))]
#[ensures(links_agree(&^p))]
#[ensures(lineage_in_session(&^p))]
#[ensures(leaf_in_set(&^p))]
#[ensures(set_only_leaves(&^p))]
#[ensures(leaf_set_exact(&^p))]
#[ensures(session_entries_ok(&^p))]
#[ensures(map_distinct((^p).sessions.e@))]
#[ensures(sessions_ok(&^p))]
#[ensures(leaf_is_session_leaf(&^p))]
#[ensures(leaf_sessions_unique(&^p))]
#[ensures(one_leaf_per_session(&^p))]
#[ensures(by_key_entries_live(&^p))]
#[ensures(by_key_covers_live(&^p))]
#[ensures(keys_unique(&^p))]
#[ensures(map_distinct((^p).by_key.e@))]
#[ensures(by_key_ok(&^p))]
#[ensures(clock_dominates(&^p))]
#[ensures(stamps_distinct(&^p))]
#[ensures(leaves_sorted((^p).leaves.e@))]
#[ensures((^p).by_key.e@.len() == (^p).len@)]
#[ensures((^p).leaves.e@.len() == (^p).sessions.e@.len())]
#[ensures((^p).len@ <= (^p).nodes@.len())]
#[ensures(counts_bounded(&^p))]
#[ensures(chain_birth_decreases(&^p))]
// BUDGET EXPOSURE (fourth pass): a caller that chains operations has to carry the arithmetic
// side conditions of the NEXT call across this one. `touch` advances the recency counter by at
// most one and never allocates, so both bounds survive with a margin of one.
#[ensures((^p).clock@ <= (*p).clock@ + 1)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
// ... and it changes NOTHING but stamps, on EITHER branch. Stated unconditionally (not under
// `result ==>`) so a caller that discards the boolean does not have to case-split on it.
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
             && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session
             && ((^p).nodes@[j]).parent == ((*p).nodes@[j]).parent
             && ((^p).nodes@[j]).child == ((*p).nodes@[j]).child
             && ((^p).nodes@[j]).active == ((*p).nodes@[j]).active
             && ((^p).nodes@[j]).birth == ((*p).nodes@[j]).birth)]
#[ensures((^p).free@ == (*p).free@)]
#[ensures((^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@)]
#[ensures((^p).len == (*p).len)]
// the rejecting branch returns before any write, so the pool is bit-for-bit the one it was given.
// Stated as a single struct equality: `state_touch`'s `result != Ok(()) ==> (^s).pools@ == (*s).pools@`
// otherwise has to rebuild the reborrowed element out of seven field-wise seq equalities, which is
// what tipped it over the budget once the contracts grew.
#[ensures(!result ==> ^p == *p)]
pub fn pool_touch(p: &mut Pool, index: u32) -> bool {
    touch_body!(p, index)
}


macro_rules! unlink_body {
    ($p:expr, $idx:expr) => { unlink_body!($p, $idx, {}) };
    ($p:expr, $idx:expr, $post:block) => {{
        let p = $p;
        let idx = $idx;
        let i = idx as usize;
        let parent = p.nodes[i].parent;
        let child = p.nodes[i].child;
        let session = p.nodes[i].session;
        let key = p.nodes[i].key;
        let stamp = p.nodes[i].stamp;
        let was_leaf = child.is_none();
        if let Some(c) = child {
            p.nodes[c as usize].parent = parent;
        }
        if let Some(q) = parent {
            p.nodes[q as usize].child = child;
        }
        if was_leaf {
            leaves_remove(&mut p.leaves, (stamp, idx));
            match parent {
                Some(q) => {
                    let pstamp = p.nodes[q as usize].stamp;
                    leaves_insert(&mut p.leaves, (pstamp, q));
                    map_insert(&mut p.sessions, session, q);
                }
                None => {
                    map_remove(&mut p.sessions, session);
                }
            }
        }
        map_remove(&mut p.by_key, key);
        p.nodes[i].active = false;
        p.nodes[i].parent = None;
        p.nodes[i].child = None;
        free_push(&mut p.free, idx);
        p.len -= 1;
        $post
        key
    }};
}

/// `Pool::unlink` (session_list.rs:144-185): splice slot `idx` out of its chain, free its slot and
/// return its key. Relinks `child.parent <-> parent.child`; if the slot was a leaf its parent (if
/// any) becomes the session's new leaf AT THE PARENT'S EXISTING STAMP, otherwise the session is
/// forgotten. Never calls `tick`, so no recency changes.
#[requires(pool_inv(p))]
#[requires(idx@ < p.nodes@.len())]
#[requires((p.nodes@[idx@]).active)]
#[ensures(result == ((*p).nodes@[idx@]).key)]
#[ensures((^p).len@ == (*p).len@ - 1)]
#[ensures((^p).clock == (*p).clock)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
#[ensures(!(((^p).nodes@[idx@]).active))]
#[ensures(((^p).nodes@[idx@]).parent == None && ((^p).nodes@[idx@]).child == None)]
#[ensures(((^p).nodes@[idx@]).key == ((*p).nodes@[idx@]).key)]
#[ensures(((^p).nodes@[idx@]).stamp == ((*p).nodes@[idx@]).stamp)]
#[ensures(!map_mem((^p).by_key.e@, result))]
// key-index frame: no key other than the removed one changes its tracked status. Needed by the
// stale-handle refutations, which register a FRESH key straight after the removal.
#[ensures(forall<k2: u64> k2 != result ==>
             map_mem((^p).by_key.e@, k2) == map_mem((*p).by_key.e@, k2))]
#[ensures((^p).free@ == (*p).free@.push_back(idx))]
// chain relink
#[ensures(match ((*p).nodes@[idx@]).child { Some(c) =>
             ((^p).nodes@[c@]).parent == ((*p).nodes@[idx@]).parent, None => true })]
#[ensures(match ((*p).nodes@[idx@]).parent { Some(q) =>
             ((^p).nodes@[q@]).child == ((*p).nodes@[idx@]).child, None => true })]
// no surviving stamp changes anywhere (remove/evict never refresh recency)
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).stamp == ((*p).nodes@[j]).stamp
             && ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
             && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session)]
// only the removed slot's own liveness changes
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != idx@ ==>
             ((^p).nodes@[j]).active == ((*p).nodes@[j]).active)]
// nothing outside {idx, its parent, its child} is disturbed at all
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != idx@
             && !opt_is(((*p).nodes@[idx@]).parent, j) && !opt_is(((*p).nodes@[idx@]).child, j) ==>
                (^p).nodes@[j] == (*p).nodes@[j])]
// leaf case: the parent is promoted at its OLD stamp; singleton case: the session is forgotten
#[ensures(((*p).nodes@[idx@]).child == None ==>
             (match ((*p).nodes@[idx@]).parent {
                Some(q) => leaves_mem((^p).leaves.e@, (((*p).nodes@[q@]).stamp, q))
                           && map_has((^p).sessions.e@, ((*p).nodes@[idx@]).session, q),
                None => !map_mem((^p).sessions.e@, ((*p).nodes@[idx@]).session) }))]
#[ensures(!leaves_mem((^p).leaves.e@, (((*p).nodes@[idx@]).stamp, idx)))]
// interior / head removal leaves the candidate set and the session leaf alone
#[ensures(((*p).nodes@[idx@]).child != None ==>
             (^p).leaves.e@ == (*p).leaves.e@ && (^p).sessions.e@ == (*p).sessions.e@)]
#[ensures(idx_ok(&^p))]
#[ensures(size_exact(&^p))]
#[ensures(free_all_inactive(&^p))]
#[ensures(free_nodup(&^p))]
#[ensures(free_covers_inactive(&^p))]
#[ensures(free_exact(&^p))]
#[ensures(parent_links_agree(&^p))]
#[ensures(child_links_agree(&^p))]
#[ensures(links_agree(&^p))]
#[ensures(lineage_in_session(&^p))]
#[ensures(leaf_in_set(&^p))]
#[ensures(set_only_leaves(&^p))]
#[ensures(leaf_set_exact(&^p))]
#[ensures(session_entries_ok(&^p))]
#[ensures(map_distinct((^p).sessions.e@))]
#[ensures(sessions_ok(&^p))]
#[ensures(leaf_is_session_leaf(&^p))]
#[ensures(leaf_sessions_unique(&^p))]
#[ensures(one_leaf_per_session(&^p))]
#[ensures(by_key_entries_live(&^p))]
#[ensures(by_key_covers_live(&^p))]
#[ensures(keys_unique(&^p))]
#[ensures(map_distinct((^p).by_key.e@))]
#[ensures(by_key_ok(&^p))]
#[ensures(clock_dominates(&^p))]
#[ensures(stamps_distinct(&^p))]
#[ensures(leaves_sorted((^p).leaves.e@))]
#[ensures((^p).by_key.e@.len() == (^p).len@)]
#[ensures((^p).leaves.e@.len() == (^p).sessions.e@.len())]
#[ensures((^p).len@ <= (^p).nodes@.len())]
#[ensures(counts_bounded(&^p))]
#[ensures(chain_birth_decreases(&^p))]
// SESSION-INDEX LENGTH (fourth pass): when a leaf with a parent goes, the parent TAKES OVER as the
// session's leaf, so the session index keeps its entry and its length does not move. `map_insert`
// already says so (`map_mem ==> len unchanged`) and `leaf_is_session_leaf` supplies the `map_mem`;
// what was missing was the composition, and `counts_bounded` then pins |leaves| to |sessions| —
// which is how a caller knows the candidate set still has exactly as many entries as before.
#[ensures(((*p).nodes@[idx@]).child == None && ((*p).nodes@[idx@]).parent != None ==>
             (^p).sessions.e@.len() == (*p).sessions.e@.len())]
pub fn pool_unlink(p: &mut Pool, idx: u32) -> u64 {
    unlink_body!(p, idx)
}

macro_rules! register_body {
    ($p:expr, $key:expr, $session:expr) => { register_body!($p, $key, $session, {}) };
    ($p:expr, $key:expr, $session:expr, $post:block) => {{
        let p = $p;
        let key = $key;
        let session = $session;
        match map_get(&p.by_key, key) {
            Some(existing) => {
                let _ = pool_touch(&mut *p, existing);
                $post
                return existing;
            }
            None => {}
        }
        let stamp = pool_tick(&mut *p);
        let parent = map_get(&p.sessions, session);
        let idx = pool_alloc(
            &mut *p,
            Node { key, session, parent, child: None, stamp, birth: stamp, active: true },
        );
        if let Some(q) = parent {
            let pstamp = p.nodes[q as usize].stamp;
            leaves_remove(&mut p.leaves, (pstamp, q));
            p.nodes[q as usize].child = Some(idx);
        }
        map_insert(&mut p.sessions, session, idx);
        leaves_insert(&mut p.leaves, (stamp, idx));
        map_insert(&mut p.by_key, key, idx);
        p.len += 1;
        $post
        idx
    }};
}

/// `Pool::register` (session_list.rs:90-119): idempotent on an already tracked key (refresh the
/// recency and hand back the SAME slot, ignoring the newly supplied session), otherwise allocate a
/// slot and link it under the session's current leaf.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)]
// J3: exactly D5 (D-RANGE-SC-002-d0657c) `pool.nodes.len() <= u32::MAX as usize` (was `< 4294967295`)
#[requires(p.nodes@.len() <= 4294967295)]
#[ensures(result@ < (^p).nodes@.len())]
#[ensures(((^p).nodes@[result@]).active)]
#[ensures(((^p).nodes@[result@]).key == key)]
#[ensures(map_has((^p).by_key.e@, key, result))]
// the idempotent path: same slot, same lineage, same session, unchanged size
#[ensures(map_mem((*p).by_key.e@, key) ==>
             map_has((*p).by_key.e@, key, result)
             && (^p).len == (*p).len
             && (^p).nodes@.len() == (*p).nodes@.len()
             && ((^p).nodes@[result@]).parent == ((*p).nodes@[result@]).parent
             && ((^p).nodes@[result@]).child == ((*p).nodes@[result@]).child
             && ((^p).nodes@[result@]).session == ((*p).nodes@[result@]).session
             && (^p).by_key.e@ == (*p).by_key.e@
             && (^p).sessions.e@ == (*p).sessions.e@)]
// the fresh-key path
#[ensures(!map_mem((*p).by_key.e@, key) ==>
             (^p).len@ == (*p).len@ + 1
             && ((^p).nodes@[result@]).session == session
             && ((^p).nodes@[result@]).child == None
             && ((^p).nodes@[result@]).stamp@ == (*p).clock@ + 1
             && (^p).clock@ == (*p).clock@ + 1
             && leaves_mem((^p).leaves.e@, (((^p).nodes@[result@]).stamp, result))
             && map_has((^p).sessions.e@, session, result))]
// a fresh key for a session that holds NO blocks starts a chain: head (no parent) and leaf
#[ensures(!map_mem((*p).by_key.e@, key) && !map_mem((*p).sessions.e@, session) ==>
             ((^p).nodes@[result@]).parent == None)]
// a fresh key for a session that already holds blocks is linked under its current leaf, and that
// leaf is demoted out of the candidate set while KEEPING its own stamp
#[ensures(forall<q: u32> !map_mem((*p).by_key.e@, key) && map_has((*p).sessions.e@, session, q) ==>
             ((^p).nodes@[result@]).parent == Some(q)
             && ((^p).nodes@[q@]).child == Some(result)
             && ((^p).nodes@[q@]).stamp == ((*p).nodes@[q@]).stamp
             && !leaves_mem((^p).leaves.e@, (((*p).nodes@[q@]).stamp, q)))]
// slot reuse: a fresh key lands either in the slot the spare list last gave up — which was
// therefore EMPTY — or in a brand-new position at the end of the arena. Needed by the stale-handle
// refutations, which turn on the freed slot being handed straight back.
#[ensures(!map_mem((*p).by_key.e@, key) && (*p).free@.len() > 0 ==>
             result == (*p).free@[(*p).free@.len() - 1])]
#[ensures(!map_mem((*p).by_key.e@, key) && (*p).free@.len() == 0 ==>
             result@ == (*p).nodes@.len())]
#[ensures(!map_mem((*p).by_key.e@, key) && result@ < (*p).nodes@.len() ==>
             !(((*p).nodes@[result@]).active))]
// every stamp handed out is strictly newer than every stamp handed out before it
#[ensures((^p).clock@ >= (*p).clock@)]
#[ensures(((^p).nodes@[result@]).stamp@ > (*p).clock@)]
// no slot ever loses its identity, and the arena only grows
#[ensures((^p).nodes@.len() >= (*p).nodes@.len())]
#[ensures((^p).nodes@.len() <= (*p).nodes@.len() + 1)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && ((*p).nodes@[j]).active
             && j != result@ ==>
                ((^p).nodes@[j]).active
                && ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
                && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session)]
#[ensures(idx_ok(&^p))]
#[ensures(size_exact(&^p))]
#[ensures(free_all_inactive(&^p))]
#[ensures(free_nodup(&^p))]
#[ensures(free_covers_inactive(&^p))]
#[ensures(free_exact(&^p))]
#[ensures(parent_links_agree(&^p))]
#[ensures(child_links_agree(&^p))]
#[ensures(links_agree(&^p))]
#[ensures(lineage_in_session(&^p))]
#[ensures(leaf_in_set(&^p))]
#[ensures(set_only_leaves(&^p))]
#[ensures(leaf_set_exact(&^p))]
#[ensures(session_entries_ok(&^p))]
#[ensures(map_distinct((^p).sessions.e@))]
#[ensures(sessions_ok(&^p))]
#[ensures(leaf_is_session_leaf(&^p))]
#[ensures(leaf_sessions_unique(&^p))]
#[ensures(one_leaf_per_session(&^p))]
#[ensures(by_key_entries_live(&^p))]
#[ensures(by_key_covers_live(&^p))]
#[ensures(keys_unique(&^p))]
#[ensures(map_distinct((^p).by_key.e@))]
#[ensures(by_key_ok(&^p))]
#[ensures(clock_dominates(&^p))]
#[ensures(stamps_distinct(&^p))]
#[ensures(leaves_sorted((^p).leaves.e@))]
#[ensures((^p).by_key.e@.len() == (^p).len@)]
#[ensures((^p).leaves.e@.len() == (^p).sessions.e@.len())]
#[ensures((^p).len@ <= (^p).nodes@.len())]
#[ensures(counts_bounded(&^p))]
#[ensures(chain_birth_decreases(&^p))]
// BUDGET EXPOSURE (fourth pass): both paths tick exactly once (the idempotent path through
// `touch`), so the recency counter advances by at most one — the bound a chained caller needs.
#[ensures((^p).clock@ <= (*p).clock@ + 1)]
// FRAME RE-EXPORT (fourth pass): which slots the two paths can touch.
// idempotent path: only the re-registered slot's own stamp moves (it goes through `touch`).
#[ensures(map_mem((*p).by_key.e@, key) ==>
             (forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != result@ ==>
                (^p).nodes@[j] == (*p).nodes@[j]))]
// fresh path: only the new slot and the block that used to end the session's chain move.
#[ensures(!map_mem((*p).by_key.e@, key) ==>
             (forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != result@
                 && !map_has_i((*p).sessions.e@, session, j) ==>
                    (^p).nodes@[j] == (*p).nodes@[j]))]
// how the session index grows — a first block for a session adds an entry, a later one does not.
// `counts_bounded` then pins |leaves| to it, which is how a driver knows the candidate list's LENGTH.
#[ensures(!map_mem((*p).by_key.e@, key) && !map_mem((*p).sessions.e@, session) ==>
             (^p).sessions.e@.len() == (*p).sessions.e@.len() + 1)]
#[ensures(!map_mem((*p).by_key.e@, key) && map_mem((*p).sessions.e@, session) ==>
             (^p).sessions.e@.len() == (*p).sessions.e@.len())]
// a fresh key NEVER lands on a live slot. Implied by the two slot-reuse clauses above, but only
// after a case split on `result@ < (*p).nodes@.len()`; stated directly it is instantiable.
#[ensures(!map_mem((*p).by_key.e@, key) ==>
             (forall<j: Int> 0 <= j && j < (*p).nodes@.len() && ((*p).nodes@[j]).active ==>
                j != result@))]
pub fn pool_register(p: &mut Pool, key: u64, session: u64) -> u32 {
    register_body!(p, key, session)
}

/// `Pool::evict_oldest` (session_list.rs:190-193): remove and return the least `(stamp, index)`
/// leaf's key — the oldest-accessed eligible block across every session in the pool.
#[requires(pool_inv(p))]
#[ensures((*p).leaves.e@.len() == 0 ==>
             result == None && (^p).nodes@ == (*p).nodes@ && (^p).len == (*p).len
             && (^p).clock == (*p).clock && (^p).leaves.e@ == (*p).leaves.e@
             && (^p).by_key.e@ == (*p).by_key.e@ && (^p).sessions.e@ == (*p).sessions.e@
             && (^p).free@ == (*p).free@)]
#[ensures((*p).leaves.e@.len() > 0 ==>
             result == Some(((*p).nodes@[(((*p).leaves.e@[0]).1)@]).key)
             && (^p).len@ == (*p).len@ - 1
             && (^p).clock == (*p).clock
             && !(((^p).nodes@[(((*p).leaves.e@[0]).1)@]).active))]
#[ensures((^p).clock == (*p).clock)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).stamp == ((*p).nodes@[j]).stamp)]
#[ensures(idx_ok(&^p))]
#[ensures(size_exact(&^p))]
#[ensures(free_all_inactive(&^p))]
#[ensures(free_nodup(&^p))]
#[ensures(free_covers_inactive(&^p))]
#[ensures(free_exact(&^p))]
#[ensures(parent_links_agree(&^p))]
#[ensures(child_links_agree(&^p))]
#[ensures(links_agree(&^p))]
#[ensures(lineage_in_session(&^p))]
#[ensures(leaf_in_set(&^p))]
#[ensures(set_only_leaves(&^p))]
#[ensures(leaf_set_exact(&^p))]
#[ensures(session_entries_ok(&^p))]
#[ensures(map_distinct((^p).sessions.e@))]
#[ensures(sessions_ok(&^p))]
#[ensures(leaf_is_session_leaf(&^p))]
#[ensures(leaf_sessions_unique(&^p))]
#[ensures(one_leaf_per_session(&^p))]
#[ensures(by_key_entries_live(&^p))]
#[ensures(by_key_covers_live(&^p))]
#[ensures(keys_unique(&^p))]
#[ensures(map_distinct((^p).by_key.e@))]
#[ensures(by_key_ok(&^p))]
#[ensures(clock_dominates(&^p))]
#[ensures(stamps_distinct(&^p))]
#[ensures(leaves_sorted((^p).leaves.e@))]
#[ensures((^p).by_key.e@.len() == (^p).len@)]
#[ensures((^p).leaves.e@.len() == (^p).sessions.e@.len())]
#[ensures((^p).len@ <= (^p).nodes@.len())]
#[ensures(counts_bounded(&^p))]
#[ensures(chain_birth_decreases(&^p))]
// BUDGET EXPOSURE (fourth pass): eviction unlinks, it never allocates.
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
// FRAME RE-EXPORT (fourth pass): the victim is a LEAF (`set_only_leaves`), so it has no child and
// `pool_unlink`'s {idx, parent, child} frame collapses to {victim, its parent}.
#[ensures((*p).leaves.e@.len() > 0 ==>
             (forall<j: Int> 0 <= j && j < (*p).nodes@.len()
                 && j != (((*p).leaves.e@[0]).1)@
                 && !opt_is(((*p).nodes@[(((*p).leaves.e@[0]).1)@]).parent, j) ==>
                    (^p).nodes@[j] == (*p).nodes@[j]))]
#[ensures((*p).leaves.e@.len() > 0 ==>
             !map_mem((^p).by_key.e@, ((*p).nodes@[(((*p).leaves.e@[0]).1)@]).key))]
#[ensures((*p).leaves.e@.len() == 0 ==> ^p == *p)]
#[ensures((*p).leaves.e@.len() > 0
          && ((*p).nodes@[(((*p).leaves.e@[0]).1)@]).parent != None ==>
             (^p).sessions.e@.len() == (*p).sessions.e@.len())]
// FRAME RE-EXPORT (fourth pass): `pool_unlink` proves both of these and the wrapper dropped them.
// The victim's PARENT inherits the victim's child — which is `None`, because the victim is a leaf —
// so the parent becomes childless and therefore eligible; that is the whole mechanism behind
// `EPSL-CANDIDATES-LISTED-IN-EVICTION-ORDER` being false, and the refutation could not see it.
#[ensures((*p).leaves.e@.len() > 0 ==>
             (match ((*p).nodes@[(((*p).leaves.e@[0]).1)@]).parent {
                Some(q) => ((^p).nodes@[q@]).child
                             == ((*p).nodes@[(((*p).leaves.e@[0]).1)@]).child,
                None => true }))]
// eviction never rewrites a surviving block's key or session (only `register` ever writes them).
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
             && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session)]
pub fn pool_evict_oldest(p: &mut Pool) -> Option<u64> {
    match leaves_first(&p.leaves) {
        Some(x) => Some(pool_unlink(p, x.1)),
        None => None,
    }
}

/// `Pool::remove` (session_list.rs:206-212): stop tracking slot `index`, rejecting an out-of-range
/// or already-empty slot. The occupancy check is the ONLY validation.
#[requires(pool_inv(p))]
#[ensures(result == (index@ < (*p).nodes@.len() && (((*p).nodes@[index@]).active)))]
#[ensures(!result ==> (^p).nodes@ == (*p).nodes@ && (^p).len == (*p).len
                      && (^p).clock == (*p).clock && (^p).leaves.e@ == (*p).leaves.e@
                      && (^p).by_key.e@ == (*p).by_key.e@ && (^p).free@ == (*p).free@
                      && (^p).sessions.e@ == (*p).sessions.e@)]
#[ensures(result ==> (^p).len@ == (*p).len@ - 1
                     && !(((^p).nodes@[index@]).active)
                     && !map_mem((^p).by_key.e@, ((*p).nodes@[index@]).key)
                     && (^p).free@ == (*p).free@.push_back(index))]
#[ensures(forall<k2: u64> k2 != ((*p).nodes@[index@]).key ==>
             map_mem((^p).by_key.e@, k2) == map_mem((*p).by_key.e@, k2))]
#[ensures((^p).clock == (*p).clock)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).stamp == ((*p).nodes@[j]).stamp)]
#[ensures(result ==>
             (match ((*p).nodes@[index@]).child { Some(c) =>
                 ((^p).nodes@[c@]).parent == ((*p).nodes@[index@]).parent, None => true })
             && (match ((*p).nodes@[index@]).parent { Some(q) =>
                 ((^p).nodes@[q@]).child == ((*p).nodes@[index@]).child, None => true }))]
#[ensures(result && ((*p).nodes@[index@]).child != None ==>
             (^p).leaves.e@ == (*p).leaves.e@ && (^p).sessions.e@ == (*p).sessions.e@)]
#[ensures(result && ((*p).nodes@[index@]).child == None ==>
             (match ((*p).nodes@[index@]).parent {
                Some(q) => leaves_mem((^p).leaves.e@, (((*p).nodes@[q@]).stamp, q))
                           && map_has((^p).sessions.e@, ((*p).nodes@[index@]).session, q),
                None => !map_mem((^p).sessions.e@, ((*p).nodes@[index@]).session) }))]
#[ensures(idx_ok(&^p))]
#[ensures(size_exact(&^p))]
#[ensures(free_all_inactive(&^p))]
#[ensures(free_nodup(&^p))]
#[ensures(free_covers_inactive(&^p))]
#[ensures(free_exact(&^p))]
#[ensures(parent_links_agree(&^p))]
#[ensures(child_links_agree(&^p))]
#[ensures(links_agree(&^p))]
#[ensures(lineage_in_session(&^p))]
#[ensures(leaf_in_set(&^p))]
#[ensures(set_only_leaves(&^p))]
#[ensures(leaf_set_exact(&^p))]
#[ensures(session_entries_ok(&^p))]
#[ensures(map_distinct((^p).sessions.e@))]
#[ensures(sessions_ok(&^p))]
#[ensures(leaf_is_session_leaf(&^p))]
#[ensures(leaf_sessions_unique(&^p))]
#[ensures(one_leaf_per_session(&^p))]
#[ensures(by_key_entries_live(&^p))]
#[ensures(by_key_covers_live(&^p))]
#[ensures(keys_unique(&^p))]
#[ensures(map_distinct((^p).by_key.e@))]
#[ensures(by_key_ok(&^p))]
#[ensures(clock_dominates(&^p))]
#[ensures(stamps_distinct(&^p))]
#[ensures(leaves_sorted((^p).leaves.e@))]
#[ensures((^p).by_key.e@.len() == (^p).len@)]
#[ensures((^p).leaves.e@.len() == (^p).sessions.e@.len())]
#[ensures((^p).len@ <= (^p).nodes@.len())]
#[ensures(counts_bounded(&^p))]
#[ensures(chain_birth_decreases(&^p))]
// FRAME RE-EXPORT (fourth pass): `pool_unlink` proves both of these; its wrapper did not pass
// them on, so `verify_epsl_remove_disturbs_only_the_chain_neighbours` had nothing to work from.
// Stated unconditionally: on the rejecting branch the arena is untouched, so they hold trivially.
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != index@
             && !opt_is(((*p).nodes@[index@]).parent, j)
             && !opt_is(((*p).nodes@[index@]).child, j) ==>
                (^p).nodes@[j] == (*p).nodes@[j])]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != index@ ==>
             ((^p).nodes@[j]).active == ((*p).nodes@[j]).active)]
#[ensures(!result ==> ^p == *p)]
pub fn pool_remove(p: &mut Pool, index: u32) -> bool {
    if !pool_is_active(&*p, index) {
        return false;
    }
    let _ = pool_unlink(p, index);
    true
}

// ===========================================================================
// Pools level — line-faithful to src/lib.rs's `impl IEvictionPolicy`
// ===========================================================================

/// Every pool in the component satisfies the data-model invariant.
#[logic]
pub fn pools_inv(s: &Pools) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < s.pools@.len() ==> pool_inv(&s.pools@[i]) }
}

/// Every pool's recency counter and arena still have room — the two disclosed narrowing
/// assumptions (`EPSL-ACCESS-CLOCK-ASSUMED-NOT-TO-OVERFLOW`,
/// `EPSL-ARENA-SLOT-COUNT-ASSUMED-BELOW-U32-MAX`).
#[logic]
pub fn pools_have_room(s: &Pools) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < s.pools@.len() ==>
            (s.pools@[i]).clock@ < 18446744073709551615 && (s.pools@[i]).nodes@.len() <= 4294967295
    }
}

/// Every pool carries the same MARGIN in its recency counter and its arena that `pready` gives a
/// single pool — the `Pools`-level form of the two disclosed narrowing assumptions
/// (`EPSL-ACCESS-CLOCK-ASSUMED-NOT-TO-OVERFLOW`, `EPSL-ARENA-SLOT-COUNT-ASSUMED-BELOW-U32-MAX`).
/// `pools_have_room` states the bound the callees literally need and therefore has NO slack, so
/// it cannot survive its own operation; this is the entry bound a driver needs in order to spend
/// one tick / one slot per call across a chain and still discharge the last call's precondition.
#[logic]
pub fn pools_room(s: &Pools) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < s.pools@.len() ==>
            (s.pools@[i]).clock@ < 18446744073709551000 && (s.pools@[i]).nodes@.len() < 4294967280
    }
}

/// The bound a DRIVER enters with. Three tiers are needed and each one is load-bearing:
///   `pools_have_room` = exactly what the pool-level callees need (no slack, cannot survive its own
///                       operation, so no `state_*` can ensure it);
///   `pools_room`      = one state-layer operation's worth of slack (`state_batch_touch2` ticks the
///                       same pool TWICE, so the zero-slack form cannot discharge its second tick);
///   `pools_ample`     = a whole driver's worth (a driver chains up to eight `state_*` calls, each
///                       of which re-establishes only "grew by at most one").
/// Collapsing any two of them reintroduces the same no-slack failure one level up — measured twice
/// on this pass, once for the pools bound and once for the log bound.
#[logic]
pub fn pools_ample(s: &Pools) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < s.pools@.len() ==>
            (s.pools@[i]).clock@ < 18446744073709550000 && (s.pools@[i]).nodes@.len() < 4294967200
    }
}

/// `Mutex::lock` on one pool.
#[requires(t.held@ < 4294967294 && t.peak@ < 4294967294 && t.acquires@ < 4294967294)]
#[ensures((^t).held@ == (*t).held@ + 1)]
#[ensures((^t).peak@ >= (*t).held@ + 1)]
#[ensures((*t).peak@ >= (*t).held@ + 1 ==> (^t).peak == (*t).peak)]
#[ensures((*t).peak@ < (*t).held@ + 1 ==> (^t).peak@ == (*t).held@ + 1)]
#[ensures((^t).acquires@ == (*t).acquires@ + 1)]
#[ensures((^t).writes == (*t).writes && (^t).reads == (*t).reads)]
pub fn lock_acquire(t: &mut LockTrace) {
    t.held += 1;
    t.acquires += 1;
    if t.held > t.peak {
        t.peak = t.held;
    }
}

/// `drop(pool_guard)`.
#[requires(t.held@ > 0)]
#[ensures((^t).held@ == (*t).held@ - 1)]
#[ensures((^t).peak == (*t).peak)]
#[ensures((^t).acquires == (*t).acquires)]
#[ensures((^t).writes == (*t).writes && (^t).reads == (*t).reads)]
pub fn lock_release(t: &mut LockTrace) {
    t.held -= 1;
}

/// `self.state.write().unwrap()` — taken by `create_pool` and by nothing else.
#[requires(t.writes@ < 4294967294)]
#[ensures((^t).writes@ == (*t).writes@ + 1)]
#[ensures((^t).reads == (*t).reads && (^t).held == (*t).held && (^t).peak == (*t).peak)]
#[ensures((^t).acquires == (*t).acquires)]
pub fn state_write_lock(t: &mut LockTrace) {
    t.writes += 1;
}

/// `self.state.read().unwrap()` — taken by every operation except `create_pool`.
#[requires(t.reads@ < 4294967294)]
#[ensures((^t).reads@ == (*t).reads@ + 1)]
#[ensures((^t).writes == (*t).writes && (^t).held == (*t).held && (^t).peak == (*t).peak)]
#[ensures((^t).acquires == (*t).acquires)]
pub fn state_read_lock(t: &mut LockTrace) {
    t.reads += 1;
}

/// One `logger.info` / `logger.debug` / `logger.warn` call site, only reached when a logger is
/// actually attached (`self.logger.get()` is `Ok`).
#[requires(log.info@ < 4294967294 && log.debug@ < 4294967294 && log.warn@ < 4294967294)]
#[requires(kind@ <= 2)]
#[ensures(connected && kind@ == 0 ==> (^log).info@ == (*log).info@ + 1)]
#[ensures(connected && kind@ == 1 ==> (^log).debug@ == (*log).debug@ + 1)]
#[ensures(connected && kind@ == 2 ==> (^log).warn@ == (*log).warn@ + 1)]
#[ensures(!connected ==> (^log).info == (*log).info && (^log).debug == (*log).debug
                         && (^log).warn == (*log).warn)]
#[ensures(kind@ != 0 ==> (^log).info == (*log).info)]
#[ensures(kind@ != 1 ==> (^log).debug == (*log).debug)]
#[ensures(kind@ != 2 ==> (^log).warn == (*log).warn)]
pub fn log_site(log: &mut Log, connected: bool, kind: u8) {
    if connected {
        if kind == 0 {
            log.info += 1;
        } else if kind == 1 {
            log.debug += 1;
        } else {
            log.warn += 1;
        }
    }
}

/// `create_pool` (src/lib.rs:103-121). Takes the ONLY `RwLock::write` in the component, appends a
/// fresh pool, and — once, and only while a logger is attached — emits the selection banner.
#[requires(pools_inv(s))]
// J3: exactly D4 (D-RANGE-IFACE-CREATE_POOL-4eb401) `state.pools.len() <= u32::MAX as usize`
// (was `< 4294967295`)
#[requires((*s).pools@.len() <= 4294967295)]
// room for the two sites THIS call can reach, with slack for the calls before it in a driver.
// `log_room` (the driver entry bound) is < 4294967280, so three chained calls stay inside this.
#[requires(log.info@ < 4294967290 && log.debug@ < 4294967290 && log.warn@ < 4294967290)]
#[requires(t.writes@ < 4294967294)]
#[ensures(result@ == (*s).pools@.len())]
#[ensures((^s).pools@.len() == (*s).pools@.len() + 1)]
#[ensures(result@ < (^s).pools@.len())]
#[ensures(pool_empty(&(^s).pools@[result@]))]
#[ensures(pool_inv(&(^s).pools@[result@]))]
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==> (^s).pools@[i] == (*s).pools@[i])]
#[ensures(pools_inv(&^s))]
#[ensures((^t).writes@ == (*t).writes@ + 1)]
#[ensures((^t).reads == (*t).reads)]
#[ensures((^s).announced == ((*s).announced || connected))]
#[ensures(!connected ==> (^s).announced == (*s).announced
                         && (^log).info == (*log).info && (^log).debug == (*log).debug)]
#[ensures(connected && (*s).announced ==> (^log).info == (*log).info)]
#[ensures(connected && !(*s).announced ==> (^log).info@ == (*log).info@ + 1)]
#[ensures((^log).info@ <= (*log).info@ + 1)]
#[ensures((^log).warn == (*log).warn)]
// BUDGET EXPOSURE (fourth pass): how much of each pool's recency counter and arena this
// operation consumed. A driver that chains k operations discharges the k-th call's arithmetic
// side conditions from its entry bound minus k; without these clauses the second call in any
// chain is undischargeable, because `pools_have_room` is required by the state layer and
// re-established by none of it.
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==>
             ((^s).pools@[i]).clock@ <= ((*s).pools@[i]).clock@ + 0
             && ((^s).pools@[i]).nodes@.len() <= ((*s).pools@[i]).nodes@.len() + 0)]
// LOCK-TRACE FRAME (fourth pass): this operation takes only the shared state lock, so the
// per-pool lock counters are untouched. `state_batch_touch2` REQUIRES `held == 0 && peak == 0`,
// which no driver could discharge after an earlier call until this was said out loud.
#[ensures((^t).held == (*t).held && (^t).peak == (*t).peak
          && (^t).acquires == (*t).acquires)]
// the banner emits at most one `info` (already stated) and at most one `debug`
#[ensures((^log).debug@ <= (*log).debug@ + 1)]
pub fn state_create_pool(s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace) -> u32 {
    state_write_lock(t);
    let id = s.pools.len() as u32;
    s.pools.push(pool_fresh());
    if connected {
        if !s.announced {
            log_site(log, connected, 0);
            s.announced = true;
        }
        log_site(log, connected, 1);
    }
    id
}

/// `track` (src/lib.rs:123-141). `Vec::get` on the pool id is the only way this can fail, and it
/// fails with `InvalidPool`, never `InvalidHandle`.
#[requires(pools_inv(s))]
// J3: the per-call ranges of `track`, on the ONE pool the call operates on and nowhere else
// (was `pools_have_room(s)`, which constrained every pool): D6 (D-RANGE-FR-004-7a18db)
// `pool.clock < u64::MAX` and D5 (D-RANGE-SC-002-d0657c) `pool.nodes.len() <= u32::MAX as usize`.
#[requires(pool@ < (*s).pools@.len() ==>
             ((*s).pools@[pool@]).clock@ < 18446744073709551615
             && ((*s).pools@[pool@]).nodes@.len() <= 4294967295)]
#[requires(log.info@ < 4294967294 && log.debug@ < 4294967294 && log.warn@ < 4294967294)]
#[requires(t.reads@ < 4294967294)]
#[ensures(pool@ >= (*s).pools@.len() ==>
             result == Err(PolicyError::InvalidPool(pool))
             && (^s).pools@ == (*s).pools@ && (^s).announced == (*s).announced)]
#[ensures(pool@ < (*s).pools@.len() ==>
             (^s).pools@.len() == (*s).pools@.len()
             && (match result { Ok(h) => h.pool == pool
                                         && h.index@ < ((^s).pools@[pool@]).nodes@.len()
                                         && (((^s).pools@[pool@]).nodes@[h.index@]).active
                                         && (((^s).pools@[pool@]).nodes@[h.index@]).key == key,
                                Err(_) => false }))]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() && j != pool@ ==>
             (^s).pools@[j] == (*s).pools@[j])]
#[ensures(pools_inv(&^s))]
#[ensures((^s).announced == (*s).announced)]
#[ensures((^t).reads@ == (*t).reads@ + 1)]
#[ensures((^t).writes == (*t).writes)]
#[ensures((^log).info == (*log).info && (^log).debug == (*log).debug)]
// BUDGET EXPOSURE (fourth pass): how much of each pool's recency counter and arena this
// operation consumed. A driver that chains k operations discharges the k-th call's arithmetic
// side conditions from its entry bound minus k; without these clauses the second call in any
// chain is undischargeable, because `pools_have_room` is required by the state layer and
// re-established by none of it.
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==>
             ((^s).pools@[i]).clock@ <= ((*s).pools@[i]).clock@ + 1
             && ((^s).pools@[i]).nodes@.len() <= ((*s).pools@[i]).nodes@.len() + 1)]
// LOCK-TRACE FRAME (fourth pass): this operation takes only the shared state lock, so the
// per-pool lock counters are untouched. `state_batch_touch2` REQUIRES `held == 0 && peak == 0`,
// which no driver could discharge after an earlier call until this was said out loud.
#[ensures((^t).held == (*t).held && (^t).peak == (*t).peak
          && (^t).acquires == (*t).acquires)]
// the `InvalidPool` arm logs a warning, so `log_room` has to be carried across this call too
#[ensures((^log).warn@ <= (*log).warn@ + 1)]
// the session of a FRESHLY registered block is the one the caller named. Conditional on freshness
// on purpose: on a re-registration the stored session is kept, which is the recorded
// `EPSL-TRACK-SESSION-COMES-FROM-CALLER` divergence, not something to promise here.
#[ensures(pool@ < (*s).pools@.len() && !map_mem(((*s).pools@[pool@]).by_key.e@, key) ==>
             (match result {
                Ok(h) => (((^s).pools@[pool@]).nodes@[h.index@]).session == session,
                Err(_) => false }))]
pub fn state_track(
    s: &mut Pools,
    pool: u32,
    key: u64,
    session: u64,
    log: &mut Log,
    connected: bool,
    t: &mut LockTrace,
) -> Result<Handle, PolicyError> {
    state_read_lock(t);
    let n = s.pools.len();
    if (pool as usize) >= n {
        log_site(log, connected, 2);
        return Err(PolicyError::InvalidPool(pool));
    }
    let index = pool_register(&mut s.pools[pool as usize], key, session);
    Ok(Handle { pool, index })
}

/// `touch` (src/lib.rs:143-155).
#[requires(pools_inv(s))]
// J12: was `pools_have_room(s)` (a clock AND arena bound on EVERY pool). Now exactly D6
// (D-RANGE-FR-004-7a18db) `pool.clock < u64::MAX` on the one pool the call operates on.
#[requires(h.pool@ < (*s).pools@.len() ==> ((*s).pools@[h.pool@]).clock@ < 18446744073709551615)]
#[requires(t.reads@ < 4294967294)]
#[ensures(h.pool@ >= (*s).pools@.len() ==>
             result == Err(PolicyError::InvalidPool(h.pool)) && (^s).pools@ == (*s).pools@)]
#[ensures(h.pool@ < (*s).pools@.len() ==>
             (match result {
                Ok(_) => h.index@ < ((*s).pools@[h.pool@]).nodes@.len()
                         && (((*s).pools@[h.pool@]).nodes@[h.index@]).active,
                Err(e) => e == PolicyError::InvalidHandle
                          && !(h.index@ < ((*s).pools@[h.pool@]).nodes@.len()
                               && (((*s).pools@[h.pool@]).nodes@[h.index@]).active) }))]
#[ensures(result != Ok(()) ==> (^s).pools@ == (*s).pools@)]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() && j != h.pool@ ==>
             (^s).pools@[j] == (*s).pools@[j])]
#[ensures((^s).pools@.len() == (*s).pools@.len())]
#[ensures(pools_inv(&^s))]
#[ensures((^s).announced == (*s).announced)]
#[ensures((^t).reads@ == (*t).reads@ + 1 && (^t).writes == (*t).writes)]
// BUDGET EXPOSURE (fourth pass): how much of each pool's recency counter and arena this
// operation consumed. A driver that chains k operations discharges the k-th call's arithmetic
// side conditions from its entry bound minus k; without these clauses the second call in any
// chain is undischargeable, because `pools_have_room` is required by the state layer and
// re-established by none of it.
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==>
             ((^s).pools@[i]).clock@ <= ((*s).pools@[i]).clock@ + 1
             && ((^s).pools@[i]).nodes@.len() <= ((*s).pools@[i]).nodes@.len() + 0)]
// LOCK-TRACE FRAME (fourth pass): this operation takes only the shared state lock, so the
// per-pool lock counters are untouched. `state_batch_touch2` REQUIRES `held == 0 && peak == 0`,
// which no driver could discharge after an earlier call until this was said out loud.
#[ensures((^t).held == (*t).held && (^t).peak == (*t).peak
          && (^t).acquires == (*t).acquires)]
// `touch` changes only recency, lifted to the Pools level: the arena, the size, the spare list,
// the key index, the session index and every slot's identity and liveness survive it.
#[ensures(h.pool@ < (*s).pools@.len() ==>
             ((^s).pools@[h.pool@]).nodes@.len() == ((*s).pools@[h.pool@]).nodes@.len()
             && ((^s).pools@[h.pool@]).len == ((*s).pools@[h.pool@]).len
             && ((^s).pools@[h.pool@]).free@ == ((*s).pools@[h.pool@]).free@
             && ((^s).pools@[h.pool@]).by_key.e@ == ((*s).pools@[h.pool@]).by_key.e@
             && ((^s).pools@[h.pool@]).sessions.e@ == ((*s).pools@[h.pool@]).sessions.e@)]
#[ensures(h.pool@ < (*s).pools@.len() ==>
             (forall<j: Int> 0 <= j && j < ((*s).pools@[h.pool@]).nodes@.len() ==>
                (((^s).pools@[h.pool@]).nodes@[j]).active
                   == (((*s).pools@[h.pool@]).nodes@[j]).active
                && (((^s).pools@[h.pool@]).nodes@[j]).key
                   == (((*s).pools@[h.pool@]).nodes@[j]).key
                && (((^s).pools@[h.pool@]).nodes@[j]).session
                   == (((*s).pools@[h.pool@]).nodes@[j]).session))]
pub fn state_touch(s: &mut Pools, h: Handle, t: &mut LockTrace) -> Result<(), PolicyError> {
    state_read_lock(t);
    let n = s.pools.len();
    if (h.pool as usize) >= n {
        return Err(PolicyError::InvalidPool(h.pool));
    }
    if pool_touch(&mut s.pools[h.pool as usize], h.index) {
        Ok(())
    } else {
        Err(PolicyError::InvalidHandle)
    }
}

/// `remove` (src/lib.rs:186-198).
#[requires(pools_inv(s))]
#[requires(t.reads@ < 4294967294)]
#[ensures(h.pool@ >= (*s).pools@.len() ==>
             result == Err(PolicyError::InvalidPool(h.pool)) && (^s).pools@ == (*s).pools@)]
#[ensures(h.pool@ < (*s).pools@.len() ==>
             (match result {
                Ok(_) => h.index@ < ((*s).pools@[h.pool@]).nodes@.len()
                         && (((*s).pools@[h.pool@]).nodes@[h.index@]).active
                         && ((^s).pools@[h.pool@]).len@ == ((*s).pools@[h.pool@]).len@ - 1
                         && !((((^s).pools@[h.pool@]).nodes@[h.index@]).active),
                Err(e) => e == PolicyError::InvalidHandle
                          && !(h.index@ < ((*s).pools@[h.pool@]).nodes@.len()
                               && (((*s).pools@[h.pool@]).nodes@[h.index@]).active) }))]
#[ensures(result != Ok(()) ==> (^s).pools@ == (*s).pools@)]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() && j != h.pool@ ==>
             (^s).pools@[j] == (*s).pools@[j])]
#[ensures((^s).pools@.len() == (*s).pools@.len())]
#[ensures(pools_inv(&^s))]
#[ensures((^s).announced == (*s).announced)]
#[ensures((^t).reads@ == (*t).reads@ + 1 && (^t).writes == (*t).writes)]
// BUDGET EXPOSURE (fourth pass): how much of each pool's recency counter and arena this
// operation consumed. A driver that chains k operations discharges the k-th call's arithmetic
// side conditions from its entry bound minus k; without these clauses the second call in any
// chain is undischargeable, because `pools_have_room` is required by the state layer and
// re-established by none of it.
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==>
             ((^s).pools@[i]).clock@ <= ((*s).pools@[i]).clock@ + 0
             && ((^s).pools@[i]).nodes@.len() <= ((*s).pools@[i]).nodes@.len() + 0)]
// LOCK-TRACE FRAME (fourth pass): this operation takes only the shared state lock, so the
// per-pool lock counters are untouched. `state_batch_touch2` REQUIRES `held == 0 && peak == 0`,
// which no driver could discharge after an earlier call until this was said out loud.
#[ensures((^t).held == (*t).held && (^t).peak == (*t).peak
          && (^t).acquires == (*t).acquires)]
pub fn state_remove(s: &mut Pools, h: Handle, t: &mut LockTrace) -> Result<(), PolicyError> {
    state_read_lock(t);
    let n = s.pools.len();
    if (h.pool as usize) >= n {
        return Err(PolicyError::InvalidPool(h.pool));
    }
    if pool_remove(&mut s.pools[h.pool as usize], h.index) {
        Ok(())
    } else {
        Err(PolicyError::InvalidHandle)
    }
}

/// `batch_touch` (src/lib.rs:157-184) specialised to a TWO-handle group, which is the smallest
/// group that exercises every branch: the up-front `is_empty` exit, the first-pool lookup, the
/// relock when consecutive handles name different pools, and the mid-walk `InvalidHandle` exit
/// that leaves the earlier refresh applied.
#[requires(pools_inv(s))]
// J3: was `pools_room(s)` (a constant margin on every pool). Now only what the two refreshes
// consume: one tick on h0's pool, and one tick on h1's pool — which is the SAME counter a second
// time when both handles name one pool (that case needs `clock + 1 < u64::MAX`, i.e. MORE than
// D6's per-call `clock < u64::MAX`; this is a callee requirement, not a driver premise).
#[requires(h0.pool@ < (*s).pools@.len() ==> ((*s).pools@[h0.pool@]).clock@ < 18446744073709551615)]
// J12: the two second-call clauses are guarded by "h0 was accepted" -- the second `Pool::touch`
// call is reached only then -- so the three clauses are exactly D6 at each `touch` call.
#[requires(h0.pool@ < (*s).pools@.len()
             && h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
             && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active
             && h1.pool@ < (*s).pools@.len() && h1.pool != h0.pool ==>
             ((*s).pools@[h1.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool@ < (*s).pools@.len()
             && h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
             && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active
             && h1.pool == h0.pool ==>
             ((*s).pools@[h1.pool@]).clock@ + 1 < 18446744073709551615)]
#[requires(t.reads@ < 4294967294)]
#[requires(t.held@ == 0 && t.peak@ == 0)]
#[ensures(h0.pool@ >= (*s).pools@.len() ==>
             result == Err(PolicyError::InvalidPool(h0.pool)) && (^s).pools@ == (*s).pools@)]
#[ensures((^s).pools@.len() == (*s).pools@.len())]
#[ensures(pools_inv(&^s))]
#[ensures((^s).announced == (*s).announced)]
#[ensures((^t).peak@ <= 1)]
#[ensures((^t).reads@ == (*t).reads@ + 1 && (^t).writes == (*t).writes)]
// the first handle is refreshed before the second is even looked at, so a failure on the second
// leaves the first one's new recency in place (this is the partial-application divergence)
#[ensures(h0.pool@ < (*s).pools@.len()
          && h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
          && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active ==>
             (((^s).pools@[h0.pool@]).nodes@[h0.index@]).stamp@ > ((*s).pools@[h0.pool@]).clock@)]
#[ensures(h0.pool@ < (*s).pools@.len()
          && !(h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
               && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active) ==>
             result == Err(PolicyError::InvalidHandle))]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() && j != h0.pool@ && j != h1.pool@ ==>
             (^s).pools@[j] == (*s).pools@[j])]
// BUDGET EXPOSURE (fourth pass): how much of each pool's recency counter and arena this
// operation consumed. A driver that chains k operations discharges the k-th call's arithmetic
// side conditions from its entry bound minus k; without these clauses the second call in any
// chain is undischargeable, because `pools_have_room` is required by the state layer and
// re-established by none of it.
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==>
             ((^s).pools@[i]).clock@ <= ((*s).pools@[i]).clock@ + 2
             && ((^s).pools@[i]).nodes@.len() <= ((*s).pools@[i]).nodes@.len() + 0)]
// the acquires bound `lock_acquire` needs; every caller has it from `ready`
#[requires(t.acquires@ < 4294967280)]
// an unknown SECOND domain is rejected when the walk reaches it, after the first refresh landed
#[ensures(h0.pool@ < (*s).pools@.len()
          && h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
          && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active
          && h1.pool != h0.pool && h1.pool@ >= (*s).pools@.len() ==>
             result == Err(PolicyError::InvalidPool(h1.pool)))]
// one per-pool lock for a group inside one domain, two when the domain changes
#[ensures(h0.pool@ < (*s).pools@.len()
          && h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
          && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active
          && h1.pool == h0.pool ==> (^t).acquires@ == (*t).acquires@ + 1)]
#[ensures(h0.pool@ < (*s).pools@.len()
          && h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
          && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active
          && h1.pool != h0.pool && h1.pool@ < (*s).pools@.len() ==>
             (^t).acquires@ == (*t).acquires@ + 2)]
pub fn state_batch_touch2(
    s: &mut Pools,
    h0: Handle,
    h1: Handle,
    t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_read_lock(t);
    let n = s.pools.len();
    let mut current = h0.pool;
    if (current as usize) >= n {
        return Err(PolicyError::InvalidPool(current));
    }
    lock_acquire(t);
    if !pool_touch(&mut s.pools[current as usize], h0.index) {
        return Err(PolicyError::InvalidHandle);
    }
    if h1.pool != current {
        lock_release(t);
        current = h1.pool;
        if (current as usize) >= n {
            return Err(PolicyError::InvalidPool(current));
        }
        lock_acquire(t);
    }
    if !pool_touch(&mut s.pools[current as usize], h1.index) {
        return Err(PolicyError::InvalidHandle);
    }
    lock_release(t);
    Ok(())
}

/// `identify_next_to_evict` (src/lib.rs:200-205) — cannot report an error, so an unknown pool
/// degrades to "nothing to evict".
#[requires(pools_inv(s))]
#[requires(t.reads@ < 4294967294)]
#[ensures(pool@ >= (*s).pools@.len() ==> result == None && (^s).pools@ == (*s).pools@)]
#[ensures(pool@ < (*s).pools@.len() ==>
             (if ((*s).pools@[pool@]).leaves.e@.len() == 0 {
                 result == None && ((^s).pools@[pool@]).len == ((*s).pools@[pool@]).len
              } else {
                 result == Some((((*s).pools@[pool@]).nodes@[((((*s).pools@[pool@]).leaves.e@[0]).1)@]).key)
                 && ((^s).pools@[pool@]).len@ == ((*s).pools@[pool@]).len@ - 1
              }))]
#[ensures(pool@ < (*s).pools@.len() ==>
             ((^s).pools@[pool@]).clock == ((*s).pools@[pool@]).clock)]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() && j != pool@ ==>
             (^s).pools@[j] == (*s).pools@[j])]
#[ensures((^s).pools@.len() == (*s).pools@.len())]
#[ensures(pools_inv(&^s))]
#[ensures((^s).announced == (*s).announced)]
#[ensures((^t).reads@ == (*t).reads@ + 1 && (^t).writes == (*t).writes)]
// BUDGET EXPOSURE (fourth pass): how much of each pool's recency counter and arena this
// operation consumed. A driver that chains k operations discharges the k-th call's arithmetic
// side conditions from its entry bound minus k; without these clauses the second call in any
// chain is undischargeable, because `pools_have_room` is required by the state layer and
// re-established by none of it.
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==>
             ((^s).pools@[i]).clock@ <= ((*s).pools@[i]).clock@ + 0
             && ((^s).pools@[i]).nodes@.len() <= ((*s).pools@[i]).nodes@.len() + 0)]
// LOCK-TRACE FRAME (fourth pass): this operation takes only the shared state lock, so the
// per-pool lock counters are untouched. `state_batch_touch2` REQUIRES `held == 0 && peak == 0`,
// which no driver could discharge after an earlier call until this was said out loud.
#[ensures((^t).held == (*t).held && (^t).peak == (*t).peak
          && (^t).acquires == (*t).acquires)]
// an empty domain is left exactly as it was — needed by `create_pool_starts_empty`, which asks a
// brand-new domain for a victim and then for its candidate list
#[ensures(pool@ < (*s).pools@.len() && pool_empty(&(*s).pools@[pool@]) ==>
             pool_empty(&(^s).pools@[pool@]))]
pub fn state_evict(s: &mut Pools, pool: u32, t: &mut LockTrace) -> Option<u64> {
    state_read_lock(t);
    let n = s.pools.len();
    if (pool as usize) >= n {
        return None;
    }
    pool_evict_oldest(&mut s.pools[pool as usize])
}

/// `get_eviction_candidates` (src/lib.rs:207-213) — an unknown pool yields an empty list.
#[requires(pools_inv(s))]
#[requires(t.reads@ < 4294967294)]
#[ensures(pool@ >= s.pools@.len() ==> result@.len() == 0)]
#[ensures(result@.len() <= n@)]
#[ensures(pool@ < s.pools@.len() ==>
             result@.len() <= (s.pools@[pool@]).leaves.e@.len()
             && (n@ <= (s.pools@[pool@]).leaves.e@.len() ==> result@.len() == n@)
             && ((s.pools@[pool@]).leaves.e@.len() <= n@
                 ==> result@.len() == (s.pools@[pool@]).leaves.e@.len())
             && (forall<j: Int> 0 <= j && j < result@.len() ==>
                    result@[j] == ((s.pools@[pool@]).nodes@[((((s.pools@[pool@]).leaves.e@[j]).1))@]).key))]
#[ensures((^t).reads@ == (*t).reads@ + 1 && (^t).writes == (*t).writes)]
// LOCK-TRACE FRAME (fourth pass): this operation takes only the shared state lock, so the
// per-pool lock counters are untouched. `state_batch_touch2` REQUIRES `held == 0 && peak == 0`,
// which no driver could discharge after an earlier call until this was said out loud.
#[ensures((^t).held == (*t).held && (^t).peak == (*t).peak
          && (^t).acquires == (*t).acquires)]
pub fn state_candidates(s: &Pools, pool: u32, n: usize, t: &mut LockTrace) -> Vec<u64> {
    state_read_lock(t);
    let total = s.pools.len();
    if (pool as usize) >= total {
        return Vec::new();
    }
    pool_candidates(&s.pools[pool as usize], n)
}

/// `len` (src/lib.rs:215-221) — an unknown pool reports zero, indistinguishable from empty.
#[requires(t.reads@ < 4294967294)]
#[ensures(pool@ >= s.pools@.len() ==> result@ == 0)]
#[ensures(pool@ < s.pools@.len() ==> result == (s.pools@[pool@]).len)]
#[ensures((^t).reads@ == (*t).reads@ + 1 && (^t).writes == (*t).writes)]
// LOCK-TRACE FRAME (fourth pass): this operation takes only the shared state lock, so the
// per-pool lock counters are untouched. `state_batch_touch2` REQUIRES `held == 0 && peak == 0`,
// which no driver could discharge after an earlier call until this was said out loud.
#[ensures((^t).held == (*t).held && (^t).peak == (*t).peak
          && (^t).acquires == (*t).acquires)]
pub fn state_len(s: &Pools, pool: u32, t: &mut LockTrace) -> usize {
    state_read_lock(t);
    let total = s.pools.len();
    if (pool as usize) >= total {
        return 0;
    }
    s.pools[pool as usize].len
}

/// `clear_pool` (src/lib.rs:223-228) — returns nothing, so an unknown pool is a silent no-op.
#[requires(pools_inv(s))]
#[requires(t.reads@ < 4294967294)]
#[ensures(pool@ >= (*s).pools@.len() ==> (^s).pools@ == (*s).pools@)]
#[ensures(pool@ < (*s).pools@.len() ==> pool_empty(&(^s).pools@[pool@]))]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() && j != pool@ ==>
             (^s).pools@[j] == (*s).pools@[j])]
#[ensures((^s).pools@.len() == (*s).pools@.len())]
#[ensures(pools_inv(&^s))]
#[ensures((^s).announced == (*s).announced)]
#[ensures((^t).reads@ == (*t).reads@ + 1 && (^t).writes == (*t).writes)]
// BUDGET EXPOSURE (fourth pass): how much of each pool's recency counter and arena this
// operation consumed. A driver that chains k operations discharges the k-th call's arithmetic
// side conditions from its entry bound minus k; without these clauses the second call in any
// chain is undischargeable, because `pools_have_room` is required by the state layer and
// re-established by none of it.
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==>
             ((^s).pools@[i]).clock@ <= ((*s).pools@[i]).clock@ + 0
             && ((^s).pools@[i]).nodes@.len() <= ((*s).pools@[i]).nodes@.len() + 0)]
// LOCK-TRACE FRAME (fourth pass): this operation takes only the shared state lock, so the
// per-pool lock counters are untouched. `state_batch_touch2` REQUIRES `held == 0 && peak == 0`,
// which no driver could discharge after an earlier call until this was said out loud.
#[ensures((^t).held == (*t).held && (^t).peak == (*t).peak
          && (^t).acquires == (*t).acquires)]
pub fn state_clear_pool(s: &mut Pools, pool: u32, t: &mut LockTrace) {
    state_read_lock(t);
    let n = s.pools.len();
    if (pool as usize) < n {
        pool_clear(&mut s.pools[pool as usize]);
    }
}

/// `batch_touch` for a group whose handles all name ONE pool — the shape that shows every named
/// block really is refreshed, not just the first or the last, and that a failure part-way through
/// leaves the earlier refreshes applied.
#[requires(pool_inv(p))]
// J12: exactly D6 (D-RANGE-FR-004-7a18db) `pool.clock < u64::MAX`, stated at EVERY `Pool::touch`
// call the walk makes (the tick, session_list.rs:62-65, lives in `touch`). The k-th call is reached
// iff handles 0..k were all accepted (live in the pre-state; `touch` never changes liveness), and
// each accepted handle ticked the clock exactly once, so at the k-th call the clock is `clock + k`.
// Was `nodes.len() < 4294967280 && clock + idxs.len() < 18446744073709551000` (constant slack).
#[requires(forall<k: Int> 0 <= k && k < idxs@.len()
              && (forall<j: Int> 0 <= j && j < k ==>
                     (idxs@[j])@ < p.nodes@.len() && ((p.nodes@[(idxs@[j])@]).active)) ==>
                 p.clock@ + k < 18446744073709551615)]
#[ensures(idx_ok(&^p))]
#[ensures(size_exact(&^p))]
#[ensures(free_all_inactive(&^p))]
#[ensures(free_nodup(&^p))]
#[ensures(free_covers_inactive(&^p))]
#[ensures(free_exact(&^p))]
#[ensures(parent_links_agree(&^p))]
#[ensures(child_links_agree(&^p))]
#[ensures(links_agree(&^p))]
#[ensures(lineage_in_session(&^p))]
#[ensures(leaf_in_set(&^p))]
#[ensures(set_only_leaves(&^p))]
#[ensures(leaf_set_exact(&^p))]
#[ensures(session_entries_ok(&^p))]
#[ensures(map_distinct((^p).sessions.e@))]
#[ensures(sessions_ok(&^p))]
#[ensures(leaf_is_session_leaf(&^p))]
#[ensures(leaf_sessions_unique(&^p))]
#[ensures(one_leaf_per_session(&^p))]
#[ensures(by_key_entries_live(&^p))]
#[ensures(by_key_covers_live(&^p))]
#[ensures(keys_unique(&^p))]
#[ensures(map_distinct((^p).by_key.e@))]
#[ensures(by_key_ok(&^p))]
#[ensures(clock_dominates(&^p))]
#[ensures(stamps_distinct(&^p))]
#[ensures(leaves_sorted((^p).leaves.e@))]
#[ensures((^p).by_key.e@.len() == (^p).len@)]
#[ensures((^p).leaves.e@.len() == (^p).sessions.e@.len())]
#[ensures((^p).len@ <= (^p).nodes@.len())]
#[ensures(counts_bounded(&^p))]
#[ensures(chain_birth_decreases(&^p))]
#[ensures((^p).len == (*p).len)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
#[ensures((^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).clock@ >= (*p).clock@)]
#[ensures(result == Ok(()) ==> (^p).clock@ == (*p).clock@ + idxs@.len())]
#[ensures(result == Ok(()) ==>
             forall<j: Int> 0 <= j && j < idxs@.len() ==>
                (idxs@[j])@ < (^p).nodes@.len()
                && ((^p).nodes@[(idxs@[j])@]).stamp@ > (*p).clock@)]
// every handle the walk got PAST keeps its new recency even when a later one fails
#[ensures(idxs@.len() > 0 && (idxs@[0])@ < (*p).nodes@.len()
          && (((*p).nodes@[(idxs@[0])@]).active) ==>
             ((^p).nodes@[(idxs@[0])@]).stamp@ > (*p).clock@)]
#[ensures(idxs@.len() > 0 && !((idxs@[0])@ < (*p).nodes@.len()
          && (((*p).nodes@[(idxs@[0])@]).active)) ==> result != Ok(()))]
// the clock has advanced for every handle the walk got past, whatever the outcome
#[ensures(idxs@.len() > 0 && (idxs@[0])@ < (*p).nodes@.len()
          && (((*p).nodes@[(idxs@[0])@]).active) ==> (^p).clock@ > (*p).clock@)]
// a slot no handle in the group names keeps everything, recency included
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len()
             && (forall<k: Int> 0 <= k && k < idxs@.len() ==> (idxs@[k])@ != j) ==>
                (^p).nodes@[j] == (*p).nodes@[j])]
// J12: a group whose every handle names a live slot is accepted in full
#[ensures((forall<k: Int> 0 <= k && k < idxs@.len() ==>
              (idxs@[k])@ < (*p).nodes@.len() && (((*p).nodes@[(idxs@[k])@]).active)) ==>
             result == Ok(()))]
// J12: the walk changes nothing but stamps, the clock and the leaf set, on EITHER outcome
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
             && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session
             && ((^p).nodes@[j]).parent == ((*p).nodes@[j]).parent
             && ((^p).nodes@[j]).child == ((*p).nodes@[j]).child
             && ((^p).nodes@[j]).active == ((*p).nodes@[j]).active
             && ((^p).nodes@[j]).birth == ((*p).nodes@[j]).birth)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@ && (^p).free@ == (*p).free@)]
// J12: on success, the LAST mention of each slot (position j) carries recency `clock + j + 1` --
// exactly the value the (j+1)-th of the one-at-a-time `touch` calls gives it.
#[ensures(result == Ok(()) ==>
             forall<j: Int> 0 <= j && j < idxs@.len()
                && (forall<k: Int> j < k && k < idxs@.len() ==> idxs@[k] != idxs@[j]) ==>
                   ((^p).nodes@[(idxs@[j])@]).stamp@ == (*p).clock@ + j + 1)]
pub fn pool_batch_touch(p: &mut Pool, idxs: &Vec<u32>) -> Result<(), PolicyError> {
    let start = snapshot! { p.clock@ };
    let nodes0 = snapshot! { p.nodes@ };
    let len0 = snapshot! { p.len };
    let bykey0 = snapshot! { p.by_key.e@ };
    let sess0 = snapshot! { p.sessions.e@ };
    let free0 = snapshot! { p.free@ };
    let n = idxs.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(pool_inv(p))]
    #[invariant(p.clock@ == *start + i@)]
    #[invariant(forall<k: Int> i@ <= k && k < n@
                   && (forall<j: Int> 0 <= j && j < k ==>
                          (idxs@[j])@ < (*nodes0).len() && (((*nodes0)[(idxs@[j])@]).active)) ==>
                      *start + k < 18446744073709551615)]
    #[invariant(p.nodes@.len() == (*nodes0).len())]
    #[invariant(forall<j: Int> 0 <= j && j < p.nodes@.len() ==>
                   (p.nodes@[j]).key == ((*nodes0)[j]).key
                   && (p.nodes@[j]).session == ((*nodes0)[j]).session
                   && (p.nodes@[j]).parent == ((*nodes0)[j]).parent
                   && (p.nodes@[j]).child == ((*nodes0)[j]).child
                   && (p.nodes@[j]).birth == ((*nodes0)[j]).birth)]
    #[invariant(p.sessions.e@ == *sess0 && p.free@ == *free0)]
    #[invariant(forall<j: Int> 0 <= j && j < i@
                   && (forall<k: Int> j < k && k < i@ ==> idxs@[k] != idxs@[j]) ==>
                      (p.nodes@[(idxs@[j])@]).stamp@ == *start + j + 1)]
    #[invariant(p.len == *len0)]
    #[invariant(p.by_key.e@ == *bykey0)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==>
                   (idxs@[j])@ < p.nodes@.len() && ((p.nodes@[(idxs@[j])@]).stamp@ > *start))]
    // `touch` never clears a slot, so liveness is frozen for the whole batch. Needed only to pull
    // the next invariant back to the PRE-state arena.
    #[invariant(forall<j: Int> 0 <= j && j < p.nodes@.len() ==>
                   ((p.nodes@[j]).active) == (((*nodes0)[j]).active))]
    // Reaching iteration i means every earlier handle was accepted, and `pool_touch` accepts only a
    // handle that was in range and live IN THE PRE-STATE. Without this the exit clause
    // "an invalid first handle forces an error" has nothing to contradict: the other invariants
    // speak about the current arena, and the obligation is about the arena on entry.
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==>
                   (idxs@[j])@ < (*nodes0).len() && (((*nodes0)[(idxs@[j])@]).active))]
    #[invariant(forall<j: Int> 0 <= j && j < p.nodes@.len()
                   && (forall<k: Int> 0 <= k && k < i@ ==> (idxs@[k])@ != j) ==>
                      p.nodes@[j] == (*nodes0)[j])]
    while i < n {
        if !pool_touch(p, idxs[i]) {
            return Err(PolicyError::InvalidHandle);
        }
        i += 1;
    }
    Ok(())
}

/// `batch_touch` (src/lib.rs:157-184) over an arbitrary group, kept line-faithful including the
/// up-front `handles.is_empty()` exit that returns BEFORE any lock is taken or any domain id is
/// validated, and the `drop(guard)` / re-`lock` when consecutive handles name different domains.
#[requires(pools_inv(s))]
#[requires(t.reads@ < 4294967280)]
// one per-pool lock per iteration in the worst case, so the bound has to carry the REMAINING
// iterations: `t.acquires@ < K` alone is not preserved by this loop.
#[requires(t.acquires@ + hs@.len() < 4294967290)]
#[requires(t.held@ == 0 && t.peak@ == 0)]
#[requires(forall<k: Int> 0 <= k && k < (*s).pools@.len() ==>
              ((*s).pools@[k]).clock@ + hs@.len() < 18446744073709551000
              && ((*s).pools@[k]).nodes@.len() < 4294967280)]
#[ensures(hs@.len() == 0 ==> result == Ok(())
          && (^s).pools@ == (*s).pools@
          && (^t).reads == (*t).reads
          && (^t).acquires == (*t).acquires
          && (^t).peak@ == 0
          && (^t).held@ == 0)]
#[ensures((^s).pools@.len() == (*s).pools@.len())]
#[ensures((^s).announced == (*s).announced)]
#[ensures(pools_inv(&^s))]
#[ensures((^t).peak@ <= 1)]
#[ensures((^t).writes == (*t).writes)]
pub fn state_batch_touch_n(
    s: &mut Pools, hs: &Vec<Handle>, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    let n = hs.len();
    if n == 0 {
        return Ok(());
    }
    state_read_lock(t);
    let total = s.pools.len();
    let mut current = hs[0].pool;
    if (current as usize) >= total {
        return Err(PolicyError::InvalidPool(current));
    }
    lock_acquire(t);
    let reads0 = snapshot! { t.reads };
    let writes0 = snapshot! { t.writes };
    let ann0 = snapshot! { s.announced };
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(pools_inv(s))]
    #[invariant(s.pools@.len() == total@)]
    #[invariant(current@ < total@)]
    #[invariant(t.held@ == 1 && t.peak@ == 1)]
    #[invariant(t.acquires@ + (n@ - i@) < 4294967291)]
    #[invariant(t.reads == *reads0 && t.writes == *writes0)]
    #[invariant(s.announced == *ann0)]
    #[invariant(forall<k: Int> 0 <= k && k < s.pools@.len() ==>
                   (s.pools@[k]).clock@ + (n@ - i@) < 18446744073709551000
                   && (s.pools@[k]).nodes@.len() < 4294967280)]
    while i < n {
        let h = hs[i];
        if h.pool != current {
            lock_release(t);
            current = h.pool;
            if (current as usize) >= total {
                return Err(PolicyError::InvalidPool(current));
            }
            lock_acquire(t);
        }
        if !pool_touch(&mut s.pools[current as usize], h.index) {
            return Err(PolicyError::InvalidHandle);
        }
        i += 1;
    }
    lock_release(t);
    Ok(())
}

// ===========================================================================
// Driver preconditions, once
// ===========================================================================

/// The component is in a usable state and every disclosed narrowing assumption still has room.
#[logic]
pub fn ready(s: &Pools, t: &LockTrace) -> bool {
    pearlite! {
        pools_inv(s) && pools_have_room(s) && pools_room(s) && pools_ample(s)
        && s.pools@.len() < 4294967280
        && t.reads@ < 4294967280 && t.writes@ < 4294967280 && t.acquires@ < 4294967280
        && t.held@ == 0 && t.peak@ == 0
    }
}

/// J3: `ready` WITHOUT any range on the pools' clocks, arenas or count. It keeps only the proved
/// invariant `pools_inv` and the bounds on the mirror-only ghost observation counters
/// (`LockTrace`), which `ready` also carries. A driver that consumes D4/D5/D6 states that range
/// itself, exactly as the declared `assume_rust`, on the pool/vector the call operates on.
#[logic]
pub fn ready_exact(s: &Pools, t: &LockTrace) -> bool {
    pearlite! {
        pools_inv(s)
        && t.reads@ < 4294967280 && t.writes@ < 4294967280 && t.acquires@ < 4294967280
        && t.held@ == 0 && t.peak@ == 0
    }
}

/// The logger's observation counters still have room.
#[logic]
pub fn log_room(log: &Log) -> bool {
    pearlite! { log.info@ < 4294967280 && log.debug@ < 4294967280 && log.warn@ < 4294967280 }
}

/// One pool is in a usable state, with room in its recency counter and its arena.
#[logic]
pub fn pready(p: &Pool) -> bool {
    pearlite! {
        pool_inv(p) && p.clock@ < 18446744073709551000 && p.nodes@.len() < 4294967280
    }
}

// ###########################################################################
// PROPERTY DRIVERS — `verify_<id>` per unified-inventory id, with a `__mutant` twin
// ###########################################################################

// ======================= create_pool =======================================

// ---- EPSL-CREATE-POOL-STARTS-EMPTY ----------------------------------------
#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
#[requires((*s).pools@.len() <= 4294967295)] // D4 D-RANGE-IFACE-CREATE_POOL-4eb401, exactly: state.pools.len() <= u32::MAX as usize
#[requires(log_room(log))]
#[ensures(result.0@ == (*s).pools@.len())]
#[ensures(result.1@ == 0)]
#[ensures(result.2 == None)]
#[ensures(result.3@.len() == 0)]
#[ensures(pool_empty(&(^s).pools@[result.0@]))]
pub fn verify_epsl_create_pool_starts_empty(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace, n: usize,
) -> (u32, usize, Option<u64>, Vec<u64>) {
    let id = state_create_pool(s, log, connected, t);
    let reported = state_len(s, id, t);
    let victim = state_evict(s, id, t);
    let cands = state_candidates(s, id, n, t);
    (id, reported, victim, cands)
}

#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
#[requires((*s).pools@.len() <= 4294967295)] // D4 D-RANGE-IFACE-CREATE_POOL-4eb401, exactly: state.pools.len() <= u32::MAX as usize
#[requires(log_room(log))]
#[ensures(result.1@ == 1)] // FALSE: a brand-new domain tracks nothing
pub fn verify_epsl_create_pool_starts_empty__mutant(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace, n: usize,
) -> (u32, usize, Option<u64>, Vec<u64>) {
    let id = state_create_pool(s, log, connected, t);
    let reported = state_len(s, id, t);
    let victim = state_evict(s, id, t);
    let cands = state_candidates(s, id, n, t);
    (id, reported, victim, cands)
}

// ---- EPSL-CREATE-POOL-ISSUES-A-FRESH-SEQUENTIAL-IDENTIFIER ---------------
#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
// D4 D-RANGE-IFACE-CREATE_POOL-4eb401, exactly, stated at EACH of the two create_pool calls:
// the first sees `len` pools, the second `len + 1` (state_create_pool appends exactly one).
#[requires((*s).pools@.len() <= 4294967295)] // D4 at call 1
#[requires((*s).pools@.len() + 1 <= 4294967295)] // D4 at call 2
#[requires(log_room(log))]
#[ensures(result.0@ == (*s).pools@.len())]
#[ensures(result.1@ == (*s).pools@.len() + 1)]
#[ensures(result.0 != result.1)]
#[ensures((^s).pools@.len() == (*s).pools@.len() + 2)]
pub fn verify_epsl_create_pool_issues_a_fresh_sequential_identifier(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> (u32, u32) {
    let a = state_create_pool(s, log, connected, t);
    let b = state_create_pool(s, log, connected, t);
    (a, b)
}

#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
// D4 D-RANGE-IFACE-CREATE_POOL-4eb401, exactly, stated at EACH of the two create_pool calls:
// the first sees `len` pools, the second `len + 1` (state_create_pool appends exactly one).
#[requires((*s).pools@.len() <= 4294967295)] // D4 at call 1
#[requires((*s).pools@.len() + 1 <= 4294967295)] // D4 at call 2
#[requires(log_room(log))]
#[ensures(result.0 == result.1)] // FALSE: identifiers count upwards and are never reused
pub fn verify_epsl_create_pool_issues_a_fresh_sequential_identifier__mutant(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> (u32, u32) {
    let a = state_create_pool(s, log, connected, t);
    let b = state_create_pool(s, log, connected, t);
    (a, b)
}

// ---- EPSL-CREATE-POOL-PRESERVES-EXISTING-POOLS ---------------------------
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[ensures(forall<i: Int> 0 <= i && i < (*s).pools@.len() ==> (^s).pools@[i] == (*s).pools@[i])]
#[ensures((^s).pools@.len() == (*s).pools@.len() + 1)]
pub fn verify_epsl_create_pool_preserves_existing_pools(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> u32 {
    state_create_pool(s, log, connected, t)
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[ensures((^s).pools@.len() == (*s).pools@.len())] // FALSE: a domain was appended
pub fn verify_epsl_create_pool_preserves_existing_pools__mutant(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> u32 {
    state_create_pool(s, log, connected, t)
}

// ---- EPSL-CREATE-POOL-ANNOUNCES-AT-MOST-ONCE ----------------------------—
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[ensures((^log).info@ <= (*log).info@ + 1)]
#[ensures((*s).announced ==> (^log).info == (*log).info)]
#[ensures((^s).announced == ((*s).announced || connected))]
#[ensures((^s).pools@.len() == (*s).pools@.len() + 3)]
pub fn verify_epsl_create_pool_announces_at_most_once(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> (u32, u32, u32) {
    let a = state_create_pool(s, log, connected, t);
    let b = state_create_pool(s, log, connected, t);
    let c = state_create_pool(s, log, connected, t);
    (a, b, c)
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[ensures((^log).info == (*log).info)] // FALSE: the first create with a logger does announce
pub fn verify_epsl_create_pool_announces_at_most_once__mutant(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> (u32, u32, u32) {
    let a = state_create_pool(s, log, connected, t);
    let b = state_create_pool(s, log, connected, t);
    let c = state_create_pool(s, log, connected, t);
    (a, b, c)
}

// ---- EPSL-ANNOUNCE-LATCH-SET-ONLY-WITH-A-LOGGER -------------------------—
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[ensures(!connected ==> (^s).announced == (*s).announced)]
#[ensures(!connected ==> (^log).info == (*log).info && (^log).debug == (*log).debug)]
#[ensures(connected ==> (^s).announced)]
pub fn verify_epsl_announce_latch_set_only_with_a_logger(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> u32 {
    state_create_pool(s, log, connected, t)
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[ensures((^s).announced)] // FALSE: with no logger attached the latch is not set
pub fn verify_epsl_announce_latch_set_only_with_a_logger__mutant(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> u32 {
    state_create_pool(s, log, connected, t)
}

// ---- EPSL-POOL-COUNT-ASSUMED-BELOW-U32-MAX ------------------------------—
// The `state.pools.len() as u32` narrowing at src/lib.rs:105 is faithful only while the domain
// count fits in a u32; the shipped code has no check, so this is a disclosed assumption, stated
// here as the precondition under which the returned identifier really is the domain count.
#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
#[requires(log_room(log))]
#[requires((*s).pools@.len() <= 4294967295)] // D4 D-RANGE-IFACE-CREATE_POOL-4eb401, exactly: state.pools.len() <= u32::MAX as usize
#[ensures(result@ == (*s).pools@.len())]
#[ensures(result@ <= 4294967295)] // J3: was `< 4294967295`, the off-by-one premise speaking
#[ensures(result@ < (^s).pools@.len())]
pub fn verify_epsl_pool_count_assumed_below_u32_max(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> u32 {
    state_create_pool(s, log, connected, t)
}

#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
#[requires(log_room(log))]
#[requires((*s).pools@.len() <= 4294967295)] // D4 D-RANGE-IFACE-CREATE_POOL-4eb401, exactly: state.pools.len() <= u32::MAX as usize
#[ensures(result@ == (*s).pools@.len() + 1)] // FALSE: the id is the count BEFORE the push
pub fn verify_epsl_pool_count_assumed_below_u32_max__mutant(
    s: &mut Pools, log: &mut Log, connected: bool, t: &mut LockTrace,
) -> u32 {
    state_create_pool(s, log, connected, t)
}

// ======================= track =============================================

// ---- EPSL-TRACK-RETURNS-USABLE-HANDLE -----------------------------------—
// The handle carries the domain the caller named plus the slot the block occupies, and those two
// pieces are exactly what a later refresh / stop-tracking finds the block by.
// J12: was `ready(s,t)` + `log_room` + `pool < len`. "Successfully registers" is now the `Ok` arm of
// the ensures (an unknown domain is the only failure). Premises: `pools_inv` + D5
// (D-RANGE-SC-002-d0657c) exactly at the track call + D6 (D-RANGE-FR-004-7a18db) exactly at each of
// the two calls containing a tick, on the one pool they touch: `clock < u64::MAX` at track and
// `clock' < u64::MAX` at touch; track ticks exactly once (fresh: tick; re-registration: touch), so
// clock' = clock + 1 and the two together are exactly `clock + 1 < u64::MAX`. remove does not tick.
// J12: the `Log` / `LockTrace` observation counters are mirror-only instruments (no production
// state). They are created FRESH (all zero) inside the driver instead of being taken as inputs, so
// the driver carries no premise on them at all (was `ready(s, t)` / `log_room(log)`).
#[requires(pools_inv(s))]
#[requires(pool@ < (*s).pools@.len() ==>
             ((*s).pools@[pool@]).clock@ + 1 < 18446744073709551615
             && ((*s).pools@[pool@]).nodes@.len() <= 4294967295)]
#[ensures(match result.0 {
             Ok(h) => h.pool == pool && result.1 == Ok(()) && result.2 == Ok(()),
             Err(_) => pool@ >= (*s).pools@.len() })]
pub fn verify_epsl_track_returns_usable_handle(
    s: &mut Pools, pool: u32, key: u64, session: u64, connected: bool,
) -> (Result<Handle, PolicyError>, Result<(), PolicyError>, Result<(), PolicyError>) {
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let mut log = Log { info: 0, debug: 0, warn: 0 };
    let h = state_track(s, pool, key, session, &mut log, connected, &mut t);
    match h {
        Ok(hh) => {
            let a = state_touch(s, Handle { pool: hh.pool, index: hh.index }, &mut t);
            let b = state_remove(s, Handle { pool: hh.pool, index: hh.index }, &mut t);
            (Ok(hh), a, b)
        }
        Err(e) => (Err(e), Err(PolicyError::InvalidHandle), Err(PolicyError::InvalidHandle)),
    }
}

#[requires(pools_inv(s))]
#[requires(pool@ < (*s).pools@.len() ==>
             ((*s).pools@[pool@]).clock@ + 1 < 18446744073709551615
             && ((*s).pools@[pool@]).nodes@.len() <= 4294967295)]
#[ensures(!(match result.0 {
             Ok(h) => h.pool == pool && result.1 == Ok(()) && result.2 == Ok(()),
             Err(_) => pool@ >= (*s).pools@.len() }))] // FLIPPED: must fail
pub fn verify_epsl_track_returns_usable_handle__mutant(
    s: &mut Pools, pool: u32, key: u64, session: u64, connected: bool,
) -> (Result<Handle, PolicyError>, Result<(), PolicyError>, Result<(), PolicyError>) {
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let mut log = Log { info: 0, debug: 0, warn: 0 };
    let h = state_track(s, pool, key, session, &mut log, connected, &mut t);
    match h {
        Ok(hh) => {
            let a = state_touch(s, Handle { pool: hh.pool, index: hh.index }, &mut t);
            let b = state_remove(s, Handle { pool: hh.pool, index: hh.index }, &mut t);
            (Ok(hh), a, b)
        }
        Err(e) => (Err(e), Err(PolicyError::InvalidHandle), Err(PolicyError::InvalidHandle)),
    }
}

// ---- EPSL-TRACK-FIRST-BLOCK-IS-HEAD-AND-LEAF ---------------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(!map_mem(p.sessions.e@, session))]
#[ensures(((^p).nodes@[result@]).parent == None)]
#[ensures(((^p).nodes@[result@]).child == None)]
#[ensures(leaves_mem((^p).leaves.e@, (((^p).nodes@[result@]).stamp, result)))]
#[ensures(map_has((^p).sessions.e@, session, result))]
pub fn verify_epsl_track_first_block_is_head_and_leaf(p: &mut Pool, key: u64, session: u64) -> u32 {
    pool_register(p, key, session)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(!map_mem(p.sessions.e@, session))]
#[ensures(((^p).nodes@[result@]).parent != None)] // FALSE: the first block of a session is a head
pub fn verify_epsl_track_first_block_is_head_and_leaf__mutant(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-LINKS-UNDER-CURRENT-LEAF -------------------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(map_has(p.sessions.e@, session, q))]
#[ensures(((^p).nodes@[result@]).parent == Some(q))]
#[ensures(((^p).nodes@[q@]).child == Some(result))]
#[ensures(((^p).nodes@[result@]).child == None)]
#[ensures(map_has((^p).sessions.e@, session, result))]
pub fn verify_epsl_track_links_under_current_leaf(
    p: &mut Pool, key: u64, session: u64, q: u32,
) -> u32 {
    pool_register(p, key, session)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(map_has(p.sessions.e@, session, q))]
#[ensures(((^p).nodes@[result@]).parent == None)] // FALSE: it is linked under the session's leaf
pub fn verify_epsl_track_links_under_current_leaf__mutant(
    p: &mut Pool, key: u64, session: u64, q: u32,
) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-SESSION-COMES-FROM-CALLER (divergent) -------------------—
// The obligation as the specification states it: the block's session is the one the caller named
// on THIS registration. See `refute_epsl_track_session_comes_from_caller` — the code honours this
// only for a key it is not already tracking.
#[requires(pready(p))]
#[ensures(((^p).nodes@[result@]).session == session)]
pub fn verify_epsl_track_session_comes_from_caller(p: &mut Pool, key: u64, session: u64) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-SETS-INITIAL-RECENCY -----------------------------------—
#[requires(pready(p))]
#[ensures(((^p).nodes@[result@]).stamp@ > (*p).clock@)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && ((*p).nodes@[j]).active ==>
             ((*p).nodes@[j]).stamp@ < ((^p).nodes@[result@]).stamp@)]
pub fn verify_epsl_track_sets_initial_recency(p: &mut Pool, key: u64, session: u64) -> u32 {
    pool_register(p, key, session)
}

#[requires(pready(p))]
#[ensures(((^p).nodes@[result@]).stamp@ <= (*p).clock@)] // FALSE: the new stamp is strictly newer
pub fn verify_epsl_track_sets_initial_recency__mutant(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-REREGISTRATION-IS-IDEMPOTENT ---------------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(map_mem(p.by_key.e@, key))]
#[ensures(map_has((*p).by_key.e@, key, result))]
#[ensures((^p).len == (*p).len)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
#[ensures(((^p).nodes@[result@]).parent == ((*p).nodes@[result@]).parent)]
#[ensures(((^p).nodes@[result@]).child == ((*p).nodes@[result@]).child)]
#[ensures(((^p).nodes@[result@]).stamp@ > (*p).clock@)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != result@ ==>
             (^p).nodes@[j] == (*p).nodes@[j])]
pub fn verify_epsl_track_reregistration_is_idempotent(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(map_mem(p.by_key.e@, key))]
#[ensures((^p).len@ == (*p).len@ + 1)] // FALSE: re-registering allocates no second block
pub fn verify_epsl_track_reregistration_is_idempotent__mutant(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-DISTINCT-SESSIONS-GIVE-PLAIN-RECENCY-ORDER -------------—
// Give every block its own session and no block ever has a parent or a child, so every block is
// a one-block chain that is always eviction-eligible and recency alone orders them — plain LRU.
// (That the oldest eligible block is the one evicted is `EPSL-EVICT-PICKS-OLDEST-ELIGIBLE-LEAF`.)
#[requires(pool_empty(p) && pool_inv(p))]
#[requires(k1 != k2)]
#[requires(s1 != s2)]
#[ensures(((^p).nodes@[result.0@]).parent == None && ((^p).nodes@[result.0@]).child == None)]
#[ensures(((^p).nodes@[result.1@]).parent == None && ((^p).nodes@[result.1@]).child == None)]
#[ensures(((^p).nodes@[result.0@]).stamp@ < ((^p).nodes@[result.1@]).stamp@)]
#[ensures(leaves_mem((^p).leaves.e@, (((^p).nodes@[result.0@]).stamp, result.0)))]
#[ensures(leaves_mem((^p).leaves.e@, (((^p).nodes@[result.1@]).stamp, result.1)))]
pub fn verify_epsl_track_distinct_sessions_give_plain_recency_order(
    p: &mut Pool, k1: u64, k2: u64, s1: u64, s2: u64,
) -> (u32, u32) {
    let a = pool_register(p, k1, s1);
    let b = pool_register(p, k2, s2);
    (a, b)
}

#[requires(pool_empty(p) && pool_inv(p))]
#[requires(k1 != k2)]
#[requires(s1 != s2)]
#[ensures(((^p).nodes@[result.1@]).parent != None)] // FALSE: distinct sessions never link
pub fn verify_epsl_track_distinct_sessions_give_plain_recency_order__mutant(
    p: &mut Pool, k1: u64, k2: u64, s1: u64, s2: u64,
) -> (u32, u32) {
    let a = pool_register(p, k1, s1);
    let b = pool_register(p, k2, s2);
    (a, b)
}

// ---- EPSL-TRACK-UNKNOWN-DOMAIN-IS-AN-ERROR -----------------------------—
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ >= (*s).pools@.len())]
#[ensures(result == Err(PolicyError::InvalidPool(pool)))]
#[ensures((^s).pools@ == (*s).pools@)]
pub fn verify_epsl_track_unknown_domain_is_an_error(
    s: &mut Pools, pool: u32, key: u64, session: u64, log: &mut Log, connected: bool,
    t: &mut LockTrace,
) -> Result<Handle, PolicyError> {
    state_track(s, pool, key, session, log, connected, t)
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ >= (*s).pools@.len())]
#[ensures(result != Err(PolicyError::InvalidPool(pool)))] // FALSE: it is exactly this error
pub fn verify_epsl_track_unknown_domain_is_an_error__mutant(
    s: &mut Pools, pool: u32, key: u64, session: u64, log: &mut Log, connected: bool,
    t: &mut LockTrace,
) -> Result<Handle, PolicyError> {
    state_track(s, pool, key, session, log, connected, t)
}

// ---- EPSL-TRACK-DOES-NOT-DISTURB-OTHER-BLOCKS --------------------------—
// Only the new block's own record and the link to the block that used to end its session's chain
// change; no block of any other session moves.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != result@
             && !map_has_i((*p).sessions.e@, session, j) ==>
                (^p).nodes@[j] == (*p).nodes@[j])]
pub fn verify_epsl_track_does_not_disturb_other_blocks(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==> (^p).nodes@[j] == (*p).nodes@[j])]
// FALSE: the new block's own slot, and the demoted leaf's child link, do change
pub fn verify_epsl_track_does_not_disturb_other_blocks__mutant(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-FRESH-KEY-BECOMES-THE-SESSION-LEAF ---------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[ensures((^p).len@ == (*p).len@ + 1)]
#[ensures(((^p).nodes@[result@]).child == None)]
#[ensures(map_has((^p).sessions.e@, session, result))]
#[ensures(leaves_mem((^p).leaves.e@, (((^p).nodes@[result@]).stamp, result)))]
pub fn verify_epsl_track_fresh_key_becomes_the_session_leaf(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[ensures((^p).len == (*p).len)] // FALSE: a fresh key adds exactly one block
pub fn verify_epsl_track_fresh_key_becomes_the_session_leaf__mutant(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-DEMOTES-THE-PREVIOUS-LEAF ------------------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(map_has(p.sessions.e@, session, q))]
#[ensures(!leaves_mem((^p).leaves.e@, (((*p).nodes@[q@]).stamp, q)))]
#[ensures(((^p).nodes@[q@]).child == Some(result))]
#[ensures(((^p).nodes@[q@]).stamp == ((*p).nodes@[q@]).stamp)]
pub fn verify_epsl_track_demotes_the_previous_leaf(
    p: &mut Pool, key: u64, session: u64, q: u32,
) -> u32 {
    pool_register(p, key, session)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(map_has(p.sessions.e@, session, q))]
#[ensures(leaves_mem((^p).leaves.e@, (((*p).nodes@[q@]).stamp, q)))]
// FALSE: the previous leaf leaves the candidate set in the same operation
pub fn verify_epsl_track_demotes_the_previous_leaf__mutant(
    p: &mut Pool, key: u64, session: u64, q: u32,
) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-NEVER-REPORTS-INVALID-HANDLE ---------------------------—
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[ensures(result != Err(PolicyError::InvalidHandle))]
#[ensures(match result { Ok(_) => pool@ < (*s).pools@.len(),
                         Err(e) => e == PolicyError::InvalidPool(pool) })]
pub fn verify_epsl_track_never_reports_invalid_handle(
    s: &mut Pools, pool: u32, key: u64, session: u64, log: &mut Log, connected: bool,
    t: &mut LockTrace,
) -> Result<Handle, PolicyError> {
    state_track(s, pool, key, session, log, connected, t)
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[ensures(result == Err(PolicyError::InvalidHandle))] // FALSE: registration never reports this
pub fn verify_epsl_track_never_reports_invalid_handle__mutant(
    s: &mut Pools, pool: u32, key: u64, session: u64, log: &mut Log, connected: bool,
    t: &mut LockTrace,
) -> Result<Handle, PolicyError> {
    state_track(s, pool, key, session, log, connected, t)
}

// ---- EPSL-TRACK-REUSES-ONLY-FREED-SLOTS --------------------------------—
// The slot is either one the free list held (hence empty) or a brand-new position at the end of
// the arena; it is never a position that still holds a tracked block.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[ensures(result@ < (*p).nodes@.len() ==> !(((*p).nodes@[result@]).active))]
#[ensures(result@ >= (*p).nodes@.len() ==> result@ == (*p).nodes@.len())]
#[ensures(result@ < (^p).nodes@.len())]
pub fn verify_epsl_track_reuses_only_freed_slots(p: &mut Pool, key: u64, session: u64) -> u32 {
    pool_register(p, key, session)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[requires(!map_mem(p.by_key.e@, key))]
#[ensures(result@ < (*p).nodes@.len() ==> (((*p).nodes@[result@]).active))]
// FALSE: a reused slot was empty, never occupied
pub fn verify_epsl_track_reuses_only_freed_slots__mutant(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

// ---- EPSL-TRACK-KEYS-ARE-SCOPED-TO-ONE-POOL ---------------------------—
// The key index is per domain, so the same key is tracked independently in two domains, each with
// its own block, its own session and its own recency.
#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
// D6 D-RANGE-FR-004-7a18db and D5 D-RANGE-SC-002-d0657c, exactly, on the pool each track call
// operates on (pa, then pb; pa != pb so the first call leaves pb untouched)
#[requires(pa@ < (*s).pools@.len() ==> ((*s).pools@[pa@]).clock@ < 18446744073709551615 && ((*s).pools@[pa@]).nodes@.len() <= 4294967295)]
#[requires(pb@ < (*s).pools@.len() ==> ((*s).pools@[pb@]).clock@ < 18446744073709551615 && ((*s).pools@[pb@]).nodes@.len() <= 4294967295)]
#[requires(log_room(log))]
#[requires(pa@ < (*s).pools@.len() && pb@ < (*s).pools@.len())]
#[requires(pa != pb)]
#[ensures(match (result.0, result.1) {
    (Ok(ha), Ok(hb)) => ha.pool == pa && hb.pool == pb
                        && (((^s).pools@[pa@]).nodes@[ha.index@]).key == key
                        && (((^s).pools@[pb@]).nodes@[hb.index@]).key == key,
    _ => false })]
// SHARPENED (fourth pass): each domain's block carries the session the caller named for it — but
// only for a key that domain was not already tracking. On a re-registration the stored session is
// kept, which is the separately recorded `EPSL-TRACK-SESSION-COMES-FROM-CALLER` divergence; the
// unconditional form asserted that divergence away instead of proving key scoping.
#[ensures(!map_mem(((*s).pools@[pa@]).by_key.e@, key) ==> (match result.0 {
    Ok(ha) => (((^s).pools@[pa@]).nodes@[ha.index@]).session == sa, Err(_) => false }))]
#[ensures(!map_mem(((*s).pools@[pb@]).by_key.e@, key) ==> (match result.1 {
    Ok(hb) => (((^s).pools@[pb@]).nodes@[hb.index@]).session == sb, Err(_) => false }))]
pub fn verify_epsl_track_keys_are_scoped_to_one_pool(
    s: &mut Pools, pa: u32, pb: u32, key: u64, sa: u64, sb: u64, log: &mut Log, connected: bool,
    t: &mut LockTrace,
) -> (Result<Handle, PolicyError>, Result<Handle, PolicyError>) {
    let a = state_track(s, pa, key, sa, log, connected, t);
    let b = state_track(s, pb, key, sb, log, connected, t);
    (a, b)
}

#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
// D6 D-RANGE-FR-004-7a18db and D5 D-RANGE-SC-002-d0657c, exactly, on the pool each track call
// operates on (pa, then pb; pa != pb so the first call leaves pb untouched)
#[requires(pa@ < (*s).pools@.len() ==> ((*s).pools@[pa@]).clock@ < 18446744073709551615 && ((*s).pools@[pa@]).nodes@.len() <= 4294967295)]
#[requires(pb@ < (*s).pools@.len() ==> ((*s).pools@[pb@]).clock@ < 18446744073709551615 && ((*s).pools@[pb@]).nodes@.len() <= 4294967295)]
#[requires(log_room(log))]
#[requires(pa@ < (*s).pools@.len() && pb@ < (*s).pools@.len())]
#[requires(pa != pb)]
#[ensures(result.1 == Err(PolicyError::InvalidHandle))]
// FALSE: the second domain tracks the same key perfectly happily
pub fn verify_epsl_track_keys_are_scoped_to_one_pool__mutant(
    s: &mut Pools, pa: u32, pb: u32, key: u64, sa: u64, sb: u64, log: &mut Log, connected: bool,
    t: &mut LockTrace,
) -> (Result<Handle, PolicyError>, Result<Handle, PolicyError>) {
    let a = state_track(s, pa, key, sa, log, connected, t);
    let b = state_track(s, pb, key, sb, log, connected, t);
    (a, b)
}

// ---- EPSL-ARENA-SLOT-COUNT-ASSUMED-BELOW-U32-MAX ----------------------—
// `Pool::alloc`'s `self.nodes.len() as u32` (session_list.rs:73) is faithful only while the arena
// fits in a u32. The shipped code has no check, so this is a disclosed assumption, stated here as
// the precondition under which the slot number a handle carries really names that slot.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(result@ < (^p).nodes@.len())]
// J3: at the D5 edge (`nodes.len() == u32::MAX`, appending) the slot is u32::MAX itself and the
// arena reaches 2^32 slots, so the former `result < u32::MAX` / `len <= u32::MAX` were the slack
// premise speaking. What the range buys is that the cast is lossless: the slot named IS the slot.
#[ensures(((^p).nodes@[result@]).key == key)]
#[ensures((^p).nodes@.len() <= 4294967296)]
pub fn verify_epsl_arena_slot_count_assumed_below_u32_max(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(result@ >= (^p).nodes@.len())] // FALSE: the slot number is always in range
pub fn verify_epsl_arena_slot_count_assumed_below_u32_max__mutant(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_register(p, key, session)
}

// ======================= touch =============================================

// ---- EPSL-TOUCH-REFRESHES-RECENCY ---------------------------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures(result)]
#[ensures(((^p).nodes@[index@]).stamp@ > (*p).clock@)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && ((*p).nodes@[j]).active
             && j != index@ ==>
                ((^p).nodes@[j]).stamp@ < ((^p).nodes@[index@]).stamp@)]
pub fn verify_epsl_touch_refreshes_recency(p: &mut Pool, index: u32) -> bool {
    pool_touch(p, index)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures(((^p).nodes@[index@]).stamp == ((*p).nodes@[index@]).stamp)] // FALSE: it is refreshed
pub fn verify_epsl_touch_refreshes_recency__mutant(p: &mut Pool, index: u32) -> bool {
    pool_touch(p, index)
}

// ---- EPSL-TOUCH-INVALID-HANDLE-IS-AN-ERROR (divergent) ------------------—
// The obligation as the specification states it: a handle naming a block that has already been
// removed must report an error. `refute_epsl_touch_invalid_handle_is_an_error` proves it false —
// once the slot has been reused the refresh silently succeeds against a different block.
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[ensures(!result)]
pub fn verify_epsl_touch_invalid_handle_is_an_error(
    p: &mut Pool, index: u32, newkey: u64, sess: u64,
) -> bool {
    let _ = pool_remove(p, index);
    let _ = pool_register(p, newkey, sess);
    pool_touch(p, index)
}

// ---- EPSL-HANDLE-OPERATIONS-REJECT-UNKNOWN-DOMAIN ----------------------—
#[requires(ready(s, t))]
#[requires(h.pool@ >= (*s).pools@.len())]
#[ensures(result.0 == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures(result.1 == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures((^s).pools@ == (*s).pools@)]
pub fn verify_epsl_handle_operations_reject_unknown_domain(
    s: &mut Pools, h: Handle, t: &mut LockTrace,
) -> (Result<(), PolicyError>, Result<(), PolicyError>) {
    let a = state_touch(s, Handle { pool: h.pool, index: h.index }, t);
    let b = state_remove(s, Handle { pool: h.pool, index: h.index }, t);
    (a, b)
}

#[requires(ready(s, t))]
#[requires(h.pool@ >= (*s).pools@.len())]
#[ensures(result.0 == Err(PolicyError::InvalidHandle))] // FALSE: it is an invalid-DOMAIN error
pub fn verify_epsl_handle_operations_reject_unknown_domain__mutant(
    s: &mut Pools, h: Handle, t: &mut LockTrace,
) -> (Result<(), PolicyError>, Result<(), PolicyError>) {
    let a = state_touch(s, Handle { pool: h.pool, index: h.index }, t);
    let b = state_remove(s, Handle { pool: h.pool, index: h.index }, t);
    (a, b)
}

// ---- EPSL-TOUCH-CHANGES-ONLY-RECENCY -----------------------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures(((^p).nodes@[index@]).parent == ((*p).nodes@[index@]).parent)]
#[ensures(((^p).nodes@[index@]).child == ((*p).nodes@[index@]).child)]
#[ensures(((^p).nodes@[index@]).session == ((*p).nodes@[index@]).session)]
#[ensures(((^p).nodes@[index@]).key == ((*p).nodes@[index@]).key)]
#[ensures((^p).len == (*p).len)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
#[ensures((^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).free@ == (*p).free@)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != index@ ==>
             (^p).nodes@[j] == (*p).nodes@[j])]
pub fn verify_epsl_touch_changes_only_recency(p: &mut Pool, index: u32) -> bool {
    pool_touch(p, index)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures((^p).len@ == (*p).len@ + 1)] // FALSE: a refresh adds nothing
pub fn verify_epsl_touch_changes_only_recency__mutant(p: &mut Pool, index: u32) -> bool {
    pool_touch(p, index)
}

// ---- EPSL-TOUCH-OF-A-LEAF-MOVES-IT-LAST-IN-THE-CANDIDATE-ORDER ---------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[ensures(leaves_mem((^p).leaves.e@, ((^p).clock, index)))]
#[ensures((^p).leaves.e@.len() == (*p).leaves.e@.len())]
#[ensures(forall<k: Int> 0 <= k && k < (^p).leaves.e@.len()
             && (^p).leaves.e@[k] != ((^p).clock, index) ==>
                pair_lt((^p).leaves.e@[k], ((^p).clock, index)))]
pub fn verify_epsl_touch_of_a_leaf_moves_it_last_in_the_candidate_order(
    p: &mut Pool, index: u32,
) -> bool {
    pool_touch(p, index)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[ensures(leaves_mem((^p).leaves.e@, (((*p).nodes@[index@]).stamp, index)))]
// FALSE: the old entry is gone — the block moved to the back of the order
pub fn verify_epsl_touch_of_a_leaf_moves_it_last_in_the_candidate_order__mutant(
    p: &mut Pool, index: u32,
) -> bool {
    pool_touch(p, index)
}

// ---- EPSL-TOUCH-OF-AN-INTERIOR-BLOCK-KEEPS-THE-CANDIDATE-SET -----------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child != None)]
#[ensures(result)]
#[ensures((^p).leaves.e@ == (*p).leaves.e@)]
#[ensures(((^p).nodes@[index@]).stamp@ > (*p).clock@)]
pub fn verify_epsl_touch_of_an_interior_block_keeps_the_candidate_set(
    p: &mut Pool, index: u32,
) -> bool {
    pool_touch(p, index)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child != None)]
#[ensures(!result)] // FALSE: refreshing a protected block is allowed and succeeds
pub fn verify_epsl_touch_of_an_interior_block_keeps_the_candidate_set__mutant(
    p: &mut Pool, index: u32,
) -> bool {
    pool_touch(p, index)
}

// ======================= batch_touch =======================================

// ---- EPSL-BATCH-TOUCH-REFRESHES-EVERY-BLOCK ---------------------------—
// J12: the statement is NOT conditioned on success, so neither is the ensures. "A group of accessed
// blocks" is a group whose every handle names a tracked block: that is D2 (D-RANGE-FR-005-e924ec)
// exactly, at pool level (the pool exists; each handle's slot is occupied). Under it the walk is
// accepted in full and every named block is refreshed.
// J12 premises: `pool_inv` (proved invariant) and D6 (D-RANGE-FR-004-7a18db) `pool.clock < u64::MAX`
// EXACTLY, at every `Pool::touch` call the walk makes: the k-th call is reached iff handles 0..k were
// accepted, each accepted handle ticks once (session_list.rs:62-65), so the clock there is clock + k.
#[requires(pool_inv(p))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len() ==>
              (idxs@[k])@ < p.nodes@.len() && ((p.nodes@[(idxs@[k])@]).active))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len()
              && (forall<j: Int> 0 <= j && j < k ==>
                     (idxs@[j])@ < p.nodes@.len() && ((p.nodes@[(idxs@[j])@]).active)) ==>
                 p.clock@ + k < 18446744073709551615)]
#[ensures(result == Ok(()) &&
             forall<j: Int> 0 <= j && j < idxs@.len() ==>
                ((idxs@[j])@ < (^p).nodes@.len()
                 && ((^p).nodes@[(idxs@[j])@]).stamp@ > (*p).clock@))]
pub fn verify_epsl_batch_touch_refreshes_every_block(
    p: &mut Pool, idxs: &Vec<u32>,
) -> Result<(), PolicyError> {
    pool_batch_touch(p, idxs)
}

#[requires(pool_inv(p))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len() ==>
              (idxs@[k])@ < p.nodes@.len() && ((p.nodes@[(idxs@[k])@]).active))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len()
              && (forall<j: Int> 0 <= j && j < k ==>
                     (idxs@[j])@ < p.nodes@.len() && ((p.nodes@[(idxs@[j])@]).active)) ==>
                 p.clock@ + k < 18446744073709551615)]
#[ensures(!(result == Ok(()) &&
             forall<j: Int> 0 <= j && j < idxs@.len() ==>
                ((idxs@[j])@ < (^p).nodes@.len()
                 && ((^p).nodes@[(idxs@[j])@]).stamp@ > (*p).clock@)))] // FLIPPED: must fail
pub fn verify_epsl_batch_touch_refreshes_every_block__mutant(
    p: &mut Pool, idxs: &Vec<u32>,
) -> Result<(), PolicyError> {
    pool_batch_touch(p, idxs)
}

// ---- EPSL-BATCH-TOUCH-MATCHES-ONE-AT-A-TIME ---------------------------—
// J12 (fresh): over an ARBITRARY group (was: two bare `pool_touch` calls, which never ran the batch).
// One-at-a-time reporting of idxs[0..n] makes the (j+1)-th `touch` give slot idxs[j] recency
// clock + j + 1 and advances the clock by one per call, changing nothing but stamps and the leaf set
// (`pool_touch`'s contract). On success the batch leaves exactly that: the clock is clock + n, the
// LAST mention j of each slot carries clock + j + 1 (so a slot named twice keeps its last mention),
// every unnamed slot is untouched, no key/session/link/liveness changes, the size, key index, session
// index and spare list are unchanged, and the invariant holds -- which fixes the leaf set too, since
// `leaf_set_exact` + `leaves_sorted` make it a function of the arena.
// J12 premises: `pool_inv` (proved invariant) and D6 (D-RANGE-FR-004-7a18db) `pool.clock < u64::MAX`
// EXACTLY, at every `Pool::touch` call the walk makes: the k-th call is reached iff handles 0..k were
// accepted, each accepted handle ticks once (session_list.rs:62-65), so the clock there is clock + k.
#[requires(pool_inv(p))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len()
              && (forall<j: Int> 0 <= j && j < k ==>
                     (idxs@[j])@ < p.nodes@.len() && ((p.nodes@[(idxs@[j])@]).active)) ==>
                 p.clock@ + k < 18446744073709551615)]
#[ensures(result == Ok(()) ==> (^p).clock@ == (*p).clock@ + idxs@.len())]
#[ensures(result == Ok(()) ==>
             forall<j: Int> 0 <= j && j < idxs@.len()
                && (forall<k: Int> j < k && k < idxs@.len() ==> idxs@[k] != idxs@[j]) ==>
                   ((^p).nodes@[(idxs@[j])@]).stamp@ == (*p).clock@ + j + 1)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len()
             && (forall<k: Int> 0 <= k && k < idxs@.len() ==> (idxs@[k])@ != j) ==>
                (^p).nodes@[j] == (*p).nodes@[j])]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
             && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session
             && ((^p).nodes@[j]).parent == ((*p).nodes@[j]).parent
             && ((^p).nodes@[j]).child == ((*p).nodes@[j]).child
             && ((^p).nodes@[j]).active == ((*p).nodes@[j]).active)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len() && (^p).len == (*p).len)]
#[ensures((^p).by_key.e@ == (*p).by_key.e@ && (^p).sessions.e@ == (*p).sessions.e@ && (^p).free@ == (*p).free@)]
#[ensures(pool_inv(&^p))]
pub fn verify_epsl_batch_touch_matches_one_at_a_time(
    p: &mut Pool, idxs: &Vec<u32>,
) -> Result<(), PolicyError> {
    pool_batch_touch(p, idxs)
}

#[requires(pool_inv(p))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len()
              && (forall<j: Int> 0 <= j && j < k ==>
                     (idxs@[j])@ < p.nodes@.len() && ((p.nodes@[(idxs@[j])@]).active)) ==>
                 p.clock@ + k < 18446744073709551615)]
#[ensures(result == Ok(()) ==> (^p).clock@ == (*p).clock@ + idxs@.len())]
#[ensures(!(result == Ok(()) ==>
             forall<j: Int> 0 <= j && j < idxs@.len()
                && (forall<k: Int> j < k && k < idxs@.len() ==> idxs@[k] != idxs@[j]) ==>
                   ((^p).nodes@[(idxs@[j])@]).stamp@ == (*p).clock@ + j + 1))] // FLIPPED: must fail
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len()
             && (forall<k: Int> 0 <= k && k < idxs@.len() ==> (idxs@[k])@ != j) ==>
                (^p).nodes@[j] == (*p).nodes@[j])]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
             && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session
             && ((^p).nodes@[j]).parent == ((*p).nodes@[j]).parent
             && ((^p).nodes@[j]).child == ((*p).nodes@[j]).child
             && ((^p).nodes@[j]).active == ((*p).nodes@[j]).active)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len() && (^p).len == (*p).len)]
#[ensures((^p).by_key.e@ == (*p).by_key.e@ && (^p).sessions.e@ == (*p).sessions.e@ && (^p).free@ == (*p).free@)]
#[ensures(pool_inv(&^p))]
pub fn verify_epsl_batch_touch_matches_one_at_a_time__mutant(
    p: &mut Pool, idxs: &Vec<u32>,
) -> Result<(), PolicyError> {
    pool_batch_touch(p, idxs)
}

// ---- EPSL-BATCH-TOUCH-INVALID-HANDLE-IS-AN-ERROR (divergent) ----------—
// The specification requires an error for ANY handle in the group naming a block that is no longer
// tracked. `refute_epsl_batch_touch_invalid_handle_is_an_error` proves it false.
#[requires(pready(p))]
#[requires(i0@ < p.nodes@.len() && (p.nodes@[i0@]).active)]
#[requires((p.nodes@[i0@]).child == None)]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[ensures(result != Ok(()))]
pub fn verify_epsl_batch_touch_invalid_handle_is_an_error(
    p: &mut Pool, i0: u32, newkey: u64, sess: u64, live: u32,
) -> Result<(), PolicyError> {
    let _ = pool_remove(p, i0);
    let _ = pool_register(p, newkey, sess);
    let mut v: Vec<u32> = Vec::new();
    v.push(live);
    v.push(i0);
    pool_batch_touch(p, &v)
}

// ---- EPSL-BATCH-TOUCH-TOUCHES-NOTHING-ELSE ----------------------------—
// J12: on EITHER outcome (the statement does not condition on success). `j` = a block not named.
// J12 premises: `pool_inv` (proved invariant) and D6 (D-RANGE-FR-004-7a18db) `pool.clock < u64::MAX`
// EXACTLY, at every `Pool::touch` call the walk makes: the k-th call is reached iff handles 0..k were
// accepted, each accepted handle ticks once (session_list.rs:62-65), so the clock there is clock + k.
#[requires(pool_inv(p))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len()
              && (forall<j: Int> 0 <= j && j < k ==>
                     (idxs@[j])@ < p.nodes@.len() && ((p.nodes@[(idxs@[j])@]).active)) ==>
                 p.clock@ + k < 18446744073709551615)]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len() ==> (idxs@[k])@ != j@)]
#[requires(j@ < p.nodes@.len())]
#[ensures((^p).nodes@[j@] == (*p).nodes@[j@])]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
             && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session
             && ((^p).nodes@[j]).parent == ((*p).nodes@[j]).parent
             && ((^p).nodes@[j]).child == ((*p).nodes@[j]).child
             && ((^p).nodes@[j]).active == ((*p).nodes@[j]).active)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
#[ensures((^p).len == (*p).len)]
#[ensures((^p).by_key.e@ == (*p).by_key.e@)]
pub fn verify_epsl_batch_touch_touches_nothing_else(
    p: &mut Pool, idxs: &Vec<u32>, j: u32,
) -> Result<(), PolicyError> {
    pool_batch_touch(p, idxs)
}

#[requires(pool_inv(p))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len()
              && (forall<j: Int> 0 <= j && j < k ==>
                     (idxs@[j])@ < p.nodes@.len() && ((p.nodes@[(idxs@[j])@]).active)) ==>
                 p.clock@ + k < 18446744073709551615)]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len() ==> (idxs@[k])@ != j@)]
#[requires(j@ < p.nodes@.len())]
#[ensures((^p).nodes@[j@] == (*p).nodes@[j@])]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).key == ((*p).nodes@[j]).key
             && ((^p).nodes@[j]).session == ((*p).nodes@[j]).session
             && ((^p).nodes@[j]).parent == ((*p).nodes@[j]).parent
             && ((^p).nodes@[j]).child == ((*p).nodes@[j]).child
             && ((^p).nodes@[j]).active == ((*p).nodes@[j]).active)]
#[ensures((^p).nodes@.len() == (*p).nodes@.len())]
#[ensures(!((^p).len == (*p).len))] // FLIPPED: must fail
#[ensures((^p).by_key.e@ == (*p).by_key.e@)]
pub fn verify_epsl_batch_touch_touches_nothing_else__mutant(
    p: &mut Pool, idxs: &Vec<u32>, j: u32,
) -> Result<(), PolicyError> {
    pool_batch_touch(p, idxs)
}

// ---- EPSL-INV-FAILED-OPERATIONS-CHANGE-NOTHING (divergent) ------------—
// The specification claims a REJECTED operation changes nothing. That is true of every
// single-handle operation — they validate before mutating — and FALSE of the group refresh, which
// refreshes each handle in turn and returns the error the moment it reaches a bad one, with no undo:
// see `refute_epsl_inv_failed_operations_change_nothing`.
//
// As the convention for a divergent id requires, THIS module states the obligation as the
// specification means it — over EVERY operation, the group refresh included — and is therefore
// EXPECTED TO FAIL. Fourth pass: it used to state only the single-handle half, which is TRUE, so it
// PROVED while its own refutation also proved. The gate rejects that pair outright
// (`scorer_creusot.py`'s CONTRADICTION branch: "both '<verify>' and its negation '<refute>' proved")
// and files the property UNRESOLVED rather than `refuted` — which is why this id was the 28th
// UNRESOLVED even though every one of its modules was clean. A divergent `verify_` module that
// proves is a mis-stated obligation, not a result.
#[requires(pready(p))]
#[requires(!(bad@ < p.nodes@.len() && (p.nodes@[bad@]).active))]
#[requires(good@ < p.nodes@.len() && (p.nodes@[good@]).active)]
#[ensures(!result.0 && !result.1 && result.2 != Ok(()))]
#[ensures((^p).nodes@ == (*p).nodes@)]
#[ensures((^p).len == (*p).len && (^p).clock == (*p).clock)]
#[ensures((^p).leaves.e@ == (*p).leaves.e@ && (^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@ && (^p).free@ == (*p).free@)]
pub fn verify_epsl_inv_failed_operations_change_nothing(
    p: &mut Pool, good: u32, bad: u32,
) -> (bool, bool, Result<(), PolicyError>) {
    let a = pool_touch(p, bad);
    let b = pool_remove(p, bad);
    // `batch_touch`'s loop body (src/lib.rs:169-182) inlined for a two-handle group: refresh
    // `good`, then hit `bad` and return the error — with no undo of the refresh already applied.
    let c = if !pool_touch(p, good) {
        Err(PolicyError::InvalidHandle)
    } else if !pool_touch(p, bad) {
        Err(PolicyError::InvalidHandle)
    } else {
        Ok(())
    };
    (a, b, c)
}

// ---- EPSL-BATCH-TOUCH-REJECTS-UNKNOWN-POOL ----------------------------—
// The check happens when the walk first REACHES a handle for that domain, not up front.
#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
// D6 D-RANGE-FR-004-7a18db, exactly, on the one existing pool the group refreshes (h1 names no pool)
#[requires(h0.pool@ < (*s).pools@.len() ==> ((*s).pools@[h0.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool@ < (*s).pools@.len())]
#[requires(h1.pool@ >= (*s).pools@.len())]
#[requires(h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len())]
#[requires((((*s).pools@[h0.pool@]).nodes@[h0.index@]).active)]
#[ensures(result == Err(PolicyError::InvalidPool(h1.pool)))]
pub fn verify_epsl_batch_touch_rejects_unknown_pool(
    s: &mut Pools, h0: Handle, h1: Handle, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_batch_touch2(s, h0, h1, t)
}

#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
// D6 D-RANGE-FR-004-7a18db, exactly, on the one existing pool the group refreshes (h1 names no pool)
#[requires(h0.pool@ < (*s).pools@.len() ==> ((*s).pools@[h0.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool@ < (*s).pools@.len())]
#[requires(h1.pool@ >= (*s).pools@.len())]
#[requires(h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len())]
#[requires((((*s).pools@[h0.pool@]).nodes@[h0.index@]).active)]
#[ensures(result == Ok(()))] // FALSE: the unknown domain in the group fails the whole call
pub fn verify_epsl_batch_touch_rejects_unknown_pool__mutant(
    s: &mut Pools, h0: Handle, h1: Handle, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_batch_touch2(s, h0, h1, t)
}

// ---- EPSL-BATCH-TOUCH-EMPTY-SUCCEEDS-WITHOUT-VALIDATION ---------------—
#[requires(ready(s, t))]
#[requires(hs@.len() == 0)]
#[ensures(result == Ok(()))]
#[ensures((^s).pools@ == (*s).pools@)]
#[ensures((^t).reads == (*t).reads)]
#[ensures((^t).peak@ == 0 && (^t).acquires == (*t).acquires)]
pub fn verify_epsl_batch_touch_empty_succeeds_without_validation(
    s: &mut Pools, hs: &Vec<Handle>, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_batch_touch_n(s, hs, t)
}

#[requires(ready(s, t))]
#[requires(hs@.len() == 0)]
#[ensures((^t).reads@ == (*t).reads@ + 1)] // FALSE: the empty group returns before any lock
pub fn verify_epsl_batch_touch_empty_succeeds_without_validation__mutant(
    s: &mut Pools, hs: &Vec<Handle>, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_batch_touch_n(s, hs, t)
}

// ---- EPSL-BATCH-TOUCH-SPANS-POOLS-ONE-LOCK-AT-A-TIME ------------------—
#[requires(ready(s, t))]
#[ensures((^t).peak@ <= 1)]
pub fn verify_epsl_batch_touch_spans_pools_one_lock_at_a_time(
    s: &mut Pools, h0: Handle, h1: Handle, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_batch_touch2(s, h0, h1, t)
}

#[requires(ready(s, t))]
#[requires(h0.pool@ < (*s).pools@.len())]
#[ensures((^t).peak@ == 0)] // FALSE: refreshing a group does take a per-domain lock
pub fn verify_epsl_batch_touch_spans_pools_one_lock_at_a_time__mutant(
    s: &mut Pools, h0: Handle, h1: Handle, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_batch_touch2(s, h0, h1, t)
}

// ---- EPSL-BATCH-TOUCH-RELOCKS-WHEN-THE-POOL-CHANGES -------------------—
// J12: CREDITED (was `narrowed_verify_…` under the constant-margin `ready(s,t)`).
// Premises: `pools_inv` (proved) + D2 (D-RANGE-FR-005-e924ec) exactly, for both handles of the
// group (each names an existing pool and an occupied slot of it) + D6 (D-RANGE-FR-004-7a18db)
// `pool.clock < u64::MAX` exactly at EACH `Pool::touch` call (the tick, session_list.rs:62-65).
// TICK COUNT, under D2 (both handles accepted, `touch` never changes liveness): the walk makes
// exactly 2 `touch` calls, one tick each. Different pools: 1 tick in each, both at the entry clock,
// so D6 at the two calls is `clock[h0.pool] < MAX && clock[h1.pool] < MAX`. Same pool: 2 ticks in
// that pool, the first at `clock`, the second at `clock + 1` (ticks_in_that_pool - 1 = 1), so D6
// at the two calls is `clock < MAX && clock + 1 < MAX`, i.e. exactly `clock + 1 < MAX`.
// The lock count is returned from a FRESH trace (acquires starts at 0).
// J12: the `Log` / `LockTrace` observation counters are mirror-only instruments (no production
// state). They are created FRESH (all zero) inside the driver instead of being taken as inputs, so
// the driver carries no premise on them at all (was `ready(s, t)` / `log_room(log)`).
#[requires(pools_inv(s))]
#[requires(h0.pool@ < (*s).pools@.len()
              && h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
              && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active)]
#[requires(h1.pool@ < (*s).pools@.len()
              && h1.index@ < ((*s).pools@[h1.pool@]).nodes@.len()
              && (((*s).pools@[h1.pool@]).nodes@[h1.index@]).active)]
#[requires(h0.pool != h1.pool ==> ((*s).pools@[h0.pool@]).clock@ < 18446744073709551615 && ((*s).pools@[h1.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool == h1.pool ==> ((*s).pools@[h0.pool@]).clock@ + 1 < 18446744073709551615)]
#[ensures(h0.pool == h1.pool ==> result.1@ == 1)]
#[ensures(h0.pool != h1.pool ==> result.1@ == 2)]
pub fn verify_epsl_batch_touch_relocks_when_the_pool_changes(
    s: &mut Pools, h0: Handle, h1: Handle,
) -> (Result<(), PolicyError>, usize) {
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let r = state_batch_touch2(s, h0, h1, &mut t);
    (r, t.acquires)
}

#[requires(pools_inv(s))]
#[requires(h0.pool@ < (*s).pools@.len()
              && h0.index@ < ((*s).pools@[h0.pool@]).nodes@.len()
              && (((*s).pools@[h0.pool@]).nodes@[h0.index@]).active)]
#[requires(h1.pool@ < (*s).pools@.len()
              && h1.index@ < ((*s).pools@[h1.pool@]).nodes@.len()
              && (((*s).pools@[h1.pool@]).nodes@[h1.index@]).active)]
#[requires(h0.pool != h1.pool ==> ((*s).pools@[h0.pool@]).clock@ < 18446744073709551615 && ((*s).pools@[h1.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool == h1.pool ==> ((*s).pools@[h0.pool@]).clock@ + 1 < 18446744073709551615)]
#[ensures(!(h0.pool == h1.pool ==> result.1@ == 1))] // FLIPPED: must fail
#[ensures(h0.pool != h1.pool ==> result.1@ == 2)]
pub fn verify_epsl_batch_touch_relocks_when_the_pool_changes__mutant(
    s: &mut Pools, h0: Handle, h1: Handle,
) -> (Result<(), PolicyError>, usize) {
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let r = state_batch_touch2(s, h0, h1, &mut t);
    (r, t.acquires)
}

// ======================= remove ============================================

// ---- EPSL-REMOVE-STOPS-TRACKING-THE-BLOCK -----------------------------—
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures(result)]
#[ensures((^p).len@ == (*p).len@ - 1)]
#[ensures(!(((^p).nodes@[index@]).active))]
#[ensures(!map_mem((^p).by_key.e@, ((*p).nodes@[index@]).key))]
#[ensures(!leaves_mem((^p).leaves.e@, (((*p).nodes@[index@]).stamp, index)))]
#[ensures((^p).free@ == (*p).free@.push_back(index))]
pub fn verify_epsl_remove_stops_tracking_the_block(p: &mut Pool, index: u32) -> bool {
    pool_remove(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures((((^p).nodes@[index@]).active))] // FALSE: the slot is released
pub fn verify_epsl_remove_stops_tracking_the_block__mutant(p: &mut Pool, index: u32) -> bool {
    pool_remove(p, index)
}

// ---- EPSL-REMOVE-RELINKS-THE-CHAIN ------------------------------------—
// J12: was `pready(p)`. Premises: the proved invariant + the obligation's words (a tracked block
// that sits in the middle of a chain: it has a parent q and a child c). `remove` has no declared
// range but D1 (never used) and D7. `links_agree` after = "one unbroken chain, no dangling block".
#[requires(pool_inv(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == Some(c))]
#[requires((p.nodes@[index@]).parent == Some(q))]
#[ensures(((^p).nodes@[c@]).parent == Some(q))]
#[ensures(((^p).nodes@[q@]).child == Some(c))]
#[ensures(((^p).nodes@[c@]).session == ((^p).nodes@[q@]).session)]
#[ensures(links_agree(&^p))]
pub fn verify_epsl_remove_relinks_the_chain(p: &mut Pool, index: u32, q: u32, c: u32) -> bool {
    pool_remove(p, index)
}

#[requires(pool_inv(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == Some(c))]
#[requires((p.nodes@[index@]).parent == Some(q))]
#[ensures(!(((^p).nodes@[c@]).parent == Some(q)))] // FLIPPED: must fail
#[ensures(((^p).nodes@[q@]).child == Some(c))]
#[ensures(((^p).nodes@[c@]).session == ((^p).nodes@[q@]).session)]
#[ensures(links_agree(&^p))]
pub fn verify_epsl_remove_relinks_the_chain__mutant(p: &mut Pool, index: u32, q: u32, c: u32) -> bool {
    pool_remove(p, index)
}

// ---- EPSL-REMOVE-LAST-BLOCK-EMPTIES-THE-SESSION -----------------------—
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires((p.nodes@[index@]).parent == None)]
#[ensures(result)]
#[ensures(!map_mem((^p).sessions.e@, ((*p).nodes@[index@]).session))]
#[ensures(!leaves_mem((^p).leaves.e@, (((*p).nodes@[index@]).stamp, index)))]
pub fn verify_epsl_remove_last_block_empties_the_session(p: &mut Pool, index: u32) -> bool {
    pool_remove(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires((p.nodes@[index@]).parent == None)]
#[ensures(map_mem((^p).sessions.e@, ((*p).nodes@[index@]).session))]
// FALSE: the session is forgotten entirely
pub fn verify_epsl_remove_last_block_empties_the_session__mutant(
    p: &mut Pool, index: u32,
) -> bool {
    pool_remove(p, index)
}

// ---- EPSL-REMOVE-INVALID-HANDLE-IS-AN-ERROR (divergent) ---------------—
// The specification requires an error for a handle naming an already removed block.
// `refute_epsl_remove_invalid_handle_is_an_error` proves it false: once the slot has been reused a
// stale handle silently removes the WRONG block.
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[ensures(!result)]
pub fn verify_epsl_remove_invalid_handle_is_an_error(
    p: &mut Pool, index: u32, newkey: u64, sess: u64,
) -> bool {
    let _ = pool_remove(p, index);
    let _ = pool_register(p, newkey, sess);
    pool_remove(p, index)
}

// ---- EPSL-REMOVE-DISTURBS-ONLY-THE-CHAIN-NEIGHBOURS -------------------—
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != index@
             && !opt_is(((*p).nodes@[index@]).parent, j)
             && !opt_is(((*p).nodes@[index@]).child, j) ==>
                (^p).nodes@[j] == (*p).nodes@[j])]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).stamp == ((*p).nodes@[j]).stamp)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && j != index@ ==>
             ((^p).nodes@[j]).active == ((*p).nodes@[j]).active)]
pub fn verify_epsl_remove_disturbs_only_the_chain_neighbours(p: &mut Pool, index: u32) -> bool {
    pool_remove(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==> (^p).nodes@[j] == (*p).nodes@[j])]
// FALSE: the removed slot itself, and its immediate neighbours' links, do change
pub fn verify_epsl_remove_disturbs_only_the_chain_neighbours__mutant(
    p: &mut Pool, index: u32,
) -> bool {
    pool_remove(p, index)
}

// ---- EPSL-INV-PARENT-BECOMES-ELIGIBLE-WHEN-ITS-CHILD-GOES -------------—
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires((p.nodes@[index@]).parent == Some(q))]
#[ensures(((^p).nodes@[q@]).child == None)]
#[ensures(leaves_mem((^p).leaves.e@, (((*p).nodes@[q@]).stamp, q)))]
#[ensures(map_has((^p).sessions.e@, ((*p).nodes@[index@]).session, q))]
pub fn verify_epsl_inv_parent_becomes_eligible_when_its_child_goes(
    p: &mut Pool, index: u32, q: u32,
) -> bool {
    pool_remove(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires((p.nodes@[index@]).parent == Some(q))]
#[ensures(!leaves_mem((^p).leaves.e@, (((*p).nodes@[q@]).stamp, q)))]
// FALSE: losing its only child makes the parent eligible immediately
pub fn verify_epsl_inv_parent_becomes_eligible_when_its_child_goes__mutant(
    p: &mut Pool, index: u32, q: u32,
) -> bool {
    pool_remove(p, index)
}

// ---- EPSL-REMOVE-REJECTS-UNKNOWN-POOL ---------------------------------—
#[requires(ready(s, t))]
#[requires(h.pool@ >= (*s).pools@.len())]
#[ensures(result == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures((^s).pools@ == (*s).pools@)]
pub fn verify_epsl_remove_rejects_unknown_pool(
    s: &mut Pools, h: Handle, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_remove(s, h, t)
}

#[requires(ready(s, t))]
#[requires(h.pool@ >= (*s).pools@.len())]
#[ensures(result == Ok(()))] // FALSE: an unknown domain is reported, not accepted
pub fn verify_epsl_remove_rejects_unknown_pool__mutant(
    s: &mut Pools, h: Handle, t: &mut LockTrace,
) -> Result<(), PolicyError> {
    state_remove(s, h, t)
}

// ---- EPSL-REMOVE-OF-AN-INTERIOR-BLOCK-KEEPS-THE-CANDIDATES ------------—
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child != None)]
#[ensures(result)]
#[ensures((^p).leaves.e@ == (*p).leaves.e@)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@)]
pub fn verify_epsl_remove_of_an_interior_block_keeps_the_candidates(
    p: &mut Pool, index: u32,
) -> bool {
    pool_remove(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child != None)]
#[ensures((^p).leaves.e@ != (*p).leaves.e@)] // FALSE: the candidate set is untouched
pub fn verify_epsl_remove_of_an_interior_block_keeps_the_candidates__mutant(
    p: &mut Pool, index: u32,
) -> bool {
    pool_remove(p, index)
}

// ---- EPSL-REMOVE-DOES-NOT-REFRESH-RECENCY -----------------------------—
#[requires(pready(p))]
#[ensures((^p).clock == (*p).clock)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).stamp == ((*p).nodes@[j]).stamp)]
pub fn verify_epsl_remove_does_not_refresh_recency(p: &mut Pool, index: u32) -> bool {
    pool_remove(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures((^p).clock@ == (*p).clock@ + 1)] // FALSE: removal is not an access
pub fn verify_epsl_remove_does_not_refresh_recency__mutant(p: &mut Pool, index: u32) -> bool {
    pool_remove(p, index)
}

// ---- EPSL-PROMOTED-PARENT-KEEPS-ITS-OLD-STAMP -------------------------—
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires((p.nodes@[index@]).parent == Some(q))]
#[ensures(((^p).nodes@[q@]).stamp == ((*p).nodes@[q@]).stamp)]
#[ensures(leaves_mem((^p).leaves.e@, (((*p).nodes@[q@]).stamp, q)))]
#[ensures((^p).clock == (*p).clock)]
pub fn verify_epsl_promoted_parent_keeps_its_old_stamp(
    p: &mut Pool, index: u32, q: u32,
) -> bool {
    pool_remove(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires((p.nodes@[index@]).parent == Some(q))]
#[ensures(((^p).nodes@[q@]).stamp@ > (*p).clock@)]
// FALSE: promotion enters the order at the parent's EXISTING recency
pub fn verify_epsl_promoted_parent_keeps_its_old_stamp__mutant(
    p: &mut Pool, index: u32, q: u32,
) -> bool {
    pool_remove(p, index)
}

// ---- EPSL-TRACKED-SIZE-NEVER-UNDERFLOWS -------------------------------—
// The `self.len -= 1` at session_list.rs:183 has no guard; it is reached only after `is_active`
// has confirmed the slot is occupied, and an occupied slot forces `len >= 1`.
#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures((*p).len@ >= 1)]
#[ensures((^p).len@ == (*p).len@ - 1)]
#[ensures((^p).len@ >= 0)]
pub fn verify_epsl_tracked_size_never_underflows(p: &mut Pool, index: u32) -> bool {
    pool_remove(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures((*p).len@ == 0)] // FALSE: an occupied slot means at least one tracked block
pub fn verify_epsl_tracked_size_never_underflows__mutant(p: &mut Pool, index: u32) -> bool {
    pool_remove(p, index)
}

// ======================= identify_next_to_evict ============================

// ---- EPSL-EVICT-PICKS-OLDEST-ELIGIBLE-LEAF ----------------------------—
// The victim is the least `(stamp, index)` entry of the candidate set, and no entry of that set is
// older, so no eligible block with an older recency is ever passed over.
#[requires(pready(p))]
#[requires(p.leaves.e@.len() > 0)]
#[ensures(result == Some(((*p).nodes@[(((*p).leaves.e@[0]).1)@]).key))]
#[ensures(forall<k: Int> 0 <= k && k < (*p).leaves.e@.len() ==>
             (*p).leaves.e@[0] == (*p).leaves.e@[k] || pair_lt((*p).leaves.e@[0], (*p).leaves.e@[k]))]
#[ensures((((*p).nodes@[(((*p).leaves.e@[0]).1)@]).child == None))]
pub fn verify_epsl_evict_picks_oldest_eligible_leaf(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

#[requires(pready(p))]
#[requires(p.leaves.e@.len() > 0)]
#[ensures(result == None)] // FALSE: a non-empty candidate set always yields a victim
pub fn verify_epsl_evict_picks_oldest_eligible_leaf__mutant(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

// ---- EPSL-EVICT-STOPS-TRACKING-THE-VICTIM -----------------------------—
#[requires(pready(p))]
#[requires(p.leaves.e@.len() > 0)]
#[ensures((^p).len@ == (*p).len@ - 1)]
#[ensures(!((((^p).nodes@[(((*p).leaves.e@[0]).1)@]).active)))]
#[ensures(!map_mem((^p).by_key.e@, ((*p).nodes@[(((*p).leaves.e@[0]).1)@]).key))]
#[ensures(!leaves_mem((^p).leaves.e@, (*p).leaves.e@[0]))]
pub fn verify_epsl_evict_stops_tracking_the_victim(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

#[requires(pready(p))]
#[requires(p.leaves.e@.len() > 0)]
#[ensures((^p).len == (*p).len)] // FALSE: the victim stops being tracked in the same operation
pub fn verify_epsl_evict_stops_tracking_the_victim__mutant(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

// ---- EPSL-EVICT-EMPTY-DOMAIN-REPORTS-NOTHING --------------------------—
#[requires(pready(p))]
#[requires(p.len@ == 0)]
#[ensures(result == None)]
#[ensures((^p).nodes@ == (*p).nodes@ && (^p).len == (*p).len && (^p).clock == (*p).clock)]
#[ensures((^p).leaves.e@ == (*p).leaves.e@ && (^p).by_key.e@ == (*p).by_key.e@)]
pub fn verify_epsl_evict_empty_domain_reports_nothing(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

#[requires(pready(p))]
#[requires(p.len@ == 0)]
#[ensures(result != None)] // FALSE: an empty domain has nothing to evict
pub fn verify_epsl_evict_empty_domain_reports_nothing__mutant(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

// ---- EPSL-EVICT-DOES-NOT-REFRESH-THE-VICTIM ---------------------------—
#[requires(pready(p))]
#[ensures((^p).clock == (*p).clock)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).stamp == ((*p).nodes@[j]).stamp)]
pub fn verify_epsl_evict_does_not_refresh_the_victim(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

#[requires(pready(p))]
#[requires(p.leaves.e@.len() > 0)]
#[ensures((^p).clock@ == (*p).clock@ + 1)] // FALSE: selection is not an access
pub fn verify_epsl_evict_does_not_refresh_the_victim__mutant(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

// ---- EPSL-EVICT-DISTURBS-ONLY-VICTIM-AND-PARENT ----------------------—
// J12: was `pready(p)` + `leaves.len() > 0`. Now only the proved invariant; "evicting a block" is
// the `leaves.len() > 0` case (a victim exists iff the leaf set is non-empty), moved into the ensures.
// Exactly one block stops being tracked (len drops by one and the victim's slot is freed).
#[requires(pool_inv(p))]
#[ensures((*p).leaves.e@.len() > 0 ==>
             forall<j: Int> 0 <= j && j < (*p).nodes@.len()
             && j != (((*p).leaves.e@[0]).1)@
             && !opt_is(((*p).nodes@[(((*p).leaves.e@[0]).1)@]).parent, j) ==>
                (^p).nodes@[j] == (*p).nodes@[j])]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).stamp == ((*p).nodes@[j]).stamp)]
#[ensures((*p).leaves.e@.len() > 0 ==> (^p).len@ == (*p).len@ - 1)]
#[ensures((*p).leaves.e@.len() == 0 ==> ^p == *p)]
pub fn verify_epsl_evict_disturbs_only_victim_and_parent(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

#[requires(pool_inv(p))]
#[ensures(!((*p).leaves.e@.len() > 0 ==>
             forall<j: Int> 0 <= j && j < (*p).nodes@.len()
             && j != (((*p).leaves.e@[0]).1)@
             && !opt_is(((*p).nodes@[(((*p).leaves.e@[0]).1)@]).parent, j) ==>
                (^p).nodes@[j] == (*p).nodes@[j]))] // FLIPPED: must fail
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() ==>
             ((^p).nodes@[j]).stamp == ((*p).nodes@[j]).stamp)]
#[ensures((*p).leaves.e@.len() > 0 ==> (^p).len@ == (*p).len@ - 1)]
#[ensures((*p).leaves.e@.len() == 0 ==> ^p == *p)]
pub fn verify_epsl_evict_disturbs_only_victim_and_parent__mutant(p: &mut Pool) -> Option<u64> {
    pool_evict_oldest(p)
}

// ======================= queries, clear ====================================

// ---- EPSL-READONLY-QUERIES-DEGRADE-SAFELY-ON-UNKNOWN-DOMAIN -----------—
// J12: was `ready(s, t)`. Premises: the proved invariant + the obligation's words (unknown domain).
// J12: the `Log` / `LockTrace` observation counters are mirror-only instruments (no production
// state). They are created FRESH (all zero) inside the driver instead of being taken as inputs, so
// the driver carries no premise on them at all (was `ready(s, t)` / `log_room(log)`).
#[requires(pools_inv(s))]
#[requires(pool@ >= (*s).pools@.len())]
#[ensures(result.0 == None)]
#[ensures(result.1@.len() == 0)]
#[ensures(result.2@ == 0)]
#[ensures((^s).pools@ == (*s).pools@)]
#[ensures((^s).announced == (*s).announced)]
pub fn verify_epsl_readonly_queries_degrade_safely_on_unknown_domain(
    s: &mut Pools, pool: u32, n: usize,
) -> (Option<u64>, Vec<u64>, usize) {
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let v = state_evict(s, pool, &mut t);
    state_clear_pool(s, pool, &mut t);
    let c = state_candidates(s, pool, n, &mut t);
    let l = state_len(s, pool, &mut t);
    (v, c, l)
}

#[requires(pools_inv(s))]
#[requires(pool@ >= (*s).pools@.len())]
#[ensures(result.0 == None)]
#[ensures(result.1@.len() == 0)]
#[ensures(!(result.2@ == 0))] // FLIPPED: must fail
#[ensures((^s).pools@ == (*s).pools@)]
#[ensures((^s).announced == (*s).announced)]
pub fn verify_epsl_readonly_queries_degrade_safely_on_unknown_domain__mutant(
    s: &mut Pools, pool: u32, n: usize,
) -> (Option<u64>, Vec<u64>, usize) {
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let v = state_evict(s, pool, &mut t);
    state_clear_pool(s, pool, &mut t);
    let c = state_candidates(s, pool, n, &mut t);
    let l = state_len(s, pool, &mut t);
    (v, c, l)
}

// ---- EPSL-INV-A-BLOCK-WITH-A-DESCENDANT-IS-NEVER-EVICTED --------------—
// Everything the candidate set holds, and therefore everything a victim request can return, is a
// slot whose `child` is `None`. A block with a tracked descendant is simply not in the set.
#[requires(pready(p))]
#[requires(n@ > 0)]
#[ensures(forall<k: Int> 0 <= k && k < (*p).leaves.e@.len() ==>
             ((*p).nodes@[(((*p).leaves.e@[k]).1)@]).child == None)]
#[ensures((*p).leaves.e@.len() > 0 ==>
             ((*p).nodes@[(((*p).leaves.e@[0]).1)@]).child == None)]
#[ensures(forall<i: Int> 0 <= i && i < (*p).nodes@.len() && ((*p).nodes@[i]).active
             && ((*p).nodes@[i]).child != None ==>
                !leaves_has_idx_i((*p).leaves.e@, i))]
pub fn verify_epsl_inv_a_block_with_a_descendant_is_never_evicted(
    p: &mut Pool, n: usize,
) -> (Vec<u64>, Option<u64>) {
    let c = pool_candidates(p, n);
    let v = pool_evict_oldest(p);
    (c, v)
}

#[requires(pready(p))]
#[requires(n@ > 0)]
#[requires(p.leaves.e@.len() > 0)]
#[ensures(((*p).nodes@[(((*p).leaves.e@[0]).1)@]).child != None)]
// FALSE: only childless blocks are ever candidates
pub fn verify_epsl_inv_a_block_with_a_descendant_is_never_evicted__mutant(
    p: &mut Pool, n: usize,
) -> (Vec<u64>, Option<u64>) {
    let c = pool_candidates(p, n);
    let v = pool_evict_oldest(p);
    (c, v)
}

// ---- EPSL-INV-VICTIM-CHOICE-IS-REPEATABLE -----------------------------—
// Selection reads only the tracking state, and because no two tracked blocks share a recency value
// the eligible blocks are strictly ordered with a unique oldest — so two requests in the same state
// name the same victim and produce the same list.
#[requires(pready(p))]
#[ensures(result.0@ == result.1@)]
#[ensures(forall<i: Int, j: Int> 0 <= i && i < j && j < p.nodes@.len()
             && ((p.nodes@[i]).active) && ((p.nodes@[j]).active) ==>
                (p.nodes@[i]).stamp != (p.nodes@[j]).stamp)]
#[ensures(forall<k: Int> 0 <= k && k < p.leaves.e@.len() && k > 0 ==>
             pair_lt(p.leaves.e@[0], p.leaves.e@[k]))]
pub fn verify_epsl_inv_victim_choice_is_repeatable(p: &Pool, n: usize) -> (Vec<u64>, Vec<u64>) {
    let a = pool_candidates(p, n);
    let b = pool_candidates(p, n);
    (a, b)
}

#[requires(pready(p))]
#[requires(p.leaves.e@.len() > 1)]
#[ensures(pair_lt(p.leaves.e@[1], p.leaves.e@[0]))]
// FALSE: the candidate order is ascending, so entry 0 is the smallest
pub fn verify_epsl_inv_victim_choice_is_repeatable__mutant(
    p: &Pool, n: usize,
) -> (Vec<u64>, Vec<u64>) {
    let a = pool_candidates(p, n);
    let b = pool_candidates(p, n);
    (a, b)
}

// ---- EPSL-CANDIDATES-NEVER-MORE-THAN-ASKED ----------------------------—
#[requires(pready(p))]
#[ensures(result@.len() <= n@)]
#[ensures(result@.len() <= p.leaves.e@.len())]
#[ensures(p.leaves.e@.len() <= n@ ==> result@.len() == p.leaves.e@.len())]
#[ensures(n@ == 0 ==> result@.len() == 0)]
pub fn verify_epsl_candidates_never_more_than_asked(p: &Pool, n: usize) -> Vec<u64> {
    pool_candidates(p, n)
}

#[requires(pready(p))]
#[ensures(result@.len() > n@)] // FALSE: never more than asked for
pub fn verify_epsl_candidates_never_more_than_asked__mutant(p: &Pool, n: usize) -> Vec<u64> {
    pool_candidates(p, n)
}

// ---- EPSL-CANDIDATES-LISTED-IN-EVICTION-ORDER (divergent) -------------—
// The specification promises the whole list is the eviction SEQUENCE. The code guarantees only the
// first entry: see `refute_epsl_candidates_listed_in_eviction_order`.
#[requires(pready(p))]
#[requires(p.leaves.e@.len() >= 2)]
#[requires(n@ >= 2)]
#[ensures(result.1 == Some(result.0@[1]))]
pub fn verify_epsl_candidates_listed_in_eviction_order(
    p: &mut Pool, n: usize,
) -> (Vec<u64>, Option<u64>, Option<u64>) {
    let c = pool_candidates(p, n);
    let v1 = pool_evict_oldest(p);
    let v2 = pool_evict_oldest(p);
    (c, v2, v1)
}

// ---- EPSL-CANDIDATES-ARE-DISTINCT -------------------------------------—
// Entries are strictly ascending, so they name pairwise different slots; and each names a childless
// block of its own session, so no two entries share a session.
#[requires(pready(p))]
#[ensures(forall<a: Int, b: Int> 0 <= a && a < b && b < p.leaves.e@.len() ==>
             (p.leaves.e@[a]).1 != (p.leaves.e@[b]).1)]
#[ensures(forall<a: Int, b: Int> 0 <= a && a < b && b < p.leaves.e@.len() ==>
             (p.nodes@[((p.leaves.e@[a]).1)@]).session != (p.nodes@[((p.leaves.e@[b]).1)@]).session)]
#[ensures(p.leaves.e@.len() == p.sessions.e@.len())]
pub fn verify_epsl_candidates_are_distinct(p: &Pool, n: usize) -> Vec<u64> {
    pool_candidates(p, n)
}

#[requires(pready(p))]
#[requires(p.leaves.e@.len() >= 2)]
#[ensures((p.leaves.e@[0]).1 == (p.leaves.e@[1]).1)] // FALSE: entries name different slots
pub fn verify_epsl_candidates_are_distinct__mutant(p: &Pool, n: usize) -> Vec<u64> {
    pool_candidates(p, n)
}

// ---- EPSL-CANDIDATES-EMPTY-DOMAIN-RETURNS-EMPTY-LIST ------------------—
// J12: was `pready(p)` (clock/arena slack). `get_eviction_candidates` has no declared range but D7.
#[requires(pool_inv(p))]
#[requires(p.len@ == 0)]
#[ensures(result@.len() == 0)]
pub fn verify_epsl_candidates_empty_domain_returns_empty_list(p: &Pool, n: usize) -> Vec<u64> {
    pool_candidates(p, n)
}

#[requires(pool_inv(p))]
#[requires(p.len@ == 0)]
#[ensures(!(result@.len() == 0))] // FLIPPED: must fail
pub fn verify_epsl_candidates_empty_domain_returns_empty_list__mutant(p: &Pool, n: usize) -> Vec<u64> {
    pool_candidates(p, n)
}

// ---- EPSL-CANDIDATES-CHANGE-NOTHING -----------------------------------—
// `Pool::candidates` takes `&self` (session_list.rs:196), so it cannot mutate anything; asking
// twice in a row returns the same list.
#[requires(pready(p))]
#[ensures(result.0@ == result.1@)]
#[ensures((^p).nodes@ == (*p).nodes@)]
#[ensures((^p).len == (*p).len && (^p).clock == (*p).clock)]
#[ensures((^p).leaves.e@ == (*p).leaves.e@ && (^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@ && (^p).free@ == (*p).free@)]
pub fn verify_epsl_candidates_change_nothing(p: &mut Pool, n: usize) -> (Vec<u64>, Vec<u64>) {
    let a = pool_candidates(p, n);
    let b = pool_candidates(p, n);
    (a, b)
}

#[requires(pready(p))]
#[requires(p.len@ > 0)]
#[ensures((^p).len@ == (*p).len@ - 1)] // FALSE: asking for candidates removes nothing
pub fn verify_epsl_candidates_change_nothing__mutant(
    p: &mut Pool, n: usize,
) -> (Vec<u64>, Vec<u64>) {
    let a = pool_candidates(p, n);
    let b = pool_candidates(p, n);
    (a, b)
}

// ---- EPSL-CANDIDATES-ARE-LEAVES-OLDEST-FIRST --------------------------—
#[requires(pready(p))]
#[ensures(forall<j: Int> 0 <= j && j < result@.len() ==>
             result@[j] == (p.nodes@[((p.leaves.e@[j]).1)@]).key
             && (p.nodes@[((p.leaves.e@[j]).1)@]).child == None
             && (p.nodes@[((p.leaves.e@[j]).1)@]).active)]
#[ensures(forall<a: Int, b: Int> 0 <= a && a < b && b < result@.len() ==>
             (p.nodes@[((p.leaves.e@[a]).1)@]).stamp@ < (p.nodes@[((p.leaves.e@[b]).1)@]).stamp@)]
pub fn verify_epsl_candidates_are_leaves_oldest_first(p: &Pool, n: usize) -> Vec<u64> {
    pool_candidates(p, n)
}

#[requires(pready(p))]
#[requires(p.leaves.e@.len() >= 2 && n@ >= 2)]
#[ensures((p.nodes@[((p.leaves.e@[1]).1)@]).stamp@ < (p.nodes@[((p.leaves.e@[0]).1)@]).stamp@)]
// FALSE: the list is least-recently-accessed FIRST
pub fn verify_epsl_candidates_are_leaves_oldest_first__mutant(p: &Pool, n: usize) -> Vec<u64> {
    pool_candidates(p, n)
}

// ---- EPSL-CANDIDATES-FIRST-IS-THE-NEXT-VICTIM -------------------------—
#[requires(pready(p))]
#[requires(p.leaves.e@.len() > 0)]
#[requires(n@ > 0)]
#[ensures(result.0@.len() > 0)]
#[ensures(result.1 == Some(result.0@[0]))]
pub fn verify_epsl_candidates_first_is_the_next_victim(
    p: &mut Pool, n: usize,
) -> (Vec<u64>, Option<u64>) {
    let c = pool_candidates(p, n);
    let v = pool_evict_oldest(p);
    (c, v)
}

#[requires(pready(p))]
#[requires(p.leaves.e@.len() > 0)]
#[requires(n@ > 0)]
#[ensures(result.1 != Some(result.0@[0]))] // FALSE: both read the same first entry
pub fn verify_epsl_candidates_first_is_the_next_victim__mutant(
    p: &mut Pool, n: usize,
) -> (Vec<u64>, Option<u64>) {
    let c = pool_candidates(p, n);
    let v = pool_evict_oldest(p);
    (c, v)
}

// ---- EPSL-CANDIDATES-UNKNOWN-POOL-RETURNS-EMPTY -----------------------—
#[requires(ready(s, t))]
#[requires(pool@ >= s.pools@.len())]
#[ensures(result@.len() == 0)]
pub fn verify_epsl_candidates_unknown_pool_returns_empty(
    s: &Pools, pool: u32, n: usize, t: &mut LockTrace,
) -> Vec<u64> {
    state_candidates(s, pool, n, t)
}

#[requires(ready(s, t))]
#[requires(pool@ >= s.pools@.len())]
#[requires(n@ > 0)]
#[ensures(result@.len() == n@)] // FALSE: an unknown domain yields an empty list
pub fn verify_epsl_candidates_unknown_pool_returns_empty__mutant(
    s: &Pools, pool: u32, n: usize, t: &mut LockTrace,
) -> Vec<u64> {
    state_candidates(s, pool, n, t)
}

// ---- EPSL-LEN-REPORTS-TRACKED-COUNT -----------------------------------—
// The reported size counts every tracked block, at every position in a chain, not only the eligible
// ones; it rises by one per fresh registration and falls by one per removal.
#[requires(pready(p))]
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(idx@ < p.nodes@.len() && (p.nodes@[idx@]).active)]
#[ensures(result.0@ == (*p).len@)]
#[ensures(result.1@ == (*p).len@ + 1)]
#[ensures(result.2@ == (*p).len@)]
// RESTATED (fourth pass): the old clause here was `result.0@ >= p.leaves.e@.len()`, i.e.
// |leaves| <= len. That is a CARDINALITY theorem about the mirror containers - it holds because the
// session index injects into the live blocks - and a first-order prover cannot count a finite set
// without a construction. Measured with a single-clause probe (`probe_len_ge_leaves`): it is the one
// goal of this module that does not close, and the other three do.
// The substance of "counts every tracked block, at every position in a chain, not only the eligible
// ones" is stated here pointwise instead, which is a statement about the component rather than about
// the mirror's cardinalities, and is what the data structure actually maintains:
//   (a) the reported size is exactly the number of tracked keys, and
//   (b) every ELIGIBLE block is one of those tracked blocks - so the count is over all of them, not
//       just the candidate set.
#[ensures(result.0@ == (*p).by_key.e@.len())]
#[ensures(forall<k: Int> 0 <= k && k < (*p).leaves.e@.len() ==>
             map_has_i((*p).by_key.e@, ((*p).nodes@[(((*p).leaves.e@[k]).1)@]).key,
                       (((*p).leaves.e@[k]).1)@))]
pub fn verify_epsl_len_reports_tracked_count(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> (usize, usize, usize) {
    let before = pool_len(p);
    let _ = pool_register(p, key, session);
    let after_add = pool_len(p);
    let _ = pool_remove(p, idx);
    let after_del = pool_len(p);
    (before, after_add, after_del)
}

#[requires(pready(p))]
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(idx@ < p.nodes@.len() && (p.nodes@[idx@]).active)]
#[ensures(result.1@ == (*p).len@)] // FALSE: a fresh registration raises the count by one
pub fn verify_epsl_len_reports_tracked_count__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> (usize, usize, usize) {
    let before = pool_len(p);
    let _ = pool_register(p, key, session);
    let after_add = pool_len(p);
    let _ = pool_remove(p, idx);
    let after_del = pool_len(p);
    (before, after_add, after_del)
}

// ---- EPSL-LEN-CHANGES-NOTHING -----------------------------------------—
#[requires(pready(p))]
#[ensures(result.0 == result.1)]
#[ensures((^p).nodes@ == (*p).nodes@)]
#[ensures((^p).len == (*p).len && (^p).clock == (*p).clock)]
#[ensures((^p).leaves.e@ == (*p).leaves.e@ && (^p).by_key.e@ == (*p).by_key.e@)]
#[ensures((^p).sessions.e@ == (*p).sessions.e@ && (^p).free@ == (*p).free@)]
pub fn verify_epsl_len_changes_nothing(p: &mut Pool) -> (usize, usize) {
    let a = pool_len(p);
    let b = pool_len(p);
    (a, b)
}

#[requires(pready(p))]
#[ensures((^p).clock@ == (*p).clock@ + 1)] // FALSE: an enquiry does not advance the counter
pub fn verify_epsl_len_changes_nothing__mutant(p: &mut Pool) -> (usize, usize) {
    let a = pool_len(p);
    let b = pool_len(p);
    (a, b)
}

// ---- EPSL-LEN-UNKNOWN-POOL-RETURNS-ZERO -------------------------------—
#[requires(ready(s, t))]
#[requires(pool@ >= s.pools@.len())]
#[ensures(result@ == 0)]
pub fn verify_epsl_len_unknown_pool_returns_zero(
    s: &Pools, pool: u32, t: &mut LockTrace,
) -> usize {
    state_len(s, pool, t)
}

#[requires(ready(s, t))]
#[requires(pool@ >= s.pools@.len())]
#[ensures(result@ > 0)] // FALSE: an unknown domain reports zero, not an error and not a count
pub fn verify_epsl_len_unknown_pool_returns_zero__mutant(
    s: &Pools, pool: u32, t: &mut LockTrace,
) -> usize {
    state_len(s, pool, t)
}

// ======================= clear_pool ========================================

// ---- EPSL-CLEAR-RETURNS-DOMAIN-TO-EMPTY -------------------------------—
// J12: NO premise (was `pready(p)`): `Pool::clear` is total and establishes `pool_inv` itself.
#[ensures(pool_empty(&^p))]
#[ensures(result.0@ == 0)]
#[ensures(result.1 == None)]
#[ensures(result.2@.len() == 0)]
#[ensures((^p).clock@ == 0)]
#[ensures((^p).nodes@.len() == 0)]
pub fn verify_epsl_clear_returns_domain_to_empty(
    p: &mut Pool, n: usize,
) -> (usize, Option<u64>, Vec<u64>) {
    pool_clear(p);
    let l = pool_len(p);
    let v = pool_evict_oldest(p);
    let c = pool_candidates(p, n);
    (l, v, c)
}

#[ensures(pool_empty(&^p))]
#[ensures(!(result.0@ == 0))] // FLIPPED: must fail
#[ensures(result.1 == None)]
#[ensures(result.2@.len() == 0)]
#[ensures((^p).clock@ == 0)]
#[ensures((^p).nodes@.len() == 0)]
pub fn verify_epsl_clear_returns_domain_to_empty__mutant(
    p: &mut Pool, n: usize,
) -> (usize, Option<u64>, Vec<u64>) {
    pool_clear(p);
    let l = pool_len(p);
    let v = pool_evict_oldest(p);
    let c = pool_candidates(p, n);
    (l, v, c)
}

// ---- EPSL-CLEAR-INVALIDATES-EXISTING-HANDLES (divergent) --------------—
// The specification promises a pre-clear handle is rejected. `refute_epsl_clear_invalidates_
// existing_handles` proves it false: clearing restarts slot numbering, so once enough blocks have
// been registered the same handle names a live but unrelated block.
#[requires(pready(p))]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[ensures(!result)]
pub fn verify_epsl_clear_invalidates_existing_handles(
    p: &mut Pool, newkey: u64, sess: u64,
) -> bool {
    pool_clear(p);
    let _ = pool_register(p, newkey, sess);
    pool_touch(p, 0)
}

// ---- EPSL-CLEAR-LEAVES-DOMAIN-USABLE ---------------------------------—
#[requires(pready(p))]
#[ensures(result@ == 0)]
#[ensures(((^p).nodes@[result@]).parent == None)]
#[ensures(((^p).nodes@[result@]).child == None)]
#[ensures(((^p).nodes@[result@]).key == key)]
#[ensures(((^p).nodes@[result@]).session == session)]
#[ensures((^p).len@ == 1)]
pub fn verify_epsl_clear_leaves_domain_usable(p: &mut Pool, key: u64, session: u64) -> u32 {
    pool_clear(p);
    pool_register(p, key, session)
}

#[requires(pready(p))]
#[ensures(((^p).nodes@[result@]).parent != None)]
// FALSE: after a clear the session has no blocks, so the new one is a head
pub fn verify_epsl_clear_leaves_domain_usable__mutant(
    p: &mut Pool, key: u64, session: u64,
) -> u32 {
    pool_clear(p);
    pool_register(p, key, session)
}

// ---- EPSL-CLEAR-POOL-UNKNOWN-POOL-IS-A-SILENT-NO-OP ------------------—
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ >= (*s).pools@.len())]
#[ensures((^s).pools@ == (*s).pools@)]
#[ensures((^s).announced == (*s).announced)]
#[ensures((^log).info == (*log).info && (^log).debug == (*log).debug
          && (^log).warn == (*log).warn)]
pub fn verify_epsl_clear_pool_unknown_pool_is_a_silent_no_op(
    s: &mut Pools, pool: u32, log: &mut Log, t: &mut LockTrace,
) {
    state_clear_pool(s, pool, t);
    let _ = log;
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ >= (*s).pools@.len())]
#[ensures((^log).warn@ == (*log).warn@ + 1)] // FALSE: it reports nothing at all
pub fn verify_epsl_clear_pool_unknown_pool_is_a_silent_no_op__mutant(
    s: &mut Pools, pool: u32, log: &mut Log, t: &mut LockTrace,
) {
    state_clear_pool(s, pool, t);
    let _ = log;
}

// ---- EPSL-CLEAR-POOL-LEAVES-OTHER-POOLS-ALONE ------------------------—
#[requires(ready(s, t))]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() && j != pool@ ==>
             (^s).pools@[j] == (*s).pools@[j])]
#[ensures((^s).pools@.len() == (*s).pools@.len())]
pub fn verify_epsl_clear_pool_leaves_other_pools_alone(
    s: &mut Pools, pool: u32, t: &mut LockTrace,
) {
    state_clear_pool(s, pool, t)
}

#[requires(ready(s, t))]
#[ensures((^s).pools@.len() == (*s).pools@.len() - 1)] // FALSE: clearing destroys no domain
pub fn verify_epsl_clear_pool_leaves_other_pools_alone__mutant(
    s: &mut Pools, pool: u32, t: &mut LockTrace,
) {
    state_clear_pool(s, pool, t)
}

// ======================= global invariants =================================
// Each of these is proved as a PRESERVATION obligation: assume the invariant conjunct (as part of
// `pool_inv`), run the mutator, and re-establish the same conjunct. `register`, `touch`, `remove`
// and `clear` are the only four mutators, and `identify_next_to_evict` goes through `remove`'s
// `unlink`, so preserving a conjunct across these four preserves it everywhere.

// ---- EPSL-INV-AT-MOST-ONE-CHILD --------------------------------------—
// A `child` slot holds at most one index by construction; the content of the obligation is that the
// chain never BRANCHES, i.e. no two live blocks record the same parent. That follows from the
// mutual-link invariant: a parent's single `child` slot can only point back at one of them.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(links_agree(&^p))]
#[ensures(forall<a: Int, b: Int> 0 <= a && a < b && b < (^p).nodes@.len()
             && ((^p).nodes@[a]).active && ((^p).nodes@[b]).active
             && ((^p).nodes@[a]).parent != None ==>
                ((^p).nodes@[a]).parent != ((^p).nodes@[b]).parent)]
pub fn verify_epsl_inv_at_most_one_child(p: &mut Pool, key: u64, session: u64, idx: u32) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    r
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(!links_agree(&^p))] // FALSE: the mutual-link invariant is maintained
pub fn verify_epsl_inv_at_most_one_child__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    r
}

// ---- EPSL-INV-EXACTLY-ONE-LEAF-PER-SESSION ---------------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(one_leaf_per_session(&^p))]
#[ensures(sessions_ok(&^p))]
#[ensures((^p).leaves.e@.len() == (^p).sessions.e@.len())]
pub fn verify_epsl_inv_exactly_one_leaf_per_session(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures((^p).leaves.e@.len() != (^p).sessions.e@.len())]
// FALSE: one candidate per non-empty session, always
pub fn verify_epsl_inv_exactly_one_leaf_per_session__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

// ---- EPSL-INV-NO-CYCLES-IN-LINEAGE -----------------------------------—
// Encoded by the strictly decreasing measure `birth` along `parent` (see `chain_birth_decreases`):
// a block's parent was registered strictly earlier, the measure is a natural number bounded above
// by the clock, so the walk up the chain strictly decreases and must terminate. No block is its own
// parent and no two blocks are each other's parent.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(chain_birth_decreases(&^p))]
#[ensures(forall<i: Int> 0 <= i && i < (^p).nodes@.len() && ((^p).nodes@[i]).active ==>
             !opt_is(((^p).nodes@[i]).parent, i))]
#[ensures(forall<i: Int> 0 <= i && i < (^p).nodes@.len() && ((^p).nodes@[i]).active ==>
             (match ((^p).nodes@[i]).parent {
                Some(x) => !opt_is(((^p).nodes@[x@]).parent, i), None => true }))]
pub fn verify_epsl_inv_no_cycles_in_lineage(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(!chain_birth_decreases(&^p))] // FALSE: the measure strictly decreases along every link
pub fn verify_epsl_inv_no_cycles_in_lineage__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

// ---- EPSL-INV-NO-ORPHANED-BLOCKS ------------------------------------—
// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(links_agree(&^p))]
pub fn verify_epsl_inv_no_orphaned_blocks(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(!links_agree(&^p))] // FALSE: both directions of every link always agree
pub fn verify_epsl_inv_no_orphaned_blocks__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// ---- EPSL-INV-ELIGIBLE-SET-IS-EXACTLY-THE-CHILDLESS -----------------—
// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(leaf_set_exact(&^p))]
pub fn verify_epsl_inv_eligible_set_is_exactly_the_childless(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(!leaf_set_exact(&^p))] // FALSE: the set is kept exactly in step with the chains
pub fn verify_epsl_inv_eligible_set_is_exactly_the_childless__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// ---- EPSL-INV-SESSION-POINTS-AT-ITS-OWN-END-OF-CHAIN ---------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(sessions_ok(&^p))]
pub fn verify_epsl_inv_session_points_at_its_own_end_of_chain(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(!sessions_ok(&^p))] // FALSE: the remembered leaf is always live, its own and childless
pub fn verify_epsl_inv_session_points_at_its_own_end_of_chain__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

// ---- EPSL-INV-ONE-TRACKED-BLOCK-PER-CACHE-KEY ---------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(by_key_ok(&^p))]
#[ensures((^p).by_key.e@.len() == (^p).len@)]
pub fn verify_epsl_inv_one_tracked_block_per_cache_key(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures((^p).by_key.e@.len() != (^p).len@) ] // FALSE: exactly one entry per tracked block
pub fn verify_epsl_inv_one_tracked_block_per_cache_key__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

// ---- EPSL-INV-SIZE-ACCOUNTING-IS-EXACT ---------------------------—
// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(size_exact(&^p))]
#[ensures((^p).nodes@.len() == (^p).len@ + (^p).free@.len())]
pub fn verify_epsl_inv_size_accounting_is_exact(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures((^p).nodes@.len() != (^p).len@ + (^p).free@.len())]
// FALSE: occupied plus spare is always the whole storage area
pub fn verify_epsl_inv_size_accounting_is_exact__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// ---- EPSL-INV-LINEAGE-NEVER-CROSSES-SESSIONS ---------------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(lineage_in_session(&^p))]
pub fn verify_epsl_inv_lineage_never_crosses_sessions(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(!lineage_in_session(&^p))] // FALSE: rejoining a chain never mixes sessions
pub fn verify_epsl_inv_lineage_never_crosses_sessions__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

// ---- EPSL-INV-BLOCK-BELONGS-TO-EXACTLY-ONE-SESSION ---------------—
// `Node::session` is written once, in the struct literal at session_list.rs:100, and never
// rewritten. The split on `map_mem` states "written at birth AND never rewritten"; the
// re-registration-with-another-session case is the separate divergent row
// EPSL-TRACK-SESSION-COMES-FROM-CALLER.
// J12: was `pready(p)`. Premises: `pool_inv` + D5 (D-RANGE-SC-002-d0657c) exactly at the register
// call, and D6 (D-RANGE-FR-004-7a18db) exactly at EACH of the two calls that contain a tick:
// `clock < u64::MAX` at register and `clock' < u64::MAX` at touch, where register ticks exactly once
// (fresh path: tick; re-registration path: `touch` of the existing block, one tick), so clock' =
// clock + 1 and the two D6 instances are together exactly `clock + 1 < u64::MAX`.
#[requires(pool_inv(p))]
#[requires(p.clock@ + 1 < 18446744073709551615)]
#[requires(p.nodes@.len() <= 4294967295)]
#[ensures(!map_mem((*p).by_key.e@, key) ==> ((^p).nodes@[result@]).session == session)]
#[ensures(map_mem((*p).by_key.e@, key) ==>
             ((^p).nodes@[result@]).session == ((*p).nodes@[result@]).session)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && ((*p).nodes@[j]).active
             && j != result@ ==>
                ((^p).nodes@[j]).session == ((*p).nodes@[j]).session)]
pub fn verify_epsl_inv_block_belongs_to_exactly_one_session(
    p: &mut Pool, key: u64, session: u64, ti: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_touch(p, ti);
    r
}

#[requires(pool_inv(p))]
#[requires(p.clock@ + 1 < 18446744073709551615)]
#[requires(p.nodes@.len() <= 4294967295)]
#[ensures(!(!map_mem((*p).by_key.e@, key) ==> ((^p).nodes@[result@]).session == session))] // FLIPPED: must fail
#[ensures(map_mem((*p).by_key.e@, key) ==>
             ((^p).nodes@[result@]).session == ((*p).nodes@[result@]).session)]
#[ensures(forall<j: Int> 0 <= j && j < (*p).nodes@.len() && ((*p).nodes@[j]).active
             && j != result@ ==>
                ((^p).nodes@[j]).session == ((*p).nodes@[j]).session)]
pub fn verify_epsl_inv_block_belongs_to_exactly_one_session__mutant(
    p: &mut Pool, key: u64, session: u64, ti: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_touch(p, ti);
    r
}

// ---- EPSL-INV-DOMAINS-ARE-INDEPENDENT ---------------------------—
// Every operation reaches at most the one domain it names.
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ < (*s).pools@.len())]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() && j != pool@ ==>
             (^s).pools@[j] == (*s).pools@[j])]
#[ensures((^s).pools@.len() == (*s).pools@.len())]
pub fn verify_epsl_inv_domains_are_independent(
    s: &mut Pools, pool: u32, key: u64, session: u64, idx: u32, n: usize,
    log: &mut Log, connected: bool, t: &mut LockTrace,
) -> (Result<Handle, PolicyError>, Option<u64>, usize) {
    let h = state_track(s, pool, key, session, log, connected, t);
    let a = state_touch(s, Handle { pool, index: idx }, t);
    let b = state_remove(s, Handle { pool, index: idx }, t);
    let v = state_evict(s, pool, t);
    let c = state_candidates(s, pool, n, t);
    let l = state_len(s, pool, t);
    state_clear_pool(s, pool, t);
    let _ = a;
    let _ = b;
    let _ = c;
    (h, v, l)
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ < (*s).pools@.len())]
#[ensures((^s).pools@.len() == (*s).pools@.len() + 1)]
// FALSE: none of these operations creates a domain
pub fn verify_epsl_inv_domains_are_independent__mutant(
    s: &mut Pools, pool: u32, key: u64, session: u64, idx: u32, n: usize,
    log: &mut Log, connected: bool, t: &mut LockTrace,
) -> (Result<Handle, PolicyError>, Option<u64>, usize) {
    let h = state_track(s, pool, key, session, log, connected, t);
    let a = state_touch(s, Handle { pool, index: idx }, t);
    let b = state_remove(s, Handle { pool, index: idx }, t);
    let v = state_evict(s, pool, t);
    let c = state_candidates(s, pool, n, t);
    let l = state_len(s, pool, t);
    state_clear_pool(s, pool, t);
    let _ = a;
    let _ = b;
    let _ = c;
    (h, v, l)
}

// ---- EPSL-INV-RECENCY-STRICTLY-ADVANCES (divergent) --------------—
// The specification promises strict advance for the whole life of a domain.
// `refute_epsl_inv_recency_strictly_advances` proves it false across a clear, which resets the
// counter to zero (session_list.rs:226).
#[requires(pready(p))]
#[requires(!map_mem(p.by_key.e@, k1))]
#[ensures(((^p).nodes@[result.1@]).stamp@ > result.0@)]
pub fn verify_epsl_inv_recency_strictly_advances(
    p: &mut Pool, k1: u64, k2: u64, sess: u64,
) -> (u64, u32) {
    let a = pool_register(p, k1, sess);
    let first = p.nodes[a as usize].stamp;
    pool_clear(p);
    let b = pool_register(p, k2, sess);
    (first, b)
}

// ---- EPSL-INV-HANDLES-KEEP-NAMING-THEIR-BLOCK (divergent) --------—
// While a block stays tracked its handle is stable, and that half IS true: nothing but `unlink`
// ever frees a slot. The half the specification also promises — that a handle never comes to name a
// DIFFERENT block — is false: `refute_epsl_inv_handles_keep_naming_their_block`.
#[requires(pready(p))]
#[requires(idx@ < p.nodes@.len() && (p.nodes@[idx@]).active)]
#[requires((p.nodes@[idx@]).child == None)]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[requires(newkey != (p.nodes@[idx@]).key)]
#[ensures(((^p).nodes@[idx@]).key == ((*p).nodes@[idx@]).key)]
pub fn verify_epsl_inv_handles_keep_naming_their_block(
    p: &mut Pool, idx: u32, newkey: u64, sess: u64,
) -> u32 {
    let _ = pool_remove(p, idx);
    pool_register(p, newkey, sess)
}

// ---- EPSL-NO-INPUT-CAN-MAKE-AN-OPERATION-PANIC -------------------—
// Totality: Creusot discharges every implicit panic obligation (index bounds, arithmetic overflow,
// `unwrap`) as part of proving a function, so a green proof over ARBITRARY inputs IS the no-panic
// claim. Every one of the nine operations is fed unconstrained arguments -- any domain id, any
// handle, any group, any key, any session, any requested count.
// J12 (fresh): was ONE state threaded through every operation under `ready(s,t)` (a constant margin
// on every pool's clock/arena/count) and without `create_pool`. Now each operation that can tick or
// allocate gets its OWN arbitrary state satisfying the proved `pools_inv` and EXACTLY the declared
// ranges of that operation at that call, so no premise accumulates across calls:
//   track  (st): D6 + D5 on the pool it names;   touch (su): D6 on the pool it names;
//   batch_touch (sb, group [h0, h1]): D6 at each `Pool::touch` call the walk makes (the 2nd call is
//     reached only if h0 was accepted; in the same pool its clock is then clock + 1);
//   remove / identify_next_to_evict / len / get_eviction_candidates / clear_pool (sr): no range;
//   create_pool (sc): D4 (D-RANGE-IFACE-CREATE_POOL-4eb401) `state.pools.len() <= u32::MAX`.
// D7 (D-RANGE-FR-015-e06e85, locks never poisoned) is the lock model itself: `state_read_lock`,
// `state_write_lock` and `lock_acquire` are total (every `.unwrap()` on a lock succeeds), which is
// exactly the assumption that no lock is poisoned at entry. D1 is not used.
// J12: the `Log` / `LockTrace` observation counters are mirror-only instruments (no production
// state). They are created FRESH (all zero) inside the driver instead of being taken as inputs, so
// the driver carries no premise on them at all (was `ready(s, t)` / `log_room(log)`).
#[requires(pools_inv(st))]
#[requires(pool@ < (*st).pools@.len() ==>
             ((*st).pools@[pool@]).clock@ < 18446744073709551615 && ((*st).pools@[pool@]).nodes@.len() <= 4294967295)]
#[requires(pools_inv(su))]
#[requires(h.pool@ < (*su).pools@.len() ==> ((*su).pools@[h.pool@]).clock@ < 18446744073709551615)]
#[requires(pools_inv(sb))]
#[requires(h0.pool@ < (*sb).pools@.len() ==> ((*sb).pools@[h0.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool@ < (*sb).pools@.len()
              && h0.index@ < ((*sb).pools@[h0.pool@]).nodes@.len()
              && (((*sb).pools@[h0.pool@]).nodes@[h0.index@]).active
              && h1.pool@ < (*sb).pools@.len() && h1.pool != h0.pool ==>
             ((*sb).pools@[h1.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool@ < (*sb).pools@.len()
              && h0.index@ < ((*sb).pools@[h0.pool@]).nodes@.len()
              && (((*sb).pools@[h0.pool@]).nodes@[h0.index@]).active
              && h1.pool == h0.pool ==>
             ((*sb).pools@[h0.pool@]).clock@ + 1 < 18446744073709551615)]
#[requires(pools_inv(sr))]
#[requires(pools_inv(sc))]
#[requires((*sc).pools@.len() <= 4294967295)]
#[ensures((^sr).pools@.len() == (*sr).pools@.len())]
#[ensures((^sc).pools@.len() == (*sc).pools@.len() + 1)]
pub fn verify_epsl_no_input_can_make_an_operation_panic(
    st: &mut Pools, su: &mut Pools, sb: &mut Pools, sr: &mut Pools, sc: &mut Pools,
    pool: u32, key: u64, session: u64, h: Handle, h0: Handle, h1: Handle, n: usize,
    connected: bool,
) -> (Result<Handle, PolicyError>, Option<u64>, usize, Vec<u64>, u32) {
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let mut log = Log { info: 0, debug: 0, warn: 0 };
    let a = state_track(st, pool, key, session, &mut log, connected, &mut t);
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let b = state_touch(su, h, &mut t);
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let d = state_batch_touch2(sb, h0, h1, &mut t);
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let c = state_remove(sr, h, &mut t);
    let v = state_evict(sr, pool, &mut t);
    let l = state_len(sr, pool, &mut t);
    let e = state_candidates(sr, pool, n, &mut t);
    state_clear_pool(sr, pool, &mut t);
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let mut log = Log { info: 0, debug: 0, warn: 0 };
    let id = state_create_pool(sc, &mut log, connected, &mut t);
    let _ = (b, c, d);
    (a, v, l, e, id)
}

#[requires(pools_inv(st))]
#[requires(pool@ < (*st).pools@.len() ==>
             ((*st).pools@[pool@]).clock@ < 18446744073709551615 && ((*st).pools@[pool@]).nodes@.len() <= 4294967295)]
#[requires(pools_inv(su))]
#[requires(h.pool@ < (*su).pools@.len() ==> ((*su).pools@[h.pool@]).clock@ < 18446744073709551615)]
#[requires(pools_inv(sb))]
#[requires(h0.pool@ < (*sb).pools@.len() ==> ((*sb).pools@[h0.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool@ < (*sb).pools@.len()
              && h0.index@ < ((*sb).pools@[h0.pool@]).nodes@.len()
              && (((*sb).pools@[h0.pool@]).nodes@[h0.index@]).active
              && h1.pool@ < (*sb).pools@.len() && h1.pool != h0.pool ==>
             ((*sb).pools@[h1.pool@]).clock@ < 18446744073709551615)]
#[requires(h0.pool@ < (*sb).pools@.len()
              && h0.index@ < ((*sb).pools@[h0.pool@]).nodes@.len()
              && (((*sb).pools@[h0.pool@]).nodes@[h0.index@]).active
              && h1.pool == h0.pool ==>
             ((*sb).pools@[h0.pool@]).clock@ + 1 < 18446744073709551615)]
#[requires(pools_inv(sr))]
#[requires(pools_inv(sc))]
#[requires((*sc).pools@.len() <= 4294967295)]
#[ensures((^sr).pools@.len() == (*sr).pools@.len())]
#[ensures(!((^sc).pools@.len() == (*sc).pools@.len() + 1))] // FLIPPED: must fail
pub fn verify_epsl_no_input_can_make_an_operation_panic__mutant(
    st: &mut Pools, su: &mut Pools, sb: &mut Pools, sr: &mut Pools, sc: &mut Pools,
    pool: u32, key: u64, session: u64, h: Handle, h0: Handle, h1: Handle, n: usize,
    connected: bool,
) -> (Result<Handle, PolicyError>, Option<u64>, usize, Vec<u64>, u32) {
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let mut log = Log { info: 0, debug: 0, warn: 0 };
    let a = state_track(st, pool, key, session, &mut log, connected, &mut t);
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let b = state_touch(su, h, &mut t);
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let d = state_batch_touch2(sb, h0, h1, &mut t);
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let c = state_remove(sr, h, &mut t);
    let v = state_evict(sr, pool, &mut t);
    let l = state_len(sr, pool, &mut t);
    let e = state_candidates(sr, pool, n, &mut t);
    state_clear_pool(sr, pool, &mut t);
    let mut t = LockTrace { held: 0, peak: 0, acquires: 0, writes: 0, reads: 0 };
    let mut log = Log { info: 0, debug: 0, warn: 0 };
    let id = state_create_pool(sc, &mut log, connected, &mut t);
    let _ = (b, c, d);
    (a, v, l, e, id)
}

// ---- EPSL-HANDLE-VALIDATION-IS-OCCUPANCY-ONLY -------------------—
// `Pool::is_active` reads exactly two things: whether the position is in range, and whether it is
// occupied. Its answer therefore depends on nothing else — not the key, not the session, not which
// registration issued the handle.
#[requires(pready(p))]
#[ensures(result == (index@ < p.nodes@.len() && ((p.nodes@[index@]).active)))]
#[ensures(result ==> ((p.nodes@[index@]).active))]
#[ensures(!result ==> index@ >= p.nodes@.len() || !((p.nodes@[index@]).active))]
pub fn verify_epsl_handle_validation_is_occupancy_only(p: &Pool, index: u32) -> bool {
    pool_is_active(p, index)
}

#[requires(pready(p))]
#[requires(index@ < p.nodes@.len() && (p.nodes@[index@]).active)]
#[ensures(!result)] // FALSE: an occupied in-range position passes validation
pub fn verify_epsl_handle_validation_is_occupancy_only__mutant(p: &Pool, index: u32) -> bool {
    pool_is_active(p, index)
}

// ---- EPSL-INV-ACTIVE-STAMPS-ARE-DISTINCT ------------------------—
// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(stamps_distinct(&^p))]
#[ensures(clock_dominates(&^p))]
pub fn verify_epsl_inv_active_stamps_are_distinct(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(!stamps_distinct(&^p))] // FALSE: every value handed out is fresh
pub fn verify_epsl_inv_active_stamps_are_distinct__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// ---- EPSL-INV-FREE-LIST-IS-EXACTLY-THE-EMPTY-SLOTS --------------—
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(free_exact(&^p))]
pub fn verify_epsl_inv_free_list_is_exactly_the_empty_slots(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly: pool.clock < u64::MAX
#[requires(p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly: pool.nodes.len() <= u32::MAX as usize
#[ensures(!free_exact(&^p))] // FALSE: the spare list holds each empty slot exactly once
pub fn verify_epsl_inv_free_list_is_exactly_the_empty_slots__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

// ---- EPSL-INV-NO-LIVE-LINK-POINTS-AT-AN-EMPTY-SLOT --------------—
// Nothing stored refers to an empty position: link targets, remembered session leaves, candidate
// entries and key-index entries all name live blocks.
// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(links_agree(&^p))]
#[ensures(sessions_ok(&^p))]
#[ensures(leaf_set_exact(&^p))]
#[ensures(by_key_ok(&^p))]
pub fn verify_epsl_inv_no_live_link_points_at_an_empty_slot(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// J3: restated as ONE STEP of every mutator from any state satisfying the proved invariant,
// each branch under exactly the declared range of the method it calls (D6 for track/touch,
// D5 for track, nothing for remove/evict). The former fixed chain register->touch->... needed
// the clock to survive TWO ticks, i.e. a premise stronger than D6; invariance over the chain
// follows from this step by induction.
#[requires(pool_inv(p))]
#[requires(op@ <= 1 ==> p.clock@ < 18446744073709551615)] // D6 D-RANGE-FR-004-7a18db, exactly, at track (op 0) / touch (op 1)
#[requires(op@ == 0 ==> p.nodes@.len() <= 4294967295)] // D5 D-RANGE-SC-002-d0657c, exactly, at track (op 0)
#[ensures(!by_key_ok(&^p))] // FALSE: every key-index entry names a live block holding that key
pub fn verify_epsl_inv_no_live_link_points_at_an_empty_slot__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32, op: u8,
) -> u32 {
    match op {
        0 => pool_register(p, key, session),
        1 => {
            let _ = pool_touch(p, ti);
            0
        }
        2 => {
            let _ = pool_remove(p, idx);
            0
        }
        _ => {
            let _ = pool_evict_oldest(p);
            0
        }
    }
}

// ---- EPSL-INV-POOL-IDS-STAY-VALID-FOREVER -----------------------—
// `EvictionState::pools` is only ever appended to, so an identifier keeps naming the same domain and
// is never recycled: no operation shortens or reorders the vector.
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ < (*s).pools@.len())]
#[ensures((^s).pools@.len() >= (*s).pools@.len())]
#[ensures(forall<j: Int> 0 <= j && j < (*s).pools@.len() ==> j < (^s).pools@.len())]
#[ensures(result@ < (^s).pools@.len())]
pub fn verify_epsl_inv_pool_ids_stay_valid_forever(
    s: &mut Pools, pool: u32, key: u64, session: u64, idx: u32,
    log: &mut Log, connected: bool, t: &mut LockTrace,
) -> u32 {
    let a = state_track(s, pool, key, session, log, connected, t);
    let b = state_remove(s, Handle { pool, index: idx }, t);
    state_clear_pool(s, pool, t);
    let _ = a;
    let _ = b;
    state_create_pool(s, log, connected, t)
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ < (*s).pools@.len())]
#[ensures((^s).pools@.len() < (*s).pools@.len())] // FALSE: the domain vector never shrinks
pub fn verify_epsl_inv_pool_ids_stay_valid_forever__mutant(
    s: &mut Pools, pool: u32, key: u64, session: u64, idx: u32,
    log: &mut Log, connected: bool, t: &mut LockTrace,
) -> u32 {
    let a = state_track(s, pool, key, session, log, connected, t);
    let b = state_remove(s, Handle { pool, index: idx }, t);
    state_clear_pool(s, pool, t);
    let _ = a;
    let _ = b;
    state_create_pool(s, log, connected, t)
}

// ---- EPSL-INV-ARENA-ONLY-GROWS-EXCEPT-ON-CLEAR ------------------—
#[requires(pready(p))]
#[ensures((^p).nodes@.len() >= (*p).nodes@.len())]
#[ensures((^p).nodes@.len() <= (*p).nodes@.len() + 1)]
pub fn verify_epsl_inv_arena_only_grows_except_on_clear(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_touch(p, ti);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

#[requires(pready(p))]
#[requires(p.nodes@.len() > 0)]
#[ensures((^p).nodes@.len() < (*p).nodes@.len())]
// FALSE: removing or evicting marks a position empty but never shrinks the area
pub fn verify_epsl_inv_arena_only_grows_except_on_clear__mutant(
    p: &mut Pool, key: u64, session: u64, idx: u32, ti: u32,
) -> u32 {
    let r = pool_register(p, key, session);
    let _ = pool_touch(p, ti);
    let _ = pool_remove(p, idx);
    let _ = pool_evict_oldest(p);
    r
}

// ---- EPSL-ACCESS-CLOCK-ASSUMED-NOT-TO-OVERFLOW ------------------—
// `self.clock += 1` (session_list.rs:63) has NO overflow guard. Creusot will not discharge the
// addition without the assumption, so the assumption is forced to appear as a precondition — which
// is exactly the disclosure the obligation asks for. Under it the counter advances soundly.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)]
#[ensures(result@ == (*p).clock@ + 1)]
#[ensures((^p).clock == result)]
#[ensures(result@ > (*p).clock@)]
pub fn verify_epsl_access_clock_assumed_not_to_overflow(p: &mut Pool) -> u64 {
    pool_tick(p)
}

#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551615)]
#[ensures(result@ == (*p).clock@)] // FALSE: the counter advances by exactly one
pub fn verify_epsl_access_clock_assumed_not_to_overflow__mutant(p: &mut Pool) -> u64 {
    pool_tick(p)
}

// ---- EPSL-LOCKS-ASSUMED-NEVER-POISONED --------------------------—
// `RwLock::read().unwrap()` / `Mutex::lock().unwrap()` abort if a previous holder panicked.
// Creusot cannot translate either type, so both are replaced by a TRUSTED, total model
// (`state_read_lock` / `state_write_lock` / `lock_acquire` never fail). That model IS the
// assumption: what is proved here is that, GIVEN no lock was ever abandoned, every operation runs
// to completion. Poisoning itself is outside the model — recorded as `trusted-boundary`.
#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
// D6 D-RANGE-FR-004-7a18db and D5 D-RANGE-SC-002-d0657c, exactly, on the pool the track call operates on
#[requires(pool@ < (*s).pools@.len() ==> ((*s).pools@[pool@]).clock@ < 18446744073709551615 && ((*s).pools@[pool@]).nodes@.len() <= 4294967295)]
#[requires(log_room(log))]
#[requires(pool@ < (*s).pools@.len())]
#[ensures((^t).reads@ > (*t).reads@)]
#[ensures((^t).held@ == 0)]
pub fn verify_epsl_locks_assumed_never_poisoned(
    s: &mut Pools, pool: u32, key: u64, session: u64, log: &mut Log, connected: bool,
    t: &mut LockTrace,
) -> Result<Handle, PolicyError> {
    let h = state_track(s, pool, key, session, log, connected, t);
    let v = state_evict(s, pool, t);
    let _ = v;
    h
}

#[requires(ready_exact(s, t))] // J3: `ready` minus every pool clock/arena/count range
// D6 D-RANGE-FR-004-7a18db and D5 D-RANGE-SC-002-d0657c, exactly, on the pool the track call operates on
#[requires(pool@ < (*s).pools@.len() ==> ((*s).pools@[pool@]).clock@ < 18446744073709551615 && ((*s).pools@[pool@]).nodes@.len() <= 4294967295)]
#[requires(log_room(log))]
#[requires(pool@ < (*s).pools@.len())]
#[ensures((^t).reads == (*t).reads)] // FALSE: each operation takes the shared state lock
pub fn verify_epsl_locks_assumed_never_poisoned__mutant(
    s: &mut Pools, pool: u32, key: u64, session: u64, log: &mut Log, connected: bool,
    t: &mut LockTrace,
) -> Result<Handle, PolicyError> {
    let h = state_track(s, pool, key, session, log, connected, t);
    let v = state_evict(s, pool, t);
    let _ = v;
    h
}

// ---- EPSL-ONLY-POOL-CREATION-TAKES-THE-EXCLUSIVE-LOCK -----------—
// Creating a domain is the only operation that takes the exclusive state lock; every other takes the
// shared lock and then at most one per-domain lock.
#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ < (*s).pools@.len())]
#[ensures((^t).writes == (*t).writes)]
#[ensures((^t).reads@ > (*t).reads@)]
#[ensures((^t).peak@ <= 1)]
pub fn verify_epsl_only_pool_creation_takes_the_exclusive_lock(
    s: &mut Pools, pool: u32, key: u64, session: u64, h: Handle, n: usize,
    log: &mut Log, connected: bool, t: &mut LockTrace,
) -> (Result<Handle, PolicyError>, usize) {
    let a = state_track(s, pool, key, session, log, connected, t);
    let b = state_touch(s, Handle { pool: h.pool, index: h.index }, t);
    let c = state_remove(s, Handle { pool: h.pool, index: h.index }, t);
    let d = state_batch_touch2(s, h, Handle { pool: h.pool, index: h.index }, t);
    let v = state_evict(s, pool, t);
    let e = state_candidates(s, pool, n, t);
    let l = state_len(s, pool, t);
    state_clear_pool(s, pool, t);
    let _ = (b, c, d, v, e);
    (a, l)
}

#[requires(ready(s, t))]
#[requires(log_room(log))]
#[requires(pool@ < (*s).pools@.len())]
#[ensures((^t).writes@ > (*t).writes@)] // FALSE: only create_pool takes the exclusive lock
pub fn verify_epsl_only_pool_creation_takes_the_exclusive_lock__mutant(
    s: &mut Pools, pool: u32, key: u64, session: u64, h: Handle, n: usize,
    log: &mut Log, connected: bool, t: &mut LockTrace,
) -> (Result<Handle, PolicyError>, usize) {
    let a = state_track(s, pool, key, session, log, connected, t);
    let b = state_touch(s, Handle { pool: h.pool, index: h.index }, t);
    let c = state_remove(s, Handle { pool: h.pool, index: h.index }, t);
    let d = state_batch_touch2(s, h, Handle { pool: h.pool, index: h.index }, t);
    let v = state_evict(s, pool, t);
    let e = state_candidates(s, pool, n, t);
    let l = state_len(s, pool, t);
    state_clear_pool(s, pool, t);
    let _ = (b, c, d, v, e);
    (a, l)
}

// ---- EPSL-EVICT-NONEMPTY-DOMAIN-ALWAYS-YIELDS-A-VICTIM ----------—
/// J12: every live block's chain reaches a live CHILDLESS block. By induction along `child`:
/// a child is live with `parent == Some(i)` (`child_links_agree`), so its registration stamp
/// `birth` is strictly larger (`chain_birth_decreases`) and bounded by the clock -- the measure
/// `clock - birth` strictly decreases and stays >= 0. Uses only the proved invariant `pool_inv`.
#[logic]
#[requires(pool_inv(p))]
#[variant(p.clock@ - (p.nodes@[i]).birth@)]
#[ensures(0 <= i && i < p.nodes@.len() && (p.nodes@[i]).active ==>
             exists<j: Int> 0 <= j && j < p.nodes@.len() && (p.nodes@[j]).active
                 && (p.nodes@[j]).child == None)]
pub fn lemma_chain_reaches_a_leaf(p: &Pool, i: Int) {
    pearlite! {
        if 0 <= i && i < p.nodes@.len() && (p.nodes@[i]).active {
            match (p.nodes@[i]).child {
                Some(c) => lemma_chain_reaches_a_leaf(p, c@),
                None => (),
            }
        } else {
            ()
        }
    }
}

// J12 (fresh): was `pready(p)` and proved only "non-empty LEAF SET => victim". The statement is about
// a domain that TRACKS at least one block (`len > 0`): now proved directly. len > 0 gives a live
// block (the key index has len entries, each naming a live slot); the lemma walks its chain to a
// live childless block; `leaf_in_set` puts that in the candidate set; a non-empty set yields a
// victim. Premise: only the proved invariant.
#[requires(pool_inv(p))]
#[ensures((*p).len@ > 0 ==> result != None)]
#[ensures((*p).leaves.e@.len() > 0 ==> result != None)]
#[ensures((*p).leaves.e@.len() == (*p).sessions.e@.len())]
pub fn verify_epsl_evict_nonempty_domain_always_yields_a_victim(p: &mut Pool) -> Option<u64> {
    proof_assert! { p.len@ > 0 ==> p.by_key.e@.len() > 0 };
    proof_assert! { p.len@ > 0 ==>
        ((p.by_key.e@[0]).1)@ < p.nodes@.len() && (p.nodes@[((p.by_key.e@[0]).1)@]).active };
    proof_assert! { lemma_chain_reaches_a_leaf(&*p, ((p.by_key.e@[0]).1)@);
        p.len@ > 0 ==> exists<j: Int> 0 <= j && j < p.nodes@.len() && (p.nodes@[j]).active
                           && (p.nodes@[j]).child == None };
    proof_assert! { p.len@ > 0 ==> p.leaves.e@.len() > 0 };
    pool_evict_oldest(p)
}

#[requires(pool_inv(p))]
#[ensures(!((*p).len@ > 0 ==> result != None))] // FLIPPED: must fail
#[ensures((*p).leaves.e@.len() > 0 ==> result != None)]
#[ensures((*p).leaves.e@.len() == (*p).sessions.e@.len())]
pub fn verify_epsl_evict_nonempty_domain_always_yields_a_victim__mutant(p: &mut Pool) -> Option<u64> {
    proof_assert! { p.len@ > 0 ==> p.by_key.e@.len() > 0 };
    proof_assert! { p.len@ > 0 ==>
        ((p.by_key.e@[0]).1)@ < p.nodes@.len() && (p.nodes@[((p.by_key.e@[0]).1)@]).active };
    proof_assert! { lemma_chain_reaches_a_leaf(&*p, ((p.by_key.e@[0]).1)@);
        p.len@ > 0 ==> exists<j: Int> 0 <= j && j < p.nodes@.len() && (p.nodes@[j]).active
                           && (p.nodes@[j]).child == None };
    proof_assert! { p.len@ > 0 ==> p.leaves.e@.len() > 0 };
    pool_evict_oldest(p)
}

// ###########################################################################
// REFUTATIONS — each states the NEGATION of an obligation the inventory marks `origin: divergent`
// and PROVES it. A proved refutation is a machine-checked defect report, not a failed proof.
// ###########################################################################

/// `EPSL-TRACK-SESSION-COMES-FROM-CALLER` is FALSE of the code.
/// Registering a key the domain already tracks takes the idempotent path at session_list.rs:91-94:
/// it refreshes the existing block and returns before the `session` argument is ever read. So
/// re-registering a key under a DIFFERENT session silently keeps the block in its original session
/// and tells the caller nothing.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551000)]
#[requires(p.nodes@.len() < 4294967280)]
#[requires(!map_mem(p.by_key.e@, key))]
#[requires(s1 != s2)]
#[ensures(result.0 == result.1)]
#[ensures(((^p).nodes@[result.1@]).session == s1)]
#[ensures(((^p).nodes@[result.1@]).session != s2)]
pub fn refute_epsl_track_session_comes_from_caller(
    p: &mut Pool, key: u64, s1: u64, s2: u64,
) -> (u32, u32) {
    let a = pool_register(p, key, s1);
    let b = pool_register(p, key, s2);
    (a, b)
}

/// `EPSL-TOUCH-INVALID-HANDLE-IS-AN-ERROR` is FALSE of the code.
/// `is_active` (session_list.rs:80-82) checks occupancy only, and `unlink` pushes the freed slot
/// onto the spare list (session_list.rs:182) which `alloc` pops straight back
/// (session_list.rs:69). A handle kept across the removal of its block therefore passes validation
/// again as soon as the slot is reused, and the refresh SUCCEEDS — against a different block.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551000)]
#[requires(p.nodes@.len() < 4294967280)]
#[requires(index@ < p.nodes@.len())]
#[requires((p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[requires(newkey != (p.nodes@[index@]).key)]
#[ensures(result.0 == index)]
#[ensures(result.1)]
#[ensures(((^p).nodes@[index@]).key == newkey)]
#[ensures(((^p).nodes@[index@]).key != ((*p).nodes@[index@]).key)]
#[ensures(((^p).nodes@[index@]).active)]
pub fn refute_epsl_touch_invalid_handle_is_an_error(
    p: &mut Pool, index: u32, newkey: u64, sess: u64,
) -> (u32, bool) {
    let _ = pool_remove(p, index);
    let a = pool_register(p, newkey, sess);
    let b = pool_touch(p, index);
    (a, b)
}

/// `EPSL-BATCH-TOUCH-INVALID-HANDLE-IS-AN-ERROR` is FALSE of the code — same root cause: the group
/// walk validates each handle with the very same occupancy-only check (src/lib.rs:179), so a stale
/// handle whose slot has been reused passes and silently refreshes the block that now occupies it.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551000)]
#[requires(p.nodes@.len() < 4294967280)]
#[requires(index@ < p.nodes@.len())]
#[requires((p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[requires(newkey != (p.nodes@[index@]).key)]
#[ensures(result == Ok(()))]
#[ensures(((^p).nodes@[index@]).key == newkey)]
#[ensures(((^p).nodes@[index@]).key != ((*p).nodes@[index@]).key)]
#[ensures(((^p).nodes@[index@]).stamp@ > (*p).clock@)]
pub fn refute_epsl_batch_touch_invalid_handle_is_an_error(
    p: &mut Pool, index: u32, newkey: u64, sess: u64,
) -> Result<(), PolicyError> {
    let _ = pool_remove(p, index);
    let _ = pool_register(p, newkey, sess);
    // `batch_touch`'s loop body (src/lib.rs:169-182) inlined for a ONE-handle group, which is
    // exactly what `batch_touch(&[h])` executes. Going through `pool_batch_touch` instead would make
    // this refutation rest on that function's still-open loop invariant.
    if !pool_touch(p, index) {
        return Err(PolicyError::InvalidHandle);
    }
    Ok(())
}

/// `EPSL-REMOVE-INVALID-HANDLE-IS-AN-ERROR` is FALSE of the code, and this is the worst of the
/// three: a stale handle does not merely refresh the wrong block, it REMOVES it. `remove` uses the
/// same occupancy-only check (session_list.rs:207), so once the slot has been reused the call
/// succeeds and stops tracking a completely unrelated block.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551000)]
#[requires(p.nodes@.len() < 4294967280)]
#[requires(index@ < p.nodes@.len())]
#[requires((p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[requires(newkey != (p.nodes@[index@]).key)]
#[ensures(result.0 == index)]
#[ensures(result.1)]
#[ensures(!map_mem((^p).by_key.e@, newkey))]
pub fn refute_epsl_remove_invalid_handle_is_an_error(
    p: &mut Pool, index: u32, newkey: u64, sess: u64,
) -> (u32, bool) {
    let _ = pool_remove(p, index);
    let a = pool_register(p, newkey, sess);
    let b = pool_remove(p, index);
    (a, b)
}

/// `EPSL-INV-FAILED-OPERATIONS-CHANGE-NOTHING` is FALSE of the code.
/// The group refresh refreshes each handle in turn and returns the error as soon as it reaches a bad
/// one (src/lib.rs:169-182), with no undo. Every handle before the failing one keeps its new
/// recency, so an operation the caller was told had FAILED has permanently changed the eviction
/// order.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551000)]
#[requires(good@ < p.nodes@.len())]
#[requires((p.nodes@[good@]).active)]
#[requires(bad@ >= p.nodes@.len())]
#[ensures(result != Ok(()))]
#[ensures(((^p).nodes@[good@]).stamp@ > (*p).clock@)]
#[ensures(((^p).nodes@[good@]).stamp != ((*p).nodes@[good@]).stamp)]
#[ensures((^p).clock@ > (*p).clock@)]
pub fn refute_epsl_inv_failed_operations_change_nothing(
    p: &mut Pool, good: u32, bad: u32,
) -> Result<(), PolicyError> {
    // `batch_touch`'s loop body (src/lib.rs:169-182) inlined for a two-handle group: refresh, then
    // refresh, returning Err the moment a handle fails validation — with no undo of the first.
    if !pool_touch(p, good) {
        return Err(PolicyError::InvalidHandle);
    }
    if !pool_touch(p, bad) {
        return Err(PolicyError::InvalidHandle);
    }
    Ok(())
}

/// `EPSL-CANDIDATES-LISTED-IN-EVICTION-ORDER` is FALSE of the code.
/// `candidates` returns the CURRENTLY eligible blocks ordered by recency (session_list.rs:196-202),
/// which pins down only the FIRST entry. Evicting that block promotes its parent into the eligible
/// set at the parent's own, possibly much older, recency — so the block actually evicted second can
/// be one that was not in the list at all. Concretely: session `sa` holds the chain `k1 -> k2`
/// (stamps 1, 2) and session `sb` holds the singleton `k3` (stamp 3), so the list is `[k2, k3]`;
/// but the second eviction is `k1`, which the list never mentioned.
#[requires(pool_empty(p) && pool_inv(p))]
#[requires(k1 != k2 && k1 != k3 && k2 != k3)]
#[requires(sa != sb)]
#[requires(n@ >= 2)]
#[ensures(result.0@.len() == 2)]
#[ensures(result.0@[0] == k2 && result.0@[1] == k3)]
#[ensures(result.1 == Some(k2))]
#[ensures(result.2 == Some(k1))]
#[ensures(result.2 != Some(result.0@[1]))]
pub fn refute_epsl_candidates_listed_in_eviction_order(
    p: &mut Pool, k1: u64, k2: u64, k3: u64, sa: u64, sb: u64, n: usize,
) -> (Vec<u64>, Option<u64>, Option<u64>) {
    let a = pool_register(p, k1, sa);
    let b = pool_register(p, k2, sa);
    let c = pool_register(p, k3, sb);
    let listed = pool_candidates(p, n);
    let first = pool_evict_oldest(p);
    // After the first eviction the candidate set is `[(stamp(a), a), (stamp(c), c)]` again — the
    // PARENT `a` has been promoted at its own, older, stamp. Spelled out step by step because the
    // last step is a pigeonhole ("two distinct members of a sorted length-2 sequence ARE that
    // sequence, in order") that the solver will not find from the invariants on its own.
    proof_assert! { p.leaves.e@.len() == 2 };
    proof_assert! { a != b && a != c && b != c };
    proof_assert! { (p.nodes@[a@]).active && (p.nodes@[a@]).child == None };
    proof_assert! { (p.nodes@[c@]).active && (p.nodes@[c@]).child == None };
    proof_assert! { leaves_mem(p.leaves.e@, ((p.nodes@[a@]).stamp, a)) };
    proof_assert! { leaves_mem(p.leaves.e@, ((p.nodes@[c@]).stamp, c)) };
    proof_assert! { pair_lt(((p.nodes@[a@]).stamp, a), ((p.nodes@[c@]).stamp, c)) };
    proof_assert! { p.leaves.e@[0] == ((p.nodes@[a@]).stamp, a)
                    || p.leaves.e@[1] == ((p.nodes@[a@]).stamp, a) };
    proof_assert! { p.leaves.e@[0] == ((p.nodes@[a@]).stamp, a) };
    proof_assert! { (p.nodes@[a@]).key == k1 };
    let second = pool_evict_oldest(p);
    let _ = b;
    (listed, first, second)
}

/// `EPSL-CLEAR-INVALIDATES-EXISTING-HANDLES` is FALSE of the code.
/// `clear` empties the arena AND the spare list (session_list.rs:220-228), so slot numbering starts
/// from zero again. A handle held across the clear is rejected only while the domain is still
/// smaller than that handle's position; register one block and slot 0 is live again, so a pre-clear
/// handle to slot 0 refreshes a live but completely unrelated block and reports success.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551000)]
#[requires(p.nodes@.len() < 4294967280)]
#[ensures(result.0@ == 0)]
#[ensures(result.1)]
#[ensures(((^p).nodes@[0]).key == newkey)]
#[ensures(((^p).nodes@[0]).active)]
pub fn refute_epsl_clear_invalidates_existing_handles(
    p: &mut Pool, newkey: u64, sess: u64,
) -> (u32, bool) {
    pool_clear(p);
    let a = pool_register(p, newkey, sess);
    let b = pool_touch(p, 0);
    (a, b)
}

/// `EPSL-INV-RECENCY-STRICTLY-ADVANCES` is FALSE of the code.
/// `clear` resets the recency counter to zero (session_list.rs:226), so a value handed out after a
/// clear repeats one handed out before it. The first block registered into a fresh domain gets stamp
/// 1; clear the domain and the next registration gets stamp 1 again, which is NOT strictly later.
#[requires(pool_empty(p) && pool_inv(p))]
#[requires(k1 != k2)]
#[ensures(result.0@ == 1)]
#[ensures(result.1@ == 1)]
#[ensures(result.1@ <= result.0@)]
#[ensures((^p).clock@ == 1)]
pub fn refute_epsl_inv_recency_strictly_advances(
    p: &mut Pool, k1: u64, k2: u64, sess: u64,
) -> (u64, u64) {
    let a = pool_register(p, k1, sess);
    let first = p.nodes[a as usize].stamp;
    pool_clear(p);
    let b = pool_register(p, k2, sess);
    let second = p.nodes[b as usize].stamp;
    (first, second)
}

/// `EPSL-INV-HANDLES-KEEP-NAMING-THEIR-BLOCK` is FALSE of the code — the root cause of the three
/// per-method invalid-handle divergences and of the clear-domain one. A handle is nothing but the
/// domain id plus the slot number (src/lib.rs:140), with no marker of WHICH use of that slot it
/// belongs to. A slot freed by a removal is handed to the very next registration, so the same handle
/// comes to name a different block, and every operation taking it acts on that unrelated block
/// while reporting success.
#[requires(pool_inv(p))]
#[requires(p.clock@ < 18446744073709551000)]
#[requires(p.nodes@.len() < 4294967280)]
#[requires(index@ < p.nodes@.len())]
#[requires((p.nodes@[index@]).active)]
#[requires((p.nodes@[index@]).child == None)]
#[requires(!map_mem(p.by_key.e@, newkey))]
#[requires(newkey != (p.nodes@[index@]).key)]
#[ensures(result == index)]
#[ensures(((^p).nodes@[index@]).key == newkey)]
#[ensures(((^p).nodes@[index@]).key != ((*p).nodes@[index@]).key)]
#[ensures(((^p).nodes@[index@]).active)]
pub fn refute_epsl_inv_handles_keep_naming_their_block(
    p: &mut Pool, index: u32, newkey: u64, sess: u64,
) -> u32 {
    let _ = pool_remove(p, index);
    pool_register(p, newkey, sess)
}

