# Spec Drift Report

Generated: 2026-07-22T21:30:26Z
Project: gpu-services

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked (FR+SC) | 59 |
| Aligned | 56 (95%) |
| Drifted | 3 (5%) |
| Not Implemented | 0 (0%) |
| Unspecced Features | 4 |
| Inter-Spec Conflicts | 1 |
| Doc-vs-Code Drift | 1 (CLAUDE.md) |

## Spec: 001-gpu-cuda-services -- GPU CUDA Services

22 functional requirements (FR-001..FR-022), 8 success criteria (SC-001..SC-008).

### Aligned (19 FR + 8 SC)

- **FR-001** initialize CUDA, report success/failure -- `src/lib.rs:80-107`
- **FR-002** enumerate GPUs with compute capability >= 7.0, filter below -- `src/device.rs:11-75`
- **FR-003** deserialize base64 IPC handle; no internal `cudaSetDevice` (matches documented precondition) -- `src/ipc.rs:11-71`
- **FR-004** verify device memory via `cudaPointerGetAttributes`, tracked in `verified` set -- `src/memory.rs:8-34`, `src/lib.rs:170-191`
- **FR-006** `create_dma_buffer` requires verified+pinned before building `GpuDmaBuffer` -- `src/lib.rs:247-275`, `src/dma.rs:727-742`
- **FR-007** all operations return `Result<_, String>`; no panics in production paths
- **FR-009** `gpu` feature gate -- `Cargo.toml:9`
- **FR-010** unit tests + Criterion benches gated on `gpu` -- `src/lib.rs:1027-1280`, `Cargo.toml:30-38`
- **FR-011** `dma_copy_to_host` (`cudaMemcpy` D2H, spdk-gated) -- `src/lib.rs:277-327`
- **FR-012** `dma_copy_to_device` (`cudaMemcpy` H2D, spdk-gated) -- `src/lib.rs:487-537`
- **FR-013** `prepare_memory_for_spdk` full pipeline -- `src/lib.rs:329-485`
- **FR-014** GPU-disabled paths error out; `shutdown()` is a no-op `Ok(())` -- `src/lib.rs:109-126`
- **FR-016** `unregister_host_memory` -- `src/lib.rs:987-1024`
- **FR-017** `create_stream`/`destroy_stream`/`stream_synchronize` -- `src/lib.rs:539-564,635-654,680-699`
- **FR-018** `dma_copy_to_device_async` -- `src/lib.rs:701-753`
- **FR-019** `memcpy_h2d_async` -- `src/lib.rs:755-797`
- **FR-020** `allocate_pinned_dma_buffer` -- `src/lib.rs:897-935`
- **FR-021** `set_device` -- `src/lib.rs:566-592`
- **FR-022** `device_of_ptr` -- `src/lib.rs:594-633`
- **SC-001..SC-005** (latency budgets) plausible given single-syscall implementations; not independently re-benchmarked here.
- **SC-006/SC-007** test suite and both Criterion benches present.
- **SC-008** demo apps exist at `apps/gpu-handle-test-client` / `apps/gpu-handle-test-server` (outside this component's tree).

### Drifted (3)

| Requirement | Spec says | Actual | Location | Severity |
|---|---|---|---|---|
| FR-005 | "For locally-pinned memory, full CUDA unregistration is performed" on unpin | `unpin_memory` only removes the pointer from the `pinned` HashSet -- never calls `cudaHostUnregister`. `pin_memory`/`unpin_memory` only ever operate on `GpuIpcHandle`, which by construction is always IPC-derived, so the "locally-pinned" branch the spec describes has no reachable implementation. | `src/lib.rs:227-245` | low |
| FR-008 | "Component MUST expose all functionality exclusively through the IGpuServices interface" | `src/dma.rs` exposes `create_spdk_dma_buffer_from_gpu_bar`, `create_spdk_dma_buffer_from_phys`, `create_spdk_dma_buffer_from_bar_direct`, `get_phys_addr` as public free functions (p2p feature), called directly by `src/bin/p2p_server.rs` and `tests/gpu_nvme_p2p.rs`, bypassing `IGpuServices` entirely. This is required by spec 002 FR-021/022/023 -- see Conflicts below. | `src/dma.rs:352-702`; `src/bin/p2p_server.rs:20-21` | medium |
| FR-015 | Roll back `cudaHostUnregister` if `spdk_mem_register` fails after `cudaHostRegister` succeeds | Correctly rolls back, but additionally treats `spdk_mem_register` rc == -16 (EBUSY) as success and skips rollback -- an undocumented special case | `src/lib.rs:973-981` | low |

### Not Implemented

None.

---

## Spec: 002-gpu-ssd-dma-prepare -- GPU-to-SSD DMA Buffer Preparation

24 functional requirements (FR-001..FR-024), 5 success criteria (SC-001..SC-005). **All aligned.**

### Aligned (24 FR + 5 SC)

- **FR-001** `prepare_memory_for_spdk(&str, Option<u32>) -> DmaBuffer` -- `src/lib.rs:329-485`
- **FR-002** opens IPC handle with `CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS` -- `src/ipc.rs:55`
- **FR-003** pin-state check via internal `pinned` HashSet, not `cudaPointerGetAttributes` -- `src/lib.rs:404-416`
- **FR-004** conditional pinning -- `src/lib.rs:419-448`
- **FR-005** pin decisions logged via logger receptacle -- `src/lib.rs:443-448`
- **FR-006/FR-007** pin-state-aware free functions (`spdk_unregister_unpin_and_ipc_close` vs `spdk_unregister_and_ipc_close`) -- `src/dma.rs:69-102,136-140`
- **FR-008** both free-fn variants close the IPC handle -- `src/dma.rs:80,100`
- **FR-009/FR-010** not-initialized and IPC-open-failure errors -- `src/lib.rs:343-346,393-399`
- **FR-011** peer-access failure surfaces as IPC open error
- **FR-012** no leaks on any error path -- `src/lib.rs:405-468`
- **FR-013** `spdk` feature gate -- `src/lib.rs:329`
- **FR-014/FR-018** device-context set before opening handle and restored on both success and error -- `src/lib.rs:350-372,388,396,412,426,436,465,474`
- **FR-015** returns SPDK `DmaBuffer`, not `GpuDmaBuffer` -- `src/lib.rs:334`
- **FR-016/FR-017** `spdk_mem_register` with `spdk_mem_unregister` rollback -- `src/dma.rs:122-159`
- **FR-019/FR-020** `register_host_memory`/`unregister_host_memory` -- `src/lib.rs:937-1024`
- **FR-021** `create_spdk_dma_buffer_from_gpu_bar` (GDRCopy pin/map + `spdk_mem_register`) -- `src/dma.rs:352-466`
- **FR-022** `create_spdk_dma_buffer_from_phys` (mmap + `rte_extmem_register` + VFIO DMA map) -- `src/dma.rs:546-617` (present and correct, but not invoked from elsewhere in the repo)
- **FR-023** `create_spdk_dma_buffer_from_bar_direct` -- `src/dma.rs:634-702`
- **FR-024** GDRCopy FFI bindings + `GPU_PAGE_SIZE` (64 KiB) -- `src/gdrcopy_ffi.rs:16-33`
- **SC-001..SC-005** verified by code inspection (single-call pipeline, pin-state-correct cleanup, logging, no leaks, consistent error conventions).

### Drifted / Not Implemented

None.

---

## Unspecced Code

| Feature | Location | Lines | Suggested Spec Action |
|---|---|---|---|
| `IGpuServices::stream_query` -- non-blocking check of whether a stream's work has completed (`cudaStreamQuery`) | `interfaces/src/igpu_services.rs:596-619`, `gpu-services/src/lib.rs:656-678` | 23 | New FR in 001, distinct from the blocking `stream_synchronize` (FR-017) |
| `IGpuServices::dma_copy_to_host_async` -- async D2H counterpart to `dma_copy_to_device_async` (FR-018) | `interfaces/src/igpu_services.rs:694-714`, `gpu-services/src/lib.rs:799-851` | 53 | New FR mirroring FR-018 |
| `IGpuServices::memcpy_d2h_async` -- async D2H counterpart to `memcpy_h2d_async` (FR-019), raw-pointer variant | `interfaces/src/igpu_services.rs:716-735`, `gpu-services/src/lib.rs:853-895` | 43 | New FR mirroring FR-019 |
| `gpu-p2p-server` binary -- standalone NVMe->GPU P2P DMA server: Unix-socket CLI, 3 benchmarking transfer modes (bounce/p2p/p2p-cold), staging-buffer pool, signal handling | `src/bin/p2p_server.rs` (entire file) | 678 | New spec (e.g. `003-gpu-p2p-server`) covering its CLI contract and socket protocol -- distinct from the demo protocol in `specs/001-gpu-cuda-services/contracts/unix_socket_protocol.md` |

## Conflicts

1. **FR-008 (spec 001) vs FR-021/022/023 (spec 002)** -- Spec 001 mandates that *all* functionality go through `IGpuServices`. Spec 002 mandates that the three GDRCopy/VFIO DMA-buffer constructors be standalone functions instead. The implementation (`src/dma.rs:352-702`) follows spec 002, so it is in violation of spec 001 FR-008 as literally written. Recommend amending FR-008 to carve out an explicit exception for the `p2p`-gated low-level constructors, since exposing raw device/BAR pointers through a dyn-safe `IGpuServices` trait method is awkward by design.
2. **CLAUDE.md vs implementation** (documentation drift, not spec drift) -- `components/gpu-services/CLAUDE.md` still describes the component as "a skeleton -- `initialize()` and `shutdown()` ... with an optional `ILogger` receptacle." The component now implements 22 (spec 001) + 24 (spec 002) functional requirements across ~3,700 lines (CUDA FFI, device discovery, IPC handling, pin/verify tracking, sync/async DMA, CUDA streams, SPDK registration, GDRCopy P2P). This should be refreshed.

## Recommendations

1. Resolve the FR-008 vs FR-021/022/023 conflict by editing spec 001 FR-008 to add an explicit carve-out for `p2p`-gated low-level DMA-buffer constructors (or move those three functions onto the interface if strict encapsulation is required).
2. Either implement the "locally-pinned memory / full CUDA unregistration" branch described in FR-005, or trim that clause from the spec since `pin_memory`/`unpin_memory` can only ever see IPC-derived pointers today.
3. Document the `spdk_mem_register` rc == -16 (EBUSY) special case in FR-015/FR-019's Assumptions, or remove the special-casing if it was accidental.
4. Backfill FRs for `stream_query`, `dma_copy_to_host_async`, and `memcpy_d2h_async` in spec 001 (they are the natural, already-implemented completions of FR-017/018/019).
5. Write a dedicated spec for the `gpu-p2p-server` binary (`src/bin/p2p_server.rs`), covering its CLI, the three transfer modes, and its Unix socket protocol -- currently the largest unspecced surface in the component (678 lines).
6. Refresh `components/gpu-services/CLAUDE.md`, which still describes the component as a bare skeleton.
