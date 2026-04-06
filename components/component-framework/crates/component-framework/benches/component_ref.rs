use criterion::{black_box, criterion_group, criterion_main, Criterion};

use component_framework::component_ref::ComponentRef;
use component_framework::{define_component, define_interface};

define_interface! {
    pub IBenchRef {
        fn val(&self) -> u32;
    }
}

define_component! {
    pub RefComp {
        version: "1.0.0",
        provides: [IBenchRef],
    }
}

impl IBenchRef for RefComp {
    fn val(&self) -> u32 {
        7
    }
}

fn bench_attach_release(c: &mut Criterion) {
    let comp = ComponentRef::from(RefComp::new());

    c.bench_function("component_ref_attach_release", |b| {
        b.iter(|| {
            let c2 = black_box(&comp).attach();
            black_box(c2);
            // c2 dropped here (release)
        });
    });
}

fn bench_clone(c: &mut Criterion) {
    let comp = ComponentRef::from(RefComp::new());

    c.bench_function("component_ref_clone", |b| {
        b.iter(|| {
            let c2 = black_box(&comp).clone();
            black_box(c2);
        });
    });
}

criterion_group!(benches, bench_attach_release, bench_clone);
criterion_main!(benches);
