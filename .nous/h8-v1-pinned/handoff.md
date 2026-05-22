# Handoff — h8-v1-pinned, Iteration 2

## Goal

Test whether P2P SSD→GPU DMA performance improves when using a same-NUMA NVMe device (NODE-level PCIe topology) vs the cross-NUMA device (SYS-level) that showed 65% penalty in iter-1. Run four conditions: bounce and P2P at each topology (NODE with `63:00.0`, SYS with `c2:00.0`). This isolates the NUMA-interconnect contribution to P2P overhead.

## Key Discoveries

1. **PCIe topology fully mapped**: GPU0 (`41:00.0`) is on root complex `0000:40` (NUMA 0). NVMe `63:00.0` is on root complex `0000:60` (NUMA 0) — NODE-level connection. NVMe `c2:00.0` is on root complex `0000:c0` (NUMA 1) — SYS-level connection. No NVMe on this system shares a root complex with any GPU.

2. **Iter-1 used SYS-level path**: NVMe `c2:00.0` (NUMA 1) with GPU0 (NUMA 0) crossed the AMD Infinity Fabric inter-socket link. The 65% P2P penalty (10498 vs 6336 us) includes both the cross-NUMA latency AND the cross-root-complex overhead. Iter-2 separates these by testing at NODE level.

3. **nvidia-smi topo confirms topology**: GPU0→NIC0 shows NODE (same-NUMA, different root complex). GPU0→GPU1 shows SYS (cross-NUMA). This is consistent with our device placement analysis.

4. **All NVMe devices bound to vfio-pci**: Confirmed by checking `/sys/bus/pci/devices/0000:63:00.0/driver` → `vfio-pci`. Both `63:00.0` and `c2:00.0` are available for SPDK.

5. **Iter-1 handoff NUMA claim was incorrect**: Handoff stated "c-bus NVMe, NUMA node 0" but `/sys/bus/pci/devices/0000:c2:00.0/numa_node` returns 1. The findings correctly identified the topology mismatch but the textual description was wrong.

6. **CPU affinity consistent**: Both NVMe `63:00.0` and GPU0 have `local_cpulist: 0-15,32-47` (NUMA 0). This means the SPDK reactor thread for `63:00.0` will run on NUMA-0 cores, with CPU cache locality for the DMA completion path.

7. **Same P2P code for both topologies**: The P2P implementation is topology-agnostic — same `prepare_memory_for_spdk`, same sub-buffer views, same ReadSync pattern. Only the `--data-pci` flag changes between conditions.

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
- **Run server (NODE-level, conditions A/B):**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/certus-server \
    --metadata-pci 0000:63:00.0 \
    --data-pci 0000:63:00.0 \
    --dispatcher-version v1 \
    --listen 0.0.0.0:50051
  ```
- **Run server (SYS-level, conditions C/D):**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:c1:00.0 /var/tmp/spdk_pci_lock_0000:c2:00.0 && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/certus-server \
    --metadata-pci 0000:c1:00.0 \
    --data-pci 0000:c2:00.0 \
    --dispatcher-version v1 \
    --listen 0.0.0.0:50051
  ```
- **Run client benchmark (1 iteration, 10 objects):**
  ```bash
  cd apps/certus-server/python-client && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 test_client.py \
    --server localhost:50051 \
    --bench-only \
    --bench-object-size 4194304 \
    --bench-num-objects 10 \
    --bench-iterations 1
  ```
- **Output format:** Stdout table with columns: Tier, Avg (us/obj), Min (us/obj), Max (us/obj), Avg (GB/s), Peak (GB/s). Parse "SSD-tier" row.
- **Baseline result (from iter-1, SYS-level):** Bounce: 6335.9 us/obj, 0.66 GB/s. P2P: 10498.3 us/obj, 0.40 GB/s.

## Code Map

