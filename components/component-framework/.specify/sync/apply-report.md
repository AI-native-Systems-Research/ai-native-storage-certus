# Sync Apply Report

Applied: 2026-07-22T21:28:00Z
Mode: AUTO-BACKFILL
Based on: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22)

## Summary

| Category | Count |
|----------|-------|
| Specs analyzed | 6 |
| Unspecced features backfilled | 4 |
| New specs created | 0 |
| Specs superseded | 0 |
| Spec text changes applied (drift) | 0 (already resolved in a prior cycle — see below) |
| Deferred to align-tasks.md | 1 |

## Changes Made

### Specs Updated

| Spec | Change | Detail |
|------|--------|--------|
| 003-actor-channels | Backfill (code -> spec) | Added FR-028 (`on_idle()` hook), FR-029 (`signal_stop()`), FR-030 (non-blocking `try_send()` on `ActorHandle`/channel sender), FR-031 (internal `register_for_unpark()` note). Added 3 new User Story 1 acceptance scenarios and 3 new Edge Cases entries covering the same four behaviors. Softened Assumptions bullet on default blocking `send()` to cross-reference the new `try_send()` FR. |
| 003-actor-channels/contracts/public-api.md | Companion doc backfill | Added `on_idle()` to the `ActorHandler` trait contract and `signal_stop()` to the `ActorHandle` impl block (both were missing; `try_send()` was already present). |
| 003-actor-channels/data-model.md | Companion doc backfill | Added a `signal_stop()` state-transition row and a "Message-loop idle behavior" note describing the `on_idle()` / idle-count / park-with-timeout / `register_for_unpark()` sequence. |

### Specs Reviewed — No Change Needed (already resolved by a prior sync cycle)

| Spec | Drift Finding | Why no change was applied |
|------|----------------|----------------------------|
| 004-channel-benchmarks | FR-015 drifted (doc-example ergonomics on third-party channels) | Current `spec.md` FR-015 text already carries the softened wording acknowledging the native-construction-API tradeoff (matches drift-report Recommendation #3, option 1). No further edit required. |
| 005-numa-aware-actors | FR-001 drifted (User Story/Edge Cases narrative implied multi-cycle affinity changes) | Current `spec.md` FR-001 and the Edge Cases entry already state the single-use/reconstruction-required design explicitly. The narrative inconsistency the drift report flagged is not present in the file as it stands. |

### New Specs Created

(none — all four unspecced items are reasonable extensions of existing actor/channel FRs in 003-actor-channels, not separate features, per task instructions)

### Superseded

(none)

### Deferred to align-tasks.md

| Spec/Req | Severity | Reason for deferral |
|----------|----------|----------------------|
| 003-actor-channels/FR-004, 005-numa-aware-actors/FR-001 | Minor | `Actor::activate()` panics via `.expect(...)` when called a second time after a full activate/deactivate cycle, instead of returning a typed `Result::Err` as both FRs' "error not panic" pattern implies. This is a code-side behavior question (possible defect), not a spec-text question — the spec already correctly describes single-use/error-on-misuse intent. Per the hard rule against rewriting specs to match panic/guarantee-violation behavior, this was NOT backfilled into the spec; it is logged in `align-tasks.md` for a human decision on the code fix. |

## Backups

Pre-modification backups saved to `.specify/sync/backups/2026-07-22/`:
- `003-spec.md`
- `003-public-api.md`
- `003-data-model.md`

## Next Steps

1. Review the updated `003-actor-channels` spec.md, contracts/public-api.md, and data-model.md for accuracy.
2. Review `align-tasks.md` and decide whether to convert the `activate()` double-consume panic into a typed `ActorError` variant.
3. Commit changes: `git add components/component-framework/specs/003-actor-channels components/component-framework/.specify/sync && git commit -m "spec-sync: backfill actor on_idle/signal_stop/try_send/register_for_unpark into 003-actor-channels"`
