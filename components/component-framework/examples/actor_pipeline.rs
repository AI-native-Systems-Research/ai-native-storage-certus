//! Producer -> Processor -> Consumer pipeline example.
//!
//! Demonstrates a 3-stage actor pipeline where:
//! 1. Producer generates numbers
//! 2. Processor squares each number
//! 3. Consumer collects and prints results
//!
//! Uses `split()`, `pipe()`, and `Actor::simple()` for concise wiring.

use component_framework::actor::{pipe, Actor, ActorHandler};
use component_framework::channel::spsc::SpscChannel;
use component_framework::channel::Sender;
use component_framework::{define_component, define_interface};

define_interface! {
    pub IExamplePipeline {
        fn example(&self) -> u32;
    }
}

define_component! {
    pub ExamplePipeline {
        version: "0.1.0",
        provides: [IExamplePipeline],
    }
}

impl IExamplePipeline for ExamplePipeline {
    fn example(&self) -> u32 {
        0
    }
}
use std::sync::{Arc, Mutex};

/// Processor: squares each number and forwards it.
/// Processor: squares each number and forwards it.
struct SquareProcessor {
    output: Sender<u64>,
}

impl ActorHandler<u64> for SquareProcessor {
    fn handle(&mut self, msg: u64) {
        let squared = msg * msg;
        println!("  Processor: {msg} -> {squared}");
        self.output.send(squared).unwrap();
    }
}

/// Consumer: collects all results.
/// Consumer: collects all results.
struct Consumer {
    results: Arc<Mutex<Vec<u64>>>,
}

impl ActorHandler<u64> for Consumer {
    fn handle(&mut self, msg: u64) {
        println!("  Consumer received: {msg}");
        self.results.lock().unwrap().push(msg);
    }

    fn on_stop(&mut self) {
        let results = self.results.lock().unwrap();
        println!(
            "\n  Consumer collected {} items: {:?}",
            results.len(),
            *results
        );
    }
}

fn main() {
    println!("=== Actor Pipeline Example ===");
    println!("  Producer -> Squarer -> Consumer\n");

    let results = Arc::new(Mutex::new(Vec::new()));

    // Channel 1: producer -> processor
    let (tx1, rx1) = SpscChannel::<u64>::new(64).split().unwrap();

    // Channel 2: processor -> consumer
    let (tx2, rx2) = SpscChannel::<u64>::new(64).split().unwrap();

    // Create and activate actors
    let consumer = Actor::simple(Consumer {
        results: results.clone(),
    });
    let consumer_handle = consumer.activate().unwrap();

    let processor = Actor::simple(SquareProcessor { output: tx2 });
    let processor_handle = processor.activate().unwrap();

    // Pipe channels to actors — replaces manual forwarder threads
    let fwd2 = pipe(rx2, consumer_handle);
    let fwd1 = pipe(rx1, processor_handle);

    // Producer: send numbers 1..=10
    println!("  Producer sending 1..=10");
    for i in 1..=10u64 {
        tx1.send(i).unwrap();
    }

    // Shutdown cascade: drop tx1 -> ch1 closes -> fwd1 exits -> processor
    // deactivates -> tx2 drops -> ch2 closes -> fwd2 exits -> consumer deactivates
    drop(tx1);
    fwd1.join().unwrap();
    fwd2.join().unwrap();

    let results = results.lock().unwrap();
    let expected: Vec<u64> = (1..=10).map(|x| x * x).collect();
    assert_eq!(*results, expected);

    println!("\n=== Pipeline complete ===");
}
