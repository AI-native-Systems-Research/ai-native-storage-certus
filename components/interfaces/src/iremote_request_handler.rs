//! IRemoteRequestHandler interface for handling remote cache requests.

use std::fmt;

use crate::idispatch_map::CacheKey;

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
        /// Handle a remote lookup request for the given cache key.
        ///
        /// Returns the cached data as a byte vector if found locally.
        ///
        /// # Errors
        ///
        /// Returns [`RemoteRequestHandlerError::KeyNotFound`] if the key is not cached.
        /// Returns [`RemoteRequestHandlerError::NotInitialized`] if called before binding.
        fn handle_lookup(&self, key: CacheKey) -> Result<Vec<u8>, RemoteRequestHandlerError>;

        /// Handle a remote check request — returns whether the key exists locally.
        ///
        /// # Errors
        ///
        /// Returns [`RemoteRequestHandlerError::NotInitialized`] if called before binding.
        fn handle_check(&self, key: CacheKey) -> Result<bool, RemoteRequestHandlerError>;

        /// Handle a batch of remote lookup requests.
        ///
        /// Returns one result per input key, in the same order.
        fn handle_batch_lookup(
            &self,
            keys: &[CacheKey],
        ) -> Vec<Result<Vec<u8>, RemoteRequestHandlerError>>;
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
}
