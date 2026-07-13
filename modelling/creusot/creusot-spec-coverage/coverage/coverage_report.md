# Coverage Report

Purpose:
- Current, human-readable coverage status for `properties_to_prove.md`.
- Includes concrete proof artifacts so this report remains useful without `history/` docs.

## Status legend

- **Verified**: proved and aligned with current runtime/spec.
- **Partial**: useful proof evidence exists but not at full required scope.
- **Unchecked**: no sufficient proof yet.
- **Stale**: proof artifact exists but mirrors removed/reworked code path.
- **Retired**: property no longer active in current spec scope.

## Snapshot (current)

- Total properties: **31**
- Verified: **4**
- Partial: **14**
- Unchecked: **11**
- Stale: **2**
- Retired: **2**

## Verified properties with concrete evidence

| Property | Component | Proof function(s) | Artifact(s) | Abstraction | Notes |
|---|---|---|---|---|---|
| P2 | dispatcher | `ensure_initialized` | `dispatcher_verif_rlib/ensure_initialized.coma` | L0 | Live and aligned (Claude July). |
| P20 (legacy but still valid guard semantics) | dispatcher | `prepare_store_guards` (re-anchored to `populate` guard behavior) | `dispatcher_verif_rlib/prepare_store_guards.coma` | L0 | Live guard proof; requirement scope became legacy after direct-store removal. |
| P18 (local strength) | dispatch-map | `convert_memory_tier_to_block`, lifecycle proofs including write-through safety | `dispatch_map_verif_rlib/convert_memory_tier_to_block.coma`, `.../lifecycle_memory_tier_to_block.coma`, `.../lifecycle_write_through_safety.coma` | L1 | Strong per-entry evidence; full dispatcher composition still partial. |
| P27 (local strength) | dispatch-map | `recover_extent`, `lifecycle_recover_extent` | `dispatch_map_verif_rlib/recover_extent.coma`, `.../lifecycle_recover_extent.coma` | L1 | Strong per-entry recovery evidence. |

## Stale / retired proofs captured explicitly

| Property | Prior proof artifact(s) | Why stale/retired | Current action |
|---|---|---|---|
| P21 | `dispatcher_verif_rlib/insert_pending.coma`, `.../consume_once.coma` | Mirrors removed `pending_writes` workflow | Keep as historical evidence; do not treat as active guarantee. |
| P24 | `dispatcher_verif_rlib/consume_pending.coma` | Same removed workflow | Same treatment. |
| P22 | none active | Workflow removed in newer spec/runtime | Retired. |
| P23 | none active | Workflow removed in newer spec/runtime | Retired. |

## Partial coverage cluster (why still partial)

- Most partial items are currently supported by **per-entry** dispatch-map proofs (`L1`) without complete **map-wide/system-level** composition.
- Main partial groups: P3–P10, P12–P13, P17, P26, P30, P31.
- Practical meaning: safety patterns are validated locally, but not all end-to-end API obligations are discharged.

## Highest-priority next closures

1. **P11** hard-fail size mismatch in dispatcher lookup paths.
2. **P1** initialization dependency gate proof.
3. **P15/P16** eviction loop postconditions.
4. **P30/P31** map-wide invariant lifting beyond per-entry proofs.

## Evidence provenance (Claude + Codex)

- Dispatcher proof details imported from July Claude reports (`claude_progress_report.md`, `property_coverage_dispatcher_july7.md`).
- Dispatch-map local strength imported from cross-check coverage matrix and artifact inventory.
- This report intentionally consolidates both to avoid dependence on archive files.

## Document Evolution Summary

- Rewritten to include explicit artifact-level evidence and stale/retired context in one place.
- Captures Claude July live/stale proof transitions directly in active documentation.
- Intended to be the first file reviewed for “what is actually proved today”.
