use component_framework::component_ref::ComponentRef;
use component_framework::error::RegistryError;
use component_framework::iunknown::{query, IUnknown};
use component_framework::registry::ComponentRegistry;
use component_framework::{define_component, define_interface};
use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

define_interface! {
    pub IValue {
        fn value(&self) -> u32;
    }
}

define_component! {
    pub ValueComponent {
        version: "1.0.0",
        provides: [IValue],
        fields: {
            val: u32,
        },
    }
}

impl IValue for ValueComponent {
    fn value(&self) -> u32 {
        self.val
    }
}

#[test]
fn attach_increments_release_decrements_destroy_at_zero() {
    static DESTROYED: AtomicBool = AtomicBool::new(false);

    define_interface! {
        pub ITrack {
            fn alive(&self) -> bool;
        }
    }

    // Use a wrapper to track drop
    struct Dropper;
    impl Drop for Dropper {
        fn drop(&mut self) {
            DESTROYED.store(true, Ordering::SeqCst);
        }
    }

    // We can't easily use define_component! with a Drop impl, so test
    // with ComponentRef directly using a manual IUnknown impl.
    use component_framework::interface::{InterfaceInfo, ReceptacleInfo};
    use std::any::TypeId;

    struct TrackedComp {
        _dropper: Dropper,
    }
    impl IUnknown for TrackedComp {
        fn query_interface_raw(&self, _id: TypeId) -> Option<&(dyn Any + Send + Sync)> {
            None
        }
        fn version(&self) -> &str {
            "1.0.0"
        }
        fn provided_interfaces(&self) -> &[InterfaceInfo] {
            &[]
        }
        fn receptacles(&self) -> &[ReceptacleInfo] {
            &[]
        }
        fn connect_receptacle_raw(&self, _: &str, _: &dyn IUnknown) -> Result<(), RegistryError> {
            Err(RegistryError::BindingFailed {
                detail: "none".into(),
            })
        }
    }

    DESTROYED.store(false, Ordering::SeqCst);

    let comp = ComponentRef::new(Arc::new(TrackedComp { _dropper: Dropper }) as Arc<dyn IUnknown>);
    assert_eq!(comp.ref_count(), 1);

    let c2 = comp.attach();
    assert_eq!(comp.ref_count(), 2);

    let c3 = c2.attach();
    assert_eq!(comp.ref_count(), 3);

    drop(c2);
    assert_eq!(comp.ref_count(), 2);
    assert!(!DESTROYED.load(Ordering::SeqCst));

    drop(comp);
    assert_eq!(c3.ref_count(), 1);
    assert!(!DESTROYED.load(Ordering::SeqCst));

    drop(c3);
    assert!(DESTROYED.load(Ordering::SeqCst));
}

#[test]
fn concurrent_attach_release() {
    let comp = ComponentRef::from(ValueComponent::new(99));

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let c = comp.clone();
            std::thread::spawn(move || {
                let c2 = c.attach();
                assert_eq!(c2.version(), "1.0.0");
                // c and c2 both drop here
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
    // Only the original comp remains (plus internal self-referential Arcs
    // from the interface map: one per provided interface + IUnknown).
    let base_count = comp.ref_count();
    let c2 = comp.attach();
    assert_eq!(c2.ref_count(), base_count + 1);
    drop(c2);
    assert_eq!(comp.ref_count(), base_count);
}

#[test]
fn component_ref_from_registry_has_stable_base_count() {
    let registry = ComponentRegistry::new();
    registry
        .register(
            "val",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(ValueComponent::new(7)))
            },
        )
        .unwrap();

    let comp = registry.create("val", None).unwrap();
    // Base count includes internal self-referential Arcs from interface map.
    // Attaching should increment by exactly 1.
    let base = comp.ref_count();
    let c2 = comp.attach();
    assert_eq!(c2.ref_count(), base + 1);
}

#[test]
fn receptacle_keeps_component_alive() {
    define_interface! {
        pub IProvider {
            fn provide(&self) -> &str;
        }
    }

    define_component! {
        pub ProvComp {
            version: "1.0.0",
            provides: [IProvider],
        }
    }

    impl IProvider for ProvComp {
        fn provide(&self) -> &str {
            "provided"
        }
    }

    define_component! {
        pub ConsComp {
            version: "1.0.0",
            provides: [],
            receptacles: {
                source: IProvider,
            },
        }
    }

    let provider = ProvComp::new();
    let consumer = ConsComp::new();

    // First-party wiring
    let prov_iface: Arc<dyn IProvider + Send + Sync> =
        query::<dyn IProvider + Send + Sync>(&*provider).unwrap();
    consumer.source.connect(prov_iface).unwrap();

    // Drop the ComponentRef-equivalent (our Arc<ProvComp>), but the
    // receptacle still holds an Arc to the interface.
    let provider_ref_count = Arc::strong_count(&provider);
    drop(provider);
    // The provider Arc was dropped, but the interface Arc inside the
    // receptacle keeps the provider alive indirectly (separate Arc).
    let iface = consumer.source.get().unwrap();
    assert_eq!(iface.provide(), "provided");
    let _ = provider_ref_count; // use value to avoid warning
}
