//! Creusot verification mirror for `components/eviction-policy-lru`.
//!
//! The shipped policy wraps its `LruList` in `RwLock<Vec<Mutex<...>>>` (lib.rs);
//! those cannot compile under Creusot. This crate proves a **standalone,
//! line-faithful mirror** of the pure arena data structure `../src/lru_list.rs`
//! — the index-based doubly-linked list that is the component's core.
//!
//! **What is proved here.** The arena's *memory-safety and cardinality* layer:
//!   * every `self.nodes[i]` index surgery stays in bounds (no OOB),
//!   * `len` bookkeeping is exact (±1 per op; `clear` → 0),
//!   * free-list recycling reuses a slot rather than growing `nodes` (FR-011),
//!   * idempotency of `remove`/`move_to_back` on inactive handles (FR-010).
//!
//! **What is NOT proved here (honest boundary).** LRU *ordering* correctness —
//! that `pop_front` returns the genuinely oldest key, that `move_to_back` makes
//! a node the most-recent, that the chain is acyclic — rests on a reachability
//! invariant over the `next`/`prev` links that is out of practical scope for the
//! SMT backend. In the same spirit as the dispatch-map mirror (which assumes the
//! condvar/refcount guards rather than re-deriving them), well-formedness of the
//! arena links is taken as an assumed precondition [`inv`]; these proofs are
//! therefore *conditional on* a well-formed arena. See `PROPERTIES.md`.
//!
//! **Drift discipline.** Each mirror body transcribes the cited source lines of
//! `../src/lru_list.rs`. Proofs are validated by fault injection (perturb a body,
//! confirm a VC goes red) — see `PROPERTIES.md`.

use creusot_std::prelude::*;

/// Mirror of `lru_list::Node`. `CacheKey` (`u64`) kept concrete; fields public
/// so the `inv` predicate can read the links.
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

// ---------------------------------------------------------------------------
// Well-formedness predicate — every stored index is a valid `nodes` index.
// This is what the shipped structure maintains by construction; here it is an
// ASSUMED precondition (a trusted boundary), giving the index accesses their
// in-bounds facts without a maintained global invariant. It says nothing about
// reachability / ordering.
// ---------------------------------------------------------------------------

/// The arena is *link-valid*: `head`, `tail`, every node's `prev`/`next`, and
/// every free-list slot are either `None` or an in-bounds `nodes` index, and
/// `len <= nodes.len()`.
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

// ===========================================================================
// len — FR-007. Unconditional: reads the counter.
// Mirror of `../src/lru_list.rs::len` (lines 157-159).
// ===========================================================================

/// Return the number of active entries. Exactly the stored counter.
#[ensures(result@ == self_.len@)]
pub fn len(self_: &LruList) -> usize {
    self_.len
}

// ===========================================================================
// clear — FR-008. Unconditional: resets to empty.
// Mirror of `../src/lru_list.rs::clear` (lines 148-154).
// ===========================================================================

/// Reset the list to empty. `len` becomes 0; `head`/`tail` become `None`.
#[ensures((^self_).len@ == 0)]
#[ensures((^self_).head == None && (^self_).tail == None)]
#[ensures((^self_).nodes@.len() == 0)]
pub fn clear(self_: &mut LruList) {
    self_.nodes.clear();
    self_.head = None;
    self_.tail = None;
    self_.free.clear();
    self_.len = 0;
}

// ===========================================================================
// push_back — FR-002 (track), FR-011 (recycling).
// Mirror of `../src/lru_list.rs::push_back` (lines 37-67).
// ===========================================================================

/// Insert a key at the back (most-recently-used); return its node index.
///
/// Proven (given a link-valid arena):
///  * **In-bounds handle:** the returned index is `< nodes.len()` — every handle
///    `track` hands out is a valid slot (FR-002).
///  * **Cardinality:** `len` increases by exactly 1.
///  * **Recycling (FR-011):** if the free list is non-empty the slot is reused
///    and `nodes` does *not* grow; only when the free list is empty does `nodes`
///    grow by one. This is the bounded-memory guarantee for high-churn pools.
#[requires(inv(self_))]
#[requires((*self_).len@ < usize::MAX@)]
#[ensures((^self_).len@ == (*self_).len@ + 1)]
#[ensures(result@ < (^self_).nodes@.len())]
// Recycling: reuse when free non-empty (no growth), else append (grow by one).
#[ensures((*self_).free@.len() > 0 ==> (^self_).nodes@.len() == (*self_).nodes@.len())]
#[ensures((*self_).free@.len() == 0 ==> (^self_).nodes@.len() == (*self_).nodes@.len() + 1)]
pub fn push_back(self_: &mut LruList, key: u64) -> u32 {
    let idx = if let Some(free_idx) = self_.free.pop() {
        self_.nodes[free_idx as usize] = Node {
            key,
            prev: self_.tail,
            next: None,
            active: true,
        };
        free_idx
    } else {
        let idx = self_.nodes.len() as u32;
        self_.nodes.push(Node {
            key,
            prev: self_.tail,
            next: None,
            active: true,
        });
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

// ===========================================================================
// remove — FR-004 (remove), FR-010 (idempotent), FR-011 (recycling).
// Mirror of `../src/lru_list.rs::remove` (lines 121-145).
// ===========================================================================

/// Unlink a node by index. Idempotent on an already-removed (inactive) node.
///
/// Proven (given a link-valid arena, and `len >= 1` when the node is active —
/// you cannot have an active node in an empty list):
///  * **Idempotent (FR-010):** if the node is already inactive, the list is
///    unchanged.
///  * **Cardinality:** removing an active node decreases `len` by exactly 1.
///  * **Deactivation + recycling (FR-011):** the node becomes inactive and its
///    slot is pushed onto the free list.
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[requires((*self_).nodes@[idx@].active ==> (*self_).len@ >= 1)]
#[ensures(!(*self_).nodes@[idx@].active ==> ^self_ == *self_)]
#[ensures((*self_).nodes@[idx@].active ==> (^self_).len@ == (*self_).len@ - 1)]
#[ensures(!(^self_).nodes@[idx@].active)]
pub fn remove(self_: &mut LruList, idx: u32) {
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

// ===========================================================================
// move_to_back — FR-003 (touch). Mirror of `../src/lru_list.rs::move_to_back`
// (lines 70-96).
// ===========================================================================

/// Move an existing node to the back (most-recently-used).
///
/// Proven (given a link-valid arena):
///  * **Idempotent / no-op guards (FR-010, FR-003):** an inactive node, or a
///    node already at the tail, leaves the list unchanged.
///  * **Cardinality:** `len` is unchanged (a touch neither adds nor drops).
///  * **Tail update:** after the move the node is the tail (most-recent).
#[requires(inv(self_))]
#[requires(idx@ < (*self_).nodes@.len())]
#[ensures(!(*self_).nodes@[idx@].active ==> ^self_ == *self_)]
#[ensures((*self_).tail == Some(idx) ==> ^self_ == *self_)]
#[ensures((^self_).len@ == (*self_).len@)]
#[ensures((*self_).nodes@[idx@].active && (*self_).tail != Some(idx) ==>
    (^self_).tail == Some(idx))]
pub fn move_to_back(self_: &mut LruList, idx: u32) {
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
