# Properties to Prove

Purpose:
- This is the maintained property baseline used to implement and review Creusot proofs.
- It now includes proof evidence pointers so this file remains useful even if `history/` is removed.

## How to read this file

- `Property`: stable ID.
- `Owner interface`: where the main contract/proof should live.
- `Status`: `Verified`, `Partial`, `Unchecked`, `Stale`, or `Retired`.
- `Evidence`: concrete proof function/artifact pointers.
- `Abstraction`: how close the proof model is to runtime code.

Abstraction levels:
- `L0` near-runtime: proof mirrors active code path closely.
- `L1` ghost-local: per-entry or local logic model (not full map/system).
- `L2` ghost-map: map-wide model, still abstracted.
- `L3` bounded/assumption-heavy: bounded keys/steps or stronger modeling assumptions.
- `Lx` stale/removed: proof artifact exists but code path was removed.

> Terminology note: the `L0`–`L3`/`Lx` scale is a **project-local convention**, not standard formal-verification vocabulary. It is shorthand for how far the proof's model sits from the executed code — i.e. it approximates the standard notion of **model-to-code refinement distance** (how much abstraction separates the verified model from the running Rust). `L0` ≈ near-runtime (minimal refinement gap); higher numbers add successive abstraction (per-entry → map-wide → bounded/temporal). Each step maps to a concrete formal task: e.g. lifting `L1`→`L2` means supplying the **map-wide abstraction function / gluing invariant** that connects per-entry proofs to a whole-map theorem.

Evidence path prefixes:
- Dispatcher verif artifacts: `components/dispatcher/verif/verif/dispatcher_verif_rlib/*.coma`
- Dispatch-map verif artifacts: `components/dispatch-map/verif/verif/dispatch_map_verif_rlib/*.coma`

## Property registry with proof evidence

