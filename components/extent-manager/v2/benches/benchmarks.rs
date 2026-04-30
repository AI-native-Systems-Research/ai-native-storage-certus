use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};

use interfaces::{FormatParams, IExtentManager};

use extent_manager_v2::test_support::create_test_component;

const DISK_SIZE: u64 = 1024 * 1024 * 1024; // 1 GiB
const METADATA_DISK_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB
const SECTOR_SIZE: u32 = 4096;
const SLAB_SIZE: u64 = 1024 * 1024;
const MAX_EXTENT_SIZE: u32 = 65536;
const METADATA_ALIGNMENT: u64 = 1048576;

fn format_params() -> FormatParams {
    FormatParams {
        data_disk_size: DISK_SIZE,
        slab_size: SLAB_SIZE,
        max_extent_size: MAX_EXTENT_SIZE,
        sector_size: SECTOR_SIZE,
        region_count: 32,
        metadata_alignment: METADATA_ALIGNMENT,
        instance_id: None,
        metadata_disk_ns_id: 1,
    }
}

fn bench_reserve_publish(c: &mut Criterion) {
    c.bench_function("reserve_publish", |b| {
        b.iter_custom(|iters| {
            let (component, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
            component.format(format_params()).expect("format");
            let start = std::time::Instant::now();
            for key in 1..=iters {
                let h = component.reserve_extent(key, 4096).expect("reserve");
                h.publish().expect("publish");
            }
            start.elapsed()
        });
    });
}

fn bench_enumerate(c: &mut Criterion) {
    let mut group = c.benchmark_group("enumerate");

    for &count in &[1u64, 1_000, 100_000] {
        let (component, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
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
                    let mut n = 0usize;
                    component.for_each_extent(&mut |_| { n += 1; });
                    n
                });
            },
        );
    }
    group.finish();
}

fn bench_remove(c: &mut Criterion) {
    c.bench_function("remove", |b| {
        b.iter_custom(|iters| {
            let (component, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
            component.format(format_params()).expect("format");

            let offsets: Vec<u64> = (1..=iters)
                .map(|k| {
                    let h = component.reserve_extent(k, 4096).expect("reserve");
                    h.publish().expect("publish").offset
                })
                .collect();

            let start = std::time::Instant::now();
            for offset in offsets {
                component.remove_extent(offset).expect("remove");
            }
            start.elapsed()
        });
    });
}

fn bench_checkpoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("checkpoint");

    for &count in &[100u64, 10_000] {
        let (component, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
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
                    let ext = component.reserve_extent(count + 1, 4096).unwrap()
                        .publish().unwrap();
                    component.checkpoint().expect("checkpoint");
                    component.remove_extent(ext.offset).unwrap();
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_reserve_publish,
    bench_enumerate,
    bench_remove,
    bench_checkpoint,
);
criterion_main!(benches);
