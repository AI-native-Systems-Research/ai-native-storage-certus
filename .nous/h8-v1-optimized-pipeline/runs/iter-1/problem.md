# Problem Framing — Dispatcher v1 Pipeline Optimization

## Research Question

Does an optimized pipeline (parallel NVMe reads via ReadAsync with a primed ring + async GPU copies with alternating CUDA streams) achieve lower SSD-tier lookup latency than the current v1 sequential pipeline for 4 MiB objects?

The current implementation at `components/dispatcher/v1/src/pipeline.rs:60-119` issues NVMe reads sequentially (ReadSync, wait for completion, copy to DRAM, copy to GPU, repeat). For a 4 MiB object with 128 KiB MDTS, this means 32 sequential round-trips to the SSD.

The reference implementation at `apps/gpu-bb-vs-p2p/src/main.rs:198-328` demonstrates the optimized pattern: prime a ring of buffers with parallel ReadAsync requests, process completions as they arrive, issue async GPU copies on alternating CUDA streams, and resubmit NVMe reads into freed ring slots.

## System Interface

### Build Command

```bash
cargo build -p certus-server -p dispatcher-v1
```

### CLI Flags (certus-server)

| Flag | Default | Semantics | Code evidence |
|------|---------|-----------|---------------|
| `--metadata-pci` | required | PCI address of metadata NVMe | `apps/certus-server/src/main.rs:30` |
| `--data-pci` | required | PCI address(es) of data NVMe | `apps/certus-server/src/main.rs:34` |
| `--listen` | `0.0.0.0:50051` | gRPC listen address | `apps/certus-server/src/main.rs:38` |
| `--dispatcher-version` | `v1` | Dispatcher version ("v0" or "v1") | `apps/certus-server/src/main.rs:49` |

### CLI Flags (test_client.py benchmark)

| Flag | Default | Semantics | Code evidence |
|------|---------|-----------|---------------|
| `--bench-only` | false | Skip functional tests, run benchmark only | `apps/certus-server/python-client/test_client.py:455` |
| `--bench-object-size` | 65536 | Object size in bytes | `apps/certus-server/python-client/test_client.py:458` |
| `--bench-num-objects` | 100 | Objects per tier to benchmark | `apps/certus-server/python-client/test_client.py:462` |
| `--bench-iterations` | 10 | Lookup iterations per tier | `apps/certus-server/python-client/test_client.py:465` |

### Code Evidence

- **Sequential pipeline**: `components/dispatcher/v1/src/pipeline.rs:67` — uses `Command::ReadSync` per chunk
- **ReadAsync command**: `components/interfaces/src/iblock_device.rs:205` — async read with timeout
- **GPU DMA (sync)**: `components/interfaces/src/igpu_services.rs:421` — `dma_copy_to_device` uses synchronous cudaMemcpy
- **Reference optimized pattern**: `apps/gpu-bb-vs-p2p/src/main.rs:234-311` — primed ring + alternating CUDA streams
- **Block device v2 supports ReadAsync**: `components/block-device-spdk-nvme/v2/src/actor.rs:568`
- **BatchSubmit supported**: `components/block-device-spdk-nvme/v2/src/actor.rs:750`
- **CUDA FFI available**: `components/gpu-services/v0/src/lib.rs:26` — `pub mod cuda_ffi`
- **Dispatcher v1 depends on gpu-services**: `components/dispatcher/v1/Cargo.toml:13`

### Output Format

The test client prints results to stdout in this format:
```
  Tier            Avg (us/obj)   Min (us/obj)   Max (us/obj)   Avg (GB/s)   Peak (GB/s)
  ---------------------------------------------------------------------------------
  Memory-tier     <value>        <value>        <value>        <value>      <value>
  SSD-tier        <value>        <value>        <value>        <value>      <value>
```

## Baseline Command

```bash
# Terminal 1: Start certus-server (determine actual PCI addresses from system)
sudo ./target/debug/certus-server \
  --metadata-pci 0000:c3:00.0 \
  --data-pci 0000:c4:00.0 \
  --dispatcher-version v1

# Terminal 2: Run benchmark
cd apps/certus-server/python-client && \
python3 test_client.py --bench-only \
  --bench-object-size 4194304 \
  --bench-num-objects 20 \
  --bench-iterations 5
```

## Baseline Validation

