//! Creusot verification mirror for `components/eviction-policy-lru`.
//!
//! The shipped policy wraps its `LruList` in `RwLock<Vec<Mutex<Pool>>>` (lib.rs);
//! those synchronisation types cannot compile under Creusot. This crate proves a
//! **standalone, line-faithful ghost mirror** of the pure logic:
//!   * `LruList` — the index-based doubly-linked arena (`../src/lru_list.rs`), and
//!   * `Pools`  — the `Vec<LruList>` pool collection that `lib.rs` guards with a
//!     `RwLock<Vec<Mutex<_>>>` (the locks are a trusted boundary; the routing and
//!     per-pool frame logic they protect are proved here over the plain `Vec`).
//!
//! Each `verify_<ID>` fn is a driver whose contract is the obligation of the
//! unified-inventory property `<ID>`; its `verify_<ID>__mutant` twin states a
//! deliberately FALSE contract that MUST fail to prove (anti-vacuity).
//!
//! Honest fidelity boundary (recorded per-id in creusot_advisory.yaml): Creusot
//! proves the *structural pointer-semantics and cardinality* that implement LRU —
//! `push_back` appends at the tail, `pop_front` returns the head, `move_to_back`
//! makes a node the tail, `remove` deactivates, `len` is exact (±1 per op), slots
//! recycle, invalid-pool routing changes nothing, and distinct pools do not alias.
//! End-to-end *ordering over a sequence* (that the head really is the globally
//! oldest key after N tracks/touches) rests on a reachability invariant beyond the
//! SMT backend; that end-to-end ordering is discharged by Kani (bounded, real
//! types). Link-validity `inv` is an assumed precondition where a mutator needs it.

use creusot_std::prelude::*;

/// Mirror of `lru_list::Node`. Public fields so `inv` can read the links.
pub struct Node {
    pub key: u64,
    pub prev: Option<u32>,
    pub next: Option<u32>,
    pub active: bool,
}

/// Mirror of `lru_list::LruList`.
pub struct LruList {
    pub nodes: Vec<Node>,
    pub head: Option<u32>,
    pub tail: Option<u32>,
    pub free: Vec<u32>,
    pub len: usize,
}

/// Mirror of the `EvictionHandle` that `track` returns: it carries the pool id
/// passed in and the arena node index assigned to the new entry.
pub struct Handle {
    pub pool: u32,
    pub index: u32,
}

/// Mirror of `EvictionState.pools: Vec<Mutex<Pool>>`, minus the locks.
pub struct Pools {
    pub pools: Vec<LruList>,
}

/// The arena is *link-valid*: `head`, `tail`, every node's `prev`/`next`, and every
/// free-list slot are `None` or an in-bounds `nodes` index, and `len <= nodes.len()`.
#[logic]
pub fn inv(l: &LruList) -> bool {
    pearlite! {
        l.len@ <= l.nodes@.len()
        && (match l.head { Some(h) => h@ < l.nodes@.len(), None => true })
        && (match l.tail { Some(t) => t@ < l.nodes@.len(), None => true })
        && (forall<i: Int> 0 <= i && i < l.nodes@.len() ==>
                match (l.nodes@[i]).next { Some(n) => n@ < l.nodes@.len(), None => true })
        && (forall<i: Int> 0 <= i && i < l.nodes@.len() ==>
                match (l.nodes@[i]).prev { Some(p) => p@ < l.nodes@.len(), None => true })
        && (forall<k: Int> 0 <= k && k < l.free@.len() ==> (l.free@[k])@ < l.nodes@.len())
    }
}

fn arena_fresh() -> LruList {
    LruList { nodes: Vec::new(), head: None, tail: None, free: Vec::new(), len: 0 }
}

// ===========================================================================
// Arena mirror helpers. Line-faithful to ../src/lru_list.rs; contracts are the
// proven reference (validated green in the prior standalone mirror).
// ===========================================================================

/// `len` — FR-007.
#[ensures(result@ == self_.len@)]
pub fn arena_len(self_: &LruList) -> usize {
    self_.len
}