| File:Line | What's there | When to look |
|-----------|-------------|--------------|
| `components/dispatcher/v1/src/pipeline.rs:30-123` | `pipelined_ssd_to_gpu` — existing sequential bounce | Baseline reference; new P2P function goes after this |
| `components/dispatcher/v1/src/pipeline.rs:16` | `PIPELINE_RING_SIZE = 4` | Ring size constant |
| `components/dispatcher/v1/src/lib.rs:60-79` | `define_component!` block — fields section | Add `gpu_dma_cache` field here |
| `components/dispatcher/v1/src/lib.rs:87` | `noop_free` function | Reuse for GPU sub-buffer views |
| `components/dispatcher/v1/src/lib.rs:190-266` | `promote_and_serve` — SSD read orchestration | Add P2P routing branch before line 240 |
| `components/dispatcher/v1/src/lib.rs:728-815` | `fn lookup` — dispatches by LookupResult | Entry point; BlockDevice at 805 calls `promote_and_serve` |
| `components/dispatcher/v1/src/io_segmenter.rs:22-55` | `segment_io()` — splits into 128 KiB segments | Used by P2P function |
| `components/interfaces/src/idispatcher.rs:112-118` | `IpcHandle` struct | Add `cuda_ipc_handle_bytes` field |
| `components/interfaces/src/igpu_services.rs:460-463` | `prepare_memory_for_spdk` signature | Called on cache miss |
| `components/interfaces/src/iblock_device.rs:187-194` | `Command::ReadSync` variant | ReadSync takes `Arc<Mutex<DmaBuffer>>` |
| `components/interfaces/src/spdk_types.rs:293-316` | `DmaBuffer::from_raw` | Create GPU sub-buffer views |
| `apps/certus-server/src/main.rs:27-51` | CLI argument parsing | PCI address flags |
| `apps/certus-server/src/service.rs:157` | IpcHandle in populate handler | Add `cuda_ipc_handle_bytes: None` |
| `apps/certus-server/src/service.rs:233` | IpcHandle construction in lookup | Add `cuda_ipc_handle_bytes: Some(...)` |
| `components/gpu-services/v0/src/ipc.rs:11-37` | `decode_ipc_payload` — base64 → (handle, size) | Payload format: 72 bytes = 64 handle + 8 LE size |
| `components/gpu-services/v0/src/dma.rs:114-162` | `create_spdk_dma_buffer_from_gpu` | spdk_mem_register logic |
| `apps/certus-server/python-client/test_client.py:310-439` | `bench_lookup_latency` | Benchmark flow |
| `apps/certus-server/python-client/test_client.py:344` | `lookup_tensor` allocation on cuda:0 | GPU0 is always the target |

## Code Targets

### 1. IpcHandle extension (`components/interfaces/src/idispatcher.rs:112-118`)
Add `pub cuda_ipc_handle_bytes: Option<Vec<u8>>` after `size: u32`. Update doc example. All `IpcHandle {..}` sites need `cuda_ipc_handle_bytes: None` (or `Some(...)` for the lookup service path). Grep `IpcHandle {` to find all sites (~20+).

### 2. GPU DMA cache field (`components/dispatcher/v1/src/lib.rs:71-76`)
In `define_component!` fields block, add:
```rust
gpu_dma_cache: Mutex<HashMap<[u8; 64], Arc<Mutex<DmaBuffer>>>>,
```
Key: 64-byte CUDA IPC handle. Value: the DmaBuffer from `prepare_memory_for_spdk`.

### 3. Cache lookup method (`components/dispatcher/v1/src/lib.rs`, new impl method)
```rust
fn get_or_create_gpu_dma(
    &self,
    handle_bytes: &[u8],
    total_size: usize,
    gpu: &dyn IGpuServices,
) -> Result<Arc<Mutex<DmaBuffer>>, DispatcherError>
```
Construct 72-byte payload (64 handle + 8 LE size), base64 encode, call `gpu.prepare_memory_for_spdk(&encoded, None)`, wrap in Arc<Mutex>, insert into cache. Use `base64::engine::general_purpose::STANDARD`.

### 4. P2P function (`components/dispatcher/v1/src/pipeline.rs`, after line 123)
```rust
pub unsafe fn p2p_ssd_to_gpu_persistent(
    drive: &dyn IBlockDevice,
    gpu_dma_buf: &DmaBuffer,
    start_lba: u64,
    total_bytes: usize,
    numa_node: i32,
) -> Result<(), DispatcherError>
```
Uses `segment_io` → per-segment: create sub-DmaBuffer view via `from_raw(base_ptr + offset, length, noop_free, -1)` → wrap in Arc<Mutex<>> → ReadSync → await ReadDone → forget sub-buffer.

### 5. P2P routing in promote_and_serve (`components/dispatcher/v1/src/lib.rs:~240`)
Before existing `pipelined_ssd_to_gpu` call: check `ipc_handle.cuda_ipc_handle_bytes.is_some()`. If yes:
- Call `get_or_create_gpu_dma` with the handle bytes
- Lock the cached DmaBuffer to get base pointer and size info
- Call `p2p_ssd_to_gpu_persistent`
- Do NOT insert into memory-tier, do NOT update dispatch-map
- `dm.release_write(key)` and return Ok(())

### 6. Service layer pass-through (`apps/certus-server/src/service.rs:233`)
In lookup: `cuda_ipc_handle_bytes: Some(handle.cuda_ipc_handle.clone())`
In populate: `cuda_ipc_handle_bytes: None`

### 7. Cargo.toml (`components/dispatcher/v1/Cargo.toml`)
Add `base64 = "0.22"` to [dependencies].

## What I Tried That Didn't Work

- **P2P without caching** (prior experiment h8-v1-vs-p2p): 17.5% slower. Per-lookup `prepare_memory_for_spdk` (~50μs) × 200 calls exceeded the cudaMemcpy savings.
- **Cross-NUMA P2P** (iter-1, SYS-level `c2:00.0`): 65% slower than bounce. PCIe path traverses Infinity Fabric + two root complex boundaries.
- **P2P with memory-tier promotion**: GPU BAR1 → host copy is uncacheable MMIO (~1 GB/s). Not viable for memory-tier population.

