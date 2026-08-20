# Spec Sync Apply Report — dispatcher (Phase B)

Generated: 2026-08-20
Mode: Phase B (classify each drift item by reading its `location` code; BACKFILL the unspecced feature)
Spec: components/dispatcher/specs/001-dispatcher-cache-interface/spec.md
Backup (pre-edit): components/dispatcher/.specify/sync/backups/specs/001-dispatcher-cache-interface/spec.md.bak
Drift source: components/dispatcher/.specify/sync/drift-report.{json,md}

## Classification (each drift item, verified against code)

- **US-011 / FR-039** — `src/lib.rs:2217` sets `let queue_depth = 128;`, matching FR-039 step (5) and
  FR-019. The User Story 11 narrative and acceptance scenario 3 carried the stale `16 / num_queues`
  (`max_queue_depth = 8`, ≤16 per drive) wording. Spec-lag → **BACKFILL** (applied to spec.md).
- **CLAUDE.md stale crate path** (`CLAUDE.md:40`) — `component-framework` moved to `lib/`; doc path
  stale, build unaffected. Doc-lag → **BACKFILL** direction, **recorded, NOT applied** (CLAUDE.md is
  outside this sync's editable scope).
- **CLAUDE.md stale `-v2` names** (`CLAUDE.md:43-44,53`) — actual crate is `block-device-spdk-nvme`
  (no `-v2`). Doc-lag → **BACKFILL** direction, **recorded, NOT applied** (out of scope).
- **Unspecced DI/test hooks** (`src/lib.rs:358-374`) — `set_block_device_factory`,
  `set_extent_manager_factory`, `set_pipeline_metrics` ship as public inherent methods with no
  requirement → **BACKFILL-UNSPECCED** (applied to spec.md as FR-057 + SC-016).

## Specs Updated

| Requirement / Section | Change Type | Summary |
|---|---|---|
| User Story 11 — narrative | Amend (backfill) | `reduced NVMe pipeline depth (16 / num_queues)` → `deep per-thread NVMe pipeline depth (max_queue_depth = 128, per FR-039 step (5) and FR-019)`. |
| User Story 11 — acceptance scenario 3 | Amend (backfill) | `max_queue_depth = 8 (16/2), ≤16 per drive` → `max_queue_depth = 128 (per FR-039 step (5) / FR-019)`. |
| FR-057 | Add (backfill-unspecced) | NEW: documents the `DispatcherComponent` dependency-injection / test setters (`set_block_device_factory`, `set_extent_manager_factory`, `set_pipeline_metrics`) as inherent methods (not `IDispatcher` trait methods) that override internally-constructed deps, with fallback to defaults when unset. |
| SC-016 | Add (backfill-unspecced) | NEW: measurable outcome for FR-057 — exercise the data path and observe pipeline timings with injected mocks and no NVMe hardware. |
| Header — Last Synced | Amend | Added the 2026-08-20 Phase B sync note (US-11 backfill, FR-057/SC-016 add, and the two deferred CLAUDE.md doc drifts). |

## Align Tasks Generated

None. No drift item is a behavioral bug against a correct spec requirement. See `align-tasks.md`.

## Unspecced Backfilled

| Feature | Location | Requirement Added |
|---|---|---|
| DI / test hooks: `set_block_device_factory`, `set_extent_manager_factory`, `set_pipeline_metrics` | `src/lib.rs:358-374` | FR-057 + SC-016 |

## Resolved (already fixed on main thread)

None applicable for this component.

## Not Applied / Deferred

| Item | Reason |
|---|---|
| CLAUDE.md stale `component-framework` path (`CLAUDE.md:40`) | Target `CLAUDE.md` is outside this sync's editable scope (`.specify/sync/` and `specs/` only). BACKFILL proposal recorded in `proposals.*`; leave for a follow-up doc pass. Suggested fix: `../../../lib/component-framework/crates/`. |
| CLAUDE.md stale `-v2` crate names (`CLAUDE.md:43-44,53`) | Same scope reason. Suggested fix: `block-device-spdk-nvme`, `extent-manager`. |

## Backups

- `components/dispatcher/.specify/sync/backups/specs/001-dispatcher-cache-interface/spec.md.bak` —
  pre-edit copy of the only spec.md modified this run.

## Notes

- Only Markdown under `components/dispatcher/specs/**` and `.specify/sync/**` was modified. No
  `src/**` source was touched and `cargo` was not run.
- Active requirement count after apply: FR-001..FR-057 (5 REMOVED: FR-020/021/022/026/027) and
  SC-001..SC-016 (1 REMOVED: SC-008).