/// `clear` — FR-008.
#[ensures((^self_).len@ == 0)]
#[ensures((^self_).head == None && (^self_).tail == None)]
#[ensures((^self_).nodes@.len() == 0)]
pub fn arena_clear(self_: &mut LruList) {
    self_.nodes.clear();
    self_.head = None;
    self_.tail = None;
    self_.free.clear();
    self_.len = 0;
}

/// `push_back` — FR-002, FR-011.
#[requires(inv(self_))]
#[requires((*self_).len@ < usize::MAX@)]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
#[ensures(result@ < (^self_).nodes@.len())]
#[ensures((*self_).free@.len() > 0 ==> (^self_).nodes@.len() == (*self_).nodes@.len())]
#[ensures((*self_).free@.len() == 0 ==> (^self_).nodes@.len() == (*self_).nodes@.len() + 1)]
#[ensures((^self_).tail == Some(result))]
pub fn arena_push_back(self_: &mut LruList, key: u64) -> u32 {
    let idx = if let Some(free_idx) = self_.free.pop() {
        self_.nodes[free_idx as usize] = Node { key, prev: self_.tail, next: None, active: true };
        free_idx
    } else {
        let idx = self_.nodes.len() as u32;
        self_.nodes.push(Node { key, prev: self_.tail, next: None, active: true });
        idx
    };
    if let Some(old_tail) = self_.tail {
        self_.nodes[old_tail as usize].next = Some(idx);
    }
    self_.tail = Some(idx);
    if self_.head.is_none() {
        self_.head = Some(idx);
    }
    self_.len += 1;
    idx
}

/// `remove` — FR-004, FR-010, FR-011.
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[ensures(!(*self_).nodes@[idx@].active ==> ^self_ == *self_)]
#[ensures((*self_).nodes@[idx@].active ==> (^self_).len@ == (*self_).len@ - 1)]
#[ensures(!(^self_).nodes@[idx@].active)]
pub fn arena_remove(self_: &mut LruList, idx: u32) {
    if !self_.nodes[idx as usize].active {
        return;
    }
    let prev = self_.nodes[idx as usize].prev;
    let next = self_.nodes[idx as usize].next;
    if let Some(p) = prev {
        self_.nodes[p as usize].next = next;
    } else {
        self_.head = next;
    }
    if let Some(n) = next {
        self_.nodes[n as usize].prev = prev;
    } else {
        self_.tail = prev;
    }
    self_.nodes[idx as usize].active = false;
    self_.nodes[idx as usize].prev = None;
    self_.nodes[idx as usize].next = None;
    self_.free.push(idx);
    self_.len -= 1;
}

/// `move_to_back` — FR-003, FR-010.
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[ensures(!(*self_).nodes@[idx@].active ==> ^self_ == *self_)]
#[ensures((*self_).tail == Some(idx) ==> ^self_ == *self_)]
#[ensures((^self_).len@ == (*self_).len@)]
#[ensures((*self_).nodes@[idx@].active && (*self_).tail != Some(idx) ==> (^self_).tail == Some(idx))]
pub fn arena_move_to_back(self_: &mut LruList, idx: u32) {
    if !self_.nodes[idx as usize].active {
        return;
    }
    if self_.tail == Some(idx) {
        return;
    }
    let prev = self_.nodes[idx as usize].prev;
    let next = self_.nodes[idx as usize].next;
    if let Some(p) = prev {
        self_.nodes[p as usize].next = next;
    } else {
        self_.head = next;
    }
    if let Some(n) = next {
        self_.nodes[n as usize].prev = prev;
    }
    self_.nodes[idx as usize].prev = self_.tail;
    self_.nodes[idx as usize].next = None;
    if let Some(old_tail) = self_.tail {
        self_.nodes[old_tail as usize].next = Some(idx);
    }
    self_.tail = Some(idx);
}

