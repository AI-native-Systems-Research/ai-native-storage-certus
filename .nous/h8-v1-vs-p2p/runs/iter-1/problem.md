# Problem Framing — Dispatcher v1 Pipelined Bounce vs P2P Direct

## Research Question

Does the existing pipelined bounce-buffer path in dispatcher v1 (`pipelined_ssd_to_gpu` at `components/dispatcher/v1/src/pipeline.rs:30-123`) outperform a direct P2P SSD→GPU path for 4 MiB lookups broken into 32 sequential 128 KiB NVMe reads?

The bounce path:
1. Reads each 128 KiB chunk from NVMe into a ring of 4 host DMA buffers (line 50-58)
2. Copies each chunk from the host buffer to the memory-tier DRAM slot (lines 93-105)
3. Copies each chunk from the host buffer to the GPU via `dma_copy_to_device` (lines 109-117)

The P2P path would:
1. Call `IGpuServices::prepare_memory_for_spdk()` to get a GPU-backed DmaBuffer registered with SPDK
2. Read each 128 KiB chunk from NVMe directly into a sub-view of the GPU DmaBuffer (eliminating the host bounce + cudaMemcpy)
3. Still copy each chunk to memory-tier for caching (separate host copy needed)

Key tradeoff: P2P eliminates 32 x `cudaMemcpy(128KiB, H2D)` but introduces PCIe P2P DMA from NVMe to GPU (which may traverse the PCIe switch or root complex depending on topology). The memory-tier copy still requires reading from GPU BAR1 which is slow (uncacheable MMIO reads) — so P2P must copy from host ring to memory-tier like bounce does, meaning it only saves the GPU-directed copy.

**Revised mechanism**: P2P reads NVMe directly into GPU, then must still populate memory-tier. Options:
- (A) Read into GPU, then `dma_copy_to_host` from GPU to memory-tier — worse than bounce (adds GPU→host copy)
- (B) Read into GPU only, skip memory-tier entirely — breaks the promote flow
- (C) Dual-path: read into host ring buffer (same as bounce) for memory-tier, AND issue parallel read into GPU — doubles NVMe reads
- (D) **Read into host ring buffer as normal → copy to memory-tier → copy to GPU DMA buffer** — this IS the existing bounce pipeline

**Correct P2P approach**: The P2P path should be used for lookups where memory-tier is NOT populated (i.e., data lives on SSD and we want to serve it directly to GPU without promoting to memory-tier). This targets the `BlockDevice` branch of the lookup path. Instead of promote-to-DRAM-then-serve, we read directly into GPU memory and skip the memory-tier promotion step.

## System Interface

### Build Command

```bash
RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
```

- Code evidence: `apps/certus-server/Cargo.toml` pulls `dispatcher-v1`, which has `gpu-services` (with SPDK + GPU features) as a dependency.

### CLI Flags

| Flag | Semantics | Code evidence |
|------|-----------|---------------|
| `--dispatcher-version v1` | Selects dispatcher v1 (memory-tier based) | `apps/certus-server/src/main.rs:49` — `default_value = "v1"` |
| `--metadata-pci` | PCI address of metadata NVMe | `apps/certus-server/src/main.rs:29` |
| `--data-pci` | PCI address(es) of data NVMe | `apps/certus-server/src/main.rs:33` |
| `--listen` | gRPC listen address | `apps/certus-server/src/main.rs:37` — `default_value = "0.0.0.0:50051"` |

### Output Format

Python client benchmark outputs to stdout with columns:
```
Tier            Avg (us/obj)   Min (us/obj)   Max (us/obj)   Avg (GB/s)   Peak (GB/s)
Memory-tier     ...            ...            ...            ...          ...
SSD-tier        ...            ...            ...            ...          ...
```

The "SSD-tier" row measures the `BlockDevice` lookup path — the path under experiment.

## Baseline Command

**Server:**
```bash
rm -f /var/tmp/spdk_pci_lock_0000:63:00.0 && \
LD_LIBRARY_PATH=/usr/local/lib:/usr/local/cuda/lib64:$LD_LIBRARY_PATH \
./target/debug/certus-server \
  --metadata-pci 0000:63:00.0 \
  --data-pci 0000:63:00.0 \
  --dispatcher-version v1 \
  --listen 0.0.0.0:50051
```

**Client (baseline benchmark):**
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

## Baseline Validation

Not yet measured (requires SPDK hardware and NVMe at 0000:63:00.0). Prior measurement from h8-v0-vs-p2p with dispatcher v0: ~2206 MB/s bounce path. Dispatcher v1's pipelined approach (4-buffer ring overlapping reads with copies) is expected to be faster. The experiment will establish the v1 baseline first.

Reference from h8-dispatcher-p2p standalone experiments: bounce 2206 MB/s, P2P-warm 3670 MB/s (1.66x) — but those used BatchSubmit (parallel NVMe reads). Sequential submission narrows the gap.

## Experimental Conditions

### Condition A: Bounce Baseline (control)

Unmodified dispatcher v1 `promote_and_serve` → `pipelined_ssd_to_gpu`. This is the existing code path at `components/dispatcher/v1/src/lib.rs:244`.

