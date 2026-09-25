# component-verify — how this pipeline is organised

Entry point for per-component formal verification (Creusot + Kani). This file records
**what lives where and why**, so anyone picking the pipeline up — or auditing a result —
can orient without reconstructing decisions from git history.

`SKILL.md` is the agent-facing procedure. This file is for the human running it.

---

## 1. Where the code lives — settled

**Branch `skills/verify-methodology-to-unstable`, folder `.claude/skills/`.**

That branch is the single home until it is pushed and PR'd to `unstable`. It is based
directly on `origin/unstable` (parent commit `4217812f`), so it carries the full repo plus
our skill changes, and merges forward cleanly.

| What | Where |
|---|---|
| Orchestrator | `.claude/skills/component-verify/SKILL.md` |
| The gate (4 scripts + 4 registries) | `.claude/skills/component-verify/gate/` |
| Role 1 — property inventory | `.claude/skills/build-property-inventory/` |
| Blind extraction primitive | `.claude/skills/extract-verifiable-properties/` |
| Role 2 — Creusot | `.claude/skills/tools-verify-creusot-with-properties/` |
| Role 2 — Kani | `.claude/skills/tools-verify-kani-with-properties/` |
| Role 2.6 — boundary refuter | `.claude/skills/refute-tool-boundary/` |
| Role 3 — scoring page | `.claude/skills/tools-aggregate-coverage-by-interface/` |

The gate, in `component-verify/gate/`:

- `run_stage.py` — stage runner: `SENTINEL` start/done markers, transcript tee, `.done`
  completion marker, watchdog that kills a wedged stage's whole process tree.
- `scorer_kani.py`, `scorer_creusot.py` — **the gate.** They re-run every artifact from
  source and write each property's `status`. The proving agent never grades itself.
- `render_scoring.py` — the one combined `<component>_scoring.html`.
- `lever_battery_{kani,creusot}.yaml`, `known_defeats.yaml`, `inexpressible_creusot.yaml` —
  registries the scorers enforce.

### Two dependencies that are NOT ours to change

