//! SPDK NVMe block device component with actor model, async IO, and telemetry.
//!
//! This crate provides a high-performance NVMe block device component using
//! SPDK for direct userspace NVMe access. It follows the actor model with
//! NUMA-aware thread pinning and exposes an [`IBlockDevice`] interface for
//! channel-based client connections.
//!
//! # Architecture
//!
//! - **Actor model**: Dedicated thread per controller, NUMA-pinned
//! - **Two-tier channels**: Actor MPSC for control + per-client SPSC for IO
//! - **Zero-copy**: DMA buffers allocated from SPDK hugepages
//! - **Feature-gated telemetry**: `--features telemetry` for IO statistics
//!
//! After connecting a client via [`IBlockDevice::connect_client()`], callers
//! send commands on `command_tx` and receive completions on `completion_rx`.
//! The actor thread self-polls and processes commands automatically.
//!
//! # Usage
//!
//! ```ignore
//! use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
//! use component_framework::iunknown::query;
//!
//! let comp = BlockDeviceSpdkNvmeComponent::new(pci_address);
//! // Wire receptacles: comp.logger, comp.spdk_env
//! // let ibd = query::<dyn IBlockDevice + Send + Sync>(&*comp).unwrap();
//! // let channels = ibd.connect_client().unwrap();
//! ```
//!
//! # Internal Modules
//!
//! - `controller` — Safe wrapper around SPDK NVMe controller
//! - `qpair` — IO queue pair pool with depth-based selection
//! - `namespace` — Namespace management operations

pub(crate) mod controller;
pub(crate) mod namespace;
pub(crate) mod qpair;

pub(crate) mod command;
pub(crate) mod telemetry;

mod actor;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use component_core::actor::{Actor, ActorHandle};
use component_core::channel::spsc::SpscChannel;
use component_framework::define_component;
use console_logger::ILogger;
use interfaces::PciAddress;
use spdk_env::ISPDKEnv;

// Re-export interface types from the interfaces crate for consumer convenience.
pub use interfaces::{
    ClientChannels, Command, Completion, IBlockDevice, NamespaceInfo, NvmeBlockError, OpHandle,
    TelemetrySnapshot,
};

use crate::actor::BlockDeviceHandler;
use crate::command::{ClientSession, ControlMessage};
use crate::controller::NvmeController;
use interfaces::IBlockDeviceAdmin;

/// Channel capacity for per-client SPSC channels.
const CLIENT_CHANNEL_CAPACITY: usize = 64;

