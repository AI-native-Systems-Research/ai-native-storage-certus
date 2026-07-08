# Cross-Check Report (Codex) - July 2

Scope:

- Target branch: `unstable-creusot`
- Target verification crate: `/home/cornel/ai-native-storage-certus/components/dispatch-map/verif`
- Goal: assess what current Creusot verification covers, what it misses vs spec-derived properties, and what to improve next.

## Current Proof Health

- Command run: `cargo creusot` in `components/dispatch-map/verif`
- Result: **Proved (28 files)**.

## What Current `dispatch-map/verif` Covers Well

1. Per entry level reference-count safety invariants.
   - `inv_write_binary`, `no_active_refs`
   - `take_read`, `release_read`, `take_write`, `release_write`, `downgrade_reference`
2. Core state transitions for per entry location changes.
   - `create_staging`, `create_memory_tier_entry`, `recover_extent`
   - `convert_to_storage`, `convert_memory_tier_to_block`
3. Eviction safety at per entry level.
   - `is_evictable`
   - `take_read_prevents_eviction`
   - write-through lifecycle proof (`lifecycle_write_through_safety`)
4. TSC-ordering fairness shape for eviction candidates.
   - `tsc_sorted`, `prefix_colder_than_suffix`, `eviction_fairness`
   - `lifecycle_cold_evicted_before_hot`

This is strong local verification for dispatch-map per entry semantics.

## What Is Not Covered (or only partially covered) vs Spec-Derived Properties

Important: our spec-derived baseline (`P1..P31`) includes many dispatcher-level properties.  
Current crate verifies mostly **dispatch-map per entry mechanics**, so several high-value properties are out of scope.

1. Dispatcher API gate and dependency lifecycle.
   - Not covered: `initialize()` gate (`P1`, `P2`) and component binding behavior.
2. Map-level uniqueness/membership semantics.
   - Partial only: real `AlreadyExists`/`KeyNotFound` map behavior is not modeled as a full key-map relation.
3. Lookup mismatch contract (`P11`) at API level.
   - Not covered in this crate:
     - `components/dispatch-map/src/lib.rs::lookup` has no requested-size parameter and no mismatch branch.
     - Verif `lookup(entry, tsc)` models ref behavior only, not size compatibility behavior.
4. Dispatcher prepare/commit/cancel lifecycle (`P21-P24`).
   - Not covered in this crate (these are dispatcher concerns).
5. Dispatcher temporal/behavioral properties (Phase B set).
   - Background write-through eventuality, shutdown drain/join, async stream semantics: not covered here.
6. End-to-end refinement from verifier model to concrete dispatcher code.
   - Not established in this crate.

## Concrete Gap Example Confirmed on This Branch

- `components/dispatch-map/src/lib.rs:166` (`lookup`) returns per entry location data but does not encode size-mismatch semantics.
- Therefore, current dispatch-map verif cannot prove property shape:  
  "lookup with mismatched requested size must return hard-fail (`InvalidParameter`/`MismatchSize`) with no partial success."
- This is a key adversarial check item because it affects dispatcher copy safety.

## Recommended Improvements (Prioritized)

1. Add an explicit coverage matrix inside this crate aligned to `P1..P31`.
   - Mark each property as `Covered`, `Partial`, `Out-of-scope`.
   - This avoids over-claiming from "28 proved files".
2. Extend dispatch-map model to include lookup-size contract where architecturally appropriate.
   - Option A: add a map-level lookup wrapper model with `(stored_size, requested_size)`.
   - Option B: keep dispatch-map scope narrow and move size-mismatch proof obligation to dispatcher verif with explicit dependency note.
3. Add map-level ghost model (not only single entry) for uniqueness/membership proofs.
   - Capture `AlreadyExists`, `KeyNotFound`, and key presence semantics at map level.
4. Create/strengthen dispatcher verification crate linkage.
   - Use dispatch-map invariants as assumptions/lemmas.
   - Prove dispatcher-level properties (`P11`, `P21-P24`, init gate, mode split).
5. Add an assumption/trusted ledger for this crate if any trusted or external assumptions are introduced.

## Practical Next Step

Start with a focused "coverage truth table" doc for this crate:

- Rows: `P1..P31`
- Columns: `dispatch-map/verif evidence`, `status`, `notes`, `next proof target`

Then implement one high-value missing property end-to-end (recommended: `P11` path split and explicit ownership of whether it belongs in dispatch-map or dispatcher proof layer).
