# Spec-Sync Phase B — Apply Report: eviction-policy-session-lists

**Applied**: 2026-08-20
**Policy**: `.specify/sync/PHASE_B_POLICY.md`
**Source**: `.specify/sync/drift-report.{json,md}` (2026-08-20; 23 aligned, 1 drifted, 0 not_implemented, 0 unspecced)
**Proposals**: `.specify/sync/proposals.{md,json}`

Component is effectively CLEAN. The single drift (SC-001) is a
spec-acknowledged, downgraded design goal — not an implementation divergence.
Classified as **BACKFILL** (spec-lag / aspirational goal), not ALIGN: the
eviction code is correct and intentional; only the benchmark scope (hot-path
throughput, no comparative hit-rate harness) fails to verify the ≥15% target.

## Specs Updated

| Spec | Requirement | Change type |
|------|-------------|-------------|
| 001-session-list-eviction | SC-001 | BACKFILL — re-verified against `benches/session_list_benchmark.rs`; sharpened to name the four Criterion benches (`track`, `touch`, `batch_touch`, `identify_next_to_evict`), state SC-001 is explicitly not a measured outcome, and point to align-tasks.md Task 1 as the tracked follow-up. |
| 001-session-list-eviction | (metadata) | BACKFILL — added `**Last-Synced**: 2026-08-20` line under `Status`. |

## Align Tasks Generated

| Task | Status |
|------|--------|
| (none new this pass) | — |

No new ALIGN task. The pre-existing `align-tasks.md` Task 1 (build the
comparative session-lists-vs-`eviction-policy-lru` prefix-reload trace-replay
harness to eventually verify/retire the ≥15% target) remains valid and is
carried forward unchanged. It is a verification follow-up, not a behavioral-bug
alignment.

## Unspecced Backfilled

| Feature | Status |
|---------|--------|
| (none) | — |

The startup-announcement log was already backfilled into the spec's
`Observability` section on 2026-08-07; still present, no action.

## Resolved

| Item | Status |
|------|--------|
| (none) | — |

## Backups

- `specs/001-session-list-eviction/spec.md` → `.specify/sync/backups/specs/001-session-list-eviction/spec.md.bak`

## Verification

- All edits confined to `specs/001-session-list-eviction/spec.md` and files
  under `.specify/sync/`. No `.rs` source modified. No cargo run.

## History

- **2026-08-07** (`apply-report` superseded by this file; see
  `proposals-20260807.json`): first apply pass. SC-001 fork resolved by
  maintainer → "Downgrade to design goal (backfill)"; startup-announcement log
  backfilled as a non-normative `Observability` section; harness queued as
  align-tasks.md Task 1.
- **2026-08-20** (this pass): re-sync re-verified SC-001 against current code;
  reaffirmed and sharpened the BACKFILL; added Last-Synced metadata.
