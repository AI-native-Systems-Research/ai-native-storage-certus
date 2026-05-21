# block-device-spdk-nvme (v2)

SPDK-based NVMe block device component for the Certus project. Provides direct userspace NVMe controller access via the actor model with shared-memory client channels.

## Summary

Version 2 of the block device component. Each instance owns a single NVMe controller and runs a dedicated actor thread pinned to the controller's NUMA node. Clients connect via lock-free SPSC channels for command submission and completion notification. The actor self-polls with adaptive parking and dispatches IO through a multi-depth queue pair pool using shallowest-fit selection.

Additions over v1:
- `TscClock` module for low-overhead TSC-based timing (calibrated rdtsc with fixed-point nanosecond conversion, deadline computation)
- Benchmarks gated behind `--features spdk` (required-features in Cargo.toml)

Key capabilities:
- Sync and async read/write with DMA buffers (zero-copy in-process)
- NVMe namespace management (probe, create, format, delete)
- Batch IO submission
- Optional telemetry (IO latency, throughput statistics)
- NUMA-aware thread affinity
- Hardware TSC clock for low-overhead latency measurement

## Structure

```
src/
  lib.rs          Component definition (BlockDeviceSpdkNvmeComponentV2), IBlockDevice impl
  actor.rs        BlockDeviceHandler — command dispatch, async completions, self-polling loop
  command.rs      ControlMessage, ClientSession (internal types)
  controller.rs   NvmeController safe wrapper
  namespace.rs    Namespace operations and validation
  qpair.rs        QueuePair, QueuePairPool, depth-based selection heuristic
  telemetry.rs    TelemetryStats (feature-gated)
  tsc.rs          TscClock — userspace TSC clock with calibration and tick-to-ns conversion
tests/
  integration.rs  Hardware-conditional integration tests (self-skip without SPDK)
benches/
  latency.rs      Per-operation latency benchmarks (requires --features spdk)
  throughput.rs   Batch IO throughput benchmarks (requires --features spdk)
```

### Interfaces

| Interface | Role | Description |
|-----------|------|-------------|
| `IBlockDevice` | Provided | Client connection, device info, IO operations, telemetry |
| `IBlockDeviceAdmin` | Provided | PCI address config, initialize, shutdown |
| `ILogger` | Receptacle | Debug logging via dependency injection |
| `ISPDKEnv` | Receptacle | SPDK environment initialization |

## Build and Test

### Prerequisites

- Linux host with hugepages and IOMMU enabled
- NVMe device bound to VFIO/UIO
- SPDK built at `deps/spdk-build/` (run `deps/build_spdk.sh`)
- Rust stable (edition 2021, MSRV 1.75+)

### Build

```bash
cargo build -p block-device-spdk-nvme-v2

# With telemetry support
cargo build -p block-device-spdk-nvme-v2 --features telemetry
```

### Test

```bash
# All tests (unit + integration)
cargo test -p block-device-spdk-nvme-v2

# Integration tests only
cargo test -p block-device-spdk-nvme-v2 --test integration
```

Unit tests run without hardware. Integration hardware tests self-skip when SPDK is unavailable.

### Benchmarks

```bash
cargo bench -p block-device-spdk-nvme-v2 --features spdk
cargo bench -p block-device-spdk-nvme-v2 --features spdk --bench latency
cargo bench -p block-device-spdk-nvme-v2 --features spdk --bench throughput
```

### Lint

```bash
cargo fmt --check
cargo clippy -p block-device-spdk-nvme-v2 -- -D warnings
cargo doc -p block-device-spdk-nvme-v2 --no-deps
```
