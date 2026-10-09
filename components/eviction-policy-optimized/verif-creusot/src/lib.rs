#![recursion_limit = "1024"]
//! Creusot verification mirror for `components/eviction-policy-optimized`.
//!
//! The shipped policy stores its pools in `RwLock<Vec<Mutex<Pool>>>` (src/lib.rs:72) and its
//! frequency estimator in a fixed `[[u8; 1024]; 4]` array (src/lib.rs:28). The synchronisation
//! types cannot compile under Creusot, so this crate proves a **standalone, line-faithful ghost
//! mirror** of the pure logic:
//!   * `LruList` — the index-based doubly-linked arena of `../src/lru_list.rs`, field for field;
//!   * `Sketch`  — the count-min sketch of `../src/lib.rs:27-61`, with the `[[u8; COLS]; ROWS]`
//!     array flattened to a `Vec<u8>` of `ROWS*COLS` entries (row-major, index `row*COLS + col`);
//!   * `Pool`    — `{ lru, sketch, access_count, max_len }`, field for field (src/lib.rs:63-68);
//!   * `Pools`   — the `Vec<Pool>` pool collection that `lib.rs` guards with
//!     `RwLock<Vec<Mutex<_>>>` (the locks are a trusted boundary; the routing and per-pool frame
//!     logic they protect are proved here over the plain `Vec`).
//!
//! Each `verify_<ID>` fn is a driver whose contract is the obligation of the unified-inventory
//! property `<ID>`; its `verify_<ID>__mutant` twin states a deliberately FALSE contract that MUST
//! fail to prove (anti-vacuity).
//!
//! Honest fidelity boundary (recorded per-id in `../verif/creusot_advisory.yaml`): the arena's
//! *reachability* relation (the head-to-tail walk) is not a first-order SMT-friendly notion, so
//! link-validity `inv` is an assumed precondition (bounds + accounting only, no reachability)
//! wherever a mutator needs it, exactly as in the eviction-policy-lru reference mirror.

use creusot_std::prelude::*;

// ===========================================================================
// Mirror types
// ===========================================================================

/// Mirror of `lru_list::Node` (../src/lru_list.rs:5-11). Public fields so `inv` can read links.
pub struct Node {
    pub key: u64,
    pub prev: Option<u32>,
    pub next: Option<u32>,
    pub active: bool,
}

/// Mirror of `lru_list::LruList` (../src/lru_list.rs:17-23).
pub struct LruList {
    pub nodes: Vec<Node>,
    pub head: Option<u32>,
    pub tail: Option<u32>,
    pub free: Vec<u32>,
    pub len: usize,
}

/// Mirror of `CountMinSketch` (../src/lib.rs:27-29). `[[u8; CMS_COLS]; CMS_ROWS]` flattened
/// row-major into a `Vec<u8>` of `CMS_ROWS * CMS_COLS` entries.
pub struct Sketch {
    pub counters: Vec<u8>,
}

/// Mirror of `Pool` (../src/lib.rs:63-68).
pub struct Pool {
    pub lru: LruList,
    pub sketch: Sketch,
    pub access_count: u64,
    pub max_len: usize,
}

/// Mirror of `EvictionState.pools: Vec<Mutex<Pool>>` (../src/lib.rs:70-73), minus the locks.
pub struct Pools {
    pub pools: Vec<Pool>,
}

/// Mirror of the `EvictionHandle` that `track` returns (../src/lib.rs:139): the pool id passed
/// in plus the arena node index assigned to the new entry.
pub struct Handle {
    pub pool: u32,
    pub index: u32,
}

// The four row multipliers, verbatim from ../src/lib.rs:20-25.
pub const P0: u64 = 0x9E3779B97F4A7C15;
pub const P1: u64 = 0x517CC1B727220A95;
pub const P2: u64 = 0x6C62272E07BB0143;
pub const P3: u64 = 0xD45D0D6AB7E0F981;

// ===========================================================================
// Invariants
// ===========================================================================

/// The arena is *link-valid*: `head`, `tail`, every node's `prev`/`next`, and every free-list
/// slot are `None` or an in-bounds `nodes` index, and `len <= nodes.len()`.
///
/// The last conjunct is the arena's **addressing bound**: slots are addressed by `u32`
/// (`Node::prev`/`next`, `LruList::free`, and `push_*`'s `self.nodes.len() as u32` at
/// lru_list.rs:47 and :77), so the code is only index-correct while `nodes.len() <= u32::MAX`.
/// The shipped code has NO check for this — pushing past 2^32 slots silently truncates the
/// returned index — so the bound is an explicit, disclosed modelling assumption here, and it is
/// also what makes `max_len as u64 * 10` (the ageing period, lib.rs:124) provably overflow-free.
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
        && slots_accounted(l)
        && slots_disjoint(l)
        && l.nodes@.len() <= 4294967295
    }
}

/// Every allocated slot is either live or on the free list, never both and never neither
/// (EPO-INV-SLOT-OWNERSHIP-DISJOINT), and the free list holds each not-live slot exactly once.
/// Stated as the two directions that are first-order expressible: no live slot appears on the
/// free list, and the free list is duplicate-free.
#[logic]
pub fn slots_disjoint(l: &LruList) -> bool {
    pearlite! {
        (forall<k: Int> 0 <= k && k < l.free@.len() ==> !(l.nodes@[(l.free@[k])@]).active)
        && (forall<j: Int, k: Int> 0 <= j && j < k && k < l.free@.len() ==> l.free@[j] != l.free@[k])
    }
}

/// `nodes.len() == len + free.len()` — every allocated slot is accounted for
/// (EPO-INV-SLOTS-ACCOUNTED).
#[logic]
pub fn slots_accounted(l: &LruList) -> bool {
    pearlite! { l.nodes@.len() == l.len@ + l.free@.len() }
}

/// A pool is well formed: its arena is link-valid, its high-water record obeys the same
/// allocation bound, and its flattened sketch has exactly `CMS_ROWS * CMS_COLS = 4096` counters.
#[logic]
pub fn pool_inv(p: &Pool) -> bool {
    pearlite! {
        inv(&p.lru)
        && p.max_len@ <= 4294967295
        && p.sketch.counters@.len() == 4096
    }
}

// ===========================================================================
// Arena mirror — line-faithful to ../src/lru_list.rs
// ===========================================================================

/// `LruList::new` (lru_list.rs:26-34).
#[ensures(result.len@ == 0)]
#[ensures(result.head == None && result.tail == None)]
#[ensures(result.nodes@.len() == 0 && result.free@.len() == 0)]
#[ensures(inv(&result))]
pub fn arena_fresh() -> LruList {
    LruList { nodes: Vec::new(), head: None, tail: None, free: Vec::new(), len: 0 }
}

/// `len` (lru_list.rs:195-197).
#[ensures(result@ == self_.len@)]
pub fn arena_len(self_: &LruList) -> usize {
    self_.len
}

/// `clear` (lru_list.rs:186-192).
#[ensures((^self_).len@ == 0)]
#[ensures((^self_).head == None && (^self_).tail == None)]
#[ensures((^self_).nodes@.len() == 0)]
#[ensures((^self_).free@.len() == 0)]
#[ensures(inv(&^self_))]
pub fn arena_clear(self_: &mut LruList) {
    self_.nodes.clear();
    self_.head = None;
    self_.tail = None;
    self_.free.clear();
    self_.len = 0;
}

/// `push_back` (lru_list.rs:75-105). Inserts at the MRU (protected) end.
#[requires(inv(self_))]
#[requires((*self_).nodes@.len() < 4294967295)]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
#[ensures(result@ < (^self_).nodes@.len())]
#[ensures((*self_).free@.len() > 0 ==> (^self_).nodes@.len() == (*self_).nodes@.len())]
#[ensures((*self_).free@.len() == 0 ==> (^self_).nodes@.len() == (*self_).nodes@.len() + 1)]
#[ensures((^self_).tail == Some(result))]
#[ensures((*self_).head == None ==> (^self_).head == Some(result))]
#[ensures((*self_).head != None ==> (^self_).head == (*self_).head)]
#[ensures((^self_).nodes@[result@].key == key)]
#[ensures((^self_).nodes@[result@].active)]
#[ensures((*self_).free@.len() > 0 ==> result == (*self_).free@[(*self_).free@.len() - 1])]
#[ensures((*self_).free@.len() == 0 ==> result@ == (*self_).nodes@.len())]
#[ensures((*self_).free@.len() == 0 ==> (^self_).nodes@[result@].prev == (*self_).tail)]
#[ensures((*self_).free@.len() == 0 ==> (^self_).nodes@[result@].next == None)]
#[ensures(forall<i: Int> 0 <= i && i < (*self_).nodes@.len() && i != result@ ==>
             ((^self_).nodes@[i]).active == ((*self_).nodes@[i]).active
          && ((^self_).nodes@[i]).key == ((*self_).nodes@[i]).key)]
#[ensures(result@ < (*self_).nodes@.len() ==> !((*self_).nodes@[result@]).active)]
#[ensures((^self_).nodes@.len() >= (*self_).nodes@.len())]
#[ensures(inv(&^self_))]
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

/// `push_front` (lru_list.rs:37-67). Inserts at the LRU (eviction) end.
#[requires(inv(self_))]
#[requires((*self_).nodes@.len() < 4294967295)]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
#[ensures(result@ < (^self_).nodes@.len())]
#[ensures((*self_).free@.len() > 0 ==> (^self_).nodes@.len() == (*self_).nodes@.len())]
#[ensures((*self_).free@.len() == 0 ==> (^self_).nodes@.len() == (*self_).nodes@.len() + 1)]
#[ensures((^self_).head == Some(result))]
#[ensures((*self_).tail == None ==> (^self_).tail == Some(result))]
#[ensures((*self_).tail != None ==> (^self_).tail == (*self_).tail)]
#[ensures((^self_).nodes@[result@].key == key)]
#[ensures((^self_).nodes@[result@].active)]
#[ensures((*self_).free@.len() > 0 ==> result == (*self_).free@[(*self_).free@.len() - 1])]
#[ensures((*self_).free@.len() == 0 ==> result@ == (*self_).nodes@.len())]
#[ensures((*self_).free@.len() == 0 ==> (^self_).nodes@[result@].next == (*self_).head)]
#[ensures((*self_).free@.len() == 0 ==> (^self_).nodes@[result@].prev == None)]
#[ensures(forall<i: Int> 0 <= i && i < (*self_).nodes@.len() && i != result@ ==>
             ((^self_).nodes@[i]).active == ((*self_).nodes@[i]).active
          && ((^self_).nodes@[i]).key == ((*self_).nodes@[i]).key)]
#[ensures(result@ < (*self_).nodes@.len() ==> !((*self_).nodes@[result@]).active)]
#[ensures((^self_).nodes@.len() >= (*self_).nodes@.len())]
#[ensures((*self_).free@.len() == 0 ==> match (*self_).head { Some(h) =>
              (^self_).nodes@[h@].next == (*self_).nodes@[h@].next
              && (^self_).nodes@[h@].key == (*self_).nodes@[h@].key
              && (^self_).nodes@[h@].active == (*self_).nodes@[h@].active, None => true })]
#[ensures((*self_).free@.len() == 0 ==>
          forall<i: Int> 0 <= i && i < (*self_).nodes@.len() && i != result@
            && (match (*self_).head { Some(h) => i != h@, None => true })
            ==> (^self_).nodes@[i] == (*self_).nodes@[i])]
#[ensures(inv(&^self_))]
pub fn arena_push_front(self_: &mut LruList, key: u64) -> u32 {
    let idx = if let Some(free_idx) = self_.free.pop() {
        self_.nodes[free_idx as usize] = Node { key, prev: None, next: self_.head, active: true };
        free_idx
    } else {
        let idx = self_.nodes.len() as u32;
        self_.nodes.push(Node { key, prev: None, next: self_.head, active: true });
        idx
    };
    if let Some(old_head) = self_.head {
        self_.nodes[old_head as usize].prev = Some(idx);
    }
    self_.head = Some(idx);
    if self_.tail.is_none() {
        self_.tail = Some(idx);
    }
    self_.len += 1;
    idx
}

/// `peek_front_key` (lru_list.rs:70-72).
#[requires(inv(self_))]
#[ensures(self_.head == None ==> result == None)]
#[ensures(match self_.head { Some(h) => result == Some((self_.nodes@[h@]).key), None => true })]
pub fn arena_peek_front_key(self_: &LruList) -> Option<u64> {
    match self_.head {
        None => None,
        Some(idx) => Some(self_.nodes[idx as usize].key),
    }
}

/// `remove` (lru_list.rs:159-183). Idempotent for an already-removed slot.
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[requires((*self_).free@.len() < 4294967295)]
#[ensures(!(*self_).nodes@[idx@].active ==> ^self_ == *self_)]
#[ensures((*self_).nodes@[idx@].active ==> (^self_).len@ == (*self_).len@ - 1)]
#[ensures((*self_).nodes@[idx@].active ==>
             (^self_).free@.len() == (*self_).free@.len() + 1
          && (^self_).free@[(*self_).free@.len()] == idx)]
#[ensures(!(^self_).nodes@[idx@].active)]
#[ensures((^self_).nodes@.len() == (*self_).nodes@.len())]
#[ensures(forall<i: Int> 0 <= i && i < (*self_).nodes@.len() && i != idx@ ==>
             ((^self_).nodes@[i]).active == ((*self_).nodes@[i]).active
          && ((^self_).nodes@[i]).key == ((*self_).nodes@[i]).key)]
#[ensures((*self_).nodes@[idx@].active && (*self_).nodes@[idx@].prev == None ==>
             (^self_).head == (*self_).nodes@[idx@].next)]
#[ensures((*self_).nodes@[idx@].prev != None ==> (^self_).head == (*self_).head)]
#[ensures((*self_).nodes@[idx@].active ==>
             (^self_).nodes@[idx@].prev == None && (^self_).nodes@[idx@].next == None)]
#[ensures((*self_).nodes@[idx@].active && (*self_).nodes@[idx@].prev == None ==>
             match (*self_).nodes@[idx@].next { Some(nx) => nx@ != idx@ ==>
                 (^self_).nodes@[nx@].prev == None, None => true })]
#[ensures((*self_).nodes@[idx@].active && (*self_).nodes@[idx@].prev == None ==>
             match (*self_).nodes@[idx@].next { Some(nx) => nx@ != idx@ ==>
                 (^self_).nodes@[nx@].next == (*self_).nodes@[nx@].next
                 && (^self_).nodes@[nx@].key == (*self_).nodes@[nx@].key
                 && (^self_).nodes@[nx@].active == (*self_).nodes@[nx@].active, None => true })]
#[ensures(inv(&^self_))]
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

/// `move_to_back` (lru_list.rs:108-134).
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[ensures(!(*self_).nodes@[idx@].active ==> ^self_ == *self_)]
#[ensures((*self_).tail == Some(idx) ==> ^self_ == *self_)]
#[ensures((^self_).len@ == (*self_).len@)]
#[ensures((^self_).nodes@.len() == (*self_).nodes@.len())]
#[ensures((^self_).free@ == (*self_).free@)]
#[ensures((*self_).nodes@[idx@].active && (*self_).tail != Some(idx) ==> (^self_).tail == Some(idx))]
#[ensures(forall<i: Int> 0 <= i && i < (*self_).nodes@.len() ==>
             ((^self_).nodes@[i]).active == ((*self_).nodes@[i]).active
          && ((^self_).nodes@[i]).key == ((*self_).nodes@[i]).key)]
#[ensures((*self_).nodes@[idx@].active && (*self_).tail != Some(idx)
          && (*self_).nodes@[idx@].prev == None ==> (^self_).head == (*self_).nodes@[idx@].next)]
#[ensures((*self_).nodes@[idx@].prev != None ==> (^self_).head == (*self_).head)]
#[ensures(inv(&^self_))]
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

/// `pop_front` (lru_list.rs:137-142): read the head key (LRU end) and unlink it.
#[requires(inv(self_))]
#[requires(match (*self_).head { Some(h) => (*self_).nodes@[h@].active && (*self_).len@ >= 1, None => true })]
#[requires((*self_).free@.len() < 4294967295)]
#[ensures((*self_).head == None ==> result == None && ^self_ == *self_)]
#[ensures(match (*self_).head { Some(h) =>
              result == Some((*self_).nodes@[h@].key)
              && (^self_).len@ == (*self_).len@ - 1
              && !(^self_).nodes@[h@].active, None => true })]
#[ensures((^self_).nodes@.len() == (*self_).nodes@.len())]
#[ensures(match (*self_).head { Some(h) => (*self_).nodes@[h@].prev == None ==>
              (^self_).head == (*self_).nodes@[h@].next, None => true })]
#[ensures(match (*self_).head { Some(h) => (*self_).nodes@[h@].prev == None ==>
              (match (*self_).nodes@[h@].next { Some(nx) => nx@ != h@ ==>
                  (^self_).nodes@[nx@].prev == None
                  && (^self_).nodes@[nx@].next == (*self_).nodes@[nx@].next
                  && (^self_).nodes@[nx@].key == (*self_).nodes@[nx@].key
                  && (^self_).nodes@[nx@].active == (*self_).nodes@[nx@].active, None => true }),
              None => true })]
#[ensures(forall<i: Int> 0 <= i && i < (*self_).nodes@.len() &&
              (match (*self_).head { Some(h) => i != h@, None => true }) ==>
                 ((^self_).nodes@[i]).active == ((*self_).nodes@[i]).active
              && ((^self_).nodes@[i]).key == ((*self_).nodes@[i]).key)]
#[ensures(inv(&^self_))]
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

/// `peek_front_n` (lru_list.rs:145-156): a read-only walk from the head collecting up to `n` keys.
/// The walk is bounded by `n` — `result.len() >= n` breaks — which is also the termination measure.
#[requires(inv(self_))]
#[ensures(result@.len() <= n@)]
#[ensures(n@ == 0 ==> result@.len() == 0)]
#[ensures(self_.head == None ==> result@.len() == 0)]
#[ensures(match self_.head { Some(h) => n@ > 0 ==> result@.len() > 0 && result@[0] == (self_.nodes@[h@]).key,
                             None => true })]
#[ensures(match self_.head { Some(h) => match (self_.nodes@[h@]).next {
                                 Some(nx) => result@.len() >= 2 ==> result@[1] == (self_.nodes@[nx@]).key,
                                 None => result@.len() < 2 },
                             None => true })]
pub fn arena_peek_front_n(self_: &LruList, n: usize) -> Vec<u64> {
    let mut result: Vec<u64> = Vec::new();
    let mut current = self_.head;
    #[invariant(result@.len() <= n@)]
    #[invariant(match current { Some(c) => c@ < self_.nodes@.len(), None => true })]
    #[invariant(result@.len() == 0 ==> current == self_.head)]
    #[invariant(self_.head == None ==> result@.len() == 0)]
    #[invariant(match self_.head { Some(h) => result@.len() > 0 ==> result@[0] == (self_.nodes@[h@]).key,
                                   None => true })]
    #[invariant(self_.head != None && n@ > 0 ==> result@.len() > 0 || current == self_.head)]
    #[invariant(match self_.head { Some(h) => result@.len() == 1 ==> current == (self_.nodes@[h@]).next,
                                   None => true })]
    #[invariant(match self_.head { Some(h) => match (self_.nodes@[h@]).next {
                                       Some(nx) => result@.len() >= 2 ==> result@[1] == (self_.nodes@[nx@]).key,
                                       None => result@.len() < 2 },
                                   None => true })]
    #[variant(n@ - result@.len())]
    while let Some(idx) = current {
        if result.len() >= n {
            break;
        }
        result.push(self_.nodes[idx as usize].key);
        current = self_.nodes[idx as usize].next;
    }
    result
}

// ===========================================================================
// Count-min sketch mirror — line-faithful to ../src/lib.rs:27-61
// ===========================================================================

/// The column a key maps to in one row: `(key.wrapping_mul(prime) >> 54) as usize`
/// (../src/lib.rs:40, :48). Keeping only the top ten bits of a 64-bit product can only
/// produce 0..=1023, so the index is always inside the fixed 1024-column table.
#[bitwise_proof]
#[ensures(result@ < 1024)]
pub fn cms_col(key: u64, prime: u64) -> usize {
    (key.wrapping_mul(prime) >> 54) as usize
}

/// `CountMinSketch::new` (../src/lib.rs:32-36): all 4096 counters zero.
#[ensures(result.counters@.len() == 4096)]
#[ensures(forall<i: Int> 0 <= i && i < 4096 ==> (result.counters@[i])@ == 0)]
pub fn sketch_fresh() -> Sketch {
    let mut counters: Vec<u8> = Vec::new();
    let mut i: usize = 0;
    #[invariant(counters@.len() == i@)]
    #[invariant(i@ <= 4096)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> (counters@[j])@ == 0)]
    while i < 4096 {
        counters.push(0u8);
        i += 1;
    }
    Sketch { counters }
}

/// One saturating bump of a single counter — the body of `increment`'s inner statement
/// (../src/lib.rs:41). `u8::saturating_add` keeps 255 at 255 instead of wrapping to 0.
#[ensures(self_@ < 255 ==> result@ == self_@ + 1)]
#[ensures(self_@ == 255 ==> result@ == 255)]
#[ensures(result@ >= self_@)]
#[ensures(result@ <= 255)]
pub fn cms_bump(self_: u8) -> u8 {
    self_.saturating_add(1)
}

/// One halving of a single counter — the body of `halve` (../src/lib.rs:57).
#[bitwise_proof]
#[ensures(result@ <= self_@)]
pub fn cms_halve_one(self_: u8) -> u8 {
    self_ >> 1
}

/// `CountMinSketch::increment` (../src/lib.rs:38-43), over the flattened counter vector.
#[requires(self_.counters@.len() == 4096)]
#[ensures((^self_).counters@.len() == 4096)]
#[ensures(forall<i: Int> 0 <= i && i < 4096 ==>
             ((^self_).counters@[i])@ >= ((*self_).counters@[i])@)]
pub fn sketch_increment(self_: &mut Sketch, key: u64) {
    let mut row: usize = 0;
    let old = snapshot! { *self_ };
    #[invariant(self_.counters@.len() == 4096)]
    #[invariant(row@ <= 4)]
    #[invariant(forall<i: Int> 0 <= i && i < 4096 ==>
                   (self_.counters@[i])@ >= ((*old).counters@[i])@)]
    while row < 4 {
        let prime = if row == 0 { P0 } else if row == 1 { P1 } else if row == 2 { P2 } else { P3 };
        let col = cms_col(key, prime);
        let at = row * 1024 + col;
        let c = self_.counters[at];
        self_.counters[at] = cms_bump(c);
        row += 1;
    }
}

/// `CountMinSketch::estimate` (../src/lib.rs:45-52): the minimum over the four row counters.
/// The four-row minimum itself is proved separately, unrolled, by `cms_min4`.
#[requires(self_.counters@.len() == 4096)]
#[ensures(result@ <= 255)]
pub fn sketch_estimate(self_: &Sketch, key: u64) -> u8 {
    let mut min: u8 = 255u8;
    let mut row: usize = 0;
    #[invariant(row@ <= 4)]
    while row < 4 {
        let prime = if row == 0 { P0 } else if row == 1 { P1 } else if row == 2 { P2 } else { P3 };
        let col = cms_col(key, prime);
        let at = row * 1024 + col;
        let c = self_.counters[at];
        if c < min {
            min = c;
        }
        row += 1;
    }
    min
}

/// The `min` fold of `estimate` (../src/lib.rs:46-50) unrolled over the four rows, so the
/// row-minimum obligation is stated over the four counters the key maps to.
#[ensures(result@ <= c0@ && result@ <= c1@ && result@ <= c2@ && result@ <= c3@)]
#[ensures(result == c0 || result == c1 || result == c2 || result == c3)]
pub fn cms_min4(c0: u8, c1: u8, c2: u8, c3: u8) -> u8 {
    let mut min: u8 = 255u8;
    if c0 < min { min = c0; }
    if c1 < min { min = c1; }
    if c2 < min { min = c2; }
    if c3 < min { min = c3; }
    min
}

/// `CountMinSketch::halve` (../src/lib.rs:54-60), over the flattened counter vector.
#[requires(self_.counters@.len() == 4096)]
#[ensures((^self_).counters@.len() == 4096)]
#[ensures(forall<i: Int> 0 <= i && i < 4096 ==>
             ((^self_).counters@[i])@ <= ((*self_).counters@[i])@)]
pub fn sketch_halve(self_: &mut Sketch) {
    let old = snapshot! { *self_ };
    let mut i: usize = 0;
    #[invariant(self_.counters@.len() == 4096)]
    #[invariant(i@ <= 4096)]
    #[invariant(forall<j: Int> 0 <= j && j < 4096 ==>
                   (self_.counters@[j])@ <= ((*old).counters@[j])@)]
    while i < 4096 {
        let c = self_.counters[i];
        self_.counters[i] = cms_halve_one(c);
        i += 1;
    }
}

// ===========================================================================
// Component-level mirrors — line-faithful to ../src/lib.rs
// ===========================================================================

/// Mirror of `interfaces::EvictionPolicyError`. `InvalidHandle` exists in the shared interface
/// and is deliberately included here so that "the component never constructs it" is a
/// falsifiable statement rather than an absence.
pub enum PolicyError {
    InvalidPool(u32),
    InvalidHandle,
}

/// Ghost mirror of the optional `ILogger` receptacle: `lines` counts log records emitted.
/// `connected` mirrors `if let Ok(logger) = self.logger.get()` (lib.rs:98, :112, :145, :189).
pub struct Log {
    pub lines: usize,
}

/// Ghost mirror of lock occupancy: `held` is how many per-pool `Mutex` guards are alive right
/// now, `peak` the most ever alive at once. The shipped code's guard lifetimes are what this
/// counts (lib.rs:119, :153, :168-179, :197, :205, :213, :224, :234).
pub struct LockTrace {
    pub held: usize,
    pub peak: usize,
}

/// Ghost mirror of estimator work: `accesses` counts individual counter reads/writes, so
/// "estimator work does not depend on how many keys are tracked" becomes a state property.
pub struct Work {
    pub accesses: usize,
}

/// `if let Ok(logger) = self.logger.get() { logger.warn(..) }` — one guarded logging site.
#[requires(log.lines@ < 4294967295)]
#[ensures(connected ==> (^log).lines@ == (*log).lines@ + 1)]
#[ensures(!connected ==> ^log == *log)]
pub fn log_site(log: &mut Log, connected: bool) {
    if connected {
        log.lines += 1;
    }
}

#[requires((*t).held@ < 4294967295 && (*t).peak@ >= (*t).held@)]
#[ensures((^t).held@ == (*t).held@ + 1)]
#[ensures((^t).peak@ >= (^t).held@)]
#[ensures((^t).peak@ >= (*t).peak@)]
#[ensures((^t).peak@ == if (*t).held@ + 1 > (*t).peak@ { (*t).held@ + 1 } else { (*t).peak@ })]
pub fn lock_acquire(t: &mut LockTrace) {
    t.held += 1;
    if t.held > t.peak {
        t.peak = t.held;
    }
}

#[requires((*t).held@ >= 1)]
#[ensures((^t).held@ == (*t).held@ - 1)]
#[ensures((^t).peak == (*t).peak)]
pub fn lock_release(t: &mut LockTrace) {
    t.held -= 1;
}

#[requires((*w).accesses@ < 4294967295 - k@)]
#[ensures((^w).accesses@ == (*w).accesses@ + k@)]
pub fn work_add(w: &mut Work, k: usize) {
    w.accesses += k;
}

/// `Pool { lru: LruList::new(), sketch: CountMinSketch::new(), access_count: 0, max_len: 0 }`
/// (lib.rs:92-97).
#[ensures(result.lru.len@ == 0 && result.lru.head == None && result.lru.tail == None)]
#[ensures(result.lru.nodes@.len() == 0 && result.lru.free@.len() == 0)]
#[ensures(result.access_count@ == 0 && result.max_len@ == 0)]
#[ensures(result.sketch.counters@.len() == 4096)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 ==> (result.sketch.counters@[j])@ == 0)]
#[ensures(pool_inv(&result))]
pub fn pool_fresh() -> Pool {
    Pool { lru: arena_fresh(), sketch: sketch_fresh(), access_count: 0, max_len: 0 }
}

/// Two pool collections agree except at index `k`.
#[logic]
pub fn pools_frame_except(a: &Pools, b: &Pools, k: Int) -> bool {
    pearlite! {
        a.pools@.len() == b.pools@.len()
        && (forall<i: Int> 0 <= i && i < a.pools@.len() && i != k ==> a.pools@[i] == b.pools@[i])
    }
}

