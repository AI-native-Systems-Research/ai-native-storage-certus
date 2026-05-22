# Handoff — h8-v0-pinned, Iteration 1

## Goal

Implement a P2P read path in dispatcher v0 with **persistent GPU staging** (pre-pinned buffer reused across the entire lookup batch), then benchmark it against the existing bounce path. All tests run through certus-server with `--dispatcher-version v0`. The key difference from h8-v0-vs-p2p is that `prepare_memory_for_spdk` is called ONCE per batch (not per lookup), amortizing the 4-5ms setup overhead that previously made P2P 33% slower.

## Key Discoveries

1. **Previous experiment (h8-v0-vs-p2p iter-1) showed P2P is 33% slower** — bounce 13764 us/obj vs P2P 18372 us/obj for 4 MiB. Root cause: `prepare_memory_for_spdk` called per-lookup adds cudaIpcOpenMemHandle + spdk_mem_register overhead (~4-5ms each call).

2. **Pre-pinning amortization math**: With 10 objects per tier and 20 iterations, one `prepare_memory_for_spdk` call (~5ms) amortized over 10×20=200 lookups = 0.025ms per lookup (negligible). This should reveal the raw DMA path difference.

3. **Sub-buffer DMA views work**: The previous experiment validated that `DmaBuffer::from_raw(gpu_base_ptr + chunk_offset, 128KiB, noop_free, -1)` works correctly with SPDK DMA. NVMe targets the correct GPU BAR1 physical address.

4. **Promotion requirement adds complexity**: The P2P path must also copy data to host DRAM for subsequent lookup caching. This means P2P does: NVMe→GPU + host copy for promotion. Bounce does: NVMe→host + GPU copy. Same total data moved, different order. The ablation arm tests whether promotion negates the path advantage.

5. **Python client benchmark reuses a single GPU buffer across all lookups** (`test_client.py:344-347`): It allocates one `lookup_tensor` and reuses its IPC handle. This means the server receives the same CUDA IPC handle bytes for all entries in a batch — ideal for the pre-pinning approach.

6. **certus-server lookup handler opens IPC handles with caching** (`service.rs:189`): For the bounce path, it caches opened handles within a batch (HashMap keyed by 64-byte handle). For P2P, we replace this with a single `prepare_memory_for_spdk` call at batch start.

7. **4 MiB = 32 chunks of 128 KiB** at default MDTS. Sequential ReadSync: each chunk is ~0.3-0.4ms (from the 13ms total / 32 chunks in baseline). The final cudaMemcpy for 4 MiB is ~1-2ms. P2P eliminates this final copy but the individual chunk DMA-to-GPU-BAR1 may be slightly slower per chunk due to BAR1 write combining behavior.

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
- **Run server (bounce baseline):**
  ```bash
  rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  ./target/debug/certus-server \
    --metadata-pci 0000:63:00.0 \
    --data-pci 0000:63:00.0 \
    --dispatcher-version v0 \
    --listen 0.0.0.0:50051
  ```
- **Run client benchmark:**
  ```bash
  cd apps/certus-server/python-client && \
  LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
  python3 test_client.py \
    --server localhost:50051 \
    --bench \
    --bench-object-size 4194304 \
    --bench-num-objects 10 \
    --bench-iterations 20
  ```
- **Output format:** Stdout table: Tier | Avg (us/obj) | Min (us/obj) | Max (us/obj) | Avg (GB/s) | Peak (GB/s). Parse "SSD-tier" row.
- **Baseline result:** SSD-tier 13763.8 us/obj (0.30 GB/s) from h8-v0-vs-p2p iter-1.

## Code Map

| File:Line | What's there | When to look |
|-----------|-------------|--------------|
| `components/dispatcher/v0/src/lib.rs:179-276` | `read_from_block_device` — bounce path (sequential ReadSync + dma_copy_to_device) | Primary target: implement P2P alternative after this method |
| `components/dispatcher/v0/src/lib.rs:694-747` | `IDispatcher::lookup` — routes to staging vs block-device path | Add P2P routing: if `cuda_ipc_handle_bytes.is_some()`, call P2P method |
| `components/dispatcher/v0/src/lib.rs:218-263` | Sequential ReadSync loop (per-chunk pattern) | Replicate for P2P but with GPU sub-view buffers |
| `components/dispatcher/v0/src/lib.rs:266-273` | `gpu.dma_copy_to_device` — the final H2D copy P2P eliminates | Verify this is NOT called in P2P path |
| `components/interfaces/src/idispatcher.rs:113-118` | `IpcHandle` struct (currently: address + size only) | Add `cuda_ipc_handle_bytes: Option<Vec<u8>>` field |
| `apps/certus-server/src/service.rs:176-253` | Lookup gRPC handler with IPC handle caching | Replace with single prepare_memory_for_spdk at batch start |
| `apps/certus-server/src/service.rs:65-93` | `open_cuda_ipc` function | Still needed for populate and bounce control |
| `components/gpu-services/v0/src/lib.rs:330-479` | `prepare_memory_for_spdk` implementation | Understanding the 72-byte payload format and what it returns |
| `components/dispatcher/v0/src/io_segmenter.rs:22-55` | `segment_io` function | Generates 32 segments for 4 MiB at 128 KiB MDTS |
| `apps/certus-server/python-client/test_client.py:310-439` | `bench_lookup_latency` function | Full benchmark flow — reuses single GPU buffer |
| `components/dispatcher/v0/Cargo.toml` | Dependencies | Add `base64 = "0.22"` |

