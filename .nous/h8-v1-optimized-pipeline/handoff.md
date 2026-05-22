# Handoff — h8-v1-optimized-pipeline iter-1

## Goal

Implement and measure a parallelized NVMe read pipeline in dispatcher v1's `pipelined_ssd_to_gpu` function, comparing it against the current sequential ReadSync baseline for 4 MiB SSD-tier lookups. The experiment has three conditions: baseline (sequential), optimized (parallel NVMe + async GPU), and ablation (parallel NVMe + sync GPU).

## Key Discoveries

1. **Sequential bottleneck identified**: `pipeline.rs:67` uses `Command::ReadSync` — each of 32 chunks (128 KiB MDTS) blocks until NVMe completion before the next is issued. At ~600us/chunk, this creates a ~19,200us serial floor for 4 MiB.

2. **ReadAsync and BatchSubmit available in block-device-spdk-nvme v2** (default for dispatcher v1): `interfaces/src/iblock_device.rs:205` defines `ReadAsync { ns_id, lba, buf, timeout_ms }`. The block device v2 actor handles it at `block-device-spdk-nvme/v2/src/actor.rs:568`.

3. **Reference implementation achieves ~3.2 GB/s** with ring_size=32 and parallel ReadAsync: `apps/gpu-bb-vs-p2p/src/main.rs:198-328`. Pattern: prime ring with N async reads → process completions → cudaMemcpyAsync on alternating streams → sync previous stream → resubmit.

4. **CUDA FFI already available** in dispatcher's dep tree: `gpu-services` crate exports `pub mod cuda_ffi` at `components/gpu-services/v0/src/lib.rs:26`. The dispatcher Cargo.toml includes `gpu-services = { workspace = true, features = ["spdk"] }`.

5. **IGpuServices::dma_copy_to_device is synchronous** (`igpu_services.rs:400`): "Performs a synchronous cudaMemcpy with HostToDevice direction." For async GPU copies, must call `cudaMemcpyAsync` directly via `gpu_services::cuda_ffi`.

6. **Memory-tier pool is CUDA-pinned** by certus-server: `apps/certus-server/src/main.rs:192` calls `cudaHostRegister` on the memory-tier pool. This means memcpy from memory-tier to GPU uses DMA (truly async when using cudaMemcpyAsync).

7. **Benchmark amortizes gRPC overhead**: The `test_client.py --bench-only` sends batch lookups of `num_objects` per iteration, measuring `(total_time / num_objects)`. This correctly isolates per-object pipeline latency from gRPC round-trip overhead.

## System Interface

- **Build:** `cargo build -p certus-server -p dispatcher-v1` (requires SPDK at deps/spdk-build/)
- **Run server:** `sudo ./target/debug/certus-server --metadata-pci <ADDR> --data-pci <ADDR> --dispatcher-version v1`
- **Run benchmark:** `cd apps/certus-server/python-client && python3 test_client.py --bench-only --bench-object-size 4194304 --bench-num-objects 20 --bench-iterations 5`
- **Output format:** Printed to stdout — `SSD-tier` row with `Avg (us/obj)` column is the primary metric
- **Baseline result:** Expected ~19,500–23,000 us per 4 MiB object (from prior experiment memory, ±20% variance)

## Code Map

| Location | What | When to check |
|----------|------|---------------|
| `components/dispatcher/v1/src/pipeline.rs:30-123` | `pipelined_ssd_to_gpu()` — THE function to modify | Primary target for both arms |
| `components/dispatcher/v1/src/pipeline.rs:16` | `PIPELINE_RING_SIZE = 4` — increase to 8 | Ring size constant |
| `components/dispatcher/v1/src/lib.rs:240-253` | `promote_and_serve()` calls `pipeline::pipelined_ssd_to_gpu()` | Verify call site unchanged |
| `components/interfaces/src/iblock_device.rs:205-213` | `Command::ReadAsync` definition | Verify field names/types |
| `apps/gpu-bb-vs-p2p/src/main.rs:198-328` | Reference `pipelined_transfer()` | Pattern to follow |
| `components/gpu-services/v0/src/cuda_ffi.rs` | CUDA FFI bindings | For cudaMemcpyAsync, cudaStreamCreate etc. |
| `apps/certus-server/src/main.rs:192` | cudaHostRegister on memory-tier pool | Verify pinning still works |
| `apps/certus-server/python-client/test_client.py:310-439` | `bench_lookup_latency()` | Benchmark logic |

## Code Targets

### h-main: Full optimized pipeline

**File:** `components/dispatcher/v1/src/pipeline.rs`
**Function:** `pipelined_ssd_to_gpu()` (line 30)
**What to change:** Replace the sequential loop (lines 60-119) with:
1. Change `PIPELINE_RING_SIZE` from 4 to 8 (line 16)
2. After allocating ring buffers (line 50-58), create 2 CUDA streams
3. Prime the ring: issue `Command::ReadAsync` for first N segments
4. Main loop: recv completion → copy to mem_tier → `cudaMemcpyAsync` on `stream[i%2]` → sync `stream[(i+1)%2]` → resubmit ReadAsync
5. Final: sync both streams

