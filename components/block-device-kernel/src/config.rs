//! Device configuration and block device management.

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{FromRawFd, IntoRawFd, OwnedFd};
use std::path::{Path, PathBuf};

// BLKGETSIZE64 ioctl number: _IOR(0x12, 114, size_t) on Linux x86_64
const BLKGETSIZE64: libc::c_ulong = 0x80081272;

/// Configuration for the kernel block device.
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    device_path: PathBuf,
    block_size: u32,
    num_blocks: u64,
    total_bytes: u64,
}

impl DeviceConfig {
    /// Create and validate a new device configuration.
    ///
    /// The path must refer to an existing Linux block device. If `num_blocks`
    /// is 0, the device size is auto-detected via `BLKGETSIZE64`.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `block_size` is less than 512
    /// - `block_size` is not a power of 2
    /// - The path is not a block device
    /// - The device size cannot be queried
    pub fn new(device_path: PathBuf, block_size: u32, num_blocks: u64) -> Result<Self, String> {
        if block_size < 512 {
            return Err(format!("block_size must be >= 512, got {block_size}"));
        }
        if !block_size.is_power_of_two() {
            return Err(format!("block_size must be a power of 2, got {block_size}"));
        }

        if num_blocks == 0 {
            let total_bytes = query_device_size(&device_path, block_size)?;
            let detected_blocks = total_bytes / block_size as u64;
            return Ok(Self {
                device_path,
                block_size,
                num_blocks: detected_blocks,
                total_bytes,
            });
        }

        let total_bytes = (block_size as u64).checked_mul(num_blocks).ok_or_else(|| {
            format!("block_size({block_size}) * num_blocks({num_blocks}) overflows u64")
        })?;

        Ok(Self {
            device_path,
            block_size,
            num_blocks,
            total_bytes,
        })
    }

    #[inline]
    pub fn device_path(&self) -> &Path {
        &self.device_path
    }

    #[inline]
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    #[inline]
    pub fn num_blocks(&self) -> u64 {
        self.num_blocks
    }

    #[inline]
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// Verify that a path is a block device using stat(2).
fn assert_block_device(path: &Path) -> Result<(), String> {
    let mut stat_buf: libc::stat = unsafe { std::mem::zeroed() };
    let c_path = std::ffi::CString::new(path.to_str().ok_or_else(|| {
        format!("path contains non-UTF8 characters: {}", path.display())
    })?)
    .map_err(|e| format!("path contains null byte: {e}"))?;

    // SAFETY: c_path is a valid C string, stat_buf is zeroed and valid.
    let ret = unsafe { libc::stat(c_path.as_ptr(), &mut stat_buf) };
    if ret < 0 {
        return Err(format!(
            "stat({}) failed: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }

    let mode = stat_buf.st_mode;
    if (mode & libc::S_IFMT) != libc::S_IFBLK {
        return Err(format!(
            "{} is not a block device (mode=0o{:o})",
            path.display(),
            mode
        ));
    }

    Ok(())
}

/// Query the size of a block device via ioctl BLKGETSIZE64.
fn query_device_size(path: &Path, block_size: u32) -> Result<u64, String> {
    assert_block_device(path)?;

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("failed to open {} for size query: {e}", path.display()))?;

    let fd = file.into_raw_fd();
    let mut size: u64 = 0;

    // SAFETY: fd is valid, size is a valid pointer to u64.
    let ret = unsafe { libc::ioctl(fd, BLKGETSIZE64, &mut size) };

    // SAFETY: fd is valid, we obtained it from File::into_raw_fd.
    unsafe { libc::close(fd) };

    if ret < 0 {
        return Err(format!(
            "BLKGETSIZE64 ioctl failed on {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }

    if size == 0 {
        return Err(format!("device {} has zero size", path.display()));
    }

    if size % block_size as u64 != 0 {
        return Err(format!(
            "device size {} is not a multiple of block_size {}",
            size, block_size
        ));
    }

    Ok(size)
}

/// Open a block device with O_DIRECT for direct IO.
///
/// The path must be a block device accessible by the current user.
/// Opens with O_DIRECT to bypass the kernel page cache, then drops
/// any stale cached pages via `posix_fadvise(POSIX_FADV_DONTNEED)`.
pub fn open_block_device(cfg: &DeviceConfig) -> Result<OwnedFd, String> {
    let path = cfg.device_path();

    assert_block_device(path)?;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_DIRECT | libc::O_DSYNC)
        .open(path)
        .map_err(|e| format!("failed to open block device {}: {e}", path.display()))?;

    let raw_fd = file.into_raw_fd();

    // Verify O_DIRECT is actually set on the fd.
    // SAFETY: raw_fd is valid.
    let flags = unsafe { libc::fcntl(raw_fd, libc::F_GETFL) };
    if flags < 0 || (flags & libc::O_DIRECT) == 0 {
        unsafe { libc::close(raw_fd) };
        return Err(format!(
            "O_DIRECT not active on {} (flags=0x{:x})",
            path.display(),
            flags
        ));
    }

    // Drop any stale page-cache pages for this device so reads cannot
    // be served from a prior buffered-IO session.
    // SAFETY: raw_fd is valid, range covers entire device.
    unsafe {
        libc::posix_fadvise(raw_fd, 0, 0, libc::POSIX_FADV_DONTNEED);
    }

    // SAFETY: raw_fd is valid, we just obtained it from File::into_raw_fd.
    Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_size_too_small() {
        let err = DeviceConfig::new(PathBuf::from("/dev/nvme0n1"), 256, 100).unwrap_err();
        assert!(err.contains("must be >= 512"));
    }

    #[test]
    fn block_size_not_power_of_two() {
        let err = DeviceConfig::new(PathBuf::from("/dev/nvme0n1"), 1000, 100).unwrap_err();
        assert!(err.contains("power of 2"));
    }

    #[test]
    fn rejects_non_block_device() {
        let err = DeviceConfig::new(PathBuf::from("/dev/null"), 4096, 0).unwrap_err();
        assert!(err.contains("not a block device"));
    }

    #[test]
    fn valid_config_with_explicit_blocks() {
        let cfg = DeviceConfig::new(PathBuf::from("/dev/nvme0n1"), 4096, 1024).unwrap();
        assert_eq!(cfg.block_size(), 4096);
        assert_eq!(cfg.num_blocks(), 1024);
        assert_eq!(cfg.total_bytes(), 4096 * 1024);
    }
}
