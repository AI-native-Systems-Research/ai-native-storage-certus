# Repair Agent

Fixes Certus code that verification proved violates an obligation, or escalates when the obligation cannot be satisfied as written.

Fix the CODE behind a verification finding, completely, and hand a non-expert reviewer a pull request they can approve from the evidence alone. The finding is either a LEVEL-2 BUG (a property spec and code agree on that formal verification refuted) or, on request, a LEVEL-1 DISCORDANCE from the component's discordances.yaml (spec and code disagree). For a discordance, FIRST confirm it: write a test that fails on today's code; if no such test can be written, report it withdrawn (a misreading) and change nothing. Then choose the fix yourself — the reviewer may not read Rust — and implement it everywhere it is needed, across components and shared interfaces if the root cause lives there; add regression tests; make the proofs of the target properties prove; and keep every touched, already-verified component verified.

Scaffolded by `./build_agent` as a tier T3 agent. `spec.yaml` is the confirmed intent,
`architecture.yaml` the derived design, and `agent.yaml` the machine-checked contract.
Runtime output belongs under `runs/`.

## Pieces

- `repair_accept_repo.sh <repo> <base-rev> --targets "c:ID:mods;..."`: the oracle. Every touched
  crate and its dependents compile, touched crates' tests pass, every target passes
  `repair_accept.sh`, proof models follow the code they mirror, and no proof (or gate-scored
  property, when the checkout has the gate) that held at base is lost. It prints the EVIDENCE table.
- `repair_accept.sh` and `repair_oracle.sh` cover one target: PROVED (the callee modules and the
  new `verify_<id>` prove, and its `__mutant` fails with the prover having run), REAL (`src/`
  changed), FAILS-BEFORE (the `repair_test_*.rs` fails when injected into base), TESTED, and
  HARMLESS.
- `checks.py`: obligation hashes, frozen pre-existing proof functions, no trust escapes, and
  writable paths, all checked against base.
- `handlers.py`: input derivation, the toolchain link, the hand-off report (`.repair/report.md`),
  the approval packet, and the result ref. `actions.py`: the pull request, opened only after
  approval in an attended run.

`oracle_dir` must point at this directory (or a copy of it) outside the repository.

## Use

```bash
agents/repair-agent/run.sh --help
agents/repair-agent/run.sh --self-check
./build_agent validate agents/repair-agent
./build_agent evaluate agents/repair-agent
```
