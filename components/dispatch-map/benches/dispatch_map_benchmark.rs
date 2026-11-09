use std::sync::Arc;

use component_core::query_interface;
use criterion::{black_box, criterion_group, criterion_main, measurement::WallTime, Criterion};
use dispatch_map::{DispatchMapComponent, DispatchMapState};
use interfaces::{IDispatchMap, IEvictionPolicy};

fn setup_bench_component() -> Arc<DispatchMapComponent> {
    let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
        query_interface!(ep_comp, IEvictionPolicy).unwrap();
    let comp = DispatchMapComponent::new(DispatchMapState::new());
    comp.eviction_policy.connect(ep).unwrap();
    comp
}

/// Helper: create a memory-tier entry for benchmarking.
fn create_entry(dm: &Arc<dyn IDispatchMap + Send + Sync>, key: u64) {
    let ptr = Box::into_raw(vec![0u8; 4096].into_boxed_slice()) as *mut u8;
    dm.create_memory_tier_entry(key, ptr, 4096).unwrap();
}

fn bench_lookup_no_contention(c: &mut Criterion) {
    let comp = setup_bench_component();
    let dm = query_interface!(comp, IDispatchMap).unwrap();

    create_entry(&dm, 1);
    dm.release_write(1).unwrap();

    c.bench_function("lookup_no_contention", |b| {
        b.iter(|| {
            let result = dm.lookup(black_box(1)).unwrap();
            dm.release_read(black_box(1)).unwrap();
            black_box(result);
        });
    });
}

fn bench_ref_ops_throughput(c: &mut Criterion) {
    let comp = setup_bench_component();
    let dm = query_interface!(comp, IDispatchMap).unwrap();

    create_entry(&dm, 1);
    dm.release_write(1).unwrap();

    c.bench_function("take_release_read", |b| {
        b.iter(|| {
            dm.take_read(black_box(1)).unwrap();
            dm.release_read(black_box(1)).unwrap();
        });
    });

    c.bench_function("take_release_write", |b| {
        b.iter(|| {
            dm.take_write(black_box(1)).unwrap();
            dm.release_write(black_box(1)).unwrap();
        });
    });
}

fn bench_lru_lookup(c: &mut Criterion<WallTime>) {
    let comp = setup_bench_component();
    let dm = query_interface!(comp, IDispatchMap).unwrap();

    // Populate the map with 1000 entries.
    for key in 0..1000u64 {
        create_entry(&dm, key);
        dm.release_write(key).unwrap();
    }

    let mut group = c.benchmark_group("lru_mean_latency");
    group.significance_level(0.01).sample_size(200);

    group.bench_function("oldest_keys_1000_entries_top10", |b| {
        b.iter(|| {
            let keys = dm.oldest_keys(black_box(10));
            black_box(keys);
        });
    });

    group.bench_function("oldest_keys_1000_entries_top100", |b| {
        b.iter(|| {
            let keys = dm.oldest_keys(black_box(100));
            black_box(keys);
        });
    });

    group.finish();
}

fn bench_entry_size(c: &mut Criterion) {
    use dispatch_map::entry_size;

    c.bench_function("entry_size_check", |b| {
        b.iter(|| {
            let size = entry_size();
            assert!(size <= 56, "DispatchEntry is {size} bytes, expected ≤ 56");
            black_box(size);
        });
    });
}

criterion_group!(
    benches,
    bench_lookup_no_contention,
    bench_ref_ops_throughput,
    bench_lru_lookup,
    bench_entry_size
);
criterion_main!(benches);
