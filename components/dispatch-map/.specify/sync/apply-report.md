# Sync Apply Report

**Date**: 2026-08-07T14:55:33Z
Based on: proposals from 2026-08-06T23:27:50Z
Backups: `.specify/sync/backups/20260807T145533Z/` (spec.md, idispatch_map.md)

## Changes Made

### Specs Updated (BACKFILL — applied directly)

| Spec | Requirement | Change Type | Detail |
|------|-------------|-------------|--------|
| 001-dispatch-map | FR-014 | Modified | Narrowed to info+debug logging; errors surface as typed `DispatchMapError`, not logged. |
| 001-dispatch-map | SC-004 | Modified | Reworded to describe compile-time largest-variant enum sizing; fixed constant via `entry_size()`. |
| 001-dispatch-map | US2 / AS4 | Modified | Marked deferred for v0 (`lookup` has no expected-size param; `MismatchSize` reserved). |
| 001-dispatch-map | User Story 12 | Added | Optional per-entry CRC-32 integrity checksums (`integrity-check` feature). |
| 001-dispatch-map | FR-027 | Added | `set_checksum`/`get_checksum` semantics under the `integrity-check` feature. |
| 001-dispatch-map | FR-028 | Added | `integrity-check` off by default; no trait/struct surface change when off. |
| 001-dispatch-map | Key Entities (Dispatch Entry) | Modified | Noted the feature-gated 4-byte `checksum` field. |
| 001-dispatch-map | contracts/idispatch_map.md | Added | `set_checksum`/`get_checksum` rows (feature-gated). |
| 001-dispatch-map | Header metadata | Modified | Updated **Last Synced** to 2026-08-07 with this session's changes. |

### New Specs Created
- None.

### Implementation Tasks Generated (ALIGN — code changes, not applied here)

4 tasks in `.specify/sync/align-tasks.md`:
- **FR-012**: `initialize()` must return `Err(NotInitialized)` instead of panicking when `IEvictionPolicy` is unbound.
- **US1/AS3**: add null-pointer guard + `DispatchMapError::NullPointer` to `create_memory_tier_entry`.
- **reuse_count**: remove the dead hot-path metric field and its `fetch_add` sites.
- **Creusot claims**: correct/soften the P1–P10 "formally proved" comment referencing the non-existent `verif/` directory.

### Not Applied
| Proposal | Reason |
|----------|--------|
| — | All 8 proposals were approved; backfills applied to specs, aligns queued as tasks. |

## Next Steps

1. Review the updated spec: `components/dispatch-map/specs/001-dispatch-map/spec.md` and `contracts/idispatch_map.md`.
2. Implement the 4 code-side tasks in `.specify/sync/align-tasks.md` (e.g. via `/speckit-implement`), then re-run `/speckit-sync-analyze` to confirm the drift closes.
3. Commit (on a feature branch — never commit directly to `unstable`):
   `git add components/dispatch-map/specs components/dispatch-map/.specify/sync && git commit -m "sync(dispatch-map): backfill integrity-check + reword FR-014/SC-004; queue aligns"`
4. Note: `components/dispatch-map/README.md` and `CLAUDE.md` still omit `IEvictionPolicy` and the newer methods — refresh via the `component-update-docs` skill (out of scope for this sync).
