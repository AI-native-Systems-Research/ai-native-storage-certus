//! Centralized interface trait definitions for the Certus component system.
//!
//! This crate defines all component interface traits (`ILogger`, `IGreeter`,
//! `ISPDKEnv`, `IBlockDevice`) in one place, allowing components to depend on
//! interface definitions without pulling in implementation crates.
//!
//! SPDK-dependent interfaces and types are gated behind the `spdk` Cargo feature.

mod ilogger;
mod igreeter;

pub use ilogger::ILogger;
pub use igreeter::IGreeter;

#[cfg(feature = "spdk")]
pub mod spdk_types;

#[cfg(feature = "spdk")]
mod ispdk_env;

#[cfg(feature = "spdk")]
mod iblock_device;

#[cfg(feature = "spdk")]
pub use spdk_types::{
    BlockDeviceError, DmaBuffer, PciAddress, PciId, SpdkEnvError, VfioDevice,
};

#[cfg(feature = "spdk")]
pub use ispdk_env::ISPDKEnv;

#[cfg(feature = "spdk")]
pub use iblock_device::IBlockDevice;
