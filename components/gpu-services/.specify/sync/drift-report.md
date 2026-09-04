---
spec_sync_component: gpu-services
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T23:55:59Z
spec_sync_git_commit: e816e33d
spec_sync_inputs_sha256: b39733da974f4d44f70f9bb4a6cfd6d2f68a0fef50ae7158c52c577b4d27f4ab
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: gpu-services

**Generated**: 2026-09-03 (Spec-Sync re-sweep + independent verification)
**Project**: gpu-services
**Specs analyzed**: `001-gpu-cuda-services`, `002-gpu-ssd-dma-prepare`,
`003-gpu-p2p-server`
**Mode**: Read-only drift analysis; **ALIGN** (code fix) for spec 001 FR-017 +
**BACKFILL** (spec doc) for spec 001 FR-008 this sweep; spec 003 FR-012 BACKFILL
carried from the prior sweep; then freshness stamp.

This sweep re-verified **every FR/SC across all three specs against the actual
implementation** (not a re-stamp of the prior artifact). The prior sweep reported
77/78 aligned with a single spec-003 FR-012 drift; independent verification this
sweep found **two additional issues the prior sweep missed** in spec 001 (FR-017
init-guard gap; `pub mod cuda_ffi` unspecced public surface). Both are resolved
this sweep — the component is now genuinely aligned.

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 3 |
| Requirements Checked | 78 (FR-001..025 + SC-001..008 [001]; FR-001..024 + SC-001..005 [002]; FR-001..012 + SC-001..004 [003]) |
| Aligned (after this sweep) | 78 |
| Drifted → resolved this sweep | 2 (001 FR-017 ALIGN, 001 FR-008 BACKFILL) |
| Drifted → resolved prior sweep | 1 (003 FR-012 BACKFILL) |
| Not Implemented | 0 |
| Unspecced (now documented) | 0 |

**Verification runs this sweep** (all green):
- `cargo build -p gpu-services` — clean
- `cargo clippy -p gpu-services --all-targets -- -D warnings` — clean
- `cargo test -p gpu-services -- --test-threads 1` — 5 + 1 passed; 0 failed
- `cargo check -p gpu-services --features gpu` — clean (compiles the edited
  `#[cfg(feature = "gpu")]` stream code)
- `cargo clippy -p gpu-services --features gpu -- -D warnings` — clean

## Resolved this sweep

### 001-gpu-cuda-services / FR-017 — ALIGN (code fix)

- **Severity**: minor (reachable robustness/contract gap).
- **Spec**: FR-017 states `create_stream`, `destroy_stream`, and
  `stream_synchronize` "**All three require prior initialization** and return
  errors when GPU support is not compiled."
- **Actual (before this sweep)**: only `create_stream` enforced the init guard
  (`src/lib.rs:570-572`). `destroy_stream` (`src/lib.rs:657-676`) and
  `stream_synchronize` (`src/lib.rs:702-721`) went straight to the CUDA call
  under `#[cfg(feature = "gpu")]` with **no `!state.initialized` check** — unlike
  the sibling `set_device`/`device_of_ptr` (FR-021/022) which do enforce it
  (`src/lib.rs:598-600,626-628`). The gap is **reachable**, not theoretical:
  `interfaces::GpuStream` is `pub struct GpuStream(pub *mut c_void)`
  (`components/interfaces/src/igpu_services.rs:225`), so a caller can fabricate a
  `GpuStream` and invoke `destroy_stream`/`stream_synchronize` without ever
  calling `create_stream`/`initialize()`.
- **Direction — code authoritative (ALIGN)**: spec 001 is an authored spec (not
  a code-backfill), and three sibling methods already implement exactly this
  guard with identical code; the two omissions are an oversight, so the spec
  states the intended contract and the code had the defect. Backfilling would
  weaken a legitimate contract to match a bug (forbidden by the HARD RULE).
- **Fix applied**: `destroy_stream` and `stream_synchronize` now lock state,
  return `Err("Not initialized: call initialize() first")` when not initialized,
  then drop the lock before the CUDA call — byte-for-byte the pattern used by
  `create_stream`/`set_device`/`device_of_ptr`. `stream_query` (FR-023) is left
  unchanged: FR-023 deliberately requires only the gpu feature + a valid handle,
  and the code matches (`src/lib.rs:678-700`).
- **Location (fixed)**: `components/gpu-services/src/lib.rs`
  (`destroy_stream`, `stream_synchronize`).

### 001-gpu-cuda-services / FR-008 — BACKFILL (spec doc)

- **Severity**: minor (undocumented but intended public surface).
- **Finding**: `pub mod cuda_ffi` (`src/lib.rs:26`, gated behind `--features
  gpu`) exposes raw CUDA FFI publicly but was **not** covered by FR-008's
  "exclusively through the interface" scope or its existing `dma`-module
  exception. Verified it is genuinely-intended public surface: consumed as
  `gpu_services::cuda_ffi::*` by the `gpu-p2p-server` binary
  (`src/bin/p2p_server.rs:20`) and four sibling apps (`certus-server`,
  `certus-server-yaml`, `gpu-bb-vs-p2p`, `nvme-bar1-bench`) for low-level CUDA
  calls (e.g. `cudaHostRegister`) not surfaced through `IGpuServices`. Reducing
  visibility would break those consumers → **BACKFILL** (document the exception),
  not ALIGN.
