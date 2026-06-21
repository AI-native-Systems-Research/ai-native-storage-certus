# Assumption Ledger (Codex) - June 14

Purpose:

- Record assumptions used by current formal models/proofs in `tools/creusot/dispatcher-verif`.
- Make proof strength explicit for review/comparison.
- Separate "proved under assumptions" from "assumption-free guarantees".

## Categories

1. **Modeling assumptions**: simplifications relative to production implementation.
2. **Progress/fairness assumptions**: conditions required for eventuality/liveness claims.
3. **Boundedness assumptions**: finite-step or finite-key restrictions.
4. **Arithmetic/safety assumptions**: preconditions used to avoid overflow/undefined behavior in proofs.

## Active Assumptions

### A1 - Bounded key-space in selected global proofs

- Where used:
  - `Cache2` model (`clear_memory_tier2`, `wf_cache2`, `memory_tier_count2`)
- Meaning:
  - Some global properties are proved for a 2-key bounded abstraction, not an unbounded key map.
- Impacts:
  - Strong for pattern validation.
  - Not yet a universal "for all keys" theorem.

### A2 - Fairness gate for background progress

- Where used:
  - `writer_step`, `shutdown_drain_join`
- Meaning:
  - Progress occurs when `fair=true`; without fairness, model allows no progress.
- Impacts:
  - Liveness-style conclusions are conditional on fairness.

### A3 - Bounded-step eventuality

- Where used:
  - `drain_jobs_in_steps`, `shutdown_drain_join`
- Meaning:
  - "Eventually drains" is represented as "drains within N steps" (with sufficient step budget).
- Impacts:
  - Proves bounded eventuality shape.
  - Not yet unbounded temporal proof.
 - Update (June 15):
   - strengthened with explicit progress lemmas:
     1. monotone non-increase (`lemma_drain_monotone`)
     2. one-step strict decrease when work exists (`lemma_drain_one_step_decrease`)
     3. eventual-zero under sufficient steps (`lemma_drain_eventual_zero`)
   - shutdown contract now includes monotonic and strict-progress clauses, reducing reliance on only a single bound-style postcondition.

### A4 - Simplified failure semantics for background write-through

- Where used:
  - `writer_step` comment/behavior
- Meaning:
  - Job consumption models FR-017 behavior (job can complete or be dropped-on-failure), but details of device error classes are abstracted.
- Impacts:
  - Correct high-level queue progress semantics.
  - No detailed fault taxonomy yet.

### A5 - Simplified stream model for async lookup

- Where used:
  - `lookup_async_model`, `stream_synchronize_model`, `lookup_sync_model`
- Meaning:
  - Stream space is abstracted to `{Null, Warm}` and a coarse `AsyncCopyState`.
- Impacts:
  - Captures contract-level semantics for FR-036/FR-037.
  - Does not model full CUDA stream lifecycle/resource contention.

### A6 - Arithmetic safety preconditions

- Where used:
  - e.g., `evict_for_space`, `drain_jobs_in_steps`, `bounded_reduce`
- Meaning:
  - Preconditions constrain arithmetic domains to keep proofs explicit and total.
- Impacts:
  - Improves proof tractability.
  - Requires eventual justification against implementation constraints.

## Assumption-to-Property Mapping (high level)

1. `P24/P25/P29` currently depend on A1 (bounded multi-key model).
2. Secondary-1/2 depend on A2 + A3.
3. Secondary-3 uses simplified bounded sweep semantics plus A6.
4. Secondary-4 uses A5.

## Planned Assumption Reduction

1. Replace bounded `Cache2` with richer multi-key model to reduce A1 impact.
2. Move from bounded-step eventuality to stronger temporal statements where possible (reduce A3).
3. Refine stream model to incorporate more realistic sequencing/resource assumptions (reduce A5).
4. Tie arithmetic preconditions to concrete implementation invariants (tighten A6 justification).

## Current Interpretation

- Safety/correctness core remains strong and mostly assumption-light.
- Temporal/liveness slices are useful and proved, but currently assumption-conditional.
- This ledger should be updated whenever a proof gains or drops assumptions.
