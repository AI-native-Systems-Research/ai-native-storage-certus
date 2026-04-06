use component_framework::binding::bind;
use component_framework::component_ref::ComponentRef;
use component_framework::error::RegistryError;
use component_framework::iunknown::query;
use component_framework::registry::ComponentRegistry;
use component_framework::{define_component, define_interface};
use std::any::Any;
use std::sync::Arc;

// --- Interfaces ---

define_interface! {
    pub IDataStore {
        fn get(&self, key: &str) -> Option<String>;
    }
}

define_interface! {
    pub IProcessor {
        fn process(&self, input: &str) -> String;
    }
}

define_interface! {
    pub IFrontend {
        fn handle_request(&self, req: &str) -> String;
    }
}

// --- Components ---

define_component! {
    pub DataStoreComp {
        version: "1.0.0",
        provides: [IDataStore],
    }
}

impl IDataStore for DataStoreComp {
    fn get(&self, key: &str) -> Option<String> {
        match key {
            "greeting" => Some("hello world".to_string()),
            _ => None,
        }
    }
}

define_component! {
    pub ProcessorComp {
        version: "2.0.0",
        provides: [IProcessor],
        receptacles: {
            store: IDataStore,
        },
    }
}

impl IProcessor for ProcessorComp {
    fn process(&self, input: &str) -> String {
        let store = self.store.get().unwrap();
        match store.get(input) {
            Some(val) => format!("processed:{val}"),
            None => format!("processed:unknown({input})"),
        }
    }
}

define_component! {
    pub FrontendComp {
        version: "3.0.0",
        provides: [IFrontend],
        receptacles: {
            processor: IProcessor,
        },
    }
}

impl IFrontend for FrontendComp {
    fn handle_request(&self, req: &str) -> String {
        let proc = self.processor.get().unwrap();
        format!("response:{}", proc.process(req))
    }
}

#[test]
fn registry_create_bind_invoke_cross_component() {
    let registry = ComponentRegistry::new();

    registry
        .register(
            "datastore",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(DataStoreComp::new()))
            },
        )
        .unwrap();

    registry
        .register(
            "processor",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(ProcessorComp::new()))
            },
        )
        .unwrap();

    let store = registry.create("datastore", None).unwrap();
    let proc_comp = registry.create("processor", None).unwrap();

    bind(&*store, "IDataStore", &*proc_comp, "store").unwrap();

    let processor: Arc<dyn IProcessor + Send + Sync> =
        query::<dyn IProcessor + Send + Sync>(&*proc_comp).unwrap();
    assert_eq!(processor.process("greeting"), "processed:hello world");
    assert_eq!(processor.process("missing"), "processed:unknown(missing)");
}

#[test]
fn multi_component_chained_wiring() {
    let registry = ComponentRegistry::new();

    registry
        .register(
            "datastore",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(DataStoreComp::new()))
            },
        )
        .unwrap();

    registry
        .register(
            "processor",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(ProcessorComp::new()))
            },
        )
        .unwrap();

    registry
        .register(
            "frontend",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(FrontendComp::new()))
            },
        )
        .unwrap();

    // Create all components
    let store = registry.create("datastore", None).unwrap();
    let proc_comp = registry.create("processor", None).unwrap();
    let frontend = registry.create("frontend", None).unwrap();

    // Wire: store -> processor -> frontend
    bind(&*store, "IDataStore", &*proc_comp, "store").unwrap();
    bind(&*proc_comp, "IProcessor", &*frontend, "processor").unwrap();

    // Invoke through the frontend
    let fe: Arc<dyn IFrontend + Send + Sync> =
        query::<dyn IFrontend + Send + Sync>(&*frontend).unwrap();
    assert_eq!(
        fe.handle_request("greeting"),
        "response:processed:hello world"
    );
}

#[test]
fn release_all_component_refs_clean_destruction() {
    let registry = ComponentRegistry::new();

    registry
        .register(
            "datastore",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(DataStoreComp::new()))
            },
        )
        .unwrap();

    registry
        .register(
            "processor",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(ProcessorComp::new()))
            },
        )
        .unwrap();

    let store = registry.create("datastore", None).unwrap();
    let proc_comp = registry.create("processor", None).unwrap();

    bind(&*store, "IDataStore", &*proc_comp, "store").unwrap();

    // Ref counts > 1 because the component's internal interface map
    // stores Arc clones pointing to the component itself.
    assert!(store.ref_count() >= 1);
    assert!(proc_comp.ref_count() >= 1);

    // Query keeps alive via Arc
    let processor: Arc<dyn IProcessor + Send + Sync> =
        query::<dyn IProcessor + Send + Sync>(&*proc_comp).unwrap();
    assert_eq!(processor.process("greeting"), "processed:hello world");

    // Drop everything — should not panic
    drop(processor);
    drop(proc_comp);
    drop(store);
}
