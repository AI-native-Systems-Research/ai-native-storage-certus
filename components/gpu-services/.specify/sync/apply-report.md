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
