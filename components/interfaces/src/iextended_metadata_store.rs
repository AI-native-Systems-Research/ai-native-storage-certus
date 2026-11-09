use component_macros::define_interface;
use std::fmt;

/// Errors returned by `IExtendedMetadataStore` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtendedMetadataStoreError {
    /// The requested key was not found.
    NotFound,
    /// A storage or I/O error occurred.
    StorageError(String),
    /// The store has reached its capacity limit.
    CapacityExhausted,
    /// The value exceeds the maximum allowed size.
    ValueTooLarge,
}

impl fmt::Display for ExtendedMetadataStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "key not found"),
            Self::StorageError(msg) => write!(f, "storage error: {msg}"),
            Self::CapacityExhausted => write!(f, "store capacity exhausted"),
            Self::ValueTooLarge => write!(f, "value exceeds maximum size"),
        }
    }
}

impl std::error::Error for ExtendedMetadataStoreError {}

define_interface! {
    pub IExtendedMetadataStore {
        /// Store a metadata entry by key.
        fn put(&self, key: &str, value: &[u8]) -> Result<(), ExtendedMetadataStoreError>;

        /// Retrieve a metadata entry by key.
        fn get(&self, key: &str) -> Result<Vec<u8>, ExtendedMetadataStoreError>;

        /// Delete a metadata entry by key.
        fn delete(&self, key: &str) -> Result<(), ExtendedMetadataStoreError>;

        /// Iterate over all key-value pairs in the store. Returns a snapshot-at-call-time view.
        fn iterate_all(&self) -> Result<Vec<(String, Vec<u8>)>, ExtendedMetadataStoreError>;

        /// Flush all pending writes to persistent storage. Returns when all data is durable.
        fn force_flush(&self) -> Result<(), ExtendedMetadataStoreError>;
    }
}
