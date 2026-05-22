# Problem Framing: Dispatcher v1 Cold Lookup Channel Optimization

## Research Question

Can the dispatcher v1 cold 16 MiB lookup throughput (~1750 MB/s as measured by `dispatcher_hw_benchmark`) match the gpu-bb-vs-p2p reference ceiling (~3250 MB/s) by eliminating per-chunk synchronization overhead in the `pipelined_ssd_to_gpu_zero_copy` pipeline, without removing the component architecture or adding new external dependencies?

The core mechanism under study is `pipelined_ssd_to_gpu_zero_copy` at `components/dispatcher/v1/src/pipeline.rs:247`. For a 16 MiB transfer with 128 KiB chunks (MDTS), 128 chunks flow through the pipeline. Each chunk currently incurs:
- 1x `Mutex::lock` on `Arc<Mutex<DmaBuffer>>` wrapper (`pipeline.rs:338`)
- 1x `Mutex::lock` on GPU services state inside `dma_copy_to_device_async` (`gpu-services/v0/src/lib.rs:632`)
- 1x `cudaStreamSynchronize` on alternate stream (`pipeline.rs:353-355`)
- Allocation of 128 `Arc<Mutex<DmaBuffer>>` wrappers at call entry (`pipeline.rs:277-288`)

The gpu-bb-vs-p2p reference (`apps/gpu-bb-vs-p2p/src/main.rs:213`) uses the same NVMe→GPU pipeline structure (ring buffer, 2 alternating CUDA streams, QD=32) but calls `cudaMemcpyAsync` directly (no Mutex), and its ring buffers are pre-allocated (no per-call wrapping overhead).

## System Interface

- **Build command:** `cargo bench -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark`
- **Reference ceiling:** `cargo run --release -p gpu-bb-vs-p2p -- --stream-size 16777216 --iterations 50` (requires `LD_LIBRARY_PATH=/usr/local/lib`)
- **Relevant CLI flags:** The benchmarks have no configurable flags — parameters are hardcoded constants (`MEASURED_ITERS=50`, `WARMUP_ITERS=5` in dispatcher_hw_benchmark).
- **Output format:** Both benchmarks print results to stdout in a table format with mean/min/p50/p99/max latency in microseconds and throughput in MB/s.

### Code Evidence

| Mechanism | Location | Purpose |
|-----------|----------|---------|
| PIPELINE_RING_SIZE=8 | `pipeline.rs:18` | Ring buffers for non-zero-copy path |
| ZERO_COPY_DEPTH=32 | `pipeline.rs:276` | Max in-flight NVMe reads in zero-copy path |
| chunk_size = max_transfer_size = 128 KiB | `controller.rs:158`, `lib.rs:654` | MDTS-sized segments |
| CLIENT_CHANNEL_CAPACITY=64 | `block-device-spdk-nvme/v2/src/lib.rs:68` | SPSC channel capacity |
| DmaBuffer Mutex per chunk | `pipeline.rs:338` | Guards buffer during H2D copy |
| GPU state Mutex per async copy | `gpu-services/v0/src/lib.rs:632` | Checks initialization state |
| Per-chunk stream sync | `pipeline.rs:353-355` | Alternate stream sync every completion |
| connect_client cached | `dispatcher/v1/src/lib.rs:608` | Avoids per-call channel creation |

## Baseline Command

```bash
cargo bench -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark
```

## Baseline Validation

The benchmark cannot be run in this environment (requires SPDK-bound NVMe SSDs and CUDA GPU). Based on the research question specification, the baseline cold_16384KiB throughput is ~1750 MB/s (approximately 9500 us mean latency for 16 MiB). The reference ceiling from gpu-bb-vs-p2p is ~3250 MB/s (~5100 us for 16 MiB).

The gap is approximately 1.86x (3250/1750). This corresponds to ~4400 us of overhead per 16 MiB transfer attributable to the dispatcher's abstraction layers.

## Experimental Conditions

### Condition 0: Baseline (control)
Unmodified dispatcher code. Run `dispatcher_hw_benchmark` as-is to establish the current cold_16384KiB performance.

### Condition 1: Eliminate per-chunk DmaBuffer Mutex (h-main)
**Intent:** In `pipelined_ssd_to_gpu_zero_copy`, replace the `Arc<Mutex<DmaBuffer>>` chunk wrappers with a direct raw-pointer approach. Since zero-copy mode reads directly into the memory-tier (which has no concurrent writers during this operation), the Mutex is unnecessary. Pass raw `*mut c_void` pointers to the NVMe read command, avoiding 128 Mutex lock/unlock operations.

**What changes:**
- `pipeline.rs:277-288`: Instead of creating `Vec<Arc<Mutex<DmaBuffer>>>` wrappers, create `Vec<DmaBuffer>` (no Arc, no Mutex) or use `UnsafeCell`-based approach
- `pipeline.rs:338`: Remove `.lock().unwrap()` — access buffer directly
- `pipeline.rs:293-301`, `pipeline.rs:358-372`: Adjust NVMe read submission to use non-Mutex buffers
- The `Command::ReadAsync` variant accepts `Arc<Mutex<DmaBuffer>>` — this requires either changing the interface to also accept a raw buffer variant, or wrapping in Arc<Mutex> only for the channel send (minimal overhead since actor immediately unwraps)

