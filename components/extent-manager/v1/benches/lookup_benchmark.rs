use criterion::{criterion_group, criterion_main, Criterion};
use extent_manager::test_support::create_test_component;
use interfaces::{IExtentManager, IExtentManagerAdmin};

fn bench_lookup_extent(c: &mut Criterion) {
    let (component, _mock) = create_test_component();
    component
        .initialize(100 * 128 * 4096, 128 * 4096, 1)
        .expect("initialize");

    for i in 0..10_000u64 {
        component
            .create_extent(i, 131072, "", 0, false)
            .expect("create");
    }

    c.bench_function("lookup_extent", |b| {
        let mut key = 0u64;
        b.iter(|| {
            let k = key % 10_000;
            let _ = component.lookup_extent(k);
            key += 1;
        });
    });
}

criterion_group!(benches, bench_lookup_extent);
criterion_main!(benches);
