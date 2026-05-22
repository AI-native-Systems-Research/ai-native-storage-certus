# Handoff — h8-v0-vs-p2p, Iteration 1

## Goal

Implement a P2P read path in dispatcher v0 (components/dispatcher/v0/) that reads NVMe chunks directly into GPU BAR1 memory via `prepare_memory_for_spdk`, then benchmark it against the existing bounce path — all through certus-server with `--dispatcher-version v0` and the Python client's `--bench` mode.

## Key Discoveries

1. **Dispatcher v0's bounce path** (`read_from_block_device` at lib.rs:179-276): allocates one contiguous host DMA buffer (line 213), reads 32 x 128 KiB chunks sequentially via ReadSync (lines 218-263), then does a single `gpu.dma_copy_to_device` (line 266-273). This is the code to bypass for P2P.

2. **`IpcHandle` struct** (interfaces/src/idispatcher.rs:113-118) currently only carries `address: *mut u8` and `size: u32`. For P2P, the dispatcher needs the raw 64-byte CUDA IPC handle bytes to call `prepare_memory_for_spdk`. Add a `cuda_ipc_handle_bytes: Option<Vec<u8>>` field.

3. **certus-server opens CUDA IPC in service.rs:65-93** (`open_cuda_ipc`). For P2P lookups, the server should NOT open the handle (let the dispatcher do it via `prepare_memory_for_spdk`). But for the control condition (bounce), the existing flow must remain unchanged.

4. **`prepare_memory_for_spdk` payload format**: base64 encoding of 72 bytes = 64-byte `cudaIpcMemHandle_t.reserved` + 8-byte little-endian u64 size. Implementation at `gpu-services/v0/src/lib.rs:330-479`. It opens the IPC handle, pins/verifies, calls `dma::create_spdk_dma_buffer_from_gpu(ptr, size, was_already_pinned)`.

5. **Sequential ReadSync pattern**: Each chunk is read via `Command::ReadSync { ns_id: 1, lba, buf }` → wait `Completion::ReadDone`. The P2P path must use the same pattern but target the GPU-backed DmaBuffer. Challenge: ReadSync takes an `Arc<Mutex<DmaBuffer>>` — the GPU buffer from `prepare_memory_for_spdk` needs to be wrapped appropriately or chunk-sized sub-buffers created.

6. **Prior experiment data (from h8-dispatcher-p2p)**: Bounce 2206 MB/s, P2P-warm 3670 MB/s (1.66x). These numbers were from the standalone `gpu-p2p-server` using BatchSubmit. Sequential submission narrows the gap because NVMe can't overlap reads. The question is whether the eliminated cudaMemcpy still provides net benefit.

7. **Hardware**: Device 0000:63:00.0 confirmed working. Device 0000:62:00.0 has VFIO group issues (avoid). Both A30 GPUs and NVMe on NUMA 0.

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
- **Output format:** Stdout table with columns: Tier, Avg (us/obj), Min (us/obj), Max (us/obj), Avg (GB/s), Peak (GB/s). Parse "SSD-tier" row.
- **Baseline result:** Not yet measured through certus-server (requires hardware). Prior p2p-server data: bounce ~2206 MB/s.

## Code Map

| File:Line | What's there | When to look |
|-----------|-------------|--------------|
| `components/dispatcher/v0/src/lib.rs:179-276` | `read_from_block_device` — bounce path (sequential ReadSync + dma_copy_to_device) | Primary target for P2P alternative |
| `components/dispatcher/v0/src/lib.rs:694-747` | `IDispatcher::lookup` — dispatches to staging vs block-device path | Where to add P2P routing logic |
| `components/dispatcher/v0/src/lib.rs:218-263` | Sequential ReadSync loop (per-chunk) | Pattern to replicate for P2P reads |
| `components/dispatcher/v0/src/lib.rs:266-273` | `gpu.dma_copy_to_device` — the call P2P eliminates | Verify this is skipped in P2P |
| `components/interfaces/src/idispatcher.rs:113-118` | `IpcHandle` struct | Add `cuda_ipc_handle_bytes` field |
| `apps/certus-server/src/service.rs:176-253` | Lookup gRPC handler | Pass raw handle bytes through |
| `apps/certus-server/src/service.rs:65-93` | `open_cuda_ipc` | For P2P, skip this; let dispatcher handle |
| `components/gpu-services/v0/src/lib.rs:330-479` | `prepare_memory_for_spdk` impl | Understand what it does with the payload |
| `components/gpu-services/v0/src/dma.rs:114-162` | `create_spdk_dma_buffer_from_gpu` | How GPU ptr becomes SPDK-registered DmaBuffer |
| `components/dispatcher/v0/src/io_segmenter.rs:15-55` | `segment_io` | Generates chunk offsets (128 KiB segments) |
| `apps/certus-server/python-client/test_client.py:310-439` | `bench_lookup_latency` | Full benchmark flow |