| Property | Plain-English requirement | Owner interface | Scope | Status | Evidence (function / artifact) | Abstraction | Notes |
|---|---|---|---|---|---|---|---|
| P1 | Initialize must fail when required dependencies are missing and succeed when they are bound. | `IDispatcher` | active | Verified | `initialize_dependency_guards` -> `initialize_dependency_guards.coma` (1/1 VC, 1 `.coma` file) | L0 | Mirrors `initialize` guard prefix (`dispatcher/src/lib.rs:1053-1065`): `dispatch_map`/`memory_tier` unbound → `NotInitialized`; empty `data_pci_addrs` → `InvalidParameter`; both bound + non-empty → `Ok`. Check order preserved. Pairs with P2. |
| P2 | Operational APIs must fail with `NotInitialized` before successful init. | `IDispatcher` | active | Verified | `ensure_initialized` -> `ensure_initialized.coma` | L0 | Claude July proof; still mirrors live code. |
| P3 | Duplicate key insertion must fail cleanly (`AlreadyExists`) without mutating existing data. | `IDispatchMap` + `IDispatcher` | active | Partial | dispatch-map local creation proofs (for example `create_memory_tier_entry.coma`, `create_staging.coma`) | L1 | Map-wide uniqueness across all keys not fully discharged. |
| P4 | Successful populate must create a correct MemoryTier entry for that key. | `IDispatcher` | active | Partial | local entry creation evidence (`create_memory_tier_entry.coma`) | L1 | Dispatcher end-to-end populate flow not fully proved. |
| P5 | Populate failures must be atomic (no partial leaked entry). | `IDispatcher` | active | Partial | partial local transition evidence only | L1 | Full failure-atomicity at dispatcher boundary still open. |
| P6 | `check(key)` result must match membership truth in dispatch-map. | `IDispatchMap` | active | Partial | `lookup.coma`, `check_removable.coma` (supporting local semantics) | L1 | Full map-level membership equivalence not complete. |
| P7 | Lookup on missing key must return `KeyNotFound` and preserve state. | `IDispatcher` | active | Partial | dispatch-map `lookup.coma` local behavior only | L1 | Dispatcher API miss/no-mutation proof still open. |
| P8 | MemoryTier lookup hit must preserve key and refresh eviction metadata. | `IDispatcher` | active | Partial | `lookup.coma`, `touch.coma`, `lifecycle_lookup.coma` | L1 | End-to-end dispatcher hit path not fully proved. |
| P9 | BlockDevice lookup success must promote entry back to MemoryTier. | `IDispatcher` | active | Partial | `convert_memory_tier_to_block.coma`, lifecycle transition artifacts | L1 | Promotion contract at dispatcher layer still partial. |
| P10 | Legacy staging lookup behavior must be safe if encountered. | `IDispatcher` | legacy | Partial | `lifecycle_staging_read.coma`, `lifecycle_staging_to_block.coma` | L1/Lx | Legacy path; spec/runtime no longer emphasize staging. |
| P11 | Lookup size mismatch must hard-fail (`InvalidParameter`) with no partial copy. | `IDispatcher` | active | Partial (decision logic verified) | `resolve_lookup` -> `resolve_lookup.coma` (3/3 VCs, 1 `.coma` file) | L0 | Models the `LookupResult`→`Result` match: `MismatchSize`⇒`InvalidParameter` w/ no copy; hits clamp copy to `min(requested,stored)` (no over-copy). GAP: `LookupResult::MismatchSize` currently has no producer (`dm.lookup` is key-only, `dispatch-map/src/lib.rs:115`), so live mismatch detection is unimplemented — a fix is staged on `unstable-codex`. Proof certifies dispatcher decision logic, not live detection. |
| P12 | Successful remove must guarantee key absence afterward. | `IDispatchMap` + `IDispatcher` | active | Partial | `check_removable.coma`, transition artifacts | L1 | Full API-level remove postcondition still partial. |
| P13 | Remove on absent key must return `KeyNotFound` with no mutation. | `IDispatchMap` + `IDispatcher` | active | Partial | local no-op/miss behavior in map models | L1 | Dispatcher-level miss proof still open. |
| P14 | Touch on existing key refreshes metadata; absent key returns `KeyNotFound`. | `IDispatcher` | active | Unchecked | planned dispatcher contract | L0 target | Not yet proved at dispatcher layer. |
| P15 | Eviction loop has a bounded attempt budget. | `IDispatcher` | active | Unchecked | planned `evict_for_space` loop proof | L0/L3 target | Needs config-aligned bound model. |
| P16 | Eviction success implies enough capacity was made available. | `IDispatcher` | active | Unchecked | planned `evict_for_space` postcondition proof | L0 target | Open. |
| P17 | Eviction failure implies capacity target was not reached. | `IDispatcher` | active | Partial | `is_evictable.coma`, `take_read_prevents_eviction.coma` (local) | L1 | Dispatcher failure postcondition not fully discharged. |
| P18 | Clean eviction transitions MemoryTier entry to BlockDevice (not delete). | `IDispatchMap` + `IDispatcher` | active | Verified (local) | `convert_memory_tier_to_block.coma`, `lifecycle_memory_tier_to_block.coma`, `lifecycle_write_through_safety.coma` | L1 | Strong per-entry evidence; system-level composition still to strengthen. |
| P19 | Blind eviction fallback must not leave dangling map state. | `IDispatcher` | active | Unchecked | planned dispatcher fallback proof | L0 target | Open. |
| P20 | Zero-size direct-store validation must reject input safely. | `IDispatcher` | legacy | Verified (stays valid guard) | `prepare_store_guards.coma` (re-anchored to `populate`) | L0 | Claude July proof; requirement became legacy after API removal. |
| P21 | Pending-write consume-once protocol for prepare/commit/cancel. | `IDispatcher` | legacy | Stale | `insert_pending.coma`, `consume_once.coma` | Lx | Proof green but mirrors removed API (`pending_writes`). |
| P22 | Commit path ends in BlockDevice and clears pending write. | `IDispatcher` | legacy | Retired | no active artifact (workflow removed) | Lx | Removed with direct-store workflow. |
| P23 | Cancel path removes key and clears pending write. | `IDispatcher` | legacy | Retired | no active artifact (workflow removed) | Lx | Removed with direct-store workflow. |
| P24 | Commit/cancel without pending write returns `KeyNotFound` and preserves state. | `IDispatcher` | legacy | Stale | `consume_pending.coma` | Lx | Proof green but mirrors removed API. |
| P25 | `clear_memory_tier` leaves no MemoryTier entries. | `IDispatcher` | active | Unchecked | planned dispatcher loop/model proof | L0/L2 target | Open. |
| P26 | `clear_memory_tier` returned count matches actual cleared entries. | `IDispatcher` | active | Partial | local recovery/transition evidence only | L1 | Full counting postcondition remains open. |
| P27 | Recovery must recreate dispatch-map entries consistent with extents. | `IDispatchMap` + `IDispatcher` | active | Verified (local) | `recover_extent.coma`, `lifecycle_recover_extent.coma` | L1 | Strong per-entry recovery evidence. |
| P28 | Drive-selection formula must be deterministic and stable. | `IDispatcher` | active | Verified | `drive_index` -> `drive_index.coma` (2/2 VCs, 1 `.coma` file) | L0 | Discharged theorem: `num_drives>0 ==> result < num_drives` (drive index always in range). Determinism/stability is structural (pure fn, no state/RNG). Mirrors `dispatcher/src/lib.rs:241`. |
| P29 | Threshold/watermark config comparisons must follow intended direction. | `IDispatcher` | active | Unchecked | planned config relation proof | L0 target | Open. |
| P30 | Each key must be in one logical state at a time (exclusive-state invariant). | `IDispatchMap` (map-wide) | active | Partial | local state invariants in lifecycle proofs | L1/L2 target | Needs map-wide ghost lifting. |
| P31 | Reference counters and state must remain mutually consistent. | `IDispatchMap` (map-wide) | active | Partial | roundtrip/refcount artifacts (`roundtrip_read.coma`, `roundtrip_write.coma`, `roundtrip_downgrade.coma`) | L1/L2 target | Strong local invariants; full map-wide theorem pending. |

## Secondary track (later phases)

- Background write-through eventuality.
- Shutdown drain/join temporal properties.
- SSD evictor periodic hysteresis.
- Async stream semantics and synchronization.
- Performance/throughput claims (tracked separately from strict functional proof).

## Document Evolution Summary

- Extended this file to carry concrete proof evidence, artifact pointers, and abstraction levels.
- Imported key Claude July dispatcher proof details directly into active registry (P2, P20, P21–P24 status transitions).
- This file is now sufficient as the primary property/proof map even if historical docs are removed.
