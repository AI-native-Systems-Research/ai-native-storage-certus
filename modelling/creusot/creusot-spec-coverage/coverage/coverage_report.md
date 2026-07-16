# Coverage Dashboard

Purpose:
- One-screen status of the Creusot proof effort for `dispatcher` + `dispatch-map`.
- Ground truth is `properties_to_prove.md` (registry) and the `.coma` artifacts in each verif crate.
- To find *where* a given property's proof lives (crate, function, `.coma`, plain English), see `proof_locator.md`.

_Last refreshed: 2026-07-16. Baseline: properties P1–P31._

## Status legend

- **Verified**: proved and aligned with current runtime/spec.
- **Verified (scoped)**: proved at a deliberately narrowed model scope (per-entry / single-map decision); the honest boundary is stated in the registry Notes.
- **Partial**: useful proof evidence exists but not at full required scope.
- **Stale**: proof artifact is green but mirrors a removed/reworked code path (legacy).
- **Retired**: property no longer active; no artifact carried.

## Headline (by status)

| Status | Count | Properties |
|---|---:|---|
| Verified | 18 | P1, P2, P3, P6, P7, P11, P12, P13, P14, P15, P16, P17, P25, P26, P28, P29, P30, P31 |
| Verified (scoped) | 7 | P4, P5, P8, P9, P18, P27, P20¹ |
| Partial | 2 | P10¹, P19 |
| Stale | 2 | P21¹, P24¹ |
| Retired | 2 | P22¹, P23¹ |
| **Total** | **31** | |

¹ legacy scope (direct-store / staging / pending-writes workflows removed from runtime).

**Bottom line:** every active, non-legacy property (P1–P19 minus legacy P10, P25–P31) now has a green proof. 25 of 31 are Verified in some form; only P19 remains deliberately Partial (its full guarantee is a cross-map concurrent invariant, out of Creusot's sequential model). The 6 legacy properties (P10, P20–P24) mirror APIs no longer in the runtime.

## By abstraction level (project-local scale)

`L0` near-runtime · `L1` per-entry/ghost-local · `L2` map-wide · `L3` bounded/opaque-oracle · `Lx` stale.

| Level | Count | Properties |
|---|---:|---|
| L0 | 14 | P1, P2, P3, P6, P7, P11, P12, P13, P14, P16, P17, P20, P28, P29 |
| L1 | 8 | P4, P5, P8, P9, P18, P27 (verified-scoped) · P10, P19 (partial) |
| L2 | 4 | P25, P26, P30, P31 |
| L3 | 1 | P15 |
| Lx | 4 | P21, P22, P23, P24 (legacy) |

The L1 verified-scoped cluster (P4/P5, P8/P9, P18, P27) is where the next refinement work sits: lifting per-entry / single-map decisions to map-wide (L2) theorems. P30/P31 already show that lift is discharged for the three map-mutation shapes (insert-fresh / overwrite / remove) — see assumption A7 in `assumptions_and_trusted.md`.

## By component (where the primary proof runs)

| Component | Verif crate | Properties (primary proof) | `.coma` files |
|---|---|---|---:|
| Dispatcher | `components/dispatcher/verif` | P1, P2, P3, P4, P5, P6, P7, P8, P9, P11, P12, P13, P14, P15, P16, P17, P19, P20, P25, P26, P28, P29 (+ legacy P21, P24) | 21 |
| Dispatch-map | `components/dispatch-map/verif` | P18, P27, P30, P31 (+ per-entry evidence for P3, P8, P9, P12, P13, and legacy P10) | 32 |

Full-crate replays green on both crates (dispatcher 21 `.coma`, dispatch-map 32 `.coma`).

## Ownership API (which interface owns each property)

| Owner interface | Property groups | Main proof location |
|---|---|---|
| `IDispatcher` | P1, P2, P4, P5, P7–P11, P14–P17, P19, P25, P26, P28, P29 (+ legacy P20–P24) | `components/dispatcher/verif` |
| `IDispatchMap` | P3, P6, P12, P13, P18, P27, P30, P31 | `components/dispatch-map/verif` |
| Shared composition | P3, P12, P13, P18, P27 | dispatcher + dispatch-map composition argument |

Reviewer guidance:
- If the owner is `IDispatcher`, a dispatch-map-only proof is *insufficient* on its own.
- If the owner is `IDispatchMap`, per-entry proofs help but may still need map-wide lifting for full claim strength (see P30/P31 for the discharged pattern).

## Deliberately out of scope (not gaps to close now)

- **P19 (blind-evict fallback):** sequential single-map decision is proved; the real guarantee is a cross-map (mt↔dm) whole-map invariant under *concurrent* eviction — outside Creusot's sequential model. Belongs with the P30/P31 map-wide track.
- **Cross-map (mt↔dm) consistency for P4/P5/P8/P9:** the Phase-2 "mt slot reserved but no dm entry" leak is a cross-map invariant, tracked with P30/P31, not with the per-property registration proofs.
- **Legacy P10, P20–P24:** direct-store / staging / pending-writes workflows were removed from the runtime. Artifacts are kept as historical evidence, not active guarantees.
- **Secondary track:** background write-through eventuality, shutdown drain/join temporal properties, async stream semantics — tracked separately from strict functional proof.

## Provenance

- Dispatcher proofs: July Claude reports (`property_coverage_dispatcher_july7.md`) plus this sprint's additions (P1, P3, P6, P7, P8, P9, P11, P12, P13, P14, P15, P16, P17, P19, P25, P26, P28, P29).
- Dispatch-map map-wide lift (P30/P31) and local strength (P18, P27): cross-check coverage matrix + artifact inventory.
- This dashboard is derived from `properties_to_prove.md`; if they disagree, the registry wins.
