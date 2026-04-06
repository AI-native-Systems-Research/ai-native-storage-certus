use component_framework::component_ref::ComponentRef;
use component_framework::error::RegistryError;
use component_framework::iunknown::query;
use component_framework::registry::ComponentRegistry;
use component_framework::{define_component, define_interface};
use std::any::Any;
use std::sync::Arc;

define_interface! {
    pub ICounter {
        fn count(&self) -> u32;
    }
}

define_component! {
    pub CounterComponent {
        version: "1.0.0",
        provides: [ICounter],
        fields: {
            value: u32,
        },
    }
}

impl ICounter for CounterComponent {
    fn count(&self) -> u32 {
        self.value
    }
}

fn counter_factory(config: Option<&dyn Any>) -> Result<ComponentRef, RegistryError> {
    let val = config
        .and_then(|c| c.downcast_ref::<u32>())
        .copied()
        .unwrap_or(0);
    Ok(ComponentRef::from(CounterComponent::new(val)))
}

#[test]
fn register_create_query_interface_end_to_end() {
    let registry = ComponentRegistry::new();
    registry.register("counter", counter_factory).unwrap();

    let comp = registry.create("counter", Some(&42u32)).unwrap();
    assert_eq!(comp.version(), "1.0.0");

    let counter: Arc<dyn ICounter + Send + Sync> =
        query::<dyn ICounter + Send + Sync>(&*comp).unwrap();
    assert_eq!(counter.count(), 42);
}

#[test]
fn concurrent_registry_access() {
    let registry = Arc::new(ComponentRegistry::new());
    registry.register("counter", counter_factory).unwrap();

    let handles: Vec<_> = (0..10)
        .map(|i| {
            let reg = Arc::clone(&registry);
            std::thread::spawn(move || {
                let comp = reg.create("counter", Some(&(i as u32))).unwrap();
                let counter: Arc<dyn ICounter + Send + Sync> =
                    query::<dyn ICounter + Send + Sync>(&*comp).unwrap();
                assert_eq!(counter.count(), i as u32);
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn factory_panic_caught_as_factory_failed() {
    let registry = ComponentRegistry::new();
    registry
        .register(
            "panicker",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                panic!("deliberate test panic");
            },
        )
        .unwrap();

    let err = registry.create("panicker", None).unwrap_err();
    match err {
        RegistryError::FactoryFailed { name, source } => {
            assert_eq!(name, "panicker");
            assert!(source.contains("panic"));
        }
        other => panic!("expected FactoryFailed, got {other}"),
    }
}

#[test]
fn register_simple_creates_component() {
    let registry = ComponentRegistry::new();
    registry
        .register_simple("simple-counter", || {
            ComponentRef::from(CounterComponent::new(99))
        })
        .unwrap();

    let comp = registry.create("simple-counter", None).unwrap();
    let counter: Arc<dyn ICounter + Send + Sync> =
        query::<dyn ICounter + Send + Sync>(&*comp).unwrap();
    assert_eq!(counter.count(), 99);
}
