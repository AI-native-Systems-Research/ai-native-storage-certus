use std::os::fd::RawFd;

/// Ioctl magic number for nvidia_p2p_pin device.
const NVP2P_IOC_MAGIC: u8 = b'N';

/// Pin a GPU virtual address range.
#[repr(C)]
#[derive(Debug, Default)]
pub struct NvP2pPinArgs {
    pub virtual_address: u64,
    pub length: u64,
    pub handle: u64,
    pub page_count: u32,
    pub page_size: u32,
}

/// Unpin a previously pinned region.
#[repr(C)]
#[derive(Debug, Default)]
pub struct NvP2pUnpinArgs {
    pub handle: u64,
}

/// Retrieve physical addresses for a pinned region.
#[repr(C)]
#[derive(Debug, Default)]
pub struct NvP2pGetPagesArgs {
    pub handle: u64,
    pub phys_addr_buf: u64,
    pub buf_count: u32,
    pub _pad: u32,
    pub entries_written: u32,
    pub page_size: u32,
    pub gpu_uuid: [u8; 16],
}

nix::ioctl_readwrite!(ioctl_pin, NVP2P_IOC_MAGIC, 1, NvP2pPinArgs);
nix::ioctl_write_ptr!(ioctl_unpin, NVP2P_IOC_MAGIC, 2, NvP2pUnpinArgs);
nix::ioctl_readwrite!(ioctl_get_pages, NVP2P_IOC_MAGIC, 3, NvP2pGetPagesArgs);

/// Execute the PIN ioctl.
///
/// # Safety
/// `fd` must be a valid open file descriptor to `/dev/nvidia_p2p`.
pub unsafe fn pin(fd: RawFd, args: &mut NvP2pPinArgs) -> nix::Result<()> {
    unsafe { ioctl_pin(fd, args) }?;
    Ok(())
}

/// Execute the UNPIN ioctl.
///
/// # Safety
/// `fd` must be a valid open file descriptor to `/dev/nvidia_p2p`.
pub unsafe fn unpin(fd: RawFd, args: &NvP2pUnpinArgs) -> nix::Result<()> {
    unsafe { ioctl_unpin(fd, args) }?;
    Ok(())
}

/// Execute the GET_PAGES ioctl.
///
/// # Safety
/// `fd` must be a valid open file descriptor to `/dev/nvidia_p2p`.
pub unsafe fn get_pages(fd: RawFd, args: &mut NvP2pGetPagesArgs) -> nix::Result<()> {
    unsafe { ioctl_get_pages(fd, args) }?;
    Ok(())
}
