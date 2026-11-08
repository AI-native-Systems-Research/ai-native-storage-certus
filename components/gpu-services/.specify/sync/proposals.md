# Drift Resolution Proposals — gpu-services

Generated: 2026-11-07T12:00:00Z · Branch: `fix/dispatcher-j33-repairs` (HEAD `f4579a41f`) · Based on: `drift-report.json` of the same run (delta `32ffa627f..HEAD`, commit `9a79224a1`, A-14).

Mode: batch. **No proposal is approved** (`approved: false` on all). Apply was not run, and no spec, source or interface file was edited.

## Summary

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 5 (P-1…P-4 component specs; P-5 interface doc, **touches `components/interfaces`**) |
| Align (Spec → Code) | 0 |
| Human Decision | 0 (2 observations listed below, no proposal) |
| New Specs | 0 |
| Remove from Spec | 0 |

| Id | Requirement | Type | Approved | Edits |
|---|---|---|---|---|
| P-1 | 001 FR-015 | BACKFILL | false | `specs/001-gpu-cuda-services/spec.md` |
| P-2 | 001 FR-016 | BACKFILL | false | `specs/001-gpu-cuda-services/spec.md` |
| P-3 | 002 FR-019 | BACKFILL | false | `specs/002-gpu-ssd-dma-prepare/spec.md` |
| P-4 | 002 FR-020 | BACKFILL | false | `specs/002-gpu-ssd-dma-prepare/spec.md` |
| P-5 | `IGpuServices` doc comments | BACKFILL (**interfaces**) | false | `components/interfaces/src/igpu_services.rs` — re-stamps every component; optional, recommend deferring |

P-1…P-4 are enough to make this component `clean`: they edit only `components/gpu-services/specs/`. P-5 is a doc-comment fix only. Bundle it with the next change that touches interfaces anyway.

**Why BACKFILL (code is authoritative), for all five:** with no SPDK environment, the SPDK memory map does not exist, so `spdk_mem_register` / `spdk_mem_unregister` dereference it and the process gets SIGSEGV. No code can meet the old unconditional wording in that mode. The change:
- fixes a crash seen in production (node7, A30, J40);
- comes with a regression test, `tests/host_memory_without_spdk.rs:26-58`;
- uses the guard pattern the tree already uses (`components/memory-tier/src/lib.rs:152`, `:314`; `DmaBuffer::drop` at `components/interfaces/src/spdk_types.rs:381`);
- leaves the SPDK-active path byte-for-byte unchanged.

The stricter spec contract (SPDK registration happens whenever the call succeeds) has no implementation that would not crash, so ALIGN is not an option.

---

### P-1: 001-gpu-cuda-services / FR-015

**Type**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Spec says** (`specs/001-gpu-cuda-services/spec.md:309-316`):
> "Component MUST provide a `register_host_memory(ptr, size)` method (gated behind `spdk` feature) that page-locks the specified host memory region via `cudaHostRegister` (enabling async GPU DMA) and registers it with SPDK via `spdk_mem_register` (enabling NVMe DMA). If `cudaHostRegister` succeeds but `spdk_mem_register` fails, the method MUST roll back by calling `cudaHostUnregister` before returning the error."

**Code does**:
- init guard: `src/lib.rs:1146-1150`
- `cudaHostRegister(ptr, size, CUDA_HOST_REGISTER_PORTABLE)`: `:1155-1157`
- `we_registered_cuda`: `:1158`
- **`if !interfaces::is_spdk_env_active() { return Ok(()); }`**: `:1179-1181`, before `spdk_mem_register` at `:1183`
- EBUSY handling and rollback: `:1184-1191`, unchanged

So without an SPDK environment, a successful call means "CUDA-pinned only".

**Old text** (exact, `spec.md:309-316`):
```
- **FR-015**: Component MUST provide a `register_host_memory(ptr, size)`
  method (gated behind `spdk` feature) that page-locks the specified
  host memory region via `cudaHostRegister` (enabling async GPU DMA)
  and registers it with SPDK via `spdk_mem_register` (enabling NVMe
  DMA). If `cudaHostRegister` succeeds but `spdk_mem_register` fails,
  the method MUST roll back by calling `cudaHostUnregister` before
  returning the error. *(Backfilled 2026-09-09 from `src/lib.rs:960-1010`.)*
```
**New text**:
```
- **FR-015**: Component MUST provide a `register_host_memory(ptr, size)`
  method (gated behind `spdk` feature) that page-locks the specified
  host memory region via `cudaHostRegister` (enabling async GPU DMA)
  and, when an SPDK environment is active, registers it with SPDK via
  `spdk_mem_register` (enabling NVMe DMA). If `cudaHostRegister` succeeds
  but `spdk_mem_register` fails, the method MUST roll back by calling
  `cudaHostUnregister` before returning the error. *(Backfilled 2026-09-09
  from `src/lib.rs:960-1010`.)* When no SPDK environment is active
  (`interfaces::is_spdk_env_active()` is false at call time), the method
  MUST NOT call `spdk_mem_register` (there is no SPDK memory map, and the
  call would crash the process); it returns `Ok(())` after the CUDA
  pinning alone, and the region is then usable for async GPU transfers but
  not as an NVMe DMA target. *(Backfilled 2026-11-07 from commit
  `9a79224a1`, A-14; `src/lib.rs:1179-1181`; test
  `tests/host_memory_without_spdk.rs`.)*
```
The rest of FR-015 (PORTABLE flag, idempotence, `we_registered_cuda`, `spec.md:317-326`) stays as written. It is still accurate.

