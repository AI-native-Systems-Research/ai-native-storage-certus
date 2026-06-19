//! IRemoteLookup interface and associated types.

use std::fmt;

use crate::idispatch_map::CacheKey;
use crate::idispatcher::IpcHandle;

/// Errors returned by `IRemoteLookup` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteLookupError {
    /// The requested key was not found on the remote node.
    NotFound,
    /// A transport or network error occurred.
    TransportError(String),
}

impl fmt::Display for RemoteLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "key not found"),
            Self::TransportError(msg) => write!(f, "transport error: {msg}"),
        }
    }
}

impl std::error::Error for RemoteLookupError {}

component_macros::define_interface! {
    pub IRemoteLookup {
        /// Batch lookup: retrieve multiple cache entries from remote nodes.
        ///
        /// Accepts the same parameter types as `IDispatcher::batch_lookup`.
        /// Returns one `Result` per input entry, preserving positional order.
        ///
        /// # Examples
        ///
        /// ```
        /// use interfaces::{CacheKey, IpcHandle, IRemoteLookup, RemoteLookupError};
        ///
        /// # fn example(rl: &dyn IRemoteLookup) {
        /// let mut buf = vec![0u8; 4096];
        /// let entries: Vec<(CacheKey, IpcHandle)> = vec![
        ///     (1, IpcHandle { address: buf.as_mut_ptr(), size: 4096 }),
        /// ];
        /// let results = rl.batch_lookup(&entries);
        /// assert_eq!(results.len(), entries.len());
        /// # }
        /// ```
        fn batch_lookup(
            &self,
            entries: &[(CacheKey, IpcHandle)],
        ) -> Vec<Result<(), RemoteLookupError>>;

        /// Join a cluster of Certus nodes at the given endpoint.
        ///
        /// # Examples
        ///
        /// ```
        /// use interfaces::{IRemoteLookup, RemoteLookupError};
        ///
        /// # fn example(rl: &dyn IRemoteLookup) -> Result<(), RemoteLookupError> {
        /// rl.join_cluster("192.168.1.10:9090")?;
        /// # Ok(())
        /// # }
        /// ```
        fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>;

        /// Leave the cluster, disconnecting from remote nodes.
        ///
        /// # Examples
        ///
        /// ```
        /// use interfaces::{IRemoteLookup, RemoteLookupError};
        ///
        /// # fn example(rl: &dyn IRemoteLookup) -> Result<(), RemoteLookupError> {
        /// rl.leave_cluster()?;
        /// # Ok(())
        /// # }
        /// ```
        fn leave_cluster(&self) -> Result<(), RemoteLookupError>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_lookup_error_display() {
        assert!(RemoteLookupError::NotFound.to_string().contains("not found"));
        assert!(RemoteLookupError::TransportError("timeout".into())
            .to_string()
            .contains("timeout"));
    }
}
