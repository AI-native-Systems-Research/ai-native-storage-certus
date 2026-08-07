# Spec Sync Apply Report
Applied: 2026-07-21
Project: gpu-services
Based on: proposals from 2026-07-21

## Summary

| Item | Value |
|------|-------|
| Target spec | 001-gpu-cuda-services |
| Proposals applied | 2 (both BACKFILL) |
| FRs added | FR-021, FR-022 |
| Spec files modified | specs/001-gpu-cuda-services/spec.md |
| Backup | .specify/sync/backups/spec.md.bak |

## Changes Applied

### FR-021 - set_device (BACKFILL, HIGH confidence)
Added after FR-020 in spec 001-gpu-cuda-services. Documents the existing
`set_device(device)` method (src/lib.rs:566-592,
interfaces/src/igpu_services.rs:555) that binds the calling thread's CUDA
device context via `cudaSetDevice`.

### FR-022 - device_of_ptr (BACKFILL, HIGH confidence)
Added after FR-021 in spec 001-gpu-cuda-services. Documents the existing
`device_of_ptr(ptr)` method (src/lib.rs:594-633,
interfaces/src/igpu_services.rs:577) that returns the owning CUDA device
ordinal via `cudaPointerGetAttributes`, returning -1 when no device
association exists.

## Before / After

- Before: spec 001 Functional Requirements ended at FR-020 (20 FRs).
- After: spec 001 Functional Requirements end at FR-022 (22 FRs). FR-021 and
  FR-022 inserted between FR-020 and the "Key Entities" section. No existing
  FR text was modified. Numbering and Markdown format match surrounding FRs.

## Verification
- Backup created at .specify/sync/backups/spec.md.bak before edit.
- proposals.json: both proposals set to "approved": true.
- No other spec (002-gpu-ssd-dma-prepare) was modified.

---

# Spec Sync Apply Report (Run 2)
Applied: 2026-07-22
Project: gpu-services
Mode: AUTO-BACKFILL
Based on: drift-report.{json,md} generated 2026-07-22 (2 specs analyzed, 59
requirements checked, 56 aligned, 3 drifted, 4 unspecced features, 1
inter-spec conflict, 1 doc-vs-code drift)

## Summary