## Code Targets

### 1. IpcHandle extension (`components/interfaces/src/idispatcher.rs:113-118`)
Add `pub cuda_ipc_handle_bytes: Option<Vec<u8>>` field. Update doc example at line 108. All existing construction sites use `cuda_ipc_handle_bytes: None`. This is identical to what h8-v0-vs-p2p did — same patch applies.

### 2. Service.rs Lookup handler (`apps/certus-server/src/service.rs:176-253`)
**Key difference from h8-v0-vs-p2p**: Instead of removing the IPC cache and passing raw bytes through (which causes per-lookup prepare_memory_for_spdk), we:
- Keep the IPC handle open (for staging lookups that use dma_copy_to_device)
- Also pass raw bytes through via `cuda_ipc_handle_bytes: Some(bytes)`
- The dispatcher handles the caching/pinning internally

### 3. Dispatcher P2P-pinned read method (`components/dispatcher/v0/src/lib.rs`, after line 276)
New method `read_from_block_device_p2p_pinned`:
- Check if a cached GPU DmaBuffer exists for this IPC handle; if not, call prepare_memory_for_spdk to create one and cache it
- Run io_segmenter → for each segment, create sub-view via `DmaBuffer::from_raw(gpu_ptr + offset, length, noop_free, -1)`
- Issue ReadSync into each sub-view sequentially
- After all chunks complete: optionally copy data from a host buffer to staging (for promotion)
- Return Ok(())

### 4. P2P buffer cache (`components/dispatcher/v0/src/lib.rs`, fields section)
Add a `Mutex<HashMap<[u8; 64], DmaBuffer>>` field to DispatcherComponentV0 (via define_component! fields). Caches prepared GPU buffers keyed by CUDA IPC handle bytes. First lookup creates it; subsequent lookups reuse.

## What I Tried That Didn't Work

- **Per-lookup prepare_memory_for_spdk (h8-v0-vs-p2p)**: 33% slower than bounce due to 4-5ms setup per call. This experiment fixes it with caching.
- **Explore agents failed** due to model auth errors.

## What I Excluded and Why

- **BatchSubmit / pipelined reads**: Research question is about sequential ReadSync. Pipelining is a separate dimension.
- **Larger/smaller object sizes**: Focus on 4 MiB. Prior data shows same direction at 1 MiB.
- **gpu-p2p-server modifications**: Forbidden by constraints.
- **Standalone benchmark binaries**: Forbidden by constraints.
- **Release build**: DMA latency dominates; debug is fine for conclusions.

## Evolution of Thinking

1. **Started with**: "Just pre-pin at batch start in the service layer." But realized prepare_memory_for_spdk returns a DmaBuffer the dispatcher needs for ReadSync, not the service layer.
2. **Key insight**: Cache in the dispatcher keyed by IPC handle bytes. Python client uses one handle → 100% cache hit after first lookup.
3. **Promotion adds nuance**: P2P moves data NVMe→GPU then needs GPU→host for DRAM cache. Bounce moves NVMe→host then host→GPU. Same total bandwidth. Ablation arm tests if this cancels the advantage.

## Current Status

- **Validated:** Build commands, baseline data (13764 us/obj bounce), sub-buffer approach works, single-handle reuse pattern confirmed
- **Uncertain:** Per-chunk NVMe→GPU BAR1 throughput vs NVMe→host DRAM at 128 KiB granularity
- **Suggested next:**
  - If P2P-pinned faster: Test pipelined P2P (BatchSubmit + pre-pinned)
  - If P2P-pinned matches bounce: Promotion cost exactly cancels path advantage; defer P2P for v0
  - If P2P-pinned still slower: BAR1 writes inherently slower per 128 KiB chunk; P2P only wins with batched/large transfers

## Warnings & Constraints

- **SPDK singleton**: Kill certus-server before restarting (`pkill certus-server; sleep 2`)
- **Lock file cleanup**: `rm -f /var/tmp/spdk_pci_lock_*` before starting
- **nvidia_peermem + gdrdrv required**: `lsmod | grep nvidia_peermem && lsmod | grep gdrdrv`
- **Only use 0000:63:00.0** (device 0000:62:00.0 has VFIO issues)
- **DmaBuffer sub-views use noop_free**: Must NOT be dropped after parent buffer
- **P2P buffer cache cleared on shutdown**: Drop cached GPU DmaBuffers before SPDK env destroyed
- **IpcHandle field addition**: Update all construction sites including test code
- **prepare_memory_for_spdk payload**: 72 bytes = 64-byte handle + 8-byte LE u64 size, base64-encoded
- **Same session for comparison**: Run conditions back-to-back without rebooting
