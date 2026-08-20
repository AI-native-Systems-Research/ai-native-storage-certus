//! Fan-in example: multiple producers -> MPSC channel -> single consumer.
//!
//! Demonstrates the MPSC channel with multiple concurrent producer threads
//! sending to a single consumer actor.
//! Uses `split()`, `pipe_mpsc()`, and `Actor::simple()` for concise wiring.

use component_framework::actor::{pipe_mpsc, Actor, ActorHandler};
use component_framework::channel::mpsc::MpscChannel;
use component_framework::{define_component, define_interface};

define_interface! {
    pub IExampleFanIn {
        fn example(&self) -> u32;
    }
}

define_component! {
    pub ExampleFanIn {
        version: "0.1.0",
        provides: [IExampleFanIn],
    }
}

impl IExampleFanIn for ExampleFanIn {
    fn example(&self) -> u32 {
        7
    }
}
use std::sync::{Arc, Mutex};

/// Consumer that counts messages from each producer.
struct FanInConsumer {
    counts: Arc<Mutex<Vec<u32>>>,
    num_producers: usize,
}

impl ActorHandler<(usize, u32)> for FanInConsumer {
    fn handle(&mut self, msg: (usize, u32)) {
        let (producer_id, _value) = msg;
        let mut counts = self.counts.lock().unwrap();
        if producer_id < self.num_producers {
            counts[producer_id] += 1;
        }
    }

    fn on_stop(&mut self) {
        let counts = self.counts.lock().unwrap();
        let total: u32 = counts.iter().sum();
        println!("\n  Consumer received {total} total messages:");
        for (i, count) in counts.iter().enumerate() {
            println!("    Producer {i}: {count} messages");
        }
    }
}

fn main() {
    println!("=== Actor Fan-In Example ===");
    println!("  3 producers -> MPSC channel -> 1 consumer\n");

    let num_producers = 3;
    let msgs_per_producer = 100;

    let counts = Arc::new(Mutex::new(vec![0u32; num_producers]));

    // MPSC channel — split into first sender + receiver
    let ch = MpscChannel::<(usize, u32)>::new(256);
    let rx = ch.receiver().unwrap();

    // Consumer actor with pipe
    let consumer = Actor::simple(FanInConsumer {
        counts: counts.clone(),
        num_producers,
    });
    let consumer_handle = consumer.activate().unwrap();
    let fwd = pipe_mpsc(rx, consumer_handle);

    // Spawn producer threads
    let mut producer_handles = vec![];
    for pid in 0..num_producers {
        let tx = ch.sender().unwrap();
        producer_handles.push(std::thread::spawn(move || {
            for i in 0..msgs_per_producer as u32 {
                tx.send((pid, i)).unwrap();
            }
            println!("  Producer {pid} sent {msgs_per_producer} messages");
        }));
    }

    // Wait for producers to finish
    for h in producer_handles {
        h.join().unwrap();
    }

    // Close channel (drop all senders)
    drop(ch);

    // Wait for consumer to drain
    fwd.join().unwrap();

    // Verify
    let counts = counts.lock().unwrap();
    let total: u32 = counts.iter().sum();
    assert_eq!(total, (num_producers * msgs_per_producer) as u32);

    println!("\n=== Fan-in complete: all {total} messages received ===");
}
