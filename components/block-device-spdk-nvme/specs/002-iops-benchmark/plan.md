# Implementation Plan: IOPS Benchmark Example Application

**Branch**: `002-iops-benchmark` | **Date**: 2026-04-15 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/002-iops-benchmark/spec.md`

## Summary

Build a command-line IOPS benchmark application (`apps/iops-benchmark`) that measures NVMe read/write throughput using the block-device-spdk-nvme component. The application accepts configurable parameters (operation type, block size, queue depth, threads, duration, PCI address, namespace, access pattern) with sensible defaults, uses async IO to keep the pipeline full, and reports IOPS, throughput (MB/s), and latency percentiles (min, mean, p50, p99, max) to stdout.

## Technical Context

**Language/Version**: Rust stable (edition 2021, MSRV 1.75)
**Primary Dependencies**: block-device-spdk-nvme v1, component-framework, spdk-env, interfaces, example-logger, clap (CLI argument parsing), rand (random LBA generation)
**Storage**: N/A (benchmark tool, no persistent state)
**Testing**: `cargo test`, `cargo clippy`, `cargo fmt --check`, `cargo doc --no-deps`
**Target Platform**: Linux (SPDK requires hugepages + VFIO/UIO)
**Project Type**: CLI binary application
**Performance Goals**: Maximize measured IOPS to within 10% of fio under equivalent conditions
**Constraints**: Must run on same host as NVMe device; SPDK environment must be pre-initialized; channel capacity is 64 slots
**Scale/Scope**: Single binary, ~800-1200 lines of Rust

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Correctness First | PASS | Unit tests for config validation, stats aggregation, LBA generation. No unsafe code in the benchmark app itself (unsafe is in the component). |
| II. Comprehensive Testing | PASS | Unit tests for all pure-logic modules (config, stats, LBA patterns). Doc tests for public types. Integration test requires hardware (skipped when unavailable). |
| III. Performance Accountability | PASS | The application IS the benchmark. No separate Criterion bench needed — the app's output IS the performance measurement. |
| IV. Documentation as Contract | PASS | Public types (BenchConfig, ThreadResult, FinalReport) will have doc comments. `--help` provides CLI documentation. |
| V. Maintainability | PASS | `cargo fmt` + `cargo clippy` enforced. Single-purpose modules. Minimal dependencies (clap, rand). |
| VI. Component-Framework Conformance | PASS | Uses IBlockDevice interface via `query()`, connects via `connect_client()`, uses receptacle wiring. |

No violations to justify.

## Project Structure

### Documentation (this feature)

```text
specs/002-iops-benchmark/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── cli-contract.md  # CLI interface contract
└── tasks.md             # Phase 2 output (from /speckit.tasks)
```

### Source Code (repository root)

```text
apps/iops-benchmark/
├── Cargo.toml
├── src/
│   ├── main.rs          # Entry point: parse args, wire components, launch threads, report
│   ├── config.rs        # BenchConfig struct, CLI parsing, validation
│   ├── worker.rs        # Per-thread IO worker loop (async submission + completion drain)
│   ├── stats.rs         # ThreadResult, FinalReport, latency histogram, aggregation
│   ├── lba.rs           # LBA generation strategies (random uniform, sequential non-overlapping)
│   └── report.rs        # Human-readable output formatting (config summary, progress, final)
└── tests/
    └── integration.rs   # End-to-end test (skipped without hardware)
