//! Remote lookup component.
//!
//! Provides remote key-value lookup capability via the `IRemoteLookup` interface.

use component_framework::define_component;
use interfaces::{ILogger, IRemoteLookup, RemoteLookupError};

define_component! {
    pub RemoteLookupComponent {
        version: "0.1.0",
        provides: [IRemoteLookup],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            connected: std::sync::atomic::AtomicBool,
        },
    }
}

impl IRemoteLookup for RemoteLookupComponent {
    fn lookup(&self, _key: &str) -> Result<Vec<u8>, RemoteLookupError> {
        if !self.is_connected() {
            return Err(RemoteLookupError::NotConnected);
        }
        Err(RemoteLookupError::NotFound)
    }

    fn exists(&self, _key: &str) -> Result<bool, RemoteLookupError> {
        if !self.is_connected() {
            return Err(RemoteLookupError::NotConnected);
        }
        Ok(false)
    }

    fn connect(&self, endpoint: &str) -> Result<(), RemoteLookupError> {
        if let Ok(logger) = self.logger.get() {
            logger.info(&format!("remote-lookup: connecting to {endpoint}"));
        }
        self.connected
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn disconnect(&self) -> Result<(), RemoteLookupError> {
        if let Ok(logger) = self.logger.get() {
            logger.info("remote-lookup: disconnecting");
        }
        self.connected
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    fn setup() -> std::sync::Arc<RemoteLookupComponent> {
        RemoteLookupComponent::new_default()
    }

    #[test]
    fn starts_disconnected() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();
        assert!(!rl.is_connected());
    }

    #[test]
    fn lookup_fails_when_disconnected() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();
        assert_eq!(rl.lookup("key"), Err(RemoteLookupError::NotConnected));
    }

    #[test]
    fn connect_and_disconnect() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        rl.connect("localhost:8080").unwrap();
        assert!(rl.is_connected());

        rl.disconnect().unwrap();
        assert!(!rl.is_connected());
    }

    #[test]
    fn exists_returns_false_when_connected() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        rl.connect("localhost:8080").unwrap();
        assert_eq!(rl.exists("missing"), Ok(false));
    }
}
