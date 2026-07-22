//! Wire-codec micro-benchmark (T033).
//!
//! Encoding/decoding the v1 framed messages is on the per-operation hot path —
//! every KEY_QUERY / KEY_RESPONSE / RDMA_REQUEST / RDMA_STATUS is serialized and
//! parsed by the actor as it correlates a `batch_lookup`. This measures that
//! round trip for representative message shapes so regressions in the framing
//! are caught (Constitution: performance-sensitive code has Criterion benches).

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use interfaces::Endpoint;
use remote_lookup::wire::{Avail, RdmaStatusCode, SlotDesc, WireMessage};

fn endpoint() -> Endpoint {
    Endpoint {
        ip: "192.0.2.10".into(),
        port: 49152,
    }
}

/// A batch of representative messages covering all four wire types.
fn sample_messages() -> Vec<WireMessage> {
    let n = 64u64;
    vec![
        WireMessage::KeyQuery {
            op_id: 7,
            entries: (0..n).map(|k| (k, 4096)).collect(),
        },
        WireMessage::KeyResponse {
            op_id: 7,
            endpoint: endpoint(),
            entries: (0..n)
                .map(|k| {
                    let avail = match k % 3 {
                        0 => Avail::Memory,
                        1 => Avail::Disk,
                        _ => Avail::None,
                    };
                    (k, 4096, avail)
                })
                .collect(),
        },
        WireMessage::RdmaRequest {
            op_id: 7,
            endpoint: endpoint(),
            rkey: 0xDEAD_BEEF,
            slots: (0..n)
                .map(|k| SlotDesc {
                    key: k,
                    addr: 0x7f00_0000 + k * 4096,
                    length: 4096,
                })
                .collect(),
        },
        WireMessage::RdmaStatus {
            op_id: 7,
            entries: (0..n)
                .map(|k| {
                    let code = match k % 3 {
                        0 => RdmaStatusCode::Success,
                        1 => RdmaStatusCode::KeyNoLongerAvailable,
                        _ => RdmaStatusCode::UnableToConnect,
                    };
                    (k, code)
                })
                .collect(),
        },
    ]
}

fn bench_wire_codec(c: &mut Criterion) {
    let messages = sample_messages();

    c.bench_function("wire_encode_batch", |b| {
        b.iter(|| {
            let mut total = 0usize;
            for m in &messages {
                total += m.encode().len();
            }
            total
        })
    });

    let encoded: Vec<Vec<u8>> = messages.iter().map(WireMessage::encode).collect();
    c.bench_function("wire_decode_batch", |b| {
        b.iter_batched(
            || encoded.clone(),
            |frames| {
                for f in &frames {
                    let _ = WireMessage::decode(f).expect("decode");
                }
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_wire_codec);
criterion_main!(benches);
