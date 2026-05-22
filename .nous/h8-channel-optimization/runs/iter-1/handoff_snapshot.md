# Handoff — Hypothesis 8 Channel Optimization, Iteration 1

## Goal

Optimize dispatcher v1 cold 16 MiB lookup throughput from ~1750 MB/s toward the ~3250 MB/s ceiling achieved by gpu-bb-vs-p2p, by reducing per-chunk synchronization overhead in the `pipelined_ssd_to_gpu_zero_copy` pipeline.

## Key Discoveries

1. **The cold lookup pipeline processes 128 chunks for 16 MiB** — chunk_size = max_transfer_size = 128 KiB (MDTS). This is set at `components/block-device-spdk-nvme/v2/src/controller.rs:158` and used via `pipeline.rs:264`.

2. **Three overhead sources per chunk in zero-copy path:** (a) Mutex lock on `Arc<Mutex<DmaBuffer>>` wrapper at `pipeline.rs:338`, (b) Mutex lock on GPU state in `dma_copy_to_device_async` at `gpu-services/v0/src/lib.rs:632`, (c) `cudaStreamSynchronize` on alternate stream at `pipeline.rs:353`.

3. **gpu-bb-vs-p2p achieves ceiling by calling CUDA directly** — no Mutex on GPU state, pre-allocated ring buffers (no per-call alloc), same channel-based NVMe commands with QD=32. The algorithmic structure is identical (ring + 2 alternating streams + sync alternate).

4. **CLIENT_CHANNEL_CAPACITY = 64** (at `block-device-spdk-nvme/v2/src/lib.rs:68`). ZERO_COPY_DEPTH = 32 (at `pipeline.rs:276`). So the channel is never a bottleneck (32 in-flight < 64 capacity).

5. **The non-zero-copy `pipelined_ssd_to_gpu` already batches stream sync** — at `pipeline.rs:192-197` it syncs both streams every `ring_size` chunks rather than per-chunk. This validates the safety of batched sync.

6. **Per-call DmaBuffer allocation creates 128 objects** — at `pipeline.rs:277-288`, each call to `pipelined_ssd_to_gpu_zero_copy` allocates a Vec of `Arc<Mutex<DmaBuffer>>` with noop_free wrappers. These involve heap allocation, atomic init (Arc), and Mutex init per element.

7. **Uncontested Mutex costs ~20ns on x86** — for 128 chunks × 2 Mutex locks = 256 locks → ~5 us total. Stream sync at ~1-5 us each × 128 = 128-640 us. Allocation overhead for 128 Arc<Mutex<DmaBuffer>> is likely ~50-100 us (heap + atomic + Mutex init).

## System Interface

- **Build:** `cargo bench -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark`
- **Run baseline:** Same as build (the bench binary IS the runner; it outputs results to stdout)
- **Reference ceiling:** `LD_LIBRARY_PATH=/usr/local/lib cargo run --release -p gpu-bb-vs-p2p -- --stream-size 16777216 --iterations 50`
- **Output format:** Table printed to stdout: `label | KiB | mean us | min us | p50 us | p99 us | max us | MB/s`
- **Baseline result:** cold_16384KiB ≈ 1750 MB/s (from research question specification)

## Code Map

| Location | What | When to look |
|----------|------|--------------|
| `components/dispatcher/v1/src/pipeline.rs:247-388` | `pipelined_ssd_to_gpu_zero_copy` — the function being optimized | Main edit target |
| `components/dispatcher/v1/src/pipeline.rs:18` | `PIPELINE_RING_SIZE=8` | Only for non-zero-copy path |
| `components/dispatcher/v1/src/pipeline.rs:27-66` | `PipelineRing` struct | Add pre-allocated wrappers here |
| `components/dispatcher/v1/src/pipeline.rs:276` | `ZERO_COPY_DEPTH=32` | Controls max in-flight NVMe reads |
| `components/dispatcher/v1/src/lib.rs:247-270` | `promote_and_serve` calls zero-copy pipeline | Understand calling context |
| `components/dispatcher/v1/src/lib.rs:648-659` | PipelineRing initialization in `initialize()` | Where to add pre-allocated wrappers |
| `components/gpu-services/v0/src/lib.rs:610-659` | `dma_copy_to_device_async` with Mutex on state | GPU state Mutex target |
| `components/gpu-services/v0/src/lib.rs:587-606` | `stream_synchronize` — no Mutex (already fast) | No change needed |
| `apps/gpu-bb-vs-p2p/src/main.rs:213-328` | Reference `pipelined_transfer` function | Compare algorithm |
| `components/block-device-spdk-nvme/v2/src/lib.rs:68` | `CLIENT_CHANNEL_CAPACITY=64` | Channel capacity |
| `components/block-device-spdk-nvme/v2/src/lib.rs:375-413` | `connect_client()` channel setup | Not on hot path (cached) |
| `components/interfaces/src/iblock_device.rs:206-214` | `Command::ReadAsync` type definition | Requires Arc<Mutex<DmaBuffer>> |
| `components/dispatcher/v1/benches/dispatcher_hw_benchmark.rs` | Full benchmark setup and measurement | Measurement harness |
| `components/dispatcher/v1/benches/pipeline_hw_benchmark.rs` | Standalone pipeline benchmark | For isolated pipeline timing |

## Code Targets

### Arm: h-main (combined optimization)

