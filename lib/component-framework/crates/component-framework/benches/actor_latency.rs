//! Benchmarks for actor message latency.

use component_framework::actor::{Actor, ActorHandler};
use criterion::{criterion_group, criterion_main, Criterion};
use std::sync::{Arc, Mutex};

struct CountHandler {
    count: Arc<Mutex<u64>>,
}

impl ActorHandler<u64> for CountHandler {
    fn handle(&mut self, _msg: u64) {
        *self.count.lock().unwrap() += 1;
    }
}

fn actor_message_latency(c: &mut Criterion) {
    c.bench_function("actor_send_1000_msgs", |b| {
        b.iter(|| {
            let count = Arc::new(Mutex::new(0u64));
            let actor = Actor::new(
                CountHandler {
                    count: count.clone(),
                },
                |_| {},
            );
            let handle = actor.activate().unwrap();

            for i in 0..1000u64 {
                handle.send(i).unwrap();
            }

            handle.deactivate().unwrap();
            assert_eq!(*count.lock().unwrap(), 1000);
        });
    });
}

criterion_group!(benches, actor_message_latency);
criterion_main!(benches);
