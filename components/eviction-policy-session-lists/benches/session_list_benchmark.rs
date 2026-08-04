//! Criterion benchmarks for the session-lineage eviction policy (SC-002/003/005).
//!
//! Exercises the hot paths through the `IEvictionPolicy` interface at scale:
//! `track`, `touch`, `batch_touch`, and `identify_next_to_evict`. Victim
//! selection is measured against the number of *active sessions* (leaves), which
//! is what its cost scales with, not the total number of tracked blocks.

use std::sync::Arc;

use component_core::query_interface;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use eviction_policy_session_lists::EvictionPolicySessionListsComponent;
use interfaces::{BlockSemantics, EvictionHandle, IEvictionPolicy};

fn ep() -> Arc<dyn IEvictionPolicy + Send + Sync> {
    let comp = EvictionPolicySessionListsComponent::new_default();
    query_interface!(comp, IEvictionPolicy).unwrap()
}

/// Populate a single pool with `blocks` blocks spread across `sessions`
/// chains (round-robin), returning the policy, pool id, and handles.
fn populate(
    blocks: u64,
    sessions: u64,
) -> (
    Arc<dyn IEvictionPolicy + Send + Sync>,
    u32,
    Vec<EvictionHandle>,
) {
    let ep = ep();
    let pool = ep.create_pool();
    let mut handles = Vec::with_capacity(blocks as usize);
    for k in 0..blocks {
        let session_id = k % sessions;
        let h = ep.track(pool, k, BlockSemantics { session_id }).unwrap();
        handles.push(h);
    }
    (ep, pool, handles)
}

fn bench_track(c: &mut Criterion) {
    let mut group = c.benchmark_group("track");
    for &blocks in &[10_000u64, 100_000, 1_000_000] {
        group.throughput(Throughput::Elements(blocks));
        group.bench_with_input(
            BenchmarkId::from_parameter(blocks),
            &blocks,
            |b, &blocks| {
                b.iter(|| {
                    let ep = ep();
                    let pool = ep.create_pool();
                    let sessions = blocks / 16; // ~16 blocks per session chain
                    for k in 0..blocks {
                        let session_id = k % sessions.max(1);
                        black_box(ep.track(pool, k, BlockSemantics { session_id }).unwrap());
                    }
                });
            },
        );
    }
    group.finish();
}

fn bench_touch(c: &mut Criterion) {
    let (ep, _pool, handles) = populate(1_000_000, 1_000_000 / 16);
    let mut group = c.benchmark_group("touch");
    group.throughput(Throughput::Elements(1));
    group.bench_function("1M_blocks", |b| {
        let mut i = 0usize;
        b.iter(|| {
            let h = handles[i % handles.len()];
            i += 1;
            ep.touch(black_box(h)).unwrap();
        });
    });
    group.finish();
}

fn bench_batch_touch(c: &mut Criterion) {
    let (ep, _pool, handles) = populate(1_000_000, 1_000_000 / 16);
    let mut group = c.benchmark_group("batch_touch");
    for &batch in &[8usize, 64, 512] {
        group.throughput(Throughput::Elements(batch as u64));
        group.bench_with_input(BenchmarkId::from_parameter(batch), &batch, |b, &batch| {
            let slice = &handles[..batch];
            b.iter(|| {
                ep.batch_touch(black_box(slice)).unwrap();
            });
        });
    }
    group.finish();
}

fn bench_identify_next_to_evict(c: &mut Criterion) {
    let mut group = c.benchmark_group("identify_next_to_evict");
    // Victim selection scales with active sessions (leaves), not total blocks:
    // hold total blocks fixed and vary the number of sessions.
    for &sessions in &[100u64, 10_000, 1_000_000] {
        group.bench_with_input(
            BenchmarkId::from_parameter(sessions),
            &sessions,
            |b, &sessions| {
                b.iter_batched(
                    || populate(1_000_000, sessions),
                    |(ep, pool, _h)| {
                        black_box(ep.identify_next_to_evict(pool));
                    },
                    criterion::BatchSize::LargeInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_track,
    bench_touch,
    bench_batch_touch,
    bench_identify_next_to_evict
);
criterion_main!(benches);