1. **GPU state atomic bypass:**
   - File: `components/gpu-services/v0/src/lib.rs`
   - Location: struct definition (around line 50-80) and `dma_copy_to_device_async` (line 632)
   - Change: Add `AtomicBool` field, check it before Mutex lock
   - WHY here: This is the only location where GPU state is checked per-call

2. **Batched stream sync:**
   - File: `components/dispatcher/v1/src/pipeline.rs`
   - Location: lines 350-355 in `pipelined_ssd_to_gpu_zero_copy`
   - Change: Replace per-chunk sync with batched sync (every ZERO_COPY_DEPTH chunks)
   - WHY here: The non-zero-copy path (`pipelined_ssd_to_gpu`, lines 192-197) already does this safely

3. **Pre-allocated DmaBuffer wrappers:**
   - File: `components/dispatcher/v1/src/pipeline.rs`
   - Location: `PipelineRing` struct (line 27) and call site (lines 277-288)
   - Change: Move wrapper allocation to PipelineRing::new(), reuse across calls
   - WHY here: Each promote_and_serve call creates 128 fresh wrappers; reusing from ring avoids heap churn

### Arm: h-ablation (sync batching only)

- File: `components/dispatcher/v1/src/pipeline.rs`, lines 350-355
- Same change as item 2 above, applied in isolation

### Arm: h-control-negative

- Same code changes as h-main, but measurement at 128 KiB (cold_128KiB row in benchmark output)

## What I Tried That Didn't Work

- **Subagent spawning:** Agent tool with subagent_type=Explore failed with 401 model access error (team can only access specific models). Had to explore manually.

## What I Excluded and Why

1. **Changing Command::ReadAsync interface** — Requires `Arc<Mutex<DmaBuffer>>`. Even though the Mutex is uncontested at receive time, modifying this interface would affect all block device consumers across the workspace. The overhead (~20ns per lock) is negligible anyway.

2. **Increasing ZERO_COPY_DEPTH beyond 32** — Channel capacity is 64, and depth of 32 already provides good overlap. Increasing would increase memory pressure without proportional benefit, and risks channel backpressure.

3. **Removing the memory-tier memcpy** — The zero-copy path already eliminated this (line 168-176 is only in the non-zero-copy `pipelined_ssd_to_gpu`). The current cold path uses `pipelined_ssd_to_gpu_zero_copy` which reads directly into memory-tier.

4. **Modifying the actor or channel infrastructure** — The channels use a lock-free SPSC ring buffer with cache-padded atomics. At QD=32, the channel is never at capacity (64 slots). Channel overhead per message is negligible (~20-50ns).

5. **P2P (GDRCopy) path** — Previous experiments showed P2P has topology-dependent results (65% slower when NVMe/GPU are on different PCIe buses). The bounce-buffer path is the correct baseline.

## Evolution of Thinking

Started by assuming the channel infrastructure (Sender/Receiver with Mutex-guarded ring buffer) was the bottleneck ("channel optimization" hypothesis name). After reading the code, realized:

1. The channels use a lock-free ring buffer — NOT Mutex-guarded. The channel is fast.
2. The real overhead comes from HIGHER layers: DmaBuffer wrapping with `Arc<Mutex<>>`, GPU services state Mutex, and per-chunk stream synchronization.
3. The gap is likely dominated by stream sync overhead (128 blocking calls) rather than Mutex costs, since uncontested Mutexes are only ~20ns.
4. The allocation overhead (128 heap allocs per call) may contribute meaningfully given that the function runs at ~9500 us — even 100 us of allocation overhead is ~1% of total.

The hypothesis name "channel optimization" is slightly misleading — the channels themselves are fine. The real target is the **pipeline synchronization strategy** and **per-call allocation pattern**.

## Current Status

- **Validated:** Code structure fully understood. Three concrete optimizations identified with code locations. The non-zero-copy path already validates safety of batched stream sync. The hypothesis is grounded in observed code patterns.
- **Uncertain:** Whether stream_synchronize is actually blocking (it may return immediately if GPU copies finish before next NVMe read). Whether allocation overhead is measurable at the 128-object scale. Whether the 4400 us gap is dominated by NVMe read latency (which no software optimization can fix) vs overhead.
- **Suggested next:** If this iteration shows minimal improvement, investigate whether the performance gap is fundamentally due to NVMe read scheduling latency (sequential chunk submission vs burst/batch submission) rather than pipeline overhead. Consider batch-submit NVMe commands (already in the Command enum as `BatchSubmit`) to reduce per-command channel round-trips.

## Warnings & Constraints

1. **Hardware required** — Both benchmarks need NVMe SSDs bound to SPDK (VFIO driver), CUDA GPU, hugepages. Cannot validate commands in this environment.
2. **DmaBuffer::from_raw is unsafe** — Changes to DmaBuffer wrapper management must preserve SAFETY invariants: ptr must be valid for the DmaBuffer's lifetime, free function must match allocation method.
3. **PipelineRing lifetime** — If pre-allocated wrappers store pointers to memory-tier offsets, those pointers must be updated per-call since different keys map to different memory-tier slots.
4. **The `mem::forget` at pipeline.rs:384** — Current code intentionally leaks DmaBuffer wrappers to avoid calling noop_free. Pre-allocated wrappers must handle this differently (keep wrappers alive in PipelineRing, don't forget them).
5. **atexit hook in benchmarks** — Both benchmarks use `unsafe { atexit(exit_hook) }` with `_exit(0)` to avoid SPDK cleanup issues. Don't add cleanup code that runs after main().
