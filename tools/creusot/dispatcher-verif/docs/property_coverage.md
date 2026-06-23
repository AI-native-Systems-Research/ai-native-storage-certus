# Property Coverage Matrix (Codex) - June 13

Scope of this matrix:

- Properties source: [first_properties.md](/home/cornel/SPECS/dispacher/first_properties.md)
- Verified model source: [lib.rs](/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs)
- Model type: abstract transition model (not yet bound to production dispatcher code)

Status labels:

- **Covered**: explicitly encoded by `requires/ensures` and proved in the current abstract model.
- **Partial**: some intent captured, but abstraction is simplified (or omits part of real behavior).
- **Not covered**: not represented yet in the current model/contracts.

## Matrix

| Property # | Property (short) | Function(s) in `lib.rs` | Status | Notes |
|---|---|---|---|---|
| 1 | `initialize()` dependency gate | N/A (modeled via `Model.initialized` checks in APIs) | Partial | We model "initialized required", but no explicit `initialize()` transition function yet. |
| 2 | Operational APIs fail with `NotInitialized` pre-init | `check`, `touch`, `remove`, `prepare_store`, `commit_store`, `cancel_store`, `populate`, `lookup` | Covered | Each function has pre-init error behavior in contracts/body. |
| 3 | `populate/prepare_store` uniqueness (`AlreadyExists`) | `populate`, `prepare_store` | Covered | Duplicate key (represented by `slot.present`) returns `AlreadyExists`. |
| 4 | `populate` inserts MemoryTier entry | `populate` | Covered | Success branch sets `state = MemoryTier` and key present. |
| 5 | No partial entry on populate allocation failure | `populate` | Covered | Allocation failure returns unchanged slot/model usage. |
| 6 | `check(key)` equals membership | `check` | Covered | `Ok(v)` contract ties result to `slot.present`. |
| 7 | `lookup` miss => `KeyNotFound` + no mutation | `lookup` | Covered | Missing key returns `KeyNotFound`, slot unchanged. |
| 8 | MemoryTier lookup keeps key and refreshes timestamp/touch | `lookup`, `touch` | Partial | Presence behavior covered; timestamp refresh is represented by separate `touch`, not fully coupled to `lookup`. |
| 9 | BlockDevice lookup promotes to MemoryTier | `lookup` | Covered | Success on `BlockDevice` changes state to `MemoryTier`. |
| 10 | Staging lookup compatibility | `lookup` | Partial | `Staging` variant exists in model, but no staging-specific branch semantics beyond generic success. |
| 11 | Lookup size-match contract (`InvalidParameter` on mismatch, no partial copy/state mutation) | `lookup` | Covered | `lookup` now checks `requested_size` against stored `slot.size`; mismatch returns `InvalidParameter` with unchanged state. |
| 12 | `remove` success => key absent | `remove` | Covered | Postcondition enforces absent key on success. |
| 13 | `remove` miss => `KeyNotFound`, no mutation | `remove` | Covered | Miss path explicitly preserved. |
| 14 | `touch` updates timestamp only, miss => `KeyNotFound` | `touch` | Partial | Timestamp monotonicity and miss behavior covered; "only timestamp changes" is approximated, not fully constrained field-by-field. |
| 15 | Eviction attempts bounded by 512 | `evict_for_space` | Covered | Contract and loop invariants enforce bound. |
| 16 | Eviction success implies capacity condition | `evict_for_space` | Covered | `Ok` postcondition enforces `used+needed <= capacity`. |
| 17 | Eviction failure implies capacity not achieved | `evict_for_space` | Covered | `AllocationFailed` postcondition enforces capacity still insufficient. |
| 18 | Clean eviction = MemoryTier -> BlockDevice | `clean_evict` | Covered | Explicit clean-eviction predicate and transition contract. |
| 19 | Blind fallback failure removes key | `blind_evict_with_fallback` | Covered | Conversion failure path enforces removal (`present=false`). |
| 20 | `prepare_store(size=0)` => `InvalidParameter` + no mutation | `prepare_store` | Covered | Explicit size check and contract. |
| 21 | Pending-write protocol lifecycle | `prepare_store`, `commit_store`, `cancel_store` | Covered | Protocol states represented (`PendingWrite` consumed by commit/cancel). |
| 22 | Commit success => BlockDevice + pending cleared | `commit_store` | Covered | State transition postconditions encoded. |
| 23 | Cancel success => key removed + pending cleared | `cancel_store` | Covered | Success path clears presence/pending. |
| 24 | Commit/cancel without pending => `KeyNotFound` + unchanged | `commit_store`, `cancel_store` | Covered | Miss path contracts preserve slot. |
| 25 | `clear_memory_tier` leaves no MemoryTier entries | `clear_memory_tier`, `clear_memory_tier2` | Covered | Bounded multi-key variant (`Cache2`) proves no MemoryTier entries remain after clear. |
| 26 | `clear_memory_tier` returned count matches removed entries | `clear_memory_tier`, `clear_memory_tier2` | Covered | `clear_memory_tier2` proves count equals pre-clear memory-tier count in bounded multi-key model. |
| 27 | Recovery soundness (`recover_extent`) | `recover_extent` | Covered | Recovered extent transitions to present entry with matching `(offset,size_blocks)`. |
| 28 | Drive mapping determinism (`key % num_drives`) | `drive_index` | Covered | Direct formula + contract. |
| 29 | Threshold/watermark comparison consistency | `watermark_order_valid` | Partial | Minimal relation encoded; real config semantics are richer. |
| 30 | Global exclusive-state invariant (all keys) | `slot_state_wf`, `wf_cache2`, `clear_memory_tier2` | Covered (bounded) | Covered for two-key bounded model (`Cache2`), not yet unbounded map-level proof. |
| 31 | Global reference/state consistency invariant | `remove_with_ref_guard`, `ref_state_consistent` | Partial | Added guard model: removal fails with `InvalidState` when active references exist; this is a local scaffold, not yet full map-level reference accounting. |

