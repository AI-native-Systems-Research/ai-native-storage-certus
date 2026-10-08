# extent-manager — Creusot level-3 re-score (node7, 2026-10-07)

Source crate: `verif-creusot-l3/` in node7's run tree (pin 2cd35bac). Gate: ~/fv-skills/skills @4c83ef4c
`scorer_creusot.py --cap-seconds 60 --cap-max 300`, every Creusot status stripped first.

- Gate: **128 proved · 19 refuted · 13 UNRESOLVED → FAILED (fail-closed)**, 9 min 48 s, 1.1 GB.
- Independent mutant audit: 147/147 proved modules vouched by a failing mutant. Forced whole crate: 225 unproved = 225 `__mutant`. `#[trusted]` 37.
- cross_check.py: 0 contradictions (the Kani column is the 10-06 run, not re-scored here).
- 13 UNRESOLVED = 12 NOT CREDITED (premise outside obligation / declared range / proved invariant; the drivers are kept
  as `narrowed_verify_<id>` so the gate cannot credit them) + EM-INV-CHECKPOINT-ROUNDTRIP (UNFINISHED).
- Every refute_ asserts the proved invariants and declared ranges on its starting state (`src/props/l3start.rs`).
- Declared assumptions are the named `a_*` predicates in `src/model/assume.rs` (21 level2_assumptions; 5 are a level-1
  ADDENDUM of 2026-10-07, 2 corrected by one value — see `domain_discordances` notes in ../verif/unified_properties.yaml).
Full log: node7 `~/FV/NODE7_PROGRESS_20261007.md` (REPORT 2a).
