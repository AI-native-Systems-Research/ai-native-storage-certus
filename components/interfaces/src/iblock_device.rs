//! IBlockDevice interface and associated types for channel-based NVMe block devices.
//!
//! This module defines the `IBlockDevice` trait for actor-model block device
//! components, along with all types that appear in its public API:
//! commands, completions, error types, telemetry, and channel endpoints.

use std::fmt;
use std::sync::{Arc, Mutex};

use component_core::channel::{Receiver, Sender};
use component_macros::define_interface;

use crate::spdk_types::{BlockDeviceError, DmaBuffer, SpdkEnvError};

// ---------------------------------------------------------------------------
// NvmeBlockError
// ---------------------------------------------------------------------------

/// Error conditions reported by NVMe block device components.
///
/// Each variant carries a descriptive message with actionable guidance.
///
/// # Examples
///
/// ```
/// use interfaces::NvmeBlockError;
///
/// let err = NvmeBlockError::Timeout("operation 42 exceeded 5000ms deadline".into());
/// assert!(err.to_string().contains("timed out"));
/// ```
#[derive(Debug, Clone)]
pub enum NvmeBlockError {
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
    /// A block device error from the lower layer.
    BlockDevice(BlockDeviceError),
    /// The SPDK environment is not ready.
    SpdkEnv(SpdkEnvError),
    /// LBA is out of range for the namespace.
    LbaOutOfRange(String),
    /// The client channel was disconnected.
    ClientDisconnected(String),
}

impl fmt::Display for NvmeBlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FeatureNotEnabled(msg) => write!(f, "feature not enabled: {msg}"),
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
            Self::Timeout(msg) => write!(f, "operation timed out: {msg}"),
            Self::Aborted(msg) => write!(f, "operation aborted: {msg}"),
            Self::InvalidNamespace(msg) => write!(f, "invalid namespace: {msg}"),
            Self::NotSupported(msg) => write!(f, "not supported: {msg}"),
            Self::BlockDevice(e) => write!(f, "block device error: {e}"),
            Self::SpdkEnv(e) => write!(f, "SPDK env error: {e}"),
            Self::LbaOutOfRange(msg) => write!(f, "LBA out of range: {msg}"),
            Self::ClientDisconnected(msg) => write!(f, "client disconnected: {msg}"),
        }
    }
}

impl std::error::Error for NvmeBlockError {}

impl From<BlockDeviceError> for NvmeBlockError {
    fn from(e: BlockDeviceError) -> Self {
        Self::BlockDevice(e)
    }
}

impl From<SpdkEnvError> for NvmeBlockError {
    fn from(e: SpdkEnvError) -> Self {
        Self::SpdkEnv(e)
    }
}

// ---------------------------------------------------------------------------
// TelemetrySnapshot
// ---------------------------------------------------------------------------

/// A snapshot of telemetry statistics at a point in time.
///
/// # Examples
///
/// ```
/// use interfaces::TelemetrySnapshot;
///
/// let snap = TelemetrySnapshot {
///     total_ops: 1000,
///     min_latency_ns: 800,
///     max_latency_ns: 50_000,
///     mean_latency_ns: 4_200,
///     mean_throughput_mbps: 1250.0,
///     elapsed_secs: 1.5,
/// };
/// assert_eq!(snap.total_ops, 1000);
/// ```
#[derive(Debug, Clone)]
pub struct TelemetrySnapshot {
    /// Total number of completed IO operations.
    pub total_ops: u64,
    /// Minimum observed IO latency in nanoseconds.
    pub min_latency_ns: u64,
    /// Maximum observed IO latency in nanoseconds.
    pub max_latency_ns: u64,
    /// Mean IO latency in nanoseconds.
    pub mean_latency_ns: u64,
    /// Mean throughput in megabytes per second.
    pub mean_throughput_mbps: f64,
    /// Elapsed time since telemetry collection started, in seconds.
    pub elapsed_secs: f64,
}

// ---------------------------------------------------------------------------
// OpHandle
// ---------------------------------------------------------------------------

/// A unique, component-assigned handle for tracking async operations.
///
/// Handles are monotonically increasing `u64` values assigned at
/// submission time.
///
/// # Examples
///
/// ```
/// use interfaces::OpHandle;
///
/// let h = OpHandle(1);
/// assert_eq!(h.0, 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpHandle(pub u64);

// ---------------------------------------------------------------------------
// NamespaceInfo
// ---------------------------------------------------------------------------