Based on previous experiment memory:
- SSD-tier lookup latency for 4 MiB objects: ~19,500–23,000 us (from `project_v1_pipeline_iter2.md`)
- Memory-tier lookup latency: significantly lower (direct memcpy, no NVMe)
- System variance: ±20% between runs
- NVMe per-chunk time: ~600 us × 32 chunks = ~19,200 us theoretical sequential floor
- GPU H2D copy per-chunk: ~5-50 us (negligible vs NVMe)

The executor should validate the baseline produces SSD-tier latency in the 15,000–30,000 us range for 4 MiB objects.

## Experimental Conditions

### Condition A: Baseline (current sequential pipeline)

No code changes. Run the server with dispatcher v1 and measure SSD-tier lookup latency for 4 MiB objects using the `test_client.py --bench-only` benchmark.

### Condition B: Optimized pipeline (parallel NVMe reads + async GPU copies)

Modify `components/dispatcher/v1/src/pipeline.rs` to replace the sequential ReadSync loop with:

1. **Pre-allocate ring buffers** (already done — keep `PIPELINE_RING_SIZE` but increase to 8 or use the number of segments, whichever is smaller)
2. **Prime the ring**: Issue `Command::ReadAsync` for the first N segments (up to ring_size) before waiting for any completions
3. **Process completions in order**: For each completion received:
   - Copy completed chunk to memory-tier slot (same as current)
   - Issue `cudaMemcpyAsync` to GPU destination on alternating CUDA streams (via `gpu_services::cuda_ffi`)
   - Sync the alternate stream (ensures previous slot's GPU copy is done before buffer reuse)
   - Resubmit next NVMe ReadAsync into the freed ring slot
4. **Final sync**: Synchronize both CUDA streams after all chunks complete

This mirrors the pattern proven at ~3.2 GB/s in `apps/gpu-bb-vs-p2p/src/main.rs:198-328`.

Key differences from baseline:
- ReadSync → ReadAsync (allows NVMe queue depth > 1)
- Sequential GPU copy → cudaMemcpyAsync on alternating streams
- Buffers need `Arc<Mutex<DmaBuffer>>` (already the case)

### Condition C: Parallel NVMe reads only (no async GPU)

Same as Condition B but keep the synchronous `dma_copy_to_device` GPU copy (no CUDA streams). This isolates the NVMe parallelism benefit from the GPU async benefit.

Specifically: Use ReadAsync with primed ring and process completions, but after each completion, use the existing `gpu.dma_copy_to_device()` call (synchronous). This tests whether the dominant speedup comes from NVMe parallelism alone.

## Success Criteria

- **Primary**: Optimized pipeline (Condition B) achieves at least 30% lower mean SSD-tier latency than baseline (Condition A) for 4 MiB objects, consistently across seeds/iterations.
- **Secondary**: Throughput (GB/s) for SSD-tier lookups improves proportionally.
- **Mechanism validation**: Condition C (NVMe-only parallelism) captures the majority of the improvement, confirming NVMe read dominance as predicted by previous experiments (RP-16).

## Constraints

- Implementation changes in `components/dispatcher/v1/` only.
- Must restart server between each condition (fresh process, fresh populate cycle).
- Must use block-device-spdk-nvme v2 (default) which supports ReadAsync.
- The `dma_copy_to_device` in `IGpuServices` is synchronous — to get async GPU copies, must call `cudaMemcpyAsync` directly via `gpu_services::cuda_ffi`.
- 4 MiB object size = 32 chunks at 128 KiB MDTS.
- Previous experiments showed ±20% system variance — need multiple iterations per condition.

## Prior Knowledge

- **RP-16** (from memory `project_v1_pipeline_iter2.md`): NVMe read dominance is the binding constraint at 128 KiB chunks. GPU copy overlap is second-order (<8% theoretical max), undetectable under measurement noise.
- **RP-12** (from memory `project_evolve_v0_batchsubmit_iter2.md`): gRPC + connect_client() overhead dominates single-lookup latency at ~15-25ms. However, the batch benchmark amortizes this over `num_objects` lookups.
- Previous iteration showed cudaHostAlloc pipeline gives no improvement because NVMe reads dominate.
- gpu-bb-vs-p2p reference achieves ~3.2 GB/s with ring_size=32 and parallel NVMe reads — this is the gold standard.
- The benchmark uses batch lookups (100 objects per iteration), which amortizes gRPC overhead and measures per-object pipeline latency directly.