**WHY this location:** This is the only function that reads from SSD during lookup. The `promote_and_serve` method (lib.rs:244) calls it directly. Changing this function transparently optimizes all SSD-tier lookups without touching any other component.

### h-ablation: NVMe parallelism only

**File:** Same `pipeline.rs`
**What to change:** Same ReadAsync primed-ring pattern, but keep `gpu.dma_copy_to_device()` call (line 109-117) instead of cudaMemcpyAsync. No CUDA streams needed.

**WHY:** Isolates the NVMe parallelism benefit. If this gives similar improvement to h-main, confirms RP-16 (NVMe dominates, GPU copy is negligible).

## What I Tried That Didn't Work

- Previous experiments (memory `project_v1_pipeline_iter2.md`) tried cudaHostAlloc staging for async GPU copies without parallel NVMe reads — **no improvement** because NVMe read time dominates.
- Previous experiment (memory `project_evolve_v0_batchsubmit_iter2.md`) tried BatchSubmit through the full gRPC stack — **no improvement** because gRPC + connect_client overhead dominated. However, THIS experiment is different because we're modifying the pipeline.rs internal loop (no gRPC overhead in the measurement path).

## What I Excluded and Why

- **P2P (GDRCopy) path**: Would require GDRCopy infrastructure, kernel modules, specific PCIe topology. The campaign asks for pipeline optimization within existing components, not hardware path changes.
- **BatchSubmit command**: Could bundle all 32 reads into one command, but ReadAsync with primed ring achieves the same effect more naturally (matches reference implementation, allows per-completion processing).
- **Changing ring buffer allocation to cudaHostAlloc**: Would enable truly async H2D via DMA engine. But certus-server already calls `cudaHostRegister` on the memory-tier pool, and the DMA buffers are SPDK-allocated (already DMA-capable). The pipeline copies to memory-tier first, then from memory-tier to GPU — the memory-tier is already pinned.
- **Larger object sizes**: Campaign specifies 4 MiB.

## Evolution of Thinking

Initial assumption was that the main bottleneck might be lack of GPU async copies (pipeline overlap). But reading the prior experiment memories (RP-16) revealed that NVMe read dominance is the binding constraint — GPU copies are only ~5-50us per chunk while NVMe reads are ~600us. The real opportunity is parallelizing the 32 sequential NVMe reads, not overlapping them with GPU copies.

The gpu-bb-vs-p2p reference confirms this: it achieves 3.2 GB/s = ~1,300us for 4 MiB, compared to sequential ~19,200us. That's a ~15x improvement, which can only come from NVMe internal parallelism (not GPU overlap which is capped at <8% improvement).

## Current Status

- **Validated:** pipeline.rs code structure understood; ReadAsync interface available; CUDA FFI available; benchmark CLI validated; prior experiment results establish baseline expectations (19,500-23,000us)
- **Uncertain:** Exact magnitude of NVMe internal parallelism at QD=8 on this specific SSD model. The reference used ring_size=32 and QD=32 (actor-level), but through dispatcher v1's connect_client() channel the effective QD may be limited.
- **Suggested next:** If parallel reads through connect_client() don't yield expected speedup, investigate whether the block device actor serializes ReadAsync requests from a single client channel. May need to use BatchSubmit instead of individual ReadAsync commands for true queue-depth parallelism.

## Warnings & Constraints

1. **SPDK symlink required in worktrees**: Previous experiments needed `ln -sf <parent>/deps/spdk <worktree>/deps/spdk` and same for `deps/spdk-build/`. Without this, the build fails.

2. **Server must restart between conditions**: Different code means rebuilding and restarting certus-server. Ensure fresh process and fresh populate cycle for each condition.

3. **Ring buffer Mutex**: `ReadAsync` requires `Arc<Mutex<DmaBuffer>>` — the ring already uses this type. However, the Mutex must be held during NVMe read (the actor locks it). After completion, the pipeline code must lock it to copy data. This should work naturally but watch for deadlocks if the actor holds the lock during the callback.

4. **cudaMemcpyAsync requires CUDA stream FFI**: The `cuda_ffi` module in gpu-services may not export cudaStreamCreate/cudaMemcpyAsync/cudaStreamSynchronize. Check and add extern declarations if needed (like gpu-bb-vs-p2p does at lines 33-44).

5. **Memory-tier copy must happen BEFORE resubmitting ReadAsync**: The ring buffer is reused after resubmission, so the mem_tier copy must complete before the buffer is passed back to NVMe. The current pattern handles this correctly.

6. **num_objects=20 for 4 MiB benchmark**: At 4 MiB per object, 20 objects = 80 MiB of SSD data. The memory-tier pool is 256 MiB, so we need to populate (256 MiB / 4 MiB) + 20 = 84 objects to force the first 20 to SSD. The test_client handles this automatically.

7. **System variance ±20%**: Run at least 5 iterations per condition. Compare medians rather than means for robustness.
