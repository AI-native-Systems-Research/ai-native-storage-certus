# Data Model: IOPS Benchmark Example Application

**Date**: 2026-04-15

## Entities

### BenchConfig

Configuration parameters for a benchmark run. Parsed from CLI arguments, validated against device properties.

| Field | Type | Default | Constraints |
|-------|------|---------|-------------|
| op | OpType | Read | One of: Read, Write, ReadWrite |
| block_size | usize | 4096 | > 0, multiple of device sector size |
| queue_depth | u32 | 32 | >= 1, clamped to device max |
| threads | u32 | 1 | >= 1 |
| duration_secs | u64 | 10 | >= 1 |
| ns_id | u32 | 1 | Must exist on device |
| pci_addr | Option\<String\> | None | Valid BDF format if provided |
| pattern | Pattern | Random | One of: Random, Sequential |
| quiet | bool | false | — |

**Lifecycle**: Created once at startup, immutable after validation. Shared across all threads via `Arc<BenchConfig>`.

### OpType (enum)

| Variant | Behavior |
|---------|----------|
| Read | All ops are reads |
| Write | All ops are writes |
| ReadWrite | 50/50 random mix of reads and writes |

### Pattern (enum)

| Variant | Behavior |
|---------|----------|
| Random | Uniform random LBA per operation |
| Sequential | Contiguous LBAs, each thread gets a non-overlapping region, wraps at end |

### ThreadResult

Per-thread statistics collected during the benchmark run. Produced by each worker thread upon completion.

| Field | Type | Description |
|-------|------|-------------|
| read_ops | u64 | Number of successful read completions |
| write_ops | u64 | Number of successful write completions |
| errors | u64 | Number of IO errors encountered |
| latencies_ns | Vec\<u64\> | Per-op latency in nanoseconds |

**Lifecycle**: Created empty at thread start, populated during IO loop, returned on thread join.

### FinalReport

Aggregated results from all threads. Computed once after all threads have joined.

| Field | Type | Description |
|-------|------|-------------|
| total_read_ops | u64 | Sum of read_ops across threads |
| total_write_ops | u64 | Sum of write_ops across threads |
| total_errors | u64 | Sum of errors across threads |
| duration_secs | f64 | Actual measured duration |
| block_size | usize | From config (for throughput calc) |
| read_iops | f64 | total_read_ops / duration_secs |
| write_iops | f64 | total_write_ops / duration_secs |
| total_iops | f64 | (total_read_ops + total_write_ops) / duration_secs |
| throughput_mbps | f64 | total_iops * block_size / 1_048_576 |
| lat_min_us | f64 | Minimum latency across all samples |
| lat_mean_us | f64 | Mean latency across all samples |
| lat_p50_us | f64 | 50th percentile latency |
| lat_p99_us | f64 | 99th percentile latency |
| lat_max_us | f64 | Maximum latency across all samples |

**Lifecycle**: Constructed from `Vec<ThreadResult>` after all workers join. Used once for final output.

## Relationships

```
BenchConfig ──1:N──▶ Worker Thread ──1:1──▶ ThreadResult
                                                │
                                         N:1 aggregate
                                                ▼
                                          FinalReport
```

- One `BenchConfig` is shared (read-only) across N worker threads.
- Each worker thread produces exactly one `ThreadResult`.
- All `ThreadResult`s are aggregated into one `FinalReport`.

## External Types (from component)

These types are used but not owned by the benchmark:

- `ClientChannels { command_tx, completion_rx }` — obtained from `IBlockDevice::connect_client()`
- `Command::ReadAsync`, `Command::WriteAsync` — sent on `command_tx`
- `Completion::ReadDone`, `Completion::WriteDone`, `Completion::Timeout`, `Completion::Error` — received on `completion_rx`
- `DmaBuffer` — allocated once per in-flight slot, reused across ops
- `OpHandle` — component-assigned, used to correlate completions with start times
- `NamespaceInfo { ns_id, num_sectors, sector_size }` — from NsProbe