- **Also documented**: the `dma`-module cleanup free-functions paired with the
  already-excepted builders (`spdk_unregister_and_ipc_close`,
  `spdk_unregister_unpin_and_ipc_close`, `spdk_unregister_and_cuda_free_host`,
  `spdk_unregister_gdr_unmap_and_close`, `vfio_unmap_extmem_munmap`,
  `vfio_unmap_extmem_only`) — `pub` counterparts used as `DmaBuffer` free-fn
  pointers (no external callers found), now named in the FR-008 exception.
- **Change applied**: extended the FR-008 exception note in
  `specs/001-gpu-cuda-services/spec.md`; added a `Last-Synced: 2026-09-03` line.

## Resolved prior sweep (verified again this sweep)

### 003-gpu-p2p-server / FR-012 — BACKFILL (spec doc)

- The original FR-012 asserted a *runtime* MDTS ceiling the code never
  implemented. Verified this sweep: `do_chunked_read` issues one `ReadAsync` per
  `--chunk-size` chunk and awaits all completions
  (`src/bin/p2p_server.rs:273-323`); an oversized chunk surfaces as an NVMe read
  error — `do_chunked_read` returns `Err` (`:308`), the handler propagates it
  (`:399,461,524`), and the accept loop writes `ERROR: <message>` to the client
  (`:658-659`) — i.e. a **safe failure**, not silent corruption. The MDTS bound
  is an operator responsibility documented via CLI help (`:54`) + the 131072-byte
  default. Correct, intentional code + overclaiming (backfilled-from-code) spec →
  BACKFILL. Applied to spec 003 FR-012 (reworded), US1 Scenario 4 (added),
  Assumptions (added). The HARD RULE holds — no bug is masked.

## Aligned ✓ (verified this sweep)

### 001-gpu-cuda-services (FR-001..025, SC-001..008)
Init/shutdown/discovery (`lib.rs:98,129,150`; compute-cap ≥7 filter
`device.rs:39`); memory attr check (`memory.rs:26`); IPC deserialize/verify/pin/
unpin (`lib.rs:166,192,215,249`; `ipc.rs`); `pin_memory` idempotent (`lib.rs:227`)
and `unpin_memory` tracking-only, never `cudaHostUnregister` (`lib.rs:249-267`,
per the 2026-08-07 backfill); DMA copy sync/async (`lib.rs:301,511,725/779/823/
877`); stream ops incl. the two ALIGN fixes above; `set_device`/`device_of_ptr`
init-guarded (`lib.rs:588-654`); host register/unregister with correct rollback +
ordering (`lib.rs:960,1010`); FR-008 interface exposure (+ documented `dma`/
`cuda_ffi` exceptions); FR-009/010 gpu-feature gating + Criterion benches; SC-008
demo apps present.

### 002-gpu-ssd-dma-prepare (FR-001..024, SC-001..005)
`prepare_memory_for_spdk` pipeline (`lib.rs:351`) with lazy peer access
(`ipc.rs:55`), internal-`HashSet` pin check (`lib.rs:427-438`), free-fn selection
by prior-pin state (`dma.rs:69-102,136-140`), device-context set/restore on all
paths (`lib.rs:372-448`), creation-failure rollback (`lib.rs:478-486`),
`spdk_mem_register`/rollback, host register/unregister, and the GDRCopy builders
`_from_gpu_bar`/`_from_phys`/`_from_bar_direct` (`dma.rs:353,547,635`) + GDRCopy
FFI (`gdrcopy_ffi.rs`, `GPU_PAGE_SIZE = 1<<16`).

### 003-gpu-p2p-server (FR-001..012, SC-001..004)
Feature gating, CLI + defaults, kernel-module check, SPDK/GPU/NVMe init + atexit,
p2p staging pool, UDS accept loop + payload parse + mode dispatch (`handle_bounce`
`:374`, `handle_p2p` `:436`, `handle_p2p_cold` `:493`), `OK …`/`ERROR: …` response
contract, `--once`, SIGINT/SIGTERM cleanup, and chunked reads (FR-012, above).

## Not Implemented ✗
None.

## Unspecced Features
None remaining — `pub mod cuda_ffi` and the `dma` cleanup free-functions are now
documented under the spec 001 FR-008 exception; auxiliary `dma.rs`/`gdrcopy_ffi.rs`
items were backfilled into spec 002 in prior rounds.

## Recommendations
- Spec 003 remains marked "backfilled — needs human review"; a maintainer pass
  would let it graduate from backfilled-draft status (doc-only; outside the
  gate's `src/` + `specs/` hash scope).