/// Information about a discovered NVMe namespace.
///
/// # Examples
///
/// ```
/// use interfaces::NamespaceInfo;
///
/// let ns = NamespaceInfo {
///     ns_id: 1,
///     num_sectors: 1_000_000,
///     sector_size: 512,
/// };
/// assert_eq!(ns.ns_id, 1);
/// ```
#[derive(Debug, Clone)]
pub struct NamespaceInfo {
    /// NVMe namespace identifier.
    pub ns_id: u32,
    /// Total number of sectors in this namespace.
    pub num_sectors: u64,
    /// Sector size in bytes.
    pub sector_size: u32,
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// IO commands sent by clients on the ingress channel.
///
/// # Examples
///
/// ```
/// use interfaces::Command;
///
/// let cmd = Command::NsProbe;
/// assert!(matches!(cmd, Command::NsProbe));
/// ```
pub enum Command {
    /// Synchronous read: blocks until completion.
    ReadSync {
        /// NVMe namespace identifier.
        ns_id: u32,
        /// Starting logical block address.
        lba: u64,
        /// DMA buffer to read into (caller-allocated).
        buf: Arc<Mutex<DmaBuffer>>,
    },
    /// Synchronous write: blocks until completion.
    WriteSync {
        /// NVMe namespace identifier.
        ns_id: u32,
        /// Starting logical block address.
        lba: u64,
        /// DMA buffer containing data to write.
        buf: Arc<DmaBuffer>,
    },
    /// Asynchronous read with timeout.
    ReadAsync {
        /// NVMe namespace identifier.
        ns_id: u32,
        /// Starting logical block address.
        lba: u64,
        /// DMA buffer to read into.
        buf: Arc<Mutex<DmaBuffer>>,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
    /// Asynchronous write with timeout.
    WriteAsync {
        /// NVMe namespace identifier.
        ns_id: u32,
        /// Starting logical block address.
        lba: u64,
        /// DMA buffer containing data to write.
        buf: Arc<DmaBuffer>,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
    /// Write zeros to a range of blocks.
    WriteZeros {
        /// NVMe namespace identifier.
        ns_id: u32,
        /// Starting logical block address.
        lba: u64,
        /// Number of blocks to zero.
        num_blocks: u32,
    },
    /// Submit a batch of operations.
    BatchSubmit {
        /// The operations to execute as a batch.
        ops: Vec<Command>,
    },
    /// Abort an in-flight asynchronous operation by handle.
    AbortOp {
        /// The operation handle to abort.
        handle: OpHandle,
    },
    /// Probe all namespaces on the controller.
    NsProbe,
    /// Create a new namespace with the given size (always uses lbaf=0).
    /// Use NsFormat to change the LBA format after creation.
    NsCreate {
        /// Size of the namespace in sectors (0 = use all remaining capacity).
        size_sectors: u64,
    },
    /// Format an existing namespace (erases all data).
    NsFormat {
        /// NVMe namespace identifier to format.
        ns_id: u32,
        /// LBA format index from the controller's format table (default 0).
        lbaf: u8,
    },
    /// Delete an existing namespace.
    NsDelete {
        /// NVMe namespace identifier to delete.
        ns_id: u32,
    },
    /// Issue a hardware controller reset.
    ControllerReset,
}

// Command contains Arc<DmaBuffer> which is Send.
// SAFETY: All fields in Command are Send (Arc, u32, u64, Vec).
unsafe impl Send for Command {}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

/// Completion messages sent by the actor on the callback channel.
///
/// Each completion for an async operation includes the [`OpHandle`]
/// assigned at submission time.
///
/// # Examples
///
/// ```
/// use interfaces::{Completion, OpHandle};
///
/// let c = Completion::AbortAck { handle: OpHandle(7) };
/// assert!(matches!(c, Completion::AbortAck { handle } if handle.0 == 7));
/// ```
#[derive(Debug)]
pub enum Completion {
    /// A read operation completed.
    ReadDone {
        /// Operation handle.
        handle: OpHandle,
        /// Result of the read.
        result: Result<(), NvmeBlockError>,
    },
    /// A write operation completed.
    WriteDone {
        /// Operation handle.
        handle: OpHandle,
        /// Result of the write.
        result: Result<(), NvmeBlockError>,
    },
    /// A write-zeros operation completed.
    WriteZerosDone {
        /// Operation handle.
        handle: OpHandle,
        /// Result of the write-zeros.
        result: Result<(), NvmeBlockError>,
    },
    /// An abort request was acknowledged.
    AbortAck {
        /// Handle of the aborted operation.
        handle: OpHandle,
    },
    /// An async operation timed out.
    Timeout {
        /// Handle of the timed-out operation.
        handle: OpHandle,
    },
    /// Namespace probe results.
    NsProbeResult {
        /// Discovered namespaces.
        namespaces: Vec<NamespaceInfo>,
    },
    /// A namespace was created.
    NsCreated {
        /// The new namespace identifier.
        ns_id: u32,
    },
    /// A namespace was formatted.
    NsFormatted {
        /// The formatted namespace identifier.
        ns_id: u32,
    },
    /// A namespace was deleted.
    NsDeleted {
        /// The deleted namespace identifier.
        ns_id: u32,
    },
    /// A controller reset completed.
    ResetDone {
        /// Result of the reset.
        result: Result<(), NvmeBlockError>,
    },
    /// A general error not tied to a specific operation.
    Error {
        /// Optional operation handle (if error is operation-specific).
        handle: Option<OpHandle>,
        /// The error.
        error: NvmeBlockError,
    },
}

// SAFETY: All fields in Completion are Send.
unsafe impl Send for Completion {}

// ---------------------------------------------------------------------------
// ClientChannels
// ---------------------------------------------------------------------------

/// Channels returned to a client on connection.
///
/// # Examples
///
/// ```ignore
/// let channels = ibd.connect_client().unwrap();
/// channels.command_tx.send(Command::NsProbe).unwrap();
/// let completion = channels.completion_rx.recv().unwrap();
/// ```
pub struct ClientChannels {
    /// Sender for submitting IO commands to the actor.
    pub command_tx: Sender<Command>,
    /// Receiver for receiving completions from the actor.
    pub completion_rx: Receiver<Completion>,
}

impl fmt::Debug for ClientChannels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientChannels")
            .field("command_tx", &"Sender<Command>")
            .field("completion_rx", &"Receiver<Completion>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// IBlockDevice
// ---------------------------------------------------------------------------

// The block device interface for channel-based client connections,
// device introspection, and telemetry.
define_interface! {
    pub IBlockDevice {
        /// Create a new client connection, returning channel endpoints.
        fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError>;

        /// Return the sector size in bytes for a namespace.
        fn sector_size(&self, ns_id: u32) -> Result<u32, NvmeBlockError>;

        /// Return the total number of sectors for a namespace.
        fn num_sectors(&self, ns_id: u32) -> Result<u64, NvmeBlockError>;

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
        fn telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError>;
    }
}

// ---------------------------------------------------------------------------
// IBlockDeviceAdmin
// ---------------------------------------------------------------------------

// Administrative lifecycle/configuration API for block device components.
define_interface! {
    pub IBlockDeviceAdmin {
        /// Set the PCI address of the controller to attach to.
        fn set_pci_address(&self, addr: crate::spdk_types::PciAddress);

        /// Pin the actor thread to a specific CPU core.
        ///
        /// Must be called before [`initialize`]. If not called, the actor
        /// pins to the first CPU on the controller's NUMA node.
        fn set_actor_cpu(&self, cpu: usize);

        /// Initialize the component and start its actor thread.
        fn initialize(&self) -> Result<(), NvmeBlockError>;

        /// Shutdown the component: stop the actor and join its thread.
        ///
        /// This ensures no actor threads are executing SPDK code when the
        /// global SPDK/DPDK environment is torn down.
        fn shutdown(&self) -> Result<(), NvmeBlockError>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_handle_equality() {
        assert_eq!(OpHandle(1), OpHandle(1));
        assert_ne!(OpHandle(1), OpHandle(2));
    }

    #[test]
    fn op_handle_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(OpHandle(1));
        set.insert(OpHandle(2));
        assert_eq!(set.len(), 2);
        assert!(set.contains(&OpHandle(1)));
    }

    #[test]
    fn namespace_info_clone() {
        let ns = NamespaceInfo {
            ns_id: 1,
            num_sectors: 1_000_000,
            sector_size: 512,
        };
        let ns2 = ns.clone();
        assert_eq!(ns2.ns_id, 1);
    }

    #[test]
    fn nvme_block_error_display() {
        let err = NvmeBlockError::Timeout("op 1 exceeded 5000ms".into());
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn nvme_block_error_from_block_device_error() {
        let err = BlockDeviceError::ReadFailed("test".into());
        let nvme_err: NvmeBlockError = err.into();
        assert!(matches!(nvme_err, NvmeBlockError::BlockDevice(_)));
    }

    #[test]
    fn nvme_block_error_from_spdk_env_error() {
        let err = SpdkEnvError::InitFailed("test".into());
        let nvme_err: NvmeBlockError = err.into();
        assert!(matches!(nvme_err, NvmeBlockError::SpdkEnv(_)));
    }

    #[test]
    fn telemetry_snapshot_clone() {
        let snap = TelemetrySnapshot {
            total_ops: 100,
            min_latency_ns: 500,
            max_latency_ns: 10_000,
            mean_latency_ns: 2_000,
            mean_throughput_mbps: 500.0,
            elapsed_secs: 1.0,
        };
        let snap2 = snap.clone();
        assert_eq!(snap2.total_ops, 100);
    }

    #[test]
    fn command_ns_probe_matches() {
        let cmd = Command::NsProbe;
        assert!(matches!(cmd, Command::NsProbe));
    }

    #[test]
    fn completion_abort_ack() {
        let c = Completion::AbortAck {
            handle: OpHandle(7),
        };
        assert!(matches!(c, Completion::AbortAck { handle } if handle.0 == 7));
    }
}