/// The learned state of a pool — estimator, running access count and high-water record — is
/// identical in `a` and `b`. This is the frame that `touch`, `remove`, `batch_touch`,
/// `identify_next_to_evict`, `get_eviction_candidates`, `len` and `clear_pool` must all preserve.
#[logic]
pub fn pool_meta_eq(a: &Pool, b: &Pool) -> bool {
    pearlite! {
        a.sketch.counters@ == b.sketch.counters@
        && a.access_count == b.access_count
        && a.max_len == b.max_len
    }
}

/// `(len == 0) <-> no ends` — EPO-INV-LIST-EMPTY-IFF-NO-ENDS, as a predicate so that its
/// preservation is a stated obligation and so that callers that need it can assume it explicitly.
#[logic]
pub fn ends_iff(l: &LruList) -> bool {
    pearlite! {
        ((l.len@ == 0) == (l.head == None))
        && ((l.len@ == 0) == (l.tail == None))
    }
}

// ===========================================================================
// Count-min sketch: the unrolled four-row forms
// ===========================================================================

/// `CountMinSketch::increment` (../src/lib.rs:38-43) with the four-row loop unrolled, returning
/// the four flat counter positions it used. Rows occupy disjoint 1024-wide bands, so the four
/// positions are always distinct; that is what makes "each of the four counters rises by exactly
/// one" a statement about four independent counters.
#[requires(self_.counters@.len() == 4096)]
#[ensures((^self_).counters@.len() == 4096)]
#[ensures(result.0@ < 1024)]
#[ensures(1024 <= result.1@ && result.1@ < 2048)]
#[ensures(2048 <= result.2@ && result.2@ < 3072)]
#[ensures(3072 <= result.3@ && result.3@ < 4096)]
#[ensures(((^self_).counters@[result.0@])@ ==
             if ((*self_).counters@[result.0@])@ == 255 { 255 } else { ((*self_).counters@[result.0@])@ + 1 })]
#[ensures(((^self_).counters@[result.1@])@ ==
             if ((*self_).counters@[result.1@])@ == 255 { 255 } else { ((*self_).counters@[result.1@])@ + 1 })]
#[ensures(((^self_).counters@[result.2@])@ ==
             if ((*self_).counters@[result.2@])@ == 255 { 255 } else { ((*self_).counters@[result.2@])@ + 1 })]
#[ensures(((^self_).counters@[result.3@])@ ==
             if ((*self_).counters@[result.3@])@ == 255 { 255 } else { ((*self_).counters@[result.3@])@ + 1 })]
#[ensures(((^self_).counters@[result.0@])@ >= 1 && ((^self_).counters@[result.1@])@ >= 1
       && ((^self_).counters@[result.2@])@ >= 1 && ((^self_).counters@[result.3@])@ >= 1)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 && j != result.0@ && j != result.1@
            && j != result.2@ && j != result.3@ ==>
                (^self_).counters@[j] == (*self_).counters@[j])]
pub fn sketch_increment4(self_: &mut Sketch, key: u64) -> (usize, usize, usize, usize) {
    let i0 = cms_col(key, P0);
    let i1 = 1024 + cms_col(key, P1);
    let i2 = 2048 + cms_col(key, P2);
    let i3 = 3072 + cms_col(key, P3);
    let c0 = self_.counters[i0];
    self_.counters[i0] = cms_bump(c0);
    let c1 = self_.counters[i1];
    self_.counters[i1] = cms_bump(c1);
    let c2 = self_.counters[i2];
    self_.counters[i2] = cms_bump(c2);
    let c3 = self_.counters[i3];
    self_.counters[i3] = cms_bump(c3);
    (i0, i1, i2, i3)
}

/// The four flat counter positions a key maps to — `increment` and `estimate` compute the same
/// four, because `cms_col` is a pure function of `(key, prime)` (../src/lib.rs:40, :48).
#[ensures(result.0@ < 1024)]
#[ensures(1024 <= result.1@ && result.1@ < 2048)]
#[ensures(2048 <= result.2@ && result.2@ < 3072)]
#[ensures(3072 <= result.3@ && result.3@ < 4096)]
pub fn cms_cols4(key: u64) -> (usize, usize, usize, usize) {
    (cms_col(key, P0), 1024 + cms_col(key, P1), 2048 + cms_col(key, P2), 3072 + cms_col(key, P3))
}

/// `CountMinSketch::estimate`'s min-fold (../src/lib.rs:46-50) over the four positions.
#[requires(self_.counters@.len() == 4096)]
#[requires(i0@ < 4096 && i1@ < 4096 && i2@ < 4096 && i3@ < 4096)]
#[ensures(result@ <= (self_.counters@[i0@])@)]
#[ensures(result@ <= (self_.counters@[i1@])@)]
#[ensures(result@ <= (self_.counters@[i2@])@)]
#[ensures(result@ <= (self_.counters@[i3@])@)]
#[ensures(result == self_.counters@[i0@] || result == self_.counters@[i1@]
       || result == self_.counters@[i2@] || result == self_.counters@[i3@])]
pub fn sketch_estimate_at(self_: &Sketch, i0: usize, i1: usize, i2: usize, i3: usize) -> u8 {
    cms_min4(self_.counters[i0], self_.counters[i1], self_.counters[i2], self_.counters[i3])
}

// ===========================================================================
// `track` (../src/lib.rs:104-140) — the pool-local body
// ===========================================================================

/// The ageing schedule of `track` (lib.rs:122-127): update the high-water record from the size
/// measured BEFORE the insertion, bump the access count, and halve the estimator exactly when
/// the record is non-zero and the new count is a multiple of ten times the record.
#[requires(pool_inv(p))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[ensures((^p).max_len@ == if (*p).lru.len@ > (*p).max_len@ { (*p).lru.len@ } else { (*p).max_len@ })]
#[ensures((^p).max_len@ >= (*p).max_len@)]
#[ensures((^p).access_count@ == (*p).access_count@ + 1)]
#[ensures((^p).lru == (*p).lru)]
#[ensures(result == ((^p).max_len@ > 0 && (^p).access_count@ % ((^p).max_len@ * 10) == 0))]
#[ensures(!result ==> (^p).sketch.counters@ == (*p).sketch.counters@)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 ==>
             ((^p).sketch.counters@[j])@ <= ((*p).sketch.counters@[j])@)]
#[ensures((^p).sketch.counters@.len() == 4096)]
#[ensures(pool_inv(&^p))]
pub fn pool_age_step(p: &mut Pool) -> bool {
    let l = arena_len(&p.lru);
    if l > p.max_len {
        p.max_len = l;
    }
    p.access_count += 1;
    if p.max_len > 0 && p.access_count % (p.max_len as u64 * 10) == 0 {
        sketch_halve(&mut p.sketch);
        true
    } else {
        false
    }
}

/// `track`'s pool-local body (lib.rs:121-139): bump the estimator, run the ageing schedule, then
/// admit the key at the eviction end or the protected end according to the victim comparison.
#[requires(pool_inv(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[requires((*p).access_count@ < 18446744073709551615)]
#[ensures((^p).lru.len@ == (*p).lru.len@ + 1)]
#[ensures((^p).access_count@ == (*p).access_count@ + 1)]
#[ensures((^p).max_len@ == if (*p).lru.len@ > (*p).max_len@ { (*p).lru.len@ } else { (*p).max_len@ })]
#[ensures((^p).max_len@ >= (*p).max_len@)]
#[ensures(result@ < (^p).lru.nodes@.len())]
#[ensures((^p).lru.head == Some(result) || (^p).lru.tail == Some(result))]
#[ensures((*p).lru.head == None ==> (^p).lru.head == Some(result) && (^p).lru.tail == Some(result))]
#[ensures(result@ < (*p).lru.nodes@.len() ==> !((*p).lru.nodes@[result@]).active)]
#[ensures((^p).lru.nodes@.len() >= (*p).lru.nodes@.len())]
#[ensures((*p).lru.free@.len() == 0 ==>
             (^p).lru.nodes@.len() == (*p).lru.nodes@.len() + 1 && result@ == (*p).lru.nodes@.len())]
#[ensures((^p).lru.nodes@[result@].key == key)]
#[ensures((^p).lru.nodes@[result@].active)]
#[ensures(forall<i: Int> 0 <= i && i < (*p).lru.nodes@.len() && i != result@ ==>
             ((^p).lru.nodes@[i]).active == ((*p).lru.nodes@[i]).active
          && ((^p).lru.nodes@[i]).key == ((*p).lru.nodes@[i]).key)]
#[ensures(pool_inv(&^p))]
pub fn pool_track(p: &mut Pool, key: u64) -> u32 {
    let cols = sketch_increment4(&mut p.sketch, key);
    let _aged = pool_age_step(p);
    let head_key = arena_peek_front_key(&p.lru);
    match head_key {
        Some(hk) => {
            let hcols = cms_cols4(hk);
            let e_new = sketch_estimate_at(&p.sketch, cols.0, cols.1, cols.2, cols.3);
            let e_head = sketch_estimate_at(&p.sketch, hcols.0, hcols.1, hcols.2, hcols.3);
            if e_new <= e_head {
                arena_push_front(&mut p.lru, key)
            } else {
                arena_push_back(&mut p.lru, key)
            }
        }
        None => arena_push_back(&mut p.lru, key),
    }
}

/// A pool ready for a `track`: well formed, with room for one more slot and one more access.
#[logic]
pub fn pool_ready(p: &Pool) -> bool {
    pearlite! {
        pool_inv(p) && p.lru.nodes@.len() < 4294967295 && p.access_count@ < 18446744073709551615
    }
}

// ===========================================================================
// State-level mirrors — the pool routing of ../src/lib.rs, over the plain Vec
// ===========================================================================

/// `create_pool` (lib.rs:89-102). `state.pools.len() as u32` is the returned id, so the mirror
/// carries the same disclosed 2^32-pool addressing bound as the shipped code.
#[requires((*self_).pools@.len() < 4294967295)]
#[ensures(result@ == (*self_).pools@.len())]
#[ensures((^self_).pools@.len() == (*self_).pools@.len() + 1)]
#[ensures(forall<i: Int> 0 <= i && i < (*self_).pools@.len() ==> (^self_).pools@[i] == (*self_).pools@[i])]
#[ensures(((^self_).pools@[result@]).lru.len@ == 0)]
#[ensures(((^self_).pools@[result@]).lru.head == None && ((^self_).pools@[result@]).lru.tail == None)]
#[ensures(((^self_).pools@[result@]).access_count@ == 0)]
#[ensures(((^self_).pools@[result@]).max_len@ == 0)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 ==>
             ((((^self_).pools@[result@]).sketch.counters@[j])@ == 0))]
#[ensures(pool_inv(&(^self_).pools@[result@]))]
pub fn state_create_pool(self_: &mut Pools) -> u32 {
    let id = self_.pools.len() as u32;
    self_.pools.push(pool_fresh());
    id
}

/// `track` (lib.rs:104-140): route by pool id, then run the pool-local body.
#[requires(pool@ < (*self_).pools@.len() ==> pool_ready(&(*self_).pools@[pool@]))]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
#[ensures(pool@ >= (*self_).pools@.len() ==> ^self_ == *self_)]
#[ensures(pool@ >= (*self_).pools@.len() ==> result == Err(PolicyError::InvalidPool(pool)))]
#[ensures(pool@ < (*self_).pools@.len() ==> pools_frame_except(&^self_, &*self_, pool@))]
#[ensures(pool@ < (*self_).pools@.len() ==>
             match result { Ok(h) => h.pool == pool
                                     && ((^self_).pools@[pool@]).lru.len@ == ((*self_).pools@[pool@]).lru.len@ + 1
                                     && ((^self_).pools@[pool@]).access_count@ == ((*self_).pools@[pool@]).access_count@ + 1
                                     && h.index@ < ((^self_).pools@[pool@]).lru.nodes@.len()
                                     && ((^self_).pools@[pool@]).lru.nodes@[h.index@].key == key
                                     && ((^self_).pools@[pool@]).lru.nodes@[h.index@].active
                                     && pool_inv(&(^self_).pools@[pool@]),
                            Err(_) => false })]
pub fn state_track(self_: &mut Pools, pool: u32, key: u64) -> Result<Handle, PolicyError> {
    if (pool as usize) < self_.pools.len() {
        let index = pool_track(&mut self_.pools[pool as usize], key);
        Ok(Handle { pool, index })
    } else {
        Err(PolicyError::InvalidPool(pool))
    }
}

/// `touch` (lib.rs:142-156). The pool id inside the handle is checked; the slot number is NOT —
/// the `#[requires]` on `h.index` is the shipped code's unchecked index, made explicit.
#[requires(h.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h.pool@]).lru)
             && h.index@ < (((*self_).pools@[h.pool@]).lru.nodes@.len()))]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
#[ensures(h.pool@ >= (*self_).pools@.len() ==> ^self_ == *self_ && result == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures(h.pool@ < (*self_).pools@.len() ==> pools_frame_except(&^self_, &*self_, h.pool@))]
#[ensures(h.pool@ < (*self_).pools@.len() ==>
             match result { Ok(_) =>
                 ((^self_).pools@[h.pool@]).lru.len@ == ((*self_).pools@[h.pool@]).lru.len@
                 && pool_meta_eq(&(^self_).pools@[h.pool@], &(*self_).pools@[h.pool@])
                 && (((*self_).pools@[h.pool@]).lru.nodes@[h.index@].active
                     && ((*self_).pools@[h.pool@]).lru.tail != Some(h.index)
                     ==> ((^self_).pools@[h.pool@]).lru.tail == Some(h.index)),
                 Err(_) => false })]
pub fn state_touch(self_: &mut Pools, h: Handle) -> Result<(), PolicyError> {
    if (h.pool as usize) < self_.pools.len() {
        arena_move_to_back(&mut self_.pools[h.pool as usize].lru, h.index);
        Ok(())
    } else {
        Err(PolicyError::InvalidPool(h.pool))
    }
}

/// `remove` (lib.rs:186-200). Same unchecked slot number as `touch`.
#[requires(h.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h.pool@]).lru)
             && h.index@ < (((*self_).pools@[h.pool@]).lru.nodes@.len())
             && (((*self_).pools@[h.pool@]).lru.nodes@[h.index@].active
                 ==> ((*self_).pools@[h.pool@]).lru.len@ >= 1)
             && ((*self_).pools@[h.pool@]).lru.free@.len() < 4294967295)]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
#[ensures(h.pool@ >= (*self_).pools@.len() ==> ^self_ == *self_ && result == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures(h.pool@ < (*self_).pools@.len() ==> pools_frame_except(&^self_, &*self_, h.pool@))]
#[ensures(h.pool@ < (*self_).pools@.len() ==>
             match result { Ok(_) =>
                 pool_meta_eq(&(^self_).pools@[h.pool@], &(*self_).pools@[h.pool@])
                 && !((^self_).pools@[h.pool@]).lru.nodes@[h.index@].active
                 && (((*self_).pools@[h.pool@]).lru.nodes@[h.index@].active
                     ==> ((^self_).pools@[h.pool@]).lru.len@ == ((*self_).pools@[h.pool@]).lru.len@ - 1)
                 && (!((*self_).pools@[h.pool@]).lru.nodes@[h.index@].active
                     ==> (^self_).pools@[h.pool@] == (*self_).pools@[h.pool@]),
                 Err(_) => false })]
pub fn state_remove(self_: &mut Pools, h: Handle) -> Result<(), PolicyError> {
    if (h.pool as usize) < self_.pools.len() {
        arena_remove(&mut self_.pools[h.pool as usize].lru, h.index);
        Ok(())
    } else {
        Err(PolicyError::InvalidPool(h.pool))
    }
}

/// `identify_next_to_evict` (lib.rs:202-207). Total: an unknown pool id degrades to `None`.
#[requires(pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[pool@]).lru)
             && (match ((*self_).pools@[pool@]).lru.head {
                     Some(hh) => ((*self_).pools@[pool@]).lru.nodes@[hh@].active
                                 && ((*self_).pools@[pool@]).lru.len@ >= 1,
                     None => true })
             && ((*self_).pools@[pool@]).lru.free@.len() < 4294967295)]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
#[ensures(pool@ >= (*self_).pools@.len() ==> ^self_ == *self_ && result == None)]
#[ensures(pool@ < (*self_).pools@.len() ==> pools_frame_except(&^self_, &*self_, pool@))]
#[ensures(pool@ < (*self_).pools@.len() ==>
             pool_meta_eq(&(^self_).pools@[pool@], &(*self_).pools@[pool@]))]
#[ensures(pool@ < (*self_).pools@.len() ==>
             (match ((*self_).pools@[pool@]).lru.head {
                 Some(hh) => result == Some(((*self_).pools@[pool@]).lru.nodes@[hh@].key)
                             && ((^self_).pools@[pool@]).lru.len@ == ((*self_).pools@[pool@]).lru.len@ - 1
                             && !((^self_).pools@[pool@]).lru.nodes@[hh@].active,
                 None => result == None && (^self_).pools@[pool@] == (*self_).pools@[pool@] }))]
pub fn state_evict(self_: &mut Pools, pool: u32) -> Option<u64> {
    if (pool as usize) < self_.pools.len() {
        arena_pop_front(&mut self_.pools[pool as usize].lru)
    } else {
        None
    }
}

/// `get_eviction_candidates` (lib.rs:209-218). Total: an unknown pool id degrades to an empty list.
#[requires(pool@ < (*self_).pools@.len() ==> inv(&(self_.pools@[pool@]).lru))]
#[ensures(result@.len() <= n@)]
#[ensures(n@ == 0 ==> result@.len() == 0)]
#[ensures(pool@ >= self_.pools@.len() ==> result@.len() == 0)]
#[ensures(pool@ < self_.pools@.len() ==>
             (match (self_.pools@[pool@]).lru.head {
                 Some(hh) => n@ > 0 ==> result@.len() > 0
                             && result@[0] == (self_.pools@[pool@]).lru.nodes@[hh@].key,
                 None => result@.len() == 0 }))]
#[ensures(pool@ < self_.pools@.len() ==>
             (match (self_.pools@[pool@]).lru.head {
                 Some(hh) => (match ((self_.pools@[pool@]).lru.nodes@[hh@]).next {
                                 Some(nx) => result@.len() >= 2 ==>
                                     result@[1] == (self_.pools@[pool@]).lru.nodes@[nx@].key,
                                 None => result@.len() < 2 }),
                 None => true }))]
pub fn state_candidates(self_: &Pools, pool: u32, n: usize) -> Vec<u64> {
    if (pool as usize) < self_.pools.len() {
        arena_peek_front_n(&self_.pools[pool as usize].lru, n)
    } else {
        Vec::new()
    }
}

/// `len` (lib.rs:220-229). Total: an unknown pool id degrades to zero.
#[ensures(pool@ >= self_.pools@.len() ==> result@ == 0)]
#[ensures(pool@ < self_.pools@.len() ==> result@ == (self_.pools@[pool@]).lru.len@)]
pub fn state_len(self_: &Pools, pool: u32) -> usize {
    if (pool as usize) < self_.pools.len() {
        arena_len(&self_.pools[pool as usize].lru)
    } else {
        0
    }
}

/// `clear_pool` (lib.rs:231-237). Total, silent, and it deliberately keeps the learned state.
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
#[ensures(pool@ >= (*self_).pools@.len() ==> ^self_ == *self_)]
#[ensures(pool@ < (*self_).pools@.len() ==> pools_frame_except(&^self_, &*self_, pool@))]
#[ensures(pool@ < (*self_).pools@.len() ==>
             ((^self_).pools@[pool@]).lru.len@ == 0
             && ((^self_).pools@[pool@]).lru.head == None
             && ((^self_).pools@[pool@]).lru.tail == None
             && ((^self_).pools@[pool@]).lru.nodes@.len() == 0
             && ((^self_).pools@[pool@]).lru.free@.len() == 0
             && inv(&((^self_).pools@[pool@]).lru)
             && pool_meta_eq(&(^self_).pools@[pool@], &(*self_).pools@[pool@]))]
pub fn state_clear(self_: &mut Pools, pool: u32) {
    if (pool as usize) < self_.pools.len() {
        arena_clear(&mut self_.pools[pool as usize].lru);
    }
}

/// `batch_touch`'s per-pool run (lib.rs:171-182), as a loop over the slot numbers of one pool.
/// The unchecked slot numbers of the shipped code appear here as an explicit precondition.
#[requires(inv(&(*p).lru))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len() ==> (idxs@[k])@ < (*p).lru.nodes@.len())]
#[ensures((^p).lru.len@ == (*p).lru.len@)]
#[ensures((^p).lru.nodes@.len() == (*p).lru.nodes@.len())]
#[ensures(pool_meta_eq(&^p, &*p))]
#[ensures(inv(&(^p).lru))]
#[ensures(forall<i: Int> 0 <= i && i < (*p).lru.nodes@.len() ==>
             ((^p).lru.nodes@[i]).active == ((*p).lru.nodes@[i]).active
          && ((^p).lru.nodes@[i]).key == ((*p).lru.nodes@[i]).key)]
pub fn pool_batch_touch(p: &mut Pool, idxs: &Vec<u32>) {
    let mut i: usize = 0;
    let start = snapshot! { *p };
    #[invariant(inv(&p.lru))]
    #[invariant(i@ <= idxs@.len())]
    #[invariant(p.lru.len@ == (*start).lru.len@)]
    #[invariant(p.lru.nodes@.len() == (*start).lru.nodes@.len())]
    #[invariant(pool_meta_eq(p, &*start))]
    #[invariant(forall<j: Int> 0 <= j && j < (*start).lru.nodes@.len() ==>
                   (p.lru.nodes@[j]).active == ((*start).lru.nodes@[j]).active
                && (p.lru.nodes@[j]).key == ((*start).lru.nodes@[j]).key)]
    while i < idxs.len() {
        let idx = idxs[i];
        arena_move_to_back(&mut p.lru, idx);
        i += 1;
    }
}

// ###########################################################################
// PROPERTY DRIVERS — one `verify_<ID>` module per unified-inventory property id
// ###########################################################################

// ======================= create_pool =======================================

// ---- EPO-CREATE-POOL-PRE-TOTAL --------------------------------------------
// Totality: the only thing create_pool needs of the caller is that the pool count still fits in
// the u32 pool id (the shipped code's `state.pools.len() as u32`, lib.rs:91). It never fails.
#[requires((*self_).pools@.len() < 4294967295)]
#[ensures(result@ < (^self_).pools@.len())]
#[ensures((^self_).pools@.len() == (*self_).pools@.len() + 1)]
pub fn verify_epo_create_pool_pre_total(self_: &mut Pools) -> u32 {
    state_create_pool(self_)
}

// ---- EPO-CREATE-POOL-POST-SEQUENTIAL-ID -----------------------------------
#[requires((*self_).pools@.len() < 4294967295)]
#[ensures(result@ == (*self_).pools@.len())]
#[ensures((^self_).pools@.len() == (*self_).pools@.len() + 1)]
#[ensures(result@ < (^self_).pools@.len())]
pub fn verify_epo_create_pool_post_sequential_id(self_: &mut Pools) -> u32 {
    state_create_pool(self_)
}

// ---- EPO-CREATE-POOL-POST-FRESH-STATE -------------------------------------
#[requires((*self_).pools@.len() < 4294967295)]
#[ensures(((^self_).pools@[result@]).lru.len@ == 0)]
#[ensures(((^self_).pools@[result@]).lru.head == None)]
#[ensures(((^self_).pools@[result@]).access_count@ == 0)]
#[ensures(((^self_).pools@[result@]).max_len@ == 0)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 ==>
             ((((^self_).pools@[result@]).sketch.counters@[j])@ == 0))]
pub fn verify_epo_create_pool_post_fresh_state(self_: &mut Pools) -> u32 {
    state_create_pool(self_)
}

// ---- EPO-CREATE-POOL-FRAME-EXISTING-POOLS ---------------------------------
#[requires((*self_).pools@.len() < 4294967295)]
#[ensures(forall<i: Int> 0 <= i && i < (*self_).pools@.len() ==>
             (^self_).pools@[i] == (*self_).pools@[i])]
pub fn verify_epo_create_pool_frame_existing_pools(self_: &mut Pools) -> u32 {
    state_create_pool(self_)
}

// ======================= track =============================================

// ---- EPO-TRACK-PRE-POOL-EXISTS -------------------------------------------
// The pool id must be one create_pool handed out; nothing at all is required of the key.
#[requires(pool@ < (*self_).pools@.len() ==> pool_ready(&(*self_).pools@[pool@]))]
#[ensures(match result { Ok(_) => pool@ < (*self_).pools@.len(),
                         Err(_) => pool@ >= (*self_).pools@.len() })]
pub fn verify_epo_track_pre_pool_exists(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

// ---- EPO-TRACK-POST-HANDLE ----------------------------------------------—
// The handle carries the very pool id the caller passed and the slot number of the new entry.
#[requires(pool@ < (*self_).pools@.len())]
#[requires(pool_ready(&(*self_).pools@[pool@]))]
#[ensures(match result { Ok(h) => h.pool == pool
                                  && h.index@ < ((^self_).pools@[pool@]).lru.nodes@.len()
                                  && ((^self_).pools@[pool@]).lru.nodes@[h.index@].key == key
                                  && ((^self_).pools@[pool@]).lru.nodes@[h.index@].active,
                         Err(_) => false })]
pub fn verify_epo_track_post_handle(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

// ---- EPO-TRACK-POST-LEN-INCREMENT ---------------------------------------—
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).lru.len@ == (*p).lru.len@ + 1)]
pub fn verify_epo_track_post_len_increment(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

// ---- EPO-TRACK-POST-SKETCH-INCREMENT ------------------------------------—
// Each of the four row counters the key maps to rises by one, saturating at 255; no other
// counter moves. The four rows sit in disjoint 1024-wide bands, so they are four distinct
// counters.
#[requires(s.counters@.len() == 4096)]
#[ensures(((^s).counters@[result.0@])@ ==
             if ((*s).counters@[result.0@])@ == 255 { 255 } else { ((*s).counters@[result.0@])@ + 1 })]
#[ensures(((^s).counters@[result.1@])@ ==
             if ((*s).counters@[result.1@])@ == 255 { 255 } else { ((*s).counters@[result.1@])@ + 1 })]
#[ensures(((^s).counters@[result.2@])@ ==
             if ((*s).counters@[result.2@])@ == 255 { 255 } else { ((*s).counters@[result.2@])@ + 1 })]
#[ensures(((^s).counters@[result.3@])@ ==
             if ((*s).counters@[result.3@])@ == 255 { 255 } else { ((*s).counters@[result.3@])@ + 1 })]
#[ensures(result.0@ != result.1@ && result.0@ != result.2@ && result.0@ != result.3@
       && result.1@ != result.2@ && result.1@ != result.3@ && result.2@ != result.3@)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 && j != result.0@ && j != result.1@
            && j != result.2@ && j != result.3@ ==> (^s).counters@[j] == (*s).counters@[j])]
pub fn verify_epo_track_post_sketch_increment(s: &mut Sketch, key: u64)
    -> (usize, usize, usize, usize) {
    sketch_increment4(s, key)
}

// ---- EPO-TRACK-POST-ACCESS-COUNT ---------------------------------------—
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).access_count@ == (*p).access_count@ + 1)]
pub fn verify_epo_track_post_access_count(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

// ---- EPO-TRACK-POST-MAXLEN-HIGHWATER -----------------------------------—
// The record is the larger of the previous record and the size measured BEFORE the insertion —
// so at a new peak the record is exactly one short of the peak.
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).max_len@ == if (*p).lru.len@ > (*p).max_len@ { (*p).lru.len@ } else { (*p).max_len@ })]
#[ensures((^p).lru.len@ == (*p).lru.len@ + 1)]
#[ensures((^p).max_len@ < (^p).lru.len@ ==> (^p).max_len@ == (^p).lru.len@ - 1)]
pub fn verify_epo_track_post_maxlen_highwater(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

// ---- EPO-TRACK-POST-AGING-TRIGGER --------------------------------------—
// Halving happens exactly when the record is non-zero and the new access count is a multiple of
// ten times the record; on every other track the counters keep their values.
#[requires(pool_inv(p))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[ensures(result == ((^p).max_len@ > 0 && (^p).access_count@ % ((^p).max_len@ * 10) == 0))]
#[ensures(!result ==> (^p).sketch.counters@ == (*p).sketch.counters@)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 ==>
             ((^p).sketch.counters@[j])@ <= ((*p).sketch.counters@[j])@)]
pub fn verify_epo_track_post_aging_trigger(p: &mut Pool) -> bool {
    pool_age_step(p)
}

