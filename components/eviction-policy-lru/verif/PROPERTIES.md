# Verified properties — eviction-policy-lru (Creusot)

Proven from spec `.specify/specs/001-lru-eviction-policy/spec.md` against code
`src/lru_list.rs`. Artifacts: `verif/`.

Formal verification of the LRU policy's core data structure — the index-based
doubly-linked list (`LruList`) — proved with
[Creusot](https://github.com/creusot-rs/creusot) and discharged by the
`alt-ergo` and `z3` SMT solvers.

- **Crate:** `components/eviction-policy-lru/verif/`
- **Status:** `Proved (5 files) ✔` — 21 verification conditions, 0 admits.

## Reproduce

```bash
cd components/eviction-policy-lru/verif
cargo creusot --only coma   # fast syntax/translation check (no solvers)
cargo creusot               # full proof — expect: Proved (5 files) ✔
```

## What is a "mirror"

The shipped policy wraps `LruList` in `RwLock<Vec<Mutex<Pool>>>` (`src/lib.rs`),
which cannot compile under Creusot (Mutex/RwLock, the component framework, the
async logger). This crate proves a **line-faithful copy** of the pure arena in
`src/lru_list.rs`. `LruList`/`Node` are transcribed field-for-field
(`CacheKey = u64` kept concrete); no substitutions to the bodies. A residual
drift gap therefore exists between mirror and shipped code; line-faithfulness to
the cited source and the fault-injection log below keep it honest.

## What is proved — and what is not

This verification covers the arena's **memory-safety and cardinality** layer:

- Every `self.nodes[i]` index into the arena stays **in bounds** — the pointer
  surgery in `push_back`/`remove`/`move_to_back` never reads or writes an invalid
  slot (no OOB, no panic).
- `len` bookkeeping is **exact**: +1 per insert, −1 per active removal, unchanged
  by a touch, 0 after `clear`.
- Free-list **recycling** (FR-011): a reused slot does not grow `nodes`.
- **Idempotency** (FR-010) of `remove`/`move_to_back` on inactive handles.

It does **not** cover LRU **ordering correctness** — that `pop_front` returns the
genuinely oldest key, that `move_to_back` makes a node the most-recent *in the
traversal order*, that the chain is acyclic. Those rest on a reachability
invariant over the `next`/`prev` links (see *Assumptions* and *Attempted but not
proven*).

## len — pool size query (FR-007)

Mirror of `src/lru_list.rs::len` (lines 157–159).

- **[Postcondition]** Returns exactly the number of active entries the list is
  tracking (the stored counter). — spec FR-007 — proved: `len.coma` (1/1 VCs)

## clear — reset a pool to empty (FR-008)

Mirror of `src/lru_list.rs::clear` (lines 148–154).

- **[Postcondition]** After `clear`, the pool holds zero entries (`len == 0`),
  the head and tail links are `None`, and the node arena is empty — the pool is
  fully reset and safe to reuse. — spec FR-008 — proved: `clear.coma` (3/3 VCs)

## push_back — track a new entry as most-recently-used (FR-002, FR-011)

Mirror of `src/lru_list.rs::push_back` (lines 37–67).

- **[Postcondition · in-bounds handle]** The returned node index is a valid slot
  (`< nodes.len()`) — every `EvictionHandle` `track` hands out points at a real
  node, so later O(1) `touch`/`remove` cannot go out of bounds. — spec FR-002 —
  proved: `push_back.coma` (7/7 VCs)
- **[Postcondition · cardinality]** `len` increases by exactly 1. — spec FR-002 —
  proved: `push_back.coma`
- **[Postcondition · recycling]** If the free list is non-empty the slot is
  reused and the node arena does **not** grow; only when the free list is empty
  does the arena grow by one. This is the bounded-memory guarantee for long-lived
  high-churn pools. — spec FR-011 — proved: `push_back.coma`
- **[Precondition]** Link-valid arena (`inv`, see below); `len < usize::MAX`.

## remove — unlink an entry (FR-004, FR-010, FR-011)

Mirror of `src/lru_list.rs::remove` (lines 121–145).

- **[Postcondition · idempotent]** Removing an already-inactive node leaves the
  list completely unchanged (no panic, no effect). — spec FR-010 — proved:
  `remove.coma` (5/5 VCs)
- **[Postcondition · cardinality]** Removing an active node decreases `len` by
  exactly 1. — spec FR-004 — proved: `remove.coma`
- **[Postcondition · deactivation]** After `remove`, the node is inactive (and its
  slot is returned to the free list for recycling). — spec FR-004 / FR-011 —
  proved: `remove.coma`
- **[Precondition]** Link-valid arena (`inv`); and if the target node is active,
  `len >= 1` (you cannot hold an active node in an empty list) — this is what
  makes the `len -= 1` underflow-free.

## move_to_back — touch an entry to most-recently-used (FR-003)

Mirror of `src/lru_list.rs::move_to_back` (lines 70–96).

- **[Postcondition · no-op guards]** An inactive node, or a node already at the
  tail, leaves the list unchanged (idempotent touch). — spec FR-003 / FR-010 —
  proved: `move_to_back.coma` (5/5 VCs)
- **[Postcondition · cardinality]** `len` is unchanged — a touch neither adds nor
  drops an entry. — spec FR-003 — proved: `move_to_back.coma`
- **[Postcondition · tail update]** Touching an active non-tail node makes it the
  new tail (the most-recently-used slot). — spec FR-003 — proved: `move_to_back.coma`
- **[Precondition]** Link-valid arena (`inv`).

## Assumptions / trusted boundaries

- **Mirror, not shipped code.** All proofs run on a standalone copy of
  `lru_list.rs`; injecting a fault into the *shipped* function will not fail these
  proofs. The gap is guarded by line-faithfulness to the cited source and the
  fault-injection log below.
- **Arena well-formedness (`inv`) is an *assumed* precondition, not a maintained
  invariant.** `inv` states that `head`, `tail`, every node's `prev`/`next`, and
  every free-list slot are `None` or an in-bounds index, and `len <= nodes.len()`.
  It is *assumed* on entry to each operation (the way the dispatch-map mirror
  assumes its condvar/refcount guards) to supply the in-bounds facts — it is **not
  re-established in the postconditions**, so these proofs are *conditional on* a
  well-formed arena and do **not** by themselves prove the arena stays well-formed
  across a sequence of operations. The shipped code maintains well-formedness by
  construction; that maintenance is not machine-checked here.
- **Ordering is not modelled.** `inv` says nothing about reachability or the
  order of the chain, so a green proof here is silent on whether the LRU order is
  correct — only that the operations are memory-safe and count-consistent.

## Attempted but not proven

- **LRU ordering correctness** — FR-005 (`identify_next_to_evict`/`pop_front`
  returns the genuinely *oldest* key), the ordering half of FR-003 (`touch` makes
  a node most-recent *in traversal order*), and FR-006
  (`get_eviction_candidates`/`peek_front_n` returns the *n oldest, in order*).
  Each needs a reachability/acyclicity invariant relating the `head→next→…→tail`
  chain to the abstract insertion order — a quantified, inductive structural
  invariant that is out of practical scope for the SMT backend at this depth.
- **`pop_front` (FR-005)** and **`peek_front_n` (FR-006)** are not proved: both
  traverse the `next` chain, whose termination and correctness require the same
  acyclicity invariant above.
- **Concurrency / thread-safety** — NFR-002 (no corruption under concurrent
  access) and NFR-003 (per-pool locking). Creusot reasons about sequential code;
  the `RwLock`/`Mutex` layer is out of scope.
- **The pool-management layer** in `src/lib.rs` — `create_pool` sequential IDs
  (FR-001), `InvalidPool` degradation (FR-009), `batch_touch` (FR-012). These sit
  behind `RwLock<Vec<Mutex<…>>>` and cannot compile under Creusot; `batch_touch`
  additionally inherits the ordering caveat (it calls `move_to_back` per handle).

## Fault-injection validation

Each proof was confirmed non-vacuous by perturbing its mirror body and observing
a verification condition go red (`✘`); all five faults were reverted afterward.

| Injected fault | Result |
|---|---|
| `len`: return `self.len + 1` (wrong count) | `vc_len` ✘ — the exact-count postcondition is load-bearing |
| `clear`: set `len = 1` instead of `0` | `vc_clear` ✘ (2/3) — the reset-to-empty postcondition is load-bearing |
| `push_back`: drop `self.len += 1` | `vc_push_back` ✘ (10/12) — the +1 cardinality postcondition is load-bearing |
| `remove`: drop `self.len -= 1` | `vc_remove` ✘ (13/14) — the −1 cardinality postcondition is load-bearing |
| `move_to_back`: drop `self.tail = Some(idx)` | `vc_move_to_back` ✘ (19/21) — the tail-update postcondition is load-bearing |
