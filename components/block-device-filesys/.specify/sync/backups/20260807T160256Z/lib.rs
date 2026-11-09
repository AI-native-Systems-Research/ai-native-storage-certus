//! File-backed block device component implementing IBlockDevice via io_uring.
//!
//! This crate provides a block device component that uses a regular file on a
//! Linux filesystem as the backing store. It follows the actor model with a
//! dedicated thread running an io_uring event loop for async operations and
//! pread/pwrite with fdatasync for sync operations.
//!
//! # Architecture
//!
//! - **Actor model**: Dedicated thread with io_uring event loop
//! - **Durability**: fdatasync after every write (simulates NVMe completion semantics)
//! - **Zero-copy**: DmaBuffer byte slices passed directly to syscalls
//! - **Feature-gated telemetry**: `--features telemetry` for IO statistics
//!
//! # Usage
//!
//! ```ignore
//! use block_device_filesys::BlockDeviceFilesysComponent;
//! use component_framework::iunknown::query;
//!
//! let comp = BlockDeviceFilesysComponent::create("/tmp/dev.img", 4096, 1024);
//! comp.initialize().expect("init failed");
//! let ibd = query::<dyn IBlockDevice + Send + Sync>(&*comp).unwrap();
//! let channels = ibd.connect_client().unwrap();
//! ```

pub mod config;
pub(crate) mod telemetry;

mod actor;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use component_core::actor::{Actor, ActorHandle};
use component_core::channel::spsc::SpscChannel;
use component_framework::define_component;
use interfaces::ILogger;

pub use interfaces::{
    ClientChannels, Command, Completion, IBlockDevice, IBlockDeviceAdmin, NamespaceInfo,
    NvmeBlockError, OpHandle, TelemetrySnapshot,
};

use crate::actor::{ControlMessage, FilesysHandler};

/// Channel capacity for per-client SPSC channels.
const CLIENT_CHANNEL_CAPACITY: usize = 64;

/// Default io_uring submission queue depth.
const DEFAULT_RING_DEPTH: u32 = 128;

define_component! {
    pub BlockDeviceFilesysComponent {
        version: "0.1.0",
        provides: [IBlockDevice, IBlockDeviceAdmin],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            file_path: Mutex<Option<PathBuf>>,
            block_size: AtomicU32,
            num_blocks: AtomicU64,
            actor_handle: Mutex<Option<ActorHandle<ControlMessage>>>,
            next_client_id: AtomicU64,
            telemetry_stats: Mutex<Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>>,
        },
    }
}

impl BlockDeviceFilesysComponent {
    /// Create a new component with the given configuration.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use block_device_filesys::BlockDeviceFilesysComponent;
    ///
    /// let comp = BlockDeviceFilesysComponent::create("/tmp/dev.img", 4096, 1024);
    /// ```
    pub fn create(file_path: &str, block_size: u32, num_blocks: u64) -> std::sync::Arc<Self> {
        Self::new(
            Mutex::new(Some(PathBuf::from(file_path))),
            AtomicU32::new(block_size),
            AtomicU64::new(num_blocks),
            Mutex::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
        )
    }

    /// Set the backing file path.
    #[allow(dead_code)]
    pub(crate) fn set_file_path(&self, path: &str) {
        *self.file_path.lock().expect("file_path lock poisoned") = Some(PathBuf::from(path));
    }

    /// Set the block size in bytes. Must be a power of 2, minimum 512.
    #[allow(dead_code)]
    pub(crate) fn set_block_size(&self, size: u32) {
        self.block_size.store(size, Ordering::Relaxed);
    }

    /// Set the total number of blocks.
    #[allow(dead_code)]
    pub(crate) fn set_num_blocks(&self, count: u64) {
        self.num_blocks.store(count, Ordering::Relaxed);
    }

