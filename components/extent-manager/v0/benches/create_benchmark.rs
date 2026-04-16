use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use extent_manager::test_support::{create_test_component, MockBlockDevice};
use interfaces::IExtentManager;

fn create_extent_benchmark(c: &mut Criterion) {
    let (comp, _mock): (
        Arc<extent_manager::ExtentManagerComponentV1>,
        Arc<MockBlockDevice>,
    ) = create_test_component(1_000_000, &[131072], &[100_000]);

    let mut key = 0u64;

    c.bench_function("create_extent", |b| {
        b.iter(|| {
            key += 1;
            comp.create_extent(key, 0, "", 0, false).unwrap();
        });
    });
}

criterion_group!(benches, create_extent_benchmark);
criterion_main!(benches);