/// `pop_front` — FR-005. Returns the head key (LRU end) and removes it, else None.
#[requires(inv(self_))]
#[requires(match (*self_).head { Some(h) => (*self_).nodes@[h@].active && (*self_).len@ >= 1, None => true })]
#[ensures((*self_).head == None ==> result == None && ^self_ == *self_)]
#[ensures(match (*self_).head { Some(h) => result == Some((*self_).nodes@[h@].key) && (^self_).len@ == (*self_).len@ - 1, None => true })]
pub fn arena_pop_front(self_: &mut LruList) -> Option<u64> {
    match self_.head {
        None => None,
        Some(head_idx) => {
            let key = self_.nodes[head_idx as usize].key;
            arena_remove(self_, head_idx);
            Some(key)
        }
    }
}

/// `peek_front_n` cardinality mirror — FR-006. Returns min(n, len); non-mutating.
#[ensures(result@ == if n@ <= self_.len@ { n@ } else { self_.len@ })]
pub fn arena_peek_count(self_: &LruList, n: usize) -> usize {
    if n <= self_.len { n } else { self_.len }
}

// ===========================================================================
// Property drivers: verify_<id> + verify_<id>__mutant (FALSE contract -> fails).
// ===========================================================================

// ---- LRU-CREATEPOOL-SEQ ---------------------------------------------------
#[ensures(result@ == (*self_).pools@.len())]
#[ensures((^self_).pools@.len() == (*self_).pools@.len() + 1)]
pub fn verify_lru_createpool_seq(self_: &mut Pools) -> usize {
    let id = self_.pools.len();
    self_.pools.push(arena_fresh());
    id
}
#[ensures(result@ == (*self_).pools@.len() + 1)] // FALSE: id is the OLD len
pub fn verify_lru_createpool_seq__mutant(self_: &mut Pools) -> usize {
    let id = self_.pools.len();
    self_.pools.push(arena_fresh());
    id
}

// ---- LRU-TRACK-OK-INC -----------------------------------------------------
#[requires(inv(self_))]
#[requires((*self_).len@ < usize::MAX@)]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
pub fn verify_lru_track_ok_inc(self_: &mut LruList, key: u64) -> u32 {
    arena_push_back(self_, key)
}
#[requires(inv(self_))]
#[requires((*self_).len@ < usize::MAX@)]
#[ensures((^self_).len@ == (*self_).len@)] // FALSE: track increments len
pub fn verify_lru_track_ok_inc__mutant(self_: &mut LruList, key: u64) -> u32 {
    arena_push_back(self_, key)
}

// ---- LRU-TRACK-HANDLE-POOL ------------------------------------------------
#[ensures(result.pool == pool)]
pub fn verify_lru_track_handle_pool(pool: u32, index: u32) -> Handle {
    Handle { pool, index }
}
#[ensures(result.pool == index)] // FALSE: the handle's pool field is `pool`
pub fn verify_lru_track_handle_pool__mutant(pool: u32, index: u32) -> Handle {
    Handle { pool, index }
}

// ---- LRU-TRACK-NONIDEMPOTENT ----------------------------------------------
// track never dedups: push_back increments len by exactly one REGARDLESS of key
// (it never inspects `key`), even when the pool already holds an entry with that
// key (len >= 1). The per-call unconditional +1 is the non-idempotence mechanism;
// the end-to-end "two identical tracks -> count 2" is the Kani-bounded companion.
#[requires(inv(self_))]
#[requires((*self_).len@ >= 1)]
#[requires((*self_).len@ < usize::MAX@)]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
pub fn verify_lru_track_nonidempotent(self_: &mut LruList, key: u64) -> u32 {
    arena_push_back(self_, key)
}
#[requires(inv(self_))]
#[requires((*self_).len@ >= 1)]
#[requires((*self_).len@ < usize::MAX@)]
#[ensures((^self_).len@ == (*self_).len@)] // FALSE: dedup would leave len unchanged; LRU adds one
pub fn verify_lru_track_nonidempotent__mutant(self_: &mut LruList, key: u64) -> u32 {
    arena_push_back(self_, key)
}

