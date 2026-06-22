use component_macros::define_interface;

use crate::spdk_types::{SpdkEnvError, VfioDevice};

define_interface! {
    pub ISPDKEnv {
        /// Initialize the SPDK/DPDK environment, perform pre-flight checks,
        /// and discover VFIO-attached devices.
        fn init(&self) -> Result<(), SpdkEnvError>;

        /// Tear down the SPDK/DPDK environment explicitly.
        ///
        /// Must be called after all NVMe controllers have been detached and all
        /// DMA buffers freed. This prevents DPDK atexit handlers from accessing
        /// freed resources on process exit.
        fn fini(&self);

        /// Return all successfully probed VFIO-attached devices.
        fn devices(&self) -> Vec<VfioDevice>;

        /// Return the number of discovered devices.
        fn device_count(&self) -> usize;

        /// Check whether the SPDK environment has been successfully initialized.
        fn is_initialized(&self) -> bool;
    }
}
