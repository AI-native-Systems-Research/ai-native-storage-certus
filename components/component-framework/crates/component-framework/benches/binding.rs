use criterion::{black_box, criterion_group, criterion_main, Criterion};

use component_framework::binding::bind;
use component_framework::{define_component, define_interface};

define_interface! {
    pub IBenchSvc {
        fn process(&self) -> u32;
    }
}

define_component! {
    pub Provider {
        version: "1.0.0",
        provides: [IBenchSvc],
    }
}

impl IBenchSvc for Provider {
    fn process(&self) -> u32 {
        1
    }
}

define_component! {
    pub Consumer {
        version: "1.0.0",
        provides: [],
        receptacles: {
            svc: IBenchSvc,
        },
    }
}

fn bench_bind(c: &mut Criterion) {
    c.bench_function("bind_third_party", |b| {
        b.iter(|| {
            let provider = Provider::new();
            let consumer = Consumer::new();
            bind(
                black_box(&*provider),
                "IBenchSvc",
                black_box(&*consumer),
                "svc",
            )
            .unwrap();
        });
    });
}

criterion_group!(benches, bench_bind);
criterion_main!(benches);
