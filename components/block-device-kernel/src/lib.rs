//! Linux kernel block device component implementing IBlockDevice via io_uring.
//!
//! This crate provides a block device component that uses a raw Linux block
//! device (e.g., `/dev/nvme0n1`) as the backing store. All IO is routed
//! through io_uring — there is no pread/pwrite fallback.
//!
//! # Architecture
//!
//! - **Actor model**: Dedicated thread with io_uring event loop
//! - **Direct IO**: O_DIRECT bypass of kernel page cache
//! - **io_uring only**: All read/write operations go through io_uring
//! - **Zero-copy**: DmaBuffer byte slices passed directly to syscalls
//! - **Feature-gated telemetry**: `--features telemetry` for IO statistics
//!
//! # Usage
//!
//! ```ignore
//! use block_device_kernel::BlockDeviceKernelComponent;
//! use component_framework::iunknown::query;
//!
//! let comp = BlockDeviceKernelComponent::create("/dev/nvme0n1", 4096, 0);
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
use io_uring::IoUring;

pub use interfaces::{
    ClientChannels, Command, Completion, IBlockDevice, IBlockDeviceAdmin, NamespaceInfo,
    NvmeBlockError, OpHandle, TelemetrySnapshot,
};

use crate::actor::{ControlMessage, KernelHandler};

/// Channel capacity for per-client SPSC channels.
const CLIENT_CHANNEL_CAPACITY: usize = 64;

/// Default io_uring submission queue depth.
const DEFAULT_RING_DEPTH: u32 = 128;

define_component! {
    pub BlockDeviceKernelComponent {
        version: "0.1.0",
        provides: [IBlockDevice, IBlockDeviceAdmin],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            device_path: Mutex<Option<PathBuf>>,
            block_size: AtomicU32,
            num_blocks: AtomicU64,
            actor_handle: Mutex<Option<ActorHandle<ControlMessage>>>,
            next_client_id: AtomicU64,
            telemetry_stats: Mutex<Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>>,
        },
    }
}

impl BlockDeviceKernelComponent {
    /// Create a new component with the given configuration.
    ///
    /// Pass `num_blocks = 0` to auto-detect the device size.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use block_device_kernel::BlockDeviceKernelComponent;
    ///
    /// let comp = BlockDeviceKernelComponent::create("/dev/nvme0n1", 4096, 0);
    /// ```
    pub fn create(device_path: &str, block_size: u32, num_blocks: u64) -> std::sync::Arc<Self> {
        Self::new(
            Mutex::new(Some(PathBuf::from(device_path))),
            AtomicU32::new(block_size),
            AtomicU64::new(num_blocks),
            Mutex::new(None),
            AtomicU64::new(0),
            Mutex::new(None),
        )
    }

    /// Initialize the component: validate config, open block device,
    /// create io_uring instance, start the actor thread.
    pub fn initialize(&self) -> Result<(), NvmeBlockError> {
        let logger: Option<std::sync::Arc<dyn ILogger + Send + Sync>> = self.logger.get().ok();

        let path = self
            .device_path
            .lock()
            .expect("device_path lock poisoned")
            .clone()
            .ok_or_else(|| NvmeBlockError::NotInitialized("device_path not set".into()))?;

        let block_size = self.block_size.load(Ordering::Relaxed);
        let num_blocks = self.num_blocks.load(Ordering::Relaxed);

        let cfg = config::DeviceConfig::new(path.clone(), block_size, num_blocks)
            .map_err(|e| NvmeBlockError::NotInitialized(format!("invalid config: {e}")))?;

        self.num_blocks.store(cfg.num_blocks(), Ordering::Relaxed);

        if let Some(ref log) = logger {
            log.info(&format!(
                "initializing kernel block device: path={}, block_size={}, num_blocks={}",
                path.display(),
                cfg.block_size(),
                cfg.num_blocks()
            ));
        }

        let fd = config::open_block_device(&cfg)
            .map_err(|e| NvmeBlockError::NotInitialized(format!("block device open error: {e}")))?;

        let ring = IoUring::new(DEFAULT_RING_DEPTH).map_err(|e| {
            NvmeBlockError::NotInitialized(format!("io_uring creation failed: {e}"))
        })?;

        if let Some(ref log) = logger {
            log.debug("block device opened, io_uring ring created");
        }

        #[cfg(feature = "telemetry")]
        let telemetry = std::sync::Arc::new(crate::telemetry::TelemetryStats::new());

        #[cfg(feature = "telemetry")]
        let handler = KernelHandler::with_telemetry(
            fd,
            cfg,
            ring,
            logger.clone(),
            std::sync::Arc::clone(&telemetry),
        );

        #[cfg(not(feature = "telemetry"))]
        let handler = KernelHandler::new(fd, cfg, ring, logger.clone());

        #[cfg(feature = "telemetry")]
        {
            *self
                .telemetry_stats
                .lock()
                .expect("telemetry lock poisoned") =
                Some(telemetry as std::sync::Arc<dyn std::any::Any + Send + Sync>);
        }

        let actor: Actor<ControlMessage, KernelHandler> = Actor::new(handler, |_panic| {});

        let handle = actor
            .activate()
            .map_err(|e| NvmeBlockError::NotInitialized(e.to_string()))?;

        *self
            .actor_handle
            .lock()
            .expect("actor_handle lock poisoned") = Some(handle);

        if let Some(ref log) = logger {
            log.info("kernel block device initialized, actor started");
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

impl IBlockDeviceAdmin for BlockDeviceKernelComponent {
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

impl IBlockDevice for BlockDeviceKernelComponent {
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

        let session = actor::ClientSession {
            id: client_id,
            ingress_rx,
            callback_tx,
        };

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
        "N/A (kernel block device)".into()
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

    fn io_byte_stats(&self) -> interfaces::ReadWriteStats {
        // This backend does not track per-direction byte/op counters.
        interfaces::ReadWriteStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::IUnknown;

    fn make_component() -> std::sync::Arc<BlockDeviceKernelComponent> {
        BlockDeviceKernelComponent::new(
            Mutex::new(Some(PathBuf::from("/dev/nvme0n1"))),
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
        assert_eq!(comp.nvme_version(), "N/A (kernel block device)");
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
