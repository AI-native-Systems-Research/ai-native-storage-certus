# Spec-Sync Phase B — Apply Report — block-device-spdk-nvme

Generated: 2026-08-20
Based on: `.specify/sync/drift-report.json` (generated 2026-08-20)
Policy: `.specify/sync/PHASE_B_POLICY.md`

## Counts

| Category | Count |
|---|---|
| BACKFILL applied (drifted reqs) | 3 |
| BACKFILL-UNSPECCED (new/extended reqs) | 8 |
| ALIGN tasks generated | 1 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

All 3 drifted requirements were spec-lag (working/intentional code, stale spec text) and
were backfilled. All 8 unspecced features were working behaviors and were backfilled. One
genuine cosmetic code defect discovered inside unspecced feature #3 was filed as an ALIGN
task (no `.rs` source modified).

## Specs Updated

| Spec | Requirement | Change Type | Summary |
|---|---|---|---|
| 001 | FR-005 | BACKFILL | Abort buffer-lifetime contract re-synced from "drafted on branch / needs hardware validation" to "implemented in mainline" (`src/actor.rs:972-1020,528-537`). |
| 001 | FR-010 | BACKFILL | `max_transfer_size` documented as MDTS-derived (`src/controller.rs:169-177`), moved out of the fixed-constants list; 131072 is only the MDTS==0 fallback. |
| 001 | SC-005 | BACKFILL | Only `nvme_version` and `numa_id` remain fixed constants; `max_transfer_size` is hardware-consistent. |
| 001 | FR-031 (new) | BACKFILL-UNSPECCED | Added `FlushSync`/`FlushDone` synchronous durability barrier requirement + acceptance scenario (User Story 1). |
| 001 | Assumptions | BACKFILL-UNSPECCED | Documented the dead `namespace::probe()` helper as superseded by `discover_namespaces` (removal candidate). |
| 002 | FR-015 | BACKFILL-UNSPECCED | Added GB/s throughput and per-thread IOPS breakdown to the reported output. |
| 002 | FR-024 | BACKFILL-UNSPECCED | Added batch send-failure in-flight rollback requirement. |
| 002 | FR-026 | BACKFILL-UNSPECCED | Documented parallel `thread::scope` device init + `[timing]` output + `=== Per-Device Summary ===` block. |
| 002 | SC-001 | BACKFILL-UNSPECCED | Documented barrier-based start sync excluding init time from the measured wall-clock window. |

Metadata `Last Synced` lines updated in both spec.md files.

## Align Tasks Generated

| ID | Spec/Req | Severity | Summary | Files |
|---|---|---|---|---|
| BD-4 | 002/FR-026 | Low | Per-device summary `println!` format string has an unbalanced `(` — PCI address not closed. Cosmetic only. | `apps/iops-benchmark/src/main.rs:423` |

(Appended to `align-tasks.md` under the 2026-08-20 sweep section, with acceptance criteria.)

## Unspecced Backfilled

| # | Feature | Location | Target |
|---|---|---|---|
| 1 | `Command::FlushSync`/`FlushDone` + `do_sync_flush` | `src/actor.rs:941-951,1214-1260` | 001/FR-031 (new) |
| 2 | Dead `probe()` free function superseded by `discover_namespaces` | `src/namespace.rs:20-47` | 001/Assumptions note |
| 3 | Multi-device per-device summary block | `apps/iops-benchmark/src/main.rs:397-428` | 002/FR-026 (+ ALIGN BD-4) |
| 4 | Barrier-based start sync excluding init time | `main.rs:262,328-329`; `worker.rs:106` | 002/SC-001 |
| 5 | `throughput_gbps` computed and reported | `stats.rs:38,83`; `report.rs:122-124` | 002/FR-015 |
| 6 | Per-thread IOPS breakdown in final report | `report.rs:74-103` | 002/FR-015 |
| 7 | Batch send-failure rollback of in-flight entries | `worker.rs:158-171` | 002/FR-024 |
| 8 | Parallel device init via `thread::scope` + `[timing]` eprintlns | `main.rs:52-55,105-153` | 002/FR-026 |

## Resolved

None resolved in this pass. (Prior Task BD-1, the FR-005 abort UAF fix, is now confirmed
present in mainline and marked RESOLVED in `align-tasks.md`; it corresponds to the FR-005
BACKFILL above rather than a code change made here.)

## Backups

Pre-edit backups written before any spec.md was modified:

- `.specify/sync/backups/specs/001-spdk-nvme-block-device/spec.md.bak`
- `.specify/sync/backups/specs/002-iops-benchmark/spec.md.bak`

## Scope compliance

- Edited only files under `components/block-device-spdk-nvme/.specify/sync/` and
  `components/block-device-spdk-nvme/specs/`.
- No `.rs` source modified; `cargo` not run.
- The nested duplicate report at
  `components/block-device-spdk-nvme/components/block-device-spdk-nvme/.specify/sync/`
  was ignored per instructions.
