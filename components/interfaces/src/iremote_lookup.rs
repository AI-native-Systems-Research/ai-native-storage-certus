//! IRemoteLookup interface and associated types.

use std::fmt;

/// Errors returned by `IRemoteLookup` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteLookupError {
    /// The connection to the remote service is not established.
    NotConnected,
    /// The requested key was not found.
    NotFound,
    /// A transport or network error occurred.
    TransportError(String),
}

impl fmt::Display for RemoteLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => write!(f, "not connected to remote service"),
            Self::NotFound => write!(f, "key not found"),
            Self::TransportError(msg) => write!(f, "transport error: {msg}"),
        }
    }
}

impl std::error::Error for RemoteLookupError {}

component_macros::define_interface! {
    pub IRemoteLookup {
        /// Look up a value by key from the remote store.
        fn lookup(&self, key: &str) -> Result<Vec<u8>, RemoteLookupError>;

        /// Check whether a key exists in the remote store.
        fn exists(&self, key: &str) -> Result<bool, RemoteLookupError>;

        /// Connect to the remote service at the given endpoint.
        fn connect(&self, endpoint: &str) -> Result<(), RemoteLookupError>;

        /// Disconnect from the remote service.
        fn disconnect(&self) -> Result<(), RemoteLookupError>;

        /// Return whether the component is currently connected.
        fn is_connected(&self) -> bool;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_lookup_error_display() {
        assert!(RemoteLookupError::NotConnected
            .to_string()
            .contains("not connected"));
        assert!(RemoteLookupError::NotFound.to_string().contains("not found"));
        assert!(RemoteLookupError::TransportError("timeout".into())
            .to_string()
            .contains("timeout"));
    }
}