## What I Excluded and Why

- **BatchSubmit / parallel reads**: Research question for this iteration focuses on topology effect isolation. Parallelism is a separate variable for a future iteration (after confirming whether topology alone can make P2P competitive).
- **Memory-tier population in P2P path**: Benchmark measures cold SSD lookups. P2P leaves entries as BlockDevice so each iteration re-tests SSD→GPU.
- **Release build**: PCIe DMA latency (microseconds) dominates CPU overhead (nanoseconds). Debug build doesn't change ratios.
- **GPU1 (`a1:00.0`, NUMA 1)**: Would pair well with NVMe c1-c3 (both NUMA 1), but the Python client uses `cuda:0` (GPU0). Changing to GPU1 would require client modification.
- **Multi-iteration measurements**: Avoided due to RP-6 (memory-tier confound). Single iteration, first-hit only.

## Evolution of Thinking

1. **Starting point from iter-1**: "P2P is 65% slower — PCIe topology mismatch is the cause." Recommended verifying with `nvidia-smi topo -m` and `lspci -tvvv`.

2. **Topology verification revealed the full picture**: The system has AMD EPYC Milan with 8 root complexes. No NVMe shares a root complex with any GPU. The best achievable is NODE (same NUMA, adjacent root complexes). This means we cannot test the "same root complex" ideal case.

3. **Revised hypothesis**: Even without same-root-complex, eliminating the NUMA interconnect traversal (Infinity Fabric) should reduce the P2P penalty significantly. The penalty has two components: (a) inter-socket Infinity Fabric latency and (b) cross-root-complex PCIe routing within a socket. Iter-2 isolates component (a).

4. **Experimental design choice**: Running both bounce and P2P at both topologies (2×2 factorial) gives clean isolation of the topology effect and validates measurement repeatability against iter-1.

5. **If NODE is still 65% slower**: Then the dominant overhead is cross-root-complex (not NUMA), and P2P will never be competitive on this hardware without same-root-complex NVMe↔GPU placement. The correct response would be to abandon P2P and pivot entirely to parallelism (BatchSubmit+ReadAsync) to improve bounce-path performance.

## Current Status

- **Validated:** PCIe topology (lspci, nvidia-smi topo), NUMA affinity (sysfs), device availability (vfio-pci), build command, benchmark invocation, iter-1 reproducibility target
- **Uncertain:** Whether NODE-level P2P will show measurably less penalty than SYS-level. The AMD EPYC internal PCIe fabric between root complexes 0000:40 and 0000:60 may still add substantial latency.
- **Suggested next (for iter-3):**
  - If NODE P2P is competitive (< 20% penalty): combine P2P + BatchSubmit at NODE topology for compounded improvement.
  - If NODE P2P is still significantly slower (> 50% penalty): abandon P2P path for this hardware, pivot to bounce + BatchSubmit (parallel NVMe reads with ring buffers) as the optimization target.
  - Either way: investigate bounce + BatchSubmit as an alternative that doesn't require favorable topology.

## Warnings & Constraints

- **SPDK singleton**: Only one SPDK process per NVMe. Kill certus-server before restarting: `pkill -f certus-server`
- **Lock file cleanup**: Always `rm -f /var/tmp/spdk_pci_lock_*` before starting
- **nvidia-peermem required**: `lsmod | grep nvidia_peermem` must show loaded
- **gdrdrv required**: `lsmod | grep gdrdrv` — GDRCopy needed for `prepare_memory_for_spdk`
- **Different metadata PCI for SYS conditions**: Use `--metadata-pci 0000:c1:00.0` with `--data-pci 0000:c2:00.0` (same as iter-1). For NODE conditions use `--metadata-pci 0000:63:00.0` with `--data-pci 0000:63:00.0` (same device for both).
- **IpcHandle field breaks all sites**: After adding `cuda_ipc_handle_bytes`, grep `IpcHandle {` — expect ~20+ sites needing `cuda_ipc_handle_bytes: None`
- **Sub-buffer forget pattern**: After ReadSync completes on a sub-DmaBuffer view, `std::mem::forget(sub_buf)` prevents drop glue. Safe because noop_free does nothing anyway.
- **Cache entry lifetime**: Cache holds IPC handles and SPDK registrations for server lifetime. Safe because benchmark client keeps tensors alive for the full duration.
- **Same IPC handle across objects**: Benchmark pre-allocates one lookup tensor (`test_client.py:344`). Cache will have exactly 1 entry.
- **Condition ordering**: Run A (bounce, NODE) → B (P2P, NODE) → C (bounce, SYS) → D (P2P, SYS). Conditions A/B share the same server PCI config. C/D require different PCI addresses.
- **Between B and C**: Must kill server, clean lock files, restart with SYS-level PCI addresses. The P2P code patch remains applied — it only activates when `cuda_ipc_handle_bytes` is present in the lookup request.