// SPDK NVMe block device component.
//
// Each instance is associated with a single NVMe controller device.
// The actor thread is pinned to the NUMA node of the controller.
define_component! {
    pub BlockDeviceSpdkNvmeComponentV1 {
        version: "0.1.0",
        provides: [IBlockDevice, IBlockDeviceAdmin],
        receptacles: {
            logger: ILogger,
            spdk_env: ISPDKEnv,
        },
        fields: {
            pci_address: RwLock<Option<PciAddress>>,
            controller_info: RwLock<Option<ControllerSnapshot>>,
            actor_handle: Mutex<Option<ActorHandle<ControlMessage>>>,
            next_client_id: AtomicU64,
            telemetry_stats: Mutex<Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>>,
        },
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ControllerSnapshot {
    pub(crate) sector_size: u32,
    pub(crate) num_sectors: u64,
    pub(crate) max_queue_depth: u32,
    pub(crate) num_io_queues: u32,
    pub(crate) max_transfer_size: u32,
    pub(crate) block_size: u32,
    pub(crate) numa_node: i32,
    pub(crate) nvme_version: controller::NvmeVersion,
}

impl BlockDeviceSpdkNvmeComponentV1 {
    /// Set the PCI address of the NVMe controller to attach to.
    ///
    /// Must be called before [`initialize()`](Self::initialize).
    pub(crate) fn set_pci_address(&self, addr: PciAddress) {
        *self.pci_address.write().expect("pci_address lock poisoned") = Some(addr);
    }

    /// Initialize the component: attach to the NVMe controller and start
    /// the actor thread.
    ///
    /// Must be called after wiring the `logger` and `spdk_env` receptacles.
    ///
    /// # Errors
    ///
    /// Returns [`NvmeBlockError::NotInitialized`] if receptacles are not
    /// wired, or if the SPDK environment is not initialized.
    pub(crate) fn initialize(&self) -> Result<(), NvmeBlockError> {
        if !self.spdk_env.is_connected() {
            return Err(NvmeBlockError::NotInitialized(
                "spdk_env receptacle not connected — wire ISPDKEnv before calling initialize()"
                    .into(),
            ));
        }

        // SPDK probe/attach for our PCI address.
        // SAFETY: SPDK environment is initialized (checked above).
        let (ctrlr_ptr, numa_node) = unsafe { self.probe_controller()? };

        // SAFETY: ctrlr_ptr is valid from probe.
        let controller = unsafe { NvmeController::attach(ctrlr_ptr, numa_node)? };

        // Take a snapshot of controller info for device queries.
        let snapshot = ControllerSnapshot {
            sector_size: controller.sector_size(),
            num_sectors: controller.num_sectors(),
            max_queue_depth: controller.max_queue_depth(),
            num_io_queues: controller.num_io_queues(),
            max_transfer_size: controller.max_transfer_size(),
            block_size: controller.sector_size(),
            numa_node: controller.numa_node(),
            nvme_version: controller.version(),
        };

        *self
            .controller_info
            .write()
            .expect("controller_info lock poisoned") = Some(snapshot.clone());

        // Create the actor handler.
        #[cfg(feature = "telemetry")]
        let telemetry = std::sync::Arc::new(crate::telemetry::TelemetryStats::new());

        #[cfg(feature = "telemetry")]
        let handler =
            BlockDeviceHandler::with_telemetry(controller, std::sync::Arc::clone(&telemetry));

        #[cfg(not(feature = "telemetry"))]
        let handler = BlockDeviceHandler::new(controller);

        // Store telemetry for snapshot queries (type-erased via Any).
        #[cfg(feature = "telemetry")]
        {
            *self
                .telemetry_stats
                .lock()
                .expect("telemetry lock poisoned") =
                Some(telemetry as std::sync::Arc<dyn std::any::Any + Send + Sync>);
        }

        // Create and activate the actor with NUMA affinity.
        let actor: Actor<ControlMessage, BlockDeviceHandler> = Actor::new(handler, |_panic| {});

        // Pin to the NUMA node of the controller.
        let numa = snapshot.numa_node;
        if numa >= 0 {
            if let Ok(topo) = component_core::numa::NumaTopology::discover() {
                if let Some(node) = topo.node(numa as usize) {
                    let cpus = node.cpus();
                    if let Some(cpu) = cpus.iter().next() {
                        if let Ok(cs) = component_core::numa::CpuSet::from_cpu(cpu) {
                            let _ = actor.set_cpu_affinity(cs);
                        }
                    }
                }
            }
        }

        let handle = actor
            .activate()
            .map_err(|e| NvmeBlockError::NotInitialized(e.to_string()))?;

        *self
            .actor_handle
            .lock()
            .expect("actor_handle lock poisoned") = Some(handle);

        Ok(())
    }

    /// Probe for our NVMe controller via SPDK.
    ///
    /// Returns the controller pointer and NUMA node.
    ///
    /// # Safety
    ///
    /// SPDK environment must be initialized.
    unsafe fn probe_controller(
        &self,
    ) -> Result<(*mut spdk_sys::spdk_nvme_ctrlr, i32), NvmeBlockError> {
        // Build the transport ID for our PCI address.
        let mut trid: spdk_sys::spdk_nvme_transport_id = std::mem::zeroed();
        trid.trtype = spdk_sys::spdk_nvme_transport_type_SPDK_NVME_TRANSPORT_PCIE;

        let guard = self.pci_address.read().expect("pci_address lock poisoned");
        let pci_addr = guard.as_ref().ok_or_else(|| {
            NvmeBlockError::NotInitialized(
                "PCI address not set — call set_pci_address() before initialize()".into(),
            )
        })?;
        let addr_str = format!("{}", pci_addr);
        let addr_bytes = addr_str.as_bytes();
        let len = addr_bytes.len().min(trid.traddr.len() - 1);
        for (i, &b) in addr_bytes[..len].iter().enumerate() {
            trid.traddr[i] = b as i8;
        }

        struct ProbeCtx {
            ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
        }

        let mut ctx = ProbeCtx {
            ctrlr_ptr: std::ptr::null_mut(),
        };

        unsafe extern "C" fn probe_cb(
            _cb_ctx: *mut std::ffi::c_void,
            _trid: *const spdk_sys::spdk_nvme_transport_id,
            _opts: *mut spdk_sys::spdk_nvme_ctrlr_opts,
        ) -> bool {
            true // Attach to all discovered controllers.
        }

        unsafe extern "C" fn attach_cb(
            cb_ctx: *mut std::ffi::c_void,
            _trid: *const spdk_sys::spdk_nvme_transport_id,
            ctrlr: *mut spdk_sys::spdk_nvme_ctrlr,
            _opts: *const spdk_sys::spdk_nvme_ctrlr_opts,
        ) {
            let ctx = &mut *(cb_ctx as *mut ProbeCtx);
            ctx.ctrlr_ptr = ctrlr;
        }

        let rc = spdk_sys::spdk_nvme_probe(
            &trid,
            &mut ctx as *mut ProbeCtx as *mut std::ffi::c_void,
            Some(probe_cb),
            Some(attach_cb),
            None,
        );

        if rc != 0 || ctx.ctrlr_ptr.is_null() {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::ProbeFailure(format!(
                    "spdk_nvme_probe for {} failed (rc={rc})",
                    pci_addr,
                )),
            ));
        }

        // NUMA node is not available from minimal bindings; default to 0.
        Ok((ctx.ctrlr_ptr, 0))
    }

    /// Get the controller snapshot, or return NotInitialized error.
    fn controller_snapshot(&self) -> Result<ControllerSnapshot, NvmeBlockError> {
        self.controller_info
            .read()
            .expect("controller_info lock poisoned")
            .clone()
            .ok_or_else(|| {
                NvmeBlockError::NotInitialized(
                    "call initialize() before using device methods".into(),
                )
            })
    }
}

