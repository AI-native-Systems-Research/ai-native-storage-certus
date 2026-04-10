use component_macros::define_interface;

use crate::spdk_types::{BlockDeviceError, DmaBuffer};

define_interface! {
    pub IBlockDevice {
        /// Open the block device: probe NVMe, attach controller, open namespace 1.
        ///
        /// Requires that `spdk_env` and `logger` receptacles are connected, and
        /// that the SPDK environment has been initialized.
        fn open(&self) -> Result<(), BlockDeviceError>;

        /// Read sectors starting at `lba` into a DMA buffer (zero-copy).
        ///
        /// `buf.len()` must be a positive multiple of [`sector_size()`].
        fn read_blocks(&self, lba: u64, buf: &mut DmaBuffer) -> Result<(), BlockDeviceError>;

        /// Write sectors starting at `lba` from a DMA buffer (zero-copy).
        ///
        /// `buf.len()` must be a positive multiple of [`sector_size()`].
        fn write_blocks(&self, lba: u64, buf: &DmaBuffer) -> Result<(), BlockDeviceError>;

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