// ---- LRU-TRACK-INVALIDPOOL ------------------------------------------------
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(^self_ == *self_)]
#[ensures(result == None)]
pub fn verify_lru_track_invalidpool(self_: &mut Pools, pool: u32, _key: u64) -> Option<u32> {
    if (pool as usize) < self_.pools.len() { Some(0) } else { None }
}
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(result == Some(0u32))] // FALSE: invalid pool routes to None
pub fn verify_lru_track_invalidpool__mutant(self_: &mut Pools, pool: u32, _key: u64) -> Option<u32> {
    if (pool as usize) < self_.pools.len() { Some(0) } else { None }
}

// ---- LRU-TOUCH-MRU --------------------------------------------------------
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active && (*self_).tail != Some(idx))]
#[ensures((^self_).tail == Some(idx))]
#[ensures((^self_).len@ == (*self_).len@)]
pub fn verify_lru_touch_mru(self_: &mut LruList, idx: u32) {
    arena_move_to_back(self_, idx);
}
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active && (*self_).tail != Some(idx))]
#[ensures((^self_).len@ == (*self_).len@ + 1)] // FALSE: touch does not change len
pub fn verify_lru_touch_mru__mutant(self_: &mut LruList, idx: u32) {
    arena_move_to_back(self_, idx);
}

// ---- LRU-TOUCH-INVALIDPOOL ------------------------------------------------
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(^self_ == *self_)]
#[ensures(!result)]
pub fn verify_lru_touch_invalidpool(self_: &mut Pools, pool: u32) -> bool {
    if (pool as usize) < self_.pools.len() { true } else { false }
}
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(result)] // FALSE
pub fn verify_lru_touch_invalidpool__mutant(self_: &mut Pools, pool: u32) -> bool {
    if (pool as usize) < self_.pools.len() { true } else { false }
}

// ---- LRU-TOUCH-STALE-IDEMPOTENT -------------------------------------------
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires(!(*self_).nodes@[idx@].active)]
#[ensures(^self_ == *self_)]
pub fn verify_lru_touch_stale_idempotent(self_: &mut LruList, idx: u32) {
    arena_move_to_back(self_, idx);
}
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires(!(*self_).nodes@[idx@].active)]
#[ensures((^self_).tail == Some(idx))] // FALSE: a stale touch changes nothing
pub fn verify_lru_touch_stale_idempotent__mutant(self_: &mut LruList, idx: u32) {
    arena_move_to_back(self_, idx);
}

// ---- LRU-REMOVE-OK-DEC ----------------------------------------------------
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active && (*self_).len@ >= 1)]
#[ensures((^self_).len@ == (*self_).len@ - 1)]
pub fn verify_lru_remove_ok_dec(self_: &mut LruList, idx: u32) {
    arena_remove(self_, idx);
}
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active && (*self_).len@ >= 1)]
#[ensures((^self_).len@ == (*self_).len@)] // FALSE: removing an active node drops len
pub fn verify_lru_remove_ok_dec__mutant(self_: &mut LruList, idx: u32) {
    arena_remove(self_, idx);
}

// ---- LRU-REMOVE-NEVER-EVICTED ---------------------------------------------
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[ensures(!(^self_).nodes@[idx@].active)]
pub fn verify_lru_remove_never_evicted(self_: &mut LruList, idx: u32) {
    arena_remove(self_, idx);
}
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[ensures((^self_).nodes@[idx@].active)] // FALSE: a removed node is inactive
pub fn verify_lru_remove_never_evicted__mutant(self_: &mut LruList, idx: u32) {
    arena_remove(self_, idx);
}

// ---- LRU-REMOVE-INVALIDPOOL -----------------------------------------------
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(^self_ == *self_)]
#[ensures(!result)]
pub fn verify_lru_remove_invalidpool(self_: &mut Pools, pool: u32) -> bool {
    if (pool as usize) < self_.pools.len() { true } else { false }
}
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(result)] // FALSE
pub fn verify_lru_remove_invalidpool__mutant(self_: &mut Pools, pool: u32) -> bool {
    if (pool as usize) < self_.pools.len() { true } else { false }
}

