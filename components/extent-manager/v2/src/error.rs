use interfaces::{ExtentManagerError, NvmeBlockError};

pub(crate) fn offset_not_found(offset: u64) -> ExtentManagerError {
    ExtentManagerError::OffsetNotFound(offset)
}

pub(crate) fn out_of_space() -> ExtentManagerError {
    ExtentManagerError::OutOfSpace
}

pub(crate) fn not_initialized(msg: &str) -> ExtentManagerError {
    ExtentManagerError::NotInitialized(msg.to_string())
}

pub(crate) fn io_error(msg: &str) -> ExtentManagerError {
    ExtentManagerError::IoError(msg.to_string())
}

pub(crate) fn corrupt_metadata(msg: &str) -> ExtentManagerError {
    ExtentManagerError::CorruptMetadata(msg.to_string())
}

pub(crate) fn nvme_to_em(e: NvmeBlockError) -> ExtentManagerError {
    ExtentManagerError::IoError(e.to_string())
}
