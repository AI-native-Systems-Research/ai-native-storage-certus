use std::fs::OpenOptions;
use std::os::fd::{AsRawFd, OwnedFd};
use nix::libc;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;

use crate::error::Error;
use crate::ioctl;

const DEVICE_PATH: &str = "/dev/nvidia_p2p";
const PAGE_SIZE_64KB: usize = 65536;

/// Page size reported by the NVIDIA P2P driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSize {
    Size4KB = 0,
    Size64KB = 1,
    Size128KB = 2,
}

impl PageSize {
    fn from_raw(val: u32) -> Self {
        match val {
            0 => PageSize::Size4KB,
            1 => PageSize::Size64KB,
            2 => PageSize::Size128KB,
            _ => PageSize::Size64KB,
        }
    }

    /// Size in bytes.
    pub fn bytes(self) -> usize {
        match self {
            PageSize::Size4KB => 4096,
            PageSize::Size64KB => 65536,
            PageSize::Size128KB => 131072,
        }
    }
}

/// Handle to the `/dev/nvidia_p2p` device.
pub struct NvP2pDevice {
    fd: OwnedFd,
}

impl NvP2pDevice {
    /// Open the nvidia_p2p device.
    pub fn open() -> Result<Arc<Self>, Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(DEVICE_PATH)
            .map_err(Error::from)?;
        let fd = OwnedFd::from(file);
        Ok(Arc::new(NvP2pDevice { fd }))
    }

    /// Pin GPU memory and retrieve physical addresses.
    pub fn pin_gpu_memory(self: &Arc<Self>, va: u64, len: u64) -> Result<PinnedMemory, Error> {
        if !va.is_multiple_of(PAGE_SIZE_64KB as u64) {
            return Err(Error::InvalidAlignment);
        }
        if len == 0 || !len.is_multiple_of(PAGE_SIZE_64KB as u64) {
            return Err(Error::InvalidLength);
        }

        let mut pin_args = ioctl::NvP2pPinArgs {
            virtual_address: va,
            length: len,
            ..Default::default()
        };

        unsafe {
            ioctl::pin(self.fd.as_raw_fd(), &mut pin_args)
                .map_err(|e| Error::from(std::io::Error::from(e)))?;
        }

        let page_count = pin_args.page_count as usize;
        let mut phys_addrs: Vec<u64> = vec![0u64; page_count];

        let mut get_args = ioctl::NvP2pGetPagesArgs {
            handle: pin_args.handle,
            phys_addr_buf: phys_addrs.as_mut_ptr() as u64,
            buf_count: page_count as u32,
            ..Default::default()
        };

        unsafe {
            ioctl::get_pages(self.fd.as_raw_fd(), &mut get_args)
                .map_err(|e| Error::from(std::io::Error::from(e)))?;
        }

        phys_addrs.truncate(get_args.entries_written as usize);

        Ok(PinnedMemory {
            device: Arc::clone(self),
            handle: pin_args.handle,
            virtual_address: va,
            length: len,
            page_size: PageSize::from_raw(pin_args.page_size),
            page_count: pin_args.page_count,
            physical_addresses: phys_addrs,
            unpinned: false,
        })
    }
}

/// Metadata for a pinned GPU memory region, returned by `query_pinned_region`.
#[derive(Debug, Clone)]
pub struct RegionMetadata {
    /// Number of pages in the pinned region.
    pub page_count: u32,
    /// Page size used by the driver.
    pub page_size: PageSize,
    /// Physical addresses, one per page.
    pub physical_addresses: Vec<u64>,
    /// GPU UUID (16 bytes).
    pub gpu_uuid: [u8; 16],
}

/// RAII wrapper for a pinned GPU memory region.
///
/// Automatically unpins on Drop. PinnedMemory MUST be dropped before the
/// corresponding CudaMemory allocation is freed.
pub struct PinnedMemory {
    #[allow(dead_code)]
    device: Arc<NvP2pDevice>,
    handle: u64,
    virtual_address: u64,
    length: u64,
    page_size: PageSize,
    page_count: u32,
    physical_addresses: Vec<u64>,
    unpinned: bool,
}

impl std::fmt::Debug for PinnedMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedMemory")
            .field("handle", &self.handle)
            .field("virtual_address", &format_args!("0x{:x}", self.virtual_address))
            .field("length", &self.length)
            .field("page_size", &self.page_size)
            .field("page_count", &self.page_count)
            .field("unpinned", &self.unpinned)
            .finish()
    }
}

impl PinnedMemory {
    /// Physical addresses of pinned pages.
    pub fn physical_addresses(&self) -> &[u64] {
        &self.physical_addresses
    }

    /// Page size used by the NVIDIA driver for this region.
    pub fn page_size(&self) -> PageSize {
        self.page_size
    }

    /// Number of pages in this pinned region.
    pub fn page_count(&self) -> u32 {
        self.page_count
    }

    /// GPU virtual address of this pinned region.
    pub fn virtual_address(&self) -> u64 {
        self.virtual_address
    }

    /// Length in bytes.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Re-query metadata for this pinned region from the kernel.
    pub fn query_metadata(&self) -> Result<RegionMetadata, Error> {
        let mut phys_addrs: Vec<u64> = vec![0u64; self.page_count as usize];

        let mut get_args = ioctl::NvP2pGetPagesArgs {
            handle: self.handle,
            phys_addr_buf: phys_addrs.as_mut_ptr() as u64,
            buf_count: self.page_count,
            ..Default::default()
        };

        unsafe {
            ioctl::get_pages(self.device.fd.as_raw_fd(), &mut get_args)
                .map_err(|e| Error::from(std::io::Error::from(e)))?;
        }

        phys_addrs.truncate(get_args.entries_written as usize);

        Ok(RegionMetadata {
            page_count: get_args.entries_written,
            page_size: PageSize::from_raw(get_args.page_size),
            physical_addresses: phys_addrs,
            gpu_uuid: get_args.gpu_uuid,
        })
    }

    /// Explicitly unpin the memory region.
    pub fn unpin(mut self) -> Result<(), Error> {
        self.do_unpin()?;
        self.unpinned = true;
        Ok(())
    }

    fn do_unpin(&self) -> Result<(), Error> {
        let args = ioctl::NvP2pUnpinArgs {
            handle: self.handle,
        };
        unsafe {
            ioctl::unpin(self.device.fd.as_raw_fd(), &args)
                .map_err(|e| Error::from(std::io::Error::from(e)))?;
        }
        Ok(())
    }
}

impl Drop for PinnedMemory {
    fn drop(&mut self) {
        if !self.unpinned {
            let _ = self.do_unpin();
        }
    }
}
