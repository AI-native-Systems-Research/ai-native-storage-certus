# Research: IOPS Benchmark Example Application

**Date**: 2026-04-15

## R1: Async IO Submission Pattern

**Decision**: Use `ReadAsync`/`WriteAsync` commands with per-op `Instant` tracking in a local `HashMap<OpHandle, Instant>` for latency measurement.

**Rationale**: The component assigns `OpHandle` on async submission. Async commands allow overlapping submission and completion, which is essential for saturating the NVMe device at high queue depths. Sync commands block the thread per-op, limiting throughput to 1 outstanding IO at a time.

**Alternatives considered**:
- `ReadSync`/`WriteSync` per-op: Simple but limited to QD=1 effective throughput per thread. Rejected because the spec requires configurable queue depth.
- `BatchSubmit` with sync ops inside: Batches are unpacked and executed individually by the actor; this is equivalent to sending individual commands. No benefit over async.

## R2: Channel Capacity vs Queue Depth

**Decision**: Cap effective in-flight ops at `min(queue_depth, CLIENT_CHANNEL_CAPACITY)` per flush cycle. The channel capacity is 64 slots (hardcoded in `lib.rs` as `CLIENT_CHANNEL_CAPACITY`).

**Rationale**: The `command_tx` channel is bounded to 64 slots. Attempting to send more than 64 commands without draining completions will block. The worker loop must interleave send and receive to keep the pipeline full without blocking.

**Alternatives considered**:
- Increase channel capacity: Would require modifying the component code, which is out of scope for a benchmark app.
- Use multiple connections per thread: Adds complexity; rejected because a single connection per thread with proper pipelining is sufficient.

## R3: Latency Sample Storage

**Decision**: Use a pre-allocated `Vec<u64>` per thread, push nanosecond latencies. Sort once at end for percentile computation.

**Rationale**: For a 10-second run at 500K IOPS, that's ~5M samples × 8 bytes = 40MB per thread — acceptable for a benchmark tool. Sorting 5M u64s takes ~200ms, negligible compared to benchmark duration.

**Alternatives considered**:
- HdrHistogram crate: More memory-efficient but adds a dependency for marginal benefit. The benchmark is a short-lived tool, not a long-running service.
- Reservoir sampling: Loses precision on tail latencies (p99). Rejected.

## R4: CLI Parsing Library

**Decision**: Use `clap` with derive API for argument parsing.

**Rationale**: `clap` is the de facto standard for Rust CLI tools. The derive API generates `--help`, validation, and type conversion with minimal boilerplate. It's widely used and well-maintained.

**Alternatives considered**:
- Manual `std::env::args()` parsing: Error-prone, no automatic `--help` generation.
- `structopt`: Deprecated in favor of clap derive.

## R5: Random Number Generation

**Decision**: Use `rand` crate with `thread_rng()` (ThreadRng, ChaCha-based) for LBA generation.

**Rationale**: `thread_rng()` is fast enough that RNG overhead is negligible compared to IO latency. Uniform distribution over the namespace LBA range is required by the spec.

**Alternatives considered**:
- `fastrand`: Slightly faster but less widely used. The difference is immaterial for this use case.
- Pre-generated LBA table: Wastes memory and doesn't improve throughput.

## R6: Progress Reporting Mechanism

**Decision**: Each worker thread maintains `Arc<AtomicU64>` counters for completed ops. The main thread reads these every second for progress output.

**Rationale**: Atomic counters have near-zero overhead on the IO hot path (a single `fetch_add` per completion). The main thread reads them periodically without locking. This avoids any contention on the IO path.

**Alternatives considered**:
- Per-second messages on a separate channel: Adds channel overhead and complexity.
- Shared `Mutex<Stats>`: Contention under high IOPS. Rejected.

## R7: Device Selection by PCI Address

**Decision**: Parse `--pci-addr` as a BDF string (e.g., `0000:03:00.0`), match against `ienv.devices()` list. If not specified, use the first device.

**Rationale**: SPDK enumerates devices by PCI address. The `PciAddress` struct has `domain`, `bus`, `dev`, `func` fields that map directly to BDF notation. This matches how SPDK tools (e.g., `spdk_nvme_perf`) identify devices.

**Alternatives considered**:
- Device index (`--device 0`): Less precise when device order can change between reboots.
- Device name or serial: Not exposed by the current ISPDKEnv interface.
