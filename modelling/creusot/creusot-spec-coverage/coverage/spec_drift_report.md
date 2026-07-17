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

## 4) Proof status snapshot (2026-07-16)

Now captured in active maintained files (not only history):
- **All active non-legacy properties are Verified in some form.** 25 of 31 are Verified (18 full + 7 verified-scoped L1); P19 is the only deliberately Partial active property (cross-map concurrent invariant, out of Creusot's sequential model). See `coverage_report.md` for the full breakdown.
- Legacy dispatcher: P20 Verified (guard semantics, re-anchored to `populate`), P21/P24 Stale (`insert_pending`, `consume_once`, `consume_pending`), P22/P23 Retired, P10 Partial (staging).
- Map-wide milestone: P30/P31 lifted to L2 via `map_inv` (exclusive-state + binary write_ref, preserved by insert-fresh/overwrite/remove).
- **Next keystone target:** cross-map (mt↔dm) consistency — the invariant that P4/P5/P8/P9/P19 currently flag as out of scope — plus lifting the remaining L1 verified-scoped cluster to map-wide claims.

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
