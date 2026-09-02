# Spec-Sync Proposals: spdk-sys

**Generated**: 2026-09-02T21:47:19Z
**Drift report**: `lib/spdk-sys/.specify/sync/drift-report.{md,json}`

Resolution types: **BACKFILL** (edit spec `.md` to match verified code) ·
**ALIGN** (code should change; append task, do not edit code here) ·
**HUMAN_DECISION** (leave in report for a human).

| # | Finding | Location | Resolution | Approved | Notes |
|---|---------|----------|------------|----------|-------|
| 1 | `spdk_nvme_ctrlr_cmd_abort_ext` + `spdk_nvme_ctrlr_get_max_xfer_size` allowlisted but not named by any FR | `build.rs:197,220` | BACKFILL | true | Both are controller operations within FR-4's scope; extend FR-4 text to name them. Confident — code + comments document their use (in-flight abort, MDTS-derived max transfer). Applied. |
| 2 | Stale `components/spdk-sys` path references in prior sync artifact | `.specify/sync/align-tasks.md:11,14,29` | BACKFILL | true | Crate relocated to `lib/spdk-sys`; correct the three paths. Housekeeping, minor. Applied. |
| 3 | Sanity suite covers only env/PCI types; no P1 NVMe type/fn-pointer coverage vs Success Criteria + FR-3/4/5 | `tests/bindings_sanity.rs:9-97` | ALIGN | true | Code-side test gap. Do NOT weaken the Success Criteria bullet. Append/keep align-task; mirrors open `tasks.md:8` item. No source edited (out of scope). |

## Rationale

- **#1 BACKFILL**: The two functions are unambiguously controller-level SPDK
  APIs already conceptually covered by FR-4 ("NVMe controller operations …
  admin commands"). Naming them removes the only unspecced-code finding without
  overstating intent. Low risk.
- **#2 BACKFILL**: Pure path correction after the `components/` → `lib/`
  relocation. The regenerated `apply-report.md` also uses corrected paths.
- **#3 ALIGN**: The gap is missing tests, not incorrect spec text. Weakening
  the Success Criteria to match under-coverage is disallowed by the hard rules,
  so this is tracked as an implementation follow-up in `align-tasks.md`.
