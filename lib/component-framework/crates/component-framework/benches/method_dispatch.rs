use component_framework::iunknown::query;
use component_framework::{define_component, define_interface};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::sync::Arc;

define_interface! {
    pub IBenchCompute {
        fn compute(&self, x: u64) -> u64;
    }
}

define_component! {
    pub ComputeComponent {
        version: "1.0.0",
        provides: [IBenchCompute],
    }
}

impl IBenchCompute for ComputeComponent {
    fn compute(&self, x: u64) -> u64 {
        x.wrapping_mul(31)
    }
}

// Direct trait call baseline
trait DirectCompute {
    fn compute(&self, x: u64) -> u64;
}

struct DirectImpl;

impl DirectCompute for DirectImpl {
    fn compute(&self, x: u64) -> u64 {
        x.wrapping_mul(31)
    }
}

fn bench_method_dispatch(c: &mut Criterion) {
    let comp = ComputeComponent::new();
    let iface: Arc<dyn IBenchCompute + Send + Sync> =
        query::<dyn IBenchCompute + Send + Sync>(&*comp).unwrap();

    // Dispatch through queried Arc<dyn Trait>
    c.bench_function("dispatch_via_query", |b| {
        b.iter(|| iface.compute(black_box(42)))
    });

    // Direct dyn Trait dispatch (baseline)
    let direct: Box<dyn DirectCompute> = Box::new(DirectImpl);
    c.bench_function("dispatch_direct_dyn", |b| {
        b.iter(|| direct.compute(black_box(42)))
    });
}

criterion_group!(benches, bench_method_dispatch);
criterion_main!(benches);