    /// Initialize the component: validate config, create/open backing file,
    /// start the actor thread.
    pub fn initialize(&self) -> Result<(), NvmeBlockError> {
        let logger: Option<std::sync::Arc<dyn ILogger + Send + Sync>> = self.logger.get().ok();

        let path = self
            .file_path
            .lock()
            .expect("file_path lock poisoned")
            .clone()
            .ok_or_else(|| NvmeBlockError::NotInitialized("file_path not set".into()))?;

        let block_size = self.block_size.load(Ordering::Relaxed);
        let num_blocks = self.num_blocks.load(Ordering::Relaxed);

        let cfg = config::DeviceConfig::new(path.clone(), block_size, num_blocks)
            .map_err(|e| NvmeBlockError::NotInitialized(format!("invalid config: {e}")))?;

        if let Some(ref log) = logger {
            log.info(&format!(
                "initializing file-backed block device: path={}, block_size={}, num_blocks={}",
                path.display(),
                block_size,
                num_blocks
            ));
        }

        let fd = config::open_or_create_backing_file(&cfg)
            .map_err(|e| NvmeBlockError::NotInitialized(format!("backing file error: {e}")))?;

        if let Some(ref log) = logger {
            log.debug("backing file opened/created successfully");
        }

        #[cfg(feature = "telemetry")]
        let telemetry = std::sync::Arc::new(crate::telemetry::TelemetryStats::new());

        #[cfg(feature = "telemetry")]
        let handler = FilesysHandler::with_telemetry(
            fd,
            cfg,
            logger.clone(),
            std::sync::Arc::clone(&telemetry),
        );

        #[cfg(not(feature = "telemetry"))]
        let handler = FilesysHandler::new(fd, cfg, logger.clone());

        #[cfg(feature = "telemetry")]
        {
            *self
                .telemetry_stats
                .lock()
                .expect("telemetry lock poisoned") =
                Some(telemetry as std::sync::Arc<dyn std::any::Any + Send + Sync>);
        }

        let actor: Actor<ControlMessage, FilesysHandler> = Actor::new(handler, |_panic| {});

        let handle = actor
            .activate()
            .map_err(|e| NvmeBlockError::NotInitialized(e.to_string()))?;

        *self
            .actor_handle
            .lock()
            .expect("actor_handle lock poisoned") = Some(handle);

        if let Some(ref log) = logger {
            log.info("block device initialized, actor started");
        }

        Ok(())
    }

    /// Shutdown the component: stop the actor and join its thread.
    pub fn shutdown(&self) -> Result<(), NvmeBlockError> {
        let maybe_handle = self
            .actor_handle
            .lock()
            .expect("actor_handle lock poisoned")
            .take();

        if let Some(handle) = maybe_handle {
            let _ = handle.send(ControlMessage::Shutdown);
            if let Err(e) = handle.deactivate() {
                return Err(NvmeBlockError::NotInitialized(format!(
                    "actor deactivate failed: {e}"
                )));
            }
        }
        Ok(())
    }
}

impl IBlockDeviceAdmin for BlockDeviceFilesysComponent {
    fn set_pci_address(&self, _addr: interfaces::PciAddress) {}

    fn set_actor_cpu(&self, _cpu: usize) {}

    fn initialize(&self) -> Result<(), NvmeBlockError> {
        self.initialize()
    }

    fn signal_stop(&self) {}

    fn shutdown(&self) -> Result<(), NvmeBlockError> {
        self.shutdown()
    }

    fn detach_controller(&self) {}
}

impl IBlockDevice for BlockDeviceFilesysComponent {
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        let handle_guard = self
            .actor_handle
            .lock()
            .expect("actor_handle lock poisoned");
        let handle = handle_guard.as_ref().ok_or_else(|| {
            NvmeBlockError::NotInitialized("call initialize() before connecting clients".into())
        })?;

        let client_id = self.next_client_id.fetch_add(1, Ordering::Relaxed);

        if let Ok(log) = self.logger.get() {
            log.debug(&format!("connecting client {client_id}"));
        }

        let ingress_ch = SpscChannel::<Command>::new(CLIENT_CHANNEL_CAPACITY);
        let (ingress_tx, ingress_rx) = ingress_ch.split().map_err(|_| {
            NvmeBlockError::ClientDisconnected("failed to create ingress channel".into())
        })?;

        let callback_ch = SpscChannel::<Completion>::new(CLIENT_CHANNEL_CAPACITY);
        let (callback_tx, callback_rx) = callback_ch.split().map_err(|_| {
            NvmeBlockError::ClientDisconnected("failed to create callback channel".into())
        })?;

        let session = actor::ClientSession::new(client_id, ingress_rx, callback_tx);

        handle
            .send(ControlMessage::ConnectClient { session })
            .map_err(|e| {
                NvmeBlockError::ClientDisconnected(format!(
                    "failed to register client with actor: {e}"
                ))
            })?;

