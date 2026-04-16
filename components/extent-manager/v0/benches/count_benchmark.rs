use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use extent_manager::test_support::{create_test_component, MockBlockDevice};
use interfaces::IExtentManager;

fn extent_count_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("extent_count");

    for &population in &[0, 100, 1000, 10_000] {
        let (comp, _mock): (
            Arc<extent_manager::ExtentManagerComponentV1>,
            Arc<MockBlockDevice>,
        ) = create_test_component(1_000_000, &[131072], &[20_000]);

        for key in 1..=population {
            comp.create_extent(key, 0, "", 0, false).unwrap();
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(population),
            &population,
            |b, _| {
                b.iter(|| {
                    std::hint::black_box(comp.extent_count());
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, extent_count_benchmark);
criterion_main!(benches);