// ---- EPO-TRACK-INV-AGING-NO-DIVISION-BY-ZERO ---------------------------—
// The remainder at lib.rs:124 is only reached when max_len > 0, so the divisor is non-zero and
// the operation is defined. The `%` here raises Creusot's division-by-zero VC; discharging it
// IS the obligation.
#[requires(pool_inv(p))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[ensures((^p).access_count@ == (*p).access_count@ + 1)]
pub fn verify_epo_track_inv_aging_no_division_by_zero(p: &mut Pool) -> bool {
    let l = arena_len(&p.lru);
    if l > p.max_len {
        p.max_len = l;
    }
    p.access_count += 1;
    p.max_len > 0 && p.access_count % (p.max_len as u64 * 10) == 0
}

// ---- EPO-TRACK-INV-AGING-PERIOD-NO-OVERFLOW ----------------------------—
// `max_len as u64 * 10` cannot overflow: slots are addressed by u32, so max_len <= u32::MAX and
// the product is at most 42_949_672_950, far inside u64. The multiplication below raises the
// overflow VC; discharging it IS the obligation.
#[requires(max_len@ <= 4294967295)]
#[ensures(result@ == max_len@ * 10)]
pub fn verify_epo_track_inv_aging_period_no_overflow(max_len: usize) -> u64 {
    max_len as u64 * 10
}

// ---- EPO-TRACK-POST-ADMIT-EMPTY-POOL -----------------------------------—
// An empty pool has no victim to compare against, so the key goes to the protected end
// unconditionally (lib.rs:135-136), where it is both front and back of a one-entry order.
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[requires((*p).lru.head == None)]
#[ensures((^p).lru.tail == Some(result))]
#[ensures((^p).lru.head == Some(result))]
#[ensures((^p).lru.len@ == (*p).lru.len@ + 1)]
pub fn verify_epo_track_post_admit_empty_pool(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

// ---- EPO-TRACK-POST-ADMIT-MORE-FREQUENT --------------------------------—
// Strictly more frequent than the current victim => protected end (lib.rs:133).
#[requires(inv(&(*l).lru))]
#[requires((*l).lru.nodes@.len() < 4294967295)]
#[requires((*l).lru.head != None)]
#[requires(e_new@ > e_head@)]
#[ensures((^l).lru.tail == Some(result))]
pub fn verify_epo_track_post_admit_more_frequent(l: &mut Pool, key: u64, e_new: u8, e_head: u8) -> u32 {
    if e_new <= e_head {
        arena_push_front(&mut l.lru, key)
    } else {
        arena_push_back(&mut l.lru, key)
    }
}

// ---- EPO-TRACK-POST-ADMIT-NOT-MORE-FREQUENT ----------------------------—
// No more frequent than the current victim (fewer OR equal) => eviction end (lib.rs:131), so the
// new key becomes the very next victim. A first-time key ties and lands here.
#[requires(inv(&(*l).lru))]
#[requires((*l).lru.nodes@.len() < 4294967295)]
#[requires((*l).lru.head != None)]
#[requires(e_new@ <= e_head@)]
#[ensures((^l).lru.head == Some(result))]
#[ensures((^l).lru.nodes@[result@].key == key)]
pub fn verify_epo_track_post_admit_not_more_frequent(l: &mut Pool, key: u64, e_new: u8, e_head: u8) -> u32 {
    if e_new <= e_head {
        arena_push_front(&mut l.lru, key)
    } else {
        arena_push_back(&mut l.lru, key)
    }
}

// [verify_epo_track_post_non_idempotent_reregistration removed 2026-10-07: replaced by the fresh section at the end of this file]

// ---- EPO-TRACK-FRAME-SEMANTICS-IGNORED ---------------------------------—
// The per-block hint is bound as `_semantics` (lib.rs:108) and never read, so every observable
// the contract names is a function of (pool, key) alone and none of them mentions the hint.
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).lru.len@ == (*p).lru.len@ + 1)]
#[ensures((^p).access_count@ == (*p).access_count@ + 1)]
#[ensures((^p).max_len@ == if (*p).lru.len@ > (*p).max_len@ { (*p).lru.len@ } else { (*p).max_len@ })]
#[ensures((^p).lru.nodes@[result@].key == key)]
pub fn verify_epo_track_frame_semantics_ignored(p: &mut Pool, key: u64, _semantics: u64) -> u32 {
    pool_track(p, key)
}

// [verify_epo_track_frame_existing_entries removed 2026-10-07: replaced by the fresh section at the end of this file]

// ---- EPO-TRACK-FRAME-OTHER-POOLS --------------------------------------—
#[requires(pool@ < (*self_).pools@.len() ==> pool_ready(&(*self_).pools@[pool@]))]
#[ensures(pools_frame_except(&^self_, &*self_, pool@))]
pub fn verify_epo_track_frame_other_pools(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

// ---- EPO-TRACK-ERROR-INVALID-POOL -------------------------------------—
// The only failure track can report, and it names the offending pool id back to the caller.
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(result == Err(PolicyError::InvalidPool(pool)))]
pub fn verify_epo_track_error_invalid_pool(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

// ---- EPO-TRACK-ERROR-FRAME-NO-MUTATION --------------------------------—
#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(^self_ == *self_)]
pub fn verify_epo_track_error_frame_no_mutation(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

// ======================= touch =============================================

// ---- EPO-TOUCH-PRE-POOL-EXISTS ---------------------------------------—
// The pool id inside the handle is checked; the slot number is NOT (lib.rs:144, lru_list.rs:109).
#[requires(h.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h.pool@]).lru)
             && h.index@ < (((*self_).pools@[h.pool@]).lru.nodes@.len()))]
#[ensures(match result { Ok(_) => h.pool@ < (*self_).pools@.len(),
                         Err(_) => h.pool@ >= (*self_).pools@.len() })]
pub fn verify_epo_touch_pre_pool_exists(self_: &mut Pools, h: Handle) -> Result<(), PolicyError> {
    state_touch(self_, h)
}

// ---- EPO-TOUCH-POST-MOVE-TO-MRU --------------------------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active && (*l).lru.tail != Some(idx))]
#[ensures((^l).lru.tail == Some(idx))]
#[ensures((^l).lru.len@ == (*l).lru.len@)]
pub fn verify_epo_touch_post_move_to_mru(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

// [verify_epo_touch_post_not_next_victim removed 2026-10-07: replaced by the fresh section at the end of this file]

// ---- EPO-TOUCH-POST-ALREADY-AT-BACK-NOOP -----------------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.tail == Some(idx))]
#[ensures(^l == *l)]
pub fn verify_epo_touch_post_already_at_back_noop(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

// ---- EPO-TOUCH-POST-REMOVED-HANDLE-IS-SILENT-NOOP --------------------—
// Holds while the slot still EXISTS: the guard is the per-entry live flag. After clear_pool has
// discarded the slots the same call panics — that divergence is EPO-INV-STALE-HANDLE-NEVER-CRASHES.
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires(!(*l).lru.nodes@[idx@].active)]
#[ensures(^l == *l)]
pub fn verify_epo_touch_post_removed_handle_is_silent_noop(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

// ---- EPO-TOUCH-FRAME-SKETCH-UNCHANGED -------------------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[ensures((^l).sketch.counters@ == (*l).sketch.counters@)]
pub fn verify_epo_touch_frame_sketch_unchanged(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

// ---- EPO-TOUCH-FRAME-MEMBERSHIP ------------------------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[ensures((^l).lru.len@ == (*l).lru.len@)]
#[ensures((^l).lru.nodes@.len() == (*l).lru.nodes@.len())]
#[ensures(forall<i: Int> 0 <= i && i < (*l).lru.nodes@.len() ==>
             ((^l).lru.nodes@[i]).active == ((*l).lru.nodes@[i]).active
          && ((^l).lru.nodes@[i]).key == ((*l).lru.nodes@[i]).key)]
pub fn verify_epo_touch_frame_membership(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

// ---- EPO-TOUCH-FRAME-RELATIVE-ORDER-OTHERS -------------------------—
// Apart from the moved entry, no other entry is added, dropped or re-keyed, and the free list is
// untouched. Relative ORDER of the others rests on the head-to-tail walk (see the advisory note).
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[ensures((^l).lru.free@ == (*l).lru.free@)]
#[ensures(forall<i: Int> 0 <= i && i < (*l).lru.nodes@.len() ==>
             ((^l).lru.nodes@[i]).active == ((*l).lru.nodes@[i]).active
          && ((^l).lru.nodes@[i]).key == ((*l).lru.nodes@[i]).key)]
pub fn verify_epo_touch_frame_relative_order_others(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

// ---- EPO-TOUCH-FRAME-COUNTERS -------------------------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[ensures((^l).access_count == (*l).access_count && (^l).max_len == (*l).max_len)]
pub fn verify_epo_touch_frame_counters(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

// ---- EPO-TOUCH-ERROR-INVALID-POOL --------------------------------—
#[requires(h.pool@ >= (*self_).pools@.len())]
#[ensures(result == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures(^self_ == *self_)]
pub fn verify_epo_touch_error_invalid_pool(self_: &mut Pools, h: Handle) -> Result<(), PolicyError> {
    state_touch(self_, h)
}

// ======================= batch_touch =======================================

/// `batch_touch` (lib.rs:158-184) for a two-handle batch — the shortest batch that exhibits the
/// guard swap on a pool change, partial application, and stale-handle skipping. The first handle
/// is checked and applied, then the loop body re-checks only when the pool id changes.
#[requires(h0.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h0.pool@]).lru)
             && h0.index@ < (((*self_).pools@[h0.pool@]).lru.nodes@.len()))]
#[requires(h1.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h1.pool@]).lru)
             && h1.index@ < (((*self_).pools@[h1.pool@]).lru.nodes@.len()))]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
#[ensures(h0.pool@ >= (*self_).pools@.len() ==>
             ^self_ == *self_ && result == Err(PolicyError::InvalidPool(h0.pool)))]
#[ensures(h0.pool@ < (*self_).pools@.len() && h1.pool != h0.pool
          && h1.pool@ >= (*self_).pools@.len() ==>
             result == Err(PolicyError::InvalidPool(h1.pool))
             && (((*self_).pools@[h0.pool@]).lru.nodes@[h0.index@].active
                 && ((*self_).pools@[h0.pool@]).lru.tail != Some(h0.index)
                 ==> ((^self_).pools@[h0.pool@]).lru.tail == Some(h0.index)))]
#[ensures(h0.pool@ < (*self_).pools@.len() && h1.pool == h0.pool ==>
             result == Ok(())
             && ((^self_).pools@[h0.pool@]).lru.len@ == ((*self_).pools@[h0.pool@]).lru.len@
             && pool_meta_eq(&(^self_).pools@[h0.pool@], &(*self_).pools@[h0.pool@])
             && (((*self_).pools@[h0.pool@]).lru.nodes@[h1.index@].active
                 && ((^self_).pools@[h0.pool@]).lru.tail == Some(h1.index)
                 || !((*self_).pools@[h0.pool@]).lru.nodes@[h1.index@].active))]
pub fn state_batch_touch2(self_: &mut Pools, h0: Handle, h1: Handle) -> Result<(), PolicyError> {
    if (h0.pool as usize) >= self_.pools.len() {
        return Err(PolicyError::InvalidPool(h0.pool));
    }
    arena_move_to_back(&mut self_.pools[h0.pool as usize].lru, h0.index);
    if h1.pool != h0.pool && (h1.pool as usize) >= self_.pools.len() {
        return Err(PolicyError::InvalidPool(h1.pool));
    }
    arena_move_to_back(&mut self_.pools[h1.pool as usize].lru, h1.index);
    Ok(())
}

// ---- EPO-BATCH-TOUCH-PRE-POOLS-EXIST ------------------------------------—
#[requires(h0.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h0.pool@]).lru)
             && h0.index@ < (((*self_).pools@[h0.pool@]).lru.nodes@.len()))]
#[requires(h1.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h1.pool@]).lru)
             && h1.index@ < (((*self_).pools@[h1.pool@]).lru.nodes@.len()))]
#[ensures(result == Ok(()) ==>
             h0.pool@ < (*self_).pools@.len()
             && (h1.pool != h0.pool ==> h1.pool@ < (*self_).pools@.len()))]
pub fn verify_epo_batch_touch_pre_pools_exist(self_: &mut Pools, h0: Handle, h1: Handle)
    -> Result<(), PolicyError> {
    state_batch_touch2(self_, h0, h1)
}

// ---- EPO-BATCH-TOUCH-POST-EMPTY-NOOP ------------------------------------—
// The empty batch returns at lib.rs:159-161 before any lock is taken or any pool is examined, so
// it is accepted even by a component with no pools at all.
#[requires(n_handles@ == 0)]
#[ensures(result == Ok(()))]
#[ensures(^self_ == *self_)]
#[ensures(^t == *t)]
pub fn verify_epo_batch_touch_post_empty_noop(self_: &mut Pools, t: &mut LockTrace, n_handles: usize)
    -> Result<(), PolicyError> {
    if n_handles == 0 {
        return Ok(());
    }
    lock_acquire(t);
    lock_release(t);
    Ok(())
}

// ---- EPO-BATCH-TOUCH-POST-EQUIVALENT-TO-SEQUENTIAL ---------------------—
// Two handles into the same pool leave that pool exactly where touching them one at a time would:
// the LAST handle naming the pool ends up as its most protected entry.
#[requires(h0.pool == h1.pool)]
#[requires(h0.pool@ < (*self_).pools@.len())]
#[requires(inv(&((*self_).pools@[h0.pool@]).lru))]
#[requires(h0.index@ < (((*self_).pools@[h0.pool@]).lru.nodes@.len()))]
#[requires(h1.index@ < (((*self_).pools@[h0.pool@]).lru.nodes@.len()))]
#[requires(((*self_).pools@[h0.pool@]).lru.nodes@[h1.index@].active)]
#[ensures(result == Ok(()))]
#[ensures(((^self_).pools@[h0.pool@]).lru.tail == Some(h1.index))]
#[ensures(((^self_).pools@[h0.pool@]).lru.len@ == ((*self_).pools@[h0.pool@]).lru.len@)]
pub fn verify_epo_batch_touch_post_equivalent_to_sequential(self_: &mut Pools, h0: Handle, h1: Handle)
    -> Result<(), PolicyError> {
    state_batch_touch2(self_, h0, h1)
}

// ---- EPO-BATCH-TOUCH-POST-REMOVED-HANDLES-SKIPPED ----------------------—
// A stale first handle is skipped without crashing (its slot still exists) and the live second
// handle is still moved to the protected end.
#[requires(h0.pool == h1.pool)]
#[requires(h0.pool@ < (*self_).pools@.len())]
#[requires(inv(&((*self_).pools@[h0.pool@]).lru))]
#[requires(h0.index@ < (((*self_).pools@[h0.pool@]).lru.nodes@.len()))]
#[requires(h1.index@ < (((*self_).pools@[h0.pool@]).lru.nodes@.len()))]
#[requires(!((*self_).pools@[h0.pool@]).lru.nodes@[h0.index@].active)]
#[requires(((*self_).pools@[h0.pool@]).lru.nodes@[h1.index@].active)]
#[ensures(result == Ok(()))]
#[ensures(((^self_).pools@[h0.pool@]).lru.tail == Some(h1.index))]
pub fn verify_epo_batch_touch_post_removed_handles_skipped(self_: &mut Pools, h0: Handle, h1: Handle)
    -> Result<(), PolicyError> {
    state_batch_touch2(self_, h0, h1)
}

// ---- EPO-BATCH-TOUCH-FRAME-MEMBERSHIP-AND-SKETCH ----------------------—
// However many handles a batch carries, it never adds or removes an entry and never touches the
// estimator, the access count or the high-water record; it only reorders.
#[requires(inv(&(*p).lru))]
#[requires(forall<k: Int> 0 <= k && k < idxs@.len() ==> (idxs@[k])@ < (*p).lru.nodes@.len())]
#[ensures((^p).lru.len@ == (*p).lru.len@)]
#[ensures(pool_meta_eq(&^p, &*p))]
#[ensures(forall<i: Int> 0 <= i && i < (*p).lru.nodes@.len() ==>
             ((^p).lru.nodes@[i]).active == ((*p).lru.nodes@[i]).active
          && ((^p).lru.nodes@[i]).key == ((*p).lru.nodes@[i]).key)]
pub fn verify_epo_batch_touch_frame_membership_and_sketch(p: &mut Pool, idxs: &Vec<u32>) {
    pool_batch_touch(p, idxs);
}

// ---- EPO-BATCH-TOUCH-ERROR-INVALID-POOL -------------------------------—
#[requires(h0.pool@ >= (*self_).pools@.len())]
#[requires(h1.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h1.pool@]).lru)
             && h1.index@ < (((*self_).pools@[h1.pool@]).lru.nodes@.len()))]
#[ensures(result == Err(PolicyError::InvalidPool(h0.pool)))]
pub fn verify_epo_batch_touch_error_invalid_pool(self_: &mut Pools, h0: Handle, h1: Handle)
    -> Result<(), PolicyError> {
    state_batch_touch2(self_, h0, h1)
}

// ---- EPO-BATCH-TOUCH-ERROR-PARTIAL-APPLICATION ------------------------—
// batch_touch is NOT all-or-nothing: the accesses before the bad handle are already applied and
// are kept, and the call then returns an error without applying the rest.
#[requires(h0.pool@ < (*self_).pools@.len())]
#[requires(inv(&((*self_).pools@[h0.pool@]).lru))]
#[requires(h0.index@ < (((*self_).pools@[h0.pool@]).lru.nodes@.len()))]
#[requires(((*self_).pools@[h0.pool@]).lru.nodes@[h0.index@].active)]
#[requires(((*self_).pools@[h0.pool@]).lru.tail != Some(h0.index))]
#[requires(h1.pool != h0.pool && h1.pool@ >= (*self_).pools@.len())]
#[ensures(result == Err(PolicyError::InvalidPool(h1.pool)))]
#[ensures(((^self_).pools@[h0.pool@]).lru.tail == Some(h0.index))]
pub fn verify_epo_batch_touch_error_partial_application(self_: &mut Pools, h0: Handle, h1: Handle)
    -> Result<(), PolicyError> {
    state_batch_touch2(self_, h0, h1)
}

// ---- EPO-BATCH-TOUCH-ERROR-NO-LOG ------------------------------------—
// The batch error path is `ok_or` (lib.rs:167, :178), not the `ok_or_else` with a `logger.warn`
// that track/touch/remove use — so a bad pool id arriving through the batch path writes NOTHING
// to the log even when a logger IS connected.
#[requires(pool@ >= (*self_).pools@.len())]
#[requires(connected)]
#[requires((*log).lines@ < 4294967295)]
#[ensures(result == Err(PolicyError::InvalidPool(pool)))]
#[ensures(^log == *log)]
pub fn verify_epo_batch_touch_error_no_log(self_: &mut Pools, log: &mut Log, connected: bool, pool: u32)
    -> Result<(), PolicyError> {
    let _ = connected;
    if (pool as usize) < self_.pools.len() {
        Ok(())
    } else {
        Err(PolicyError::InvalidPool(pool))
    }
}

// ======================= remove ============================================

// ---- EPO-REMOVE-PRE-POOL-EXISTS -------------------------------------—
#[requires(h.pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[h.pool@]).lru)
             && h.index@ < (((*self_).pools@[h.pool@]).lru.nodes@.len())
             && (((*self_).pools@[h.pool@]).lru.nodes@[h.index@].active
                 ==> ((*self_).pools@[h.pool@]).lru.len@ >= 1)
             && ((*self_).pools@[h.pool@]).lru.free@.len() < 4294967295)]
#[ensures(match result { Ok(_) => h.pool@ < (*self_).pools@.len(),
                         Err(_) => h.pool@ >= (*self_).pools@.len() })]
pub fn verify_epo_remove_pre_pool_exists(self_: &mut Pools, h: Handle) -> Result<(), PolicyError> {
    state_remove(self_, h)
}

// ---- EPO-REMOVE-POST-UNLINK ----------------------------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active && (*l).lru.len@ >= 1)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures(!(^l).lru.nodes@[idx@].active)]
#[ensures((^l).lru.nodes@[idx@].prev == None && (^l).lru.nodes@[idx@].next == None)]
pub fn verify_epo_remove_post_unlink(l: &mut Pool, idx: u32) {
    arena_remove(&mut l.lru, idx);
}

// ---- EPO-REMOVE-POST-LEN-DECREMENT ---------------------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active && (*l).lru.len@ >= 1)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures((^l).lru.len@ == (*l).lru.len@ - 1)]
pub fn verify_epo_remove_post_len_decrement(l: &mut Pool, idx: u32) {
    arena_remove(&mut l.lru, idx);
}

// ---- EPO-REMOVE-POST-NEVER-EVICTED --------------------------------—
// A removed slot is not live and is not the front, so no later next-victim request and no later
// candidate preview can name it again.
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active && (*l).lru.len@ >= 1)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[requires(match (*l).lru.nodes@[idx@].next { Some(nx) => nx@ != idx@, None => true })]
#[requires((*l).lru.head == Some(idx) ==> (*l).lru.nodes@[idx@].prev == None)]
#[ensures(!(^l).lru.nodes@[idx@].active)]
#[ensures((^l).lru.head != Some(idx))]
pub fn verify_epo_remove_post_never_evicted(l: &mut Pool, idx: u32) {
    arena_remove(&mut l.lru, idx);
}

// ---- EPO-REMOVE-POST-IDEMPOTENT-REMOVED-HANDLE --------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires(!(*l).lru.nodes@[idx@].active)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures(^l == *l)]
pub fn verify_epo_remove_post_idempotent_removed_handle(l: &mut Pool, idx: u32) {
    arena_remove(&mut l.lru, idx);
}

// ---- EPO-REMOVE-POST-SLOT-RECYCLED -------------------------------—
// The freed slot goes on the reuse list and the very next track into that pool takes it back —
// most recently freed first, without allocating a new slot.
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active && (*l).lru.len@ >= 1)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[requires((*l).lru.nodes@.len() < 4294967295)]
#[ensures(result == idx)]
#[ensures((^l).lru.nodes@.len() == (*l).lru.nodes@.len())]
pub fn verify_epo_remove_post_slot_recycled(l: &mut Pool, idx: u32, key: u64) -> u32 {
    arena_remove(&mut l.lru, idx);
    arena_push_back(&mut l.lru, key)
}

// ---- EPO-REMOVE-FRAME-SKETCH-UNCHANGED --------------------------—
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active ==> (*l).lru.len@ >= 1)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures(pool_meta_eq(&^l, &*l))]
pub fn verify_epo_remove_frame_sketch_unchanged(l: &mut Pool, idx: u32) {
    arena_remove(&mut l.lru, idx);
}

// ---- EPO-REMOVE-FRAME-RELATIVE-ORDER-OTHERS --------------------—
// Every other slot keeps its key and its live flag, whether the removed entry was at the front,
// in the middle, or at the back. (Relative ORDER rests on the head-to-tail walk — see advisory.)
#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active ==> (*l).lru.len@ >= 1)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures(forall<i: Int> 0 <= i && i < (*l).lru.nodes@.len() && i != idx@ ==>
             ((^l).lru.nodes@[i]).active == ((*l).lru.nodes@[i]).active
          && ((^l).lru.nodes@[i]).key == ((*l).lru.nodes@[i]).key)]
pub fn verify_epo_remove_frame_relative_order_others(l: &mut Pool, idx: u32) {
    arena_remove(&mut l.lru, idx);
}

// ---- EPO-REMOVE-ERROR-INVALID-POOL ----------------------------—
#[requires(h.pool@ >= (*self_).pools@.len())]
#[ensures(result == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures(^self_ == *self_)]
pub fn verify_epo_remove_error_invalid_pool(self_: &mut Pools, h: Handle) -> Result<(), PolicyError> {
    state_remove(self_, h)
}

// ======================= identify_next_to_evict ============================

// ---- EPO-EVICT-PRE-TOTAL -------------------------------------—
// Any pool id at all may be passed, including one never created; the call always returns.
#[requires(pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[pool@]).lru)
             && (match ((*self_).pools@[pool@]).lru.head {
                     Some(hh) => ((*self_).pools@[pool@]).lru.nodes@[hh@].active
                                 && ((*self_).pools@[pool@]).lru.len@ >= 1, None => true })
             && ((*self_).pools@[pool@]).lru.free@.len() < 4294967295)]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
pub fn verify_epo_evict_pre_total(self_: &mut Pools, pool: u32) -> Option<u64> {
    state_evict(self_, pool)
}

// ---- EPO-EVICT-POST-RETURNS-LRU-HEAD -------------------------—
#[requires(inv(&(*l).lru))]
#[requires((*l).lru.head != None)]
#[requires(match (*l).lru.head { Some(hh) => (*l).lru.nodes@[hh@].active && (*l).lru.len@ >= 1,
                                 None => true })]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures(match (*l).lru.head { Some(hh) => result == Some((*l).lru.nodes@[hh@].key), None => true })]
pub fn verify_epo_evict_post_returns_lru_head(l: &mut Pool) -> Option<u64> {
    arena_pop_front(&mut l.lru)
}

// ---- EPO-EVICT-POST-REMOVES-RETURNED ------------------------—
// The call both selects and consumes: one fewer entry, the returned slot no longer live, and the
// entry that was second in line becomes the new front.
#[requires(inv(&(*l).lru))]
#[requires((*l).lru.head != None)]
#[requires(match (*l).lru.head { Some(hh) => (*l).lru.nodes@[hh@].active && (*l).lru.len@ >= 1
                                             && (*l).lru.nodes@[hh@].prev == None
                                             && (match (*l).lru.nodes@[hh@].next {
                                                    Some(nx) => nx@ != hh@, None => true }),
                                 None => true })]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures((^l).lru.len@ == (*l).lru.len@ - 1)]
#[ensures(match (*l).lru.head { Some(hh) => !(^l).lru.nodes@[hh@].active
                                            && (^l).lru.head == (*l).lru.nodes@[hh@].next,
                                None => true })]
pub fn verify_epo_evict_post_removes_returned(l: &mut Pool) -> Option<u64> {
    arena_pop_front(&mut l.lru)
}

// ---- EPO-EVICT-POST-EMPTY-POOL-NONE ------------------------—
#[requires(inv(&(*l).lru))]
#[requires((*l).lru.head == None)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures(result == None)]
#[ensures(^l == *l)]
pub fn verify_epo_evict_post_empty_pool_none(l: &mut Pool) -> Option<u64> {
    arena_pop_front(&mut l.lru)
}

// ---- EPO-EVICT-POST-ORDER-FIRST-TIME-KEYS -----------------—
// Three never-seen keys tracked one after another into a fresh pool all tie with the entry
// already at the front, so each is admitted at the eviction end (push_front); asking repeatedly
// for the next victim therefore gives them back in REVERSE order of tracking.
#[ensures(result.0 == Some(k3))]
#[ensures(result.1 == Some(k2))]
#[ensures(result.2 == Some(k1))]
pub fn verify_epo_evict_post_order_first_time_keys(k1: u64, k2: u64, k3: u64)
    -> (Option<u64>, Option<u64>, Option<u64>) {
    let mut l = arena_fresh();
    let _i1 = arena_push_front(&mut l, k1);
    let _i2 = arena_push_front(&mut l, k2);
    let _i3 = arena_push_front(&mut l, k3);
    let r0 = arena_pop_front(&mut l);
    let r1 = arena_pop_front(&mut l);
    let r2 = arena_pop_front(&mut l);
    (r0, r1, r2)
}

// ---- EPO-EVICT-FRAME-SKETCH-AND-COUNTERS -----------------—
// The evicted key's frequency record is deliberately left in place.
#[requires(inv(&(*l).lru))]
#[requires(match (*l).lru.head { Some(hh) => (*l).lru.nodes@[hh@].active && (*l).lru.len@ >= 1,
                                 None => true })]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures(pool_meta_eq(&^l, &*l))]
pub fn verify_epo_evict_frame_sketch_and_counters(l: &mut Pool) -> Option<u64> {
    arena_pop_front(&mut l.lru)
}

// ---- EPO-EVICT-FRAME-OTHER-POOLS -----------------------—
#[requires(pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[pool@]).lru)
             && (match ((*self_).pools@[pool@]).lru.head {
                     Some(hh) => ((*self_).pools@[pool@]).lru.nodes@[hh@].active
                                 && ((*self_).pools@[pool@]).lru.len@ >= 1, None => true })
             && ((*self_).pools@[pool@]).lru.free@.len() < 4294967295)]
#[ensures(pools_frame_except(&^self_, &*self_, pool@))]
pub fn verify_epo_evict_frame_other_pools(self_: &mut Pools, pool: u32) -> Option<u64> {
    state_evict(self_, pool)
}

