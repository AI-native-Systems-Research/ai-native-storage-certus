# Spec-Sync Apply Report: spdk-sys

**Applied**: 2026-09-02T21:47:19Z (AUTO-BACKFILL mode)
**Drift report**: `lib/spdk-sys/.specify/sync/drift-report.{json,md}`
**Spec**: `lib/spdk-sys/specs/001-spdk-sys/spec.md`
**Scope**: Markdown only, under `lib/spdk-sys/specs/**` and
`lib/spdk-sys/.specify/sync/**`. No source/build files were modified.

## Backups

Pre-edit copies saved to `lib/spdk-sys/.specify/sync/backups/20260902T214719Z/`:
- `spec.md.bak`

## Resolutions Applied

| # | Finding | Resolution | Change |
|---|---|---|---|
| 1 | `spdk_nvme_ctrlr_cmd_abort_ext` + `spdk_nvme_ctrlr_get_max_xfer_size` allowlisted (`build.rs:197,220`) but not named by any FR | BACKFILL | `spec.md` FR-4 description extended: "… admin commands, in-flight command abort via `spdk_nvme_ctrlr_cmd_abort_ext`, MDTS-derived max transfer size via `spdk_nvme_ctrlr_get_max_xfer_size`". Removes the sole unspecced-code finding. |
| 2 | Stale `components/spdk-sys` path references after relocation to `lib/spdk-sys` | BACKFILL | `align-tasks.md:11,14,29` corrected from `components/spdk-sys/...` to `lib/spdk-sys/...`. |
| 3 | Sanity suite covers only env/PCI types; no P1 NVMe type/fn-pointer coverage (`tests/bindings_sanity.rs:9-97`) vs Success Criteria + FR-3/4/5 | ALIGN-TASK (medium) | Success Criteria text left unchanged (not weakened, per hard rule). Pre-existing align-task in `align-tasks.md` ("Extend sanity-test coverage to P1 NVMe FFI surface") retained and its paths refreshed. No source edited (out of scope). Mirrors open `tasks.md:8`. |

## Not Applied / Out of Scope

- `README.md` — not under `specs/**` or `.specify/sync/**`; not touched.
- `build.rs`, `Cargo.toml`, `wrapper.h`, `tests/bindings_sanity.rs` —
  source/build files, never touched.
- Historical `apply-report.md` narrative from the prior run is superseded by
  this file rather than edited in place.

## Superseded Specs

None.

## New Specs

None — the one unspecced finding was a narrow addition to the existing
`001-spdk-sys` spec (FR-4 wording), handled as BACKFILL.

## Deferred Items

None. All drift items and findings were backfilled into `spec.md` /
`align-tasks.md` or routed to the existing medium-severity align-task; nothing
required deferral.

## Status

`spec.md` `Status` field left as `Backfilled` — per `tasks.md:10`, the flip to
`Reviewed` is gated on human review plus resolution of the still-open tasks
(including the NVMe sanity-test coverage gap tracked in `align-tasks.md`), not
on this sync pass alone.
