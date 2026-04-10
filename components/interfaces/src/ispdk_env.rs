use component_macros::define_interface;

use crate::spdk_types::{SpdkEnvError, VfioDevice};

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
