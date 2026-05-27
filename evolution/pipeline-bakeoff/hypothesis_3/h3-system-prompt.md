# H3 System Prompt — Multi-Client Concurrent Throughput

## Background: Why H3 Exists

H1 (single-client, fixed 4 MiB) found that ZERO_COPY_DEPTH=32 is essentially the
only meaningful tunable in pipeline.rs. Beyond that, single-client throughput hits
the NVMe ceiling (~5.2 GB/s). The real bottleneck for multi-client workloads is
the outer Mutex serialization in the dispatcher component (lib.rs).

H2 (mixed workload sizes) confirmed that pipeline.rs constants are already at their
sweet spot for single-client throughput across all object sizes.

H3 shifts the optimization target from pipeline constants to architectural contention.

## The Bottleneck: Mutex Serialization

In `components/dispatcher/src/lib.rs`, the `DispatcherComponent` holds:

```rust
fields: {
    data_drives: Mutex<Vec<DataDrive>>,
    pipeline_ring: Mutex<Option<pipeline::PipelineRing>>,
    warm_stream: AtomicU64,   // only this one is lock-free
}
```

When a cold lookup arrives (`lookup_async` -> `promote_and_serve`):
1. `self.data_drives.lock()` — serializes drive access across ALL clients
2. `self.pipeline_ring.lock()` — serializes the entire NVMe→DRAM→GPU pipeline

With 8 concurrent clients doing cold lookups:
- Only 1 client runs the pipeline at a time
- The other 7 wait on the mutex
- Effective throughput = single-client throughput (not 8x!)

The warm path (MemoryTier hit) is already lock-free via `warm_stream` (AtomicU64),
so warm hits scale well. The problem is exclusively cold lookups.

## Hardware Context

- 7x NVMe Gen4 SSDs (PCIe addresses 0000:62-64, 0000:c1-c3)
- NVIDIA A30 GPU (PCIe Gen4 x16)
- Each NVMe supports QD=32 at 128 KiB MDTS
- Per-drive sequential read: ~3.5 GB/s
- Aggregate NVMe bandwidth (7 drives): ~24.5 GB/s theoretical
- GPU PCIe Gen4 x16 bandwidth: ~25 GB/s

## What H3 Can Evolve

Three files are in scope:
1. **service.rs** (`apps/certus-server/src/service.rs`) — The gRPC handler with the OUTERMOST
   `Arc<Mutex<Arc<dyn IDispatcher>>>` that serializes ALL requests (THE #1 bottleneck)
2. **lib.rs** (`components/dispatcher/src/lib.rs`) — The dispatcher component with Mutex<> fields
3. **pipeline.rs** (`components/dispatcher/src/pipeline.rs`) — NVMe->DRAM->GPU transfer logic

## Architectural Opportunities

### A. Drive-Sharded Pipeline Rings
Instead of one shared `Mutex<Option<PipelineRing>>`, allocate N rings (one per drive).
Keys are already sharded by `drive_index = key % num_drives`, so cold lookups to
different drives never need to contend.

### B. Lock-Free Ring Pool
Use `crossbeam::ArrayQueue<PipelineRing>` or similar. Clients pop a ring, use it,
push it back. No mutex contention — just atomic CAS.

### C. Per-Client CUDA Streams
Currently `warm_stream` is a single shared AtomicU64. With 8 clients doing async
H2D copies on the SAME stream, they serialize on the GPU too. Allocating per-client
streams (or a stream pool) enables true GPU-side parallelism.

### D. RwLock for Read-Heavy Paths
`data_drives` is read-only after initialization. Replace `Mutex<Vec<DataDrive>>`
with `RwLock<Vec<DataDrive>>` so multiple readers never block each other.
NOTE: The `define_component!` macro only supports `Mutex<T>` syntax, so this
requires wrapping in `Arc<RwLock<Vec<DataDrive>>>` inside the Mutex.

### E. Batched Dispatch
Collect multiple concurrent cold lookup requests and dispatch them together,
using all 7 drives in parallel for maximum aggregate bandwidth.

### F. Async/Tokio Integration
Replace blocking Mutex with `tokio::sync::Mutex` or `tokio::sync::RwLock` to
allow the runtime to schedule other work while waiting for the pipeline.

## Constraints

- Must compile: `cargo build -p certus-server --release`
- Must pass data integrity verification (correct data delivered to GPU)
- Must maintain IDispatcher trait signatures (lookup, lookup_async, populate, etc.)
- Must use the interfaces crate types (Command, Completion, DmaBuffer, GpuStream)
- Function signatures called from outside (IDispatcher impl) cannot change
- The `define_component!` macro has limited flexibility — work within or around it

## Evaluation

The evaluator runs with `--eval concurrent`:
- Starts certus-server with ALL 6 data drives
- Runs `certus-api-bench.py --clients 8 --num-objects 16 --iterations 10 --block-size 4194304`
- Measures aggregate cold lookup throughput (GB/s) across 8 concurrent clients
- Score = aggregate throughput

Baseline (current): ~5 GB/s aggregate with 8 clients (same as 1 client due to mutex)
Target: approach 7x single-drive throughput = ~24 GB/s aggregate (NVMe ceiling)
Realistic target: 15-20 GB/s (accounting for shared GPU PCIe and contention)

## Pipeline Constants (Already Optimized)

From H1 ablation:
- ZERO_COPY_DEPTH=32-48 is optimal for single-drive (already set to 48)
- SYNC_FREQUENCY=16 with adaptive logic is optimal
- 2 CUDA streams per pipeline instance is optimal

The opportunity is NOT in changing these constants — it's in removing the mutex
that prevents multiple instances from running simultaneously.
