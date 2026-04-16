use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use extent_manager::test_support::{create_test_component, MockBlockDevice};
use interfaces::IExtentManager;

fn remove_extent_benchmark(c: &mut Criterion) {
    let (comp, _mock): (
        Arc<extent_manager::ExtentManagerComponentV1>,
        Arc<MockBlockDevice>,
    ) = create_test_component(1_000_000, &[131072], &[200_000]);

    // Pre-populate a large number of extents.
    let mut next_key = 0u64;
    for _ in 0..100_000 {
        next_key += 1;
        comp.create_extent(next_key, 0, "", 0, false).unwrap();
    }

    let mut remove_key = 1u64;

    c.bench_function("remove_extent", |b| {
        b.iter(|| {
            if comp.lookup_extent(remove_key).is_err() {
                // Re-create if already removed.
                next_key += 1;
                comp.create_extent(next_key, 0, "", 0, false).unwrap();
                remove_key = next_key;
            }
            comp.remove_extent(remove_key).unwrap();
            remove_key += 1;
        });
    });
}

criterion_group!(benches, remove_extent_benchmark);
criterion_main!(benches);
