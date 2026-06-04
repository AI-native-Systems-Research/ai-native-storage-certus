# Problem Framing — Iteration 2: GPU Sync Stall Elimination

## Research Question

The cold lookup pipeline at `pipeline.rs:244` (`pipelined_ssd_to_gpu_zero_copy`) achieves 4.53 GB/s with QD64 + 4 CUDA streams (iteration 1 best) against a raw NVMe ceiling of 5.4 GB/s per drive. The pipeline synchronizes both CUDA streams every 16 H2D copies (`pipeline.rs:381`), blocking the main completion-processing loop and creating a bubble where no NVMe completions are consumed. Can removing or relaxing this GPU sync interval close the remaining ~16% throughput gap?

The sync at line 381 blocks the thread that processes NVMe completions and submits new reads. During the sync (~167 us for 2 MiB of H2D at 12 GB/s), NVMe completions queue up unprocessed and no new reads are submitted — the NVMe device's command queue drains toward empty, reducing effective queue depth below the configured maximum.

## System Interface

- **Build:** `cargo build -p certus-server --release --features p2p`
- **Server start:** `./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2`
- **Benchmark:** `python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304`
- **Key metric:** Look for `Lookup (cold)` section → `per-client=X.XX GB/s` and `p99=XXXXX.X us`
- **Integrity check:** `python3 apps/python/certus-api-bench.py --verify-integrity`

**Code evidence for flags:**
- `--device-pci`: `apps/certus-server/src/main.rs:29` (`#[arg(long = "device-pci")]`)
- `--format`: `apps/certus-server/src/main.rs:48` (`#[arg(long = "format")]`)
- `--poller-base-cpu`: `apps/certus-server/src/main.rs:62` (`#[arg(long = "poller-base-cpu")]`)
- `--block-size`: `apps/python/certus-api-bench.py:658` (benchmark argument)
- `--verify-integrity`: `apps/python/certus-api-bench.py:677` (benchmark argument)

## Baseline Command

```bash
# Kill any existing server
kill $(ps -ef | grep "certus-server" | grep -v grep | grep -v bash | awk '{print $2}') 2>/dev/null
sleep 2

# Start server
./target/release/certus-server --device-pci 0000:61:00.0 --device-pci 0000:62:00.0 --format --poller-base-cpu 2 &
sleep 6

# Run benchmark
python3 apps/python/certus-api-bench.py --clients 1 --num-objects 16 --iterations 10 --block-size 4194304
```

## Baseline Validation

Ran the baseline command on the current system. Results from 2 consecutive runs:
- Run 1: Cold lookup per-client = 3.91 GB/s, p99 = 1213 us
- Run 2: Cold lookup per-client = 3.83 GB/s, p99 = (similar)
- Mean baseline: ~3.87 GB/s
- Server started successfully on port 50051, PID observed, zero errors.

This matches the iteration 1 baseline of 3.93 GB/s (within run-to-run variance).

## Experimental Conditions

### Condition: baseline
Current code, unmodified. QD16, 2 CUDA streams, sync every 16 H2D copies.

### Condition: h-main (Remove GPU mid-transfer sync)
Remove the periodic `stream_synchronize` at `pipeline.rs:381-386` entirely. Keep only the final sync at lines 389-393. This allows all H2D copies to be submitted without blocking the NVMe completion loop. With 32 chunks per object, removing the sync eliminates one ~167 us bubble mid-transfer.

**Safety justification:** Each NVMe read targets a unique segment of the memory-tier slot. GPU copies read from segments whose NVMe reads have already completed (checked at line 317-340). There is no data race — the sync exists only to bound GPU command queue depth, which CUDA handles internally.

### Condition: h-ablation (QD64 + sync removal)
Apply iteration 1's winning QD64 + 4 streams config AND remove the mid-transfer sync. This tests whether sync removal compounds with deeper queues. At QD64 with 32 chunks, the pipeline window is larger — removing the sync means 32 consecutive H2D copies without interruption.

Changes from baseline:
- `pipeline.rs:381-386`: Remove periodic sync
- `lib.rs:271`: Change max_queue_depth from 16 to 64
- `pipeline.rs:29`: Expand streams to `[GpuStream; 4]`, update new() to create 4 streams
- `pipeline.rs:365`: Change `% 2` to `% 4` for stream distribution
- `lib.rs:1046`: Change MAX_QUEUES_PER_DRIVE from 2 to 4
- `lib.rs:1091`: Change to `64 / num_queues`
- `lib.rs:1112`: Create 4 streams in batch_lookup path

### Condition: h-robustness (Relaxed sync interval — sync every 64)
Instead of removing the sync entirely, increase the interval from 16 to 64. This provides a safety margin (bounding GPU queue depth to 64 outstanding copies) while still eliminating the mid-transfer stall for 4 MiB objects (32 chunks < 64 threshold, so no mid-object sync occurs). Tests whether the improvement comes from eliminating per-object stalls specifically.

Changes from baseline:
- `pipeline.rs:381`: Change `% 16` to `% 64`

## Success Criteria

- **h-main**: Cold lookup throughput consistently above baseline mean (3.87 GB/s). Data integrity must PASS.
- **h-ablation (QD64 + sync removal)**: Cold lookup throughput consistently above the iteration 1 best of 4.53 GB/s. Zero NVMe errors. Data integrity PASS.
- **h-robustness**: Cold lookup throughput matches h-main (confirming that the interval increase is sufficient to eliminate per-object stalls).

Hard constraints: build must succeed, data integrity must pass, zero NVMe I/O errors.

## Constraints

- From RP-1: QD64 + 4 streams is the validated winning configuration (+15%).
- From RP-2: Keep chunk size at 128 KiB (512 KiB hurts throughput).
- From RP-3: Do NOT combine 512 KiB chunks with QD64 (causes ENOMEM).
- Server must be killed and restarted between code changes.
- Sleep 6s after server start before benchmarking.
- All conditions use 128 KiB chunk size (controller.rs:158 default).

## Prior Knowledge

- **RP-1 (high confidence)**: QD64 + 4 streams → 4.53 GB/s (+15% over 3.93 baseline).
- **RP-2 (high confidence)**: 512 KiB chunks hurt throughput (fewer pipeline segments).
- **RP-3 (medium confidence)**: QD64 + 512 KiB causes SPDK ENOMEM errors.
- **Iteration 1 observed**: The NVMe drive saturates around QD32-64 for sequential access. Further QD increases unlikely to help alone. The remaining gap (~16%) suggests a non-NVMe bottleneck — GPU sync stalls are the candidate.
