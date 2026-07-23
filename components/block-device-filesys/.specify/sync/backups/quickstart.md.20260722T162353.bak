# Quickstart: block-device-filesys

## Prerequisites

- Rust stable >= 1.75
- Linux kernel >= 5.6 (for io_uring)
- A writable filesystem (ext4, XFS, tmpfs for tests)

## Build

```bash
cd components/block-device-filesys
cargo build
```

## Run Tests

```bash
cargo test
```

Tests use temporary files in the system temp directory — no special setup needed.

## Run Benchmarks

```bash
cargo bench
```

## Usage Example

```rust,ignore
use block_device_filesys::BlockDeviceFilesysComponent;
use component_framework::iunknown::query;
use interfaces::{IBlockDevice, Command, Completion};

// Create and configure the component
let comp = BlockDeviceFilesysComponent::new(/* fields */);

// Configure: file path, block size, number of blocks
comp.set_file_path("/tmp/test-device.img");
comp.set_block_size(4096);
comp.set_num_blocks(1024); // 4MB device

// Initialize (creates backing file, starts actor)
comp.initialize().expect("init failed");

// Connect a client
let ibd = query::<dyn IBlockDevice + Send + Sync>(&*comp).unwrap();
let channels = ibd.connect_client().unwrap();

// Send a write command
channels.command_tx.send(Command::WriteZeros {
    ns_id: 1,
    lba: 0,
    num_blocks: 8,
}).unwrap();

// Receive completion
let completion = channels.completion_rx.recv().unwrap();
assert!(matches!(completion, Completion::WriteZerosDone { .. }));

// Shutdown
comp.shutdown().expect("shutdown failed");
```

## Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| file_path | (required) | Path to backing file |
| block_size | 512 | Sector size in bytes (must be power of 2) |
| num_blocks | (required) | Total block count |

## Feature Flags

| Feature | Description |
|---------|-------------|
| `telemetry` | Enable IO statistics collection (latency, throughput) |
