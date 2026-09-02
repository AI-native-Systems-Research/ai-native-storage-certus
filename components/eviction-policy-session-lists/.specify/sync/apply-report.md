# Spec-Sync — Apply Report: eviction-policy-session-lists

**Applied**: 2026-09-02
**Source**: `.specify/sync/drift-report.{json,md}` (2026-09-02; 23 aligned, 1 drifted, 0 not_implemented, 0 unspecced)
**Proposals**: `.specify/sync/proposals.{md,json}`

The component's eviction implementation is **CLEAN** against every functional
and behavioural requirement (FR-001..FR-018, SC-002..SC-006). The one drift is
SC-001, and this pass **corrects** it rather than reaffirming the prior
(incorrect) wording: the comparative trace-replay harness that SC-001 said did
not exist **does** exist — `apps/eviction-replay-benchmark`, present since the
component's introducing commit `c247b890` (2026-08-04) — and was missed by both
prior syncs. Classified as **BACKFILL** (stale/incorrect spec text vs. correct
code), not ALIGN.

## Specs Updated

| Spec | Requirement | Change type |
|------|-------------|-------------|
| 001-session-list-eviction | SC-001 | BACKFILL — corrected: names `apps/eviction-replay-benchmark` as the existing comparative session-lists-vs-LRU hit-rate harness; states it has existed since the introducing commit and was overlooked by the 2026-08-07/2026-08-20 syncs; reports that the measured effect is workload/cache-size dependent and does **not** reach the ≥15% target on sampled runs; recasts ≥15% as measurable-but-unmet rather than unverifiable. |
| 001-session-list-eviction | (metadata) | BACKFILL — `**Last-Synced**` line updated to `2026-09-02` with the correction note. |

## Align Tasks Generated

| Task | Status |
|------|--------|
| (none new) | — |

`align-tasks.md` Task 1 was **re-scoped in place** (not code): it previously
asked to *build* the comparative harness. That harness already exists, so the
"build/report" acceptance criteria are checked off and Task 1 now reads "Pin
SC-001 to a measured operating point" — the only remaining criterion is
rewriting SC-001 to a concrete measured figure using `apps/eviction-replay-benchmark`.

## Unspecced Backfilled

| Feature | Status |
|---------|--------|
| (none) | — |

The startup-announcement log remains documented in the spec's `Observability`
section (backfilled 2026-08-07); no action. `apps/eviction-replay-benchmark` is
a separate application, not part of this component's `src/`, so it is not
"unspecced component code" — it is referenced from SC-001 as the verification
vehicle.

## Resolved

| Item | Status |
|------|--------|
| align-tasks.md Task 1 "build the harness" portion | Recognised as already satisfied by `apps/eviction-replay-benchmark`; criteria checked off. |

## Backups

- `specs/001-session-list-eviction/spec.md` →
  `.specify/sync/backups/20260902T212933Z/specs/001-session-list-eviction/spec.md`

## Verification

- Edits confined to `components/eviction-policy-session-lists/`:
  `specs/001-session-list-eviction/spec.md` and files under `.specify/sync/`
  (`align-tasks.md`, reports, backup). No `.rs` source modified. No
  `components/interfaces/**` touched. No cargo run. No git add/commit.
- `spec_sync_inputs_sha256` recomputed **after** the spec edit via
  `scripts/spec-sync-hash.sh` and stamped into `drift-report.md`.

## History

- **2026-08-07**: first apply pass. SC-001 downgraded to a design goal;
  startup-announcement log backfilled; harness queued as align-tasks.md Task 1.
- **2026-08-20**: re-sync; reaffirmed + sharpened SC-001 BACKFILL, added
  Last-Synced metadata. **(Both passes incorrectly asserted no comparative
  harness exists.)**
- **2026-09-02** (this pass): discovered `apps/eviction-replay-benchmark`
  already provides the comparative session-lists-vs-LRU hit-rate harness;
  corrected SC-001's factual claim, recast ≥15% as measurable-but-unmet, and
  re-scoped align-tasks.md Task 1.
