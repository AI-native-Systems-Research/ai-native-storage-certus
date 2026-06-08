//! Device configuration and backing file management.

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{FromRawFd, IntoRawFd, OwnedFd};
use std::path::{Path, PathBuf};

/// Configuration for the file-backed block device.
///
/// # Examples
///
/// ```
/// use block_device_filesys::config::DeviceConfig;
/// use std::path::PathBuf;
///
/// let cfg = DeviceConfig::new(PathBuf::from("/tmp/dev.img"), 4096, 1024).unwrap();
/// assert_eq!(cfg.block_size(), 4096);
/// assert_eq!(cfg.num_blocks(), 1024);
/// assert_eq!(cfg.total_bytes(), 4096 * 1024);
/// ```
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    file_path: PathBuf,
    block_size: u32,
    num_blocks: u64,
    total_bytes: u64,
}

impl DeviceConfig {
    /// Create and validate a new device configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `block_size` is less than 512
    /// - `block_size` is not a power of 2
    /// - `num_blocks` is 0
    /// - `block_size * num_blocks` overflows u64
    ///
    /// # Examples
    ///
    /// ```
    /// use block_device_filesys::config::DeviceConfig;
    /// use std::path::PathBuf;
    ///
    /// // Valid configuration
    /// let cfg = DeviceConfig::new(PathBuf::from("/tmp/dev.img"), 512, 2048);
    /// assert!(cfg.is_ok());
    ///
    /// // Invalid: block_size not power of 2
    /// let cfg = DeviceConfig::new(PathBuf::from("/tmp/dev.img"), 500, 1024);
    /// assert!(cfg.is_err());
    ///
    /// // Invalid: zero blocks
    /// let cfg = DeviceConfig::new(PathBuf::from("/tmp/dev.img"), 4096, 0);
    /// assert!(cfg.is_err());
    /// ```
    pub fn new(file_path: PathBuf, block_size: u32, num_blocks: u64) -> Result<Self, String> {
        if block_size < 512 {
            return Err(format!("block_size must be >= 512, got {block_size}"));
        }
        if !block_size.is_power_of_two() {
            return Err(format!("block_size must be a power of 2, got {block_size}"));
        }
        if num_blocks == 0 {
            return Err("num_blocks must be > 0".into());
        }

        let total_bytes = (block_size as u64).checked_mul(num_blocks).ok_or_else(|| {
            format!("block_size({block_size}) * num_blocks({num_blocks}) overflows u64")
        })?;

        Ok(Self {
            file_path,
            block_size,
            num_blocks,
            total_bytes,
        })
    }

    /// Return the backing file path.
    #[inline]
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }

    /// Return the block size in bytes.
    #[inline]
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Return the total number of blocks.
    #[inline]
    pub fn num_blocks(&self) -> u64 {
        self.num_blocks
    }

    /// Return the total device size in bytes.
    #[inline]
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

/// Open or create the backing file according to the device configuration.
///
/// - If the file does not exist: creates it and pre-allocates to full size via fallocate.
/// - If the file exists: verifies its size matches the expected total_bytes.
/// - Opens with O_DIRECT to bypass the kernel page cache. Falls back to buffered IO
///   on filesystems that do not support O_DIRECT (e.g., tmpfs).
///
/// Returns an owned file descriptor on success.
pub fn open_or_create_backing_file(cfg: &DeviceConfig) -> Result<OwnedFd, String> {
    let path = cfg.file_path();
    let total = cfg.total_bytes();

    if path.exists() {
        let file = try_open_direct(path, false)?;

        let meta = file
            .metadata()
            .map_err(|e| format!("failed to stat {}: {e}", path.display()))?;

        let actual_size = meta.len();
        if actual_size != total {
            return Err(format!(
                "backing file size mismatch: expected {total} bytes, got {actual_size} bytes \
                 (path: {})",
                path.display()
            ));
        }

        let raw_fd = file.into_raw_fd();
        // SAFETY: raw_fd is valid, we just obtained it from File::into_raw_fd.
        Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
    } else {
        let parent = path.parent().ok_or_else(|| {
            format!(
                "invalid file path (no parent directory): {}",
                path.display()
            )
        })?;
        if !parent.exists() {
            return Err(format!(
                "parent directory does not exist: {}",
                parent.display()
            ));
        }

        let file = try_open_direct(path, true)?;
        let raw_fd = file.into_raw_fd();

        // SAFETY: raw_fd is valid. fallocate pre-allocates disk space without writing.
        let ret = unsafe { libc::fallocate(raw_fd, 0, 0, total as libc::off_t) };
        if ret != 0 {
            let errno = std::io::Error::last_os_error();
            // Clean up the created file on failure.
            unsafe { libc::close(raw_fd) };
            let _ = std::fs::remove_file(path);
            return Err(format!("fallocate({total} bytes) failed: {errno}"));
        }

        // SAFETY: raw_fd is valid from OpenOptions above.
        Ok(unsafe { OwnedFd::from_raw_fd(raw_fd) })
    }
}

/// Try to open a file with O_DIRECT; fall back to buffered IO if the filesystem
/// does not support direct IO (EINVAL).
fn try_open_direct(path: &Path, create: bool) -> Result<std::fs::File, String> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true).custom_flags(libc::O_DIRECT);
    if create {
        opts.create_new(true);
    }

    match opts.open(path) {
        Ok(f) => Ok(f),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {
            eprintln!(
                "WARNING: O_DIRECT not supported on {} — falling back to buffered IO. \
                 Benchmark results will be unreliable due to OS page cache.",
                path.display()
            );
            let mut opts2 = OpenOptions::new();
            opts2.read(true).write(true);
            if create {
                opts2.create_new(true);
            }
            opts2
                .open(path)
                .map_err(|e2| format!("failed to open {}: {e2}", path.display()))
        }
        Err(e) => Err(format!("failed to open {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_config() {
        let cfg = DeviceConfig::new(PathBuf::from("/tmp/test.img"), 4096, 1024).unwrap();
        assert_eq!(cfg.block_size(), 4096);
        assert_eq!(cfg.num_blocks(), 1024);
        assert_eq!(cfg.total_bytes(), 4096 * 1024);
    }

    #[test]
    fn block_size_too_small() {
        let err = DeviceConfig::new(PathBuf::from("/tmp/t.img"), 256, 100).unwrap_err();
        assert!(err.contains("must be >= 512"));
    }

    #[test]
    fn block_size_not_power_of_two() {
        let err = DeviceConfig::new(PathBuf::from("/tmp/t.img"), 1000, 100).unwrap_err();
        assert!(err.contains("power of 2"));
    }

    #[test]
    fn zero_blocks() {
        let err = DeviceConfig::new(PathBuf::from("/tmp/t.img"), 512, 0).unwrap_err();
        assert!(err.contains("must be > 0"));
    }

    #[test]
    fn default_block_size_512() {
        let cfg = DeviceConfig::new(PathBuf::from("/tmp/t.img"), 512, 2048).unwrap();
        assert_eq!(cfg.total_bytes(), 512 * 2048);
    }
}
