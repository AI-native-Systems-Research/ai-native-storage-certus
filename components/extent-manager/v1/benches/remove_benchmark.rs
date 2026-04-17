use criterion::{criterion_group, criterion_main, Criterion};
use extent_manager::test_support::create_test_component;
use interfaces::{IExtentManager, IExtentManagerAdmin};

fn bench_remove_extent(c: &mut Criterion) {
    c.bench_function("remove_extent", |b| {
        let (component, _mock) = create_test_component();
        component
            .initialize(100 * 128 * 4096, 128 * 4096, 1)
            .expect("initialize");

        let mut key = 0u64;
        b.iter_custom(|iters| {
            for i in 0..iters {
                let k = key + i;
                component
                    .create_extent(k, 131072, "", 0, false)
                    .expect("create");
            }
            let start = std::time::Instant::now();
            for i in 0..iters {
                let k = key + i;
                component.remove_extent(k).expect("remove");
            }
            key += iters;
            start.elapsed()
        });
    });
}

criterion_group!(benches, bench_remove_extent);
criterion_main!(benches);
