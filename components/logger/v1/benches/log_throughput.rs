use criterion::{black_box, criterion_group, criterion_main, Criterion};
use interfaces::ILogger;
use logger::{LogLevel, LoggerComponentV1};
use std::io::Write;
use std::sync::Arc;

struct NullWriter;

impl Write for NullWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bench_log_info(c: &mut Criterion) {
    let component =
        LoggerComponentV1::new_with_writer(Box::new(NullWriter), LogLevel::Debug, false);
    c.bench_function("log_info", |b| {
        b.iter(|| {
            component.info(black_box("benchmark message"));
        });
    });
}

fn bench_log_info_colored(c: &mut Criterion) {
    let component = LoggerComponentV1::new_with_writer(Box::new(NullWriter), LogLevel::Debug, true);
    c.bench_function("log_info_colored", |b| {
        b.iter(|| {
            component.info(black_box("benchmark message"));
        });
    });
}

fn bench_log_filtered_out(c: &mut Criterion) {
    let component =
        LoggerComponentV1::new_with_writer(Box::new(NullWriter), LogLevel::Error, false);
    c.bench_function("log_filtered_out", |b| {
        b.iter(|| {
            component.debug(black_box("this will be filtered"));
        });
    });
}

fn bench_log_concurrent(c: &mut Criterion) {
    let component =
        LoggerComponentV1::new_with_writer(Box::new(NullWriter), LogLevel::Debug, false);
    c.bench_function("log_concurrent_4_threads", |b| {
        b.iter(|| {
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let comp = Arc::clone(&component);
                    std::thread::spawn(move || {
                        for _ in 0..100 {
                            comp.info(black_box("concurrent message"));
                        }
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
        });
    });
}

criterion_group!(
    benches,
    bench_log_info,
    bench_log_info_colored,
    bench_log_filtered_out,
    bench_log_concurrent
);
criterion_main!(benches);
