use component_framework::iunknown::query;
use component_framework::receptacle::Receptacle;
use component_framework::{define_component, define_interface};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::sync::Arc;

define_interface! {
    pub IBenchLogger {
        fn log(&self, msg: &str) -> String;
    }
}

define_component! {
    pub LogProvider {
        version: "1.0.0",
        provides: [IBenchLogger],
    }
}

impl IBenchLogger for LogProvider {
    fn log(&self, msg: &str) -> String {
        format!("LOG: {msg}")
    }
}

fn bench_receptacle(c: &mut Criterion) {
    let provider = LogProvider::new();
    let ilogger: Arc<dyn IBenchLogger + Send + Sync> =
        query::<dyn IBenchLogger + Send + Sync>(&*provider).unwrap();

    c.bench_function("receptacle_connect", |b| {
        let r: Receptacle<dyn IBenchLogger + Send + Sync> = Receptacle::new();
        b.iter(|| {
            r.connect(black_box(ilogger.clone())).unwrap();
            r.disconnect().unwrap();
        })
    });

    c.bench_function("receptacle_get", |b| {
        let r: Receptacle<dyn IBenchLogger + Send + Sync> = Receptacle::new();
        r.connect(ilogger.clone()).unwrap();
        b.iter(|| {
            let _ = black_box(r.get().unwrap());
        })
    });
}

criterion_group!(benches, bench_receptacle);
criterion_main!(benches);