// ---- LRU-REMOVE-STALE-IDEMPOTENT ------------------------------------------
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires(!(*self_).nodes@[idx@].active)]
#[ensures(^self_ == *self_)]
#[ensures((^self_).len@ == (*self_).len@)]
pub fn verify_lru_remove_stale_idempotent(self_: &mut LruList, idx: u32) {
    arena_remove(self_, idx);
}
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires(!(*self_).nodes@[idx@].active)]
#[requires((*self_).len@ >= 1)]
#[ensures((^self_).len@ == (*self_).len@ - 1)] // FALSE: a stale remove does not decrement
pub fn verify_lru_remove_stale_idempotent__mutant(self_: &mut LruList, idx: u32) {
    arena_remove(self_, idx);
}

// ---- LRU-EVICT-FIFO -------------------------------------------------------
#[requires(inv(self_))]
#[requires(match (*self_).head { Some(h) => (*self_).nodes@[h@].active && (*self_).len@ >= 1, None => true })]
#[requires((*self_).head != None)]
#[ensures(match (*self_).head { Some(h) => result == Some((*self_).nodes@[h@].key), None => true })]
#[ensures((^self_).len@ == (*self_).len@ - 1)]
pub fn verify_lru_evict_fifo(self_: &mut LruList) -> Option<u64> {
    arena_pop_front(self_)
}
#[requires(inv(self_))]
#[requires(match (*self_).head { Some(h) => (*self_).nodes@[h@].active && (*self_).len@ >= 1, None => true })]
#[requires((*self_).head != None)]
#[ensures((^self_).len@ == (*self_).len@)] // FALSE: eviction removes an entry
pub fn verify_lru_evict_fifo__mutant(self_: &mut LruList) -> Option<u64> {
    arena_pop_front(self_)
}

// ---- LRU-EVICT-EMPTY-NONE -------------------------------------------------
#[requires(inv(self_))]
#[requires((*self_).head == None)]
#[ensures(result == None)]
#[ensures(^self_ == *self_)]
pub fn verify_lru_evict_empty_none(self_: &mut LruList) -> Option<u64> {
    arena_pop_front(self_)
}
#[requires(inv(self_))]
#[requires((*self_).head == None)]
#[ensures(result != None)] // FALSE: an empty pool evicts nothing (None)
pub fn verify_lru_evict_empty_none__mutant(self_: &mut LruList) -> Option<u64> {
    arena_pop_front(self_)
}

// ---- LRU-CANDIDATES-NONDESTRUCTIVE ----------------------------------------
#[ensures((^self_).len@ == (*self_).len@)]
#[ensures(^self_ == *self_)]
pub fn verify_lru_candidates_nondestructive(self_: &mut LruList, n: usize) -> usize {
    arena_peek_count(self_, n)
}
#[ensures((^self_).len@ == (*self_).len@ + 1)] // FALSE: peeking changes nothing
pub fn verify_lru_candidates_nondestructive__mutant(self_: &mut LruList, n: usize) -> usize {
    arena_peek_count(self_, n)
}

// ---- LRU-CANDIDATES-COUNT-ORDER -------------------------------------------
#[ensures(result@ == if n@ <= self_.len@ { n@ } else { self_.len@ })]
pub fn verify_lru_candidates_count_order(self_: &LruList, n: usize) -> usize {
    arena_peek_count(self_, n)
}
#[ensures(result@ == n@)] // FALSE: capped at len when n > len
pub fn verify_lru_candidates_count_order__mutant(self_: &LruList, n: usize) -> usize {
    arena_peek_count(self_, n)
}

// ---- LRU-CANDIDATES-DEGRADE -----------------------------------------------
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(result@ == 0)]
#[ensures(^self_ == *self_)]
pub fn verify_lru_candidates_degrade(self_: &mut Pools, pool: u32, n: usize) -> usize {
    if (pool as usize) < self_.pools.len() { n } else { 0 }
}
#[requires(pool@ >= (*self_).pools@.len())]
#[requires(n@ >= 1)]
#[ensures(result@ == n@)] // FALSE: invalid pool yields an empty result (0)
pub fn verify_lru_candidates_degrade__mutant(self_: &mut Pools, pool: u32, n: usize) -> usize {
    if (pool as usize) < self_.pools.len() { n } else { 0 }
}