```

**Structure Decision**: Single binary crate under `apps/` following the existing `helloworld-mainline` pattern. Modules are split by concern: config parsing, IO worker, statistics, LBA generation, and output formatting. No library crate needed — this is a standalone tool.

## Detailed Design

### Module: `config.rs`

Defines `BenchConfig` with all CLI parameters. Uses `clap` derive API for parsing. Validation method checks:
- `block_size > 0` and `block_size % sector_size == 0`
- `threads >= 1`, `duration >= 1`, `queue_depth >= 1`
- Clamps `queue_depth` to device max with warning to stderr

```
BenchConfig {
    op: OpType,              // enum { Read, Write, ReadWrite }
    block_size: usize,       // bytes, default 4096
    queue_depth: u32,        // default 32
    threads: u32,            // default 1
    duration_secs: u64,      // default 10
    ns_id: u32,              // default 1
    pci_addr: Option<String>,// e.g. "0000:03:00.0", default None (first device)
    pattern: Pattern,        // enum { Random, Sequential }, default Random
    quiet: bool,             // default false
}
```

### Module: `worker.rs`

Each thread runs a `Worker` struct that:
1. Calls `ibd.connect_client()` to get its own `ClientChannels`
2. Probes namespaces to get sector count for LBA range
3. Pre-allocates `queue_depth` DMA buffers (reused via ring of `Arc` handles)
4. Runs a tight loop:
   - Submit async commands up to `queue_depth` outstanding
   - Call `flush_io()` to wake the actor
   - Drain completions from `completion_rx` (non-blocking `try_recv` loop)
   - Record latency for each completion (Instant-based)
   - Re-submit to keep pipeline full
   - Check `AtomicBool` stop flag each iteration
5. Returns `ThreadResult` when stopped

**Key design**: Use `ReadAsync`/`WriteAsync` (not sync) to overlap submissions. Each in-flight op tracks its `Instant` start time in a local `HashMap<OpHandle, Instant>`. On completion, compute `elapsed` and record in stats.

The channel capacity is 64 slots. If `queue_depth > 64`, the worker batches submissions: send up to 64, flush, drain some completions, send more. The effective pipeline depth is `min(queue_depth, 64)` per flush cycle.

### Module: `stats.rs`

`ThreadResult`: per-thread counters (read_ops, write_ops, errors) + Vec of latency samples (in nanoseconds).

`FinalReport`: aggregated from all `ThreadResult`s. Computes:
- Total IOPS = total_ops / duration_secs
- Read IOPS, Write IOPS (for rw mode)
- Throughput MB/s = total_ops * block_size / duration_secs / 1MB
- Latency percentiles: sort all samples, pick indices for p50, p99
- Min, max, mean from sorted samples

For high-IOPS runs (millions of samples), use a pre-allocated `Vec<u64>` per thread. Sorting happens once at the end, which is acceptable for report generation.

### Module: `lba.rs`

Two strategies:
- `RandomLba`: uses `rand::thread_rng().gen_range(0..max_lba)` where `max_lba = num_sectors - blocks_per_io`
- `SequentialLba`: each thread gets a contiguous region `[start..end)` of the namespace, wraps around at end. Region = `namespace_sectors / num_threads`, offset by `thread_index * region_size`.

### Module: `report.rs`

Three output functions:
- `print_config(config, device_info)` → config summary to stdout
- `print_progress(elapsed_secs, instant_iops)` → one-line to stderr (unless quiet)
- `print_final(report)` → formatted table to stdout with IOPS, throughput, latency stats

### Module: `main.rs`

Entry point flow:
1. Parse CLI args → `BenchConfig`
2. Create and wire components (logger, spdk_env, block_dev) following bench/latency.rs pattern
3. Initialize SPDK env, select device (by `--pci-addr` or first available)
4. Set PCI address, initialize block device
5. Validate config against device properties (sector size, max queue depth)
6. Print config summary
7. Create `Arc<AtomicBool>` stop flag
8. Spawn `config.threads` worker threads, each gets its own `connect_client()` channel pair
9. Spawn a timer thread that sets stop flag after `duration_secs`
10. If not quiet: main thread prints progress every 1 second (polls thread-local counters via `Arc<AtomicU64>`)
11. Join all worker threads, collect `ThreadResult`s
12. Aggregate into `FinalReport`, print final summary

## Complexity Tracking

No constitution violations to justify. The application is straightforward: CLI parsing, component wiring, multi-threaded IO loop, statistics aggregation.