| Item | Value |
|------|-------|
| Specs analyzed | 2 (001-gpu-cuda-services, 002-gpu-ssd-dma-prepare) |
| BACKFILL (code -> spec) | 3 FRs added to spec 001 (FR-023, FR-024, FR-025) |
| NEW_SPEC | 1 (003-gpu-p2p-server, backfilled, Draft — needs human review) |
| SUPERSEDE | 0 (no applicable case identified in this drift report) |
| DEFERRED to align-tasks.md | 4 (1 inter-spec conflict, 2 code-vs-spec ambiguities, 1 doc-only follow-up) |
| Spec files modified | specs/001-gpu-cuda-services/spec.md |
| Spec files created | specs/003-gpu-p2p-server/spec.md, specs/003-gpu-p2p-server/tasks.md |
| Files NOT modified (out of scope) | components/gpu-services/CLAUDE.md (source/doc outside specs/**), src/**/*.rs (source code, never touched) |
| Backup | .specify/sync/backups/001-spec.md.20260722T232054Z.bak |

## Changes Applied (BACKFILL)

### FR-023 - stream_query (BACKFILL, code -> spec)
Added after FR-022 in spec 001-gpu-cuda-services. Documents the existing
non-blocking `stream_query(stream)` method (`src/lib.rs:656-678`,
`interfaces/src/igpu_services.rs:596-619`) that wraps `cudaStreamQuery`,
distinct from the blocking `stream_synchronize` already covered by FR-017.

### FR-024 - dma_copy_to_host_async (BACKFILL, code -> spec)
Added after FR-023. Documents the existing async device-to-host copy
method (`src/lib.rs:799-851`, `interfaces/src/igpu_services.rs:694-714`),
the D2H mirror of `dma_copy_to_device_async` (FR-018).

### FR-025 - memcpy_d2h_async (BACKFILL, code -> spec)
Added after FR-024. Documents the existing raw-pointer async
device-to-host copy method (`src/lib.rs:853-895`,
`interfaces/src/igpu_services.rs:716-735`), the D2H mirror of
`memcpy_h2d_async` (FR-019).

## New Spec Created (NEW_SPEC)

### 003-gpu-p2p-server
Created `specs/003-gpu-p2p-server/spec.md` and a minimal
`specs/003-gpu-p2p-server/tasks.md`, `Status: Draft (backfilled — needs
human review)`. Documents the previously-unspecced `gpu-p2p-server` binary
(`src/bin/p2p_server.rs`, 678 lines, `p2p` feature): its CLI contract
(`--socket`, `--pci`, `--mode`, `--staging-size`, `--chunk-size`,
`--once`), the three benchmarking transfer modes (bounce / p2p / p2p-cold),
the staging-buffer pool, the one-line-in/one-line-out Unix socket protocol,
and signal-driven shutdown. Explicitly distinguished from the demo protocol
in `specs/001-gpu-cuda-services/contracts/unix_socket_protocol.md`.

## Superseded (SUPERSEDE)

None. No spec, requirement, or contract in this drift report was found to
be superseded by another; the FR-008 vs FR-021/022/023 disagreement (see
Deferred below) is an unresolved conflict between two still-current specs,
not a supersession.

## Deferred to align-tasks.md (ALIGN / DEFECT / AMBIGUOUS)

All four items below required human judgment and were **not** auto-applied
to any spec or code file. Full detail in `.specify/sync/align-tasks.md`.

1. **[Medium]** `001-gpu-cuda-services/FR-008` vs
   `002-gpu-ssd-dma-prepare/FR-021,FR-022,FR-023` — genuine inter-spec
   conflict (001 mandates "everything through IGpuServices"; 002 mandates
   three standalone DMA constructors; code follows 002). Recommend
   relaxing FR-008 with an explicit `p2p`-carve-out, flagged for human
   sign-off.
2. **[Low]** `001-gpu-cuda-services/FR-005` — `unpin_memory` never
   performs the "full CUDA unregistration for locally-pinned memory"
   branch the spec describes; no code path currently produces a
   locally-pinned (non-IPC) pointer. Ambiguous whether aspirational-spec-
   never-built or intentional simplification; deferred.
3. **[Low]** `001-gpu-cuda-services/FR-015` — `register_host_memory`
   treats SPDK `EBUSY` (-16) as success and skips the documented rollback,
   undocumented in the spec. Ambiguous whether intentional idempotency
   accommodation or an oversight masking a real double-registration bug;
   deferred.
4. **[Low, doc-only]** `components/gpu-services/CLAUDE.md` Overview still
   describes the component as a bare `initialize()`/`shutdown()` skeleton,
   ~25+24 FRs and a new spec (003) stale. Out of scope for this pass
   (component-root `CLAUDE.md` is not under `specs/**`); logged as a
   follow-up doc task only.

## Verification

- Backup of spec 001 created at
  `.specify/sync/backups/001-spec.md.20260722T232054Z.bak` before editing
  (distinct from the pre-existing `spec.md.bak` from the prior run).
- Only Markdown under `components/gpu-services/specs/**` and
  `components/gpu-services/.specify/sync/**` was modified or created in
  this pass. No source code (`.rs` files) was touched.
- Spec 002-gpu-ssd-dma-prepare was not modified (no BACKFILL/NEW_SPEC items
  belonged to it in this run; its one conflict reference is documented in
  align-tasks.md only).
- `components/gpu-services/CLAUDE.md` was intentionally left unmodified
  per hard rules; its staleness is tracked in align-tasks.md instead.

---

# 2026-08-07 Sweep (branch `sync/spec-drift-sweep-20260807`)

Mode: sweep re-analysis of all three gpu-services specs. Pacing: auto-apply
safe BACKFILL/doc-soften on-branch; stop-and-ask only on genuine forks.
Regenerated drift report found spec-001 the only drifted spec (FR-005, FR-008
minor) plus the unspecced `GpuIpcHandle::{verified,pinned}` API surface and the
FR-008-vs-002/003 conflict carried over from the 2026-07-22 run.

## Safe BACKFILL / doc-soften applied (spec Markdown only)

| Spec | Item | Change |
|------|------|--------|
| 001-gpu-cuda-services | FR-005 | Reworded to remove the false "for locally-pinned memory, full CUDA unregistration is performed" clause. New text states `unpin_memory` is tracking-removal-only in all cases and never calls `cudaHostUnregister`; full host un/registration is handled exclusively by `register_host_memory`/`unregister_host_memory` (FR-015/FR-016). Cites `src/lib.rs:249-267`. **Resolves 2026-07-22 align-task FR-005 (option 1, documentation trim).** |
| 001-gpu-cuda-services | US3 acceptance scenario 2 | Softened to a device-type-only check: the implemented `check_memory_attributes` (`src/memory.rs:26`) is a `cudaPointerGetAttributes` device-type check per FR-004; it does not separately diagnose contiguity vs pin status. |
| 002-gpu-ssd-dma-prepare | Auxiliary Public Helpers | Added a new "### Auxiliary Public Helpers *(backfilled)*" subsection before Key Entities, documenting `create_spdk_dma_buffer_from_cuda_malloc`/`spdk_unregister_and_cuda_free`, `get_phys_addr`, `GPU_PAGE_SHIFT` as intentionally-`pub` helpers (not `IGpuServices` methods). |

## Fork resolutions applied (user decisions, all spec-only BACKFILL)

| Fork | User decision | Change applied |
|------|---------------|----------------|
| FR-008 vs 002/003 conflict (2026-07-22 align-task, Medium) | **"Soften FR-008 wording (backfill)"** | FR-008 relaxed with an explicit documented carve-out for the `p2p`/GDRCopy DMA-buffer builders in the `dma` module (`create_spdk_dma_buffer_from_gpu`/`_from_cuda_malloc`/`_from_cuda_host_alloc`/`_from_gpu_bar`/`_from_phys`/`_from_bar_direct`, and `get_phys_addr`) being intentionally `pub`. **Resolves the 2026-07-22 align-task FR-008 (recommended direction, human-approved).** |
| `GpuIpcHandle::{verified,pinned}` unspecced API | **"Document as reserved (backfill)"** | Key Entities `GpuIpcHandle` note appended marking the shared struct's `verified`/`pinned` fields + `set_*`/`is_*` accessors (`components/interfaces/src/igpu_services.rs:63-118`) as **reserved for future use**, deliberately retained rather than removed. |

## Still open (carried forward, NOT resolved this sweep)

- **FR-015 EBUSY special-case** (2026-07-22 align-task, Low) — `register_host_memory` treats SPDK `EBUSY` (-16) as success and skips rollback; not re-surfaced as drift this sweep (spec-001 drift was FR-005/FR-008 only). Remains an open align-task pending maintainer decision (document idempotency vs. remove the special-case).
- **CLAUDE.md staleness** (2026-07-22 align-task, Low, doc-only) — component-root `CLAUDE.md` Overview still describes a bare skeleton; outside `specs/**`, still tracked in align-tasks.md only.

## Verification
- All edits confined to Markdown under `specs/**`. No `.rs` source touched.
- No new backup dir created this sweep (edits are additive annotations dated 2026-08-07 and recoverable via git on-branch).
