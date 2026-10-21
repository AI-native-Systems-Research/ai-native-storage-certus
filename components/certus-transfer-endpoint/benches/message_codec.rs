//! The message path (T058): `ITransferEndpoint::fetch` of 340 entries — one
//! full `OP_FETCH` — as a caller sees it.
//!
//! The codec is measured inside the real path, not alone. An **all-miss** fetch
//! writes nothing, so what remains is encoding the request, the source decoding
//! and resolving it, encoding the reply and the destination decoding it, with
//! the credits and thread hops between. An **all-hit** fetch of the same shape
//! adds the 340 writes, so the difference attributes them. A chunked fetch of
//! 1,536 entries (five messages) shows the per-message cost at the prefix
//! length chunking exists for.
//!
//! ```text
//! cargo bench -p certus-transfer-endpoint --bench message_codec              # mock
//! CERTUS_RDMA_TEST_IP=10.0.0.102 \
//!     cargo bench -p certus-transfer-endpoint --features rdma --bench message_codec
//! ```

mod common;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use interfaces::EntryOutcome;

const ENTRY: usize = 64 << 10;

fn fetch(c: &mut Criterion) {
    let Some(host) = common::host() else { return };
    let mut g = c.benchmark_group(format!("message_codec/{}", common::transport()));
    for &(name, n, resident) in &[
        ("340_all_miss", 340usize, false),
        ("340_all_hit", 340, true),
        ("1536_all_miss", 1536, false),
    ] {
        let src = common::Node::new(&host, if resident { n * ENTRY } else { 4096 });
        if resident {
            src.hold(n, ENTRY);
        }
        let dst = common::Node::new(&host, 4096);
        let slots = common::Slots::new(&dst.ep, n, ENTRY);
        let items = slots.items(n, ENTRY);
        let want = vec![
            if resident {
                EntryOutcome::HitMemoryTier
            } else {
                EntryOutcome::Miss
            };
            n
        ];
        assert_eq!(common::fetch(&dst.ep, &src.addr, &items), want, "warm-up");
        g.throughput(Throughput::Elements(n as u64));
        g.bench_with_input(BenchmarkId::from_parameter(name), &n, |b, _| {
            b.iter(|| common::fetch(&dst.ep, &src.addr, &items))
        });
        assert_eq!(src.map.held(), 0);
        drop((src, dst, slots));
    }
    g.finish();
}

criterion_group!(benches, fetch);
criterion_main!(benches);
