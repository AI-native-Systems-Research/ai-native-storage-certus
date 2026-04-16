//! Error types for the extent manager component.
//!
//! The canonical [`ExtentManagerError`] definition lives in the shared
//! `interfaces` crate. This module re-exports it for internal convenience.
//!
//! The `From<NvmeBlockError>` conversion is provided by the interfaces
//! crate (behind the `spdk` feature gate).

pub use interfaces::ExtentManagerError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_duplicate_key() {
        let err = ExtentManagerError::DuplicateKey(42);
        assert_eq!(err.to_string(), "duplicate key: 42");
    }

    #[test]
    fn display_key_not_found() {
        let err = ExtentManagerError::KeyNotFound(99);
        assert_eq!(err.to_string(), "key not found: 99");
    }

    #[test]
    fn display_invalid_size_class() {
        let err = ExtentManagerError::InvalidSizeClass(5);
        assert_eq!(err.to_string(), "invalid size class: 5");
    }

    #[test]
    fn display_out_of_space() {
        let err = ExtentManagerError::OutOfSpace { size_class: 0 };
        assert_eq!(err.to_string(), "out of space for size class: 0");
    }

    #[test]
    fn display_io_error() {
        let err = ExtentManagerError::IoError("write failed".into());
        assert_eq!(err.to_string(), "I/O error: write failed");
    }

    #[test]
    fn display_corrupt_metadata() {
        let err = ExtentManagerError::CorruptMetadata("CRC mismatch".into());
        assert_eq!(err.to_string(), "corrupt metadata: CRC mismatch");
    }

    #[test]
    fn display_not_initialized() {
        let err = ExtentManagerError::NotInitialized("block_device not connected".into());
        assert!(err.to_string().contains("not initialized"));
    }

    #[test]
    fn error_is_clone() {
        let err = ExtentManagerError::DuplicateKey(1);
        let err2 = err.clone();
        assert_eq!(err.to_string(), err2.to_string());
    }

    #[test]
    fn error_is_debug() {
        let err = ExtentManagerError::KeyNotFound(7);
        let debug = format!("{err:?}");
        assert!(debug.contains("KeyNotFound"));
    }
}