        Ok(ClientChannels {
            command_tx: ingress_tx,
            completion_rx: callback_rx,
        })
    }

    fn sector_size(&self, ns_id: u32) -> Result<u32, NvmeBlockError> {
        if ns_id != 1 {
            return Err(NvmeBlockError::InvalidNamespace(format!(
                "ns_id {ns_id} invalid; only ns_id=1 supported"
            )));
        }
        Ok(self.block_size.load(Ordering::Relaxed))
    }

    fn num_sectors(&self, ns_id: u32) -> Result<u64, NvmeBlockError> {
        if ns_id != 1 {
            return Err(NvmeBlockError::InvalidNamespace(format!(
                "ns_id {ns_id} invalid; only ns_id=1 supported"
            )));
        }
        Ok(self.num_blocks.load(Ordering::Relaxed))
    }

    fn max_queue_depth(&self) -> u32 {
        DEFAULT_RING_DEPTH
    }

    fn num_io_queues(&self) -> u32 {
        1
    }

    fn max_transfer_size(&self) -> u32 {
        self.block_size.load(Ordering::Relaxed).saturating_mul(256)
    }

    fn block_size(&self) -> u32 {
        self.block_size.load(Ordering::Relaxed)
    }

    fn numa_node(&self) -> i32 {
        -1
    }

    fn nvme_version(&self) -> String {
        "N/A (file-backed)".into()
    }

    fn telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError> {
        #[cfg(feature = "telemetry")]
        {
            let stats_guard = self
                .telemetry_stats
                .lock()
                .expect("telemetry lock poisoned");
            match stats_guard.as_ref() {
                Some(any_arc) => {
                    let stats = any_arc
                        .downcast_ref::<crate::telemetry::TelemetryStats>()
                        .ok_or_else(|| {
                            NvmeBlockError::NotInitialized("telemetry stats type mismatch".into())
                        })?;
                    stats.snapshot()
                }
                None => Err(NvmeBlockError::NotInitialized(
                    "call initialize() before querying telemetry".into(),
                )),
            }
        }

        #[cfg(not(feature = "telemetry"))]
        {
            Err(NvmeBlockError::FeatureNotEnabled(
                "compile with --features telemetry to enable IO statistics".into(),
            ))
        }
    }

    /// Per-direction read/write counters. The filesys block device does not
    /// track per-direction rw-telemetry, so this returns zeroed counters.
    fn read_write_stats(&self) -> interfaces::ReadWriteStats {
        interfaces::ReadWriteStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::IUnknown;

    fn make_component() -> std::sync::Arc<BlockDeviceFilesysComponent> {
        BlockDeviceFilesysComponent::new(
            Mutex::new(Some(PathBuf::from("/tmp/test-dev.img"))),
            AtomicU32::new(4096),
            AtomicU64::new(1024),
            Mutex::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
        )
    }

    #[test]
    fn component_version() {
        let comp = make_component();
        assert_eq!(comp.version(), "0.1.0");
    }

    #[test]
    fn component_provides_iblock_device() {
        let comp = make_component();
        let ifaces = comp.provided_interfaces();
        assert!(ifaces.iter().any(|i| i.name == "IBlockDevice"));
    }

    #[test]
    fn component_has_logger_receptacle() {
        let comp = make_component();
        let receps = comp.receptacles();
        assert!(receps.iter().any(|r| r.name == "logger"));
    }

    #[test]
    fn connect_client_not_initialized() {
        let comp = make_component();
        let err = comp.connect_client().unwrap_err();
        assert!(matches!(err, NvmeBlockError::NotInitialized(_)));
    }

    #[test]
    fn device_info_returns_configured_values() {
        let comp = make_component();
        assert_eq!(comp.block_size(), 4096);
        assert_eq!(comp.max_queue_depth(), DEFAULT_RING_DEPTH);
        assert_eq!(comp.num_io_queues(), 1);
        assert_eq!(comp.max_transfer_size(), 4096 * 256);
        assert_eq!(comp.numa_node(), -1);
        assert_eq!(comp.nvme_version(), "N/A (file-backed)");
    }

    #[test]
    fn sector_size_invalid_namespace() {
        let comp = make_component();
        let err = comp.sector_size(2).unwrap_err();
        assert!(matches!(err, NvmeBlockError::InvalidNamespace(_)));
    }

    #[test]
    fn num_sectors_valid() {
        let comp = make_component();
        assert_eq!(comp.num_sectors(1).unwrap(), 1024);
    }

    #[test]
    fn telemetry_not_available_without_feature() {
        let comp = make_component();
        let result = comp.telemetry();
        assert!(result.is_err());
    }
}