**Rationale**: see "Why BACKFILL" above. The flag is read at call time, and the wording says so.

---

### P-2: 001-gpu-cuda-services / FR-016

**Type**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Spec says** (`specs/001-gpu-cuda-services/spec.md:327-331`):
> "Component MUST provide an `unregister_host_memory(ptr, size)` method (gated behind `spdk` feature) that unregisters memory from SPDK via `spdk_mem_unregister` then removes page-locking via `cudaHostUnregister`. MUST be called before freeing the underlying allocation."

**Code does**:
- `spdk_mem_unregister` only `if interfaces::is_spdk_env_active()`: `src/lib.rs:1220-1225`
- a non-zero rc returns `Err` before the CUDA step: `:1222-1224`
- `cudaHostUnregister` always: `:1228-1234`

**Old text** (exact, `spec.md:327-331`):
```
- **FR-016**: Component MUST provide an `unregister_host_memory(ptr,
  size)` method (gated behind `spdk` feature) that unregisters memory
  from SPDK via `spdk_mem_unregister` then removes page-locking via
  `cudaHostUnregister`. MUST be called before freeing the underlying
  allocation.
```
**New text**:
```
- **FR-016**: Component MUST provide an `unregister_host_memory(ptr,
  size)` method (gated behind `spdk` feature) that, when an SPDK
  environment is active, unregisters memory from SPDK via
  `spdk_mem_unregister`, then removes page-locking via
  `cudaHostUnregister`. When no SPDK environment is active
  (`interfaces::is_spdk_env_active()` is false at call time) the SPDK step
  is skipped and only `cudaHostUnregister` runs. If `spdk_mem_unregister`
  fails, the method returns the error without calling `cudaHostUnregister`.
  MUST be called before freeing the underlying allocation. Callers SHOULD
  register and unregister under the same SPDK-environment state (e.g. in
  `initialize` / `shutdown`); the flag is read independently by each call.
  *(Backfilled 2026-11-07 from commit `9a79224a1`, A-14;
  `src/lib.rs:1220-1225`.)*
```
**Rationale**: matches the code, including the existing early `Err` on SPDK failure, which the old text did not state. The SHOULD makes the call-time edge case visible (see the drift report observation). No caller in the tree violates it: dispatcher `components/dispatcher/src/lib.rs:2256` / `:2419`, dispatcher-p2p `components/dispatcher-p2p/src/lib.rs:1451` / `:1735`.

---

### P-3: 002-gpu-ssd-dma-prepare / FR-019

**Type**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Spec says** (`specs/002-gpu-ssd-dma-prepare/spec.md:95`):
> "… page-locks an existing host allocation via `cudaHostRegister` (enabling async GPU H2D/D2H DMA from the GPU's DMA engine) and registers it with SPDK via `spdk_mem_register` (enabling NVMe controllers to DMA directly to/from it). On partial failure (CUDA succeeds, SPDK fails), MUST roll back `cudaHostRegister` before returning error. *(Backfilled 2026-09-09; consistent with spec-001 FR-015.)*"

**Code does**: same as P-1 (`src/lib.rs:1179-1181`).

**Old text** (exact substring of `spec.md:95`):
```
and registers it with SPDK via `spdk_mem_register` (enabling NVMe controllers to DMA directly to/from it). On partial failure (CUDA succeeds, SPDK fails), MUST roll back `cudaHostRegister` before returning error. *(Backfilled 2026-09-09; consistent with spec-001 FR-015.)*
```
**New text**:
```
and, when an SPDK environment is active, registers it with SPDK via `spdk_mem_register` (enabling NVMe controllers to DMA directly to/from it). On partial failure (CUDA succeeds, SPDK fails), MUST roll back `cudaHostRegister` before returning error. *(Backfilled 2026-09-09; consistent with spec-001 FR-015.)* When no SPDK environment is active (`interfaces::is_spdk_env_active()` false at call time) the method MUST NOT call `spdk_mem_register` and returns `Ok(())` after CUDA pinning alone. *(Backfilled 2026-11-07, A-14, `9a79224a1`; consistent with spec-001 FR-015.)*
```
The rest of the line (PORTABLE, idempotence, `we_registered_cuda`) is unchanged.

---

### P-4: 002-gpu-ssd-dma-prepare / FR-020

**Type**: BACKFILL · **Confidence**: HIGH · **approved: false**

**Spec says** (`specs/002-gpu-ssd-dma-prepare/spec.md:96`):
> "… that reverses FR-019: calls `spdk_mem_unregister` then `cudaHostUnregister`. MUST be called before the underlying allocation is freed."

**Code does**: same as P-2 (`src/lib.rs:1220-1225`, `:1228-1234`).

