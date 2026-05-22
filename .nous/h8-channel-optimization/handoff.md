# Handoff — Hypothesis 8 Channel Optimization, Iteration 2

## Goal

Reduce dispatcher v1 cold 16 MiB lookup latency variance and improve mean throughput by replacing the per-completion NVMe command resubmission pattern with BatchSubmit (all 128 reads in one message to a single deep qpair), then comparing two GPU DMA scheduling strategies: deferred (all at once after reads) vs interleaved (per completion as they arrive).

## Key Discoveries

1. **Qpair scattering causes fragmented SSD command visibility.** The current pipeline submits individual ReadAsync commands. The actor's `select_index(pending_ops.len() + 1)` distributes them across qpairs of depth 4, 16, and 64 as in-flight count grows (`qpair.rs:256-265`). With 32 in-flight, commands end up on 3 different qpairs. BatchSubmit forces all ops to one qpair via `select_index(batch_size)` — for 128 ops, this selects the depth-256 qpair (`qpair.rs:141`).

2. **Completion ordering is safe for sequential reads.** NVMe SSDs process sequential reads in submission order within a single qpair. The zero-copy path reads sequential LBAs (chunked from a contiguous extent). With all commands on one qpair, FIFO completion is maintained — the existing `for completed in 0..num_chunks` pattern remains correct.

3. **GPU DMA is already hidden behind NVMe latency.** Each 128 KiB GPU DMA takes ~1 us at 12 GB/s. Each 128 KiB NVMe read takes ~18 us at 7 GB/s. The `stream_synchronize` on the alternate stream is effectively a no-op in steady state (the previous DMA finished long ago). Deferring all GPU DMA to after reads costs ~128 us total — negligible vs ~2700 us NVMe total.

4. **The variance source appears to be inter-thread scheduling jitter, not pool management.** The benchmark removes/re-inserts a single key — the FreeList allocator returns the same offset each time (sub-microsecond). The memory-tier Mutex, dispatch-map HashMap lookups, and actor polling timing are the remaining candidates. BatchSubmit eliminates 127 of 128 actor→dispatcher round-trip synchronization points.

5. **Actor parking is NOT the issue.** `PARK_THRESHOLD = 10,000,000` iterations (`actor.rs:631`). At ~3 GHz, this is ~3.3 ms of spinning. Benchmark iterations are spaced < 1 ms (NVMe completions keep the actor busy until end of iteration). The actor stays in its busy-poll loop.

6. **`ZERO_COPY_DEPTH` was already increased to 32 in uncommitted changes** (from the iter-1 experiment). The current working tree has this change plus per-chunk alternate stream sync (matching the reference algorithm).

7. **gpu-bb-vs-p2p reference uses individual ReadAsync (not BatchSubmit)** in its `pipelined_transfer` function (`apps/gpu-bb-vs-p2p/src/main.rs:235-311`). It achieves ~3250 MB/s with ring_size=32 and direct `cudaMemcpyAsync` (no Mutex). The p2p_server uses BatchSubmit for a different access pattern (`do_chunked_read`, all reads then all copies). The p2p_server achieved 27.3x speedup but that was vs a different baseline (gRPC overhead dominated).

## System Interface

- **Build:** `cargo bench -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark`
- **Run baseline:** Same command (the bench binary IS the runner; it outputs results to stdout)
- **Output format:** Table printed to stdout: `label | KiB | mean us | min us | p50 us | p99 us | max us | MB/s`
- **Baseline result:** cold_16384KiB ≈ 1200-1500 MB/s mean (from iter-1 data; extreme variance)
- **Reference ceiling:** `LD_LIBRARY_PATH=/usr/local/lib cargo run --release -p gpu-bb-vs-p2p -- --stream-size 16777216 --iterations 50` (~3250 MB/s)

## Code Map

