# Quickstart: block-device-spdk-nvme

## Prerequisites

- Linux with VFIO/UIO drivers configured
- Hugepages enabled (at least 1GB recommended)
- SPDK built and installed at `../../deps/spdk-build/`
- NVMe device bound to VFIO (not kernel driver)

## Build

```bash
# From repo root — build only this component (SPDK required):
cargo build -p block-device-spdk-nvme

# With telemetry:
cargo build -p block-device-spdk-nvme --features telemetry

# Run tests:
cargo test -p block-device-spdk-nvme --all

# Run benchmarks:
cargo bench -p block-device-spdk-nvme
```

## Basic Usage

```rust
use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_framework::iunknown::query;
use example_logger::LoggerComponent;
use spdk_env::SPDKEnvComponent;
use interfaces::{IBlockDevice, ILogger, ISPDKEnv, PciAddress};

// 1. Create and wire components
let logger = LoggerComponent::new();
let spdk_env = SPDKEnvComponent::new(/* ... */);
let ilogger = query::<dyn ILogger + Send + Sync>(&*logger).unwrap();
let ispdk = query::<dyn ISPDKEnv + Send + Sync>(&*spdk_env).unwrap();

ispdk.init().unwrap();

let block_dev = BlockDeviceSpdkNvmeComponent::new(
    PciAddress { domain: 0, bus: 1, dev: 0, func: 0 },
);
block_dev.logger.connect(ilogger).unwrap();
block_dev.spdk_env.connect(ispdk).unwrap();

// 2. Get the IBlockDevice interface and connect a client
let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();
let channels = ibd.connect_client().unwrap();

// 3. Send a write command
use interfaces::DmaBuffer;
let buf = DmaBuffer::new(4096, 4096, None).unwrap();
buf.as_mut_slice().fill(0xAB);

channels.command_tx.send(Command::WriteSync {
    ns_id: 1,
    lba: 0,
    buf: Arc::new(buf),
}).unwrap();

// 4. Receive completion
let completion = channels.completion_rx.recv().unwrap();
```

## Project Structure

```
block-device-spdk-nvme/
├── Cargo.toml
├── src/
│   ├── lib.rs           # Component definition, IBlockDevice impl
│   ├── actor.rs          # Actor handler (message loop, SPDK calls)
│   ├── command.rs        # Command and Completion enums
│   ├── controller.rs     # NVMe controller wrapper (safe SPDK FFI)
│   ├── namespace.rs      # Namespace management operations
│   ├── qpair.rs          # IO queue pair pool and selection
│   └── telemetry.rs      # Feature-gated telemetry collection
├── tests/
│   ├── integration.rs    # Cross-component integration tests
│   └── mock_spdk.rs      # Mock SPDK environment for unit testing
└── benches/
    ├── latency.rs        # IO latency benchmarks at varying queue depths
    └── throughput.rs     # Throughput benchmarks with batch operations
```
