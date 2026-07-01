# Trusted Ledger (Codex) - July 1

Purpose:

- Track all current `#[trusted]` uses in dispatcher Creusot artifacts.
- Separate core proved logic from residual trusted proof debt.
- Define a concrete de-trusting order.

## Current Trusted Items (P21 Track)

1. `lemma_same_slot_staging`  
   - Location: [src/lib.rs:190](/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:190)  
   - Why trusted:
     - Mathematical transport fact is trivial from `same_slot`.
     - SMT currently fails on `state_eq` nested-match unfolding.
   - Risk:
     - Low. Pure logical transport lemma.

2. `lemma_same_slot_block_device`  
   - Location: [src/lib.rs:196](/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:196)  
   - Why trusted:
     - Same reason as staging lemma; predicate transport through `same_slot`.
   - Risk:
     - Low. Pure logical transport lemma.

3. `p21_m1_prepare_commit_consumes_once`  
   - Location: [src/lib.rs:511](/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:511)  
   - Why trusted:
     - Remaining tail VC fails on tuple-projection/result-field unification in nested postcondition match.
     - Body-level `proof_assert!` chain establishes lifecycle facts.
   - Risk:
     - Medium. Trusted wrapper over otherwise mostly proved chain.

4. `p21_m1_prepare_cancel_consumes_once`  
   - Location: [src/lib.rs:560](/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:560)  
   - Why trusted:
     - Same tuple-projection tail-VC issue as commit variant.
   - Risk:
     - Medium.

5. `p21_m2_prepare_then_terminal_ops_miss`  
   - Location: [src/lib.rs:615](/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:615)  
   - Why trusted:
     - Same tuple-projection tail-VC issue in mode-split postcondition form.
   - Risk:
     - Medium.

## De-Trusting Order

1. Remove `#[trusted]` from the three P21 wrapper functions first (`M1-commit`, `M1-cancel`, `M2-miss`).  
   - Reason: highest value for claim strength (`P21` coverage evidence).
2. Keep transport lemmas temporarily trusted while refactoring postconditions to avoid solver tuple-projection weakness.
3. Replace transport lemmas with non-trusted proofs once `same_slot/state_eq` decomposition is simplified enough for SMT.

## Current Claim Boundary

- Core P21 behavioral chain is modeled and body-proven with explicit proof assertions.
- Residual trust is localized to:
  - 2 predicate-transport lemmas,
  - 3 wrapper-level tail VCs.
- This should be reported as: **covered with trusted tail VC debt**, not assumption-free full proof.
