# Problem Framing — Iteration 2: Sliding-Window P2P Pipeline

## Research Question

Can a sliding-window P2P pipeline (NVMe→GPU BAR1 with overlapped D2D copies) match or exceed the baseline zero-copy DRAM path throughput?

Iteration 1 demonstrated that a **batch-based** P2P pipeline (submit all reads → wait all → copy all → sync) was 19% slower than baseline due to lost pipeline overlap (RP-2). The baseline `pipelined_ssd_to_gpu_zero_copy` (`components/dispatcher/src/pipeline.rs:244`) uses a sliding-window design where NVMe completions immediately trigger H2D copies while further reads proceed in parallel.

This iteration implements P2P with the **same sliding-window pattern**: as each NVMe read into a BAR1 ring slot completes, immediately issue `cudaMemcpyAsync(D2D)` to the final GPU destination and submit the next NVMe read. Since D2D copies between GPU BAR1 staging and GPU VRAM use internal GPU bandwidth (~1.5 TB/s on A30), they complete in <0.1 µs for 128 KiB chunks — effectively free compared to the ~22 µs NVMe latency.

Key source files:
- `components/dispatcher/src/pipeline.rs:244` — baseline sliding-window pipeline
- `components/dispatcher/src/pipeline.rs:85` — batch-based ring pipeline (model to NOT follow)
- `components/gpu-services/src/dma.rs:353` — `create_spdk_dma_buffer_from_gpu_bar`
- `components/gpu-services/src/cuda_ffi.rs:66` — `CUDA_MEMCPY_DEVICE_TO_DEVICE`
- `components/gpu-services/src/cuda_ffi.rs:118` — `cudaMemcpyAsync` declaration

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
  - Feature chain: `certus-server/p2p` → `dispatcher/p2p` → `gpu-services/p2p`
  - Code evidence: `apps/certus-server/Cargo.toml:9`, `components/dispatcher/Cargo.toml:24`
- **Server start:** `./target/release/certus-server --drive-count 1 --format`
  - Code evidence: `apps/certus-server/src/main.rs:34` (--drive-count), `:48` (--format)
  - Wait 4+ seconds for SPDK init + NVMe probe + gRPC port 50051
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Integrity check:** `python3 apps/python/certus-api-bench.py --clients 1 --verify-integrity --integrity-objects 8`
- **Output:** stdout, parse `Lookup (cold)` section then `per-client=X.XX GB/s`
- **Kill between conditions:** Required — pipeline ring allocated at init.

## Baseline Command

```bash
./target/release/certus-server --drive-count 1 --format &
sleep 5
python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304
kill %1
```

## Baseline Validation

Build exits 0. Baseline performance from iter-1: 2.41–2.43 GB/s cold lookup throughput (avg latency ~1733 µs, p99 ~1820 µs). Data integrity PASSED.

## Experimental Conditions

### Condition A: Baseline (no code changes)
Unchanged `pipelined_ssd_to_gpu_zero_copy` path. NVMe→DRAM→GPU.

### Condition B: Sliding-Window P2P (h-main)
Replace the batch-based `pipelined_ssd_to_gpu_p2p` with a new sliding-window implementation. Key changes:
1. In `pipeline.rs`: new function `pipelined_ssd_to_gpu_p2p` that uses a VecDeque-based sliding window (matching the zero-copy pattern at line 292–387) but targets BAR1 ring slots and issues `cudaMemcpyAsync(D2D)` instead of `dma_copy_to_device_async(H2D)`.
2. Ring slot reuse: after issuing D2D copy on a stream, track which stream holds the oldest pending copy. Before reusing the ring slot, sync only if needed (D2D at ~1.5 TB/s completes in <0.1 µs for 128 KiB, so in practice slots are always safe by the time they cycle back through 8 slots).
3. Skip memory-tier allocation in `promote_and_serve` for the P2P path — data goes GPU-only for this condition.

### Condition C: Sliding-Window P2P + DRAM backfill (h-ablation)
Same sliding-window P2P as condition B, but also issues `cudaMemcpyAsync(D2H)` from the staging slot to the memory-tier DRAM slot after each D2D copy. This maintains cache coherence for subsequent warm lookups.

## Success Criteria

- **h-main:** P2P throughput ≥ baseline throughput (direction: positive or neutral). If P2P eliminates one PCIe crossing, throughput should improve. At minimum, the sliding-window approach must not be SLOWER than baseline (unlike iter-1's batch approach).
- **h-ablation:** DRAM backfill overhead should be small (<5% reduction from h-main), confirming RP-3.
- **Hard constraints:** Build succeeds, data integrity passes for both conditions.

## Constraints

- RP-1: P2P with staging ring is slower when DRAM path reads directly to final destination. **Addressed:** This iteration's sliding-window overlaps reads with D2D copies (eliminating the batch serialization that caused RP-1's observed regression).
- RP-2: Batch pipelines are significantly slower than sliding-window. **Addressed:** New implementation uses sliding-window pattern.
- RP-3: GPU→DRAM backfill adds ~3% overhead when serialized. **Tested:** h-ablation measures this on the new sliding-window path.
- Ring slot reuse: Must ensure D2D copy completes before NVMe reuses the slot.
- Server must be killed and restarted between conditions.

## Prior Knowledge

Active principles RP-1, RP-2, RP-3 from iteration 1 all apply. This iteration directly addresses RP-2 by switching from batch to sliding-window, and re-tests RP-1 under corrected conditions (proper pipeline overlap). RP-3's ~3% estimate is validated via the h-ablation arm.
