//! The per-entry push path (T057, FR-008): `ITransferEndpoint::push` of N
//! memory-tier entries, measured as a caller sees it — lookup, pin, post, reap
//! and release for each entry, plus the command round trip to the endpoint
//! thread.
//!
//! ```text
//! cargo bench -p certus-transfer-endpoint --bench entry_push              # mock
//! CERTUS_RDMA_TEST_IP=10.0.0.102 \
//!     cargo bench -p certus-transfer-endpoint --features rdma --bench entry_push
//! ```
//!
//! Over the mock the number is the endpoint's own per-entry cost, and the
//! regression guard; over verbs it is the real cost, at 64 KiB and 2.25 MiB
//! entries (research D19 predicts ~90 µs a 2.25 MiB write at 200G). Two
//! endpoints in one process, on two ports of one address (D16).

mod common;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use interfaces::{EntryOutcome, ITransferEndpoint};

fn push(c: &mut Criterion) {
    let Some(host) = common::host() else { return };
    let mut g = c.benchmark_group(format!("entry_push/{}", common::transport()));
    // Entry sizes: 64 KiB (the measured deployment's block) everywhere; 2.25
    // MiB (Qwen3-8B, 16 tokens) where the bytes mean something — over verbs.
    let sizes: &[usize] = if cfg!(feature = "rdma") {
        &[64 << 10, 2304 << 10]
    } else {
        &[64 << 10]
    };
    for &size in sizes {
        // 340 x 2.25 MiB is 765 MiB a side; the large size stops at 64.
        let counts: &[usize] = if size > (1 << 20) {
            &[1, 64]
        } else {
            &[1, 64, 340]
        };
        for &n in counts {
            let src = common::Node::new(&host, n * size);
            src.hold(n, size);
            let dst = common::Node::new(&host, n * size);
            let slots = common::Slots::new(&dst, n, size);
            let items = slots.regions(n, size);
            // Connect and warm up outside the measurement.
            assert_eq!(
                src.ep.push(&dst.addr, &items).unwrap(),
                vec![EntryOutcome::HitMemoryTier; n]
            );
            g.throughput(Throughput::Bytes((n * size) as u64));
            g.bench_with_input(
                BenchmarkId::new(format!("{}KiB", size >> 10), n),
                &n,
                |b, _| {
                    b.iter(|| {
                        let out = src.ep.push(&dst.addr, &items).unwrap();
                        debug_assert!(out.iter().all(|o| *o == EntryOutcome::HitMemoryTier));
                        out
                    })
                },
            );
            assert_eq!(src.map.held(), 0, "every pin released");
            drop((src, dst, slots));
        }
    }
    g.finish();
}

criterion_group!(benches, push);
criterion_main!(benches);
