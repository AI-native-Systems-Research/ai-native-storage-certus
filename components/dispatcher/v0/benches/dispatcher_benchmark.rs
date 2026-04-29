use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dispatcher::io_segmenter::segment_io;

fn bench_segment_io_small(c: &mut Criterion) {
    c.bench_function("segment_io_4k", |b| {
        b.iter(|| segment_io(black_box(0), black_box(4096), 131072, 4096));
    });
}

fn bench_segment_io_1m(c: &mut Criterion) {
    c.bench_function("segment_io_1m", |b| {
        b.iter(|| segment_io(black_box(0), black_box(1024 * 1024), 131072, 4096));
    });
}

criterion_group!(benches, bench_segment_io_small, bench_segment_io_1m);
criterion_main!(benches);
