# Contract: Benchmark Suite

**Feature**: 004-channel-benchmarks
**Date**: 2026-03-31

## Benchmark Organization

All benchmarks use Criterion 0.5 and live in `crates/component-framework/benches/`.

### Benchmark Files

| File | Purpose | Groups |
|------|---------|--------|
| `channel_spsc_benchmark.rs` | SPSC throughput comparison | spsc_throughput_small, spsc_throughput_large |
| `channel_mpsc_benchmark.rs` | MPSC throughput comparison | mpsc_throughput_2p, mpsc_throughput_4p, mpsc_throughput_8p |
| `channel_latency_benchmark.rs` | Per-message latency | spsc_latency, mpsc_latency |

### SPSC Benchmark Group

**Backends**: built-in SpscChannel, CrossbeamBoundedChannel, CrossbeamUnboundedChannel, KanalChannel, RtrbChannel

**Parameters**:
- Message count: 100,000
- Message sizes: u64 (8 bytes), Vec<u8> (1024 bytes)
- Queue capacities: 64, 1024, 16384
- Producers: 1, Consumers: 1

**Pattern**:
```
1 producer thread → channel → 1 consumer thread
Producer sends N messages; consumer receives N messages
Measure wall-clock time for full transfer
```

### MPSC Benchmark Group

**Backends**: built-in MpscChannel, CrossbeamBoundedChannel, CrossbeamUnboundedChannel, KanalChannel, TokioMpscChannel

**Note**: rtrb excluded (SPSC only)

**Parameters**:
- Message count: 100,000 total (evenly distributed across producers)
- Message sizes: u64 (8 bytes), Vec<u8> (1024 bytes)
- Queue capacities: 64, 1024, 16384
- Producer counts: 2, 4, 8

**Pattern**:
```
N producer threads → channel → 1 consumer thread
Each producer sends count/N messages
Consumer receives count messages total
Measure wall-clock time for full transfer
```

### Latency Benchmark

**Method**: Single-message round-trip timing using `std::time::Instant`

**Pattern**:
```
For each iteration:
  record start = Instant::now()
  sender.send(message)
  receiver.recv()
  record elapsed = start.elapsed()
```

### Benchmark Naming Convention

```
{topology}/{backend}/{message_size}/capacity_{n}
```

Examples:
- `spsc/builtin/u64/capacity_1024`
- `mpsc/crossbeam_bounded/vec1024/capacity_4096/producers_4`
- `latency/spsc/kanal/u64`

### Running Benchmarks

```bash
# All channel benchmarks
cargo bench --bench channel_spsc_benchmark
cargo bench --bench channel_mpsc_benchmark
cargo bench --bench channel_latency_benchmark

# Specific group
cargo bench --bench channel_spsc_benchmark -- spsc_throughput_small
```

### Output

Criterion produces HTML reports in `target/criterion/` with:
- Statistical analysis (mean, median, std dev)
- Comparison across runs (regression detection)
- Throughput calculations
