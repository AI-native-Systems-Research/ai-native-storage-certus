# Drift Resolution Proposals

Generated: 2026-05-21
Based on: drift-report from 2026-05-21

## Summary

| Resolution Type | Count | Applied |
|-----------------|-------|---------|
| Backfill (Code -> Spec) | 1 | 1 applied (P7 — naming) |
| Pending Human Decision | 2 | — |

---

## Applied Proposals

### Proposal 7: 002/tasks — Rename BlockDeviceSpdkNvmeComponentV1 to BlockDeviceSpdkNvmeComponent

**Direction**: BACKFILL (naming alignment)
**Status**: APPLIED (2026-05-21)

The source code uses `BlockDeviceSpdkNvmeComponent` (no version suffix) as the component struct name. The spec 002 tasks.md at T016 still referenced the old `BlockDeviceSpdkNvmeComponentV1` name. Updated to match the current source.

**File changed**: `specs/002-iops-benchmark/tasks.md` line 59

---

## Pending Proposals (require human decision)

### Proposal 8: 001/SC-008 — Missing Criterion benchmarks

**Direction**: IMPLEMENT
**Status**: PENDING

SC-008 requires "Performance-sensitive paths (IO submission, batch processing, qpair selection) MUST have benchmarks." No `benches/` directory exists in the block-device-spdk-nvme crate. The IOPS benchmark app measures aggregate throughput but is not a per-function microbenchmark.

**Options**:
- A) Create `benches/` with Criterion benchmarks for qpair selection, context pool acquire/release, and batch dispatch overhead (can run without hardware using detached queue pairs).
- B) Relax SC-008 wording to accept the IOPS benchmark app as sufficient coverage.
- C) Defer until hardware CI is available for meaningful latency benchmarks.

---

### Proposal 9: 002/ — Unspecced features in IOPS benchmark

**Direction**: BACKFILL (three unspecced features)
**Status**: PENDING

Three features in the IOPS benchmark application have no corresponding spec requirements:

1. **Mixed block sizes** (`config.rs:74-75`): `--block-size` accepts comma-separated values for random mixed-size workloads. Spec FR-002 says "IO block size in bytes" (singular).
2. **Batch submit flag** (`config.rs:80-82`): `--batch-size` groups commands into `BatchSubmit` messages. Not mentioned in spec 002.
3. **NUMA worker pinning** (`main.rs:176-206`): Worker threads pinned round-robin to NUMA-local cores, skipping the actor core. Not specified.

**Options**:
- A) Add FR-023 (mixed block sizes), FR-024 (batch-size), FR-025 (NUMA worker pinning) to spec 002.
- B) Add FR-023 and FR-024 only; treat NUMA pinning as an implementation optimization (no spec entry needed).
- C) Defer until the features stabilize further.

---

## Previously Applied (history)

| # | Proposal | Applied |
|---|----------|---------|
| P1 | 001/FR-003 sync timeout wording | Deferred (user chose not to apply) |
| P2 | 001/SC-008 benchmark wording update | 2026-04-23 |
| P3 | 001/NEW FR-021 IBlockDeviceAdmin | 2026-04-23 |
| P4 | 002/NEW FR-022 --io-mode | 2026-04-23 |
| P5 | Cleanup stale WriteAsync comment | 2026-04-23 |
| P6 | SC-001/SC-002/SC-006 placeholder tests | 2026-04-23 |
| P7 | 002/tasks naming V1->no suffix | 2026-05-21 |