## Code Targets

### 1. IpcHandle extension (interfaces/src/idispatcher.rs:113-118)
Add `pub cuda_ipc_handle_bytes: Option<Vec<u8>>` field. Update all construction sites to include the new field (default `None` for existing code). This is the minimal interface change.

### 2. Service.rs lookup handler (apps/certus-server/src/service.rs:176-253)
For P2P condition: pass `cuda_ipc_handle_bytes: Some(handle.cuda_ipc_handle.clone())` in the IpcHandle. For bounce condition (control): continue calling `open_cuda_ipc` and set `cuda_ipc_handle_bytes: None`.

Decision: To A/B test without a flag, implement P2P by detecting `cuda_ipc_handle_bytes.is_some()` in the dispatcher. The server always passes the bytes; the dispatcher chooses P2P when available. For the bounce control, patch the server to NOT pass the bytes.

### 3. Dispatcher P2P read method (components/dispatcher/v0/src/lib.rs, after line 276)
New method `read_from_block_device_p2p`:
- Construct 72-byte payload: `cuda_ipc_handle_bytes (64) + size.to_le_bytes() (8)`
- Base64-encode → call `gpu.prepare_memory_for_spdk(payload, None)`
- Get DmaBuffer backed by GPU BAR1 (size = `ipc_handle.size`)
- Run same `segment_io` + sequential ReadSync loop, but reading each chunk into the GPU buffer at offset
- Return Ok(()) — no dma_copy_to_device needed

Challenge: ReadSync takes `Arc<Mutex<DmaBuffer>>`. The GPU DmaBuffer from prepare_memory_for_spdk is a single allocation. For chunk reads, create per-chunk DmaBuffers by offsetting into the GPU buffer, OR read into temp SPDK host buffers and copy to GPU buffer (which defeats the purpose). Best approach: issue ReadSync targeting the GPU DmaBuffer directly — SPDK can DMA to any registered address at any offset. Need to check if DmaBuffer supports sub-range access for ReadSync.

Alternative approach: Since `Command::ReadSync { buf: Arc<Mutex<DmaBuffer>> }` expects a buffer per read, and SPDK reads into it at offset 0, we need per-chunk GPU DmaBuffers. But `prepare_memory_for_spdk` gives one large buffer. Solution: call `prepare_memory_for_spdk` once for the full 4 MiB, then construct per-chunk `DmaBuffer` wrappers (unsafe from_raw at offset) OR allocate 32 x 128 KiB GPU DmaBuffers. The latter is cleaner but slower (32 x spdk_mem_register). Best: allocate one large GPU DmaBuffer, then for each ReadSync, create a temporary view/slice — OR read into host SPDK buffer then copy to GPU offset (hybrid approach that still avoids the final large cudaMemcpy).

**Recommended approach**: Read each chunk into a per-segment host DMA buffer (same as current code), then immediately copy each chunk to the GPU DMA buffer at the correct offset. This avoids the final large `dma_copy_to_device` by doing incremental copies. Wait — this is still a bounce. 

**Correct P2P approach**: The NVMe controller must DMA directly into GPU memory. For this, the ReadSync buffer must BE the GPU memory. Since SPDK ReadSync needs the target buffer passed as `Arc<Mutex<DmaBuffer>>`, and the GPU DmaBuffer IS a DmaBuffer (just backed by GPU BAR1 via spdk_mem_register), we can pass the full GPU buffer for each read at offset. BUT ReadSync reads into buffer offset 0 always. So we DO need per-chunk GPU DmaBuffers (each 128 KiB, each at the right GPU offset).

**Final approach**: Call `prepare_memory_for_spdk` to get the full GPU DmaBuffer (4 MiB). Then for each chunk, create a sub-DmaBuffer using unsafe `DmaBuffer::from_raw(gpu_buf.as_ptr() + chunk_offset, 128KiB, noop_free, -1)` — a non-owning view. Use noop free function since the parent buffer owns the memory. Pass this sub-buffer as the ReadSync target. SPDK will DMA each 128 KiB chunk directly to the GPU BAR1 address at the correct offset.