| Location | What | When to look |
|----------|------|--------------|
| `components/dispatcher/v1/src/pipeline.rs:247-388` | `pipelined_ssd_to_gpu_zero_copy` — EDIT TARGET for all arms | Main edit target |
| `components/dispatcher/v1/src/pipeline.rs:276` | `ZERO_COPY_DEPTH=32` (controls max in-flight, already modified) | Reference only |
| `components/dispatcher/v1/src/pipeline.rs:8` | `use std::sync::{Arc, Mutex}` | May need `use interfaces::Command` |
| `components/interfaces/src/iblock_device.rs:236-239` | `Command::BatchSubmit { ops: Vec<Command> }` definition | Verify BatchSubmit API |
| `components/block-device-spdk-nvme/v2/src/actor.rs:750-768` | BatchSubmit handler — iterates ops, selects single qpair | Understand dispatch behavior |
| `components/block-device-spdk-nvme/v2/src/qpair.rs:141` | `STANDARD_DEPTHS: [4, 16, 64, 256]` | Understand qpair topology |
| `components/block-device-spdk-nvme/v2/src/qpair.rs:256-265` | `select_index()` — finds shallowest qpair with capacity | Why scattering happens |
| `components/gpu-services/v0/src/bin/p2p_server.rs:271-322` | `do_chunked_read()` — reference BatchSubmit usage pattern | Copy this pattern |
| `components/dispatcher/v1/benches/dispatcher_hw_benchmark.rs:518-593` | `bench_cold_lookup()` — measurement harness | Understanding measurement |
| `components/dispatcher/v1/src/lib.rs:192-270` | `promote_and_serve()` — calls zero-copy pipeline | Calling context |
| `components/component-framework/crates/component-core/src/actor.rs:631-668` | Actor park/idle loop | Not on critical path |
| `components/memory-tier/v0/src/lib.rs:152-183` | `insert()` — FreeList alloc + HashMap + LRU | Not the bottleneck |

## Code Targets

### Arm: h-main (BatchSubmit all-at-once + deferred GPU DMA)

- **File:** `components/dispatcher/v1/src/pipeline.rs`
- **Function:** `pipelined_ssd_to_gpu_zero_copy` (line 247)
- **Change:** Replace lines 290-374 (prime + steady-state + resubmit loop) with:
  1. Build `Vec<Command>` of all `num_chunks` ReadAsync commands
  2. Send as `Command::BatchSubmit { ops }` — one channel message
  3. Receive all `num_chunks` completions in a simple recv() loop (no GPU DMA here)
  4. After all reads complete: iterate chunks, issue `dma_copy_to_device_async` on alternating streams, sync both streams every 8 chunks
  5. Keep the final sync of both streams (lines 376-380)
  6. Keep the `mem::forget` cleanup (lines 382-385)
- **WHY here:** This is the only location where NVMe commands are submitted for the cold read path. The function signature and DmaBuffer setup (lines 276-288) remain unchanged.

### Arm: h-ablation (BatchSubmit + per-completion GPU DMA)

- **File:** `components/dispatcher/v1/src/pipeline.rs`
- **Function:** `pipelined_ssd_to_gpu_zero_copy` (line 247)
- **Change:** Replace lines 290-374 with:
  1. Build `Vec<Command>` of all `num_chunks` ReadAsync commands
  2. Send as `Command::BatchSubmit { ops }` — one channel message
  3. Process completions one at a time: for each, issue GPU DMA on alternating stream + sync alternate stream (same as current per-chunk pattern, just without the resubmission)
- **WHY here:** Same function, same location. Only difference from h-main is GPU DMA timing.

### Arm: h-control-negative

- Same code as h-main. Measurement target is `cold_128KiB` row in benchmark output.
- **WHY:** At 1 chunk, BatchSubmit({1 op}) is functionally identical to sending 1 ReadAsync. Validates mechanism specificity.

## What I Tried That Didn't Work

(From iter-1, preserved:)
- **Pipeline micro-optimizations (iter-1):** Atomic GPU state check, batched stream sync every 32 chunks, pre-allocated DmaBuffer wrappers — all produced no measurable improvement. The combined effect (~600 us) was below measurement noise (~5000 us variance). DO NOT retry these.
- **ZERO_COPY_DEPTH increase from 16→32:** Showed min latency improvement (25%) but didn't reduce mean variance. Already incorporated into current working tree.

## What I Excluded and Why

1. **Increasing PIPELINE_RING_SIZE for the non-zero-copy path** — The cold lookup uses the zero-copy path exclusively. The non-zero-copy path (with ring buffers) is a separate code path for cases where memory-tier isn't CUDA-pinned.

2. **Doorbell batching optimization** — SPDK handles NVMe submission queue doorbell internally. The `qp.submit()` in the actor is just an in-flight counter increment, not a doorbell ring. No optimization possible here.

3. **Removing `Arc<Mutex<DmaBuffer>>` wrapping** — The `Command::ReadAsync` interface requires `Arc<Mutex<DmaBuffer>>`. Changing this would require modifying the interface crate, affecting all block device consumers. The overhead (~25 us for 128 wraps) is negligible.

4. **Increasing measured iterations** — MEASURED_ITERS is already 50, which is sufficient for statistical analysis. More iterations won't reduce the WITHIN-run variance caused by inter-thread scheduling.

