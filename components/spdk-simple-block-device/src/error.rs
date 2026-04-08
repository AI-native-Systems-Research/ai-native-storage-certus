//! Error types for the simple block device component.

use std::fmt;

/// Error conditions reported by the simple block device component.
///
/// Each variant carries a descriptive message with actionable guidance.
#[derive(Debug, Clone)]
pub enum BlockDeviceError {
    /// The block device has not been opened yet.
    NotOpen(String),
    /// The block device is already open.
    AlreadyOpen(String),
    /// NVMe probe/attach failed — no controller found.
    ProbeFailure(String),
    /// No active NVMe namespace found on the controller.
    NamespaceNotFound(String),
    /// Failed to allocate an I/O queue pair.
    QpairAllocationFailed(String),
    /// A read I/O operation failed.
    ReadFailed(String),
    /// A write I/O operation failed.
    WriteFailed(String),
    /// The supplied buffer size does not match the required sector-aligned size.
    BufferSizeMismatch(String),
    /// Failed to allocate DMA-safe memory for I/O buffers.
    DmaAllocationFailed(String),
    /// The SPDK environment receptacle is not connected or not initialized.
    EnvNotInitialized(String),
    /// The logger receptacle is not connected.
    LoggerNotConnected(String),
}

impl fmt::Display for BlockDeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockDeviceError::NotOpen(msg) => write!(f, "Block device not open: {msg}"),
            BlockDeviceError::AlreadyOpen(msg) => write!(f, "Block device already open: {msg}"),
            BlockDeviceError::ProbeFailure(msg) => write!(f, "NVMe probe failed: {msg}"),
            BlockDeviceError::NamespaceNotFound(msg) => {
                write!(f, "NVMe namespace not found: {msg}")
            }
            BlockDeviceError::QpairAllocationFailed(msg) => {
                write!(f, "I/O queue pair allocation failed: {msg}")
            }
            BlockDeviceError::ReadFailed(msg) => write!(f, "Read failed: {msg}"),
            BlockDeviceError::WriteFailed(msg) => write!(f, "Write failed: {msg}"),
            BlockDeviceError::BufferSizeMismatch(msg) => {
                write!(f, "Buffer size mismatch: {msg}")
            }
            BlockDeviceError::DmaAllocationFailed(msg) => {
                write!(f, "DMA allocation failed: {msg}")
            }
            BlockDeviceError::EnvNotInitialized(msg) => {
                write!(f, "SPDK environment not initialized: {msg}")
            }
            BlockDeviceError::LoggerNotConnected(msg) => {
                write!(f, "Logger not connected: {msg}")
            }
        }
    }
}

impl std::error::Error for BlockDeviceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_not_open() {
        let e = BlockDeviceError::NotOpen("call open() first".into());
        assert_eq!(e.to_string(), "Block device not open: call open() first");
    }

    #[test]
    fn error_display_already_open() {
        let e = BlockDeviceError::AlreadyOpen("close first".into());
        assert_eq!(e.to_string(), "Block device already open: close first");
    }

    #[test]
    fn error_display_probe_failure() {
        let e = BlockDeviceError::ProbeFailure("no devices".into());
        assert_eq!(e.to_string(), "NVMe probe failed: no devices");
    }

    #[test]
    fn error_display_namespace_not_found() {
        let e = BlockDeviceError::NamespaceNotFound("ns1 inactive".into());
        assert_eq!(e.to_string(), "NVMe namespace not found: ns1 inactive");
    }

    #[test]
    fn error_display_qpair_failed() {
        let e = BlockDeviceError::QpairAllocationFailed("returned null".into());
        assert_eq!(
            e.to_string(),
            "I/O queue pair allocation failed: returned null"
        );
    }

    #[test]
    fn error_display_read_failed() {
        let e = BlockDeviceError::ReadFailed("completion error".into());
        assert_eq!(e.to_string(), "Read failed: completion error");
    }

    #[test]
    fn error_display_write_failed() {
        let e = BlockDeviceError::WriteFailed("completion error".into());
        assert_eq!(e.to_string(), "Write failed: completion error");
    }

    #[test]
    fn error_display_buffer_mismatch() {
        let e = BlockDeviceError::BufferSizeMismatch("expected 4096, got 1024".into());
        assert_eq!(
            e.to_string(),
            "Buffer size mismatch: expected 4096, got 1024"
        );
    }

    #[test]
    fn error_display_dma_failed() {
        let e = BlockDeviceError::DmaAllocationFailed("spdk_dma_zmalloc returned null".into());
        assert_eq!(
            e.to_string(),
            "DMA allocation failed: spdk_dma_zmalloc returned null"
        );
    }

    #[test]
    fn error_display_env_not_initialized() {
        let e = BlockDeviceError::EnvNotInitialized("connect and init ISPDKEnv first".into());
        assert_eq!(
            e.to_string(),
            "SPDK environment not initialized: connect and init ISPDKEnv first"
        );
    }

    #[test]
    fn error_display_logger_not_connected() {
        let e = BlockDeviceError::LoggerNotConnected("connect ILogger first".into());
        assert_eq!(
            e.to_string(),
            "Logger not connected: connect ILogger first"
        );
    }

    #[test]
    fn error_is_std_error() {
        let e: Box<dyn std::error::Error> =
            Box::new(BlockDeviceError::ReadFailed("test".into()));
        assert!(e.to_string().contains("test"));
    }

    #[test]
    fn error_clone() {
        let e = BlockDeviceError::WriteFailed("clone test".into());
        let e2 = e.clone();
        assert_eq!(e.to_string(), e2.to_string());
    }

    #[test]
    fn error_debug() {
        let e = BlockDeviceError::ProbeFailure("debug test".into());
        let dbg = format!("{:?}", e);
        assert!(dbg.contains("ProbeFailure"));
        assert!(dbg.contains("debug test"));
    }

    #[test]
    fn error_all_variants_are_std_error() {
        let variants: Vec<Box<dyn std::error::Error>> = vec![
            Box::new(BlockDeviceError::NotOpen("a".into())),
            Box::new(BlockDeviceError::AlreadyOpen("b".into())),
            Box::new(BlockDeviceError::ProbeFailure("c".into())),
            Box::new(BlockDeviceError::NamespaceNotFound("d".into())),
            Box::new(BlockDeviceError::QpairAllocationFailed("e".into())),
            Box::new(BlockDeviceError::ReadFailed("f".into())),
            Box::new(BlockDeviceError::WriteFailed("g".into())),
            Box::new(BlockDeviceError::BufferSizeMismatch("h".into())),
            Box::new(BlockDeviceError::DmaAllocationFailed("i".into())),
            Box::new(BlockDeviceError::EnvNotInitialized("j".into())),
            Box::new(BlockDeviceError::LoggerNotConnected("k".into())),
        ];
        for e in &variants {
            assert!(!e.to_string().is_empty());
        }
    }
}