impl IBlockDeviceAdmin for BlockDeviceSpdkNvmeComponentV1 {
    fn set_pci_address(&self, addr: PciAddress) {
        // Inherent method is `pub(crate)`; call it here (impl is in same crate).
        self.set_pci_address(addr);
    }

    fn initialize(&self) -> Result<(), NvmeBlockError> {
        self.initialize()
    }
}

impl IBlockDevice for BlockDeviceSpdkNvmeComponentV1 {
    /// Create a new client connection, returning channel endpoints.
    ///
    /// The returned [`ClientChannels`] contain a `command_tx` for submitting
    /// IO commands and a `completion_rx` for receiving completions. The actor
    /// thread self-polls and processes commands automatically.
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        let handle_guard = self
            .actor_handle
            .lock()
            .expect("actor_handle lock poisoned");
        let handle = handle_guard.as_ref().ok_or_else(|| {
            NvmeBlockError::NotInitialized("call initialize() before connecting clients".into())
        })?;

        let client_id = self.next_client_id.fetch_add(1, Ordering::Relaxed);

        // Create per-client SPSC channels.
        let ingress_ch = SpscChannel::<Command>::new(CLIENT_CHANNEL_CAPACITY);
        let (ingress_tx, ingress_rx) = ingress_ch.split().map_err(|_| {
            NvmeBlockError::ClientDisconnected("failed to create ingress channel".into())
        })?;

        let callback_ch = SpscChannel::<Completion>::new(CLIENT_CHANNEL_CAPACITY);
        let (callback_tx, callback_rx) = callback_ch.split().map_err(|_| {
            NvmeBlockError::ClientDisconnected("failed to create callback channel".into())
        })?;

