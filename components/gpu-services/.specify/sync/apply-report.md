# Sync Apply Report — gpu-services

Applied: 2026-11-08T00:17:01Z (apply run for the 2026-11-07 proposals) · Branch `fix/dispatcher-j33-repairs` (HEAD `f4579a41f`) · Mode: batch · Approved by Cornel 2026-11-07
Based on: `proposals.json` / `proposals.md` (generated 2026-11-07T12:00:00Z, commit `9a79224a1`, A-14)
Backup: `backups/20261108T001701Z/` (`specs/001-gpu-cuda-services/spec.md`, `specs/002-gpu-ssd-dma-prepare/spec.md`, written before the edits and checked byte-identical with `cmp`; `apply-report.{md,json}.prev`)

## Changes Made

### Specs Updated
| Proposal | Spec | Requirement | Change Type |
|---|---|---|---|
| P-1 | 001-gpu-cuda-services | FR-015 | Modified (SPDK registration only when an SPDK environment is active; no-SPDK case returns `Ok(())` after CUDA pinning) |
| P-2 | 001-gpu-cuda-services | FR-016 | Modified (SPDK unregister only when active; early `Err` on SPDK failure; SHOULD on matching SPDK state) |
| P-3 | 002-gpu-ssd-dma-prepare | FR-019 | Modified (same as P-1) |
| P-4 | 002-gpu-ssd-dma-prepare | FR-020 | Modified (same as P-2) |
| P-1, P-2 | 001-gpu-cuda-services | `Last-Synced` header | Added (2026-11-07 line) |
| P-3, P-4 | 002-gpu-ssd-dma-prepare | `Last-Synced` header | Added (2026-11-07 line) |

Each proposal's old text (the "Old text" block in `proposals.md`) occurred exactly once in its target file before the edit (checked with Grep, then the Edit tool's own uniqueness check). The new text is exactly the "New text" block in `proposals.md`. The rest of FR-015 / FR-019 (PORTABLE flag, idempotence, `we_registered_cuda`) was left unchanged, as the proposals say.

### New Specs Created
- None.

### Implementation Tasks Generated
- 0 ALIGN tasks (there were no ALIGN proposals).
- 1 open item in `.specify/sync/align-tasks.md` (section "2026-11-07 Apply"): P-5, unapproved, with its approval note quoted.

### Not Applied
| Proposal | Reason |
|---|---|
| P-5 (`IGpuServices` doc comments, `components/interfaces/src/igpu_services.rs`) | Not approved. Note: "Cornel 2026-11-07: not in this PR (edits components/interfaces, re-stamps every component); bundle with the next interfaces change." Recorded as an OPEN item in align-tasks.md. |

### Files changed
- `components/gpu-services/specs/001-gpu-cuda-services/spec.md`
- `components/gpu-services/specs/002-gpu-ssd-dma-prepare/spec.md`
- `components/gpu-services/.specify/sync/align-tasks.md`, `apply-report.{md,json}`, `drift-report.md` (note under the frontmatter), `backups/20261108T001701Z/`

No `src/` file, no `components/interfaces` file and no file outside `components/gpu-services/{specs,.specify/sync}/` was changed. The `spec_sync_*` frontmatter of `drift-report.md` was not touched (the stamp script owns it). Nothing was committed.

### Observations carried over (no proposal, from `proposals.md`)
1. The SPDK-environment flag is read independently by each call. Registering while SPDK is inactive and then unregistering while it is active returns `Err` before `cudaHostUnregister`. No caller does this. P-2's SHOULD now documents the constraint.
2. `allocate_pinned_dma_buffer` → `src/dma.rs:259` calls `spdk_mem_register` without the guard. That matches 001 FR-020, but it has the same crash class if reached without SPDK (dispatcher `pipeline.rs:219`). A dispatcher-side check is recommended.

## Verification
Doc-only spec edits. No build or test was run in this apply (no code changed).

## Next Steps
1. Review the updated specs (`git diff components/gpu-services/specs/`).
2. Commit: `git add components/gpu-services/specs components/gpu-services/.specify/sync && git commit -m "gpu-services: spec-sync apply P-1..P-4 (A-14)"`.
3. Re-stamp with `scripts/spec-sync-hash.sh`. Expected status: `clean` (P-5 is optional).
4. Bundle P-5 with the next `components/interfaces` change.

Previous report: `backups/20261108T001701Z/apply-report.md.prev` (2026-10-31, B-1/B-2).
