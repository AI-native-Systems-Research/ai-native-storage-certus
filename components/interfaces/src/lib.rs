//! Centralized interface trait definitions for the Certus component system.
//!
//! This crate defines all component interface traits (`IGreeter`,
//! `ISPDKEnv`, `IBlockDevice`) in one place, allowing components to depend on
//! interface definitions without pulling in implementation crates.
//!
//! SPDK-dependent interfaces and types are gated behind the `spdk` Cargo feature.

mod idispatch_map;
mod idispatcher;
mod ieviction_policy;
mod igpu_services;
mod igreeter;
mod ilogger;
mod imemory_tier;
mod iremote_lookup;
mod ipartition_table;
mod iremote_request_handler;

pub use idispatch_map::CacheKey;
pub use idispatch_map::DispatchMapError;
#[cfg(feature = "spdk")]
pub use idispatch_map::LookupResult;
#[cfg(feature = "spdk")]
pub use idispatch_map::IDispatchMap;
pub use idispatcher::DispatcherConfig;
pub use idispatcher::DispatcherError;
pub use idispatcher::IpcHandle;
#[cfg(feature = "spdk")]
pub use idispatcher::IDispatcher;
pub use igpu_services::GpuDeviceInfo;
pub use igpu_services::GpuDmaBuffer;
pub use igpu_services::GpuIpcHandle;
pub use igpu_services::GpuStream;
pub use igpu_services::IGpuServices;
pub use ieviction_policy::EvictionHandle;
pub use ieviction_policy::EvictionPolicyError;
pub use ieviction_policy::IEvictionPolicy;
pub use ieviction_policy::PoolId;
pub use igreeter::IGreeter;
pub use ilogger::ILogger;
pub use iremote_lookup::IRemoteLookup;
pub use iremote_lookup::RemoteLookupError;
pub use iremote_request_handler::IRemoteRequestHandler;
pub use iremote_request_handler::LookupRef;
pub use iremote_request_handler::RemoteRequestHandlerError;
pub use imemory_tier::MemoryTierError;
#[cfg(feature = "spdk")]
pub use imemory_tier::IMemoryTier;

#[cfg(feature = "spdk")]
pub mod spdk_types;

#[cfg(feature = "spdk")]
mod ispdk_env;

#[cfg(feature = "spdk")]
pub mod iblock_device;
mod iextent_manager;

#[cfg(feature = "spdk")]
pub use spdk_types::DmaAllocFn;
#[cfg(feature = "spdk")]
pub use spdk_types::{is_spdk_env_active, set_spdk_env_active};
#[cfg(feature = "spdk")]
pub use spdk_types::{BlockDeviceError, DmaBuffer, PciAddress, PciId, SpdkEnvError, VfioDevice};

#[cfg(feature = "spdk")]
pub use ispdk_env::ISPDKEnv;

#[cfg(feature = "spdk")]
pub use iblock_device::{
    ClientChannels, Command, Completion, IBlockDevice, NamespaceInfo, NvmeBlockError, OpHandle,
    TelemetrySnapshot,
};

#[cfg(feature = "spdk")]
pub use iblock_device::IBlockDeviceAdmin;

pub use iextent_manager::Extent;
pub use iextent_manager::ExtentKey;
pub use iextent_manager::ExtentManagerError;
pub use iextent_manager::FormatParams;
#[cfg(feature = "spdk")]
pub use iextent_manager::IExtentManager;
pub use iextent_manager::WriteHandle;

pub use ipartition_table::PartitionConfig;
pub use ipartition_table::PartitionInfo;
pub use ipartition_table::PartitionSpec;
pub use ipartition_table::PartitionTable;
pub use ipartition_table::PartitionTableError;
pub use ipartition_table::type_guids;
#[cfg(feature = "spdk")]
pub use ipartition_table::IPartitionTable;
