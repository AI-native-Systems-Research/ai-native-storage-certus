use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use extent_manager::test_support::{create_test_component, MockBlockDevice};
use interfaces::IExtentManager;

fn lookup_extent_benchmark(c: &mut Criterion) {
    let (comp, _mock): (
        Arc<extent_manager::ExtentManagerComponentV1>,
        Arc<MockBlockDevice>,
    ) = create_test_component(1_000_000, &[131072], &[10_000]);

    // Pre-populate 1000 extents.
    for key in 1..=1000 {
        comp.create_extent(key, 0, "", 0, false).unwrap();
    }

    let mut idx = 0u64;

    c.bench_function("lookup_extent", |b| {
        b.iter(|| {
            idx = (idx % 1000) + 1;
            comp.lookup_extent(idx).unwrap();
        });
    });
}

criterion_group!(benches, lookup_extent_benchmark);
criterion_main!(benches);
