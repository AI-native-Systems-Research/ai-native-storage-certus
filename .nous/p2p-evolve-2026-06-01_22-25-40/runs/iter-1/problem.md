# Problem Framing — Cold Lookup Throughput Optimization

## Research Question

The current SSD-to-GPU cold lookup achieves ~2.4 GB/s (single drive, 4 MiB objects) against a raw NVMe ceiling of ~5.4 GB/s. The cold path uses `pipelined_ssd_to_gpu_zero_copy()` (`components/dispatcher/src/pipeline.rs:244`) which reads NVMe → pinned memory-tier DRAM → GPU via async H2D copies. The pipeline is configured with:
- **Chunk size**: 128 KiB (NVMe max_transfer_size, `components/block-device-spdk-nvme/src/controller.rs:158`)
- **Queue depth**: 16 in-flight NVMe reads (`components/dispatcher/src/lib.rs:271`)
- **CUDA streams**: 2 (hardcoded in `PipelineRing`, `components/dispatcher/src/pipeline.rs:29`)
- **GPU sync interval**: every 16 H2D copies (`components/dispatcher/src/pipeline.rs:381`)

Can increasing pipeline parallelism (deeper NVMe queue, more CUDA streams, larger effective transfers) significantly improve throughput and reduce tail latency?

## System Interface

- **Build command**: `cargo build -p certus-server --release --features p2p`
- **Server start**: `./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2`
  - `--device-pci`: NVMe PCI address(es), parsed at `apps/certus-server/src/main.rs:29`
  - `--format`: reformat extent managers, `apps/certus-server/src/main.rs:48`
  - `--poller-base-cpu`: pin NVMe poller thread to dedicated CPU core, `apps/certus-server/src/main.rs:62`
  - `--memory-tier-size`: pool size (default 2G), `apps/certus-server/src/main.rs:42`
  - gRPC listens on `0.0.0.0:50051` by default (`apps/certus-server/src/main.rs:38`)
- **Benchmark**: `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
  - Reports per-client GB/s and latency percentiles for Populate, Lookup (hot), Lookup (cold)
  - Output format: `per-client=X.XX GB/s` and `p99=XXXXX.X us`
  - Cold path forces eviction via `ClearMemoryTier` then issues lookups against SSD-backed entries

**Code evidence:**
- Pipeline ring size constant: `components/dispatcher/src/pipeline.rs:18`
- Zero-copy function signature: `components/dispatcher/src/pipeline.rs:244`
- Queue depth parameter in batch_lookup: `components/dispatcher/src/lib.rs:1091`
- max_transfer_size default 131072: `components/block-device-spdk-nvme/src/controller.rs:158`
- CUDA stream creation (2 streams): `components/dispatcher/src/pipeline.rs:46-52`
- GPU sync every 16 ops: `components/dispatcher/src/pipeline.rs:381`

## Baseline Command

```bash
# Kill any running server, rebuild, start, wait, benchmark
pkill -f certus-server || true
sleep 1
cargo build -p certus-server --release --features p2p
./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2 &
sleep 5
python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304
pkill -f certus-server
```

## Baseline Validation

Baseline to be validated by executor. Expected output: cold lookup `per-client=~2.4 GB/s` based on prior measurements. Server starts on port 50051 after ~3s SPDK initialization.

## Experimental Conditions

### Condition 1: Baseline (h-control)
Unmodified code. Run the baseline command above.

### Condition 2: Increased Queue Depth + 4 CUDA Streams (h-main)

**Changes to `components/dispatcher/src/pipeline.rs`:**
1. Change `PIPELINE_RING_SIZE` from 8 to 32 (line 18)
2. Expand the `streams` array from `[GpuStream; 2]` to `[GpuStream; 4]` (line 29) and create 4 streams in `PipelineRing::new()`

**Changes to `components/dispatcher/src/lib.rs`:**
1. In `promote_and_serve()` around line 271: change `max_queue_depth` argument from `16` to `64`
2. In `batch_lookup()`: change `MAX_QUEUES_PER_DRIVE` from 2 to 4 (line 1046), and set `queue_depth = 64 / num_queues` (line 1091)

**Rationale:** With 128 KiB chunks and queue depth 16, only 2 MB of NVMe I/O can be in-flight. At 5.4 GB/s, that's 0.37 ms to drain — barely enough to hide the NVMe submission latency. Increasing to QD64 puts 8 MB in flight, fully saturating the drive's internal parallelism. 4 CUDA streams allow more H2D copy overlap with NVMe reads.

### Condition 3: Larger Chunk Size (512 KiB) (h-ablation)

**Changes to `components/block-device-spdk-nvme/src/controller.rs`:**
1. Change `max_transfer_size` default from 131072 to 524288 (line 158)

**Rationale:** Tests whether larger individual NVMe commands (fewer but bigger) improve throughput by reducing per-command overhead, independent of queue depth changes. This isolates the chunk size variable from the queue depth variable.

### Condition 4: Combined — QD64 + 4 Streams + 512 KiB Chunks (h-super-additivity)

Apply all changes from Conditions 2 and 3 simultaneously.

**Rationale:** Tests whether the combined effect of deeper queue depth, more streams, and larger chunks exceeds the sum of individual improvements (super-additivity from simultaneously reducing overhead at multiple pipeline stages).

## Success Criteria

- **h-main (QD64 + 4 streams)**: Cold lookup throughput increases consistently above baseline (directional, expect >30% given NVMe queue depth sensitivity)
- **h-ablation (512 KiB chunks)**: Measurable throughput improvement demonstrating chunk size contribution
- **h-super-additivity (combined)**: Throughput exceeds the individual gains of h-main and h-ablation combined
- **Hard constraints**: Build succeeds, data integrity passes (verify with `--verify-integrity`)
- **Scoring**: `score = 0.60 * min(1.0, throughput_gbps / 12.0) + 0.40 * min(1.0, 0.4 / p99_latency_ms)`

## Constraints

- Single NVMe Gen4 drive (0000:61:00.0 for data), raw ceiling ~5.4 GB/s
- GPU: NVIDIA A30, PCIe Gen4 x16 (bidirectional 32 GB/s, unidirectional ~25 GB/s — not the bottleneck)
- Memory-tier pool: 2 GiB default
- Server must pass data integrity verification
- Must not regress hot-path (memory-tier) latency

## Prior Knowledge

This is the first iteration. No active principles apply yet.
