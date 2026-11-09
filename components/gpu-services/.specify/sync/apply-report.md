# Sync Apply Report — gpu-services

Applied: 2026-11-09T07:09:01Z (apply run for the 2026-11-08 proposals) · Branch `fix/dispatcher-j33-repairs` (HEAD `d45ab524d`) · Mode: batch · Approved by Cornel 2026-11-08
Based on: `proposals.json` / `proposals.md` (generated 2026-11-08T12:00:00Z, commit `d45ab524d`, review of PR #591)
Backup: `backups/20261109T070901Z/` (`specs/001-gpu-cuda-services/spec.md`, `apply-report.{md,json}.prev`). Written with the Write tool before the edits and checked byte-identical with `cmp`.

## Changes Made

### Specs Updated
| Proposal | Spec | Requirement | Change Type |
|---|---|---|---|
| P-6 | 001-gpu-cuda-services | FR-016 | Modified: if `spdk_mem_unregister` fails, `cudaHostUnregister` still runs, then the SPDK error is returned; a CUDA failure returns the CUDA error; post-condition stated |
| P-6 | 001-gpu-cuda-services | FR-016 provenance | Added: *(Backfilled 2026-11-08 from commit `d45ab524d`, review of PR #591 … `src/lib.rs:1218-1242`.)* |
| P-6 | 001-gpu-cuda-services | `Last-Synced` header | Added (2026-11-08 line, after the 2026-11-07 line) |

Old text check: the drifted sentence "fails, the method returns the error without calling `cudaHostUnregister`." occurred exactly once in `specs/` (Grep), and the full FR-016 "Old text" block from `proposals.md` matched once (Edit tool uniqueness check). The insertion point for the header line (end of the 2026-11-07 `Last-Synced` line, followed by `**Input**:`) also matched once. The resulting FR-016 block is exactly the "New text" block of P-6 in `proposals.md` (checked with `git diff`). Nothing else in the spec changed.

### New Specs Created
- None.

### Implementation Tasks Generated
- 0 ALIGN tasks (there were no ALIGN proposals).
- 1 open item in `.specify/sync/align-tasks.md` (section "2026-11-08 Apply"): P-5, unapproved, approval note quoted.

### Not Applied
| Proposal | Reason |
|---|---|
| P-5 (`IGpuServices` doc comments, `components/interfaces/src/igpu_services.rs`) | Not approved. Note: "Cornel 2026-11-08: not in this PR (edits components/interfaces, re-stamps every component); bundle with the next interfaces change." Recorded as OPEN in align-tasks.md. Also outside the allowed edit scope of this pass. |

### Files changed
- `components/gpu-services/specs/001-gpu-cuda-services/spec.md`
- `components/gpu-services/.specify/sync/align-tasks.md`, `apply-report.{md,json}`, `drift-report.md` (one note under the frontmatter), `backups/20261109T070901Z/`

No `src/` file, no `components/interfaces` file and no file outside `components/gpu-services/{specs,.specify/sync}/` was changed. The `spec_sync_*` frontmatter of `drift-report.md` was not touched. Nothing was committed.

### Observations carried over (no proposal, from `proposals.md`)
1. `tests/host_memory_without_spdk.rs` passes vacuously without a GPU (`:35-39`); only a GPU-host run counts as evidence.
2. No test covers the `d45ab524d` path (SPDK unregister fails → CUDA unregister → SPDK `Err`); needs SPDK + GPU.
3. After the SPDK-failure `Err` the region is already unpinned, so a retry fails in `cudaHostUnregister`; in-tree callers ignore the result (dispatcher `lib.rs:2424`, dispatcher-p2p `lib.rs:1735`). FR-016 now states the post-condition.
4. `allocate_pinned_dma_buffer` → `src/dma.rs:259` calls `spdk_mem_register` without the guard (matches FR-020; dispatcher-side check recommended).

## Verification
Doc-only spec edit. No build or test was run (no code changed). The new FR-016 text describes the code as read at `d45ab524d` (`src/lib.rs:1218-1242`, re-read in this pass); the SPDK-failure path itself has not been executed.

## Next Steps
1. Review: `git diff components/gpu-services/specs/`.
2. Commit: `git add components/gpu-services/specs components/gpu-services/.specify/sync && git commit -m "gpu-services: spec-sync apply P-6 (FR-016, d45ab524d)"`.
3. Re-stamp with `scripts/spec-sync-hash.sh`. Expected status: `clean` (P-5 is optional).
4. Bundle P-5 with the next `components/interfaces` change.

Previous report: `backups/20261109T070901Z/apply-report.md.prev` (2026-11-07, P-1…P-4).
