# Problem Framing — h8-v1-pinned Iteration 1

## Research Question

Is direct SSD→GPU P2P DMA with persistently cached GPU staging buffers faster than the existing pipelined bounce-buffer path (SSD→DRAM→GPU) for 4 MiB object lookups broken into sequential 128 KiB NVMe reads?

The bounce path is implemented at `components/dispatcher/v1/src/pipeline.rs:30-123` (`pipelined_ssd_to_gpu`). It allocates a ring of 4 host-DRAM DMA buffers, reads each 128 KiB chunk via NVMe ReadSync into the ring, then for each chunk: (1) memcpy to memory-tier slot, (2) `dma_copy_to_device` to GPU. The P2P path eliminates both copies by reading NVMe directly into a GPU-registered DMA buffer.

The prior experiment (h8-v1-vs-p2p) showed P2P was 17.5% *slower* because `prepare_memory_for_spdk` was called per-lookup (~50μs: cudaIpcOpenMemHandle + spdk_mem_register). Caching the GPU DmaBuffer across lookups (keyed by 64-byte CUDA IPC handle) eliminates this overhead (RP-7).

## System Interface

### Build Command

```bash
RUSTFLAGS='-L /usr/local/lib' cargo build -p certus-server
```

**Code evidence:** Build requires `-L /usr/local/lib` for `libgdrapi.so` linkage (GDRCopy dependency of gpu-services). gpu-services Cargo.toml links `gdrapi` via `build.rs`.

### CLI Flags

- `--dispatcher-version v1` — selects dispatcher v1 component (`apps/certus-server/src/main.rs` via clap)
- `--metadata-pci 0000:63:00.0` — NVMe device for metadata extents
- `--data-pci 0000:63:00.0` — NVMe device for data extents (same device)
- `--listen 0.0.0.0:50051` — gRPC bind address

### Benchmark Client Flags

- `--bench-only` — skip unit tests, run only `bench_lookup_latency` (`test_client.py:310`)
- `--bench-object-size 4194304` — 4 MiB objects (= 32 × 128 KiB chunks)
- `--bench-num-objects 10` — number of cold/hot objects to measure
- `--bench-iterations N` — number of measurement iterations per tier

**Code evidence:** Benchmark at `apps/certus-server/python-client/test_client.py:310-439`. Pre-allocates single GPU tensor (`line 344`), reuses its IPC handle (`line 347`).

### Output Format

Stdout table: `Tier | Avg (us/obj) | Min (us/obj) | Max (us/obj) | Avg (GB/s) | Peak (GB/s)`. Parse the "SSD-tier" row for cold latency.

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

**Client (1 iteration — first-hit measurement):**
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

## Baseline Validation

From prior experiment (h8-v1-vs-p2p): bounce SSD-tier avg 12969.1 μs/obj, 0.32 GB/s (4 MiB, 10 objects, 20 iterations). This is the known baseline for the unmodified pipelined bounce-buffer path.

## Experimental Conditions

### Condition A: Baseline (pipelined bounce, 1 iteration)

Unmodified code. Measures cold SSD-tier latency on first hit only (before memory-tier warming). Both paths hit SSD on iteration 1, making this the fair comparison point.

Command: standard client with `--bench-iterations 1`.

### Condition B: P2P persistent staging (1 iteration)

Code changes implement `p2p_ssd_to_gpu_persistent` — reads NVMe chunks directly into a cached GPU DmaBuffer. On first lookup for a given IPC handle, `prepare_memory_for_spdk` is called once and the resulting DmaBuffer is cached in a `HashMap<[u8; 64], Arc<Mutex<DmaBuffer>>>`. Subsequent lookups with the same handle skip registration.

The P2P path activates when `IpcHandle.cuda_ipc_handle_bytes` is `Some(...)`. It does NOT promote to memory-tier (leaves dispatch-map as BlockDevice), ensuring every benchmark iteration re-tests the SSD→GPU path.

Code changes:
1. Add `cuda_ipc_handle_bytes: Option<Vec<u8>>` to `IpcHandle` struct
2. Add `gpu_dma_cache` field to dispatcher component
3. Add `get_or_create_gpu_dma()` cache lookup method
4. Add `p2p_ssd_to_gpu_persistent()` function in pipeline.rs
5. Add P2P routing branch in `promote_and_serve()`
6. Pass `cuda_ipc_handle_bytes` from service layer

Command: same client with `--bench-iterations 1` against modified server.

### Condition C: P2P persistent staging (20 iterations — stability)

Same code as Condition B, 20 iterations. Since P2P skips memory-tier, all 20 iterations hit SSD. Characterizes sustained P2P performance and variance. Note: not directly comparable to bounce-20-iter (which serves from DRAM after iter 1) per RP-6.

Command: same client with `--bench-iterations 20` against modified server.

## Success Criteria

1. **Primary (1-iteration):** P2P persistent (B) SSD-tier avg latency < baseline bounce (A) SSD-tier avg latency. This confirms eliminating 32× cudaMemcpy + 32× host memcpy produces a net win.

2. **Stability:** P2P 20-iteration (C) shows stable SSD-tier latency (max/min ratio < 1.3), confirming caching doesn't degrade.

3. **Minimum detectable effect:** The 32× cudaMemcpy elimination saves ~100-130 μs total (each H2D copy of 128 KiB ≈ 3-4 μs). Against ~13000 μs total SSD time, this is ~1%. If improvement < 3%, conclude: cudaMemcpy savings are real but dominated by sequential NVMe. Next step: P2P + BatchSubmit.

## Constraints

- All measurements through certus-server with `--dispatcher-version v1` (campaign constraint)
- No modification to gpu-p2p-server (campaign constraint)
- No standalone benchmark binaries (campaign constraint)
- P2P implementation in `components/dispatcher/v1/` only
- Memory-tier confound acknowledged per RP-6: multi-iteration not apples-to-apples
- nvidia-peermem and gdrdrv kernel modules required
- Single NVMe at 0000:63:00.0; one SPDK process at a time

## Prior Knowledge

- **RP-5:** P2P with persistent staging achieves 2x lower first-hit SSD latency [high confidence]. Predicts significant effect.
- **RP-6:** Multi-iteration benchmarks confounded by memory-tier promotion [high confidence]. Motivates 1-iteration primary comparison.
- **RP-7:** Caching prepare_memory_for_spdk eliminates per-lookup overhead [high confidence]. This is the mechanism under test.
