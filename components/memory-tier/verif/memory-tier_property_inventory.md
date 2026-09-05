# memory-tier — Property Inventory (tool-independent, reconciled)

**Role 1 output** (`build-property-inventory`). Single source of truth: id-keyed verifiable
properties, each traced to spec + code, bundled onto the 17 `IMemoryTier` public methods.
**No prover is named here** and **no proof status is recorded** — lanes/status are added downstream
(Role 2 verify, Role 3 aggregate).

- Spec read: `components/memory-tier/specs/001-memory-tier/spec.md` (blind extraction → 37 obligations)
- Code read: `src/allocator.rs`, `src/lib.rs`, `interfaces/src/imemory_tier.rs` (blind extraction → 94 obligations)
- Reconciled: **2026-09-05** (both extractions independent/blind; agreement is real signal).

## Denominator
- **N = 17 public methods** (the `define_interface!` block; excludes `Display::fmt` and `#[cfg(test)]`).
- **M = 54 distinct verifiable properties.**
- **Attachments = 101** (Σ of bundle sizes; a property shared by k methods counts k times).

The two counts differ because 11 properties are **named global invariants / shared obligations** that
attach to several methods (e.g. `ALIGN-4K` serves insert/get/peek/pool_info; `INIT-GATE` serves all 16
non-`initialize` methods). This is expected and correct — one property, many bundles.

Every property carries `global: true|false` (true iff `|methods| > 1`) and `attachments` (= `|methods|`).
These are the **leverage signal** Role 2 consumes: prove globals **once, highest-`attachments` first**, and
the `proved` status propagates to every bundle by `id`.

## Global-invariant ledger — Role-2 prove-once worklist (sorted by attachments ↓)

Prove each of these **once** as a maintained invariant, in this order; discharging the top of the list
unblocks the most public methods per proof.

| rank | id | attachments | global | proof note (Creusot) |
|---|---|---|---|---|
| 1 | INIT-GATE | 16 | ✓ | **highest leverage.** Model `MemoryTier{initialized}` state; each data-path op gated → returns gated result, no mutation. Cheap; unblocks all 16. |
| 2 | USED-LE-CAP | 7 | ✓ | already proved on FreeList mirror (`alloc_admits`); reuse. |
| 3 | USED-CONSERVE | 6 | ✓ | already proved (`allocate_split`/`deallocate_used`/`merge`/`lifecycle`); reuse. |
| 4 | CONTAINS-REFLECTS | 6 | ✓ | **the hard one** — needs logic-level `FMap` slot-map model. Not on FreeList mirror. |
| 5 | COALESCE | 5 | ✓ | already proved (`coalesce_prev`/`coalesce_next`/`deallocate_merge`); reuse. |
| 6 | ALIGN-4K | 4 | ✓ | already proved (`align_up`,`leftover_offset_aligned`); reuse. |
| 7 | SPACE-REUSE | 4 | ✓ | partial (`lifecycle_alloc_free`, single cycle); extend. |
| 8 | PTR-IN-BOUNDS | 4 | ✓ | safety; offset+aligned(size) ≤ pool_size. Not yet. |
| 9 | CAP-CONST | 2 | ✓ | init sets, mutators preserve. |
| 10 | FREECAP-DERIVED | 2 | ✓ | already proved (`free_capacity`); reuse. |
| — | INIT-MONOTONIC | (supports INIT-GATE) | ✓ | the `initialized` latch; prove alongside #1. |

**Reading of this table:** ranks 2,3,5,6,10 are *already discharged* on the FreeList mirror — Role 2 reuses
them. The **new** high-leverage work is rank 1 (`INIT-GATE`, cheap, +16 methods) and rank 4
(`CONTAINS-REFLECTS`, the `FMap` campaign, +6 methods). Ranks 7–9 are follow-ons.

---

## Bundle size per public method (counted, not eyeballed)

| # | method | bundle size | of which: unique to method | shared globals |
|---|---|---|---|---|
| 1 | insert | 13 | 5 | USED-LE-CAP, USED-CONSERVE, ALIGN-4K, CONTAINS-REFLECTS, INIT-GATE, COALESCE, SPACE-REUSE, PTR-IN-BOUNDS |
| 2 | evict_next | 10 | 4 | USED-LE-CAP, USED-CONSERVE, CONTAINS-REFLECTS, INIT-GATE, COALESCE, SPACE-REUSE |
| 3 | initialize | 9 | 7 | CAP-CONST, INIT-MONOTONIC |
| 4 | remove | 8 | 2 | USED-LE-CAP, USED-CONSERVE, CONTAINS-REFLECTS, INIT-GATE, COALESCE, SPACE-REUSE |
| 5 | evict_next_for_key | 7 | 1 (alias) | USED-LE-CAP, USED-CONSERVE, CONTAINS-REFLECTS, INIT-GATE, COALESCE, SPACE-REUSE |
| 6 | get | 7 | 4 | ALIGN-4K, INIT-GATE, PTR-IN-BOUNDS |
| 7 | clear | 7 | 2 | USED-LE-CAP, USED-CONSERVE, CONTAINS-REFLECTS, INIT-GATE, COALESCE |
| 8 | peek | 6 | 3 | ALIGN-4K, INIT-GATE, PTR-IN-BOUNDS |
| 9 | batch_touch | 6 | 5 | INIT-GATE |
| 10 | oldest_keys | 5 | 4 | INIT-GATE |
| 11 | touch | 4 | 3 | INIT-GATE |
| 12 | capacity | 4 | 0 | CAP-CONST, USED-LE-CAP, FREECAP-DERIVED, INIT-GATE |
| 13 | used | 4 | 0 | USED-LE-CAP, USED-CONSERVE, FREECAP-DERIVED, INIT-GATE |
| 14 | pool_info | 4 | 1 | ALIGN-4K, INIT-GATE, PTR-IN-BOUNDS |
| 15 | telemetry_snapshot | 3 | 1 | TELEM-EVICTCOUNT (shared w/ evict_next), INIT-GATE |
| 16 | contains | 2 | 0 | CONTAINS-REFLECTS, INIT-GATE |
| 17 | is_dma_capable | 2 | 1 | INIT-GATE |

