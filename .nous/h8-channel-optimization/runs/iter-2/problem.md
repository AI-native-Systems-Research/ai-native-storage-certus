# Problem Framing — Iteration 2: BatchSubmit + Single Deep Qpair

## Research Question

Can submitting all NVMe read chunks via a single `BatchSubmit` command (concentrating them on one deep qpair) reduce cold lookup latency variance and improve mean throughput by eliminating per-completion command resubmission overhead and enabling NVMe SSD read scheduling optimization?

**Key code locations:**
- Current zero-copy pipeline: `components/dispatcher/v1/src/pipeline.rs:247-388`
- BatchSubmit dispatch: `components/block-device-spdk-nvme/v2/src/actor.rs:750-768`
- Qpair selection logic: `components/block-device-spdk-nvme/v2/src/qpair.rs:256-265`
- Standard qpair depths: `components/block-device-spdk-nvme/v2/src/qpair.rs:141` — `[4, 16, 64, 256]`
- Reference BatchSubmit usage: `components/gpu-services/v0/src/bin/p2p_server.rs:271-322`

## System Interface

- **Build command:** `cargo bench -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark`
- **Run benchmark:** The built binary executes directly (no separate run step). Output goes to stdout.
- **Reference ceiling:** `LD_LIBRARY_PATH=/usr/local/lib cargo run --release -p gpu-bb-vs-p2p -- --stream-size 16777216 --iterations 50`
- **CLI flags relevant:** None for benchmark — constants are compile-time (`ZERO_COPY_DEPTH`, `PIPELINE_RING_SIZE`).
- **Code evidence:**
  - `ZERO_COPY_DEPTH = 32`: `pipeline.rs:276`
  - `PIPELINE_RING_SIZE = 8`: `pipeline.rs:18`
  - `MEASURED_ITERS = 50`: `dispatcher_hw_benchmark.rs:273`
  - `WARMUP_ITERS = 5`: `dispatcher_hw_benchmark.rs:272`
  - Qpair STANDARD_DEPTHS `[4, 16, 64, 256]`: `qpair.rs:141`

## Baseline Command

```bash
cargo bench -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark
```

## Baseline Validation

Build verified: `cargo build -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark` exits 0 (compiled successfully). Cannot run on this machine (no NVMe SSD or CUDA GPU hardware). Previous iteration baseline data (from hardware machine):
- cold_16384KiB: mean 11855-17424 us, min 2730-9579 us, throughput 918-1499 MB/s (extreme variance 2-5x mean/min ratio)

## Experimental Conditions

### Condition A: Baseline (current code)

Current zero-copy pipeline with:
- Individual `ReadAsync` commands submitted one at a time (32 primed, then 1 per completion)
- Per-completion stream synchronize (alternate stream)
- Commands distributed across multiple qpairs as `pending_ops` grows (depths 4→16→64)
- 128 `Arc<Mutex<DmaBuffer>>` wrappers created per call, `mem::forget`'d at end

### Condition B: BatchSubmit All-At-Once (h-main)

Replace the prime-and-resubmit pattern with:
1. Submit ALL 128 chunk reads via a single `BatchSubmit { ops: vec![ReadAsync x 128] }` — one channel message, all commands concentrated on the depth-256 qpair
2. Receive all 128 completions (NVMe reads complete into their respective memory-tier offsets)
3. After all reads complete, issue GPU DMA for all 128 chunks using dual-stream interleaving with batched sync (per ring cycle, not per chunk)

This sacrifices NVMe↔GPU DMA overlap but eliminates:
- 127 channel sends (reduced from 128 to 1)
- Per-completion resubmission scheduling jitter
- Qpair scattering (all ops on single depth-256 qpair)
- 128 stream_synchronize calls in the read loop (moved to post-read GPU DMA phase)

### Condition C: BatchSubmit with Pipelined GPU DMA (h-ablation)

Hybrid approach:
1. Submit ALL 128 chunk reads via BatchSubmit (same as Condition B)
2. Process completions AS THEY ARRIVE, issuing GPU DMA for each completed chunk (maintaining NVMe↔GPU overlap)
3. Per-chunk alternate stream sync (same as current baseline)

This tests whether BatchSubmit's qpair concentration alone provides benefit, independent of the GPU DMA scheduling change.

### Condition D: Negative Control (h-control-negative)

Same as Condition B (BatchSubmit all-at-once) but measured at cold_128KiB (single chunk). At 1 chunk, BatchSubmit contains exactly 1 ReadAsync — functionally identical to the baseline. No improvement expected.

## Success Criteria

- **Primary:** cold_16384KiB mean throughput consistently > 2000 MB/s across multiple runs (vs baseline ~1200-1500 MB/s mean)
- **Secondary:** cold_16384KiB mean/min ratio < 2.0 (vs baseline 2-5x), indicating reduced variance
- **Tertiary:** cold_16384KiB min latency ≤ baseline min latency (no regression in best-case)
- **Negative control:** cold_128KiB shows no meaningful throughput difference from baseline

## Constraints

- Do NOT remove the component architecture (actors, channels, components)
- Do NOT add new external crate dependencies
- Hardware required: NVMe SSD bound to SPDK (VFIO), CUDA GPU, hugepages
- `BatchSubmit` is an existing `Command` variant — no interface changes needed
- The zero-copy path assumes FIFO completion ordering; with BatchSubmit on a single qpair, NVMe guarantees in-order completion for sequential reads within a namespace

## Prior Knowledge

Active principles that apply:
- **RP-17:** Pool eviction dominates variance, not pipeline overhead. This iteration targets variance by eliminating per-completion scheduling jitter — a different mechanism than the pipeline micro-optimizations attempted in iter-1.
- **RP-18:** Best-case min latency is 2700-4200 us (NVMe floor). We don't expect to beat this; we aim to bring the MEAN closer to it.
- **RP-19:** Micro-optimizations (stream sync batching, allocation elimination) don't produce measurable improvement. This iteration's BatchSubmit is a structural change (different NVMe command scheduling), not a micro-optimization.
- **RP-20:** ZERO_COPY_DEPTH increase from 16→32 showed min latency improvement (-25%), suggesting deeper pipelining helps NVMe throughput. BatchSubmit with all 128 commands is the logical extension.
