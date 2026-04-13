use nix::libc;
use std::fmt;
use std::io;

/// Error type for nvidia-p2p-pin operations.
#[derive(Debug)]
pub enum Error {
    /// `/dev/nvidia_p2p` does not exist or cannot be opened.
    DeviceNotFound,
    /// Caller lacks CAP_SYS_RAWIO.
    PermissionDenied,
    /// GPU virtual address not 64KB-aligned.
    InvalidAlignment,
    /// Length not a positive multiple of 64KB.
    InvalidLength,
    /// Handle not found or already freed.
    InvalidHandle,
    /// Overlapping VA range already pinned on this fd.
    AlreadyPinned,
    /// NVIDIA driver out of memory.
    OutOfMemory,
    /// Unexpected NVIDIA driver error code.
    DriverError(i32),
    /// System-level I/O error.
    IoError(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DeviceNotFound => write!(f, "/dev/nvidia_p2p device not found"),
            Error::PermissionDenied => write!(f, "permission denied (need CAP_SYS_RAWIO)"),
            Error::InvalidAlignment => write!(f, "GPU virtual address not 64KB-aligned"),
            Error::InvalidLength => write!(f, "length must be a positive multiple of 64KB"),
            Error::InvalidHandle => write!(f, "handle not found or already freed"),
            Error::AlreadyPinned => write!(f, "overlapping VA range already pinned"),
            Error::OutOfMemory => write!(f, "NVIDIA driver out of memory"),
            Error::DriverError(code) => write!(f, "NVIDIA driver error: {}", code),
            Error::IoError(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::IoError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.raw_os_error() {
            Some(libc::EACCES) | Some(libc::EPERM) => Error::PermissionDenied,
            Some(libc::ENOENT) | Some(libc::ENODEV) => Error::DeviceNotFound,
            Some(libc::EINVAL) => Error::InvalidAlignment,
            Some(libc::EEXIST) => Error::AlreadyPinned,
            Some(libc::ENOMEM) => Error::OutOfMemory,
            _ => Error::IoError(e),
        }
    }
}
