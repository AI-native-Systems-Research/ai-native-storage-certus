# Spec-Sync Apply Report — eviction-policy-session-lists

**Mode**: sweep (analyze → propose --interactive → apply)
**Applied**: 2026-08-07
**Branch**: `sync/spec-drift-sweep-20260807`
**Source**: `.specify/sync/drift-report.{json,md}` (generated 2026-08-07T08:32Z)

This is the first apply pass for this component — previously only a
drift-report existed. Pacing: auto-apply safe BACKFILL/doc items on-branch;
stop-and-ask on genuine forks. One fork arose (SC-001) and was resolved by
the maintainer.

## Drift summary

22 requirements aligned, 0 drifted. Two findings drove this pass:
- **SC-001 Not-Implemented / unverified** — claims a ≥15% prefix-reload
  reduction vs. basic LRU, but no comparative trace-replay harness exists in
  the repo (`benches/session_list_benchmark.rs` measures only this component's
  hot-path throughput, not a hit-rate comparison). → **fork**.
- **Unspecced startup-announcement log** — a one-time informational log line
  on first selection as active policy (`announced` flag; `src/lib.rs:83-87,107-120`)
  via the optional `ILogger`, not covered by any FR. → **safe BACKFILL**.

## Changes applied (spec Markdown only)

| Spec | Item | Change |
|------|------|--------|
| 001-session-list-eviction | Observability (new section) | Added a `## Observability *(non-normative, backfilled 2026-08-07)*` section at end of `spec.md` documenting the one-time startup-announcement log via the optional `ILogger` receptacle. Explicitly non-normative: a missing logger turns no operation into an error, consistent with the shared component logging convention. |
| 001-session-list-eviction | SC-001 | **Fork resolution** — maintainer chose **"Downgrade to design goal (backfill)"**. SC-001 relabelled *(design goal — not yet verified; downgraded 2026-08-07)*; the ≥15% figure is now stated as an aspirational target pending a comparative session-lists-vs-`eviction-policy-lru` trace-replay harness, not a measured outcome. |

## Fork resolved

| Fork | Options presented | Maintainer decision |
|------|-------------------|---------------------|
| SC-001 ≥15% prefix-reload reduction is unverified (no comparative harness) | (a) downgrade SC-001 to a design goal / target; (b) build the comparative trace-replay harness and measure | **(a) Downgrade to design goal (backfill)** — recorded here and in `proposals-20260807.json`. Building the harness is queued as an open align-task (non-blocking). |

## Queued to align-tasks.md

- **Build comparative prefix-reload trace-replay harness** (to eventually
  verify or retire the SC-001 ≥15% target). Non-HIGH; queued, not drafted.

## Not changed / no action

- FR-001…FR-018, SC-002…SC-006 all aligned; no edits.
- No `NEW_SPEC`, no `SUPERSEDE`.
- No source code touched.

## Verification
- All edits confined to `specs/001-session-list-eviction/spec.md`. No `.rs` source modified.
- Artifacts created this pass: this `apply-report.md`, `proposals-20260807.json`, `align-tasks.md`.
