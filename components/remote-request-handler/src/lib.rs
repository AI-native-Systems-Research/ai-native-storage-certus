//! Remote request handler component for the Certus storage system.
//!
//! Handles incoming cache lookup requests from peer Certus nodes,
//! resolving them against the local dispatcher.
//!
//! # Examples
//!
//! ```no_run
//! use remote_request_handler::RemoteRequestHandlerComponent;
//!
//! let handler = RemoteRequestHandlerComponent::new();
//! // Bind logger and dispatcher receptacles before use.
//! ```

use component_framework::define_component;
use interfaces::{
    CacheKey, IDispatcher, ILogger, IRemoteRequestHandler, RemoteRequestHandlerError,
};

define_component! {
    pub RemoteRequestHandlerComponent {
        version: "0.1.0",
        provides: [IRemoteRequestHandler],
        receptacles: {
            logger: ILogger,
            dispatcher: IDispatcher,
        },
    }
}

impl IRemoteRequestHandler for RemoteRequestHandlerComponent {
    fn handle_lookup(&self, key: CacheKey) -> Result<Vec<u8>, RemoteRequestHandlerError> {
        let _ = key;
        Err(RemoteRequestHandlerError::NotInitialized(
            "not yet implemented".into(),
        ))
    }

    fn handle_check(&self, key: CacheKey) -> Result<bool, RemoteRequestHandlerError> {
        let _ = key;
        Err(RemoteRequestHandlerError::NotInitialized(
            "not yet implemented".into(),
        ))
    }

    fn handle_batch_lookup(
        &self,
        keys: &[CacheKey],
    ) -> Vec<Result<Vec<u8>, RemoteRequestHandlerError>> {
        keys.iter()
            .map(|_| {
                Err(RemoteRequestHandlerError::NotInitialized(
                    "not yet implemented".into(),
                ))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_instantiation() {
        let component = RemoteRequestHandlerComponent::new();
        assert!(component.handle_check(1).is_err());
    }

    #[test]
    fn batch_lookup_returns_per_key_errors() {
        let component = RemoteRequestHandlerComponent::new();
        let results = component.handle_batch_lookup(&[1, 2, 3]);
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.is_err()));
    }
}