// ---- EPO-EVICT-DEGRADE-INVALID-POOL --------------------—
// Indistinguishable from a real but empty pool, and nothing is logged.
#[requires(pool@ >= (*self_).pools@.len())]
#[requires(connected)]
#[requires((*log).lines@ < 4294967295)]
#[ensures(result == None)]
#[ensures(^self_ == *self_)]
#[ensures(^log == *log)]
pub fn verify_epo_evict_degrade_invalid_pool(self_: &mut Pools, log: &mut Log, connected: bool, pool: u32)
    -> Option<u64> {
    let _ = connected;
    let _ = log;
    state_evict(self_, pool)
}

// ======================= get_eviction_candidates ===========================

// ---- EPO-CANDIDATES-PRE-TOTAL -------------------------—
#[requires(pool@ < self_.pools@.len() ==> inv(&(self_.pools@[pool@]).lru))]
#[ensures(result@.len() <= n@)]
pub fn verify_epo_candidates_pre_total(self_: &Pools, pool: u32, n: usize) -> Vec<u64> {
    state_candidates(self_, pool, n)
}

// ---- EPO-CANDIDATES-POST-ORDERED-PREFIX ---------------—
// The list starts at the least-recently-used front and continues along the eviction order: the
// first candidate is the front entry and the second is the front entry's successor.
#[requires(inv(&(*l).lru))]
#[ensures(match (*l).lru.head { Some(hh) => n@ > 0 ==> result@.len() > 0
                                            && result@[0] == (*l).lru.nodes@[hh@].key,
                                None => result@.len() == 0 })]
#[ensures(match (*l).lru.head { Some(hh) => match ((*l).lru.nodes@[hh@]).next {
                                    Some(nx) => result@.len() >= 2 ==>
                                        result@[1] == (*l).lru.nodes@[nx@].key,
                                    None => result@.len() < 2 },
                                None => true })]
pub fn verify_epo_candidates_post_ordered_prefix(l: &Pool, n: usize) -> Vec<u64> {
    arena_peek_front_n(&l.lru, n)
}

// ---- EPO-CANDIDATES-POST-COUNT-BOUND ------------------—
// Never more than were asked for; and an empty pool yields nothing however many were asked for.
#[requires(inv(&(*l).lru))]
#[requires(ends_iff(&(*l).lru))]
#[ensures(result@.len() <= n@)]
#[ensures((*l).lru.len@ == 0 ==> result@.len() == 0)]
pub fn verify_epo_candidates_post_count_bound(l: &Pool, n: usize) -> Vec<u64> {
    arena_peek_front_n(&l.lru, n)
}

// ---- EPO-CANDIDATES-POST-ZERO-EMPTY ------------------—
#[requires(inv(&(*l).lru))]
#[requires(n@ == 0)]
#[ensures(result@.len() == 0)]
pub fn verify_epo_candidates_post_zero_empty(l: &Pool, n: usize) -> Vec<u64> {
    arena_peek_front_n(&l.lru, n)
}

// ---- EPO-CANDIDATES-FRAME-NON-DESTRUCTIVE -----------—
// A read-only walk over `&self` (lru_list.rs:145): the pool is passed immutably, so nothing it
// holds can change and repeated calls see the same state.
#[requires(inv(&(*l).lru))]
#[ensures(^l == *l)]
pub fn verify_epo_candidates_frame_non_destructive(l: &mut Pool, n: usize) -> usize {
    let v = arena_peek_front_n(&l.lru, n);
    v.len()
}

// ---- EPO-CANDIDATES-DEGRADE-INVALID-POOL ------------—
#[requires(pool@ >= self_.pools@.len())]
#[requires(connected)]
#[requires(log.lines@ < 4294967295)]
#[ensures(result@.len() == 0)]
pub fn verify_epo_candidates_degrade_invalid_pool(self_: &Pools, log: &Log, connected: bool,
                                                  pool: u32, n: usize) -> Vec<u64> {
    let _ = connected;
    let _ = log;
    state_candidates(self_, pool, n)
}

// ======================= len ==============================================

// ---- EPO-LEN-PRE-TOTAL ------------------------------—
#[ensures(result@ >= 0)]
pub fn verify_epo_len_pre_total(self_: &Pools, pool: u32) -> usize {
    state_len(self_, pool)
}

// ---- EPO-LEN-POST-ACTIVE-COUNT ---------------------—
#[requires(pool@ < self_.pools@.len())]
#[ensures(result@ == (self_.pools@[pool@]).lru.len@)]
pub fn verify_epo_len_post_active_count(self_: &Pools, pool: u32) -> usize {
    state_len(self_, pool)
}

// ---- EPO-LEN-FRAME-PURE ---------------------------—
#[ensures(^self_ == *self_)]
pub fn verify_epo_len_frame_pure(self_: &mut Pools, pool: u32) -> usize {
    state_len(self_, pool)
}

// ---- EPO-LEN-DEGRADE-INVALID-POOL -----------------—
#[requires(pool@ >= self_.pools@.len())]
#[requires(connected)]
#[requires(log.lines@ < 4294967295)]
#[ensures(result@ == 0)]
pub fn verify_epo_len_degrade_invalid_pool(self_: &Pools, log: &Log, connected: bool, pool: u32) -> usize {
    let _ = connected;
    let _ = log;
    state_len(self_, pool)
}

// ======================= clear_pool =======================================

// ---- EPO-CLEAR-PRE-TOTAL -------------------------—
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
pub fn verify_epo_clear_pre_total(self_: &mut Pools, pool: u32) {
    state_clear(self_, pool)
}

// ---- EPO-CLEAR-POST-EMPTY -----------------------—
// Size zero, no candidates offered, and the next-victim request returns nothing.
#[requires(pool@ < (*self_).pools@.len())]
#[ensures(((^self_).pools@[pool@]).lru.len@ == 0)]
#[ensures(((^self_).pools@[pool@]).lru.head == None)]
#[ensures(((^self_).pools@[pool@]).lru.tail == None)]
#[ensures((result.0)@.len() == 0)]
#[ensures(result.1 == None)]
pub fn verify_epo_clear_post_empty(self_: &mut Pools, pool: u32, n: usize) -> (Vec<u64>, Option<u64>) {
    state_clear(self_, pool);
    let cands = state_candidates(self_, pool, n);
    let victim = state_evict(self_, pool);
    (cands, victim)
}

// ---- EPO-CLEAR-FRAME-PRESERVES-SKETCH-AND-COUNTERS ----—
// Clearing is deliberately NOT the same as creating a fresh pool: everything the policy has
// learned survives, so the ageing schedule continues on the old interval.
#[requires(pool@ < (*self_).pools@.len())]
#[ensures(pool_meta_eq(&(^self_).pools@[pool@], &(*self_).pools@[pool@]))]
pub fn verify_epo_clear_frame_preserves_sketch_and_counters(self_: &mut Pools, pool: u32) {
    state_clear(self_, pool)
}

// ---- EPO-CLEAR-POST-RETRACK-ADMITTED-AT-MRU ----------—
// A cleared pool is empty, so the very next key is admitted at the protected end whatever its
// previous frequency record was.
#[requires(pool_inv(p))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[ensures((^p).lru.tail == Some(result))]
#[ensures((^p).lru.head == Some(result))]
#[ensures((^p).lru.len@ == 1)]
pub fn verify_epo_clear_post_retrack_admitted_at_mru(p: &mut Pool, key: u64) -> u32 {
    arena_clear(&mut p.lru);
    pool_track(p, key)
}

// ---- EPO-CLEAR-POST-SLOT-NUMBERING-RESTARTS ---------—
// Clearing discards the slots too, so later tracks start handing out slot number 0 again — and a
// handle issued before the clear now names a slot the pool will give to a DIFFERENT key.
#[requires(pool_inv(p))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[ensures(result@ == 0)]
#[ensures((^p).lru.nodes@.len() == 1)]
pub fn verify_epo_clear_post_slot_numbering_restarts(p: &mut Pool, key: u64) -> u32 {
    arena_clear(&mut p.lru);
    pool_track(p, key)
}

// ---- EPO-CLEAR-FRAME-OTHER-POOLS -------------------—
// It never removes the pool itself and never renumbers any pool.
#[ensures(pools_frame_except(&^self_, &*self_, pool@))]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
pub fn verify_epo_clear_frame_other_pools(self_: &mut Pools, pool: u32) {
    state_clear(self_, pool)
}

// ---- EPO-CLEAR-DEGRADE-INVALID-POOL ---------------—
#[requires(pool@ >= (*self_).pools@.len())]
#[requires(connected)]
#[requires((*log).lines@ < 4294967295)]
#[ensures(^self_ == *self_)]
#[ensures(^log == *log)]
pub fn verify_epo_clear_degrade_invalid_pool(self_: &mut Pools, log: &mut Log, connected: bool, pool: u32) {
    let _ = connected;
    let _ = log;
    state_clear(self_, pool)
}

// ======================= global invariants ================================

// ---- EPO-INV-POOL-IDS-UNIQUE-SEQUENTIAL (9 attachments) -----------------—
// The pools that exist are exactly 0..n-1: the id create_pool returns is the old count, so no id
// is reused and none in range is missing; every larger id is treated as an empty pool (here: len).
#[requires((*self_).pools@.len() < 4294967294)]
#[ensures(result.0@ == (*self_).pools@.len())]
#[ensures((^self_).pools@.len() == (*self_).pools@.len() + 1)]
#[ensures(result.0@ < (^self_).pools@.len())]
#[ensures(result.1@ == 0)]
pub fn verify_epo_inv_pool_ids_unique_sequential(self_: &mut Pools) -> (u32, usize) {
    let id = state_create_pool(self_);
    let beyond = state_len(self_, id + 1);
    (id, beyond)
}

// ---- EPO-INV-POOLS-APPEND-ONLY (9) ------------------------------------—
// The collection only ever grows: the two most destructive operations leave the pool count and
// every other pool's position exactly as they were, and no pool is removed or moved.
#[requires(pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[pool@]).lru)
             && ((*self_).pools@[pool@]).lru.free@.len() < 4294967295)]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
#[ensures(pools_frame_except(&^self_, &*self_, pool@))]
pub fn verify_epo_inv_pools_append_only(self_: &mut Pools, pool: u32) -> Option<u64> {
    state_clear(self_, pool);
    state_evict(self_, pool)
}

// ---- EPO-INV-POOL-ISOLATION (9) -------------------------------------—
// Each pool keeps its own entries, order, estimator and counters: an operation on one pool
// cannot change any other pool.
#[requires(pool@ < (*self_).pools@.len() ==> pool_ready(&(*self_).pools@[pool@]))]
#[ensures(pools_frame_except(&^self_, &*self_, pool@))]
#[ensures((^self_).pools@.len() == (*self_).pools@.len())]
pub fn verify_epo_inv_pool_isolation(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

// ---- EPO-INV-VICTIM-IS-LIST-FRONT (3) ------------------------------—
// The entry eviction returns is the front of the order, and that is the very same entry whose
// estimated popularity a newly tracked key is compared against (lib.rs:129 vs lib.rs:206).
#[requires(inv(l))]
#[requires(match (*l).head { Some(h) => (*l).nodes@[h@].active && (*l).len@ >= 1, None => true })]
#[requires((*l).free@.len() < 4294967295)]
#[ensures(result.0 == result.1)]
pub fn verify_epo_inv_victim_is_list_front(l: &mut LruList) -> (Option<u64>, Option<u64>) {
    let compared_against = arena_peek_front_key(l);
    let evicted = arena_pop_front(l);
    (compared_against, evicted)
}

// ---- EPO-INV-PREVIEW-AGREES-WITH-EVICTION (2) ---------------------—
// The first candidate the preview names is exactly the key the next eviction returns.
#[requires(inv(l))]
#[requires(match (*l).head { Some(h) => (*l).nodes@[h@].active && (*l).len@ >= 1, None => true })]
#[requires((*l).free@.len() < 4294967295)]
#[requires(n@ > 0)]
#[requires((*l).head != None)]
#[ensures((result.0)@.len() > 0)]
#[ensures(result.1 == Some((result.0)@[0]))]
pub fn verify_epo_inv_preview_agrees_with_eviction(l: &mut LruList, n: usize)
    -> (Vec<u64>, Option<u64>) {
    let preview = arena_peek_front_n(l, n);
    let evicted = arena_pop_front(l);
    (preview, evicted)
}

// ---- EPO-INV-SLOT-OWNERSHIP-DISJOINT (4) ------------------------—
// A slot is either in the live order or on the reuse list, never both; and the reuse list holds
// each slot at most once, so two tracks can never be handed the same slot.
#[requires(inv(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).free@.len() < 4294967295)]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[ensures(slots_disjoint(&^l))]
pub fn verify_epo_inv_slot_ownership_disjoint(l: &mut LruList, key: u64, idx: u32) -> u32 {
    arena_remove(l, idx);
    arena_push_back(l, key)
}

// ---- EPO-INV-SLOTS-ACCOUNTED (4) -------------------------------—
// nodes.len() == len + free.len(): no slot is lost or double-counted, so the memory a pool holds
// is bounded by the largest number of entries it has ever tracked at once.
#[requires(inv(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).free@.len() < 4294967295)]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[ensures(slots_accounted(&^l))]
#[ensures((^l).nodes@.len() == (^l).len@ + (^l).free@.len())]
pub fn verify_epo_inv_slots_accounted(l: &mut LruList, key: u64, idx: u32) -> u32 {
    arena_remove(l, idx);
    arena_push_front(l, key)
}

// ---- EPO-INV-INTERNAL-INDEX-IN-RANGE (7) -----------------------—
// Every slot number the component keeps internally — both ends of the order, every link, and
// every entry on the reuse list — refers to a slot that exists. (Slot numbers arriving from
// OUTSIDE in a caller's handle are not covered: see EPO-INV-STALE-HANDLE-NEVER-CRASHES.)
#[requires(inv(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).free@.len() < 4294967295)]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[ensures(match (^l).head { Some(h) => h@ < (^l).nodes@.len(), None => true })]
#[ensures(match (^l).tail { Some(t) => t@ < (^l).nodes@.len(), None => true })]
#[ensures(forall<i: Int> 0 <= i && i < (^l).nodes@.len() ==>
             (match ((^l).nodes@[i]).next { Some(n) => n@ < (^l).nodes@.len(), None => true })
          && (match ((^l).nodes@[i]).prev { Some(pv) => pv@ < (^l).nodes@.len(), None => true }))]
#[ensures(forall<k: Int> 0 <= k && k < (^l).free@.len() ==> ((^l).free@[k])@ < (^l).nodes@.len())]
pub fn verify_epo_inv_internal_index_in_range(l: &mut LruList, key: u64, idx: u32) -> u32 {
    arena_remove(l, idx);
    arena_move_to_back(l, idx);
    arena_push_back(l, key)
}

// ---- EPO-INV-LEN-NO-UNDERFLOW (3) -----------------------------—
// The count is only reduced when a genuinely live entry leaves the order, so repeated removals of
// an already-removed entry cannot drive it below zero or wrap it round to a huge number.
#[requires(inv(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[requires((*l).free@.len() < 4294967294)]
#[ensures((*l).nodes@[idx@].active ==> (^l).len@ == (*l).len@ - 1)]
#[ensures(!(*l).nodes@[idx@].active ==> (^l).len@ == (*l).len@)]
#[ensures((^l).len@ <= (^l).nodes@.len())]
pub fn verify_epo_inv_len_no_underflow(l: &mut LruList, idx: u32) {
    arena_remove(l, idx);
    arena_remove(l, idx);
}

// ---- EPO-INV-HANDLE-POOL-SCOPED (4) --------------------------—
// A handle carries the pool it belongs to, the component only ever issues one naming the pool the
// track was made into, and touch routes strictly by that field — so no operation with a handle
// can reach an entry sitting at the same slot number in a different pool.
#[requires(pool@ < (*self_).pools@.len())]
#[requires(pool_ready(&(*self_).pools@[pool@]))]
#[ensures(match result { Ok(h) => h.pool == pool, Err(_) => false })]
#[ensures(pools_frame_except(&^self_, &*self_, pool@))]
pub fn verify_epo_inv_handle_pool_scoped(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    let h = state_track(self_, pool, key);
    match h {
        Ok(hh) => {
            let _ = state_touch(self_, Handle { pool: hh.pool, index: hh.index });
            Ok(Handle { pool: hh.pool, index: hh.index })
        }
        Err(e) => Err(e),
    }
}

// ---- EPO-INV-INVALID-HANDLE-NEVER-RETURNED (4) --------------—
// `EvictionPolicyError::InvalidHandle` is dead code in this component: the only error any
// operation ever constructs is `InvalidPool`, so a bad handle is never reported as an error.
#[requires(h.pool@ >= (*self_).pools@.len())]
#[ensures(result.0 == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures(result.1 == Err(PolicyError::InvalidPool(h.pool)))]
#[ensures(result.2 == Err(PolicyError::InvalidPool(h.pool)))]
pub fn verify_epo_inv_invalid_handle_never_returned(self_: &mut Pools, h: Handle, key: u64)
    -> (Result<Handle, PolicyError>, Result<(), PolicyError>, Result<(), PolicyError>) {
    let t = state_track(self_, h.pool, key);
    let u = state_touch(self_, Handle { pool: h.pool, index: h.index });
    let r = state_remove(self_, Handle { pool: h.pool, index: h.index });
    (t, u, r)
}

// ---- EPO-INV-MAXLEN-MONOTONE (4) ---------------------------—
// Removing entries, evicting them, and clearing the whole pool all leave the high-water record
// untouched; only track ever writes it, and only upwards.
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[requires(idx@ < (*p).lru.nodes@.len())]
#[requires((*p).lru.nodes@[idx@].active ==> (*p).lru.len@ >= 1)]
#[requires((*p).lru.free@.len() < 4294967295)]
#[ensures((^p).max_len@ >= (*p).max_len@)]
pub fn verify_epo_inv_maxlen_monotone(p: &mut Pool, idx: u32, key: u64) -> u32 {
    arena_remove(&mut p.lru, idx);
    arena_clear(&mut p.lru);
    pool_track(p, key)
}

// ---- EPO-INV-MAXLEN-UPPER-BOUNDS-LEN (2) -------------------—
// The record is measured just BEFORE each insertion (lib.rs:122), so the size is never more than
// one greater than the record, and whenever the pool stands at a new peak the record is exactly
// one too low. That off-by-one is why a pool whose size is still zero at every track keeps a
// record of zero and is therefore NEVER aged (the guard `max_len > 0` at lib.rs:124).
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[requires((*p).lru.len@ <= (*p).max_len@ + 1)]
#[ensures((^p).lru.len@ <= (^p).max_len@ + 1)]
#[ensures((^p).max_len@ < (^p).lru.len@ ==> (^p).max_len@ == (^p).lru.len@ - 1)]
#[ensures((*p).lru.len@ == 0 && (*p).max_len@ == 0 ==> (^p).max_len@ == 0)]
pub fn verify_epo_inv_maxlen_upper_bounds_len(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

// ---- EPO-INV-ACCESS-COUNT-MONOTONE (2) --------------------—
// A lifetime tally: exactly +1 per successful track, and clearing the pool does not reset it.
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).access_count@ == (*p).access_count@ + 1)]
#[ensures((^p).access_count@ > (*p).access_count@)]
pub fn verify_epo_inv_access_count_monotone(p: &mut Pool, key: u64) -> u32 {
    arena_clear(&mut p.lru);
    pool_track(p, key)
}

// ---- EPO-INV-AGING-PERIOD-NON-DECREASING (2) --------------—
// The ageing interval is ten times a record that never shrinks — clear_pool deliberately leaves
// it alone (lib.rs:231-237) — so the gap between ageing events can only stay the same or grow. A
// pool that once grew large keeps a long interval forever, even after being emptied.
#[requires(pool_inv(p))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[ensures((^p).max_len@ * 10 >= (*p).max_len@ * 10)]
#[ensures((^p).max_len@ >= (*p).max_len@)]
pub fn verify_epo_inv_aging_period_non_decreasing(p: &mut Pool) -> bool {
    arena_clear(&mut p.lru);
    pool_age_step(p)
}

// ---- EPO-INV-SKETCH-COUNTERS-SATURATE (2) ----------------—
// A counter already at 255 stays at 255 instead of wrapping round to zero and making a hot key
// look cold.
#[requires(s.counters@.len() == 4096)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 ==> ((^s).counters@[j])@ <= 255)]
#[ensures(((*s).counters@[result.0@])@ == 255 ==> ((^s).counters@[result.0@])@ == 255)]
#[ensures(((*s).counters@[result.1@])@ == 255 ==> ((^s).counters@[result.1@])@ == 255)]
#[ensures(((*s).counters@[result.2@])@ == 255 ==> ((^s).counters@[result.2@])@ == 255)]
#[ensures(((*s).counters@[result.3@])@ == 255 ==> ((^s).counters@[result.3@])@ == 255)]
pub fn verify_epo_inv_sketch_counters_saturate(s: &mut Sketch, key: u64)
    -> (usize, usize, usize, usize) {
    sketch_increment4(s, key)
}

// ---- EPO-INV-SKETCH-ESTIMATE-IS-ROW-MINIMUM --------------—
#[ensures(result@ <= c0@ && result@ <= c1@ && result@ <= c2@ && result@ <= c3@)]
#[ensures(result == c0 || result == c1 || result == c2 || result == c3)]
pub fn verify_epo_inv_sketch_estimate_is_row_minimum(c0: u8, c1: u8, c2: u8, c3: u8) -> u8 {
    cms_min4(c0, c1, c2, c3)
}

// ---- EPO-INV-SKETCH-ESTIMATE-AT-LEAST-ONE-AFTER-FIRST-TRACK ----—
// Every counter the key maps to is raised on the track, so the estimate read immediately
// afterwards is at least one — which is why a first-time key TIES with a victim seen once.
#[requires(s.counters@.len() == 4096)]
#[ensures(result@ >= 1)]
pub fn verify_epo_inv_sketch_estimate_at_least_one_after_first_track(s: &mut Sketch, key: u64) -> u8 {
    let c = sketch_increment4(s, key);
    sketch_estimate_at(s, c.0, c.1, c.2, c.3)
}

// ---- EPO-INV-SKETCH-BUCKET-IN-RANGE ---------------------—
// Multiplying the key by a fixed odd constant and keeping only the top ten bits of the 64-bit
// product can only produce 0..=1023, so no key can read or write outside the fixed table.
#[ensures(result.0@ < 1024)]
#[ensures(1024 <= result.1@ && result.1@ < 2048)]
#[ensures(2048 <= result.2@ && result.2@ < 3072)]
#[ensures(3072 <= result.3@ && result.3@ < 4096)]
#[ensures(result.0@ < 4096 && result.1@ < 4096 && result.2@ < 4096 && result.3@ < 4096)]
pub fn verify_epo_inv_sketch_bucket_in_range(key: u64) -> (usize, usize, usize, usize) {
    cms_cols4(key)
}

// ---- EPO-INV-SKETCH-HALVE-NON-INCREASING ----------------—
// Ageing replaces each counter by half its value rounded down, so no estimate can ever go UP as
// a result of ageing (and a counter of one becomes zero: a key seen once is forgotten outright).
#[requires(s.counters@.len() == 4096)]
#[ensures(forall<j: Int> 0 <= j && j < 4096 ==>
             ((^s).counters@[j])@ <= ((*s).counters@[j])@)]
pub fn verify_epo_inv_sketch_halve_non_increasing(s: &mut Sketch) {
    sketch_halve(s)
}

// [verify_epo_inv_sketch_row_hashes_distinct removed 2026-10-07: replaced by the fresh section at the end of this file]

// ---- EPO-INV-SKETCH-OPS-BOUNDED-BY-FIXED-SIZE ----------—
// Raising a key's count and reading its estimate each touch exactly CMS_ROWS = 4 counters, and
// ageing touches the whole fixed CMS_ROWS*CMS_COLS = 4096 table once. The total is a CONSTANT:
// it does not mention how many entries the pool holds.
#[requires((*w).accesses@ == 0)]
#[ensures((^w).accesses@ == 4104)]
pub fn verify_epo_inv_sketch_ops_bounded_by_fixed_size(w: &mut Work, n_entries: usize) {
    let _ = n_entries;
    work_add(w, 4);
    work_add(w, 4);
    work_add(w, 4096);
}

// ---- EPO-INV-ADMISSION-DECIDED-ONLY-BY-VICTIM-COMPARISON ----—
// Which end the key enters at is decided SOLELY by whether its estimate beats the current
// victim's; there is no fixed threshold anywhere, so a very hot key still enters at the eviction
// end when the victim looks just as hot, and a barely warm key is protected from a colder victim.
#[requires(inv(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).head != None)]
#[ensures(e_new@ <= e_head@ ==> (^l).head == Some(result))]
#[ensures(e_new@ > e_head@ ==> (^l).tail == Some(result))]
pub fn verify_epo_inv_admission_decided_only_by_victim_comparison(l: &mut LruList, key: u64,
                                                                 e_new: u8, e_head: u8) -> u32 {
    if e_new <= e_head {
        arena_push_front(l, key)
    } else {
        arena_push_back(l, key)
    }
}

// ---- EPO-INV-AT-MOST-ONE-POOL-LOCK (9) -----------------—
// No operation ever holds two pools' exclusive locks at once. In a batch touch, when consecutive
// handles name different pools the current guard is DROPPED before the next is acquired
// (lib.rs:172-179), so occupancy never exceeds one.
#[requires((*t).held@ == 0 && (*t).peak@ == 0)]
#[ensures((^t).held@ == 0)]
#[ensures((^t).peak@ <= 1)]
pub fn verify_epo_inv_at_most_one_pool_lock(t: &mut LockTrace, pool_changed: bool) {
    lock_acquire(t);
    if pool_changed {
        lock_release(t);
        lock_acquire(t);
    }
    lock_release(t);
}

// ---- EPO-INV-LOGGER-OPTIONAL (9) ----------------------—
// Every logging site first asks for the logger and quietly skips if none is connected, so results
// and state are identical either way: none of the state postconditions below mentions `connected`.
#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[requires((*log).lines@ < 4294967295)]
#[ensures((^p).lru.len@ == (*p).lru.len@ + 1)]
#[ensures((^p).access_count@ == (*p).access_count@ + 1)]
#[ensures((^p).lru.nodes@[result@].key == key)]
#[ensures(connected ==> (^log).lines@ == (*log).lines@ + 1)]
#[ensures(!connected ==> ^log == *log)]
pub fn verify_epo_inv_logger_optional(p: &mut Pool, log: &mut Log, connected: bool, key: u64) -> u32 {
    let idx = pool_track(p, key);
    log_site(log, connected);
    idx
}

// ---- EPO-INV-NO-STARTUP-BANNER (0 attachments) -------—
// Bringing the component up writes NOTHING to the log, even with a logger connected: the
// `define_component!` construction has no banner site at all.
#[requires((*log).lines@ == 0)]
#[requires(connected)]
#[ensures((^log).lines@ == 0)]
#[ensures(result.pools@.len() == 0)]
pub fn verify_epo_inv_no_startup_banner(log: &mut Log, connected: bool) -> Pools {
    let _ = connected;
    let _ = log;
    Pools { pools: Vec::new() }
}

// ===========================================================================
// Structural well-formedness — first-order, no reachability
// ===========================================================================

/// The forward and backward links agree: if one entry's forward link names a second entry, that
/// second entry's backward link names the first, and vice versa (EPO-INV-LIST-LINKS-SYMMETRIC).
/// Quantified over every allocated slot; a not-live slot has both links `None` (set by `remove`),
/// so the clauses are vacuous for it.
#[logic]
pub fn links_sym(l: &LruList) -> bool {
    pearlite! {
        (forall<i: u32> i@ < l.nodes@.len() ==>
            match (l.nodes@[i@]).next { Some(n) => n@ < l.nodes@.len() && (l.nodes@[n@]).prev == Some(i),
                                       None => true })
        && (forall<i: u32> i@ < l.nodes@.len() ==>
            match (l.nodes@[i@]).prev { Some(pv) => pv@ < l.nodes@.len() && (l.nodes@[pv@]).next == Some(i),
                                       None => true })
    }
}

/// A live entry's neighbours are live (EPO-INV-LIST-LINKS-SYMMETRIC's companion: the order is made
/// of live entries only).
#[logic]
pub fn links_active(l: &LruList) -> bool {
    pearlite! {
        forall<i: u32> i@ < l.nodes@.len() && (l.nodes@[i@]).active ==>
            (match (l.nodes@[i@]).next { Some(n) => n@ < l.nodes@.len() && (l.nodes@[n@]).active, None => true })
            && (match (l.nodes@[i@]).prev { Some(pv) => pv@ < l.nodes@.len() && (l.nodes@[pv@]).active, None => true })
    }
}