        // Register the client with the actor.
        let session = ClientSession {
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

    fn sector_size(&self, _ns_id: u32) -> Result<u32, NvmeBlockError> {
        let snap = self.controller_snapshot()?;
        Ok(snap.sector_size)
    }

    fn num_sectors(&self, _ns_id: u32) -> Result<u64, NvmeBlockError> {
        let snap = self.controller_snapshot()?;
        Ok(snap.num_sectors)
    }

    fn max_queue_depth(&self) -> u32 {
        self.controller_snapshot()
            .map(|s| s.max_queue_depth)
            .unwrap_or(0)
    }

    fn num_io_queues(&self) -> u32 {
        self.controller_snapshot()
            .map(|s| s.num_io_queues)
            .unwrap_or(0)
    }

    fn max_transfer_size(&self) -> u32 {
        self.controller_snapshot()
            .map(|s| s.max_transfer_size)
            .unwrap_or(0)
    }

    fn block_size(&self) -> u32 {
        self.controller_snapshot()
            .map(|s| s.block_size)
            .unwrap_or(512)
    }

    fn numa_node(&self) -> i32 {
        self.controller_snapshot()
            .map(|s| s.numa_node)
            .unwrap_or(-1)
    }

    fn nvme_version(&self) -> String {
        self.controller_snapshot()
            .map(|s| s.nvme_version.to_string())
            .unwrap_or_else(|_| "unknown".into())
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
                    crate::telemetry::get_telemetry(stats)
                }
                None => Err(NvmeBlockError::NotInitialized(
                    "call initialize() before querying telemetry".into(),
                )),
            }
        }

        #[cfg(not(feature = "telemetry"))]
        {
            crate::telemetry::telemetry_not_available()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::IUnknown;

    fn make_component() -> std::sync::Arc<BlockDeviceSpdkNvmeComponentV1> {
        BlockDeviceSpdkNvmeComponentV1::new(
            RwLock::new(Some(PciAddress {
                domain: 0,
                bus: 1,
                dev: 0,
                func: 0,
            })),
            RwLock::new(None),
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
    fn component_has_receptacles() {
        let comp = make_component();
        let receps = comp.receptacles();
        assert!(receps.iter().any(|r| r.name == "logger"));
        assert!(receps.iter().any(|r| r.name == "spdk_env"));
    }

    #[test]
    fn controller_snapshot_not_initialized() {
        let comp = make_component();
        let err = comp.controller_snapshot().unwrap_err();
        assert!(matches!(err, NvmeBlockError::NotInitialized(_)));
    }

    #[test]
    fn connect_client_not_initialized() {
        let comp = make_component();
        let err = comp.connect_client().unwrap_err();
        assert!(matches!(err, NvmeBlockError::NotInitialized(_)));
    }

    #[test]
    fn device_info_defaults_when_not_initialized() {
        let comp = make_component();
        assert_eq!(comp.max_queue_depth(), 0);
        assert_eq!(comp.num_io_queues(), 0);
        assert_eq!(comp.max_transfer_size(), 0);
        assert_eq!(comp.block_size(), 512);
        assert_eq!(comp.numa_node(), -1);
        assert_eq!(comp.nvme_version(), "unknown");
    }

    #[test]
    fn telemetry_not_available_without_feature() {
        let comp = make_component();
        let result = comp.telemetry();
        assert!(result.is_err());
    }

    #[test]
    fn controller_snapshot_struct() {
        let snap = ControllerSnapshot {
            sector_size: 4096,
            num_sectors: 1_000_000,
            max_queue_depth: 256,
            num_io_queues: 4,
            max_transfer_size: 131072,
            block_size: 4096,
            numa_node: 0,
            nvme_version: controller::NvmeVersion {
                major: 1,
                minor: 4,
                tertiary: 0,
            },
        };
        let snap2 = snap.clone();
        assert_eq!(snap2.sector_size, 4096);
        assert_eq!(snap2.numa_node, 0);
    }
}
