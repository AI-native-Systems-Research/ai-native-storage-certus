//! Criterion benchmark for the message codec (task T058).
//!
//! Placeholder: the stanza and harness exist so `cargo bench` and
//! `cargo build --all-targets` stay green from T001 onward. The real
//! measurement arrives with the codec itself.

use criterion::{criterion_group, criterion_main, Criterion};

fn placeholder(c: &mut Criterion) {
    c.bench_function("message_codec/not_yet_implemented", |b| b.iter(|| {}));
}

criterion_group!(benches, placeholder);
criterion_main!(benches);
