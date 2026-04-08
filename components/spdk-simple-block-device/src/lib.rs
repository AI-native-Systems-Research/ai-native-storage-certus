//! Simple NVMe block device component for the Certus system.
//!
//! Provides [`IBlockDevice`], a component interface for synchronous block I/O
//! over SPDK's NVMe driver. The component probes the first NVMe controller on
//! the local PCIe bus, opens namespace 1, and exposes read/write at LBA
//! granularity.
//!
//! # Prerequisites
//!
//! - The SPDK environment must be initialized via [`spdk_env::ISPDKEnv`] before
//!   calling [`IBlockDevice::open`].
//! - NVMe devices must be bound to `vfio-pci` and hugepages must be configured.
//!
//! # Usage
//!
//! ```ignore
//! use spdk_simple_block_device::{IBlockDevice, SimpleBlockDevice};
//! use spdk_env::{ISPDKEnv, SPDKEnvComponent};
//! use example_logger::{ILogger, LoggerComponent};
//! use component_framework::prelude::*;
//!
//! // Create and wire components.
//! let logger = LoggerComponent::new();
//! let env_comp = SPDKEnvComponent::new(Default::default(), Default::default());
//! let bdev = SimpleBlockDevice::new(Default::default());
//!
//! // Wire receptacles...
//! // env_comp.logger.connect(query::<dyn ILogger>(&*logger).unwrap()).unwrap();
//! // bdev.logger.connect(query::<dyn ILogger>(&*logger).unwrap()).unwrap();
//! // bdev.spdk_env.connect(query::<dyn ISPDKEnv>(&*env_comp).unwrap()).unwrap();
//!
//! // Initialize env, then open the block device.
//! // env_comp.init().unwrap();
//! // bdev.open().unwrap();
//!
//! // Read/write at sector granularity.
//! // let mut buf = vec![0u8; bdev.sector_size() as usize];
//! // bdev.read_blocks(0, &mut buf).unwrap();
//! ```

pub mod actor;
pub mod error;
pub mod io;

pub use actor::{BlockDeviceClient, BlockDeviceHandler, BlockIoRequest, DeviceInfo};
pub use error::BlockDeviceError;

use component_framework::{define_component, define_interface};
use example_logger::ILogger;
use spdk_env::ISPDKEnv;
use std::sync::Mutex;

/// Type alias for the internal device state. Use [`default_device_state()`]
/// when constructing a new `SimpleBlockDevice`.
pub type DeviceState = Mutex<Option<io::InnerState>>;

/// Create the default (closed) device state for `SimpleBlockDevice::new()`.
pub fn default_device_state() -> DeviceState {
    Mutex::new(None)
}

define_interface! {
    pub IBlockDevice {
        /// Open the block device: probe NVMe, attach controller, open namespace 1.
        ///
        /// Requires that `spdk_env` and `logger` receptacles are connected, and
        /// that the SPDK environment has been initialized.
        fn open(&self) -> Result<(), BlockDeviceError>;

        /// Read sectors starting at `lba` into `buf`.
        ///
        /// `buf.len()` must be a positive multiple of [`sector_size()`].
        fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError>;

        /// Write sectors starting at `lba` from `buf`.
        ///
        /// `buf.len()` must be a positive multiple of [`sector_size()`].
        fn write_blocks(&self, lba: u64, buf: &[u8]) -> Result<(), BlockDeviceError>;

        /// Close the block device: free the I/O queue pair and detach the controller.
        fn close(&self) -> Result<(), BlockDeviceError>;

        /// Return the sector size in bytes (e.g., 512 or 4096). Returns 0 if not open.
        fn sector_size(&self) -> u32;

        /// Return the total number of sectors. Returns 0 if not open.
        fn num_sectors(&self) -> u64;

        /// Check whether the block device is currently open.
        fn is_open(&self) -> bool;
    }
}

define_component! {
    pub SimpleBlockDevice {
        version: "0.1.0",
        provides: [IBlockDevice],
        receptacles: {
            spdk_env: ISPDKEnv,
            logger: ILogger,
        },
        fields: {
            inner: Mutex<Option<io::InnerState>>,
        },
    }
}

impl IBlockDevice for SimpleBlockDevice {
    fn open(&self) -> Result<(), BlockDeviceError> {
        if !self.logger.is_connected() {
            return Err(BlockDeviceError::LoggerNotConnected(
                "Logger receptacle not connected.".into(),
            ));
        }
        let env = self
            .spdk_env
            .get()
            .map_err(|_| BlockDeviceError::EnvNotInitialized("spdk_env receptacle not connected.".into()))?;

        let mut guard = self.inner.lock().expect("inner lock poisoned");
        if guard.is_some() {
            return Err(BlockDeviceError::AlreadyOpen(
                "Block device already open. Call close() first.".into(),
            ));
        }

        let state = io::open_device(&*env)?;
        *guard = Some(state);
        Ok(())
    }

