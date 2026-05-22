# Problem Framing — Dispatcher v0 True Pipelining vs P2P

## Research Question

Can evolving dispatcher v0's `read_from_block_device` to overlap NVMe reads with GPU copies (true pipelining via `cudaMemcpyAsync` + double-buffered `cudaHostAlloc` DMA buffers) match or exceed direct P2P DMA throughput for 4 MiB lookups (32 × 128 KiB chunks) through certus-server?

**Key mechanism under test:** `read_from_block_device` at `components/dispatcher/v0/src/lib.rs:179-276`:
1. Allocates one contiguous host DMA buffer — line 213
2. Reads 32 × 128 KiB chunks sequentially via `Command::ReadSync` — lines 218-263
3. Performs a single synchronous `cudaMemcpy` H2D — line 266-273 via `gpu.dma_copy_to_device`

The pipelined alternative overlaps stages: while chunk[N] copies to GPU asynchronously via `cudaMemcpyAsync`, chunk[N+1] reads from NVMe concurrently on the same thread (ReadSync blocks but the async GPU copy proceeds on the copy engine independently).

**Code implementing the mechanism:**
- `components/dispatcher/v0/src/lib.rs:179-276` — current sequential read path
- `components/dispatcher/v0/src/lib.rs:218-263` — ReadSync loop (per-chunk, blocks on completion_rx.recv())
- `components/dispatcher/v0/src/lib.rs:266-273` — `gpu.dma_copy_to_device` (synchronous cudaMemcpy)
- `components/dispatcher/v0/src/io_segmenter.rs:22-55` — `segment_io` generates 128 KiB segments
- `components/gpu-services/v0/src/lib.rs:489-537` — `dma_copy_to_device` uses `cudaMemcpy` (sync)
- `components/gpu-services/v0/src/cuda_ffi.rs:71-111` — current FFI (lacks `cudaMemcpyAsync`, `cudaStream_t`)
- `components/gpu-services/v0/src/dma.rs:253-283` — `create_spdk_dma_buffer_from_cuda_host_alloc` wraps pinned memory as SPDK DmaBuffer

## System Interface

### Build command
```bash
RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
```
Code evidence: `apps/certus-server/Cargo.toml` specifies dependencies; `RUSTFLAGS` needed for `libgdrapi.so` at `/usr/local/lib`. Build validated: exits 0.

### CLI flags (code evidence)

| Flag | Semantics | Source |
|------|-----------|--------|
| `--dispatcher-version v0` | Selects dispatcher v0 (staging-based) | `apps/certus-server/src/main.rs:49` |
| `--metadata-pci DDDD:BB:DD.F` | Metadata NVMe PCI address | `apps/certus-server/src/main.rs:29` |
| `--data-pci DDDD:BB:DD.F` | Data NVMe PCI address(es) | `apps/certus-server/src/main.rs:33` |
| `--listen ADDR` | gRPC listen address | `apps/certus-server/src/main.rs:37` |
| `--bench-only` (client) | Skip functional tests, benchmark only | `apps/certus-server/python-client/test_client.py:455` |
| `--bench-object-size N` | Object size in bytes | `apps/certus-server/python-client/test_client.py:457` |
| `--bench-num-objects N` | Objects per tier to benchmark | `apps/certus-server/python-client/test_client.py:461` |
| `--bench-iterations N` | Lookup iterations | `apps/certus-server/python-client/test_client.py:465` |

### Output format
Python client stdout table: `Tier | Avg (us/obj) | Min (us/obj) | Max (us/obj) | Avg (GB/s) | Peak (GB/s)`.
Parse "SSD-tier" row. For v0 dispatcher (no memory-tier), all data goes staging→SSD.
Source: `apps/certus-server/python-client/test_client.py:425-428`

## Baseline Command

```bash
# Terminal 1 — Start server
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/certus-server \
  --metadata-pci 0000:63:00.0 \
  --data-pci 0000:63:00.0 \
  --dispatcher-version v0 \
  --listen 0.0.0.0:50051

# Terminal 2 — Run benchmark (1 iteration, first-hit, 4 MiB)
cd apps/certus-server/python-client && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
python3 test_client.py \
  --server localhost:50051 \
  --bench-only \
  --bench-object-size 4194304 \
  --bench-num-objects 1 \
  --bench-iterations 1
```

## Baseline Validation

Build verified: `RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server` exits 0 (0.16s, already compiled). Hardware execution requires SPDK + NVMe + GPU.

Prior experiment data from this hardware:
- h8-v1-pinned iter-2 with `63:00.0` (NODE-level NVMe): bounce SSD-tier ~21,247 us/obj for 4 MiB through certus-server (different object count and params)
- h8-pipelined iter-2: `cudaHostAlloc` + SPDK pipeline achieved 2.4-3x faster than sequential bounce
- h8-evolve-v0 iter-2 (standalone gpu-p2p-server): sequential bounce 1440 MB/s, pipelined-with-channel-reuse projected >2000 MB/s, P2P warm 3082 MB/s

