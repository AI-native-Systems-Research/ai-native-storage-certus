# Drift Resolution Proposals — dispatcher

Generated: 2026-10-11 · Branch: `perf/coldload-striping-fixes` · Mode: coordinated (proposals approved by the coordinating agent) · Baseline: certified `83bcd4ff` · Delta: `87468671`, `ba462acb`, `bf52dd83`

All proposals are code-authoritative BACKFILL / BACKFILL-UNSPECCED. No ALIGN (code-change) proposals.

| # | Requirement | Direction | Severity | Decision |
|---|---|---|---|---|
| 1 | FR-025 (format_on_init / recovery) | BACKFILL | major | Approved |
| 2 | FR-061 per-drive cap bullet | BACKFILL | major | Approved |
| 3 | FR-061 head / drain / results bullets | BACKFILL | moderate | Approved |
| 4 | FR-019 | BACKFILL | moderate | Approved |
| 5 | FR-039 step (5) | BACKFILL | moderate | Approved |
| 6 | Q&A "How does the pipelined reader work?" | BACKFILL | moderate | Approved |
| 7 | Q&A "How does the SSD evictor determine drive ownership" | BACKFILL | minor | Approved |
| 8 | User Story 9 narrative + scenario 1 | BACKFILL | moderate | Approved |
| 9 | User Story 11 narrative | BACKFILL | moderate | Approved |
| 10 | User Story 11 scenario 3 | BACKFILL | minor | Approved |
| 11 | User Story 11 scenario 4 | BACKFILL | minor | Approved |
| 12 | User Story 11 scenario 1 | BACKFILL | minor | Approved |
| 13 | Key Entity "Pipelined Reader" | BACKFILL | moderate | Approved |
| 14 | SC-007 | BACKFILL | moderate | Approved |
| 15 | User Story 10 narrative | BACKFILL | minor | Approved |
| 16 | FR-015 | BACKFILL | minor | Approved |
| 17 | FR-034 | BACKFILL | minor | Approved |
| 18 | FR-040 / FR-041 | BACKFILL | minor | Approved |
| 19 | Key Entities "Extent", "Background Writer" | BACKFILL | minor | Approved |
| 20 | Edge case (SSD evictor frees extent) | BACKFILL | minor | Approved |
| 21 | contracts/errors.md, contracts/idispatcher.md, data-model.md | BACKFILL | minor | Approved (interfaces crate doc comment excluded — follow-up) |
| 22 | Last Synced header | — | — | Approved |
| U1 | New FR-062 (striped placement) | BACKFILL-UNSPECCED | moderate | Approved |
| U2 | New FR-063 (multi-drive geometry validation) | BACKFILL-UNSPECCED | moderate | Approved |
| U3 | User Story 5 scenario 5 | BACKFILL-UNSPECCED | minor | Approved |
| U4 | Two edge cases (forced format; partial GPU bytes) | BACKFILL-UNSPECCED | minor | Approved |
| U5 | New SC-018 | BACKFILL-UNSPECCED | minor | Approved |

## 1. FR-025

- Spec said: when `format_on_init=false`, iterate each extent manager's extents and `recover_extent(key, offset, size)`.
- Code does: `initialize()` forces `format_on_init = true` when `data_pci_addrs.len() > 1` (`force_format_when_striped`, `src/lib.rs:1721-1732`) and warns when overriding (`src/lib.rs:2264-2272`); recovery iterates only drive 0's extents with size `N · extent.size` (`src/lib.rs:2301-2322`).
- Rationale: `bf52dd83` — the striped layout records no stripe count / drive order / layout version, so it is not recoverable; the SSD tier is a cache.

## 2. FR-061 cap bullet

- Spec said (optimizer iter-006 edit): `batch_lookup`'s scatter caps every drive at 24, "so N=3 also gets 24".
- Code does: the cap applies only when ≥ `PER_DRIVE_QD_CAP_MIN_DRIVES` (4) drives take part (`effective_qd_cap`, `src/pipeline.rs:845-849`, counted over `drive_works.len()`); fewer drives keep the proportional share.
- Rationale: `bf52dd83` — cap cost 14% cold-load throughput on 1 drive.

## 3. FR-061 other sentences

- Spec said: "that `batch_lookup` uses"; "The function MUST: The function MUST:"; drive-wide failure list without initial send failure; "first error per job"; results "indexed by drive, then by job" (contradicting its own "results are indexed by object").
- Code does: `batch_lookup` calls crate-private `scatter_gather_multi_drive_with_policy` (`src/lib.rs:2982-2991`); public fn serves `read_striped_object`; one `Result` per object (`src/pipeline.rs:1263-1275`); initial send failure fails the drive's objects (`src/pipeline.rs:1437-1440`).

## 4. FR-019

- Spec said: cap 24 unconditional; `promote_and_serve` uses `pipelined_multi_object_zero_copy` at 128; staging fallback `pipelined_ssd_to_gpu_zero_copy` at 16.
- Code does: those paths only with one drive; with N > 1 both use `read_striped_object` → public scatter-gather (128 / 16 total, uncapped, one coalesced H2D) (`src/lib.rs:880-937`, `1111-1150`, `969-1035`).

## 5. FR-039 step (5)

- Spec said: single `scatter_gather_multi_drive_zero_copy`, uncapped split, coalesced H2D the moment the last segment lands.
- Code does: `scatter_gather_multi_drive_with_policy` + `BATCH_SCATTER_POLICY` (backlog-aware 512 KiB groups, cap 24 at ≥ 4 drives); a checkout failure fails every cold object (`src/lib.rs:2963-2972`).

## 6–20. Remaining BACKFILL items

See `apply-report.md` for the per-item resolution; each mirrors the code evidence listed in the 2026-10-11 block of `drift-report.md`.

## U1–U5. Unspecced behaviour

- FR-062: stripe layout (`stripe_unit`/`stripe_range`, `src/lib.rs:355-385`), stripe allocator, per-drive write-through slices (`src/background.rs:22-102, 285-325`; `src/lib.rs:1626-1720`), enqueue-failure read-ref release (`src/lib.rs:3877-3895, 4205-4222`), remove/evictor on drive 0.
- FR-063: block size / data partition validation returning `InvalidParameter` (`src/lib.rs:2165-2189`).
- User Story 5 scenario 5, two edge cases, SC-018 (tests `stripe_round_trip_*`, `stripe_scatter_*`, `h2d_planner_partitions_objects`, `scatter_qd_cap_needs_four_drives`, `striped_config_always_formats`).
