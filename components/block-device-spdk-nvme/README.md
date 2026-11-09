# block-device-spdk-nvme

SPDK-based NVMe block device component for the Certus project.

## Summary

This component provides direct userspace NVMe controller access via SPDK, wrapped in the Certus actor-based component model. Each instance owns a single NVMe controller identified by PCI address. IO is performed through zero-copy DMA buffers allocated from SPDK hugepages, with lock-free shared-memory channels connecting clients to the actor thread.

Key capabilities include synchronous and asynchronous read/write, NVMe namespace management (probe, create, format, delete), batch IO submission, hardware TSC-based latency measurement, optional compile-time telemetry for IO statistics, and debug-build-only logging of per-command DMA transfer sizes.

## Architecture

### Component Wiring

The component is defined via `define_component!` and provides two interfaces:

| Interface | Role | Description |
|-----------|------|-------------|
| `IBlockDevice` | Provided | Client connection, device info, IO operations, telemetry |
| `IBlockDeviceAdmin` | Provided | PCI address configuration, initialize, shutdown |

It declares two receptacles that must be wired before initialization:

| Receptacle | Type | Purpose |
|------------|------|---------|
| `spdk_env` | `ISPDKEnv` | SPDK environment (hugepages, drivers) |
| `logger` | `ILogger` | Debug/info logging via dependency injection |

### Actor Model

Each component instance runs a dedicated actor thread that is NUMA-pinned to the same node as the NVMe controller. The actor self-polls all attached client channels and dispatches IO through a queue pair pool using a shallowest-fit depth selection heuristic. Lifecycle: set PCI address, wire receptacles, call `initialize()` to probe the controller and start the actor.

### Client Channels

Clients call `connect_client()` to obtain a `ClientChannels` struct containing:

- `command_tx` -- SPSC sender for submitting `Command` messages (read, write, write-zeros, batch, reset, namespace ops)
- `completion_rx` -- SPSC receiver for asynchronous `Completion` notifications

Channel capacity is 64 slots. Multiple clients can connect simultaneously; each gets an independent channel pair.

### Debug Diagnostics

In debug builds (`#[cfg(debug_assertions)]`), the actor emits one stderr line
per successfully submitted read/write command reporting the size of the DMA
data transfer issued to the controller:

```
[block-device-spdk-nvme][dma] <op> lba=<lba> blocks=<n> bytes=<n>
```

`<op>` is one of `read-sync`, `write-sync`, `read-async`, `write-async`;
`bytes` is the host transfer size (blocks × sector size) handed to SPDK. This
is the size of the logical request as submitted by the driver — SPDK may split
a transfer larger than the controller's MDTS into multiple NVMe commands on the
wire. The logging macro compiles out entirely in release builds (no argument
evaluation, zero cost) and is independent of the `telemetry` feature.

### Module Layout

```
src/
  lib.rs          Component definition, IBlockDevice/IBlockDeviceAdmin impls
  actor.rs        BlockDeviceHandler -- command dispatch, completions, self-polling
  command.rs      ControlMessage, ClientSession (internal types)
  controller.rs   NvmeController safe wrapper around SPDK ctrlr APIs
  namespace.rs    Namespace operations and validation
  qpair.rs        QueuePairPool with depth-based queue selection
  telemetry.rs    TelemetryStats (feature-gated behind "telemetry")
  tsc.rs          TscClock -- calibrated rdtsc for low-overhead timing
```

## Build

Prerequisites: Linux with hugepages and IOMMU, NVMe device bound to VFIO/UIO, SPDK built at `deps/spdk-build/` (run `deps/build_spdk.sh`), Rust stable (MSRV 1.75).

```bash
# Standard build
cargo build -p block-device-spdk-nvme

# With telemetry support
cargo build -p block-device-spdk-nvme --features telemetry
```

## Test

```bash
# All tests (unit + integration)
cargo test -p block-device-spdk-nvme

# Integration tests only
cargo test -p block-device-spdk-nvme --test integration
```

Unit tests run without hardware. Integration tests self-skip when SPDK is unavailable.

## Benchmarks

Two Criterion benchmark suites are available, both gated behind the `spdk` feature (requires hardware):

```bash
# Run all benchmarks
cargo bench -p block-device-spdk-nvme --features spdk

# Latency benchmark only
cargo bench -p block-device-spdk-nvme --features spdk --bench latency

# Throughput benchmark only
cargo bench -p block-device-spdk-nvme --features spdk --bench throughput
```