**Constraint concern:** The `Command::ReadAsync` type in `interfaces/src/iblock_device.rs:209` requires `Arc<Mutex<DmaBuffer>>`. The optimization cannot change this interface without broader impact. However, the DmaBuffer mutex is only held by the actor during the actual NVMe I/O, and released before the completion is sent back. The client-side mutex lock at `pipeline.rs:338` happens AFTER the completion arrives — meaning the lock is uncontested. An uncontested Mutex::lock costs ~20ns on modern x86. Over 128 chunks that's only ~2.5 us — likely negligible.

**Revised analysis:** The per-chunk Mutex overhead alone (~2.5 us total) cannot explain the 4400 us gap. The dominant overhead must be elsewhere.

### Condition 2: Bypass GPU state Mutex in hot path (h-main, combined)
**Intent:** The `dma_copy_to_device_async` call in `gpu-services/v0/src/lib.rs:632` acquires a Mutex lock on the GPU services state to check `state.initialized`. For the hot path (128 calls per 16 MiB transfer), replace this with an `AtomicBool` check, eliminating the Mutex acquisition.

**What changes:**
- `gpu-services/v0/src/lib.rs:632`: Replace `self.state().lock()` with an `AtomicBool` field `is_initialized` checked via `Ordering::Acquire`
- This eliminates 128 Mutex lock/unlock operations per transfer on the GPU services component

### Condition 3: Batched stream synchronization (h-main, combined)
**Intent:** Replace per-chunk alternate-stream synchronization with batched synchronization every N chunks (similar to the non-zero-copy `pipelined_ssd_to_gpu` which syncs every `ring_size` chunks). Since NVMe read latency (~50-100 us per 128 KiB chunk) far exceeds GPU H2D copy time (~5 us per 128 KiB at PCIe Gen4), the GPU copies complete well before ring slots need reuse.

**What changes:**
- `pipeline.rs:350-355`: Replace per-completion `gpu.stream_synchronize(prev_stream)` with a check: only sync both streams when `(completed + 1) % SYNC_INTERVAL == 0` where SYNC_INTERVAL equals ZERO_COPY_DEPTH or half of it
- This reduces cudaStreamSynchronize calls from 128 to 4-8 per 16 MiB transfer

### Condition 4: Pre-allocated DmaBuffer array (eliminate per-call allocation)
**Intent:** The current code creates 128 `Arc<Mutex<DmaBuffer>>` wrappers at the start of every `pipelined_ssd_to_gpu_zero_copy` call (`pipeline.rs:277-288`). Move these wrappers into the `PipelineRing` struct so they're created once during initialization and reused across calls.

**What changes:**
- `pipeline.rs:27-31` (PipelineRing): Add a `chunk_wrappers: Vec<Arc<Mutex<DmaBuffer>>>` field with ZERO_COPY_DEPTH pre-allocated wrappers
- `pipeline.rs:247-388` (pipelined_ssd_to_gpu_zero_copy): Accept pre-allocated wrappers from PipelineRing; only update the internal pointer of each DmaBuffer at call start rather than allocating new ones
- `pipeline.rs:384-386`: Remove the `mem::forget` loop since wrappers are owned by PipelineRing

### Condition 5: Combined optimization (all of above)
Apply conditions 2, 3, and 4 together to measure cumulative effect.

## Success Criteria

- **Primary:** cold_16384KiB mean throughput increases from ~1750 MB/s toward the 3250 MB/s ceiling. A successful result shows the combined optimization achieves ≥2500 MB/s (i.e., closes at least 50% of the gap).
- **Directional:** Each individual optimization shows measurable improvement in the predicted direction (lower latency, higher throughput).
- **Variance:** cold_16384KiB mean/min ratio ≤ 1.3 (variance remains controlled).

## Constraints

- Must NOT remove the component architecture (actors, channels, components)
- Must NOT add new external crate dependencies
- The `Command::ReadAsync` interface in `interfaces/src/iblock_device.rs` requires `Arc<Mutex<DmaBuffer>>` — this constraint limits how much the DmaBuffer wrapping overhead can be eliminated
- Hardware required: NVMe SSD bound to SPDK (VFIO), CUDA GPU, hugepages
- Benchmark measures 50 iterations after 5 warmup

## Prior Knowledge

This is the first iteration. No active principles exist yet.

From memory: previous experiments (p2p-batch QD=32, iter-2) showed that P2P+BatchSubmit with QD=32 achieves 27.3x speedup at 4 MiB (777 us vs 21,247 us). The dispatcher v0 batchsubmit experiment showed that gRPC overhead dominates — but dispatcher v1 uses direct function calls, not gRPC. The v1 pipeline experiments confirmed that cudaHostAlloc staging gives no improvement when NVMe read dominates at 128 KiB chunks.

Key insight from prior work: at 128 KiB chunk size, NVMe read latency (~50-100 us) dominates per-chunk time. Software overhead (Mutex, stream sync) adds only a fraction per chunk, but over 128 chunks it accumulates. The real opportunity may be in reducing the NUMBER of synchronization points and memory allocations rather than per-operation overhead.
