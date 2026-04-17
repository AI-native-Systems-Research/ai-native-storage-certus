use criterion::{criterion_group, criterion_main, Criterion};
use extent_manager::test_support::create_test_component;
use interfaces::{IExtentManager, IExtentManagerAdmin};

fn bench_create_extent(c: &mut Criterion) {
    c.bench_function("create_extent", |b| {
        let (component, _mock) = create_test_component();
        component
            .initialize(100 * 128 * 4096, 128 * 4096, 1)
            .expect("initialize");

        let mut key = 0u64;
        b.iter(|| {
            key += 1;
            let _ = component.create_extent(key, 131072, "", 0, false);
        });
    });
}

criterion_group!(benches, bench_create_extent);
criterion_main!(benches);