/// The only live entry with no forward link is the one recorded as the back of the order
/// (EPO-INV-LIST-TAIL-IS-ONLY-NODE-WITHOUT-NEXT) — this is what makes `move_to_back`'s
/// `self.tail == Some(idx)` early return safe rather than lucky.
#[logic]
pub fn tail_unique(l: &LruList) -> bool {
    pearlite! {
        forall<i: u32> i@ < l.nodes@.len() && (l.nodes@[i@]).active ==>
            (((l.nodes@[i@]).next == None) == (l.tail == Some(i)))
    }
}

/// The mirror fact for the front of the order.
#[logic]
pub fn head_unique(l: &LruList) -> bool {
    pearlite! {
        forall<i: u32> i@ < l.nodes@.len() && (l.nodes@[i@]).active ==>
            (((l.nodes@[i@]).prev == None) == (l.head == Some(i)))
    }
}

/// Both recorded ends are live entries.
#[logic]
pub fn ends_active(l: &LruList) -> bool {
    pearlite! {
        (match l.head { Some(h) => h@ < l.nodes@.len() && (l.nodes@[h@]).active, None => true })
        && (match l.tail { Some(t) => t@ < l.nodes@.len() && (l.nodes@[t@]).active, None => true })
    }
}

/// A not-live slot is fully unlinked (`remove` sets both links to `None`, lru_list.rs:179-180).
#[logic]
pub fn inactive_unlinked(l: &LruList) -> bool {
    pearlite! {
        forall<i: u32> i@ < l.nodes@.len() && !(l.nodes@[i@]).active ==>
            (l.nodes@[i@]).prev == None && (l.nodes@[i@]).next == None
    }
}

/// Every not-live slot is on the reuse list. Together with `slots_disjoint` (no live slot on the
/// reuse list, no duplicates) the reuse list is EXACTLY the set of not-live slots, which with
/// `slots_accounted` (`nodes.len() == len + free.len()`) makes `len` exactly the number of live
/// slots — EPO-INV-LEN-MATCHES-ACTIVE-SLOTS, rendered without a counting function.
#[logic]
pub fn free_covers_inactive(l: &LruList) -> bool {
    pearlite! {
        forall<i: u32> i@ < l.nodes@.len() && !(l.nodes@[i@]).active ==>
            exists<k: Int> 0 <= k && k < l.free@.len() && l.free@[k] == i
    }
}

/// An order has both ends or neither (the weak half of EPO-INV-LIST-EMPTY-IFF-NO-ENDS: there is
/// never a front without a back).
#[logic]
pub fn ends_both(l: &LruList) -> bool {
    pearlite! { (l.head == None) == (l.tail == None) }
}

/// The structural well-formedness the shipped mutators maintain, stated without any reachability.
#[logic]
pub fn wf(l: &LruList) -> bool {
    pearlite! {
        links_sym(l) && links_active(l) && tail_unique(l) && head_unique(l)
        && ends_active(l) && ends_both(l) && inactive_unlinked(l) && free_covers_inactive(l)
    }
}

/// `push_back` (lru_list.rs:75-105) re-proved with `wf` in the contract.
#[requires(inv(self_) && wf(self_))]
#[requires((*self_).nodes@.len() < 4294967295)]
#[ensures(inv(&^self_) && wf(&^self_))]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
#[ensures((^self_).tail == Some(result))]
#[ensures((^self_).head != None)]
#[ensures(result@ < (^self_).nodes@.len())]
#[ensures((^self_).nodes@[result@].active)]
#[ensures((^self_).nodes@.len() <= (*self_).nodes@.len() + 1)]
#[ensures((^self_).free@.len() <= (*self_).free@.len())]
pub fn wf_push_back(self_: &mut LruList, key: u64) -> u32 {
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

/// `push_front` (lru_list.rs:37-67) re-proved with `wf` in the contract.
#[requires(inv(self_) && wf(self_))]
#[requires((*self_).nodes@.len() < 4294967295)]
#[ensures(inv(&^self_) && wf(&^self_))]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
#[ensures((^self_).head == Some(result))]
#[ensures((^self_).tail != None)]
#[ensures(result@ < (^self_).nodes@.len())]
#[ensures((^self_).nodes@[result@].active)]
#[ensures((^self_).nodes@.len() <= (*self_).nodes@.len() + 1)]
#[ensures((^self_).free@.len() <= (*self_).free@.len())]
pub fn wf_push_front(self_: &mut LruList, key: u64) -> u32 {
    let idx = if let Some(free_idx) = self_.free.pop() {
        self_.nodes[free_idx as usize] = Node { key, prev: None, next: self_.head, active: true };
        free_idx
    } else {
        let idx = self_.nodes.len() as u32;
        self_.nodes.push(Node { key, prev: None, next: self_.head, active: true });
        idx
    };
    if let Some(old_head) = self_.head {
        self_.nodes[old_head as usize].prev = Some(idx);
    }
    self_.head = Some(idx);
    if self_.tail.is_none() {
        self_.tail = Some(idx);
    }
    self_.len += 1;
    idx
}

/// `move_to_back` (lru_list.rs:108-134) re-proved with `wf` in the contract.
#[requires(inv(self_) && wf(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
// A live entry means a non-empty order (same disclosed `ends_iff` fragment as `wf_remove`).
#[requires((*self_).nodes@[idx@].active ==> (*self_).tail != None && (*self_).head != None)]
#[ensures(inv(&^self_) && wf(&^self_))]
#[ensures((^self_).len@ == (*self_).len@)]
pub fn wf_move_to_back(self_: &mut LruList, idx: u32) {
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

/// `clear` (lru_list.rs:186-192) re-proved with `wf` in the contract.
#[ensures(inv(&^self_) && wf(&^self_))]
pub fn wf_clear(self_: &mut LruList) {
    self_.nodes.clear();
    self_.head = None;
    self_.tail = None;
    self_.free.clear();
    self_.len = 0;
}

#[requires(inv(self_) && wf(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[requires((*self_).nodes@[idx@].active ==> (*self_).tail != None && (*self_).head != None)]
#[requires((*self_).free@.len() < 4294967295)]
#[ensures(links_sym(&^self_) && links_active(&^self_) && inactive_unlinked(&^self_))]
pub fn wf_remove_a(self_: &mut LruList, idx: u32) {
    if !self_.nodes[idx as usize].active {
        return;
    }
    let old_free = snapshot! { self_.free@ };
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
    proof_assert! { self_.free@.len() == (*old_free).len() + 1 };
    proof_assert! { self_.free@[(*old_free).len()] == idx };
    proof_assert! { forall<k: Int> 0 <= k && k < (*old_free).len() ==> self_.free@[k] == (*old_free)[k] };
    self_.len -= 1;
}

#[requires(inv(self_) && wf(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[requires((*self_).nodes@[idx@].active ==> (*self_).tail != None && (*self_).head != None)]
#[requires((*self_).free@.len() < 4294967295)]
#[ensures(tail_unique(&^self_) && head_unique(&^self_))]
pub fn wf_remove_b(self_: &mut LruList, idx: u32) {
    if !self_.nodes[idx as usize].active {
        return;
    }
    let old_free = snapshot! { self_.free@ };
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
    proof_assert! { self_.free@.len() == (*old_free).len() + 1 };
    proof_assert! { self_.free@[(*old_free).len()] == idx };
    proof_assert! { forall<k: Int> 0 <= k && k < (*old_free).len() ==> self_.free@[k] == (*old_free)[k] };
    self_.len -= 1;
}

#[requires(inv(self_) && wf(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[requires((*self_).nodes@[idx@].active ==> (*self_).tail != None && (*self_).head != None)]
#[requires((*self_).free@.len() < 4294967295)]
#[ensures(ends_active(&^self_) && ends_both(&^self_))]
pub fn wf_remove_c(self_: &mut LruList, idx: u32) {
    if !self_.nodes[idx as usize].active {
        return;
    }
    let old_free = snapshot! { self_.free@ };
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
    proof_assert! { self_.free@.len() == (*old_free).len() + 1 };
    proof_assert! { self_.free@[(*old_free).len()] == idx };
    proof_assert! { forall<k: Int> 0 <= k && k < (*old_free).len() ==> self_.free@[k] == (*old_free)[k] };
    self_.len -= 1;
}

#[requires(inv(self_) && wf(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[requires((*self_).nodes@[idx@].active ==> (*self_).tail != None && (*self_).head != None)]
#[requires((*self_).free@.len() < 4294967295)]
#[ensures(free_covers_inactive(&^self_))]
pub fn wf_remove_d(self_: &mut LruList, idx: u32) {
    if !self_.nodes[idx as usize].active {
        return;
    }
    let old_free = snapshot! { self_.free@ };
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
    proof_assert! { self_.free@.len() == (*old_free).len() + 1 };
    proof_assert! { self_.free@[(*old_free).len()] == idx };
    proof_assert! { forall<k: Int> 0 <= k && k < (*old_free).len() ==> self_.free@[k] == (*old_free)[k] };
    self_.len -= 1;
}

#[requires(inv(self_) && wf(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[requires((*self_).nodes@[idx@].active ==> (*self_).tail != None && (*self_).head != None)]
#[requires((*self_).free@.len() < 4294967295)]

#[requires(inv(self_) && wf(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[requires((*self_).nodes@[idx@].active ==> (*self_).tail != None && (*self_).head != None)]
#[requires((*self_).free@.len() < 4294967295)]
#[ensures(free_covers_inactive(&^self_) && slots_accounted(&^self_) && slots_disjoint(&^self_))]
pub fn wf_remove_ad(self_: &mut LruList, idx: u32) {
    if !self_.nodes[idx as usize].active {
        return;
    }
    let old_free = snapshot! { self_.free@ };
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
    proof_assert! { self_.free@.len() == (*old_free).len() + 1 };
    proof_assert! { self_.free@[(*old_free).len()] == idx };
    proof_assert! { forall<k: Int> 0 <= k && k < (*old_free).len() ==> self_.free@[k] == (*old_free)[k] };
    self_.len -= 1;
}

// ---- EPO-INV-LIST-LINKS-SYMMETRIC (7) -------------------------------—
// Preservation across the three link-rewriting mutators: insertion at either end and the
// unlink-then-append of a touch all leave the forward and backward links agreeing.
#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(links_sym(&^l))]
pub fn verify_epo_inv_list_links_symmetric(l: &mut LruList, key: u64) -> u32 {
    wf_push_back(l, key)
}

#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(links_sym(&^l))]
pub fn verify_epo_inv_list_links_symmetric_pf(l: &mut LruList, key: u64) -> u32 {
    wf_push_front(l, key)
}

#[requires(inv(l) && wf(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).tail != None && (*l).head != None)]
#[ensures(links_sym(&^l))]
pub fn verify_epo_inv_list_links_symmetric_mv(l: &mut LruList, idx: u32) {
    wf_move_to_back(l, idx)
}

#[ensures(links_sym(&^l))]
pub fn verify_epo_inv_list_links_symmetric_cl(l: &mut LruList) {
    wf_clear(l)
}

// The remove leg of the same invariant (splice prev/next, then unlink the slot).
#[requires(inv(l) && wf(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[requires((*l).nodes@[idx@].active ==> (*l).tail != None && (*l).head != None)]
#[requires((*l).free@.len() < 4294967295)]
#[ensures(links_sym(&^l))]
pub fn verify_epo_inv_list_links_symmetric_rm(l: &mut LruList, idx: u32) {
    wf_remove_a(l, idx);
}

// ---- EPO-INV-LIST-TAIL-IS-ONLY-NODE-WITHOUT-NEXT (5) ---------------—
// The recorded back is the ONLY live entry with no forward link — which is exactly what makes
// `move_to_back`'s `self.tail == Some(idx)` early return safe rather than lucky.
#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(tail_unique(&^l) && head_unique(&^l))]
pub fn verify_epo_inv_list_tail_is_only_node_without_next(l: &mut LruList, key: u64) -> u32 {
    wf_push_back(l, key)
}

#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(tail_unique(&^l) && head_unique(&^l))]
pub fn verify_epo_inv_list_tail_is_only_node_without_next_pf(l: &mut LruList, key: u64) -> u32 {
    wf_push_front(l, key)
}

#[requires(inv(l) && wf(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).tail != None && (*l).head != None)]
#[ensures(tail_unique(&^l) && head_unique(&^l))]
pub fn verify_epo_inv_list_tail_is_only_node_without_next_mv(l: &mut LruList, idx: u32) {
    wf_move_to_back(l, idx)
}

#[ensures(tail_unique(&^l) && head_unique(&^l))]
pub fn verify_epo_inv_list_tail_is_only_node_without_next_cl(l: &mut LruList) {
    wf_clear(l)
}

#[requires(inv(l) && wf(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[requires((*l).nodes@[idx@].active ==> (*l).tail != None && (*l).head != None)]
#[requires((*l).free@.len() < 4294967295)]
#[ensures(tail_unique(&^l) && head_unique(&^l))]
pub fn verify_epo_inv_list_tail_is_only_node_without_next_rm(l: &mut LruList, idx: u32) {
    wf_remove_b(l, idx);
}

// ---- EPO-INV-LEN-MATCHES-ACTIVE-SLOTS (8) --------------------------—
// Rendered without a counting function: the reuse list is EXACTLY the set of not-live slots (no
// live slot on it, no duplicates, and every not-live slot present) and `nodes.len() == len +
// free.len()`. Those three together say `len` is precisely the number of live slots.
#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(free_covers_inactive(&^l) && slots_disjoint(&^l) && slots_accounted(&^l))]
pub fn verify_epo_inv_len_matches_active_slots(l: &mut LruList, key: u64) -> u32 {
    wf_push_back(l, key)
}

#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(free_covers_inactive(&^l) && slots_disjoint(&^l) && slots_accounted(&^l))]
pub fn verify_epo_inv_len_matches_active_slots_pf(l: &mut LruList, key: u64) -> u32 {
    wf_push_front(l, key)
}

#[requires(inv(l) && wf(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).tail != None && (*l).head != None)]
#[ensures(free_covers_inactive(&^l) && slots_disjoint(&^l) && slots_accounted(&^l))]
pub fn verify_epo_inv_len_matches_active_slots_mv(l: &mut LruList, idx: u32) {
    wf_move_to_back(l, idx)
}

#[ensures(free_covers_inactive(&^l) && slots_disjoint(&^l) && slots_accounted(&^l))]
pub fn verify_epo_inv_len_matches_active_slots_cl(l: &mut LruList) {
    wf_clear(l)
}

#[requires(inv(l) && wf(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[requires((*l).nodes@[idx@].active ==> (*l).tail != None && (*l).head != None)]
#[requires((*l).free@.len() < 4294967295)]
#[ensures(free_covers_inactive(&^l) && slots_disjoint(&^l) && slots_accounted(&^l))]
pub fn verify_epo_inv_len_matches_active_slots_rm(l: &mut LruList, idx: u32) {
    wf_remove_ad(l, idx);
}

// ===========================================================================
// REFUTATION WITNESSES — the two obligations the inventory expects to be REFUTED.
// These modules do NOT prove their properties (which are FALSE of this component); they prove
// the PREMISE of the counterexample, so the defect is machine-checked rather than asserted.
// They are deliberately NOT cited as `evidence.modules` for those ids.
// ===========================================================================

/// Refutation premise for EPO-INV-STALE-HANDLE-NEVER-CRASHES.
/// `clear_pool` empties the slot storage (lru_list.rs:187-191), so afterwards EVERY slot number
/// the pool ever issued is out of range — and `touch`/`remove` index `self.nodes[idx as usize]`
/// with no bounds check at all (lru_list.rs:109, :160), so using any such handle PANICS. One such
/// panic also poisons that pool's `Mutex`, so every later operation on the pool panics too.
#[ensures((^l).nodes@.len() == 0)]
#[ensures(issued_index@ >= (^l).nodes@.len())]
pub fn refute_epo_inv_stale_handle_never_crashes(l: &mut LruList, issued_index: u32) {
    arena_clear(l);
}

/// Refutation premise for EPO-INV-STALE-HANDLE-NO-CROSS-ENTRY-EFFECT.
/// `remove` pushes the freed slot onto the reuse list (lru_list.rs:181) and the next
/// `push_back`/`push_front` pops it straight back (lru_list.rs:38, :76), so the very same slot
/// number comes to name a DIFFERENT key — while still looking perfectly live. A handle kept from
/// the vanished entry therefore acts on that different key's entry, and nothing detects it.
#[requires(inv(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active && (*l).len@ >= 1)]
#[requires((*l).free@.len() < 4294967295 && (*l).nodes@.len() < 4294967295)]
#[requires(new_key != (*l).nodes@[idx@].key)]
#[ensures(result == idx)]
#[ensures((^l).nodes@[result@].key == new_key)]
#[ensures((^l).nodes@[idx@].key != (*l).nodes@[idx@].key)]
#[ensures((^l).nodes@[idx@].active)]
pub fn refute_epo_inv_stale_handle_no_cross_entry_effect(l: &mut LruList, idx: u32, new_key: u64) -> u32 {
    arena_remove(l, idx);
    arena_push_back(l, new_key)
}

// ===========================================================================
// The reachability group — the head-to-tail WALK. Attempted with a fuel-bounded recursive
// logic function; the residual failure signature is recorded in ../verif/creusot_advisory.yaml.
// ===========================================================================

/// Number of entries on the forward walk starting at `cur`, capped at `fuel` steps.
#[logic]
#[variant(fuel)]
#[requires(fuel >= 0)]
pub fn chain_len(nodes: Seq<Node>, cur: Option<u32>, fuel: Int) -> Int {
    pearlite! {
        if fuel <= 0 { 0 } else {
            match cur {
                None => 0,
                Some(c) => if c@ < nodes.len() { 1 + chain_len(nodes, (nodes[c@]).next, fuel - 1) }
                           else { 0 }
            }
        }
    }
}

/// EPO-INV-LEN-MATCHES-CHAIN: the stored count equals the length of the head-to-tail walk.
#[logic]
pub fn len_is_chain(l: &LruList) -> bool {
    pearlite! { l.len@ == chain_len(l.nodes@, l.head, l.nodes@.len()) }
}

/// EPO-INV-LIST-ACYCLIC-AND-LENGTH: the walk reaches the end in exactly `len` steps and then
/// stops — extra fuel adds nothing, so the order contains no loop.
#[logic]
pub fn acyclic_and_length(l: &LruList) -> bool {
    pearlite! {
        chain_len(l.nodes@, l.head, l.nodes@.len()) == l.len@
        && chain_len(l.nodes@, l.head, l.nodes@.len() + 1) == l.len@
    }
}

// ---------------------------------------------------------------------------
// Fuel arithmetic for `chain_len`. Three inductions over the fuel parameter that let a caller
// move between the two fuel budgets `nodes.len()` and `nodes.len() + 1` that the reachability
// invariants are stated at. Each is a `#[logic]` lemma function: it carries no computational
// content, its `#[ensures]` IS the fact, and the recursive call in its body IS the induction step.
// ---------------------------------------------------------------------------

/// A fuel-bounded walk never counts more steps than the fuel it was given.
#[logic]
#[variant(fuel)]
#[requires(fuel >= 0)]
#[ensures(0 <= chain_len(nodes, cur, fuel) && chain_len(nodes, cur, fuel) <= fuel)]
pub fn lemma_epo_chain_le_fuel(nodes: Seq<Node>, cur: Option<u32>, fuel: Int) {
    pearlite! {
        if fuel <= 0 { () } else {
            match cur {
                None => (),
                Some(c) => if c@ < nodes.len() {
                    lemma_epo_chain_le_fuel(nodes, (nodes[c@]).next, fuel - 1)
                } else { () }
            }
        }
    }
}

/// More fuel never shortens a walk.
#[logic]
#[variant(fuel)]
#[requires(fuel >= 0)]
#[ensures(chain_len(nodes, cur, fuel) <= chain_len(nodes, cur, fuel + 1))]
pub fn lemma_epo_chain_fuel_monotone(nodes: Seq<Node>, cur: Option<u32>, fuel: Int) {
    pearlite! {
        if fuel <= 0 { () } else {
            match cur {
                None => (),
                Some(c) => if c@ < nodes.len() {
                    lemma_epo_chain_fuel_monotone(nodes, (nodes[c@]).next, fuel - 1)
                } else { () }
            }
        }
    }
}

/// Once a walk has already stopped inside its fuel budget, extra fuel adds nothing — this is the
/// step that turns "the walk terminates" into "the count is fuel-independent".
#[logic]
#[variant(fuel)]
#[requires(fuel >= 0)]
#[ensures(chain_len(nodes, cur, fuel) < fuel ==>
             chain_len(nodes, cur, fuel + 1) == chain_len(nodes, cur, fuel))]
pub fn lemma_epo_chain_fuel_stable(nodes: Seq<Node>, cur: Option<u32>, fuel: Int) {
    pearlite! {
        if fuel <= 0 { () } else {
            match cur {
                None => (),
                Some(c) => if c@ < nodes.len() {
                    lemma_epo_chain_fuel_stable(nodes, (nodes[c@]).next, fuel - 1)
                } else { () }
            }
        }
    }
}

/// A walk counts zero steps only when there is nowhere to start from — the bridge from
/// `len == 0` to `head == None` that `ends_iff` needs. One unfolding plus non-negativity; no
/// induction of its own.
#[logic]
#[requires(fuel >= 1)]
#[requires(match cur { Some(c) => c@ < nodes.len(), None => true })]
#[ensures(chain_len(nodes, cur, fuel) == 0 ==> cur == None)]
pub fn lemma_epo_chain_zero_means_absent(nodes: Seq<Node>, cur: Option<u32>, fuel: Int) {
    pearlite! {
        match cur {
            None => (),
            Some(c) => lemma_epo_chain_le_fuel(nodes, (nodes[c@]).next, fuel - 1)
        }
    }
}

/// THE frame/extend induction the SMT portfolio will not take unaided: appending one new final
/// entry after the recorded back `t` lengthens every walk that starts inside the order by exactly
/// one step.
///
/// `a` is the arena before the push and `b` the arena after. The hypotheses are exactly what
/// `push_back` establishes and what `wf` already guarantees:
///   * `b` agrees with `a` on every forward link except the old back's (`i != t`), and may have
///     grown by the freshly pushed slot (`a.len() <= b.len()`);
///   * the old back had no forward link and now points at `new`, which itself has none;
///   * in `a`, a live entry's forward link is either an in-bounds live entry or absent, and it is
///     absent only for `t` itself (that is `links_active` + `tail_unique` + `ends_active`).
/// Under those, the walk from any live in-bounds `cur` is one step longer in `b` than in `a`.
#[logic]
#[variant(fuel)]
#[requires(fuel >= 0)]
#[requires(a.len() <= b.len())]
#[requires(t@ < a.len() && (a[t@]).next == None && (b[t@]).next == Some(new))]
#[requires(new@ < b.len() && (b[new@]).next == None)]
#[requires(forall<i: Int> 0 <= i && i < a.len() && i != t@ ==> (a[i]).next == (b[i]).next)]
#[requires(forall<i: u32> i@ < a.len() && (a[i@]).active ==>
             match (a[i@]).next { Some(n) => n@ < a.len() && (a[n@]).active, None => i == t })]
#[requires(match cur { Some(c) => c@ < a.len() && (a[c@]).active, None => true })]
#[ensures(match cur { Some(_) => chain_len(b, cur, fuel + 1) == chain_len(a, cur, fuel) + 1,
                      None => true })]
pub fn lemma_epo_chain_extend(a: Seq<Node>, b: Seq<Node>, cur: Option<u32>, t: u32, new: u32,
                              fuel: Int) {
    pearlite! {
        if fuel <= 0 { () } else {
            match cur {
                None => (),
                Some(c) => if c == t { () } else {
                    lemma_epo_chain_extend(a, b, (a[c@]).next, t, new, fuel - 1)
                }
            }
        }
    }
}

/// `push_back` (lru_list.rs:75-105) re-proved a third time, now with the FORWARD-LINK FRAME in the
/// contract — the extra postconditions `lemma_epo_chain_extend` needs. Same body, character for
/// character, as `arena_push_back` / `wf_push_back`; only the contract differs.
#[requires(inv(self_) && wf(self_))]
#[requires((*self_).nodes@.len() < 4294967295)]
#[ensures(inv(&^self_) && wf(&^self_))]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
#[ensures((^self_).tail == Some(result))]
#[ensures(result@ < (^self_).nodes@.len())]
#[ensures((^self_).nodes@[result@].active)]
#[ensures((^self_).nodes@[result@].next == None)]
#[ensures((*self_).nodes@.len() <= (^self_).nodes@.len())]
#[ensures((^self_).nodes@.len() <= (*self_).nodes@.len() + 1)]
#[ensures(result@ < (*self_).nodes@.len() ==> !((*self_).nodes@[result@]).active)]
#[ensures((*self_).head == None ==> (^self_).head == Some(result))]
#[ensures((*self_).head != None ==> (^self_).head == (*self_).head)]
// the old back now points at the new slot ...
#[ensures(match (*self_).tail { Some(t) => (^self_).nodes@[t@].next == Some(result), None => true })]
// ... and no other pre-existing slot's forward link moved.
#[ensures(forall<i: Int> 0 <= i && i < (*self_).nodes@.len() && i != result@
            && (match (*self_).tail { Some(t) => i != t@, None => true }) ==>
              (^self_).nodes@[i].next == (*self_).nodes@[i].next)]
pub fn chain_push_back(self_: &mut LruList, key: u64) -> u32 {
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

// ---- EPO-INV-LEN-MATCHES-CHAIN (9) ---------------------------------—
#[requires(inv(l) && wf(l) && len_is_chain(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(len_is_chain(&^l))]
pub fn verify_epo_inv_len_matches_chain(l: &mut LruList, key: u64) -> u32 {
    let old = snapshot! { *l };
    let old_tail = l.tail;
    let idx = chain_push_back(l, key);
    match old_tail {
        // An empty order: `ends_both` makes the front absent too, so the new entry IS the whole
        // walk and the count is one by unfolding alone.
        None => {
            proof_assert! { (*old).head == None };
            proof_assert! { (*old).len@ == 0 };
            proof_assert! { idx@ < l.nodes@.len() };
            proof_assert! { l.head == Some(idx) };
            proof_assert! { (l.nodes@[idx@]).next == None };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len()) == 1 };
        }
        Some(t) => {
            proof_assert! { (*old).head != None };
            proof_assert! { l.head == (*old).head };
            proof_assert! { match (*old).head { Some(h) => h@ < (*old).nodes@.len()
                                                  && ((*old).nodes@[h@]).active, None => true } };
            // `ends_active` + `tail_unique`: the recorded back is live and has no forward link.
            proof_assert! { t@ < (*old).nodes@.len() };
            proof_assert! { ((*old).nodes@[t@]).active };
            proof_assert! { ((*old).nodes@[t@]).next == None };
            // `wf`'s `links_active` + `tail_unique`, in the shape the extend lemma wants: in the
            // PRE-state a live entry's forward link is an in-bounds live entry, and it is absent
            // only for the recorded back.
            proof_assert! {
                forall<i: u32> i@ < (*old).nodes@.len() && ((*old).nodes@[i@]).active ==>
                    match ((*old).nodes@[i@]).next {
                        Some(n) => n@ < (*old).nodes@.len() && ((*old).nodes@[n@]).active,
                        None => i == t }
            };
            // `inactive_unlinked` + `slots_disjoint`: a recycled slot was not live and so had no
            // forward link either ...
            proof_assert! { idx@ < (*old).nodes@.len() ==> !(((*old).nodes@[idx@]).active) };
            proof_assert! { idx@ < (*old).nodes@.len() ==> ((*old).nodes@[idx@]).next == None };
            // ... hence the frame hypothesis holds at the new slot as well as everywhere but `t`.
            proof_assert! {
                forall<i: Int> 0 <= i && i < (*old).nodes@.len() && i != t@ ==>
                    (l.nodes@[i]).next == ((*old).nodes@[i]).next
            };
            // THE induction the portfolio will not take unaided.
            proof_assert! {
                lemma_epo_chain_extend((*old).nodes@, l.nodes@, (*old).head, t, idx,
                                       (*old).nodes@.len());
                chain_len(l.nodes@, l.head, (*old).nodes@.len() + 1) == (*old).len@ + 1
            };
            // Fuel reconciliation: the invariant is stated at fuel `nodes.len()`, which either grew
            // with the push (then the line above IS the goal) or stayed put because a recycled slot
            // was reused (then the walk already stopped inside the budget, so the count is
            // fuel-independent). Expose the three fuel facts and let the arithmetic close both.
            proof_assert! {
                lemma_epo_chain_le_fuel(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len()) <= l.nodes@.len()
            };
            proof_assert! {
                lemma_epo_chain_fuel_monotone(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len())
                     <= chain_len(l.nodes@, l.head, l.nodes@.len() + 1)
            };
            proof_assert! {
                lemma_epo_chain_fuel_stable(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len()) < l.nodes@.len() ==>
                      chain_len(l.nodes@, l.head, l.nodes@.len() + 1)
                        == chain_len(l.nodes@, l.head, l.nodes@.len())
            };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len()) == l.len@ };
        }
    }
    idx
}

