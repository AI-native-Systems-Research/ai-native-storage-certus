# Explanation Report (Codex) - July 2

Purpose:

- Explain the cross-check results in plain English for a broader team audience.
- Keep technical detail, but remove verifier jargon where possible.
- Clarify what is already verified, what is not verified, and what we should do next.

Related technical report:

- [crosscheck_codex_july2.md](/home/cornel/ai-native-storage-certus/components/dispatch-map/verif/crosscheck_codex_july2.md)

## What We Checked

We reviewed the Creusot verification code in:

- `/home/cornel/ai-native-storage-certus/components/dispatch-map/verif`

and compared it to the spec-derived property baseline we built earlier (the `P1..P31` style property set extracted from `dispatcher/spec.md`).

We also ran the verifier directly:

- `cargo creusot` in `components/dispatch-map/verif`
- Result: **Proved (28 files)**.

This means the proofs inside this crate are currently passing.

## Key Point for the Team

“28 files proved” is good, but it does **not** mean “the whole dispatcher behavior is verified.”

Reason:

- This crate is mostly proving **per entry level** behavior.
- Per entry level means: one `DispatchEntry` record at a time (local state transitions and local refcount safety).
- It is not yet a full system-level proof over all keys, all APIs, and all dispatcher workflows.

## What Is Verified Well Right Now

The current `dispatch-map/verif` crate does a strong job on local safety rules:

1. Reference count rules on one per entry record.
   - Taking and releasing read/write refs behaves correctly.
   - Basic overflow/underflow guard behavior is modeled.
2. Per entry location transitions.
   - Staging/MemoryTier/BlockDevice transitions are modeled and constrained.
3. Eviction safety preconditions on one per entry state.
   - A referenced per entry state is not evictable.
   - Per entry state becomes evictable only after references are cleared.
4. Fairness shape for eviction ordering.
   - TSC-order predicates show “older before newer” under sorted-list assumptions.

This is valuable foundation work and should be kept.

## What Is Not Verified Yet (Important Gaps)

Even with 28 proved files, several spec-critical areas are still outside this crate’s coverage:

1. Dispatcher initialization/dependency gate (`P1`, `P2` family).
   - Not covered here because this crate focuses on dispatch-map per entry logic.
2. Full map-level behavior across many keys.
   - Not fully covered (local per entry rules are covered; map-wide relation is not fully modeled).
3. Dispatcher store lifecycle (`prepare_store` -> `commit_store/cancel_store`).
   - Not covered here (that belongs to dispatcher-level verification).
4. Background/temporal behavior (shutdown drain/join, async stream semantics, eventuality).
   - Not covered here.
5. End-to-end refinement argument.
   - We do not yet have a complete proof that “abstract model + contracts” fully refines production dispatcher behavior.

## Concrete Cross-Check Finding the Team Should Know

A concrete risk remains around lookup size-mismatch semantics:

- In `components/dispatch-map/src/lib.rs`, `lookup` returns location info but does not take a requested-size argument.
- So this crate cannot directly prove a property like:
  - “If requested size mismatches stored size, operation must hard-fail with no partial success.”

Why this matters:

- That mismatch-hard-fail rule is a key safety expectation from the spec-derived property set (our `P11` line of reasoning).
- Because dispatch-map lookup API has no requested-size input, this rule must be proven at dispatcher layer (or redesign API ownership explicitly).

So the team should not claim this crate alone closes the `P11` safety story.

## What We Need to Do Next

### 1) Add a clear coverage matrix for this crate

Create a table with `P1..P31` rows and mark each as:

- `Covered in dispatch-map/verif`
- `Partial`
- `Out of scope for this crate`

This avoids confusion when reporting proof counts.

### 2) Decide ownership of lookup mismatch property (`P11`)

Two options:

1. Keep ownership in dispatcher verification:
   - Dispatch-map remains a storage/index primitive.
   - Dispatcher enforces requested-size contract.
2. Move part of ownership down into dispatch-map API:
   - Change API/model to include requested-size checks.

Either is possible, but it must be explicit and documented.

### 3) Strengthen map-level proofs (not just per entry level)

Introduce a map-level ghost model for key presence and uniqueness semantics:

- Better proof of `AlreadyExists`, `KeyNotFound`, and global key-state consistency.
- Better bridge from local per entry invariants to system claims.

### 4) Link dispatch-map proofs and dispatcher proofs explicitly

Use dispatch-map proofs as building blocks, then prove dispatcher-level properties:

- `P11` lookup mismatch hard-fail
- `P21-P24` prepare/commit/cancel lifecycle
- init gate and mode-split behavior

This is the path to real end-to-end confidence.

### 5) Keep proof debt transparent

If trusted lemmas/assumptions are used, keep a ledger and audit trail.
This helps reviewers understand exactly what is proved vs trusted.

## Suggested Presentation Narrative for the Team

You can present the result as:

1. “We validated that existing dispatch-map Creusot proofs are healthy (28 proved files).”
2. “These are strong local safety proofs on per entry behavior.”
3. “They are not yet full dispatcher/system coverage.”
4. “We identified specific uncovered obligations (notably lookup mismatch ownership and dispatcher lifecycle properties).”
5. “We have a concrete, staged plan to close those gaps with traceable property coverage.”

This framing is accurate, technically defensible, and easy for a mixed audience to follow.