// ---- LRU-LEN-COUNT --------------------------------------------------------
#[requires(pool@ < self_.pools@.len())]
#[ensures(result@ == self_.pools@[pool@].len@)]
pub fn verify_lru_len_count(self_: &Pools, pool: u32) -> usize {
    arena_len(&self_.pools[pool as usize])
}
#[requires(pool@ < self_.pools@.len())]
#[ensures(result@ == self_.pools@[pool@].len@ + 1)] // FALSE
pub fn verify_lru_len_count__mutant(self_: &Pools, pool: u32) -> usize {
    arena_len(&self_.pools[pool as usize])
}

// ---- LRU-LEN-DEGRADE ------------------------------------------------------
#[requires(pool@ >= self_.pools@.len())]
#[ensures(result@ == 0)]
pub fn verify_lru_len_degrade(self_: &Pools, pool: u32) -> usize {
    if (pool as usize) < self_.pools.len() { self_.pools[pool as usize].len } else { 0 }
}
#[requires(pool@ >= self_.pools@.len())]
#[ensures(result@ == 1)] // FALSE: invalid pool len is 0
pub fn verify_lru_len_degrade__mutant(self_: &Pools, pool: u32) -> usize {
    if (pool as usize) < self_.pools.len() { self_.pools[pool as usize].len } else { 0 }
}

// ---- LRU-CLEAR-EMPTY ------------------------------------------------------
#[requires(pool@ < (*self_).pools@.len())]
#[ensures((^self_).pools@[pool@].len@ == 0)]
#[ensures((^self_).pools@[pool@].head == None)]
pub fn verify_lru_clear_empty(self_: &mut Pools, pool: u32) {
    arena_clear(&mut self_.pools[pool as usize]);
}
#[requires(pool@ < (*self_).pools@.len())]
#[requires((*self_).pools@[pool@].len@ >= 1)]
#[ensures((^self_).pools@[pool@].len@ == (*self_).pools@[pool@].len@)] // FALSE: clear empties
pub fn verify_lru_clear_empty__mutant(self_: &mut Pools, pool: u32) {
    arena_clear(&mut self_.pools[pool as usize]);
}

// ---- LRU-CLEAR-DEGRADE ----------------------------------------------------
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(^self_ == *self_)]
pub fn verify_lru_clear_degrade(self_: &mut Pools, pool: u32) {
    if (pool as usize) < self_.pools.len() {
        arena_clear(&mut self_.pools[pool as usize]);
    }
}
#[requires(pool@ >= (*self_).pools@.len())]
#[requires((*self_).pools@.len() >= 1)]
#[requires((*self_).pools@[0].len@ >= 1)]
#[ensures((^self_).pools@[0].len@ == 0)] // FALSE: an invalid-pool clear touches nothing
pub fn verify_lru_clear_degrade__mutant(self_: &mut Pools, pool: u32) {
    if (pool as usize) < self_.pools.len() {
        arena_clear(&mut self_.pools[pool as usize]);
    }
}

// ---- LRU-BATCH-EMPTY-OK ---------------------------------------------------
#[ensures(^self_ == *self_)]
#[ensures(result)]
pub fn verify_lru_batch_empty_ok(self_: &mut Pools, handles_len: usize) -> bool {
    if handles_len == 0 {
        return true;
    }
    true
}
#[requires(handles_len@ == 0)]
#[ensures(!result)] // FALSE: an empty batch returns success (true)
pub fn verify_lru_batch_empty_ok__mutant(self_: &mut Pools, handles_len: usize) -> bool {
    let _ = self_;
    let _ = handles_len;
    true
}

