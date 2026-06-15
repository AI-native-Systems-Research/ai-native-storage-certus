# Creusot Verification Plan: Dispatcher First Properties

This plan maps `first_properties.md` into a proof structure usable in Creusot.
Primary objective: prove safety/correctness of dispatcher state transitions and API contracts before liveness/performance claims.

## 1. Verification boundary and assumptions

1. Verify at the dispatcher/domain layer (state transitions, map consistency, error behavior).
2. Treat GPU/SPDK/threads/I/O as abstract effects behind traits; model return values and failures nondeterministically.
3. Exclude throughput and real-time/liveness guarantees from the first pass.
4. Use sequential abstraction for core functional proofs even if runtime implementation is concurrent.

## 2. Core abstract model (ghost)

Define a ghost model independent of concrete storage:

1. `GhostMap<CacheKey, EntryState>`
2. `EntryState`:
   - `MemoryTier { size: usize, ssd_offset: Option<u64>, ts: u64 }`
   - `BlockDevice { size: usize, ssd_offset: u64, ts: u64 }`
   - `Staging { size: usize, ts: u64 }`
   - `PendingWrite { size: usize, drive: usize }`
3. `GhostMemoryTierUsed: usize`
4. `GhostMemoryTierCap: usize`
5. `GhostAttemptCounter` for bounded loops in `evict_for_space`

Concrete fields (dispatch map, pending-write map, memory-tier allocator) are linked to this ghost state via representation invariants.

## 3. Global invariants to encode

1. **Exclusive state**: each key maps to exactly one `EntryState` variant.
2. **Capacity accounting**: sum of `MemoryTier` entry sizes equals `GhostMemoryTierUsed`.
3. **Capacity bound**: `GhostMemoryTierUsed <= GhostMemoryTierCap`.
4. **Pending-write uniqueness**: key in `PendingWrite` is not also in non-pending state.
5. **BlockDevice offset validity**: `BlockDevice` entries always have concrete offset.
6. **MemoryTier offset optionality**: `MemoryTier` may have `ssd_offset` none/some.
7. **Drive mapping consistency**: whenever drive is referenced for a key, `drive == key % num_drives`.
8. **Reference/state consistency** (abstract): no operation leaves key both removed and referenced in model.

## 4. Function contract skeletons (requires/ensures)

Use these as Creusot contract templates in Rust (`#[requires]`, `#[ensures]`).

1. `initialize()`:
   - Requires: not initialized.
   - Ensures success iff required deps bound.
   - Ensures: initialized flag true on success, unchanged map semantics.

2. `check(key)`:
   - Requires: initialized.
   - Ensures: result == `ghost_map.contains(key)`.
   - Ensures: model state unchanged.

3. `populate(key, size, ...)`:
   - Requires: initialized, valid size.
   - Ensures on `Ok`: key exists in `MemoryTier` with given size.
   - Ensures on `AlreadyExists`: full state unchanged for that key.
   - Ensures on `AllocationFailed`: no new key insertion.

4. `lookup(key, ...)`:
   - Requires: initialized.
   - Ensures on miss: `KeyNotFound`, state unchanged.
   - Ensures on MemoryTier hit: key remains MemoryTier; timestamp monotonic non-decreasing.
   - Ensures on BlockDevice success: key transitions to MemoryTier.
   - Ensures on Staging hit: key remains valid entry.

5. `remove(key)`:
   - Requires: initialized.
   - Ensures on `Ok`: key absent.
   - Ensures on `KeyNotFound`: state unchanged.

6. `touch(key)`:
   - Requires: initialized.
   - Ensures on `Ok`: key present, only timestamp may change.
   - Ensures on `KeyNotFound`: state unchanged.

7. `prepare_store(key, size)`:
   - Requires: initialized.
   - Ensures `size==0` -> `InvalidParameter`, unchanged.
   - Ensures existing key -> `AlreadyExists`, unchanged.
   - Ensures success -> `PendingWrite` state for key.

8. `commit_store(key)`:
   - Requires: initialized.
   - Ensures success: `PendingWrite -> BlockDevice`.
   - Ensures success: no pending write remains for key.
   - Ensures no pending write -> `KeyNotFound`, unchanged.

9. `cancel_store(key)`:
   - Requires: initialized.
   - Ensures success: key absent and no pending write remains.
   - Ensures no pending write -> `KeyNotFound`, unchanged.

10. `evict_for_space(needed)`:
   - Requires: initialized.
   - Ensures iteration count `<= 512`.
   - Ensures success: `used + needed <= capacity`.
   - Ensures `AllocationFailed`: unable to establish capacity condition.

11. `clear_memory_tier()`:
   - Requires: initialized.
   - Ensures no key in MemoryTier afterward.
   - Ensures return value equals number of MemoryTier entries removed/transitioned.

12. recovery path (`format_on_init=false` reconstruction):
   - Ensures each enumerated extent creates matching BlockDevice map entry.

## 5. Loop invariants and variants

For `evict_for_space`:

1. Loop invariant: global invariants preserved each iteration.
2. Loop invariant: attempts `<= 512`.
3. Variant: `512 - attempts` (strictly decreases).
4. Progress condition: either capacity condition met or attempts increase.

For `clear_memory_tier`:

1. Loop invariant: processed count equals number removed so far.
2. Variant: number of MemoryTier entries remaining.

## 6. Error-preservation lemmas

Create reusable lemmas to reduce proof duplication:

1. `already_exists_preserves_state`
2. `not_found_preserves_state`
3. `invalid_parameter_preserves_state`
4. `allocation_failed_no_insertion`

These are used across `populate`, `prepare_store`, `remove`, `touch`, `commit`, `cancel`.

## 7. Proof decomposition strategy (implementation order)

1. Model + invariants only (no API proofs yet).
2. Easy read-only APIs: `check`, miss path of `lookup`.
3. Local mutators: `touch`, `remove`.
4. Store workflow: `prepare_store`, `commit_store`, `cancel_store`.
5. Populate + lookup promotion transitions.
6. Eviction algorithm (`evict_for_space`) with bounded-loop proof.
7. `clear_memory_tier`.
8. Recovery reconstruction (`format_on_init=false`).

## 8. Expected Creusot artifacts

1. Spec module with:
   - ghost `EntryState`
   - predicates: `well_formed_map`, `capacity_ok`, `key_state_exclusive`
2. Contracted dispatcher methods with `requires/ensures`.
3. Helper proof functions/lemmas for error-preservation and transition soundness.
4. Optional refinement layer proving concrete struct refines ghost model.

## 9. What is intentionally out of scope (first pass)

1. Real thread interleaving proofs.
2. Stream/NVMe completion ordering guarantees at hardware level.
3. Throughput and latency success criteria.
4. Background-thread eventual completion properties (temporal liveness).

## 10. Completion criteria for this plan

The first-pass verification is considered complete when:

1. All 30 core properties from `first_properties.md` are mapped to either:
   - a proven API contract, or
   - a proven global invariant/lemma.
2. No admitted proof (`#[trusted]` / unchecked assumptions) remains for core transition logic.
3. Remaining trusted assumptions are limited to external trait behavior (GPU/SPDK/I/O boundaries) and documented explicitly.