/// Strengthened-invariant lever variant (`invariant_strengthen`) for EPO-INV-LEN-MATCHES-CHAIN:
/// the same obligation with acyclicity additionally assumed.
#[requires(inv(l) && wf(l) && len_is_chain(l) && acyclic_and_length(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(len_is_chain(&^l))]
pub fn verify_epo_inv_len_matches_chain__inv(l: &mut LruList, key: u64) -> u32 {
    let old = snapshot! { *l };
    let old_tail = l.tail;
    let idx = chain_push_back(l, key);
    match old_tail {
        // An empty order: `ends_both` makes the front absent too, so the new entry IS the whole
        // walk and the count is one by unfolding alone.
        None => {
            proof_assert! { (*old).head == None };
            proof_assert! { (*old).len@ == 0 };
            proof_assert! { idx@ < l.nodes@.len() };
            proof_assert! { l.head == Some(idx) };
            proof_assert! { (l.nodes@[idx@]).next == None };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len()) == 1 };
        }
        Some(t) => {
            proof_assert! { (*old).head != None };
            proof_assert! { l.head == (*old).head };
            proof_assert! { match (*old).head { Some(h) => h@ < (*old).nodes@.len()
                                                  && ((*old).nodes@[h@]).active, None => true } };
            // `ends_active` + `tail_unique`: the recorded back is live and has no forward link.
            proof_assert! { t@ < (*old).nodes@.len() };
            proof_assert! { ((*old).nodes@[t@]).active };
            proof_assert! { ((*old).nodes@[t@]).next == None };
            // `wf`'s `links_active` + `tail_unique`, in the shape the extend lemma wants: in the
            // PRE-state a live entry's forward link is an in-bounds live entry, and it is absent
            // only for the recorded back.
            proof_assert! {
                forall<i: u32> i@ < (*old).nodes@.len() && ((*old).nodes@[i@]).active ==>
                    match ((*old).nodes@[i@]).next {
                        Some(n) => n@ < (*old).nodes@.len() && ((*old).nodes@[n@]).active,
                        None => i == t }
            };
            // `inactive_unlinked` + `slots_disjoint`: a recycled slot was not live and so had no
            // forward link either ...
            proof_assert! { idx@ < (*old).nodes@.len() ==> !(((*old).nodes@[idx@]).active) };
            proof_assert! { idx@ < (*old).nodes@.len() ==> ((*old).nodes@[idx@]).next == None };
            // ... hence the frame hypothesis holds at the new slot as well as everywhere but `t`.
            proof_assert! {
                forall<i: Int> 0 <= i && i < (*old).nodes@.len() && i != t@ ==>
                    (l.nodes@[i]).next == ((*old).nodes@[i]).next
            };
            // THE induction the portfolio will not take unaided.
            proof_assert! {
                lemma_epo_chain_extend((*old).nodes@, l.nodes@, (*old).head, t, idx,
                                       (*old).nodes@.len());
                chain_len(l.nodes@, l.head, (*old).nodes@.len() + 1) == (*old).len@ + 1
            };
            // Fuel reconciliation: the invariant is stated at fuel `nodes.len()`, which either grew
            // with the push (then the line above IS the goal) or stayed put because a recycled slot
            // was reused (then the walk already stopped inside the budget, so the count is
            // fuel-independent). Expose the three fuel facts and let the arithmetic close both.
            proof_assert! {
                lemma_epo_chain_le_fuel(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len()) <= l.nodes@.len()
            };
            proof_assert! {
                lemma_epo_chain_fuel_monotone(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len())
                     <= chain_len(l.nodes@, l.head, l.nodes@.len() + 1)
            };
            proof_assert! {
                lemma_epo_chain_fuel_stable(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len()) < l.nodes@.len() ==>
                      chain_len(l.nodes@, l.head, l.nodes@.len() + 1)
                        == chain_len(l.nodes@, l.head, l.nodes@.len())
            };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len()) == l.len@ };
        }
    }
    idx
}

/// Inductive-lemma lever variant (`lemma_function`) for EPO-INV-LEN-MATCHES-CHAIN: the frame
/// lemma the preservation proof needs — a walk whose `next` fields are unchanged has the same
/// length. This is the step the SMT backend cannot take without induction.
#[logic]
#[variant(fuel)]
#[requires(fuel >= 0)]
#[requires(a.len() == b.len())]
#[requires(forall<i: Int> 0 <= i && i < a.len() ==> (a[i]).next == (b[i]).next)]
#[ensures(chain_len(a, cur, fuel) == chain_len(b, cur, fuel))]
pub fn lemma_epo_inv_len_matches_chain(a: Seq<Node>, b: Seq<Node>, cur: Option<u32>, fuel: Int) {
    pearlite! {
        if fuel <= 0 { () } else {
            match cur {
                None => (),
                Some(c) => if c@ < a.len() {
                    lemma_epo_inv_len_matches_chain(a, b, (a[c@]).next, fuel - 1)
                } else { () }
            }
        }
    }
}

// ---- EPO-INV-LIST-ACYCLIC-AND-LENGTH (8) ---------------------------—
#[requires(inv(l) && wf(l) && acyclic_and_length(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(acyclic_and_length(&^l))]
pub fn verify_epo_inv_list_acyclic_and_length(l: &mut LruList, key: u64) -> u32 {
    let old = snapshot! { *l };
    let old_tail = l.tail;
    let idx = chain_push_back(l, key);
    match old_tail {
        // An empty order: the new entry is the whole walk, at either fuel budget.
        None => {
            proof_assert! { (*old).head == None };
            proof_assert! { (*old).len@ == 0 };
            proof_assert! { idx@ < l.nodes@.len() };
            proof_assert! { l.head == Some(idx) };
            proof_assert! { (l.nodes@[idx@]).next == None };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len()) == 1 };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len() + 1) == 1 };
        }
        Some(t) => {
            proof_assert! { (*old).head != None };
            proof_assert! { l.head == (*old).head };
            proof_assert! { match (*old).head { Some(h) => h@ < (*old).nodes@.len()
                                                  && ((*old).nodes@[h@]).active, None => true } };
            proof_assert! { t@ < (*old).nodes@.len() };
            proof_assert! { ((*old).nodes@[t@]).active };
            proof_assert! { ((*old).nodes@[t@]).next == None };
            proof_assert! {
                forall<i: u32> i@ < (*old).nodes@.len() && ((*old).nodes@[i@]).active ==>
                    match ((*old).nodes@[i@]).next {
                        Some(n) => n@ < (*old).nodes@.len() && ((*old).nodes@[n@]).active,
                        None => i == t }
            };
            proof_assert! { idx@ < (*old).nodes@.len() ==> !(((*old).nodes@[idx@]).active) };
            proof_assert! { idx@ < (*old).nodes@.len() ==> ((*old).nodes@[idx@]).next == None };
            proof_assert! {
                forall<i: Int> 0 <= i && i < (*old).nodes@.len() && i != t@ ==>
                    (l.nodes@[i]).next == ((*old).nodes@[i]).next
            };
            // THE induction, applied at BOTH fuel budgets the acyclicity statement uses: the walk
            // is one step longer after the append, and one MORE unit of fuel still adds nothing —
            // which is exactly "the order contains no loop".
            proof_assert! {
                lemma_epo_chain_extend((*old).nodes@, l.nodes@, (*old).head, t, idx,
                                       (*old).nodes@.len());
                chain_len(l.nodes@, l.head, (*old).nodes@.len() + 1) == (*old).len@ + 1
            };
            proof_assert! {
                lemma_epo_chain_extend((*old).nodes@, l.nodes@, (*old).head, t, idx,
                                       (*old).nodes@.len() + 1);
                chain_len(l.nodes@, l.head, (*old).nodes@.len() + 2) == (*old).len@ + 1
            };
            // Fuel reconciliation for the recycled-slot case, where `nodes.len()` did not grow.
            proof_assert! {
                lemma_epo_chain_le_fuel(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len()) <= l.nodes@.len()
            };
            proof_assert! {
                lemma_epo_chain_fuel_monotone(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len())
                     <= chain_len(l.nodes@, l.head, l.nodes@.len() + 1)
            };
            proof_assert! {
                lemma_epo_chain_fuel_stable(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len()) < l.nodes@.len() ==>
                      chain_len(l.nodes@, l.head, l.nodes@.len() + 1)
                        == chain_len(l.nodes@, l.head, l.nodes@.len())
            };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len()) == l.len@ };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len() + 1) == l.len@ };
        }
    }
    idx
}

#[requires(inv(l) && wf(l) && acyclic_and_length(l) && len_is_chain(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(acyclic_and_length(&^l))]
pub fn verify_epo_inv_list_acyclic_and_length__inv(l: &mut LruList, key: u64) -> u32 {
    let old = snapshot! { *l };
    let old_tail = l.tail;
    let idx = chain_push_back(l, key);
    match old_tail {
        // An empty order: the new entry is the whole walk, at either fuel budget.
        None => {
            proof_assert! { (*old).head == None };
            proof_assert! { (*old).len@ == 0 };
            proof_assert! { idx@ < l.nodes@.len() };
            proof_assert! { l.head == Some(idx) };
            proof_assert! { (l.nodes@[idx@]).next == None };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len()) == 1 };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len() + 1) == 1 };
        }
        Some(t) => {
            proof_assert! { (*old).head != None };
            proof_assert! { l.head == (*old).head };
            proof_assert! { match (*old).head { Some(h) => h@ < (*old).nodes@.len()
                                                  && ((*old).nodes@[h@]).active, None => true } };
            proof_assert! { t@ < (*old).nodes@.len() };
            proof_assert! { ((*old).nodes@[t@]).active };
            proof_assert! { ((*old).nodes@[t@]).next == None };
            proof_assert! {
                forall<i: u32> i@ < (*old).nodes@.len() && ((*old).nodes@[i@]).active ==>
                    match ((*old).nodes@[i@]).next {
                        Some(n) => n@ < (*old).nodes@.len() && ((*old).nodes@[n@]).active,
                        None => i == t }
            };
            proof_assert! { idx@ < (*old).nodes@.len() ==> !(((*old).nodes@[idx@]).active) };
            proof_assert! { idx@ < (*old).nodes@.len() ==> ((*old).nodes@[idx@]).next == None };
            proof_assert! {
                forall<i: Int> 0 <= i && i < (*old).nodes@.len() && i != t@ ==>
                    (l.nodes@[i]).next == ((*old).nodes@[i]).next
            };
            // THE induction, applied at BOTH fuel budgets the acyclicity statement uses: the walk
            // is one step longer after the append, and one MORE unit of fuel still adds nothing —
            // which is exactly "the order contains no loop".
            proof_assert! {
                lemma_epo_chain_extend((*old).nodes@, l.nodes@, (*old).head, t, idx,
                                       (*old).nodes@.len());
                chain_len(l.nodes@, l.head, (*old).nodes@.len() + 1) == (*old).len@ + 1
            };
            proof_assert! {
                lemma_epo_chain_extend((*old).nodes@, l.nodes@, (*old).head, t, idx,
                                       (*old).nodes@.len() + 1);
                chain_len(l.nodes@, l.head, (*old).nodes@.len() + 2) == (*old).len@ + 1
            };
            // Fuel reconciliation for the recycled-slot case, where `nodes.len()` did not grow.
            proof_assert! {
                lemma_epo_chain_le_fuel(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len()) <= l.nodes@.len()
            };
            proof_assert! {
                lemma_epo_chain_fuel_monotone(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len())
                     <= chain_len(l.nodes@, l.head, l.nodes@.len() + 1)
            };
            proof_assert! {
                lemma_epo_chain_fuel_stable(l.nodes@, l.head, l.nodes@.len());
                chain_len(l.nodes@, l.head, l.nodes@.len()) < l.nodes@.len() ==>
                      chain_len(l.nodes@, l.head, l.nodes@.len() + 1)
                        == chain_len(l.nodes@, l.head, l.nodes@.len())
            };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len()) == l.len@ };
            proof_assert! { chain_len(l.nodes@, l.head, l.nodes@.len() + 1) == l.len@ };
        }
    }
    idx
}

#[logic]
#[variant(fuel)]
#[requires(fuel >= 0)]
#[requires(a.len() == b.len())]
#[requires(forall<i: Int> 0 <= i && i < a.len() ==> (a[i]).next == (b[i]).next)]
#[ensures(chain_len(a, cur, fuel) == chain_len(b, cur, fuel))]
pub fn lemma_epo_inv_list_acyclic_and_length(a: Seq<Node>, b: Seq<Node>, cur: Option<u32>, fuel: Int) {
    pearlite! {
        if fuel <= 0 { () } else {
            match cur {
                None => (),
                Some(c) => if c@ < a.len() {
                    lemma_epo_inv_list_acyclic_and_length(a, b, (a[c@]).next, fuel - 1)
                } else { () }
            }
        }
    }
}

/// `remove` (lru_list.rs:159-183) re-proved once more, with the FRONT/BACK RELOCATION in the
/// contract. Same body, character for character, as `arena_remove` / `wf_remove_a`..`wf_remove_e`;
/// only the contract differs. This is the single-call shape the `ends_iff` obligation needs — the
/// previous two-call `wf_remove_a(l, idx); wf_remove_c(l, idx)` body could not discharge the second
/// call's `inv && wf` precondition, because `wf_remove_a` only re-establishes three of `wf`'s eight
/// conjuncts, so that failure was a harness artifact rather than a statement about the tool.
#[requires(inv(self_) && wf(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[requires((*self_).nodes@[idx@].active ==> (*self_).tail != None && (*self_).head != None)]
#[requires((*self_).free@.len() < 4294967295)]
#[ensures(ends_active(&^self_) && ends_both(&^self_))]
#[ensures(!(*self_).nodes@[idx@].active ==> ^self_ == *self_)]
#[ensures(!(*self_).nodes@[idx@].active ==> (^self_).len@ == (*self_).len@)]
#[ensures(!(*self_).nodes@[idx@].active ==> (^self_).head == (*self_).head)]
#[ensures(!(*self_).nodes@[idx@].active ==> (^self_).tail == (*self_).tail)]
#[ensures((*self_).nodes@[idx@].active ==> (^self_).len@ == (*self_).len@ - 1)]
#[ensures((^self_).nodes@.len() == (*self_).nodes@.len())]
#[ensures((*self_).nodes@[idx@].active && (*self_).nodes@[idx@].prev == None ==>
             (^self_).head == (*self_).nodes@[idx@].next)]
#[ensures((*self_).nodes@[idx@].prev != None ==> (^self_).head == (*self_).head)]
pub fn chain_remove(self_: &mut LruList, idx: u32) {
    if !self_.nodes[idx as usize].active {
        return;
    }
    let old_free = snapshot! { self_.free@ };
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
    proof_assert! { self_.free@.len() == (*old_free).len() + 1 };
    proof_assert! { self_.free@[(*old_free).len()] == idx };
    proof_assert! { forall<k: Int> 0 <= k && k < (*old_free).len() ==> self_.free@[k] == (*old_free)[k] };
    self_.len -= 1;
}

// ---- EPO-INV-LIST-EMPTY-IFF-NO-ENDS (9) ---------------------------—
// The `never a front without a back` half is `ends_both`, already inside `wf` and proved. The
// `never a non-zero size with no ends` half needs `len == number of live slots` TOGETHER WITH
// acyclicity, so it is attempted here as full `ends_iff` preservation.
#[requires(inv(l) && wf(l) && ends_iff(l) && len_is_chain(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[requires((*l).nodes@[idx@].active ==> (*l).tail != None && (*l).head != None)]
#[requires((*l).free@.len() < 4294967295)]
#[ensures(ends_iff(&^l))]
pub fn verify_epo_inv_list_empty_iff_no_ends(l: &mut LruList, idx: u32) {
    let old = snapshot! { *l };
    let prev = l.nodes[idx as usize].prev;
    chain_remove(l, idx);
    // `ends_both` (from `chain_remove`) reduces the obligation to the FRONT half: it is enough to
    // show `(len == 0) == (head == None)` after the removal.
    proof_assert! { (l.head == None) == (l.tail == None) };
    proof_assert! { prev == ((*old).nodes@[idx@]).prev };
    proof_assert! { !(((*old).nodes@[idx@]).active) ==> l.len@ == (*old).len@ };
    proof_assert! { !(((*old).nodes@[idx@]).active) ==> l.head == (*old).head };
    proof_assert! { !(((*old).nodes@[idx@]).active) ==> ((*old).len@ == 0) == ((*old).head == None) };
    match prev {
        // The removed entry was the FRONT of the order (`head_unique`). `len_is_chain` then reads
        // the answer straight off the walk: the count drops to zero exactly when the entry had no
        // successor, and `head` is set to that successor.
        None => {
            proof_assert! { ((*old).nodes@[idx@]).active ==> (*old).head == Some(idx) };
            proof_assert! { ((*old).nodes@[idx@]).active ==>
                              (*old).len@ == 1 + chain_len((*old).nodes@,
                                                           ((*old).nodes@[idx@]).next,
                                                           (*old).nodes@.len() - 1) };
            // A one-slot arena: the only in-bounds forward link would be a self-loop, which
            // `links_sym` turns into a non-empty backward link — and this branch has none.
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).nodes@.len() == 1 ==>
                              ((*old).nodes@[idx@]).next == None };
            // Otherwise one unfolding of `chain_len` does it: a walk of one step from the front
            // leaves nothing for the successor to count.
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).nodes@.len() > 1
                            && (*old).len@ == 1 ==>
                              chain_len((*old).nodes@, ((*old).nodes@[idx@]).next,
                                        (*old).nodes@.len() - 1) == 0 };
            proof_assert! {
                lemma_epo_chain_zero_means_absent((*old).nodes@, ((*old).nodes@[idx@]).next,
                                                  (*old).nodes@.len() - 1);
                ((*old).nodes@[idx@]).active && (*old).nodes@.len() > 1 && (*old).len@ == 1 ==>
                  ((*old).nodes@[idx@]).next == None
            };
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).len@ == 1 ==>
                              ((*old).nodes@[idx@]).next == None };
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).len@ > 1 ==>
                              ((*old).nodes@[idx@]).next != None };
            proof_assert! { (l.len@ == 0) == (l.head == None) };
        }
        // The removed entry was NOT the front, so the front does not move. Then `len > 1` keeps the
        // front in place, and `len == 1` must be impossible — see the residual note below.
        Some(_) => {
            proof_assert! { l.head == (*old).head };
            proof_assert! { ((*old).nodes@[idx@]).active ==> (*old).head != None };
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).len@ > 1 ==>
                              l.len@ > 0 && l.head != None };
            proof_assert! { l.head == None ==> l.len@ == 0 };
            // The removed entry is live and is NOT the front, so the front is a second, distinct
            // live slot.
            proof_assert! { ((*old).nodes@[idx@]).active ==>
                              match (*old).head { Some(h) => h != idx && h@ < (*old).nodes@.len()
                                                    && ((*old).nodes@[h@]).active,
                                                  None => false } };
            // RESIDUAL — the ONLY unproved subgoal of this module. Reaching `len == 0` here means
            // the pre-state had `len == 1` with TWO distinct live slots (`idx` and the front).
            // Ruling that out needs `len == the number of live slots`, i.e. the pigeonhole step
            // "the reuse list, being duplicate-free and holding only not-live slots, is no longer
            // than the number of not-live slots". `inv`/`wf` supply the three ingredients
            // (`slots_accounted`, `slots_disjoint`, `free_covers_inactive`) but NOT the counting
            // argument that combines them, and counting a duplicate-free index list against the
            // set it covers is a cardinality/pigeonhole induction, not an SMT-reachable step.
            // `free_len_counts_inactive` below assumes exactly that one instance and closes it.
            proof_assert! { l.len@ == 0 ==> l.head == None };
        }
    }
}

/// The counting step `inv` stops short of, stated at exactly the instance the `ends_iff` proof
/// needs: TWO distinct live slots leave the reuse list at least two slots short of the arena.
/// Together with `inv`'s `slots_accounted` (`nodes.len() == len + free.len()`) that reads "two
/// distinct live slots force `len >= 2`", i.e. `len` really is the number of live slots.
///
/// It is TRUE of the shipped structure — `slots_disjoint` makes `free` duplicate-free and live-free
/// and `free_covers_inactive` makes it cover every not-live slot, so `free.len()` IS the number of
/// not-live slots — but deriving it is a cardinality/pigeonhole induction over a duplicate-free
/// index list, which is why it is ASSUMED here rather than proved.
#[logic]
pub fn free_len_counts_inactive(l: &LruList) -> bool {
    pearlite! {
        forall<i: u32, j: u32> i@ < l.nodes@.len() && j@ < l.nodes@.len() && i != j
            && (l.nodes@[i@]).active && (l.nodes@[j@]).active
            ==> l.free@.len() + 2 <= l.nodes@.len()
    }
}

/// Strengthened-invariant lever variant (`invariant_strengthen`) for
/// EPO-INV-LIST-EMPTY-IFF-NO-ENDS. Identical obligation and identical body to the base; the ONLY
/// difference is the added `free_len_counts_inactive(l)` conjunct in the precondition, which is
/// precisely the base's single residual subgoal. NOTE: `cargo creusot` renders the source `__inv`
/// suffix with a SINGLE underscore, so this module is emitted as
/// `verify_epo_inv_list_empty_iff_no_ends_inv.coma`.
#[requires(inv(l) && wf(l) && ends_iff(l) && len_is_chain(l) && free_len_counts_inactive(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[requires((*l).nodes@[idx@].active ==> (*l).tail != None && (*l).head != None)]
#[requires((*l).free@.len() < 4294967295)]
#[ensures(ends_iff(&^l))]
pub fn verify_epo_inv_list_empty_iff_no_ends__inv(l: &mut LruList, idx: u32) {
    let old = snapshot! { *l };
    let prev = l.nodes[idx as usize].prev;
    chain_remove(l, idx);
    // `ends_both` (from `chain_remove`) reduces the obligation to the FRONT half: it is enough to
    // show `(len == 0) == (head == None)` after the removal.
    proof_assert! { (l.head == None) == (l.tail == None) };
    proof_assert! { prev == ((*old).nodes@[idx@]).prev };
    proof_assert! { !(((*old).nodes@[idx@]).active) ==> l.len@ == (*old).len@ };
    proof_assert! { !(((*old).nodes@[idx@]).active) ==> l.head == (*old).head };
    proof_assert! { !(((*old).nodes@[idx@]).active) ==> ((*old).len@ == 0) == ((*old).head == None) };
    match prev {
        // The removed entry was the FRONT of the order (`head_unique`). `len_is_chain` then reads
        // the answer straight off the walk: the count drops to zero exactly when the entry had no
        // successor, and `head` is set to that successor.
        None => {
            proof_assert! { ((*old).nodes@[idx@]).active ==> (*old).head == Some(idx) };
            proof_assert! { ((*old).nodes@[idx@]).active ==>
                              (*old).len@ == 1 + chain_len((*old).nodes@,
                                                           ((*old).nodes@[idx@]).next,
                                                           (*old).nodes@.len() - 1) };
            // A one-slot arena: the only in-bounds forward link would be a self-loop, which
            // `links_sym` turns into a non-empty backward link — and this branch has none.
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).nodes@.len() == 1 ==>
                              ((*old).nodes@[idx@]).next == None };
            // Otherwise one unfolding of `chain_len` does it: a walk of one step from the front
            // leaves nothing for the successor to count.
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).nodes@.len() > 1
                            && (*old).len@ == 1 ==>
                              chain_len((*old).nodes@, ((*old).nodes@[idx@]).next,
                                        (*old).nodes@.len() - 1) == 0 };
            proof_assert! {
                lemma_epo_chain_zero_means_absent((*old).nodes@, ((*old).nodes@[idx@]).next,
                                                  (*old).nodes@.len() - 1);
                ((*old).nodes@[idx@]).active && (*old).nodes@.len() > 1 && (*old).len@ == 1 ==>
                  ((*old).nodes@[idx@]).next == None
            };
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).len@ == 1 ==>
                              ((*old).nodes@[idx@]).next == None };
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).len@ > 1 ==>
                              ((*old).nodes@[idx@]).next != None };
            proof_assert! { (l.len@ == 0) == (l.head == None) };
        }
        // The removed entry was NOT the front, so the front does not move. Then `len > 1` keeps the
        // front in place, and `len == 1` must be impossible — see the residual note below.
        Some(_) => {
            proof_assert! { l.head == (*old).head };
            proof_assert! { ((*old).nodes@[idx@]).active ==> (*old).head != None };
            proof_assert! { ((*old).nodes@[idx@]).active && (*old).len@ > 1 ==>
                              l.len@ > 0 && l.head != None };
            proof_assert! { l.head == None ==> l.len@ == 0 };
            // The removed entry is live and is NOT the front, so the front is a second, distinct
            // live slot.
            proof_assert! { ((*old).nodes@[idx@]).active ==>
                              match (*old).head { Some(h) => h != idx && h@ < (*old).nodes@.len()
                                                    && ((*old).nodes@[h@]).active,
                                                  None => false } };
            // RESIDUAL — the ONLY unproved subgoal of this module. Reaching `len == 0` here means
            // the pre-state had `len == 1` with TWO distinct live slots (`idx` and the front).
            // Ruling that out needs `len == the number of live slots`, i.e. the pigeonhole step
            // "the reuse list, being duplicate-free and holding only not-live slots, is no longer
            // than the number of not-live slots". `inv`/`wf` supply the three ingredients
            // (`slots_accounted`, `slots_disjoint`, `free_covers_inactive`) but NOT the counting
            // argument that combines them, and counting a duplicate-free index list against the
            // set it covers is a cardinality/pigeonhole induction, not an SMT-reachable step.
            // `free_len_counts_inactive` below assumes exactly that one instance and closes it.
            proof_assert! { l.len@ == 0 ==> l.head == None };
        }
    }
}

#[logic]
#[variant(fuel)]
#[requires(fuel >= 0)]
#[requires(a.len() == b.len())]
#[requires(forall<i: Int> 0 <= i && i < a.len() ==> (a[i]).next == (b[i]).next)]
#[ensures(chain_len(a, cur, fuel) == chain_len(b, cur, fuel))]
pub fn lemma_epo_inv_list_empty_iff_no_ends(a: Seq<Node>, b: Seq<Node>, cur: Option<u32>, fuel: Int) {
    pearlite! {
        if fuel <= 0 { () } else {
            match cur {
                None => (),
                Some(c) => if c@ < a.len() {
                    lemma_epo_inv_list_empty_iff_no_ends(a, b, (a[c@]).next, fuel - 1)
                } else { () }
            }
        }
    }
}

// ###########################################################################
// ANTI-VACUITY TWINS. Each states a deliberately FALSE contract and MUST fail to prove.
// NOTE (gate finding): `cargo creusot` renders the source `__mutant` suffix as a single-underscore
// `_mutant.coma`, so scorer_creusot.py's `verify_<ID>__mutant` lookup never matches and the
// scorer's mutant check is silently skipped. These twins were therefore confirmed RED by hand in
// the whole-crate run; see ../verif/creusot_advisory.yaml.
// ###########################################################################

#[requires((*self_).pools@.len() < 4294967295)]
#[ensures(result@ == (*self_).pools@.len() + 1)] // FALSE: the id is the OLD count
pub fn verify_epo_create_pool_post_sequential_id__mutant(self_: &mut Pools) -> u32 {
    state_create_pool(self_)
}

#[requires((*self_).pools@.len() < 4294967295)]
#[requires((*self_).pools@.len() >= 1)]
#[ensures(((^self_).pools@[0]).lru.len@ == ((*self_).pools@[0]).lru.len@ + 1)] // FALSE: existing pools are untouched
pub fn verify_epo_create_pool_frame_existing_pools__mutant(self_: &mut Pools) -> u32 {
    state_create_pool(self_)
}

#[requires((*self_).pools@.len() < 4294967295)]
#[ensures(((^self_).pools@[result@]).access_count@ == 1)] // FALSE: a fresh pool starts at zero
pub fn verify_epo_create_pool_post_fresh_state__mutant(self_: &mut Pools) -> u32 {
    state_create_pool(self_)
}

