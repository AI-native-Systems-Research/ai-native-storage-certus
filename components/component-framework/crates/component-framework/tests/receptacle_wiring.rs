use component_core::error::ReceptacleError;
use component_core::iunknown::{query, IUnknown};
use component_macros::{define_component, define_interface};
use std::sync::Arc;

define_interface! {
    pub ILogger {
        fn log(&self, msg: &str) -> String;
        fn flush(&self);
    }
}

define_interface! {
    pub IStorage {
        fn read(&self, key: &str) -> Option<Vec<u8>>;
    }
}

// Component A provides ILogger
define_component! {
    pub LoggerComponent {
        version: "1.0.0",
        provides: [ILogger],
    }
}

impl ILogger for LoggerComponent {
    fn log(&self, msg: &str) -> String {
        format!("LOG: {msg}")
    }
    fn flush(&self) {}
}

// Component B requires ILogger via receptacle
define_component! {
    pub ConsumerComponent {
        version: "1.0.0",
        provides: [IStorage],
        receptacles: {
            logger: ILogger,
        },
    }
}

impl IStorage for ConsumerComponent {
    fn read(&self, key: &str) -> Option<Vec<u8>> {
        // Use the logger receptacle if connected
        if let Ok(logger) = self.logger.get() {
            let _ = logger.log(&format!("reading {key}"));
        }
        Some(key.as_bytes().to_vec())
    }
}

// T043: Two-component wiring with method dispatch
#[test]
fn two_component_wiring() {
    let logger = LoggerComponent::new();
    let consumer = ConsumerComponent::new();

    // Get ILogger from logger component
    let ilogger: Arc<dyn ILogger + Send + Sync> =
        query::<dyn ILogger + Send + Sync>(&*logger).unwrap();

    // Connect consumer's receptacle
    consumer.logger.connect(ilogger.clone()).unwrap();
    assert!(consumer.logger.is_connected());

    // Method dispatch through receptacle
    let provider = consumer.logger.get().unwrap();
    assert_eq!(provider.log("test"), "LOG: test");
}

#[test]
fn receptacle_disconnect_and_reconnect() {
    let logger = LoggerComponent::new();
    let consumer = ConsumerComponent::new();

    let ilogger: Arc<dyn ILogger + Send + Sync> =
        query::<dyn ILogger + Send + Sync>(&*logger).unwrap();

    consumer.logger.connect(ilogger.clone()).unwrap();
    consumer.logger.disconnect().unwrap();

    // After disconnect, get returns error
    assert!(matches!(
        consumer.logger.get(),
        Err(ReceptacleError::NotConnected)
    ));

    // Can reconnect
    consumer.logger.connect(ilogger).unwrap();
    assert!(consumer.logger.is_connected());
}

#[test]
fn receptacle_already_connected_error() {
    let logger = LoggerComponent::new();
    let consumer = ConsumerComponent::new();

    let ilogger: Arc<dyn ILogger + Send + Sync> =
        query::<dyn ILogger + Send + Sync>(&*logger).unwrap();

    consumer.logger.connect(ilogger.clone()).unwrap();
    assert_eq!(
        consumer.logger.connect(ilogger),
        Err(ReceptacleError::AlreadyConnected)
    );
}

// T049: receptacles() returns list with correct names
#[test]
fn receptacles_metadata() {
    let consumer = ConsumerComponent::new();
    let receps = consumer.receptacles();
    assert_eq!(receps.len(), 1);
    assert_eq!(receps[0].name, "logger");
    assert_eq!(receps[0].interface_name, "ILogger");
}

#[test]
fn component_with_receptacle_used_via_provided_interface() {
    let logger = LoggerComponent::new();
    let consumer = ConsumerComponent::new();

    let ilogger: Arc<dyn ILogger + Send + Sync> =
        query::<dyn ILogger + Send + Sync>(&*logger).unwrap();
    consumer.logger.connect(ilogger).unwrap();

    // Use consumer through its provided IStorage interface
    let storage: Arc<dyn IStorage + Send + Sync> =
        query::<dyn IStorage + Send + Sync>(&*consumer).unwrap();
    let result = storage.read("mykey").unwrap();
    assert_eq!(result, b"mykey");
}

// Thread safety: can share components across threads
#[test]
fn receptacle_across_threads() {
    let logger = LoggerComponent::new();
    let consumer = ConsumerComponent::new();

    let ilogger: Arc<dyn ILogger + Send + Sync> =
        query::<dyn ILogger + Send + Sync>(&*logger).unwrap();
    consumer.logger.connect(ilogger).unwrap();

    let consumer_clone = consumer.clone();
    let handle = std::thread::spawn(move || {
        let provider = consumer_clone.logger.get().unwrap();
        provider.log("from thread")
    });

    let result = handle.join().unwrap();
    assert_eq!(result, "LOG: from thread");
}
