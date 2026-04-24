use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};

use interfaces::{FormatParams, IExtentManager};

use extent_manager_v2::test_support::create_test_component;

const DISK_SIZE: u64 = 1024 * 1024 * 1024; // 1 GiB
const SECTOR_SIZE: u32 = 4096;
const SLAB_SIZE: u32 = 1024 * 1024;
const MAX_ELEMENT_SIZE: u32 = 65536;
const METADATA_BLOCK_SIZE: u32 = 131072;

fn format_params() -> FormatParams {
    FormatParams {
        slab_size: SLAB_SIZE,
        max_element_size: MAX_ELEMENT_SIZE,
        metadata_block_size: METADATA_BLOCK_SIZE,
        sector_size: SECTOR_SIZE,
        region_count: 32,
    }
}

fn bench_reserve_publish(c: &mut Criterion) {
    let (component, _mock) = create_test_component(DISK_SIZE);
    component.format(format_params()).expect("format");

    let mut key = 0u64;
    c.bench_function("reserve_publish", |b| {
        b.iter(|| {
            key += 1;
            let h = component.reserve_extent(key, 4096).expect("reserve");
            h.publish().expect("publish");
        });
    });
}

fn bench_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("lookup");

    for &count in &[1, 1_000, 100_000] {
        let (component, _mock) = create_test_component(DISK_SIZE);
        component.format(format_params()).expect("format");

        for k in 1..=count {
            let h = component.reserve_extent(k, 4096).expect("reserve");
            h.publish().expect("publish");
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |b, _| {
                let mut key = 1u64;
                b.iter(|| {
                    component.lookup_extent(key).expect("lookup");
                    key = key % count + 1;
                });
            },
        );
    }
    group.finish();
}

fn bench_remove(c: &mut Criterion) {
    c.bench_function("remove", |b| {
        b.iter_custom(|iters| {
            let (component, _mock) = create_test_component(DISK_SIZE);
            component.format(format_params()).expect("format");

            for k in 1..=iters {
                let h = component.reserve_extent(k, 4096).expect("reserve");
                h.publish().expect("publish");
            }

            let start = std::time::Instant::now();
            for k in 1..=iters {
                component.remove_extent(k).expect("remove");
            }
            start.elapsed()
        });
    });
}

fn bench_checkpoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("checkpoint");

    for &count in &[100, 10_000] {
        let (component, _mock) = create_test_component(DISK_SIZE);
        component.format(format_params()).expect("format");

        for k in 1..=count {
            let h = component.reserve_extent(k, 4096).expect("reserve");
            h.publish().expect("publish");
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            &count,
            |b, _| {
                b.iter(|| {
                    // Mark dirty before each checkpoint
                    {
                        let h = component.reserve_extent(count + 1, 4096).unwrap();
                        h.publish().unwrap();
                    }
                    component.checkpoint().expect("checkpoint");
                    component.remove_extent(count + 1).unwrap();
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_reserve_publish,
    bench_lookup,
    bench_remove,
    bench_checkpoint,
);
criterion_main!(benches);
