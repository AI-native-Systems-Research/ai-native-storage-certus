use component_core::component_ref::ComponentRef;
use component_core::iunknown::{query, IUnknown};
use component_core::query_interface;
use component_macros::{define_component, define_interface};
use std::sync::Arc;

define_interface! {
    pub IStorage {
        fn read(&self, key: &str) -> Option<Vec<u8>>;
        fn write(&self, key: &str, value: &[u8]) -> Result<(), String>;
    }
}

define_interface! {
    pub ILogger {
        fn log(&self, level: u8, message: &str);
        fn flush(&self);
    }
}

define_interface! {
    pub INetwork {
        fn send(&self, data: &[u8]) -> Result<usize, String>;
    }
}

define_component! {
    pub StorageComponent {
        version: "1.2.0",
        provides: [IStorage],
    }
}

impl IStorage for StorageComponent {
    fn read(&self, _key: &str) -> Option<Vec<u8>> {
        Some(vec![1, 2, 3])
    }
    fn write(&self, _key: &str, _value: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

// T027: query_interface returns valid Arc for provided interface
#[test]
fn query_interface_returns_arc_for_provided_interface() {
    let comp = StorageComponent::new();
    let storage: Arc<dyn IStorage + Send + Sync> =
        query::<dyn IStorage + Send + Sync>(&*comp).unwrap();
    assert_eq!(storage.read("x"), Some(vec![1, 2, 3]));
}

// T028: query_interface returns None for unsupported interface
#[test]
fn query_interface_returns_none_for_unsupported() {
    let comp = StorageComponent::new();
    let result = query::<dyn INetwork + Send + Sync>(&*comp);
    assert!(result.is_none());
}

// T029: version() returns declared version string
#[test]
fn version_returns_declared_string() {
    let comp = StorageComponent::new();
    assert_eq!(comp.version(), "1.2.0");
}

// T030: query() typed free function works through dyn IUnknown
#[test]
fn query_through_dyn_iunknown() {
    let comp = StorageComponent::new();
    let iunknown: &dyn IUnknown = &*comp;
    let storage: Arc<dyn IStorage + Send + Sync> =
        query::<dyn IStorage + Send + Sync>(iunknown).unwrap();
    assert!(storage.write("k", &[1]).is_ok());
}

// T048/T050: provided_interfaces returns complete list including IUnknown
#[test]
fn provided_interfaces_includes_iunknown() {
    let comp = StorageComponent::new();
    let infos = comp.provided_interfaces();
    let names: Vec<&str> = infos.iter().map(|i| i.name).collect();
    assert!(names.contains(&"IStorage"), "missing IStorage: {names:?}");
    assert!(names.contains(&"IUnknown"), "missing IUnknown: {names:?}");
}

// Query for IUnknown itself
#[test]
fn query_iunknown_returns_self() {
    let comp = StorageComponent::new();
    let unk: Arc<dyn IUnknown> = query::<dyn IUnknown>(&*comp).unwrap();
    assert_eq!(unk.version(), "1.2.0");
}

// Component with multiple interfaces
define_component! {
    pub MultiComponent {
        version: "2.0.0",
        provides: [IStorage, ILogger],
    }
}

impl IStorage for MultiComponent {
    fn read(&self, _key: &str) -> Option<Vec<u8>> {
        None
    }
    fn write(&self, _key: &str, _value: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

impl ILogger for MultiComponent {
    fn log(&self, _level: u8, _message: &str) {}
    fn flush(&self) {}
}

#[test]
fn multi_interface_component_queries() {
    let comp = MultiComponent::new();
    assert!(query::<dyn IStorage + Send + Sync>(&*comp).is_some());
    assert!(query::<dyn ILogger + Send + Sync>(&*comp).is_some());
    assert!(query::<dyn INetwork + Send + Sync>(&*comp).is_none());
}

#[test]
fn multi_interface_provided_list() {
    let comp = MultiComponent::new();
    let names: Vec<&str> = comp.provided_interfaces().iter().map(|i| i.name).collect();
    assert_eq!(names.len(), 3); // IStorage, ILogger, IUnknown
    assert!(names.contains(&"IStorage"));
    assert!(names.contains(&"ILogger"));
    assert!(names.contains(&"IUnknown"));
}

// --- query_interface! macro integration tests (FR-011) ---

#[test]
fn query_interface_macro_with_direct_ref() {
    let comp = StorageComponent::new();
    let storage: Arc<dyn IStorage + Send + Sync> = query_interface!(&*comp, IStorage).unwrap();
    assert_eq!(storage.read("x"), Some(vec![1, 2, 3]));
}

#[test]
fn query_interface_macro_with_arc() {
    let comp: Arc<StorageComponent> = StorageComponent::new();
    let storage: Arc<dyn IStorage + Send + Sync> = query_interface!(comp, IStorage).unwrap();
    assert_eq!(storage.read("x"), Some(vec![1, 2, 3]));
}

#[test]
fn query_interface_macro_with_component_ref() {
    let comp = StorageComponent::new();
    let comp_ref = ComponentRef::from(comp);
    let storage: Arc<dyn IStorage + Send + Sync> = query_interface!(comp_ref, IStorage).unwrap();
    assert_eq!(storage.read("x"), Some(vec![1, 2, 3]));
}

#[test]
fn query_interface_macro_returns_none_for_unsupported() {
    let comp = StorageComponent::new();
    let result = query_interface!(&*comp, INetwork);
    assert!(result.is_none());
}

#[test]
fn query_interface_macro_with_multi_interface() {
    let comp = MultiComponent::new();
    let storage: Arc<dyn IStorage + Send + Sync> = query_interface!(&*comp, IStorage).unwrap();
    assert!(storage.read("x").is_none());

    let logger: Arc<dyn ILogger + Send + Sync> = query_interface!(&*comp, ILogger).unwrap();
    logger.log(1, "test");
}

// --- new_default() integration test (FR-013) ---

define_component! {
    pub DefaultableComponent {
        version: "1.0.0",
        provides: [IStorage],
        fields: {
            data: Vec<u8>,
            count: u32,
        },
    }
}

impl IStorage for DefaultableComponent {
    fn read(&self, _key: &str) -> Option<Vec<u8>> {
        Some(self.data.clone())
    }
    fn write(&self, _key: &str, _value: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn new_default_creates_component_with_default_fields() {
    let comp = DefaultableComponent::new_default();
    assert_eq!(comp.version(), "1.0.0");
    assert_eq!(comp.count, 0);
    assert!(comp.data.is_empty());

    let storage: Arc<dyn IStorage + Send + Sync> = query_interface!(comp, IStorage).unwrap();
    assert_eq!(storage.read("x"), Some(vec![]));
}

#[test]
fn new_default_component_has_same_behavior_as_new() {
    let from_new = DefaultableComponent::new(Vec::new(), 0);
    let from_default = DefaultableComponent::new_default();

    assert_eq!(from_new.version(), from_default.version());
    assert_eq!(from_new.count, from_default.count);
    assert_eq!(from_new.data, from_default.data);
}