**Old text** (exact, `spec.md:96`):
```
- **FR-020**: The interface MUST provide an `unregister_host_memory(ptr, size)` method (gated behind `spdk` feature) that reverses FR-019: calls `spdk_mem_unregister` then `cudaHostUnregister`. MUST be called before the underlying allocation is freed.
```
**New text**:
```
- **FR-020**: The interface MUST provide an `unregister_host_memory(ptr, size)` method (gated behind `spdk` feature) that reverses FR-019: calls `spdk_mem_unregister` (only when an SPDK environment is active at call time) then `cudaHostUnregister`. MUST be called before the underlying allocation is freed. *(Backfilled 2026-11-07, A-14, `9a79224a1`; consistent with spec-001 FR-016.)*
```

---

### P-5: `IGpuServices::register_host_memory` / `unregister_host_memory` doc comments — **EDITS `components/interfaces` (re-stamps every component)**

**Type**: BACKFILL (interface doc) · **Confidence**: HIGH on the content, LOW priority · **approved: false**

**Doc says** (`components/interfaces/src/igpu_services.rs`):
- `:786`: "Register existing host memory as CUDA-pinned and SPDK DMA-capable."
- `:788-790`: "Calls `cudaHostRegister` to page-lock the memory (enabling async GPU DMA) then `spdk_mem_register` so NVMe controllers can DMA directly to/from it."
- `:802`: "Returns an error if CUDA host registration or SPDK registration fails."
- `:808-809`: "Calls `spdk_mem_unregister` then `cudaHostUnregister`. Must be called before freeing the underlying allocation."

**Code does**: `components/gpu-services/src/lib.rs:1179-1181` and `:1220-1225` (SPDK step conditional). The `# Errors` line at `:802` is still literally true, because SPDK registration can only fail when it is attempted. Only the description lines are incomplete.

**Old text** (exact, `igpu_services.rs:788-790`):
```
        /// Calls `cudaHostRegister` to page-lock the memory (enabling async
        /// GPU DMA) then `spdk_mem_register` so NVMe controllers can DMA
        /// directly to/from it.
```
**New text**:
```
        /// Calls `cudaHostRegister` to page-lock the memory (enabling async
        /// GPU DMA) then, if an SPDK environment is active
        /// ([`crate::is_spdk_env_active`]), `spdk_mem_register` so NVMe
        /// controllers can DMA directly to/from it. Without an SPDK
        /// environment the memory is CUDA-pinned only.
```
**Old text** (exact, `igpu_services.rs:808-809`):
```
        /// Calls `spdk_mem_unregister` then `cudaHostUnregister`. Must be
        /// called before freeing the underlying allocation.
```
**New text**:
```
        /// Calls `spdk_mem_unregister` (only if an SPDK environment is
        /// active) then `cudaHostUnregister`. Must be called before freeing
        /// the underlying allocation.
```
Leave `:786` and `:802` unchanged, or change `:786` to "Register existing host memory as CUDA-pinned and, with an SPDK environment, SPDK DMA-capable."

Two caveats before applying:
- `is_spdk_env_active` is re-exported only under `cfg(feature = "spdk")` (`components/interfaces/src/lib.rs:112-113`). Both methods are `spdk`-gated too, so the intra-doc link resolves. Even so, run `cargo doc -p interfaces --no-deps --features spdk` to confirm it is warning-free, or drop the link and use plain backticks.
- **This edits `components/interfaces`, which changes every component's spec-sync digest.** Recommendation: defer and bundle with the next interfaces change. The component is `clean` without it, because the interfaces spec (`components/interfaces/specs/001-interfaces/spec.md:274-275`) is generic and still aligned.

---

## Observations (no proposal; for a human)

1. **Call-time flag, independent per call.** If a region is registered while SPDK is inactive and unregistered after SPDK became active, `spdk_mem_unregister` runs on a region SPDK never saw. A non-zero rc returns `Err` before `cudaHostUnregister` (`src/lib.rs:1222-1224`), so the CUDA pin is left in place. No caller does this today. P-2's SHOULD documents the constraint. A code ALIGN (tracking per-region SPDK registration, like `dma::registered_regions`) is possible but not proposed, because no requirement asks for it.
2. **`allocate_pinned_dma_buffer` has the same crash class.**
   - It goes `src/lib.rs:1097-1130` → `dma::create_spdk_dma_buffer_from_cuda_host_alloc`, which calls `spdk_mem_register` unguarded (`src/dma.rs:259`).
   - This matches 001 FR-020 (`spec.md:350-356`), so it is not drift.
   - However, the dispatcher's `StagingPool::new` calls it (`components/dispatcher/src/pipeline.rs:219`). If that is reachable in memory-tier-only mode, it would SIGSEGV like A-14 did.
   - Worth a dispatcher-side check. Out of scope for this component's sync.

## Verification status

Static only. I could not run `cargo test` / `cargo check` in this session (the commands were not approved). The new test needs a real GPU and `--features p2p`. The node7 pass reported in commit `9a79224a1` was not reproduced here.

Previous proposals: `backups/20261107T120000Z/proposals.md.prev` (2026-10-31, B-1/B-2, applied).
