# Spec-Sync Apply Report: spdk-sys

**Applied**: 2026-07-22 (AUTO-BACKFILL mode)
**Drift report**: `components/spdk-sys/.specify/sync/drift-report.{json,md}`
**Spec**: `components/spdk-sys/specs/001-spdk-sys/spec.md`
**Scope**: Markdown only, under `components/spdk-sys/specs/**` and
`components/spdk-sys/.specify/sync/**`. No source/build files were modified.

## Backups

Pre-edit copies saved to `components/spdk-sys/.specify/sync/backups/`:
- `spec.md.20260722T231948Z.bak`
- `tasks.md.20260722T231948Z.bak` (unedited; backed up defensively)

## Resolutions Applied

| # | Drift item | Resolution | Change |
|---|---|---|---|
| 1 | "28 DPDK libraries" claim (Implementation Notes) vs. code's 30 | BACKFILL | `spec.md`: "28 DPDK `rte_*` libraries" → "30 DPDK `rte_*` libraries" |
| 2 | `libm` linked (`build.rs:134`) but missing from Dependencies table | BACKFILL | `spec.md`: Dependencies table system-libs row appended with `m` (`pthread, dl, numa, uuid, ssl, crypto, fuse3, m`) |
| 3 | 5 allowlisted FFI types missing from Key Entities (`spdk_pci_driver`, `spdk_nvme_io_qpair_opts`, `spdk_nvme_cmd`, `spdk_nvme_format`, `spdk_nvme_ctrlr_list`) | BACKFILL | `spec.md`: 5 rows added to Key Entities table |
| 4 | `SPDK_PCI_*` / `SPDK_NVME_TRANSPORT_*` constant bindings (`allowlist_var`, `build.rs:239-240`) have no covering FR | BACKFILL | `spec.md`: added `FR-9 | Generate bindings for SPDK PCI and NVMe transport constants needed by callers (SPDK_PCI_*, SPDK_NVME_TRANSPORT_*) | P2` |
| 5 | `tests/bindings_sanity.rs` covers only env/PCI types; no P1 NVMe types/functions (FR-3/4/5), contradicting the Success Criteria sanity-test claim | ALIGN-TASK (moderate) | Success Criteria text left unchanged (not weakened, per hard rule). Appended `## Task: Extend sanity-test coverage to P1 NVMe FFI surface` to `.specify/sync/align-tasks.md`, cross-linked to the pre-existing open item in `specs/001-spdk-sys/tasks.md` ("Validate that sanity tests cover all critical types used by downstream consumers"). No source edits made (out of scope). |

## Not Applied / Out of Scope

- `README.md:33` ("28 `rte_*` libraries") repeats the same drift as item #1 but
  is not under `specs/**` or `.specify/sync/**`, so it was left untouched per
  the hard rules restricting edits to Markdown in those two locations.
- `build.rs`, `Cargo.toml`, `tests/bindings_sanity.rs` — source/build files,
  never touched.

## Superseded Specs

None.

## New Specs

None — no unspecced feature warranted a standalone new spec; both unspecced
findings were narrow additions to the existing `001-spdk-sys` spec (Key
Entities rows + one FR), handled as BACKFILL.

## Deferred Items

None. All drift items and unspecced findings from the drift report were
either backfilled directly into `spec.md` or routed to `align-tasks.md` as a
moderate-severity align-task; nothing required deferral for lack of
confidence or missing information.

## Status

`spec.md` `Status` field left as `Backfilled` — per `tasks.md:10`, the flip to
`Reviewed` is gated on human review of the backfilled spec plus resolution of
the still-open tasks (including the NVMe sanity-test gap tracked in
`align-tasks.md`), not on this sync-apply pass alone.
