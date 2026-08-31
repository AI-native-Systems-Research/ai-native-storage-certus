# Spec Sync Apply Report — dispatcher

Generated: 2026-08-31
Mode: interactive (all three proposals approved by the user)
Spec: components/dispatcher/specs/001-dispatcher-cache-interface/spec.md
Backup (pre-edit): components/dispatcher/.specify/sync/backups/specs/001-dispatcher-cache-interface/spec.md.bak
Drift source: components/dispatcher/.specify/sync/drift-report.{json,md}

## Classification (each drift item, verified against code)

- **FR-040** — `src/lib.rs:5,44` show control-plane requests now arrive from the shmq serve
  layer; `BatchTouchRequest` no longer exists in the dispatcher source; gRPC removed in
  commit `97e26738`. Spec-lag → **BACKFILL** (applied to spec.md).
- **FR-042** — `src/lib.rs:442` shows the eviction receiver is drained by the shmq serve
  layer via `TakeEvents`. Spec-lag → **BACKFILL** (applied to spec.md).
- **Unspecced tier-event counters** — `tier_event_stats()` (`idispatcher.rs:564`) + `TierEventStats`
  (`:191`) + `TierEventCounters` (`lib.rs:111-159`, impl `:3390`, ~11 record sites) ship and are
  committed with no requirement → **BACKFILL-UNSPECCED** (applied to spec.md as FR-058 + SC-017,
  FR-001 inventory amended).

## Specs Updated

| Requirement / Section | Change Type | Summary |
|---|---|---|
| Header — Last Synced | Amend | Added the 2026-08-31 sync note (FR-040/FR-042 gRPC→shmq backfill; FR-058/SC-017 add; CLAUDE.md drifts now resolved; out-of-scope notes). |
| FR-001 — introspection inventory | Amend (backfill) | Added `tier_event_stats` to the durability/introspection method list (cross-ref FR-058). |
| FR-040 | Amend (backfill) | `The gRPC handler spawns this ... when BatchTouchRequest.promote = true` → `The shmq serve layer spawns this ... on the control-plane promote request` (+ sync note citing commit 97e26738). |
| FR-042 | Amend (backfill) | `external consumers (e.g., gRPC TakeEvents stream)` → `external consumers (the shmq serve layer's TakeEvents drain)` (+ sync note). |
| FR-058 | Add (backfill-unspecced) | NEW: documents `tier_event_stats() -> TierEventStats` and the `TierEventCounters` subsystem (4 monotonic Arc-shared counters; `snapshot()` without reset; always populated, unlike telemetry-gated `read_write_stats`). |
| SC-017 | Add (backfill-unspecced) | NEW: measurable outcome for FR-058 — successive `tier_event_stats()` snapshots yield non-decreasing counters whose delta reflects the window's promotions/serves/evictions, with no `read_write_stats` telemetry required. |

## Align Tasks Generated

None. No drift item is a behavioral bug against a correct spec requirement. See `align-tasks.md`.

## Unspecced Backfilled

| Feature | Location | Requirement Added |
|---|---|---|
| Tier-event counters: `tier_event_stats()` + `TierEventCounters` | `idispatcher.rs:564,191`; `lib.rs:111-159,3390` | FR-058 + SC-017 (FR-001 inventory amended) |

## Resolved Since Last Sync (no action needed)

| Item | Status |
|---|---|
| CLAUDE.md stale `component-framework` path (2026-08-20 deferred) | Corrected in `components/dispatcher/CLAUDE.md` (`../../lib/component-framework/crates/`). No longer drift. |
| CLAUDE.md stale `-v2` crate names (2026-08-20 deferred) | Corrected (`block-device-spdk-nvme`, `extent-manager`). No longer drift. |

## Not Applied / Deferred (out of scope)

| Item | Reason |
|---|---|
| Two "gRPC handler" mentions in `src/lib.rs` code comments (`:2983`, `:3016`) | Source comments are outside this sync's editable scope (`.specify/sync/` and `specs/` only). Suggested follow-up: reword to "shmq serve layer / null-stream caller". |
| `components/dispatcher/verif/` (untracked Creusot build state) | Not committed; the `IDispatcher` interface makes no verification claims, so there is no spec/interface claim to reconcile. |

## Backups

- `components/dispatcher/.specify/sync/backups/specs/001-dispatcher-cache-interface/spec.md.bak` —
  pre-edit copy of the only spec.md modified this run.

## Notes

- Only Markdown under `components/dispatcher/specs/**` and `.specify/sync/**` was modified. No
  `src/**` source was touched and `cargo` was not run.
- Active requirement count after apply: FR-001..FR-058 (5 REMOVED: FR-020/021/022/026/027) and
  SC-001..SC-017 (1 REMOVED: SC-008).