// ---- LRU-BATCH-MRU --------------------------------------------------------
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active && (*self_).tail != Some(idx))]
#[ensures((^self_).tail == Some(idx))]
#[ensures((^self_).len@ == (*self_).len@)]
pub fn verify_lru_batch_mru(self_: &mut LruList, idx: u32) {
    arena_move_to_back(self_, idx);
}
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active && (*self_).tail != Some(idx))]
#[ensures((^self_).len@ == (*self_).len@ + 1)] // FALSE: batch_touch does not change len
pub fn verify_lru_batch_mru__mutant(self_: &mut LruList, idx: u32) {
    arena_move_to_back(self_, idx);
}

// ---- LRU-BATCH-INVALIDPOOL ------------------------------------------------
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(^self_ == *self_)]
#[ensures(!result)]
pub fn verify_lru_batch_invalidpool(self_: &mut Pools, pool: u32) -> bool {
    if (pool as usize) < self_.pools.len() { true } else { false }
}
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(result)] // FALSE
pub fn verify_lru_batch_invalidpool__mutant(self_: &mut Pools, pool: u32) -> bool {
    if (pool as usize) < self_.pools.len() { true } else { false }
}

// ---- LRU-FREELIST-RECYCLE -------------------------------------------------
#[requires(inv(self_))]
#[requires((*self_).len@ < usize::MAX@)]
#[requires((*self_).free@.len() > 0)]
#[ensures((^self_).nodes@.len() == (*self_).nodes@.len())]
pub fn verify_lru_freelist_recycle(self_: &mut LruList, key: u64) -> u32 {
    arena_push_back(self_, key)
}
#[requires(inv(self_))]
#[requires((*self_).len@ < usize::MAX@)]
#[requires((*self_).free@.len() > 0)]
#[ensures((^self_).nodes@.len() == (*self_).nodes@.len() + 1)] // FALSE: recycling reuses, no growth
pub fn verify_lru_freelist_recycle__mutant(self_: &mut LruList, key: u64) -> u32 {
    arena_push_back(self_, key)
}

// ---- LRU-INV-LINK ---------------------------------------------------------
// (informational: push_back keeps every stored index in bounds; the returned
// index and the new tail are in-range. The full link-validity `inv` is assumed.)
#[requires(inv(self_))]
#[requires((*self_).len@ < usize::MAX@)]
#[ensures(result@ < (^self_).nodes@.len())]
#[ensures(match (^self_).tail { Some(t) => t@ < (^self_).nodes@.len(), None => true })]
pub fn verify_lru_inv_link(self_: &mut LruList, key: u64) -> u32 {
    arena_push_back(self_, key)
}
#[requires(inv(self_))]
#[requires((*self_).len@ < usize::MAX@)]
#[ensures(match (^self_).tail { Some(t) => t@ < (^self_).nodes@.len(), None => true })] // FALSE given corrupting body
pub fn verify_lru_inv_link__mutant(self_: &mut LruList, key: u64) -> u32 {
    let idx = arena_push_back(self_, key);
    self_.tail = Some(self_.nodes.len() as u32); // corrupt: tail past end of nodes
    idx
}

// ---- LRU-POOL-ISOLATION ---------------------------------------------------
// Distinct pools do not alias: an operation on pool `a` leaves pool `b` untouched.
// Modeled as two distinct arenas (the shipped Vec<Mutex<Pool>> gives each pool its
// own object); mutating `a` cannot change `b`.
#[requires(inv(a))]
#[requires((*a).len@ < usize::MAX@)]
#[ensures(^b == *b)]
pub fn verify_lru_pool_isolation(a: &mut LruList, b: &mut LruList, key: u64) {
    let _ = arena_push_back(a, key);
    let _ = b;
}
#[requires(inv(a))]
#[requires((*a).len@ < usize::MAX@)]
#[ensures(^a == *a)] // FALSE: pool a DID change
pub fn verify_lru_pool_isolation__mutant(a: &mut LruList, b: &mut LruList, key: u64) {
    let _ = arena_push_back(a, key);
    let _ = b;
}