Run server with `--dispatcher-version v1`, run client benchmark as above.

### Condition B: P2P Direct (h-main)

Implement a new `p2p_ssd_to_gpu` function in `components/dispatcher/v1/src/pipeline.rs` that:
1. Accepts the raw CUDA IPC handle bytes (from a new `cuda_ipc_handle_bytes` field on `IpcHandle`)
2. Constructs a base64 payload (64 bytes handle + 8 bytes LE size) and calls `gpu.prepare_memory_for_spdk(payload, None)` to get a GPU-backed DmaBuffer
3. For each 128 KiB segment, creates a non-owning sub-DmaBuffer at the GPU offset (using `DmaBuffer::from_raw(gpu_ptr + offset, chunk_size, noop_free, numa)`)
4. Issues `ReadSync` targeting the sub-buffer — NVMe DMA goes directly to GPU BAR1
5. After each chunk read, copies from the GPU-DmaBuffer sub-region to memory-tier (via `ptr::copy_nonoverlapping` from a host-accessible view of GPU memory — NOTE: this is uncacheable MMIO and will be slow)
6. Returns without calling `dma_copy_to_device`

**Refinement**: Step 5 is problematic — reading from GPU BAR1 (uncacheable) to copy to host memory-tier is extremely slow (~1 GB/s for uncached reads). A better P2P approach:
- Read each chunk into host ring buffer (like bounce does)
- Copy chunk to memory-tier (like bounce does)  
- But instead of `dma_copy_to_device` (cudaMemcpy H2D), issue the NVMe read ALSO into the GPU sub-buffer

This doubles NVMe reads. Not viable.

**Final P2P design**: Skip memory-tier promotion entirely for the P2P path. The new code path:
1. Does NOT call `mt.insert()` — no memory-tier promotion
2. Calls `prepare_memory_for_spdk` to get GPU DmaBuffer
3. Reads all chunks directly from NVMe into GPU sub-buffers
4. Returns — data is in GPU, not cached in memory-tier
5. Dispatch-map entry remains as `BlockDevice` (no transition to MemoryTier)

This tests the pure P2P path benefit: eliminates both the host ring allocation AND the cudaMemcpy, at the cost of not populating memory-tier (subsequent lookups for the same key still hit SSD). For the benchmark, each iteration reads from SSD anyway (cold path), so skipping memory-tier promotion doesn't affect measurement.

### Interface changes required:

1. **IpcHandle** (`components/interfaces/src/idispatcher.rs:113-118`): Add `pub cuda_ipc_handle_bytes: Option<Vec<u8>>` field
2. **service.rs** (`apps/certus-server/src/service.rs:233`): Pass `cuda_ipc_handle_bytes: Some(handle.cuda_ipc_handle.clone())` in the IpcHandle constructed for lookup
3. **dispatcher v1 promote_and_serve** (`components/dispatcher/v1/src/lib.rs:190-266`): When `ipc_handle.cuda_ipc_handle_bytes.is_some()`, call new `p2p_ssd_to_gpu` instead of memory-tier promotion + `pipelined_ssd_to_gpu`
4. **New function** in `components/dispatcher/v1/src/pipeline.rs`: `p2p_ssd_to_gpu`
5. **Cargo.toml**: Add `base64 = "0.22"` dependency

### Condition C: Bounce Control (h-control-negative)

Run the P2P code with `--bench-object-size 4096` (1 block = 1 chunk, no pipelining). At this size the single-chunk overhead dominates — the `prepare_memory_for_spdk` call (IPC open + pin + SPDK register) is amortized over only 1 block (4 KiB). The P2P advantage should vanish or reverse because setup cost exceeds the eliminated cudaMemcpy for tiny transfers.

## Success Criteria

- **h-main (P2P vs bounce)**: P2P direct path shows lower SSD-tier latency (us/obj) and higher throughput (GB/s) than the pipelined bounce baseline, consistently across all 20 iterations. Direction: P2P latency < bounce latency.
- **h-control-negative (4 KiB)**: P2P does NOT outperform bounce at 4 KiB objects (setup overhead dominates for small transfers).

## Constraints

- All benchmarks through certus-server with `--dispatcher-version v1`
- Do NOT modify gpu-p2p-server
- Do NOT create standalone benchmark binaries
- Python client at `apps/certus-server/python-client/` is the test harness
- P2P implementation in `components/dispatcher/v1/`
- Hardware: NVMe at 0000:63:00.0, GPU A30 on CUDA device 0, both NUMA 0
- nvidia-peermem and gdrdrv kernel modules must be loaded
- Debug build acceptable (PCIe DMA latency dominates)

## Prior Knowledge

This is the first iteration of h8-v1-vs-p2p. Prior experiment context from related campaigns:
- h8-dispatcher-p2p (standalone gpu-p2p-server): Bounce 2206 MB/s, P2P-warm 3670 MB/s (1.66x) — used BatchSubmit parallel reads
- h8-v0-vs-p2p (dispatcher v0): Designed but handoff only, same interface extension approach (IpcHandle + prepare_memory_for_spdk)
- P2P cold-start adds ~6ms overhead from first `prepare_memory_for_spdk` call (GDRCopy registration)
