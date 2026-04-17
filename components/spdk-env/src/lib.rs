//! SPDK/DPDK environment component for the Certus system.
//!
//! Provides [`ISPDKEnv`], a component interface for initializing the SPDK/DPDK
//! environment and discovering VFIO-attached devices. The component performs
//! pre-flight checks (VFIO availability, permissions, hugepages) and reports
//! actionable error messages.
//!
//! # Usage
//!
//! ```ignore
//! use spdk_env::{ISPDKEnv, SPDKEnvComponent};
//! use component_framework::prelude::*;
//!
//! let comp = SPDKEnvComponent::new();
//! // comp.logger.connect(logger_arc).unwrap();
//! // let env = query_interface!(comp, ISPDKEnv).unwrap();
//! // env.init().unwrap();
//! // for dev in env.devices() { /* ... */ }
//! ```

pub mod checks;
pub mod device;
pub mod dma;
pub mod env;
pub mod error;

pub use device::{PciAddress, PciId, VfioDevice};
pub use dma::DmaBuffer;
pub use error::SpdkEnvError;

use component_framework::{define_component, define_interface};
use console_logger::ILogger;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

define_interface! {
    pub ISPDKEnv {
        /// Initialize the SPDK/DPDK environment, perform pre-flight checks,
        /// and discover VFIO-attached devices.
        fn init(&self) -> Result<(), SpdkEnvError>;

        /// Return all successfully probed VFIO-attached devices.
        fn devices(&self) -> Vec<VfioDevice>;

        /// Return the number of discovered devices.
        fn device_count(&self) -> usize;

        /// Check whether the SPDK environment has been successfully initialized.
        fn is_initialized(&self) -> bool;
    }
}

define_component! {
    pub SPDKEnvComponent {
        version: "0.1.0",
        provides: [ISPDKEnv],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            discovered_devices: RwLock<Vec<VfioDevice>>,
            initialized: AtomicBool,
        },
    }
}

impl ISPDKEnv for SPDKEnvComponent {
    fn init(&self) -> Result<(), SpdkEnvError> {
        env::do_init(self)
    }

    fn devices(&self) -> Vec<VfioDevice> {
        self.discovered_devices
            .read()
            .expect("devices lock poisoned")
            .clone()
    }

    fn device_count(&self) -> usize {
        self.discovered_devices
            .read()
            .expect("devices lock poisoned")
            .len()
    }

    fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }
}

impl Drop for SPDKEnvComponent {
    fn drop(&mut self) {
        if self.initialized.load(Ordering::Acquire) {
            env::do_fini();
            self.initialized.store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_framework::iunknown::{query, IUnknown};
    use console_logger::{ILogger, LoggerComponent};
    use std::sync::Arc;

    fn make_component() -> Arc<SPDKEnvComponent> {
        SPDKEnvComponent::new(RwLock::new(Vec::new()), AtomicBool::new(false))
    }

    fn make_wired_component() -> Arc<SPDKEnvComponent> {
        let comp = make_component();
        let logger = LoggerComponent::new();
        let ilogger = query::<dyn ILogger + Send + Sync>(&*logger).expect("ILogger not found");
        comp.logger.connect(ilogger).expect("connect failed");
        comp
    }

    // --- Component construction ---

    #[test]
    fn component_new_not_initialized() {
        let comp = make_component();
        assert!(!comp.is_initialized());
    }

    #[test]
    fn component_new_no_devices() {
        let comp = make_component();
        assert_eq!(comp.device_count(), 0);
        assert!(comp.devices().is_empty());
    }

    #[test]
    fn component_version() {
        let comp = make_component();
        assert_eq!(comp.version(), "0.1.0");
    }

    // --- ISPDKEnv interface query ---

    #[test]
    fn component_provides_ispdk_env() {
        let comp = make_component();
        let env = query::<dyn ISPDKEnv + Send + Sync>(&*comp);
        assert!(env.is_some());
    }

    // --- Logger receptacle ---

    #[test]
    fn logger_receptacle_initially_disconnected() {
        let comp = make_component();
        assert!(!comp.logger.is_connected());
    }

    #[test]
    fn logger_receptacle_connect() {
        let comp = make_component();
        let logger = LoggerComponent::new();
        let ilogger = query::<dyn ILogger + Send + Sync>(&*logger).expect("ILogger not found");
        assert!(comp.logger.connect(ilogger).is_ok());
        assert!(comp.logger.is_connected());
    }

    // --- init() pre-flight failures ---

    #[test]
    fn init_fails_without_logger() {
        let comp = make_component();
        let result = comp.init();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, SpdkEnvError::LoggerNotConnected(_)));
    }

