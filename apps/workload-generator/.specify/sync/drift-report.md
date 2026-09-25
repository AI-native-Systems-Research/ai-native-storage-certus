# Spec Drift Report

Generated: 2026-09-25T16:45:45Z
Project: apps/workload-generator — feature 001-synthetic-workload-generator

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 100 (88 FR + 12 SC) |
| ✓ Aligned | 99 (99%) |
| ⚠️ Drifted | 0 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 0 |
| ⏳ Unverified at stated scale | 1 |

Method: every requirement id counted across `crates/**/*.rs`, then each uncited
id checked by hand. This code cites requirements densely, so absence of a
citation is a usable signal — but it is only a signal: 20 ids were implemented
and simply uncited.

## Detailed Findings

### Spec: 001-synthetic-workload-generator — Synthetic workload generator

#### Drifted ⚠️

None outstanding. Four findings were raised by the first run of this analysis
on 2026-09-25 and all four are resolved:

- **FR-045, SC-001** promised a local path with "no daemon", which FR-079
  deleted. Reworded to keep the half that is true — no node list, no hardware
  file, no instance flag — with the supersession cross-referenced. The agent is
  launched as a child process, so the property a user relies on is intact.
- **SC-011, SC-012** required that "the plan queue never reaches zero", which
  no passing run can satisfy: every lane's queue is empty at `t = 0`. Reworded
  to the bound FR-062 actually enforces — lane-time waited under tolerance.
  Verified against two live runs that were **valid** while reporting `min depth
  0`.
- **`--probe` had no requirement at all** → **FR-083**, placed beside FR-072b.
  Writing it exposed that the mode reached only the structured report; the
  rendered report now names it too, so FR-083 is fully implemented rather than
  half.
- **`--clear-cache` cited FR-046** ("a daemon per instance"); corrected to
  **FR-067** (the timed window excludes a startup clear).

#### Unverified at stated scale ⏳

- **SC-012**: 10 000 concurrent sessions, 10 000 000 live keys, a run lasting
  hours, no unbounded resident-memory growth, and steady throughput between the
  first and last hour. The longest run to date is 60 minutes at a far smaller
  session count, with no first-hour-versus-last-hour comparison. **Recorded as
  outstanding in the spec rather than weakened to match what has been
  measured.**

#### Aligned but not traceable (19)

Implemented and hand-verified, but carrying no requirement id in the code, so a
mechanical pass cannot see them: FR-001, FR-007, FR-019, FR-031, FR-032,
FR-033, FR-055, FR-057, FR-066b, FR-068, FR-072b, FR-076, SC-002, SC-005,
SC-006, SC-007, SC-008, SC-009, SC-010.

FR-055 and FR-057 are **negative** requirements — "MUST NOT define a private
interchange format", "no output MUST imply dense identifiers" — satisfied by
the absence of such code, so they are not citable by construction.

### Unspecced Code 🆕

None. `--probe` was the only entry and now has FR-083.

## Inter-Spec Conflicts

None outstanding. The one conflict found (FR-045 and SC-001 versus FR-079) is
resolved in favour of FR-079, which the code follows.

## Known deferrals — deliberate, not drift

Listed so a future run of this analysis does not re-raise them:

| Item | Where it is recorded |
|------|----------------------|
| WekaTrace reader and its test | `tasks.md` T062i / T062j, marked DEFERRED |
| Trace replay and placement derivation | `research.md` D10 |
| Dense block identifiers | `research.md` D11 |
| FR-072's "identical" vs "isomorphic" wording | `research.md` D11 — "recommended, and deliberately not done here", conditional on the dense-key decision |
| A columnar container (parquet) | `research.md` D4, recorded as a rejected alternative |
| `stores declined` has no per-instance attribution | no requirement asks for it; noted here because it made a past investigation harder |

## Recommendations

1. **Nothing blocking.** The requirement set and the code agree.
2. **SC-012 needs a dedicated long run** at 10 000 sessions before it can be
   claimed. Do not weaken it to match a shorter run.
3. **A read timeout still renders as "instance became unreachable … os error
   11"**, because `ClientError` has no `TimedOut` variant and there is no
   `--read-timeout` flag. No requirement is violated — FR-064 only demands that
   a lost node be named — but the message misattributes the cause and has cost
   real investigation time. Worth a follow-up; it needs a new error variant and
   plumbing, not a wording change.
