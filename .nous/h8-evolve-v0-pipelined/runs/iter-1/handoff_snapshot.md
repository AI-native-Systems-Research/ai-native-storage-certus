# Handoff — h8-evolve-v0-pipelined Iteration 1

## Goal

Implement true pipelining in dispatcher v0's `read_from_block_device` (overlap NVMe reads with GPU copies via `cudaMemcpyAsync` + double-buffered `cudaHostAlloc` DMA buffers), and compare against P2P direct DMA — all benchmarked through certus-server with `--dispatcher-version v0`.

## Key Discoveries

1. **`read_from_block_device` sequential pattern** (lib.rs:179-276): allocates one contiguous host DMA buffer, reads 32 × 128 KiB chunks via ReadSync (lines 218-263, each blocking on `completion_rx.recv()`), then does one `gpu.dma_copy_to_device` (lines 266-273). Zero overlap between NVMe and GPU.

2. **`dma_copy_to_device` is synchronous `cudaMemcpy`** (gpu-services/v0/src/lib.rs:519-526). For pipelining, need `cudaMemcpyAsync` — NOT present in current `cuda_ffi.rs` (lines 71-111). Must add: `cudaStream_t`, `cudaStreamCreate`, `cudaStreamSynchronize`, `cudaMemcpyAsync`.

3. **`create_spdk_dma_buffer_from_cuda_host_alloc` exists** (dma.rs:253-283). Takes a `cudaHostAlloc` pointer, calls `spdk_mem_register`, returns DmaBuffer. This gives a buffer that's both CUDA-pinned (for true async) and SPDK-registered (for NVMe DMA). This is the proven path from h8-pipelined RP-4.

4. **ReadSync uses `Arc<Mutex<DmaBuffer>>`** (interfaces/src/iblock_device.rs:187-194). The pipelined path needs 2 such buffers for double-buffering. ReadAsync also exists (line 205-214) and adds a timeout — either works.

5. **Single `connect_client()` supports multiple sequential ReadSync/ReadAsync sends.** Channel capacity is 64 (block-device-spdk-nvme lib.rs:67). 32 chunks fits easily. Per-chunk connect was 13-17μs × 32 = 544μs overhead (h8-evolve-v0 iter-1 finding).

6. **certus-server opens CUDA IPC handle in service.rs:65-93** (`open_cuda_ipc`). For P2P, the dispatcher needs the raw 64-byte handle bytes to call `prepare_memory_for_spdk`. The `IpcHandle` struct (interfaces/src/idispatcher.rs) needs a `cuda_ipc_handle_bytes: Option<Vec<u8>>` field.

7. **NVMe `0000:63:00.0` is NODE-level to GPU0** (same NUMA 0). Prior P2P at SYS level (c2:00.0) was 65% slower. NODE-level should avoid that penalty.

## System Interface

- **Build:**
  ```bash
  RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
  ```
  Validated: exits 0 (0.16s, already compiled).