Note the shape flips from my earlier *guessed* table: rigorous reconciliation **grows** insert (7→13),
evict (4→10), and the accessors (because every method carries `INIT-GATE`, and the allocator methods
carry the accounting + coalesce + reuse invariants). That growth is the defect-#4 fix in action.

---

## Named global invariants / shared obligations (11)

| id | statement | methods (attachments) | origin |
|---|---|---|---|
| CAP-CONST | `capacity()` is fixed after `initialize` for the pool's life. | initialize, capacity | spec+code |
| USED-LE-CAP | `used() ≤ capacity()` at every reachable state. | insert, remove, evict_next, evict_next_for_key, clear, used, capacity (7) | spec+code |
| USED-CONSERVE | `used()` = Σ of 4 KiB-rounded sizes of live slots. | insert, remove, evict_next, evict_next_for_key, clear, used (6) | spec+code |
| FREECAP-DERIVED | free capacity = `capacity() − used()` always. | capacity, used (2) | spec+code |
| ALIGN-4K | every allocation / offset / returned pointer is 4 KiB-aligned. | insert, get, peek, pool_info (4) | spec+code |
| CONTAINS-REFLECTS | membership = exactly inserted-and-not-removed keys. | contains, insert, remove, evict_next, evict_next_for_key, clear (6) | spec+code |
| INIT-GATE | no data-path op acts before `initialize` succeeds. | all 16 non-initialize methods | spec+code |
| COALESCE | adjacent free regions merge on deallocation. | remove, evict_next, evict_next_for_key, clear, insert (5) | spec+code |
| SPACE-REUSE | bytes freed by remove/evict become allocatable again. | insert, remove, evict_next, evict_next_for_key (4) | spec+code |
| PTR-IN-BOUNDS | `slot.offset + aligned(size) ≤ pool_size`, so returned pointers stay in the mapped pool. | insert, get, peek, pool_info (4) | code-only (safety) |
| INIT-MONOTONIC | `initialized` latches true; never reset during the object's life. | initialize (1) | code-only |

---

## Per-method unique obligations (43)