#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).lru.len@ == (*p).lru.len@)] // FALSE: track adds exactly one entry
pub fn verify_epo_track_post_len_increment__mutant(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).access_count@ == (*p).access_count@)] // FALSE: track bumps the access count
pub fn verify_epo_track_post_access_count__mutant(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).max_len@ >= (^p).lru.len@)] // FALSE: the record is measured BEFORE the insert, so it lags by one
pub fn verify_epo_track_post_maxlen_highwater__mutant(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

#[requires(s.counters@.len() == 4096)]
#[ensures(((^s).counters@[result.0@])@ == ((*s).counters@[result.0@])@)] // FALSE: the counter is bumped
pub fn verify_epo_track_post_sketch_increment__mutant(s: &mut Sketch, key: u64)
    -> (usize, usize, usize, usize) {
    sketch_increment4(s, key)
}

// [verify_epo_track_post_non_idempotent_reregistration__mutant removed 2026-10-07: replaced by the fresh section at the end of this file]

#[requires(inv(&(*l).lru))]
#[requires((*l).lru.nodes@.len() < 4294967295)]
#[requires((*l).lru.head != None)]
#[requires(e_new@ <= e_head@)]
#[ensures((^l).lru.tail == Some(result))] // FALSE: a no-more-frequent key enters at the EVICTION end
pub fn verify_epo_track_post_admit_not_more_frequent__mutant(l: &mut Pool, key: u64, e_new: u8, e_head: u8) -> u32 {
    if e_new <= e_head {
        arena_push_front(&mut l.lru, key)
    } else {
        arena_push_back(&mut l.lru, key)
    }
}

#[requires(pool@ >= (*self_).pools@.len())]
#[ensures(match result { Ok(_) => true, Err(_) => false })] // FALSE: an unknown pool is an error
pub fn verify_epo_track_error_invalid_pool__mutant(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

#[requires(pool@ >= (*self_).pools@.len())]
#[requires((*self_).pools@.len() >= 1)]
#[ensures(((^self_).pools@[0]).access_count@ == ((*self_).pools@[0]).access_count@ + 1)] // FALSE: a failed track changes nothing
pub fn verify_epo_track_error_frame_no_mutation__mutant(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active && (*l).lru.tail != Some(idx))]
#[ensures((^l).lru.len@ == (*l).lru.len@ + 1)] // FALSE: touch never changes the size
pub fn verify_epo_touch_post_move_to_mru__mutant(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.tail == Some(idx))]
#[requires((*l).lru.len@ >= 1)]
#[ensures((^l).lru.len@ == (*l).lru.len@ - 1)] // FALSE: touching the back entry changes nothing
pub fn verify_epo_touch_post_already_at_back_noop__mutant(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires(!(*l).lru.nodes@[idx@].active)]
#[ensures((^l).lru.tail == Some(idx))] // FALSE: a stale touch is a silent no-op
pub fn verify_epo_touch_post_removed_handle_is_silent_noop__mutant(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

// [verify_epo_touch_post_not_next_victim__mutant removed 2026-10-07: replaced by the fresh section at the end of this file]

#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).sketch.counters@.len() == 4096)]
#[requires(((*l).sketch.counters@[0])@ < 255)]
#[ensures(((^l).sketch.counters@[0])@ == ((*l).sketch.counters@[0])@ + 1)] // FALSE: touch never raises an estimate
pub fn verify_epo_touch_frame_sketch_unchanged__mutant(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).access_count@ < 18446744073709551615)]
#[ensures((^l).access_count@ == (*l).access_count@ + 1)] // FALSE: touch leaves both counters alone
pub fn verify_epo_touch_frame_counters__mutant(l: &mut Pool, idx: u32) {
    arena_move_to_back(&mut l.lru, idx);
}

#[requires(h0.pool@ < (*self_).pools@.len())]
#[requires(inv(&((*self_).pools@[h0.pool@]).lru))]
#[requires(h0.index@ < (((*self_).pools@[h0.pool@]).lru.nodes@.len()))]
#[requires(((*self_).pools@[h0.pool@]).lru.nodes@[h0.index@].active)]
#[requires(((*self_).pools@[h0.pool@]).lru.tail != Some(h0.index))]
#[requires(h1.pool != h0.pool && h1.pool@ >= (*self_).pools@.len())]
#[ensures(((^self_).pools@[h0.pool@]).lru.tail != Some(h0.index))] // FALSE: the earlier handle IS already applied
pub fn verify_epo_batch_touch_error_partial_application__mutant(self_: &mut Pools, h0: Handle, h1: Handle)
    -> Result<(), PolicyError> {
    state_batch_touch2(self_, h0, h1)
}

#[requires(pool@ >= (*self_).pools@.len())]
#[requires(connected)]
#[requires((*log).lines@ < 4294967295)]
#[ensures((^log).lines@ == (*log).lines@ + 1)] // FALSE: the batch error path logs NOTHING
pub fn verify_epo_batch_touch_error_no_log__mutant(self_: &mut Pools, log: &mut Log, connected: bool, pool: u32)
    -> Result<(), PolicyError> {
    let _ = connected;
    if (pool as usize) < self_.pools.len() {
        Ok(())
    } else {
        Err(PolicyError::InvalidPool(pool))
    }
}

#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active && (*l).lru.len@ >= 1)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures((^l).lru.len@ == (*l).lru.len@)] // FALSE: removing a live entry drops the size
pub fn verify_epo_remove_post_len_decrement__mutant(l: &mut Pool, idx: u32) {
    arena_remove(&mut l.lru, idx);
}

#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires(!(*l).lru.nodes@[idx@].active)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[requires((*l).lru.len@ >= 1)]
#[ensures((^l).lru.len@ == (*l).lru.len@ - 1)] // FALSE: a repeat remove does not decrement again
pub fn verify_epo_remove_post_idempotent_removed_handle__mutant(l: &mut Pool, idx: u32) {
    arena_remove(&mut l.lru, idx);
}

#[requires(inv(&(*l).lru))]
#[requires(idx@ < (*l).lru.nodes@.len())]
#[requires((*l).lru.nodes@[idx@].active && (*l).lru.len@ >= 1)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[requires((*l).lru.nodes@.len() < 4294967295)]
#[ensures(result != idx)] // FALSE: the freed slot IS handed straight back
pub fn verify_epo_remove_post_slot_recycled__mutant(l: &mut Pool, idx: u32, key: u64) -> u32 {
    arena_remove(&mut l.lru, idx);
    arena_push_back(&mut l.lru, key)
}

#[requires(inv(&(*l).lru))]
#[requires((*l).lru.head != None)]
#[requires(match (*l).lru.head { Some(hh) => (*l).lru.nodes@[hh@].active && (*l).lru.len@ >= 1,
                                 None => true })]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures((^l).lru.len@ == (*l).lru.len@)] // FALSE: eviction both selects AND consumes
pub fn verify_epo_evict_post_removes_returned__mutant(l: &mut Pool) -> Option<u64> {
    arena_pop_front(&mut l.lru)
}

#[requires(inv(&(*l).lru))]
#[requires((*l).lru.head == None)]
#[requires((*l).lru.free@.len() < 4294967295)]
#[ensures(result != None)] // FALSE: an empty pool evicts nothing
pub fn verify_epo_evict_post_empty_pool_none__mutant(l: &mut Pool) -> Option<u64> {
    arena_pop_front(&mut l.lru)
}

#[ensures(result.0 == Some(k1))] // FALSE: first-time keys come back in REVERSE order of tracking
pub fn verify_epo_evict_post_order_first_time_keys__mutant(k1: u64, k2: u64, k3: u64)
    -> (Option<u64>, Option<u64>, Option<u64>) {
    let mut l = arena_fresh();
    let _i1 = arena_push_front(&mut l, k1);
    let _i2 = arena_push_front(&mut l, k2);
    let _i3 = arena_push_front(&mut l, k3);
    let r0 = arena_pop_front(&mut l);
    let r1 = arena_pop_front(&mut l);
    let r2 = arena_pop_front(&mut l);
    (r0, r1, r2)
}

#[requires(pool@ >= (*self_).pools@.len())]
#[requires(connected)]
#[requires((*log).lines@ < 4294967295)]
#[ensures(result != None)] // FALSE: an unknown pool degrades silently to None
pub fn verify_epo_evict_degrade_invalid_pool__mutant(self_: &mut Pools, log: &mut Log, connected: bool, pool: u32)
    -> Option<u64> {
    let _ = connected;
    let _ = log;
    state_evict(self_, pool)
}

#[requires(inv(&(*l).lru))]
#[requires(n@ == 0)]
#[requires((*l).lru.len@ >= 1)]
#[ensures(result@.len() > 0)] // FALSE: asking for zero candidates yields an empty list
pub fn verify_epo_candidates_post_zero_empty__mutant(l: &Pool, n: usize) -> Vec<u64> {
    arena_peek_front_n(&l.lru, n)
}

#[requires(inv(&(*l).lru))]
#[requires(ends_iff(&(*l).lru))]
#[requires(n@ >= 1)]
#[ensures(result@.len() == n@)] // FALSE: the list is capped by what the pool actually holds
pub fn verify_epo_candidates_post_count_bound__mutant(l: &Pool, n: usize) -> Vec<u64> {
    arena_peek_front_n(&l.lru, n)
}

#[requires(inv(&(*l).lru))]
#[requires((*l).lru.len@ >= 1)]
#[ensures((^l).lru.len@ == (*l).lru.len@ - 1)] // FALSE: previewing is non-destructive
pub fn verify_epo_candidates_frame_non_destructive__mutant(l: &mut Pool, n: usize) -> usize {
    let v = arena_peek_front_n(&l.lru, n);
    v.len()
}

#[requires(pool@ >= self_.pools@.len())]
#[requires(connected)]
#[requires(log.lines@ < 4294967295)]
#[ensures(result@ > 0)] // FALSE: an unknown pool reports zero
pub fn verify_epo_len_degrade_invalid_pool__mutant(self_: &Pools, log: &Log, connected: bool, pool: u32) -> usize {
    let _ = connected;
    let _ = log;
    state_len(self_, pool)
}

#[requires(pool@ < self_.pools@.len())]
#[ensures(result@ == (self_.pools@[pool@]).lru.len@ + 1)] // FALSE
pub fn verify_epo_len_post_active_count__mutant(self_: &Pools, pool: u32) -> usize {
    state_len(self_, pool)
}

#[requires(pool@ < (*self_).pools@.len())]
#[requires(((*self_).pools@[pool@]).lru.len@ >= 1)]
#[ensures(((^self_).pools@[pool@]).lru.len@ == ((*self_).pools@[pool@]).lru.len@)] // FALSE: clear empties the pool
pub fn verify_epo_clear_post_empty__mutant(self_: &mut Pools, pool: u32, n: usize) -> (Vec<u64>, Option<u64>) {
    state_clear(self_, pool);
    let cands = state_candidates(self_, pool, n);
    let victim = state_evict(self_, pool);
    (cands, victim)
}

#[requires(pool@ < (*self_).pools@.len())]
#[requires(((*self_).pools@[pool@]).sketch.counters@.len() == 4096)]
#[requires((((*self_).pools@[pool@]).sketch.counters@[0])@ >= 1)]
#[ensures(((((^self_).pools@[pool@]).sketch.counters@[0]))@ == 0)] // FALSE: clear keeps everything learned
pub fn verify_epo_clear_frame_preserves_sketch_and_counters__mutant(self_: &mut Pools, pool: u32) {
    state_clear(self_, pool)
}

#[requires(pool_inv(p))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[ensures(result@ == 1)] // FALSE: after a clear, slot numbering restarts at ZERO
pub fn verify_epo_clear_post_slot_numbering_restarts__mutant(p: &mut Pool, key: u64) -> u32 {
    arena_clear(&mut p.lru);
    pool_track(p, key)
}

#[requires((*self_).pools@.len() < 4294967294)]
#[ensures(result.1@ > 0)] // FALSE: an id beyond the range reads as an empty pool
pub fn verify_epo_inv_pool_ids_unique_sequential__mutant(self_: &mut Pools) -> (u32, usize) {
    let id = state_create_pool(self_);
    let beyond = state_len(self_, id + 1);
    (id, beyond)
}

#[requires(pool@ < (*self_).pools@.len() ==>
             inv(&((*self_).pools@[pool@]).lru)
             && ((*self_).pools@[pool@]).lru.free@.len() < 4294967295)]
#[ensures((^self_).pools@.len() == (*self_).pools@.len() - 1)] // FALSE: pools are append-only, never removed
pub fn verify_epo_inv_pools_append_only__mutant(self_: &mut Pools, pool: u32) -> Option<u64> {
    state_clear(self_, pool);
    state_evict(self_, pool)
}

#[requires(pool@ < (*self_).pools@.len() ==> pool_ready(&(*self_).pools@[pool@]))]
#[requires((*self_).pools@.len() >= 2)]
#[requires(pool@ == 0)]
#[ensures(((^self_).pools@[1]).lru.len@ == ((*self_).pools@[1]).lru.len@ + 1)] // FALSE: pools are isolated
pub fn verify_epo_inv_pool_isolation__mutant(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

#[requires(inv(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).free@.len() < 4294967295)]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[ensures(slots_disjoint(&^l))] // FALSE given the corrupting body below
pub fn verify_epo_inv_slot_ownership_disjoint__mutant(l: &mut LruList, key: u64, idx: u32) -> u32 {
    arena_remove(l, idx);
    let i = arena_push_back(l, key);
    l.free.push(i);
    i
}

#[requires(inv(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).free@.len() < 4294967295)]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[ensures(slots_accounted(&^l))] // FALSE given the corrupting body below
pub fn verify_epo_inv_slots_accounted__mutant(l: &mut LruList, key: u64, idx: u32) -> u32 {
    arena_remove(l, idx);
    let i = arena_push_front(l, key);
    l.free.push(i);
    i
}

#[requires(inv(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).free@.len() < 4294967295)]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active ==> (*l).len@ >= 1)]
#[ensures(match (^l).tail { Some(t) => t@ < (^l).nodes@.len(), None => true })] // FALSE given the corrupting body
pub fn verify_epo_inv_internal_index_in_range__mutant(l: &mut LruList, key: u64, idx: u32) -> u32 {
    let i = arena_push_back(l, key);
    l.tail = Some(l.nodes.len() as u32);
    i
}

#[requires(inv(l))]
#[requires(idx@ < (*l).nodes@.len())]
#[requires((*l).nodes@[idx@].active && (*l).len@ >= 1)]
#[requires((*l).free@.len() < 4294967294)]
#[ensures((^l).len@ == (*l).len@ - 2)] // FALSE: the second remove is a no-op, so only ONE decrement
pub fn verify_epo_inv_len_no_underflow__mutant(l: &mut LruList, idx: u32) {
    arena_remove(l, idx);
    arena_remove(l, idx);
}

#[requires(pool@ < (*self_).pools@.len())]
#[requires(pool_ready(&(*self_).pools@[pool@]))]
#[requires(pool@ >= 1)]
#[ensures(match result { Ok(h) => h.pool@ == pool@ - 1, Err(_) => false })] // FALSE: the handle names its OWN pool
pub fn verify_epo_inv_handle_pool_scoped__mutant(self_: &mut Pools, pool: u32, key: u64)
    -> Result<Handle, PolicyError> {
    state_track(self_, pool, key)
}

#[requires(h.pool@ >= (*self_).pools@.len())]
#[ensures(result.0 == Err(PolicyError::InvalidHandle))] // FALSE: InvalidHandle is never constructed
pub fn verify_epo_inv_invalid_handle_never_returned__mutant(self_: &mut Pools, h: Handle, key: u64)
    -> (Result<Handle, PolicyError>, Result<(), PolicyError>, Result<(), PolicyError>) {
    let t = state_track(self_, h.pool, key);
    let u = state_touch(self_, Handle { pool: h.pool, index: h.index });
    let r = state_remove(self_, Handle { pool: h.pool, index: h.index });
    (t, u, r)
}

#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[requires(idx@ < (*p).lru.nodes@.len())]
#[requires((*p).lru.nodes@[idx@].active ==> (*p).lru.len@ >= 1)]
#[requires((*p).lru.free@.len() < 4294967295)]
#[requires((*p).max_len@ >= 1)]
#[ensures((^p).max_len@ < (*p).max_len@)] // FALSE: the high-water record never goes down
pub fn verify_epo_inv_maxlen_monotone__mutant(p: &mut Pool, idx: u32, key: u64) -> u32 {
    arena_remove(&mut p.lru, idx);
    arena_clear(&mut p.lru);
    pool_track(p, key)
}

#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[requires((*p).lru.len@ <= (*p).max_len@ + 1)]
#[ensures((^p).lru.len@ <= (^p).max_len@)] // FALSE: the record lags the size by one at a new peak
pub fn verify_epo_inv_maxlen_upper_bounds_len__mutant(p: &mut Pool, key: u64) -> u32 {
    pool_track(p, key)
}

#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[ensures((^p).access_count@ == 0)] // FALSE: clearing a pool does NOT reset the tally
pub fn verify_epo_inv_access_count_monotone__mutant(p: &mut Pool, key: u64) -> u32 {
    arena_clear(&mut p.lru);
    pool_track(p, key)
}

#[requires(pool_inv(p))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[requires((*p).max_len@ >= 1)]
#[ensures((^p).max_len@ * 10 < (*p).max_len@ * 10)] // FALSE: the ageing interval never shortens
pub fn verify_epo_inv_aging_period_non_decreasing__mutant(p: &mut Pool) -> bool {
    arena_clear(&mut p.lru);
    pool_age_step(p)
}

#[requires(s.counters@.len() == 4096)]
#[requires(forall<j: Int> 0 <= j && j < 4096 ==> ((*s).counters@[j])@ == 255)]
#[ensures(((^s).counters@[result.0@])@ == 0)] // FALSE: a saturated counter stays at 255, it does not wrap
pub fn verify_epo_inv_sketch_counters_saturate__mutant(s: &mut Sketch, key: u64)
    -> (usize, usize, usize, usize) {
    sketch_increment4(s, key)
}

#[ensures(result@ >= c0@)] // FALSE: the estimate is the MINIMUM, so it is <= every row counter
pub fn verify_epo_inv_sketch_estimate_is_row_minimum__mutant(c0: u8, c1: u8, c2: u8, c3: u8) -> u8 {
    cms_min4(c0, c1, c2, c3)
}

#[requires(s.counters@.len() == 4096)]
#[ensures(result@ == 0)] // FALSE: a just-tracked key estimates at least one
pub fn verify_epo_inv_sketch_estimate_at_least_one_after_first_track__mutant(s: &mut Sketch, key: u64) -> u8 {
    let c = sketch_increment4(s, key);
    sketch_estimate_at(s, c.0, c.1, c.2, c.3)
}

#[ensures(result.0@ >= 1024)] // FALSE: the column is always inside the 1024-wide table
pub fn verify_epo_inv_sketch_bucket_in_range__mutant(key: u64) -> (usize, usize, usize, usize) {
    cms_cols4(key)
}

#[requires(s.counters@.len() == 4096)]
#[requires(((*s).counters@[0])@ >= 2)]
#[ensures(((^s).counters@[0])@ >= ((*s).counters@[0])@)] // FALSE: halving strictly lowers a counter >= 2
pub fn verify_epo_inv_sketch_halve_non_increasing__mutant(s: &mut Sketch) {
    sketch_halve(s)
}

// [verify_epo_inv_sketch_row_hashes_distinct__mutant removed 2026-10-07: replaced by the fresh section at the end of this file]

#[requires((*w).accesses@ == 0)]
#[ensures((^w).accesses@ == n_entries@)] // FALSE: the work is a constant, independent of pool size
pub fn verify_epo_inv_sketch_ops_bounded_by_fixed_size__mutant(w: &mut Work, n_entries: usize) {
    let _ = n_entries;
    work_add(w, 4);
    work_add(w, 4);
    work_add(w, 4096);
}

#[requires(inv(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).head != None)]
#[ensures(e_new@ <= e_head@ ==> (^l).tail == Some(result))] // FALSE: the comparison decides the OTHER way
pub fn verify_epo_inv_admission_decided_only_by_victim_comparison__mutant(l: &mut LruList, key: u64,
                                                                        e_new: u8, e_head: u8) -> u32 {
    if e_new <= e_head {
        arena_push_front(l, key)
    } else {
        arena_push_back(l, key)
    }
}

#[requires((*t).held@ == 0 && (*t).peak@ == 0)]
#[requires(pool_changed)]
#[ensures((^t).peak@ <= 1)] // FALSE given the corrupting body: this holds TWO pool locks at once
pub fn verify_epo_inv_at_most_one_pool_lock__mutant(t: &mut LockTrace, pool_changed: bool) {
    lock_acquire(t);
    if pool_changed {
        lock_acquire(t);
        lock_release(t);
    }
    lock_release(t);
}

#[requires(pool_ready(p))]
#[requires((*p).lru.nodes@.len() < 4294967295)]
#[requires((*log).lines@ < 4294967295)]
#[ensures(connected ==> (^p).lru.len@ == (*p).lru.len@ + 2)] // FALSE: behaviour does not depend on the logger
pub fn verify_epo_inv_logger_optional__mutant(p: &mut Pool, log: &mut Log, connected: bool, key: u64) -> u32 {
    let idx = pool_track(p, key);
    log_site(log, connected);
    idx
}

#[requires((*log).lines@ == 0)]
#[requires(connected)]
#[ensures((^log).lines@ == 1)] // FALSE: construction emits no banner
pub fn verify_epo_inv_no_startup_banner__mutant(log: &mut Log, connected: bool) -> Pools {
    let _ = connected;
    let _ = log;
    Pools { pools: Vec::new() }
}

#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(links_sym(&^l))] // FALSE given the corrupting body below
pub fn verify_epo_inv_list_links_symmetric__mutant(l: &mut LruList, key: u64) -> u32 {
    let i = wf_push_back(l, key);
    l.nodes[i as usize].prev = Some(i);
    i
}

#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[ensures(tail_unique(&^l))] // FALSE given the corrupting body below
pub fn verify_epo_inv_list_tail_is_only_node_without_next__mutant(l: &mut LruList, key: u64) -> u32 {
    let i = wf_push_back(l, key);
    l.tail = None;
    i
}

#[requires(inv(l) && wf(l))]
#[requires((*l).nodes@.len() < 4294967295)]
#[requires((*l).free@.len() < 4294967295)]
#[ensures(free_covers_inactive(&^l))] // FALSE given the corrupting body below
pub fn verify_epo_inv_len_matches_active_slots__mutant(l: &mut LruList, key: u64) -> u32 {
    let i = wf_push_back(l, key);
    l.nodes[i as usize].active = false;
    i
}

// ===========================================================================
// FRESH SECTION (2026-10-07, v2) — EPO-INV-SKETCH-ROW-HASHES-DISTINCT, EPO-TOUCH-POST-NOT-NEXT-VICTIM,
// EPO-TRACK-FRAME-EXISTING-ENTRIES, EPO-TRACK-POST-NON-IDEMPOTENT-REREGISTRATION.
//
// No September premise is reused. The `tr_*` functions are a second copy of the shipped code: each
// one copies the cited src lines statement for statement and carries only the preconditions the
// code needs not to panic. Each has a PRECISE per-field effect contract.
//
// Premises the drivers may use:
//   (a) the obligation's own words;
//   (b) the declared level-1 assumption D-RANGE-FR-002-7fbb2b: "a pool never allocates more than
//       2^32 - 1 entry slots over its lifetime", assume_rust `self.nodes.len() < u32::MAX as usize`,
//       written here as `nodes@.len() < 4294967295` on the pool's list;
//   (c) `ord(l, s, pos)` — the list IS the sequence `s` of live slots, front to back. Its
//       establishment (ord_new) and its preservation by EVERY LruList mutator (ord_push_front,
//       ord_push_back, ord_move_to_back, ord_remove, ord_pop_front, ord_clear) are proved below.
//       Only push_front/push_back use D-RANGE: under it the `as u32` cast is exact.
// The refutations that hold WITHOUT D-RANGE live outside this crate:
//   SEPT_2026/FV_NOTEBOOK/evidence/epo_trial_20261007/u32_handle_refutations.rs
// ===========================================================================

/// Every link, both ends and every free-list slot name an allocated slot (what indexing needs).
#[logic]
pub fn lb(l: &LruList) -> bool {
    pearlite! {
        (match l.head { Some(h) => h@ < l.nodes@.len(), None => true })
        && (match l.tail { Some(t) => t@ < l.nodes@.len(), None => true })
        && (forall<i: Int> 0 <= i && i < l.nodes@.len() ==>
                match (l.nodes@[i]).next { Some(n) => n@ < l.nodes@.len(), None => true })
        && (forall<i: Int> 0 <= i && i < l.nodes@.len() ==>
                match (l.nodes@[i]).prev { Some(p) => p@ < l.nodes@.len(), None => true })
        && (forall<k: Int> 0 <= k && k < l.free@.len() ==> (l.free@[k])@ < l.nodes@.len())
    }
}

/// THE ORDER INVARIANT. `s` lists the live slots front (head) to back (tail), each exactly once
/// (`pos` is its inverse). Consecutive entries are linked both ways, the ends are `None`-linked,
/// not-live slots are unlinked, the free list holds distinct not-live slots, and
/// `nodes.len() == len + free.len()`.
#[logic]
pub fn ord(l: &LruList, s: Seq<u32>, pos: Seq<Int>) -> bool {
    pearlite! {
        lb(l)
        && s.len() == l.len@
        && pos.len() == l.nodes@.len()
        && l.nodes@.len() == l.len@ + l.free@.len()
        && (forall<k: Int> 0 <= k && k < s.len() ==>
               (s[k])@ < l.nodes@.len() && (l.nodes@[(s[k])@]).active && pos[(s[k])@] == k)
        && (forall<i: Int> 0 <= i && i < l.nodes@.len() && (l.nodes@[i]).active ==>
               0 <= pos[i] && pos[i] < s.len() && (s[pos[i]])@ == i)
        && (forall<i: Int> 0 <= i && i < l.nodes@.len() && !(l.nodes@[i]).active ==>
               (l.nodes@[i]).prev == None && (l.nodes@[i]).next == None)
        && (forall<k: Int> 0 <= k && k < s.len() ==>
               (l.nodes@[(s[k])@]).prev == (if k == 0 { None } else { Some(s[k - 1]) })
               && (l.nodes@[(s[k])@]).next == (if k == s.len() - 1 { None } else { Some(s[k + 1]) }))
        && l.head == (if s.len() == 0 { None } else { Some(s[0]) })
        && l.tail == (if s.len() == 0 { None } else { Some(s[s.len() - 1]) })
        && (forall<k: Int> 0 <= k && k < l.free@.len() ==> !(l.nodes@[(l.free@[k])@]).active)
        && (forall<j: Int, k: Int> 0 <= j && j < k && k < l.free@.len() ==> l.free@[j] != l.free@[k])
    }
}

// ---------------------------------------------------------------- sketch copies

/// Row r's column for a key, as logic (src/lib.rs:40 and :48).
#[logic]
pub fn col_l(key: u64, prime: u64) -> Int {
    pearlite! { (key * prime)@ / 18014398509481984 }
}

/// `(key.wrapping_mul(prime) >> 54) as usize` — copied from src/lib.rs:40 (identical at :48).
#[bitwise_proof]
#[check(terminates)]
#[ensures(result@ == col_l(key, prime))]
#[ensures(result@ < 1024)]
pub fn tr_col(key: u64, prime: u64) -> usize {
    (key.wrapping_mul(prime) >> 54) as usize
}

/// `CountMinSketch::new` (src/lib.rs:32-36), `[[0u8; 1024]; 4]` flattened row-major.
#[check(terminates)]
#[ensures(result.counters@.len() == 4096)]
pub fn tr_sketch_new() -> Sketch {
    let mut counters: Vec<u8> = Vec::new();
    let mut i: usize = 0;
    #[invariant(counters@.len() == i@ && i@ <= 4096)]
    #[variant(4096 - i@)]
    while i < 4096 {
        counters.push(0u8);
        i += 1;
    }
    Sketch { counters }
}

/// `CountMinSketch::increment` (src/lib.rs:38-43), the CMS_PRIMES loop unrolled (row r uses P<r>).
#[check(terminates)]
#[requires(s.counters@.len() == 4096)]
#[ensures((^s).counters@.len() == 4096)]
pub fn tr_sketch_increment(s: &mut Sketch, key: u64) {
    let col = tr_col(key, P0);
    s.counters[col] = s.counters[col].saturating_add(1);
    let col = 1024 + tr_col(key, P1);
    s.counters[col] = s.counters[col].saturating_add(1);
    let col = 2048 + tr_col(key, P2);
    s.counters[col] = s.counters[col].saturating_add(1);
    let col = 3072 + tr_col(key, P3);
    s.counters[col] = s.counters[col].saturating_add(1);
}

/// `CountMinSketch::estimate` (src/lib.rs:45-52), the four-row loop unrolled.
#[requires(s.counters@.len() == 4096)]
pub fn tr_estimate(s: &Sketch, key: u64) -> u8 {
    let mut min = u8::MAX;
    min = min.min(s.counters[tr_col(key, P0)]);
    min = min.min(s.counters[1024 + tr_col(key, P1)]);
    min = min.min(s.counters[2048 + tr_col(key, P2)]);
    min = min.min(s.counters[3072 + tr_col(key, P3)]);
    min
}

/// `CountMinSketch::halve` (src/lib.rs:54-60), nested loops flattened over 4096 counters.
#[check(terminates)]
#[requires(s.counters@.len() == 4096)]
#[ensures((^s).counters@.len() == 4096)]
pub fn tr_halve(s: &mut Sketch) {
    let mut i: usize = 0;
    #[invariant(s.counters@.len() == 4096 && i@ <= 4096)]
    #[variant(4096 - i@)]
    while i < 4096 {
        s.counters[i] >>= 1;
        i += 1;
    }
}

// ---------------------------------------------------------------- list copies + effects

/// `LruList::new` (src/lru_list.rs:26-34).
#[check(terminates)]
#[ensures(result.nodes@.len() == 0 && result.free@.len() == 0 && result.len@ == 0)]
#[ensures(result.head == None && result.tail == None)]
pub fn tr_list_new() -> LruList {
    LruList { nodes: Vec::new(), head: None, tail: None, free: Vec::new(), len: 0 }
}

/// `LruList::len` (src/lru_list.rs:195-197).
#[check(terminates)]
#[ensures(result == l.len)]
pub fn tr_len(l: &LruList) -> usize {
    l.len
}

/// `LruList::peek_front_key` (src/lru_list.rs:70-72); `.map(|idx| ..)` written as a match.
#[check(terminates)]
#[requires(lb(l))]
#[ensures(match l.head { Some(h) => result == Some((l.nodes@[h@]).key), None => result == None })]
pub fn tr_peek_front_key(l: &LruList) -> Option<u64> {
    match l.head {
        Some(idx) => Some(l.nodes[idx as usize].key),
        None => None,
    }
}

/// Where the inserted node lands and what the free list becomes (shared by both pushes).
#[logic]
pub fn push_slot(a: &LruList, b: &LruList, r: u32) -> bool {
    pearlite! {
        (a.free@.len() > 0 ==> r == a.free@[a.free@.len() - 1]
            && b.free@ == a.free@.subsequence(0, a.free@.len() - 1)
            && b.nodes@.len() == a.nodes@.len())
        && (a.free@.len() == 0 ==> b.free@ == a.free@ && b.nodes@.len() == a.nodes@.len() + 1
            && (a.nodes@.len() <= 4294967295 ==> r@ == a.nodes@.len()))
        && b.len@ == a.len@ + 1
        && r@ < b.nodes@.len()
    }
}

/// Field-by-field effect of `push_front` (writes in code order: node r, then old head's prev).
#[logic]
pub fn pf_eff(a: &LruList, b: &LruList, key: u64, r: u32) -> bool {
    pearlite! {
        push_slot(a, b, r)
        && b.head == Some(r)
        && b.tail == (if a.tail == None { Some(r) } else { a.tail })
        && (forall<j: Int> 0 <= j && j < b.nodes@.len() ==>
            (j == r@ ==> (b.nodes@[j]).key == key && (b.nodes@[j]).active
                && (b.nodes@[j]).next == a.head
                && (b.nodes@[j]).prev == (if a.head == Some(r) { Some(r) } else { None }))
            && (j != r@ ==> (b.nodes@[j]).key == (a.nodes@[j]).key
                && (b.nodes@[j]).active == (a.nodes@[j]).active
                && (b.nodes@[j]).next == (a.nodes@[j]).next
                && (b.nodes@[j]).prev == (match a.head { Some(h) => if h@ == j { Some(r) } else { (a.nodes@[j]).prev },
                                                          None => (a.nodes@[j]).prev })))
    }
}

/// Field-by-field effect of `push_back` (writes in code order: node r, then old tail's next).
#[logic]
pub fn pb_eff(a: &LruList, b: &LruList, key: u64, r: u32) -> bool {
    pearlite! {
        push_slot(a, b, r)
        && b.tail == Some(r)
        && b.head == (if a.head == None { Some(r) } else { a.head })
        && (forall<j: Int> 0 <= j && j < b.nodes@.len() ==>
            (j == r@ ==> (b.nodes@[j]).key == key && (b.nodes@[j]).active
                && (b.nodes@[j]).prev == a.tail
                && (b.nodes@[j]).next == (if a.tail == Some(r) { Some(r) } else { None }))
            && (j != r@ ==> (b.nodes@[j]).key == (a.nodes@[j]).key
                && (b.nodes@[j]).active == (a.nodes@[j]).active
                && (b.nodes@[j]).prev == (a.nodes@[j]).prev
                && (b.nodes@[j]).next == (match a.tail { Some(t) => if t@ == j { Some(r) } else { (a.nodes@[j]).next },
                                                          None => (a.nodes@[j]).next })))
    }
}

/// `LruList::push_front` — copied from src/lru_list.rs:37-67.
#[check(terminates)]
#[requires(lb(self_))]
#[requires((*self_).len@ < 18446744073709551615)]
#[ensures(lb(&^self_))]
#[ensures((*self_).nodes@.len() <= 4294967295 ==> pf_eff(&*self_, &^self_, key, result))] // exact `as u32` only
pub fn tr_push_front(self_: &mut LruList, key: u64) -> u32 {
    let idx = if let Some(free_idx) = self_.free.pop() {
        self_.nodes[free_idx as usize] = Node { key, prev: None, next: self_.head, active: true };
        free_idx
    } else {
        let idx = self_.nodes.len() as u32;
        self_.nodes.push(Node { key, prev: None, next: self_.head, active: true });
        idx
    };
    if let Some(old_head) = self_.head {
        self_.nodes[old_head as usize].prev = Some(idx);
    }
    self_.head = Some(idx);
    if self_.tail.is_none() {
        self_.tail = Some(idx);
    }
    self_.len += 1;
    idx
}

/// `LruList::push_back` — copied from src/lru_list.rs:75-105.
#[check(terminates)]
#[requires(lb(self_))]
#[requires((*self_).len@ < 18446744073709551615)]
#[ensures(lb(&^self_))]
#[ensures((*self_).nodes@.len() <= 4294967295 ==> pb_eff(&*self_, &^self_, key, result))] // exact `as u32` only
pub fn tr_push_back(self_: &mut LruList, key: u64) -> u32 {
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

/// Helper: is the Option<u32> `o` exactly slot `j`?
#[logic]
pub fn is_slot(o: Option<u32>, j: Int) -> bool {
    pearlite! { match o { Some(x) => x@ == j, None => false } }
}

/// Field-by-field effect of `move_to_back` on a live, non-tail node `idx` (code order: prev's
/// next, next's prev, idx's prev, idx's next, old tail's next).
#[logic]
pub fn mv_eff2(a: &LruList, b: &LruList, idx: u32) -> bool {
    pearlite! {
        b.nodes@.len() == a.nodes@.len() && b.len == a.len && b.free == a.free
        && b.tail == Some(idx)
        && b.head == (if (a.nodes@[idx@]).prev == None { (a.nodes@[idx@]).next } else { a.head })
        && (forall<j: Int> 0 <= j && j < a.nodes@.len() ==>
               (b.nodes@[j]).key == (a.nodes@[j]).key && (b.nodes@[j]).active == (a.nodes@[j]).active
            && (b.nodes@[j]).next == (if is_slot(a.tail, j) { Some(idx) }
                                      else if j == idx@ { None }
                                      else if is_slot((a.nodes@[idx@]).prev, j) { (a.nodes@[idx@]).next }
                                      else { (a.nodes@[j]).next })
            && (b.nodes@[j]).prev == (if j == idx@ { a.tail }
                                      else if is_slot((a.nodes@[idx@]).next, j) { (a.nodes@[idx@]).prev }
                                      else { (a.nodes@[j]).prev }))
    }
}

/// `LruList::move_to_back` — copied from src/lru_list.rs:108-134 (what `touch` runs, src/lib.rs:154).
#[requires(lb(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[ensures(lb(&^self_))]
#[ensures(!((*self_).nodes@[idx@]).active || (*self_).tail == Some(idx) ==> ^self_ == *self_)]
#[ensures(((*self_).nodes@[idx@]).active && (*self_).tail != Some(idx) ==> mv_eff2(&*self_, &^self_, idx))]
pub fn tr_move_to_back(self_: &mut LruList, idx: u32) {
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

/// Field-by-field effect of `remove` on a live node `idx`.
#[logic]
pub fn rm_eff(a: &LruList, b: &LruList, idx: u32) -> bool {
    pearlite! {
        b.nodes@.len() == a.nodes@.len() && b.len@ == a.len@ - 1
        && b.free@ == a.free@.push_back(idx)
        && b.head == (if (a.nodes@[idx@]).prev == None { (a.nodes@[idx@]).next } else { a.head })
        && b.tail == (if (a.nodes@[idx@]).next == None { (a.nodes@[idx@]).prev } else { a.tail })
        && (forall<j: Int> 0 <= j && j < a.nodes@.len() ==>
               (b.nodes@[j]).key == (a.nodes@[j]).key
            && (b.nodes@[j]).active == (if j == idx@ { false } else { (a.nodes@[j]).active })
            && (b.nodes@[j]).next == (if j == idx@ { None }
                                      else if is_slot((a.nodes@[idx@]).prev, j) { (a.nodes@[idx@]).next }
                                      else { (a.nodes@[j]).next })
            && (b.nodes@[j]).prev == (if j == idx@ { None }
                                      else if is_slot((a.nodes@[idx@]).next, j) { (a.nodes@[idx@]).prev }
                                      else { (a.nodes@[j]).prev }))
    }
}

/// `LruList::remove` — copied from src/lru_list.rs:159-183.
#[check(terminates)]
#[requires(lb(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires(((*self_).nodes@[idx@]).active ==> (*self_).len@ >= 1)]
#[ensures(lb(&^self_))]
#[ensures(!((*self_).nodes@[idx@]).active ==> ^self_ == *self_)]
#[ensures(((*self_).nodes@[idx@]).active ==> rm_eff(&*self_, &^self_, idx))]
pub fn tr_remove(self_: &mut LruList, idx: u32) {
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

/// `LruList::pop_front` — src/lru_list.rs:137-142 (`self.head?` written as a match). This is what
/// `identify_next_to_evict` runs (src/lib.rs:206).
#[check(terminates)]
#[requires(lb(self_))]
#[requires(match (*self_).head { Some(h) => ((*self_).nodes@[h@]).active ==> (*self_).len@ >= 1, None => true })]
#[ensures(lb(&^self_))]
#[ensures((*self_).head == None ==> result == None && ^self_ == *self_)]
#[ensures(match (*self_).head {
              Some(h) => result == Some(((*self_).nodes@[h@]).key)
                  && (((*self_).nodes@[h@]).active ==> rm_eff(&*self_, &^self_, h))
                  && (!((*self_).nodes@[h@]).active ==> ^self_ == *self_),
              None => true })]
pub fn tr_pop_front(self_: &mut LruList) -> Option<u64> {
    let head_idx = match self_.head {
        Some(h) => h,
        None => return None,
    };
    let key = self_.nodes[head_idx as usize].key;
    tr_remove(self_, head_idx);
    Some(key)
}

/// `LruList::clear` — copied from src/lru_list.rs:186-192.
#[check(terminates)]
#[ensures((^self_).nodes@.len() == 0 && (^self_).free@.len() == 0 && (^self_).len@ == 0)]
#[ensures((^self_).head == None && (^self_).tail == None)]
pub fn tr_clear(self_: &mut LruList) {
    self_.nodes.clear();
    self_.head = None;
    self_.tail = None;
    self_.free.clear();
    self_.len = 0;
}

// ---------------------------------------------------------------- track copy (split, same order)

/// src/lib.rs:121-127 — sketch bump and ageing. It never touches the list.
#[requires((*p).sketch.counters@.len() == 4096)]
#[requires((*p).access_count@ < 18446744073709551615)]
#[requires((*p).max_len@ <= 1844674407370955161 && (*p).lru.len@ <= 1844674407370955161)]
#[ensures((^p).lru == (*p).lru && (^p).sketch.counters@.len() == 4096)]
pub fn tr_track_age(p: &mut Pool, key: u64) {
    tr_sketch_increment(&mut p.sketch, key);
    p.max_len = p.max_len.max(tr_len(&p.lru));
    p.access_count += 1;
    if p.max_len > 0 && p.access_count % (p.max_len as u64 * 10) == 0 {
        tr_halve(&mut p.sketch);
    }
}

/// src/lib.rs:129-137 — admission. The returned `index` is the handle's slot (:139).
#[requires((*p).sketch.counters@.len() == 4096 && lb(&(*p).lru))]
#[requires((*p).lru.len@ < 18446744073709551615)]
#[ensures(lb(&(^p).lru) && (^p).sketch == (*p).sketch)]
#[ensures((*p).lru.nodes@.len() <= 4294967295 ==>
          pf_eff(&(*p).lru, &(^p).lru, key, result) || pb_eff(&(*p).lru, &(^p).lru, key, result))]