- **Run server (baseline):**
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
    --bench-only \
    --bench-object-size 4194304 \
    --bench-num-objects 1 \
    --bench-iterations 1
  ```

- **Output format:** Stdout table. Parse "SSD-tier" row: Avg (us/obj), Avg (GB/s).
- **Baseline result:** Not yet measured through certus-server for this campaign. Prior data: ~21,247 us/obj for 4 MiB at this NVMe (h8-v1-pinned iter-2, dispatcher v1, different test params). Standalone gpu-p2p-server: sequential bounce 1440 MB/s.

## Code Map

| Location | What | When to look |
|----------|------|-------------|
| `components/dispatcher/v0/src/lib.rs:179-276` | `read_from_block_device` — sequential read + single GPU copy | Primary target for pipelining |
| `components/dispatcher/v0/src/lib.rs:218-263` | ReadSync loop per chunk | Replace with ReadAsync + double-buffer |
| `components/dispatcher/v0/src/lib.rs:266-273` | `gpu.dma_copy_to_device` (final sync copy) | Remove — replaced by per-chunk async copies |
| `components/dispatcher/v0/src/lib.rs:694-747` | `IDispatcher::lookup` — routes to read path | Where to add P2P routing (check cuda_ipc_handle_bytes) |
| `components/dispatcher/v0/src/lib.rs:200-207` | `connect_client()` call and channel setup | Use single connection for all chunks |
| `components/dispatcher/v0/src/io_segmenter.rs:22-55` | `segment_io` | Generates chunk LBAs and offsets |
| `components/gpu-services/v0/src/cuda_ffi.rs:71-111` | CUDA FFI declarations | Add async symbols here |
| `components/gpu-services/v0/src/dma.rs:253-283` | `create_spdk_dma_buffer_from_cuda_host_alloc` | Creates dual CUDA+SPDK buffer |
| `components/gpu-services/v0/src/lib.rs:489-537` | `dma_copy_to_device` sync impl | Reference for how cudaMemcpy is called |
| `components/interfaces/src/idispatcher.rs:112-118` | `IpcHandle` struct | Add `cuda_ipc_handle_bytes` field for P2P |
| `components/interfaces/src/iblock_device.rs:187-194` | `Command::ReadSync` | Takes `Arc<Mutex<DmaBuffer>>` |
| `components/interfaces/src/iblock_device.rs:205-214` | `Command::ReadAsync` | Alternative with timeout |
| `components/interfaces/src/spdk_types.rs:293-316` | `DmaBuffer::from_raw` | For P2P sub-buffer views |
| `apps/certus-server/src/service.rs:176-254` | Lookup gRPC handler | Pass cuda_ipc_handle_bytes for P2P |
| `apps/certus-server/src/service.rs:65-93` | `open_cuda_ipc` | Opens CUDA IPC handle → device ptr |
| `apps/certus-server/python-client/test_client.py:310-439` | `bench_lookup_latency` | Full benchmark cycle |

## Code Targets

### h-main: Pipelined read path

**File:** `components/gpu-services/v0/src/cuda_ffi.rs` (line 71, inside extern "C" block)
- Add: `pub type cudaStream_t = *mut c_void;`
- Add: `pub fn cudaStreamCreate(p_stream: *mut cudaStream_t) -> cudaError_t;`
- Add: `pub fn cudaStreamDestroy(stream: cudaStream_t) -> cudaError_t;`
- Add: `pub fn cudaStreamSynchronize(stream: cudaStream_t) -> cudaError_t;`
- Add: `pub fn cudaMemcpyAsync(dst: *mut c_void, src: *const c_void, count: usize, kind: c_int, stream: cudaStream_t) -> cudaError_t;`
- **Why here:** cuda_ffi.rs is the single-point FFI module for CUDA. All symbols exist in libcudart.so already linked.

**File:** `components/dispatcher/v0/src/lib.rs` (replace lines 179-276)
- Add component fields: `pipeline_bufs: Mutex<Option<PipelineState>>` where PipelineState holds 2 DMA buffers + 1 CUDA stream
- Lazy-init in `read_from_block_device` on first call: `cudaHostAlloc` 2 × 131072 bytes, wrap each via `create_spdk_dma_buffer_from_cuda_host_alloc`, `cudaStreamCreate`
- Pipeline algorithm:
  ```
  connect_client() once → channels
  submit ReadAsync chunk[0] into buf[0]
  for i in 1..num_chunks:
      recv completion
      cudaMemcpyAsync(gpu_ptr + (i-1)*chunk_size, buf[current].as_ptr(), chunk_size, H2D, stream)
      submit ReadAsync chunk[i] into buf[1-current]
      current = 1 - current
  recv last completion
  cudaMemcpyAsync(gpu_ptr + (num_chunks-1)*chunk_size, buf[current].as_ptr(), last_chunk_size, H2D, stream)
  cudaStreamSynchronize(stream)
  ```
- **Why:** ReadAsync + double-buffer + cudaMemcpyAsync achieves hardware-level parallelism between NVMe DMA and GPU copy engine.

**File:** `components/dispatcher/v0/Cargo.toml`
- Add `gpu-services` dependency (for cuda_ffi and dma module access)
- **Why:** Dispatcher needs to call `create_spdk_dma_buffer_from_cuda_host_alloc` and CUDA FFI directly.

### h-robustness: P2P direct DMA path

**File:** `components/interfaces/src/idispatcher.rs` (IpcHandle struct, ~line 113)
- Add: `pub cuda_ipc_handle_bytes: Option<Vec<u8>>`
- Update all IpcHandle construction sites (service.rs builds IpcHandle)

**File:** `apps/certus-server/src/service.rs` (lookup handler, lines 176-254)
- Set `cuda_ipc_handle_bytes: Some(handle.cuda_ipc_handle.clone())` when building IpcHandle

**File:** `components/dispatcher/v0/src/lib.rs` (after read_from_block_device, ~line 276)
- New method `read_from_block_device_p2p`:
  1. Build 72-byte payload: `cuda_ipc_handle_bytes[0..64] ++ total_size.to_le_bytes()`
  2. Base64 encode → `gpu.prepare_memory_for_spdk(encoded, None)` → GPU DmaBuffer
  3. For each segment: `DmaBuffer::from_raw(gpu_buf.as_ptr() + seg.buffer_offset, seg.length, noop_free, -1)` → Arc<Mutex<>> → ReadSync
  4. Return Ok(())
- In `lookup()` (line 731): if `ipc_handle.cuda_ipc_handle_bytes.is_some()`, call P2P path

**File:** `components/dispatcher/v0/Cargo.toml`
- Add `base64 = "0.22"` dependency

## What I Tried That Didn't Work

- Agent-based exploration failed (model auth errors). Used direct tool calls.
- No runtime probes possible (requires hardware + SPDK ownership).

## What I Excluded and Why

- **Components/dispatcher/v1/**: Campaign constraint explicitly forbids referencing v1. Design is independent.
- **gpu-p2p-server modifications**: Campaign constraint forbids.
- **Standalone benchmark binaries**: Campaign constraint forbids.
- **BatchSubmit pipelining (submit all 32 at once)**: Requires 32 DMA buffers allocated simultaneously. Double-buffer approach uses only 2 buffers and achieves equivalent overlap. Separate concern.
- **Multi-stream CUDA variants**: h8-pipelined iter-1 proved 1-stream vs 2-stream has 0.6% difference. GPU copy engine is not the bottleneck.
- **Larger chunk sizes**: MDTS constraint limits to 128 KiB. Campaign spec fixes this.
- **Scoped-thread approach for overlap**: cudaMemcpyAsync provides hardware-level parallelism without thread complexity. The async call returns immediately; the copy engine runs independently of the CPU thread that then blocks on ReadSync recv.

## Evolution of Thinking

1. Initially considered scoped threads for overlap (read on main thread, copy on helper). But h8-pipelined proved `cudaMemcpyAsync` achieves hardware-level parallelism without thread complexity — the CUDA copy engine operates independently once the async call returns.

2. Key insight: the overlap doesn't need threading because ReadSync blocks on `completion_rx.recv()` (CPU waits for NVMe), while `cudaMemcpyAsync` has already returned immediately and the copy engine is processing. By the time the NVMe read completes, the previous chunk's GPU copy has likely also completed (128 KiB at 20 GB/s = ~6μs, vs NVMe read ~3-4μs per chunk).

3. Critical learning from h8-pipelined: `cudaHostRegister` on SPDK hugepages does NOT enable true async. Must use `cudaHostAlloc` memory wrapped via `create_spdk_dma_buffer_from_cuda_host_alloc`.

4. Critical learning from h8-evolve-v0: per-chunk `connect_client()` was the dominant overhead (544μs). Must connect once and reuse the channel for all reads.

## Current Status

- **Validated:** Build works. Code paths mapped. Key APIs identified. Prior experiment principles inform design.
- **Uncertain:** (1) Whether `create_spdk_dma_buffer_from_cuda_host_alloc` works within the dispatcher crate context (may need dependency wiring). (2) Whether DmaBuffer sub-views via `from_raw` at GPU offset produce correct SPDK vtophys results for P2P. (3) Exact per-chunk latencies through certus-server (gRPC overhead may dominate at small chunk counts).
- **Suggested next (iter-2):** If pipelining works but doesn't match P2P: the remaining gap is the extra PCIe hop. Consider increasing pipeline depth (triple-buffer, or submit 2 reads ahead). If P2P fails (vtophys issue): try 32 separate `prepare_memory_for_spdk` calls (one per 128 KiB chunk). If both work similarly: the NVMe sequential read is the true bottleneck — explore BatchSubmit integration for queue depth.

## Warnings & Constraints

- **SPDK singleton:** Only one process per NVMe device. Kill certus-server (Ctrl-C) and clean lock files before restart.
- **Lock file cleanup:** Always `rm -f /var/tmp/spdk_pci_lock_0000:63:00.0` before starting.
- **nvidia-peermem required for P2P:** `lsmod | grep nvidia_peermem` must show loaded.
- **gdrdrv required for P2P:** `lsmod | grep gdrdrv` must show loaded.
- **RUSTFLAGS required:** `-L /usr/local/lib` for libgdrapi.so linkage.
- **LD_LIBRARY_PATH:** Must include `/usr/local/lib:/usr/local/cuda/lib64` at runtime.
- **Python client pool assumption:** `bench_lookup_latency` assumes 256 MiB memory-tier pool. For v0 dispatcher (no memory-tier), all objects land in staging then get written to SSD by background writer. Wait at least 3s (built into client) for write-through before measuring SSD lookups.
- **Debug build OK:** PCIe DMA latency dominates. CPU-bound code (~ns) is negligible vs DMA (~μs).
- **Per-chunk NVMe read is blocking:** ReadSync blocks on `completion_rx.recv()`. The `cudaMemcpyAsync` launched before this recv executes on GPU copy engine independently. This is the overlap mechanism — no threads needed.
- **Buffer lifetime:** The double-buffer DmaBuffers must outlive all ReadSync operations. Store in component field (Mutex<Option<PipelineState>>), lazily initialized, dropped only on shutdown.
- **IpcHandle is not Send by default** (contains raw pointer). It has `unsafe impl Send` — adding `Option<Vec<u8>>` doesn't affect this.