    fn read_blocks(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let guard = self.inner.lock().expect("inner lock poisoned");
        let state = guard
            .as_ref()
            .ok_or_else(|| BlockDeviceError::NotOpen("Block device not open.".into()))?;
        io::read_blocks(state, lba, buf)
    }

    fn write_blocks(&self, lba: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        let guard = self.inner.lock().expect("inner lock poisoned");
        let state = guard
            .as_ref()
            .ok_or_else(|| BlockDeviceError::NotOpen("Block device not open.".into()))?;
        io::write_blocks(state, lba, buf)
    }

    fn close(&self) -> Result<(), BlockDeviceError> {
        let mut guard = self.inner.lock().expect("inner lock poisoned");
        let state = guard
            .take()
            .ok_or_else(|| BlockDeviceError::NotOpen("Block device not open.".into()))?;
        io::close_device(state);
        Ok(())
    }

    fn sector_size(&self) -> u32 {
        let guard = self.inner.lock().expect("inner lock poisoned");
        guard.as_ref().map_or(0, |s| s.sector_size)
    }

    fn num_sectors(&self) -> u64 {
        let guard = self.inner.lock().expect("inner lock poisoned");
        guard.as_ref().map_or(0, |s| s.num_sectors)
    }

    fn is_open(&self) -> bool {
        let guard = self.inner.lock().expect("inner lock poisoned");
        guard.is_some()
    }
}

impl Drop for SimpleBlockDevice {
    fn drop(&mut self) {
        let mut guard = self.inner.lock().expect("inner lock poisoned");
        if let Some(state) = guard.take() {
            eprintln!("[spdk-simple-block-device] Dropping open device, cleaning up...");
            // SAFETY: qpair and ctrlr are valid (device was open).
            unsafe {
                spdk_sys::spdk_nvme_ctrlr_free_io_qpair(state.qpair);
                spdk_sys::spdk_nvme_detach(state.ctrlr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_framework::iunknown::{query, IUnknown};
    use example_logger::{ILogger, LoggerComponent};
    use std::sync::Arc;

    fn make_component() -> Arc<SimpleBlockDevice> {
        SimpleBlockDevice::new(Mutex::new(None))
    }

    fn make_wired_component() -> Arc<SimpleBlockDevice> {
        let comp = make_component();
        let logger = LoggerComponent::new();
        let ilogger =
            query::<dyn ILogger + Send + Sync>(&*logger).expect("ILogger not found");
        comp.logger.connect(ilogger).expect("connect failed");
        comp
    }

    // --- Component construction ---

    #[test]
    fn component_not_open_initially() {
        let comp = make_component();
        assert!(!comp.is_open());
    }

    #[test]
    fn component_sector_size_zero_when_closed() {
        let comp = make_component();
        assert_eq!(comp.sector_size(), 0);
    }

    #[test]
    fn component_num_sectors_zero_when_closed() {
        let comp = make_component();
        assert_eq!(comp.num_sectors(), 0);
    }

    #[test]
    fn component_version() {
        let comp = make_component();
        assert_eq!(comp.version(), "0.1.0");
    }

    // --- IBlockDevice interface query ---

    #[test]
    fn component_provides_iblock_device() {
        let comp = make_component();
        let iface = query::<dyn IBlockDevice + Send + Sync>(&*comp);
        assert!(iface.is_some());
    }

    // --- Receptacles ---

    #[test]
    fn logger_receptacle_initially_disconnected() {
        let comp = make_component();
        assert!(!comp.logger.is_connected());
    }

    #[test]
    fn spdk_env_receptacle_initially_disconnected() {
        let comp = make_component();
        assert!(!comp.spdk_env.is_connected());
    }

    #[test]
    fn logger_receptacle_connect() {
        let comp = make_wired_component();
        assert!(comp.logger.is_connected());
    }

    // --- open() pre-flight failures ---

    #[test]
    fn open_fails_without_logger() {
        let comp = make_component();
        let result = comp.open();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BlockDeviceError::LoggerNotConnected(_)
        ));
    }

    #[test]
    fn open_fails_without_spdk_env() {
        let comp = make_wired_component();
        let result = comp.open();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BlockDeviceError::EnvNotInitialized(_)
        ));
    }

    // --- read/write/close when not open ---

    #[test]
    fn read_fails_when_not_open() {
        let comp = make_component();
        let mut buf = vec![0u8; 512];
        let result = comp.read_blocks(0, &mut buf);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BlockDeviceError::NotOpen(_)
        ));
    }

    #[test]
    fn write_fails_when_not_open() {
        let comp = make_component();
        let buf = vec![0u8; 512];
        let result = comp.write_blocks(0, &buf);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BlockDeviceError::NotOpen(_)
        ));
    }

    #[test]
    fn close_fails_when_not_open() {
        let comp = make_component();
        let result = comp.close();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BlockDeviceError::NotOpen(_)
        ));
    }

    // --- Drop when not open ---

    #[test]
    fn drop_does_not_panic_when_not_open() {
        let comp = make_component();
        assert!(!comp.is_open());
        drop(comp);
    }
}