5. **Pinning the benchmark to specific cores** — Would require modifying the benchmark harness and may mask real-world behavior. The actor already pins to a NUMA-local core.

6. **Multiple seeds/repeated runs** — The executor handles this via multi-run protocol. Two sessions (S1, S2) per condition provide inter-run variance measurement.

## Evolution of Thinking

**Iter-1 hypothesis:** Pipeline overhead (stream sync, Mutex, allocation) is the bottleneck.
**Iter-1 finding:** Pipeline overhead is only ~600 us out of ~9500 us mean. Variance (2-5x mean/min) dominates. Min latency (2700 us) is already close to optimal.

**Iter-2 shift:** The problem isn't the pipeline ALGORITHM — it's the NVMe command SCHEDULING pattern. The current pattern creates 128 inter-thread synchronization points (send ReadAsync → wait for actor → get completion → resubmit). Each point has scheduling jitter from OS thread scheduling between the dispatcher thread and the actor thread. BatchSubmit collapses 128 sends into 1, and eliminates 96 resubmissions (128 - 32 primed = 96 resubmits become 0).

Additionally, qpair scattering means the SSD sees commands arriving on 3 different queues. A single deep qpair lets the SSD firmware coalesce the sequential reads internally. This is a fundamentally different optimization class than iter-1's micro-optimizations — it changes HOW commands reach the hardware, not how fast we process results.

**Key realization:** The gpu-bb-vs-p2p reference achieves 3250 MB/s with the SAME per-command pattern as the dispatcher (not BatchSubmit). This means BatchSubmit qpair concentration might not be the primary differentiator. The reference's advantage is likely: (a) direct cudaMemcpyAsync (no Mutex), (b) ring_size=32 pre-allocated CUDA-pinned buffers (no per-call DmaBuffer wrapping), (c) no memory-tier/dispatch-map overhead around the pipeline. If BatchSubmit doesn't help, the remaining gap is likely attributable to the dispatcher infrastructure overhead AROUND the pipeline (Mutex locks, HashMap lookups, etc.) — not fixable without violating the "no architecture removal" constraint.

## Current Status

- **Validated:** Build compiles. BatchSubmit interface exists and is correctly dispatched. Qpair selection behavior understood. Completion ordering safe for sequential reads on single qpair.
- **Uncertain:** (a) Whether qpair scattering actually occurs at runtime (vs all commands fitting in the depth-64 qpair). (b) Whether SSD read coalescing provides measurable benefit for sequential 128 KiB reads. (c) Whether the variance comes from NVMe scheduling or from dispatcher infrastructure (Mutex, HashMap, dispatch-map operations) outside the pipeline.
- **Suggested next:** If BatchSubmit shows no improvement, the remaining gap between dispatcher (1750 MB/s mean) and reference (3250 MB/s) is likely attributable to dispatcher infrastructure overhead AROUND the pipeline (not the pipeline itself). The min latency of ~2700 us already matches or beats the reference. Future investigation should profile the FULL `d.lookup()` path (evict_for_space + mt.insert + dispatch-map + pipeline + dispatch-map cleanup) to identify which non-pipeline operation contributes the most to high-latency iterations. Consider adding per-stage timing instrumentation inside `promote_and_serve`.

## Warnings & Constraints

1. **Hardware required** — Both benchmarks need NVMe SSDs bound to SPDK (VFIO driver), CUDA GPU, hugepages.
2. **DmaBuffer wrappers must still be created** — BatchSubmit ReadAsync requires `Arc<Mutex<DmaBuffer>>` per command. The wrapping overhead cannot be eliminated by BatchSubmit.
3. **`mem::forget` at pipeline.rs:384** — After BatchSubmit, the DmaBuffer wrappers still need to be forgotten (noop_free avoids double-free of memory-tier memory).
4. **Completion order assumption** — The code after BatchSubmit MUST still process completions in submission order (using the `for completed in 0..num_chunks` pattern). This is safe for sequential reads on a single qpair but would break if the SSD reorders.
5. **atexit hook in benchmarks** — Both benchmarks use `unsafe { atexit(exit_hook) }` with `_exit(0)` to avoid SPDK cleanup issues.
6. **Working tree has uncommitted changes** — `pipeline.rs` already has ZERO_COPY_DEPTH=32 and per-chunk stream sync from iter-1. The executor should start from this state (not from the committed baseline with ZERO_COPY_DEPTH=16).
7. **The `connect_client()` in the current zero-copy path was removed** — The function now takes `channels: &ClientChannels` as a parameter (cached channels from `promote_and_serve`). The BatchSubmit command should use these cached channels.