## Plain-English Legend: What Each Function Means

These are **model functions** (not production dispatcher methods). They are small, simplified behavior rules used for proof.

1. `check`  
   Means: "Is this key present in the cache?"  
   If dispatcher is not initialized, returns `NotInitialized`.

2. `touch`  
   Means: "Mark this key as recently used."  
   In model terms, increases a timestamp-like field (`ts`) for an existing key.

3. `remove`  
   Means: "Delete this key from cache."  
   Sets key presence to false.

4. `prepare_store`  
   Means: "Reserve a write slot for this key before final commit."  
   Transitions key into `PendingWrite`.

5. `commit_store`  
   Means: "Finalize a prepared write."  
   Moves key from `PendingWrite` to `BlockDevice`.

6. `cancel_store`  
   Means: "Abort a prepared write."  
   Removes the key if it was pending.

7. `populate`  
   Means: "Insert fresh data into memory tier."  
   Creates a key and puts it in `MemoryTier` if capacity allows.

8. `lookup`  
   Means: "Read key."  
   If key is on `BlockDevice`, this model promotes it back to `MemoryTier`.

9. `clear_memory_tier`  
   Means: single-key clear behavior (one key slot).

10. `clear_memory_tier2`  
    Means: bounded multi-key clear behavior (two-key cache model), with count correctness.

11. `evict_for_space`  
    Means: "Try repeated evictions until enough capacity or give up after 512 tries."

12. `clean_evict`  
    Means: "Evict only if safe/clean (already persisted + no active refs), then move MemoryTier -> BlockDevice."

13. `blind_evict_with_fallback`  
    Means: "Blind eviction path; if conversion fails, remove key."

14. `recover_extent`  
    Means: "Model recovery of persisted extent into dispatch visibility."

15. `drive_index`  
    Means: "Select device by key modulo number of drives."

16. `watermark_order_valid`  
    Means: "Basic check that low watermark is not above high watermark."

## Single-key vs Multi-key (Plain Explanation)

Current model is now **mixed**:

- Single-key functions still cover most operation-local rules via `KeySlot`.
- Added bounded multi-key model `Cache2` for selected global properties.

A **multi-key** model would include a map/set of many keys:

- You can then prove global properties like:
  - "no two keys share contradictory states",
  - "clear_memory_tier removed all memory-tier entries in the whole map",
  - "recovery inserted all extents",
  - eviction interactions between many keys.

So "single-key" means local per-key correctness; "multi-key" means global cache correctness.

## Recommended Next Matrix Update

After moving from bounded `Cache2` to an unbounded map model, re-evaluate row 30 and especially row 31 for full global coverage.

## Next Phases, Steps, and Final Goal

