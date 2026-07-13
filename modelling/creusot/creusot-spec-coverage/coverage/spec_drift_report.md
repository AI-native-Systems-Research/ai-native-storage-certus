# Spec Drift and Ownership Report

Purpose:
- Track how spec changes affect property validity and proof placement.
- Keep ownership mapping explicit so team members know where each proof belongs.

## 1) Spec discovery and selected scope

Automated discovery source pattern:
- `components/*/specs/*/spec.md`
- `components/*/.specify/specs/*/spec.md`

Primary scope for this property namespace:
- `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`
- `components/dispatch-map/specs/001-dispatch-map/spec.md`

Why this matters:
- Prevents verifying against obsolete specs.
- Makes component ownership explicit for each property family.

## 2) Confirmed drift impacting current proofs

Key dispatcher drift already reflected in active status:
- Direct-store workflow requirements (`FR-020`, `FR-021`, `FR-022`) were removed/superseded.
- Runtime path changed (no `pending_writes` prepare/commit/cancel protocol).

Impact on properties:
- P20–P24 moved to `legacy` scope; P21/P24 are `Stale`, P22/P23 are `Retired`.
- Claude July proofs for old workflow are preserved as historical evidence, not active guarantees.

Other notable clarifications/additions in newer dispatcher spec:
- `promote_to_memory_tier`
- `evict_lru_for_key`
- `max_eviction_attempts`
- `IRemoteLookup` integration

These indicate future property expansions/rewrites may be needed beyond original P1..P31 wording.

## 3) Property ownership by interface (with proof-location intent)

| Owner interface | Property groups | Main proof location |
|---|---|---|
| `IDispatcher` | P1, P2, P4, P5, P7–P11, P14–P17, P19, P25, P26, P28, P29 (+ legacy P20–P24) | `components/dispatcher/verif` |
| `IDispatchMap` | P3, P6, P12, P13, P18, P27, P30, P31 | `components/dispatch-map/verif` |
| Shared composition | P3, P12, P13, P18, P27 | dispatcher + dispatch-map composition argument |

Reviewer guidance:
- If property owner is dispatcher, dispatch-map-only proof is insufficient.
- If property owner is dispatch-map, per-entry proofs are helpful but may still require map-wide lifting for full claim strength.

## 4) Claude July proof details preserved in active docs

Now captured in active maintained files (not only history):
- Live dispatcher verified: P2 (`ensure_initialized.coma`), P20 (`prepare_store_guards.coma`, re-anchored).
- Stale dispatcher proofs: P21/P24 artifacts (`insert_pending`, `consume_once`, `consume_pending`).
- Retired properties: P22/P23.
- Current next keystone target: P11 (size mismatch hard-fail in lookup paths).

## 5) Open drift-sensitive risks

1. Property text may still mention legacy terms (for example staging/direct-store language) that no longer describe active runtime.
2. Some dispatch-map proof claims are strong locally but can be misread as full dispatcher guarantees.
3. Continuous refactors can invalidate proof-to-code mirroring unless checked at each proof update.

## 6) Update protocol (automation + manual)

1. Use automation (`spec_trace_agent.py`) to discover specs and detect textual drift.
2. Perform manual semantic review:
   - confirm active spec selection,
   - update property scope/owner,
   - update status (`Verified/Partial/Unchecked/Stale/Retired`) with artifact pointers.
3. Update `properties_to_prove.md` and `coverage/coverage_report.md` together.

## Document Evolution Summary

- Reworked to include explicit ownership/proof-location mapping and Claude-July carry-over details.
- Active docs now contain required stale/retired proof context without depending on `history/`.
- This report is now the main place to understand “why a proof status changed”.
