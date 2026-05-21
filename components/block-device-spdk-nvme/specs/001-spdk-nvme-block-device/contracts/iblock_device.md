# Contract: IBlockDevice Interface

## Overview

The `IBlockDevice` interface is the primary provided interface of the
`BlockDeviceSpdkNvmeComponent`. It extends the existing `IBasicBlockDevice`
with channel-based client connections, device introspection, and telemetry.

## Interface Definition

```rust
define_interface! {
    pub IBlockDevice {
        /// Create a new client connection, returning channel endpoints.
        ///
        /// Returns a pair: (ingress_sender, callback_receiver) that the
        /// client uses to send commands and receive completions.
        fn connect_client(&self) -> Result<ClientChannels, BlockDeviceError>;

        /// Return the sector size in bytes (e.g., 512 or 4096).
        fn sector_size(&self, ns_id: u32) -> Result<u32, BlockDeviceError>;

        /// Return the total number of sectors for a namespace.
        fn num_sectors(&self, ns_id: u32) -> Result<u64, BlockDeviceError>;

        /// Return the maximum queue depth supported by the controller.
        fn max_queue_depth(&self) -> u32;

        /// Return the number of NVMe IO queues.
        fn num_io_queues(&self) -> u32;

        /// Return the maximum data transfer size in bytes.
        fn max_transfer_size(&self) -> u32;

        /// Return the block/sector size for the default namespace.
        fn block_size(&self) -> u32;

        /// Return the NUMA node ID of the NVMe controller.
        fn numa_node(&self) -> i32;

        /// Return the NVMe specification version string.
        fn nvme_version(&self) -> String;

        /// Return telemetry statistics (requires `telemetry` feature).
        ///
        /// Returns `Err(BlockDeviceError::FeatureNotEnabled)` when compiled
        /// without the `telemetry` feature.
        fn telemetry(&self) -> Result<TelemetrySnapshot, BlockDeviceError>;
    }
}
```

## ClientChannels Return Type

```rust
pub struct ClientChannels {
    /// Sender for submitting IO commands to the actor.
    pub command_tx: Sender<Command>,
    /// Receiver for receiving completions from the actor.
    pub completion_rx: Receiver<Completion>,
}
```

## Error Variants (additions to BlockDeviceError)

```rust
pub enum BlockDeviceError {
    // ... existing variants from interfaces crate ...

    /// The telemetry feature is not compiled in.
    FeatureNotEnabled(String),
    /// The controller has not been initialized yet.
    NotInitialized(String),
    /// An async operation timed out.
    Timeout(String),
    /// An operation was aborted by the client.
    Aborted(String),
    /// Namespace does not exist.
    InvalidNamespace(String),
    /// The requested operation is not supported.
    NotSupported(String),
}
```

## TelemetrySnapshot

```rust
pub struct TelemetrySnapshot {
    pub total_ops: u64,
    pub min_latency_ns: u64,
    pub max_latency_ns: u64,
    pub mean_latency_ns: u64,
    pub mean_throughput_mbps: f64,
    pub elapsed_secs: f64,
}
```
