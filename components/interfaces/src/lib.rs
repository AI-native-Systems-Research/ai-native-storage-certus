//! Centralized interface trait definitions for the Certus component system.
//!
//! This crate defines all component interface traits (`IGreeter`,
//! `ISPDKEnv`, `IBlockDevice`) in one place, allowing components to depend on
//! interface definitions without pulling in implementation crates.
//!
//! SPDK-dependent interfaces and types are gated behind the `spdk` Cargo feature.

mod idispatch_map;
mod igreeter;
mod ilogger;

pub use idispatch_map::CacheKey;
pub use idispatch_map::DispatchMapError;
#[cfg(feature = "spdk")]
pub use idispatch_map::LookupResult;
#[cfg(feature = "spdk")]
pub use idispatch_map::IDispatchMap;
pub use igreeter::IGreeter;
pub use ilogger::ILogger;

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
