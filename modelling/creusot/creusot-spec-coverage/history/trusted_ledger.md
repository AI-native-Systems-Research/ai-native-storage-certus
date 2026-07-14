# Trusted Ledger (Codex) - July 1

Purpose:

- Track current `#[trusted]` uses in dispatcher Creusot artifacts.
- Separate proved core logic from residual trusted proof debt.
- Define de-trusting order.

## Current Trusted Items (P21 Track)

1. `lemma_same_slot_staging`  
   - Location: `/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:190`  
   - Reason:
     - Trivial transport fact from `same_slot`.
     - SMT does not unfold nested `state_eq` match reliably.
   - Risk: low.

2. `lemma_same_slot_block_device`  
   - Location: `/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:196`  
   - Reason:
     - Same transport issue as above.
   - Risk: low.

3. `p21_m1_prepare_commit_consumes_once`  
   - Location: `/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:511`  
   - Reason:
     - Residual tail VC on tuple-projection/postcondition unification.
     - Body `proof_assert!` chain establishes lifecycle content.
   - Risk: medium.

4. `p21_m1_prepare_cancel_consumes_once`  
   - Location: `/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:560`  
   - Reason:
     - Same tuple-projection tail-VC issue.
   - Risk: medium.

5. `p21_m2_prepare_then_terminal_ops_miss`  
   - Location: `/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/src/lib.rs:615`  
   - Reason:
     - Same tuple-projection tail-VC issue in mode-split postconditions.
   - Risk: medium.

## De-Trusting Order

1. Remove `#[trusted]` from the three P21 wrapper functions first.
2. Keep transport lemmas trusted temporarily while simplifying postconditions.
3. Replace transport lemmas with non-trusted proofs once `same_slot/state_eq` decomposition is SMT-friendly.

## Current Claim Boundary

- Core P21 chain is modeled and body-proven with explicit proof assertions.
- Residual trust is localized to two transport lemmas plus three wrapper-level tail VCs.
- Report status as: covered with trusted tail VC debt.
