//! IRemoteRequestHandler interface for handling remote cache requests.
//!
//! Lookup operations return zero-copy references to data in the memory-tier
//! pool. The caller MUST call `release_lookup` after consuming the data
//! (e.g., after an RDMA Write completes) to release the read reference
//! and allow eviction.

use std::fmt;

use crate::idispatch_map::CacheKey;

/// A zero-copy reference to cached data in the memory-tier pool.
///
/// The pointer is valid until `release_lookup(key)` is called. Holding this
/// reference prevents eviction of the entry.
///
/// # Safety
///
/// The caller must not dereference `ptr` after calling `release_lookup(key)`.
///
/// # Examples
///
/// ```no_run
/// use interfaces::{IRemoteRequestHandler, LookupRef};
///
/// fn use_ref(handler: &dyn IRemoteRequestHandler) {
///     let lookup = handler.handle_lookup(42).unwrap();
///     // ... RDMA Write from lookup.ptr, lookup.size bytes ...
///     handler.release_lookup(lookup.key);
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LookupRef {
    /// Pointer to data in the memory-tier pool.
    pub ptr: *const u8,
    /// Size of the data in bytes.
    pub size: u32,
    /// The cache key (for passing to `release_lookup`).
    pub key: CacheKey,
}

// SAFETY: The pointer references memory in the memory-tier pool which is
// a long-lived mmap'd region. A read reference held by the dispatch-map
// prevents eviction while this struct is live.
unsafe impl Send for LookupRef {}
unsafe impl Sync for LookupRef {}

/// Errors returned by `IRemoteRequestHandler` operations.
///
/// # Examples
///
/// ```
/// use interfaces::RemoteRequestHandlerError;
///
/// let err = RemoteRequestHandlerError::InvalidRequest("missing key".into());
/// assert!(err.to_string().contains("invalid request"));
/// ```
#[derive(Debug, Clone)]
pub enum RemoteRequestHandlerError {
    /// The request payload was malformed or missing required fields.
    InvalidRequest(String),
    /// The requested cache key was not found locally.
    KeyNotFound(CacheKey),
    /// An internal dispatch error occurred.
    DispatchError(String),
    /// The handler is not initialized or missing required receptacles.
    NotInitialized(String),
}

impl fmt::Display for RemoteRequestHandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(msg) => write!(f, "invalid request: {msg}"),
            Self::KeyNotFound(k) => write!(f, "key not found: {k}"),
            Self::DispatchError(msg) => write!(f, "dispatch error: {msg}"),
            Self::NotInitialized(msg) => write!(f, "not initialized: {msg}"),
        }
    }
}

impl std::error::Error for RemoteRequestHandlerError {}

component_macros::define_interface! {
    pub IRemoteRequestHandler {
        /// Look up a cache key and return a zero-copy reference to the data.
        ///
        /// The returned `LookupRef` contains a pointer to data in the memory-tier
        /// pool. The caller MUST call `release_lookup(key)` after the data has been
        /// consumed (e.g., after RDMA Write completes).
        ///
        /// # Errors
        ///
        /// Returns [`RemoteRequestHandlerError::KeyNotFound`] if the key is not cached.
        /// Returns [`RemoteRequestHandlerError::NotInitialized`] if called before binding.
        fn handle_lookup(&self, key: CacheKey) -> Result<LookupRef, RemoteRequestHandlerError>;

        /// Check whether a cache key exists locally without acquiring a reference.
        ///
        /// # Errors
        ///
        /// Returns [`RemoteRequestHandlerError::NotInitialized`] if called before binding.
        fn handle_check(&self, key: CacheKey) -> Result<bool, RemoteRequestHandlerError>;

        /// Look up a batch of cache keys, returning zero-copy references.
        ///
        /// Returns one result per input key, in the same order. Each successful
        /// result holds a read reference that must be released via `release_lookup`.
        fn handle_batch_lookup(
            &self,
            keys: &[CacheKey],
        ) -> Vec<Result<LookupRef, RemoteRequestHandlerError>>;

        /// Release the read reference acquired by `handle_lookup` or `handle_batch_lookup`.
        ///
        /// Must be called after the RDMA Write (or other data consumption) is complete.
        /// Failing to call this blocks eviction of the entry.
        fn release_lookup(&self, key: CacheKey);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_invalid_request() {
        let err = RemoteRequestHandlerError::InvalidRequest("bad payload".into());
        assert!(err.to_string().contains("invalid request"));
    }

    #[test]
    fn error_display_key_not_found() {
        let err = RemoteRequestHandlerError::KeyNotFound(42);
        assert!(err.to_string().contains("42"));
    }

    #[test]
    fn error_display_dispatch_error() {
        let err = RemoteRequestHandlerError::DispatchError("timeout".into());
        assert!(err.to_string().contains("dispatch error"));
    }

    #[test]
    fn error_display_not_initialized() {
        let err = RemoteRequestHandlerError::NotInitialized("no dispatcher".into());
        assert!(err.to_string().contains("not initialized"));
    }

    #[test]
    fn lookup_ref_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LookupRef>();
    }
}