    #[test]
    fn init_logger_error_is_actionable() {
        let comp = make_component();
        let err = comp.init().unwrap_err();
        assert!(err.to_string().contains("connect"));
    }

    // --- devices() and device_count() with pre-populated state ---

    #[test]
    fn devices_returns_clone_of_internal_state() {
        let devices = vec![VfioDevice {
            address: PciAddress {
                domain: 0,
                bus: 1,
                dev: 0,
                func: 0,
            },
            id: PciId {
                class_id: 0x010802,
                vendor_id: 0x8086,
                device_id: 0x0a54,
                subvendor_id: 0,
                subdevice_id: 0,
            },
            numa_node: 0,
            device_type: "nvme".into(),
        }];
        let comp = SPDKEnvComponent::new(RwLock::new(devices), AtomicBool::new(false));
        assert_eq!(comp.device_count(), 1);
        assert_eq!(comp.devices().len(), 1);
        assert_eq!(comp.devices()[0].device_type, "nvme");
    }

    #[test]
    fn devices_returns_independent_clone() {
        let devices = vec![VfioDevice {
            address: PciAddress {
                domain: 0,
                bus: 1,
                dev: 0,
                func: 0,
            },
            id: PciId {
                class_id: 0,
                vendor_id: 0,
                device_id: 0,
                subvendor_id: 0,
                subdevice_id: 0,
            },
            numa_node: 0,
            device_type: "test".into(),
        }];
        let comp = SPDKEnvComponent::new(RwLock::new(devices), AtomicBool::new(false));
        let d1 = comp.devices();
        let d2 = comp.devices();
        assert_eq!(d1.len(), d2.len());
        assert_eq!(d1[0].address, d2[0].address);
    }

    #[test]
    fn device_count_matches_devices_len() {
        let devices = vec![
            VfioDevice {
                address: PciAddress {
                    domain: 0,
                    bus: 1,
                    dev: 0,
                    func: 0,
                },
                id: PciId {
                    class_id: 0,
                    vendor_id: 0,
                    device_id: 0,
                    subvendor_id: 0,
                    subdevice_id: 0,
                },
                numa_node: 0,
                device_type: "a".into(),
            },
            VfioDevice {
                address: PciAddress {
                    domain: 0,
                    bus: 2,
                    dev: 0,
                    func: 0,
                },
                id: PciId {
                    class_id: 0,
                    vendor_id: 0,
                    device_id: 0,
                    subvendor_id: 0,
                    subdevice_id: 0,
                },
                numa_node: 1,
                device_type: "b".into(),
            },
        ];
        let comp = SPDKEnvComponent::new(RwLock::new(devices), AtomicBool::new(false));
        assert_eq!(comp.device_count(), 2);
        assert_eq!(comp.devices().len(), 2);
    }

    // --- Drop behavior (without actual SPDK init) ---

    #[test]
    fn drop_does_not_panic_when_not_initialized() {
        let comp = make_component();
        assert!(!comp.is_initialized());
        drop(comp);
    }

    // --- Wired component tests ---

    #[test]
    fn wired_component_has_logger() {
        let comp = make_wired_component();
        assert!(comp.logger.is_connected());
    }

    #[test]
    fn wired_component_still_not_initialized() {
        let comp = make_wired_component();
        // Wiring logger does not auto-initialize.
        assert!(!comp.is_initialized());
    }
}
