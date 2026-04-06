use component_framework::iunknown::query;
use component_framework::{define_component, define_interface};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::sync::Arc;

define_interface! {
    pub IBenchStorage {
        fn read(&self, key: &str) -> Option<Vec<u8>>;
    }
}

define_component! {
    pub BenchComponent {
        version: "1.0.0",
        provides: [IBenchStorage],
    }
}

impl IBenchStorage for BenchComponent {
    fn read(&self, _key: &str) -> Option<Vec<u8>> {
        Some(vec![1, 2, 3])
    }
}

fn bench_query_interface(c: &mut Criterion) {
    let comp = BenchComponent::new();

    c.bench_function("query_interface_hit", |b| {
        b.iter(|| {
            let _: Arc<dyn IBenchStorage + Send + Sync> =
                query::<dyn IBenchStorage + Send + Sync>(black_box(&*comp)).unwrap();
        })
    });

    c.bench_function("query_interface_miss", |b| {
        b.iter(|| {
            let result = query::<dyn std::fmt::Debug + Send + Sync>(black_box(&*comp));
            assert!(result.is_none());
        })
    });
}

criterion_group!(benches, bench_query_interface);
criterion_main!(benches);
