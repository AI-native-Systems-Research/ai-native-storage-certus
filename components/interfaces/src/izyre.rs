//! IZyre interface definition.

use std::fmt;

/// Errors returned by `IZyre` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZyreError {
    /// `zyre_new()` returned null (resource exhaustion).
    CreateFailed,
    /// `zyre_start()` returned an error.
    StartFailed(String),
    /// An operation was attempted before the node was started.
    NotStarted,
    /// Configuration validation failed.
    InvalidConfig(String),
    /// A send operation (whisper/shout) failed.
    SendFailed,
    /// Receive returned unexpectedly (node stopped).
    RecvFailed,
}

impl fmt::Display for ZyreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateFailed => write!(f, "failed to create zyre node"),
            Self::StartFailed(reason) => write!(f, "failed to start zyre node: {reason}"),
            Self::NotStarted => write!(f, "node not started"),
            Self::InvalidConfig(reason) => write!(f, "invalid configuration: {reason}"),
            Self::SendFailed => write!(f, "send operation failed"),
            Self::RecvFailed => write!(f, "receive failed (node stopped)"),
        }
    }
}

impl std::error::Error for ZyreError {}

component_macros::define_interface! {
    pub IZyre {
        /// Check if the zyre subsystem is available and healthy.
        fn ping(&self) -> Result<String, ZyreError>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zyre_error_display() {
        assert!(ZyreError::CreateFailed
            .to_string()
            .contains("failed to create zyre node"));
        assert!(ZyreError::InvalidConfig("bad".into())
            .to_string()
            .contains("bad"));
    }
}