**initialize (7):** INIT-ZERO (0 size → InvalidSize), INIT-DOUBLE (second init → error), INIT-CAPACITY
(capacity==pool_size), INIT-EMPTY (slots empty, used==0), INIT-EP-NOT-CONNECTED (ep receptacle absent →
NotInitialized · *code-only, undocumented*), INIT-ERR-NO-STATE-CHANGE (frame on error), INIT-SPDK-ALLOC-FAILED
(*feature `spdk`*).
**insert (5):** INSERT-ZERO, INSERT-DEDUP (dup → AlreadyExists), INSERT-DEDUP-NO-ALLOC (frame: allocator
untouched on dup · *code-only*), INSERT-POOLFULL, INSERT-SUCCESS (slot recorded, writable ptr returned).
**get (4):** GET-ROUNDTRIP (same ptr+size insert gave), GET-TOUCHES (calls ep.touch), GET-ABSENT-NONE
(*code-only*), GET-NO-MUTATION (frame · *code-only*).
**peek (3):** PEEK-RETURNS, PEEK-NOTOUCH (frame: no eviction-order change), PEEK-ABSENT-NONE (*code-only*).
**evict_next (4):** EVICT-FREES (removes victim, frees, used↓), EVICT-EMPTY-NONE (no victim → None · *code-only*),
EVICT-DEALLOC-GUARDED (frame · *code-only*), TELEM-EVICTCOUNT (*feature*, shared with telemetry_snapshot).
**evict_next_for_key (1):** EVICTKEY-ALIAS (== evict_next; key ignored).
**oldest_keys (4):** OLDEST-BOUNDED (≤ n), OLDEST-ORDER (oldest-first), OLDEST-NOREMOVE (frame),
OLDEST-GUARD-EMPTY (not-init OR n==0 → empty · *code-only*).
**remove (2):** REMOVE-FREES, REMOVE-NOTFOUND (absent → KeyNotFound).
**touch (3):** TOUCH-UPDATES (calls ep.touch), TOUCH-NODATA (frame), TOUCH-ABSENT-NOOP (frame · *code-only*).
**batch_touch (5):** BATCHTOUCH-ALL (all present keys), BATCHTOUCH-SKIP (absent skipped), BATCHTOUCH-EMPTY
(∅ no-op), BATCH-EP-NOT-CONNECTED (ep absent → no-op · *code-only, undocumented*), BATCH-NO-MUTATION (frame · *code-only*).
**pool_info (1):** POOLINFO-RETURNS (Some(base ptr, size) iff init & non-null).
**is_dma_capable (1):** DMA-IFF-SPDK (true iff SPDK-allocated).
**clear (2):** CLEAR-EMPTIES (slots emptied, allocator reset, used→0), CLEAR-COUNT (returns #removed).
**telemetry_snapshot (1):** TELEM-SNAPSHOT-ZERO (all-zero when feature off; live counters when on).

*(contains, capacity, used have 0 unique obligations — their entire contract is global invariants.)*

---

## Reconciliation flags (surfaced, not smoothed)

1. **`evict_next_for_key` — stale doc, not a behavior gap.** Spec FR-014 and the code AGREE it is a pure
   alias (key ignored; design collapsed to a single `RwLock<Pool>` on 2026-08-20). The **interface
   doc-comment still promises same-shard eviction**, which is not implemented → documentation divergence
   to fix; the *property* is consistent across spec+code.
2. **`oldest_keys` ordering & bound are cross-component.** Spec places OLDEST-ORDER and OLDEST-BOUNDED on
   memory-tier, but the code **delegates entirely** to `IEvictionPolicy::get_eviction_candidates`.
   memory-tier itself only guarantees OLDEST-GUARD-EMPTY and OLDEST-NOREMOVE; the order/bound truth lives
   in the eviction-policy component. → not a memory-tier proof target.
3. **`INIT-GATE` manifests three ways** (refinement, not divergence): `None` for Option-returning ops
   (get/peek/evict/evict_for_key), `Err(NotInitialized)` for insert/remove/clear, silent no-op for
   touch/batch_touch, and 0/false/default for the accessors. One obligation, method-specific shape.
4. **Two code-only error cases the spec omits:** `INIT-EP-NOT-CONNECTED` and `BATCH-EP-NOT-CONNECTED`.
   Behavior is reasonable but **undocumented** → recommend the spec confirm/record them.
5. **`get`/`peek` absent-key → None is code-only.** The spec states no absent-key obligation; the
   extractor correctly did not invent one. Captured from code, consistent.
6. **`NotEvictable` error variant is dead** — defined, never constructed. No property.
7. **Non-interface helpers** (`free_capacity`, `telemetry`, `reset_telemetry`, `alloc_mmap`) are not among
   the 17. `free_capacity`'s obligation is captured as the FREECAP-DERIVED invariant on capacity/used.

## Not verifiable (out of every proof lane — assumptions/environment)
Raw-pointer / mmap validity, SPDK `spdk_zmalloc` + DMA backing, NUMA `mbind` placement, `Drop` teardown,
`unsafe impl Send/Sync`, RwLock interleavings + lock-contention counters, relaxed-atomic ordering,
logging side-effects, O(1)/O(log n) complexity, deadlock-freedom. (These are the slide-11 "assumed /
trusted" boundary, not bundle members.)

---

## Implementing-obligation map (for the verify step — NOT extra bundle members)

The allocator machinery the code extraction surfaced (94-list) discharges the observable invariants:

| observable invariant | implementing obligations (allocator internals) |
|---|---|
| ALIGN-4K | ALIGN-ROUNDUP (`next_multiple_of 4096`), INV-OFFSET-ALIGNED, ALLOC-RETURNS-REGION-START |
| USED-CONSERVE | ALLOC-USED-INC, DEALLOC-USED-DEC, ALLOC-SPLIT-REMAINDER, INV-CONSERVATION, INV-SLOT-ALLOC-CONSISTENCY |
| USED-LE-CAP | INV-USED-LE-CAP, ALLOC-FIRSTFIT (only takes a fitting region) |
| COALESCE | DEALLOC-COALESCE-PREV, DEALLOC-COALESCE-NEXT, DEALLOC-INSERT-FREE, INV-NO-ADJACENT-FREE |
| SPACE-REUSE | ALLOC-FIRSTFIT + COALESCE + NEW-SINGLE-FREE-REGION |
| FREECAP-DERIVED | FREECAP-EQ-CAP-MINUS-USED |
| CLEAR-EMPTIES (allocator half) | NEW-USED-ZERO, NEW-CAPACITY-STORED (clear rebuilds FreeList::new) |
| PTR-IN-BOUNDS | INV-POOL-SLOT-BOUNDS |

**Container obligations with NO allocator-arithmetic implementation** (they live in the HashMap slot map,
not the FreeList): INSERT-DEDUP, REMOVE-NOTFOUND, CONTAINS-REFLECTS, CLEAR-COUNT, INSERT-POOLFULL
(faithful decision needs the free-region set). These are the ones Creusot's current proofs do **not**
reach — the `FMap` modelling gap.