`tools-verify-creusot` and `tools-verify-kani` hold the actual proof/harness mechanics; the
`…-with-properties` skills are a thin layer over them ("this skill = `tools-verify-creusot`
+ spec pairing + status write-back"). Both were already current on `unstable`, so our commits
left them alone. **They are live dependencies — not deprecated, do not remove.**

`component-check-spec-translation`, `component-check-verif-translation` and
`component-check-leakage` are **not part of this pipeline.** They are separate read-only
audits; our skills reference them only to state that boundary.

---

## 2. Where results go — two places, on purpose

**(a) Two git branches per component — the durable, reproducible record.**

    verif/creusot/<component>      verif/kani/<component>

Each branch carries the *identical* full metadata bundle (`spec_properties.yaml`,
`code_properties.yaml`, `unified_properties.yaml`, and the combined
`<component>_scoring.html`) **plus that one tool's proof artifacts**. So either branch stands
alone: it shows the complete scoring picture and carries reproducible proofs for its tool.
Branches are overwritten in place on a re-run; git history keeps the prior commit. Every
commit is gated to touch only `components/<component>/`.

At 22 components × 2 tools that is **44 result branches**. Nothing goes to `unstable`, and
there is no third deliverables branch.

**(b) A dated folder on green — the browsable/shareable copy.**

    /home/cornel/SEPT_2026/SEPT_<NN>/<component>/
        <component>_scoring.html
        spec_properties.yaml
        code_properties.yaml
        unified_properties.yaml

Same shape as the existing `SEPT_22/`. This is what gets read and scp'd to a Mac; the HTML is
self-contained (no CDN or external assets) so it opens anywhere.

The branches are the source of truth; the dated folder is a snapshot for humans.

---

## 3. How a result is traced back to the code that produced it

The branches record *what was proved*. They do not, by themselves, record *which gate version
and which command* produced it — and across a 22-component batch the gate may change between
runs.

**Decision: stamp the provenance into `unified_properties.yaml`**, which already carries `pin`
and `generated`, and which travels onto both verif branches and feeds the HTML's provenance
block:

```yaml
run:
  gate_commit: <sha of this skills branch>
  command:     "component-verify <component> --tools creusot,kani [--no-push]"
  pin:         <origin/unstable commit the run was based on>
  started / finished
  kani_version / creusot_version / why3 prover set
```

Every result then carries its own provenance in the same commit — self-describing, nothing to
keep in sync. **Status: decided, not yet implemented.**

### What we deliberately did *not* build

- **No `MANIFEST.md`.** A commit SHA already pins file content exactly. A hand-maintained
  inventory adds nothing git guarantees, and a stale one actively misleads.
- **No `RUNS/` index.** The 44 branches plus the dated folder *are* the results ledger. A
  separate index is a second copy of the truth that can disagree with the first.

The rule behind both: prefer a self-describing artifact over a side-ledger that can rot.

---

## 4. Running it

```bash
export PATH="$HOME/.local/share/creusot/bin:$HOME/.cargo/bin:$PATH"
```

Then invoke the skill on a component name:

```
component-verify <component> [--tools creusot,kani] [--no-push] [--dry-run]
```

- `--dry-run` — rehearse Roles 1–3 and the contamination gate; no branch, commit, or push.
  Use this first on any component.
- `--no-push` — do everything locally including commits, but never push.

The orchestrator refuses to start on a dirty tree, makes a **fresh worktree off
`origin/unstable`** for every run, and checks the SMT portfolio (alt-ergo, z3, cvc5, cvc4)
before any Creusot run. Those are hard preconditions, not advice.

`SKILL.md` has the full procedure; don't duplicate its commands here — that is how runbooks
drift.

### Checking a run you left unattended

Each heavy stage runs under `run_stage.py`, so progress is visible without watching:

```bash
tail -f components/<component>/verif/.run/kani.log      # or creusot.log
ls    components/<component>/verif/.run/*.done          # stage completion markers
grep  SENTINEL components/<component>/verif/.run/*.log  # START / DONE exit=<n>
```

A wedged stage is killed by the watchdog (whole process tree, so no orphaned cbmc/why3/solver
processes survive). Scorers checkpoint `unified_properties.yaml` atomically after **every**
property, so an interrupted run resumes with `--resume` instead of re-proving everything.

### Reading the outcome

Every run ends with exactly one line:

| Verdict | Meaning |
|---|---|
| `COMPONENT-VERIFY: PASSED` | Both scorers exited 0, the `--require-complete` render wrote the deliverable, every tool-boundary survived the refuter. Branches committed. |
| `COMPONENT-VERIFY: FAILED` | A hard precondition, the zero-artifact stop, the contamination gate, or the render blocked it. Nothing committed. |
| `COMPONENT-VERIFY: PAUSED(iterate-cap)` | 3 re-attempt rounds reached with properties still UNRESOLVED. Nothing committed; outstanding ids listed; `--resume`-able. |

Pausing is a first-class outcome, not a hidden failure. A run that cannot finish stops and
hands back a bounded work-list rather than spinning.

### Why you can trust a `proved`

The proving agent produces artifacts; it never writes its own status. The scorer re-runs each
artifact **from source** (Creusot `touch`es the `.rs`; Kani rebuilds), so a hand-edited
artifact cannot fake a pass. A claimed tool-boundary must survive the full lever battery, must
not match a signature in `known_defeats.yaml`, and must then survive an independent refuter
agent. Anything else is `UNRESOLVED`, which fails the gate — it is never persisted as a rating.

---

## 5. Status

| Item | State |
|---|---|
| 7 skills + 8 gate files consolidated on the branch | done |
| `creusot-std` path made checkout-relative (portable to another clone) | done |
| Gate smoke-tested in place (block-device-filesys dry-run reproduces baseline) | done |
| Provenance `run:` stamp (§3) | decided, not implemented |
| Orchestrator end-to-end run | **never executed — first run must be supervised** |
| Push / PR to `unstable` | pending, supervised |

The orchestrator has not yet been run end to end. The plan is a `--dry-run` on `dispatch-map`,
then one supervised real run, before any unattended batch.

Older `component-verify` copies still exist in the dated per-component worktrees
(`*-verify-2026*/`). Those are stale run-time snapshots, not edit targets. **This branch is
the only place to change the pipeline.**
