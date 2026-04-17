use criterion::{criterion_group, criterion_main, Criterion};
use extent_manager::test_support::create_test_component;
use interfaces::{IExtentManager, IExtentManagerAdmin};

fn bench_extent_count(c: &mut Criterion) {
    let (component, _mock) = create_test_component();
    component
        .initialize(100 * 128 * 4096, 128 * 4096, 1)
        .expect("initialize");

    for i in 0..1_000u64 {
        component
            .create_extent(i, 131072, "", 0, false)
            .expect("create");
    }

    c.bench_function("extent_count", |b| {
        b.iter(|| {
            let _ = component.extent_count();
        });
    });
}

criterion_group!(benches, bench_extent_count);
criterion_main!(benches);
