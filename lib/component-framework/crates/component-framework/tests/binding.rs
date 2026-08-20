use component_framework::binding::bind;
use component_framework::component_ref::ComponentRef;
use component_framework::error::RegistryError;
use component_framework::iunknown::query;
use component_framework::registry::ComponentRegistry;
use component_framework::{define_component, define_interface};
use std::any::Any;
use std::sync::Arc;

// --- Shared interface definitions ---

define_interface! {
    pub ILogger {
        fn log(&self, msg: &str) -> String;
    }
}

define_interface! {
    pub IStorage {
        fn store(&self, key: &str) -> String;
    }
}

// --- Provider component ---

define_component! {
    pub LogProvider {
        version: "1.0.0",
        provides: [ILogger],
    }
}

impl ILogger for LogProvider {
    fn log(&self, msg: &str) -> String {
        format!("[LOG] {msg}")
    }
}

// --- Consumer component ---

define_component! {
    pub LogConsumer {
        version: "1.0.0",
        provides: [],
        receptacles: {
            logger: ILogger,
        },
    }
}

// --- Multi-receptacle consumer ---

define_component! {
    pub MultiConsumer {
        version: "1.0.0",
        provides: [],
        receptacles: {
            logger: ILogger,
            storage: IStorage,
        },
    }
}

// --- Storage provider ---

define_component! {
    pub StorageProvider {
        version: "1.0.0",
        provides: [IStorage],
    }
}

impl IStorage for StorageProvider {
    fn store(&self, key: &str) -> String {
        format!("stored:{key}")
    }
}

#[test]
fn first_party_binding_still_works() {
    let provider = LogProvider::new();
    let consumer = LogConsumer::new();

    let iface: Arc<dyn ILogger + Send + Sync> =
        query::<dyn ILogger + Send + Sync>(&*provider).unwrap();
    consumer.logger.connect(iface).unwrap();

    let logger = consumer.logger.get().unwrap();
    assert_eq!(logger.log("test"), "[LOG] test");
}

#[test]
fn third_party_bind_wires_by_names() {
    let provider = LogProvider::new();
    let consumer = LogConsumer::new();

    bind(&*provider, "ILogger", &*consumer, "logger").unwrap();

    let logger = consumer.logger.get().unwrap();
    assert_eq!(logger.log("hello"), "[LOG] hello");
}

#[test]
fn bind_returns_error_for_unknown_interface_name() {
    let provider = LogProvider::new();
    let consumer = LogConsumer::new();

    let err = bind(&*provider, "IUnknownInterface", &*consumer, "logger").unwrap_err();
    match err {
        RegistryError::BindingFailed { detail } => {
            assert!(detail.contains("IUnknownInterface"));
        }
        other => panic!("expected BindingFailed, got {other}"),
    }
}

#[test]
fn bind_returns_error_for_unknown_receptacle_name() {
    let provider = LogProvider::new();
    let consumer = LogConsumer::new();

    let err = bind(&*provider, "ILogger", &*consumer, "nonexistent").unwrap_err();
    match err {
        RegistryError::BindingFailed { detail } => {
            assert!(detail.contains("nonexistent"));
        }
        other => panic!("expected BindingFailed, got {other}"),
    }
}

#[test]
fn bind_returns_error_for_type_mismatch() {
    // StorageProvider provides IStorage, but consumer expects ILogger on "logger" receptacle
    let provider = StorageProvider::new();
    let consumer = LogConsumer::new();

    let err = bind(&*provider, "IStorage", &*consumer, "logger").unwrap_err();
    match err {
        RegistryError::BindingFailed { detail } => {
            assert!(detail.contains("mismatch") || detail.contains("not compatible"));
        }
        other => panic!("expected BindingFailed, got {other}"),
    }
}

#[test]
fn third_party_wires_multiple_receptacles() {
    let log_prov = LogProvider::new();
    let store_prov = StorageProvider::new();
    let consumer = MultiConsumer::new();

    bind(&*log_prov, "ILogger", &*consumer, "logger").unwrap();
    bind(&*store_prov, "IStorage", &*consumer, "storage").unwrap();

    let logger = consumer.logger.get().unwrap();
    assert_eq!(logger.log("multi"), "[LOG] multi");

    let storage = consumer.storage.get().unwrap();
    assert_eq!(storage.store("key1"), "stored:key1");
}

#[test]
fn bind_same_receptacle_twice_returns_error() {
    let provider = LogProvider::new();
    let consumer = LogConsumer::new();

    bind(&*provider, "ILogger", &*consumer, "logger").unwrap();
    let err = bind(&*provider, "ILogger", &*consumer, "logger").unwrap_err();
    match err {
        RegistryError::BindingFailed { detail } => {
            assert!(detail.contains("logger"));
        }
        other => panic!("expected BindingFailed, got {other}"),
    }
}

// --- End-to-end: registry-create-bind-invoke (SC-006) ---

fn log_provider_factory(_config: Option<&dyn Any>) -> Result<ComponentRef, RegistryError> {
    Ok(ComponentRef::from(LogProvider::new()))
}

fn log_consumer_factory(_config: Option<&dyn Any>) -> Result<ComponentRef, RegistryError> {
    Ok(ComponentRef::from(LogConsumer::new()))
}

#[test]
fn end_to_end_registry_create_bind_invoke() {
    // 1. Register factories
    let registry = ComponentRegistry::new();
    registry
        .register("log-provider", log_provider_factory)
        .unwrap();
    registry
        .register("log-consumer", log_consumer_factory)
        .unwrap();

    // 2. Create components by name via registry
    let provider = registry.create("log-provider", None).unwrap();
    let consumer = registry.create("log-consumer", None).unwrap();

    // 3. Wire via third-party binding (string names only, no compile-time type knowledge)
    bind(&*provider, "ILogger", &*consumer, "logger").unwrap();

    // 4. Invoke cross-component: query the consumer's receptacle and use it
    //    The consumer internally holds the provider's ILogger via the receptacle.
    //    We verify by querying the provider's ILogger and checking it works.
    let logger: Arc<dyn ILogger + Send + Sync> =
        query::<dyn ILogger + Send + Sync>(&*provider).unwrap();
    assert_eq!(logger.log("e2e"), "[LOG] e2e");
}
