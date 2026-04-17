use interfaces::ExtentManagerError;
use interfaces::NvmeBlockError;

pub(crate) fn duplicate_key(key: u64) -> ExtentManagerError {
    ExtentManagerError::DuplicateKey(key)
}

pub(crate) fn key_not_found(key: u64) -> ExtentManagerError {
    ExtentManagerError::KeyNotFound(key)
}

pub(crate) fn invalid_size_class(size: u32) -> ExtentManagerError {
    ExtentManagerError::InvalidSizeClass(size)
}

pub(crate) fn out_of_space(size_class: u32) -> ExtentManagerError {
    ExtentManagerError::OutOfSpace { size_class }
}

pub(crate) fn not_initialized(msg: &str) -> ExtentManagerError {
    ExtentManagerError::NotInitialized(msg.to_string())
}

#[allow(dead_code)]
pub(crate) fn io_error(msg: &str) -> ExtentManagerError {
    ExtentManagerError::IoError(msg.to_string())
}

#[allow(dead_code)]
pub(crate) fn corrupt_metadata(msg: &str) -> ExtentManagerError {
    ExtentManagerError::CorruptMetadata(msg.to_string())
}

pub(crate) fn nvme_to_em(e: NvmeBlockError) -> ExtentManagerError {
    ExtentManagerError::from(e)
}
