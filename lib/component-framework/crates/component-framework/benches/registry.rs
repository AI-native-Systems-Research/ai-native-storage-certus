use criterion::{black_box, criterion_group, criterion_main, Criterion};

use component_framework::component_ref::ComponentRef;
use component_framework::error::RegistryError;
use component_framework::registry::ComponentRegistry;
use component_framework::{define_component, define_interface};
use std::any::Any;

define_interface! {
    pub IBench {
        fn op(&self) -> u32;
    }
}

define_component! {
    pub BenchComp {
        version: "1.0.0",
        provides: [IBench],
    }
}

impl IBench for BenchComp {
    fn op(&self) -> u32 {
        42
    }
}

fn bench_registry_create(c: &mut Criterion) {
    let registry = ComponentRegistry::new();
    registry
        .register(
            "bench",
            |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                Ok(ComponentRef::from(BenchComp::new()))
            },
        )
        .unwrap();

    c.bench_function("registry_create", |b| {
        b.iter(|| {
            let comp = registry.create(black_box("bench"), None).unwrap();
            black_box(comp);
        });
    });
}

fn bench_registry_register_unregister(c: &mut Criterion) {
    c.bench_function("registry_register_unregister", |b| {
        let registry = ComponentRegistry::new();
        let mut i = 0u64;
        b.iter(|| {
            let name = format!("comp_{i}");
            i += 1;
            registry
                .register(
                    &name,
                    |_: Option<&dyn Any>| -> Result<ComponentRef, RegistryError> {
                        Ok(ComponentRef::from(BenchComp::new()))
                    },
                )
                .unwrap();
            registry.unregister(&name).unwrap();
        });
    });
}

criterion_group!(
    benches,
    bench_registry_create,
    bench_registry_register_unregister
);
criterion_main!(benches);
