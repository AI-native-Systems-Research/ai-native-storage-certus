# Assumptions and Trusted Boundaries

Purpose:
- This maintained file records the proof-strength limits of current verification artifacts.
- It links assumptions/trusted items to concrete properties so reviewers can see where abstraction may hide risk.

## How to read

- **Assumption**: a condition required by the model/proof.
- **Trusted boundary**: theorem/function accepted without full local discharge.
- **Linked properties**: property IDs currently affected.
- **Risk level**: practical risk to over-claiming correctness.

Abstraction level reference:
- `L0`: near-runtime logic mirror.
- `L1`: local ghost model (per-entry/per-function).
- `L2`: map-wide ghost abstraction.
- `L3`: bounded/temporal abstraction with stronger assumptions.
- `Lx`: stale or retired workflow.

> Terminology note: `L0`–`L3`/`Lx` is a **project-local convention**, not standard formal-verification vocabulary. It approximates the standard notion of **model-to-code refinement distance** — how much abstraction separates the verified model from the running Rust. `L0` ≈ near-runtime; higher numbers add abstraction. Lifting a level corresponds to a concrete formal obligation (e.g. `L1`→`L2` = supplying the map-wide **abstraction function / gluing invariant**).

## Active assumptions (model-level)

| ID | Plain-English meaning | Linked properties | Level | Risk |
|---|---|---|---|---|
| A1 | Some global claims use bounded key-space model instead of full unbounded map. | P30, P31, parts of P25/P26 | L2/L3 | medium |
| A2 | Progress/liveness claims assume fairness (scheduler keeps making progress). | Secondary temporal track | L3 | medium |
| A3 | Some eventuality claims are bounded-step, not fully unbounded temporal theorems. | Secondary temporal track | L3 | medium |
| A4 | Background write-through fault detail is abstracted to high-level success/failure. | FR-004/FR-017 style secondary claims | L3 | medium |
| A5 | Async stream model is coarse (few stream states, reduced concurrency realism). | Secondary async track, future P11 extensions | L2/L3 | medium |
| A6 | Arithmetic preconditions are explicit to keep solver obligations total/tractable. | P15, P16, P28, P29 and loop proofs | L0/L2 | low-medium |
| A7 | Dispatch-map global properties are inferred from per-entry proofs plus composition reasoning. **Partially discharged (2026-07-16):** `map_inv` (P30/P31) now proves the map-wide exclusive-state + binary-write_ref invariant is preserved by the three mutation shapes — insert-fresh, overwrite, remove — closing the L1→L2 gluing gap for those ops. Remaining open part: cross-map (mt↔dm) consistency and concurrent interleavings. | P3, P6, P12, P13, P30, P31 | L1/L2 | low-medium |

## Trusted boundaries (proof-level)

### Current project-level trusted items

| Item | Where | Linked properties | Why trusted | Status | Risk |
|---|---|---|---|---|---|
| `under_pressure` (opaque pressure oracle) | dispatcher verif, `evict_attempt_budget` (P15) | P15 | The eviction while-guard `used + needed > capacity` reads concurrently-mutated external state. Modeled as a `#[trusted]` oracle returning an arbitrary bool each call, so the bound is proved for **every** possible pressure behavior rather than one fixed trace. | active | low |
| `tier_used` (no-overflow bound) | dispatcher verif, `evict_for_capacity` (P16/P17) | P16, P17 | `#[trusted]` function ensuring `result@ + needed@ <= usize::MAX@` — a byte count plus one allocation never overflows 64-bit `usize`. Used only for the guard's in-bounds check; eviction's effect on `used` stays opaque (P16 claims "success ⇒ capacity holds", not progress). | active | low |
| `lemma_same_slot_staging` | dispatcher verif model (legacy P21 track) | P21 | SMT transport/unfolding limitation | stale-context | low |
| `lemma_same_slot_block_device` | dispatcher verif model (legacy P21 track) | P21 | SMT transport/unfolding limitation | stale-context | low |
| `p21_m1_prepare_commit_consumes_once` | dispatcher verif model | P21 | residual tuple-projection tail VC | stale-context | medium |
| `p21_m1_prepare_cancel_consumes_once` | dispatcher verif model | P21 | same tail VC pattern | stale-context | medium |
| `p21_m2_prepare_then_terminal_ops_miss` | dispatcher verif model | P24 | same tail VC pattern | stale-context | medium |

### Toolchain-level trusted primitives

| Item | Where used | Linked properties | Why it matters |
|---|---|---|---|
| `creusot_std::logic::FMap` ghost primitives (`insert_ghost`, `remove_ghost`, `remove_one_ghost`, `get`, `contains`, `ext_eq`) | pervasive across active dispatcher + dispatch-map map models, plus legacy stale proofs | P3, P4, P5, P7, P8, P9, P12, P13, P25, P26, P30, P31 (active); P21, P24 (historical) | All `#[trusted]` in creusot-std. Enables map reasoning where std `HashMap` insert/remove/membership specs are missing in current creusot-std extern coverage. This is the foundational trusted layer under every FMap-based proof. |

## Proof-track notes

- The July live dispatcher guards (P2, P20) require **no** extra project trusted lemmas.
- The eviction proofs (P15, P16/P17) each introduce exactly one `#[trusted]` oracle (`under_pressure`, `tier_used`) to abstract concurrently-mutated external state and a no-overflow bound; see the trusted-boundaries table.
- All FMap-based map proofs (the bulk of the active set) rest on the `creusot_std::logic::FMap` ghost primitives, which are `#[trusted]` in creusot-std.
- Stale dispatcher proofs (P21/P24) rely on the same FMap ghost modeling and are preserved as historical evidence only. The stale transition happened after commit `25a7273` removed `prepare_store/commit_store/cancel_store/pending_writes` from the active runtime path.

## What this means for non-formal-method readers

- Current strongest evidence is for local/per-entry safety and a small number of live dispatcher guards.
- Risk rises when we extrapolate local proofs to full system behavior without map-wide or dispatcher-level composition proofs.
- `Stale` and `Retired` items are explicitly tracked to prevent accidental over-claims.

## Assumption reduction plan

_Progress as of 2026-07-16._

1. ~~Prioritize P11, P1, and eviction postconditions at dispatcher level (`L0` targets).~~ **Done** — P1, P11 (L0), and P15/P16/P17 eviction postconditions are Verified.
2. ~~Lift map-wide invariants P30/P31 from per-entry (`L1`) toward explicit map ghost invariants (`L2`).~~ **Done for the three mutation shapes** via `map_inv` (insert-fresh/overwrite/remove); see A7. Remaining: cross-map (mt↔dm) consistency and concurrent interleavings.
3. **Next:** lift the L1 verified-scoped cluster (P4/P5, P8/P9, P18, P27) toward map-wide claims, and close the cross-map mt↔dm invariant that P4/P5/P8/P9/P19 currently flag as out of scope.
4. Reduce/retire stale trusted wrappers tied to the removed pending-write workflow (P21/P24 lemmas).
5. Introduce stronger temporal reasoning gradually only after the safety baseline is broadened (secondary track).

## Document Evolution Summary

- Expanded from a generic ledger into a property-linked risk map.
- Added explicit capture of Claude July stale/live transition details.
- File now serves as the authoritative explanation of abstraction depth and trust debt for current claims.