### Phase A - Complete Core Property Coverage (current)

Goal of this phase: strengthen first-property coverage quality and traceability in the abstract model.

Steps:

1. Add per-clause `Pxx` mapping comments in `dispatcher-verif/src/lib.rs` (fine-grained traceability).
2. Upgrade model from single-key to multi-key abstraction to address global invariants.
3. Add missing model operations for currently uncovered core properties:
   - clean vs blind eviction behavior (`P17`, `P18`)
   - recovery soundness transition (`P26`)
4. Regenerate this matrix with updated statuses and rationale.
5. Keep `cargo creusot` green after each incremental change.

### Phase B - Secondary Behavioral/Temporal Properties

Goal of this phase: address deferred system-behavior properties (not just local transition safety).

Targets:

1. Background write-through eventuality (`FR-004`, `FR-017`).
2. Shutdown drain/join behavior for writer and evictor (`FR-014`, `FR-029`).
3. SSD evictor threshold/hysteresis behavior (`FR-030`..`FR-033`).
4. Async stream semantics and synchronization contracts (`FR-036`, `FR-037`).

Notes:

- These likely need explicit assumptions/fairness statements.
- Some parts may be better captured with temporal/spec-level modeling in addition to function contracts.

### Phase C - Performance Claims Evidence Track

Goal of this phase: address performance-oriented requirements separately from safety proofs.

Targets:

1. Throughput/pipeline claims (`SC-012`, `SC-014` and related `FR-019`, `FR-039` parts).
2. Reproducible benchmark/test evidence with explicit environment assumptions.
3. Clear separation between "proved safety/correctness" and "measured performance".

### Final Goal

Produce a verification package that is easy to compare with other agents and usable by reviewers:

1. Full property-to-proof traceability (`first_properties.md` -> contracts/lemmas -> proof status).
2. Minimal or no trusted assumptions in core logic.
3. Explicitly documented assumptions for environment/concurrency/liveness.
4. Clear gap list for anything not fully proven on production code yet.
5. Repeatable artifacts (proof outputs + coverage matrix + progress report) on `unstable-codex`.

## Secondary Properties Coverage (Phase B)

Status labels here use:

- **Covered (bounded/assumption-based)**: proved in current model with explicit assumptions.
- **Covered (stronger model)**: proved with fewer simplifying assumptions.
- **Partial**: some aspects modeled, important aspects still missing.
- **Not covered**: not modeled yet.

### Secondary-1: Background write-through eventuality (`FR-004`, `FR-017`)

- Model functions:
  - `writer_step`
  - `drain_jobs_in_steps`
- Status: **Covered (bounded/assumption-based)**
- Notes:
  - Progress depends on fairness flag.
  - Eventuality expressed in bounded-step form.
  - FR-017 "drop-on-failure best-effort" captured at abstract queue-consumption level.

### Secondary-2: Shutdown drain + thread joins (`FR-014`, `FR-029`)

- Model function:
  - `shutdown_drain_join`
- Status: **Covered (bounded/assumption-based)**
- Notes:
  - If fairness holds and step budget is sufficient, pending jobs drain and join flags become true.
  - Still an abstract temporal model (not concrete thread implementation proof).

### Secondary-3: SSD evictor hysteresis (`FR-030`..`FR-033`)

- Model functions:
  - `valid_hysteresis_pair`
  - `evictor_should_start`
  - `bounded_reduce`
  - `evictor_sweep`
- Status: **Covered (bounded/assumption-based)**
- Notes:
  - Captures threshold/low-water consistency and bounded per-sweep non-increasing usage behavior.
  - Hysteresis semantics are abstracted; full production scheduler/device effects are not modeled.

### Secondary-4: Async stream semantics/synchronization (`FR-036`, `FR-037`)

- Model functions:
  - `warm_stream_available`
  - `lookup_async_model`
  - `stream_synchronize_model`
  - `lookup_sync_model`
- Status: **Covered (bounded/assumption-based)**
- Notes:
  - Distinguishes warm-stream async path vs null-stream synchronous paths.
  - Encodes sync lookup as async + synchronize contract shape.
  - Stream space is simplified (`Null`/`Warm`) relative to production runtime details.

## Assumption Reference

For the detailed assumption list and impact analysis, see:

- [assumption_ledger_codex_june14.md](/home/cornel/SPECS/dispacher/assumption_ledger_codex_june14.md)