### 4. Cargo.toml update (components/dispatcher/v0/Cargo.toml)
Add `base64 = "0.22"` to dependencies.

## What I Tried That Didn't Work

- **Attempting to use Explore agents**: Model authentication errors forced direct tool usage.

## What I Excluded and Why

- **BatchSubmit pipelining**: The research question specifically asks about sequential ReadSync. BatchSubmit is a separate dimension explored in h8-dispatcher-p2p. We isolate the path effect here.
- **Modifying gpu-p2p-server**: Campaign constraint explicitly forbids this.
- **Standalone benchmark binaries**: Campaign constraint requires all benchmarks through certus-server.
- **Condition C (per-chunk DmaBuffer allocation)**: Deferred. The primary comparison (A vs B) answers the research question. If P2P wins, a follow-up can test whether the sub-buffer approach or per-chunk allocation is better.
- **Larger object sizes**: 4 MiB is specified. Robustness arm tests 1 MiB to establish crossover behavior.

## Evolution of Thinking

1. **Initial assumption**: P2P would be straightforward — just swap the buffer target. Reality: the `IpcHandle` struct doesn't carry raw CUDA handle bytes, so `prepare_memory_for_spdk` can't be called without interface changes.

2. **ReadSync buffer constraint**: ReadSync writes to offset 0 of its buffer. A single 4 MiB GPU DmaBuffer can't be reused across reads without sub-buffer views. The solution is to create non-owning DmaBuffer sub-views at chunk offsets — unsafe but sound because the parent buffer's lifetime encompasses all reads.

3. **Open vs prepare dilemma**: certus-server currently opens CUDA IPC handles before passing to the dispatcher. For P2P, the dispatcher needs the raw handle to call `prepare_memory_for_spdk` (which opens it internally). This means for P2P lookups, the server should NOT open the handle first — it passes raw bytes and lets the dispatcher handle everything.

## Current Status

- **Validated:** Build commands, code paths understood, interface identified, prior experiment data available as reference
- **Uncertain:** Whether per-chunk sub-buffer DmaBuffer views work correctly with SPDK DMA (the NVMe controller must target the correct GPU BAR1 physical address for each chunk offset). Need to verify that `DmaBuffer::from_raw(ptr+offset, ...)` produces correct vtophys results.
- **Suggested next:**
  - If P2P is faster: Measure the cold-start penalty (first call to prepare_memory_for_spdk) vs amortized cost. Consider caching GPU DmaBuffers across lookups.
  - If P2P is slower: The sub-buffer vtophys resolution may fail (returns contiguous range only from registration base). Try allocating 32 separate 128 KiB GPU DmaBuffers instead.
  - If similar: The cudaMemcpy eliminated by P2P is not the bottleneck — the sequential NVMe reads dominate. Focus on BatchSubmit for next iteration.

## Warnings & Constraints

- **SPDK singleton**: Only one SPDK process per NVMe device. Kill certus-server before restarting.
- **Lock file cleanup**: Always `rm -f /var/tmp/spdk_pci_lock_*` before starting.
- **nvidia-peermem required**: `lsmod | grep nvidia_peermem` must show loaded. Without it, `spdk_mem_register` on GPU memory returns error.
- **gdrdrv required for P2P feature**: `lsmod | grep gdrdrv`. The `gpu-services` P2P code path uses GDRCopy for BAR1 mapping.
- **IpcHandle is `unsafe impl Send`**: Adding `Vec<u8>` field doesn't affect Send/Sync. But verify the test code compiles with the new field.
- **DmaBuffer sub-views (from_raw at offset) must NOT be dropped before parent**: Use a noop free function (`unsafe extern "C" fn noop_free(_: *mut c_void) {}`) for sub-views. The parent DmaBuffer (from prepare_memory_for_spdk) handles SPDK unregistration on drop.
- **Same session for valid comparison**: Run bounce and P2P conditions back-to-back without restarting NVMe. Ratios are more reliable than absolutes.
- **Python client bench flow**: Populates objects → waits 3s for write-through → measures lookups. For v0 dispatcher, all objects go staging→SSD. The "SSD-tier" measurement is what we want.
- **Debug build OK**: PCIe DMA latency dominates. Release build only helps CPU-bound command preparation (~ns vs ~us for DMA).