## Experimental Conditions

### Condition A: Baseline Sequential (existing v0)
No code changes. Run baseline command above. Measures `read_from_block_device` at lib.rs:179-276: sequential ReadSync + single `dma_copy_to_device`.

### Condition B: True Pipelined v0 (double-buffer + cudaMemcpyAsync)
Modify `read_from_block_device` in `components/dispatcher/v0/src/lib.rs`:

**Intent:**
1. Add CUDA async FFI declarations (`cudaStream_t`, `cudaStreamCreate`, `cudaStreamSynchronize`, `cudaMemcpyAsync`) to `components/gpu-services/v0/src/cuda_ffi.rs`
2. Pre-allocate 2 × 128 KiB pinned host buffers via `cudaHostAlloc` + `create_spdk_dma_buffer_from_cuda_host_alloc` (these are both CUDA-pinned and SPDK-registered)
3. Create one CUDA stream via `cudaStreamCreate`
4. Use a single `connect_client()` call (not per-chunk) with `ReadAsync` commands
5. Double-buffer pipeline: submit ReadAsync chunk[0] into buf_a → for each subsequent chunk: recv completion, launch `cudaMemcpyAsync` from completed buf to correct GPU offset on stream, submit ReadAsync for next chunk into other buf → after last: recv + cudaMemcpyAsync + `cudaStreamSynchronize`

**Why `cudaHostAlloc` not SPDK hugepage buffers:** Prior h8-pipelined iter-1 proved that `cudaHostRegister` on SPDK-allocated hugepage buffers causes CUDA to fall back to synchronous copy (RP-4). True async requires memory born from `cudaHostAlloc`. The `create_spdk_dma_buffer_from_cuda_host_alloc` function (dma.rs:253) handles the dual registration.

**Why single `connect_client()`:** Prior h8-evolve-v0 iter-1 showed per-chunk `connect_client()` adds 13-17μs × 32 = 544μs overhead. Channel reuse eliminates this.

### Condition C: P2P Direct DMA (NVMe → GPU BAR1)
Add a P2P read path eliminating host bounce entirely:

**Intent:**
1. Add `cuda_ipc_handle_bytes: Option<Vec<u8>>` field to `IpcHandle` struct in `components/interfaces/src/idispatcher.rs`
2. Modify `apps/certus-server/src/service.rs` lookup handler to pass raw CUDA IPC bytes
3. In dispatcher, construct 72-byte payload (64-byte handle + 8-byte LE size), base64 encode, call `gpu.prepare_memory_for_spdk()` to get GPU-backed DmaBuffer
4. Create per-chunk non-owning DmaBuffer sub-views via `DmaBuffer::from_raw(ptr+offset, 131072, noop_free, -1)`
5. Issue ReadSync into each sub-view (NVMe DMAs directly to GPU BAR1)
6. No `dma_copy_to_device` needed

## Success Criteria

- **Direction (primary):** Pipelined v0 (B) achieves consistently lower SSD-tier Avg (us/obj) than sequential v0 (A).
- **Mechanism validation:** The latency reduction should approximately equal the time saved by hiding the GPU copy behind NVMe reads. With 4 MiB sync copy taking ~200μs and 32 per-chunk async copies at ~6μs each fully hidden, expected savings ≈ 200μs - 6μs = ~194μs.
- **P2P comparison:** Determines whether pipelining (B) approaches P2P (C) performance, or whether the extra PCIe hop (host bounce) remains the dominant bottleneck.

## Constraints

- All benchmarks through certus-server with `--dispatcher-version v0`
- No standalone benchmark binaries
- No modification to gpu-p2p-server
- Implementation changes in `components/dispatcher/v0/` (plus minimal interface/FFI extensions)
- Use `--bench-iterations 1` for fair first-hit comparison
- Restart server between conditions (fresh process, fresh populate)
- NVMe device: 0000:63:00.0 (NUMA 0, NODE-level to GPU0). Avoid 0000:62:00.0 (VFIO issues)
- `nvidia_peermem` + `gdrdrv` kernel modules required for P2P condition
- Always `rm -f /var/tmp/spdk_pci_lock_*` before starting server

## Prior Knowledge

No active principles from this campaign (first iteration). Related findings:
- **RP-4 (high confidence):** CUDA async requires `cudaHostAlloc`-born memory. `cudaHostRegister` on SPDK hugepages falls back to sync.
- **h8-pipelined iter-2:** `cudaHostAlloc` + SPDK pipeline achieved 2.4-3x improvement over non-pipelined bounce.
- **h8-evolve-v0 iter-2:** Per-chunk `connect_client()` was 13-17μs overhead. Channel reuse eliminates it.
- **h8-v0-vs-p2p:** P2P sub-buffer approach via `DmaBuffer::from_raw` at GPU offset. Uncertainty: vtophys correctness for offsets.
- **h8-v0-pinned:** P2P 65% slower with cross-NUMA PCIe topology. NODE-level (63:00.0) should be better.