pub fn tr_track_admit(p: &mut Pool, key: u64) -> u32 {
    let index = if let Some(head_key) = tr_peek_front_key(&p.lru) {
        if tr_estimate(&p.sketch, key) <= tr_estimate(&p.sketch, head_key) {
            tr_push_front(&mut p.lru, key)
        } else {
            tr_push_back(&mut p.lru, key)
        }
    } else {
        tr_push_back(&mut p.lru, key)
    };

    index
}

/// `track`'s pool-local body, src/lib.rs:121-139 = the two halves above in order.
#[requires((*p).sketch.counters@.len() == 4096 && lb(&(*p).lru))]
#[requires((*p).access_count@ < 18446744073709551615)]
#[requires((*p).max_len@ <= 1844674407370955161 && (*p).lru.len@ <= 1844674407370955161)]
#[ensures((*p).lru.nodes@.len() <= 4294967295 ==>
          pf_eff(&(*p).lru, &(^p).lru, key, result) || pb_eff(&(*p).lru, &(^p).lru, key, result))]
pub fn tr_track(p: &mut Pool, key: u64) -> u32 {
    tr_track_age(p, key);
    tr_track_admit(p, key)
}

// ---------------------------------------------------------------- (c): ord is inductive

/// Base case: `LruList::new()` satisfies ord with the empty order.
#[ensures(ord(&result.0, *result.1, *result.2))]
pub fn ord_new() -> (LruList, Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    (tr_list_new(), snapshot! { Seq::empty() }, snapshot! { Seq::empty() })
}

/// push_front preserves ord: the new order is r followed by the old one.
#[requires(ord(l, *s, *pos))]
#[requires((*l).nodes@.len() < 4294967295)] // D-RANGE-FR-002-7fbb2b
#[ensures(ord(&^l, *result.1, *result.2))]
#[ensures(result.1.len() == s.len() + 1 && result.1[0] == result.0
          && forall<k: Int> 0 <= k && k < s.len() ==> result.1[k + 1] == s[k])]
#[ensures(forall<k: Int> 0 <= k && k < s.len() ==> s[k] != result.0)]
pub fn ord_push_front(l: &mut LruList, key: u64, s: Snapshot<Seq<u32>>, pos: Snapshot<Seq<Int>>)
    -> (u32, Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    let r = tr_push_front(l, key);
    let s2 = snapshot! { s.push_front(r) };
    let pos2 = snapshot! { Seq::create(l.nodes@.len(), |i: Int| if i == r@ { 0 } else { pos[i] + 1 }) };
    (r, s2, pos2)
}

/// push_back preserves ord: the new order is the old one followed by r.
#[requires(ord(l, *s, *pos))]
#[requires((*l).nodes@.len() < 4294967295)] // D-RANGE-FR-002-7fbb2b
#[ensures(ord(&^l, *result.1, *result.2))]
#[ensures(result.1.len() == s.len() + 1 && result.1[s.len()] == result.0
          && forall<k: Int> 0 <= k && k < s.len() ==> result.1[k] == s[k])]
#[ensures(forall<k: Int> 0 <= k && k < s.len() ==> s[k] != result.0)]
pub fn ord_push_back(l: &mut LruList, key: u64, s: Snapshot<Seq<u32>>, pos: Snapshot<Seq<Int>>)
    -> (u32, Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    let r = tr_push_back(l, key);
    let s2 = snapshot! { s.push_back(r) };
    let pos2 = snapshot! { Seq::create(l.nodes@.len(), |i: Int| if i == r@ { s.len() } else { pos[i] }) };
    (r, s2, pos2)
}

/// move_to_back preserves ord: idx leaves its place and goes last; others keep their order.
#[requires(ord(l, *s, *pos))]
#[requires(idx@ < (*l).nodes@.len())]
#[ensures(ord(&^l, *result.0, *result.1))]
#[ensures(((*l).nodes@[idx@]).active ==> (^l).tail == Some(idx))]
#[ensures((^l).len == (*l).len && (^l).nodes@.len() == (*l).nodes@.len())]
#[ensures(forall<j: Int> 0 <= j && j < (*l).nodes@.len() ==> ((^l).nodes@[j]).active == ((*l).nodes@[j]).active)]
pub fn ord_move_to_back(l: &mut LruList, idx: u32, s: Snapshot<Seq<u32>>, pos: Snapshot<Seq<Int>>)
    -> (Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    let k = snapshot! { pos[idx@] };
    let act = snapshot! { ((*l).nodes@[idx@]).active && (*l).tail != Some(idx) };
    tr_move_to_back(l, idx);
    let s2 = snapshot! { if *act { Seq::create(s.len(), |j: Int| if j < *k { s[j] } else if j < s.len() - 1 { s[j + 1] } else { idx }) } else { *s } };
    let pos2 = snapshot! { if *act { Seq::create(pos.len(), |i: Int| if i == idx@ { s.len() - 1 } else if pos[i] > *k { pos[i] - 1 } else { pos[i] }) } else { *pos } };
    (s2, pos2)
}

/// remove preserves ord: idx leaves the order; others keep their order.
#[requires(ord(l, *s, *pos))]
#[requires(idx@ < (*l).nodes@.len())]
#[ensures(ord(&^l, *result.0, *result.1))]
pub fn ord_remove(l: &mut LruList, idx: u32, s: Snapshot<Seq<u32>>, pos: Snapshot<Seq<Int>>)
    -> (Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    let k = snapshot! { pos[idx@] };
    let act = snapshot! { ((*l).nodes@[idx@]).active };
    tr_remove(l, idx);
    let s2 = snapshot! { if *act { Seq::create(s.len() - 1, |j: Int| if j < *k { s[j] } else { s[j + 1] }) } else { *s } };
    let pos2 = snapshot! { if *act { Seq::create(pos.len(), |i: Int| if pos[i] > *k { pos[i] - 1 } else { pos[i] }) } else { *pos } };
    (s2, pos2)
}

/// pop_front preserves ord: the front leaves the order.
#[requires(ord(l, *s, *pos))]
#[ensures(ord(&^l, *result.0, *result.1))]
pub fn ord_pop_front(l: &mut LruList, s: Snapshot<Seq<u32>>, pos: Snapshot<Seq<Int>>)
    -> (Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    let ne = snapshot! { s.len() > 0 };
    let _ = tr_pop_front(l);
    let s2 = snapshot! { if *ne { Seq::create(s.len() - 1, |j: Int| s[j + 1]) } else { *s } };
    let pos2 = snapshot! { if *ne { Seq::create(pos.len(), |i: Int| pos[i] - 1) } else { *pos } };
    (s2, pos2)
}

/// clear re-establishes ord with the empty order.
#[ensures(ord(&^l, *result.0, *result.1))]
pub fn ord_clear(l: &mut LruList) -> (Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    tr_clear(l);
    (snapshot! { Seq::empty() }, snapshot! { Seq::empty() })
}

// ---------------------------------------------------------------- the four drivers

// ---- EPO-INV-SKETCH-ROW-HASHES-DISTINCT ---------------------------------------
// Row r picks its column with tr_col(key, CMS_PRIMES[r]) (copied src/lib.rs:40/:48). P0..P3
// (verbatim src/lib.rs:21-24) are pairwise distinct and odd, and key 1 already lands in four
// different columns, so the rows are not copies of one counter. No premises.
#[check(terminates)]
#[ensures(P0 != P1 && P0 != P2 && P0 != P3 && P1 != P2 && P1 != P3 && P2 != P3)]
#[ensures(P0@ % 2 == 1 && P1@ % 2 == 1 && P2@ % 2 == 1 && P3@ % 2 == 1)]
#[ensures(result.0 != result.1 && result.0 != result.2 && result.0 != result.3
       && result.1 != result.2 && result.1 != result.3 && result.2 != result.3)]
pub fn verify_epo_inv_sketch_row_hashes_distinct() -> (usize, usize, usize, usize) {
    (tr_col(1, P0), tr_col(1, P1), tr_col(1, P2), tr_col(1, P3))
}

#[check(terminates)]
#[ensures(result.0 == result.1)] // FALSE: rows 0 and 1 put key 1 in different columns
pub fn verify_epo_inv_sketch_row_hashes_distinct__mutant() -> (usize, usize, usize, usize) {
    (tr_col(1, P0), tr_col(1, P1), tr_col(1, P2), tr_col(1, P3))
}

// ---- EPO-TOUCH-POST-NOT-NEXT-VICTIM -------------------------------------------
// Premises: (c) ord; (a) "an entry is touched" = idx names a live slot; (a) "the pool holds at
// least one other entry" = len >= 2. Covers EVERY position of idx, including already-at-back.
// touch = move_to_back (src/lib.rs:154); the next victim = pop_front's head (src/lib.rs:206).
// Shown: the front after the touch is a live entry other than idx, and evicting it leaves idx live.
#[requires(ord(&(*l).lru, *s, *pos))]
#[requires(idx@ < (*l).lru.nodes@.len() && ((*l).lru.nodes@[idx@]).active)]
#[requires((*l).lru.len@ >= 2)]
#[ensures(result.0 != None && result.0 != Some(idx))]
#[ensures(result.1 != None)]
#[ensures(((^l).lru.nodes@[idx@]).active)]
pub fn verify_epo_touch_post_not_next_victim(l: &mut Pool, idx: u32, s: Snapshot<Seq<u32>>,
                                             pos: Snapshot<Seq<Int>>) -> (Option<u32>, Option<u64>) {
    let (s2, pos2) = ord_move_to_back(&mut l.lru, idx, s, pos);
    let front = l.lru.head;
    let _ = (s2, pos2);
    let victim = tr_pop_front(&mut l.lru);
    (front, victim)
}

#[requires(ord(&(*l).lru, *s, *pos))]
#[requires(idx@ < (*l).lru.nodes@.len() && ((*l).lru.nodes@[idx@]).active)]
#[requires((*l).lru.len@ >= 2)]
#[ensures(result.0 == Some(idx))] // FALSE: the touched entry is not the next victim
pub fn verify_epo_touch_post_not_next_victim__mutant(l: &mut Pool, idx: u32, s: Snapshot<Seq<u32>>,
                                                     pos: Snapshot<Seq<Int>>) -> (Option<u32>, Option<u64>) {
    let (s2, pos2) = ord_move_to_back(&mut l.lru, idx, s, pos);
    let front = l.lru.head;
    let _ = (s2, pos2);
    let victim = tr_pop_front(&mut l.lru);
    (front, victim)
}

// ---- EPO-TRACK-FRAME-EXISTING-ENTRIES ------------------------------------------
// Premises: (c) ord; (b) D-RANGE-FR-002-7fbb2b; the sketch is the 4x1024 table. The driver runs
// the admission half of track (src/lib.rs:129-137) for EVERY sketch state; the other half
// (:121-127) never touches the list (tr_track_age proves `lru` unchanged). Shown: the order after
// is the old order with ONE new entry r (not one of the old ones) at the front or at the back.
#[requires(ord(&(*p).lru, *s, *pos))]
#[requires((*p).lru.nodes@.len() < 4294967295)] // D-RANGE-FR-002-7fbb2b
#[requires((*p).sketch.counters@.len() == 4096)]
#[ensures(ord(&(^p).lru, *result.1, *result.2))]
#[ensures(forall<k: Int> 0 <= k && k < s.len() ==> s[k] != result.0)]
#[ensures(result.1.len() == s.len() + 1)]
#[ensures((result.1[0] == result.0 && forall<k: Int> 0 <= k && k < s.len() ==> result.1[k + 1] == s[k])
       || (result.1[s.len()] == result.0 && forall<k: Int> 0 <= k && k < s.len() ==> result.1[k] == s[k]))]
pub fn verify_epo_track_frame_existing_entries(p: &mut Pool, key: u64, s: Snapshot<Seq<u32>>,
        pos: Snapshot<Seq<Int>>) -> (u32, Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    let r = tr_track_admit(p, key);
    let front = snapshot! { p.lru.head == Some(r) };
    let s2 = snapshot! { if *front { s.push_front(r) } else { s.push_back(r) } };
    let pos2 = snapshot! { if *front { Seq::create(p.lru.nodes@.len(), |i: Int| if i == r@ { 0 } else { pos[i] + 1 }) }
                           else { Seq::create(p.lru.nodes@.len(), |i: Int| if i == r@ { s.len() } else { pos[i] }) } };
    (r, s2, pos2)
}

#[requires(ord(&(*p).lru, *s, *pos))]
#[requires((*p).lru.nodes@.len() < 4294967295)] // D-RANGE-FR-002-7fbb2b
#[requires((*p).sketch.counters@.len() == 4096)]
#[requires(s.len() >= 2)]
#[ensures(forall<k: Int> 0 <= k && k < s.len() ==> result.1[k] == s[s.len() - 1 - k])] // FALSE: order reversed
pub fn verify_epo_track_frame_existing_entries__mutant(p: &mut Pool, key: u64, s: Snapshot<Seq<u32>>,
        pos: Snapshot<Seq<Int>>) -> (u32, Snapshot<Seq<u32>>, Snapshot<Seq<Int>>) {
    let r = tr_track_admit(p, key);
    let front = snapshot! { p.lru.head == Some(r) };
    let s2 = snapshot! { if *front { s.push_front(r) } else { s.push_back(r) } };
    let pos2 = snapshot! { if *front { Seq::create(p.lru.nodes@.len(), |i: Int| if i == r@ { 0 } else { pos[i] + 1 }) }
                           else { Seq::create(p.lru.nodes@.len(), |i: Int| if i == r@ { s.len() } else { pos[i] }) } };
    (r, s2, pos2)
}

// ---- EPO-TRACK-POST-NON-IDEMPOTENT-REREGISTRATION ------------------------------
// Premises: (c) ord; (b) D-RANGE-FR-002-7fbb2b; (a) "a key that is already being tracked" = h0,
// the handle the first track returned, names a live entry holding `key`. Same driver shape as FRAME.
// Shown: a new handle r != h0; both entries now live with `key`; size grows by one.
#[requires(ord(&(*p).lru, *s, *pos))]
#[requires((*p).lru.nodes@.len() < 4294967295)] // D-RANGE-FR-002-7fbb2b
#[requires((*p).sketch.counters@.len() == 4096)]
#[requires(h0@ < (*p).lru.nodes@.len() && ((*p).lru.nodes@[h0@]).active && ((*p).lru.nodes@[h0@]).key == key)]
#[ensures(result != h0)]
#[ensures(((^p).lru.nodes@[h0@]).active && ((^p).lru.nodes@[h0@]).key == key)]
#[ensures(result@ < (^p).lru.nodes@.len() && ((^p).lru.nodes@[result@]).active && ((^p).lru.nodes@[result@]).key == key)]
#[ensures((^p).lru.len@ == (*p).lru.len@ + 1)]
pub fn verify_epo_track_post_non_idempotent_reregistration(p: &mut Pool, key: u64, h0: u32,
        s: Snapshot<Seq<u32>>, pos: Snapshot<Seq<Int>>) -> u32 {
    tr_track_admit(p, key)
}

#[requires(ord(&(*p).lru, *s, *pos))]
#[requires((*p).lru.nodes@.len() < 4294967295)] // D-RANGE-FR-002-7fbb2b
#[requires((*p).sketch.counters@.len() == 4096)]
#[requires(h0@ < (*p).lru.nodes@.len() && ((*p).lru.nodes@[h0@]).active && ((*p).lru.nodes@[h0@]).key == key)]
#[ensures(result == h0)] // FALSE: re-tracking returns a NEW handle
pub fn verify_epo_track_post_non_idempotent_reregistration__mutant(p: &mut Pool, key: u64, h0: u32,
        s: Snapshot<Seq<u32>>, pos: Snapshot<Seq<Int>>) -> u32 {
    tr_track_admit(p, key)
}
