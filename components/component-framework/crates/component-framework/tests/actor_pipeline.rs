//! Integration tests for actor-to-actor communication pipelines.

use component_framework::actor::{Actor, ActorHandler};
use component_framework::channel::spsc::SpscChannel;
use component_framework::channel::Sender;
use std::sync::{Arc, Mutex};

/// A processor that doubles each value and forwards it.
struct DoublerHandler {
    output: Sender<u64>,
}

impl ActorHandler<u64> for DoublerHandler {
    fn handle(&mut self, msg: u64) {
        self.output.send(msg * 2).unwrap();
    }
}

/// A consumer that collects messages.
struct CollectorHandler {
    collected: Arc<Mutex<Vec<u64>>>,
}

impl ActorHandler<u64> for CollectorHandler {
    fn handle(&mut self, msg: u64) {
        self.collected.lock().unwrap().push(msg);
    }
}

#[test]
fn two_actors_through_spsc_channel() {
    let collected = Arc::new(Mutex::new(Vec::new()));

    // Channel between producer and consumer
    let ch = SpscChannel::<u64>::new(1024);
    let tx = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    // Consumer actor reads from channel
    let consumer = Actor::new(
        CollectorHandler {
            collected: collected.clone(),
        },
        |_| {},
    );
    let consumer_handle = consumer.activate().unwrap();

    // Spawn a thread to forward from channel to consumer actor
    let forward_handle = std::thread::spawn(move || {
        loop {
            match rx.recv() {
                Ok(val) => consumer_handle.send(val).unwrap(),
                Err(_) => break,
            }
        }
        consumer_handle.deactivate().unwrap();
    });

    // Producer sends through channel
    for i in 0..100u64 {
        tx.send(i).unwrap();
    }
    drop(tx); // close channel → forwarder exits → consumer deactivates

    forward_handle.join().unwrap();

    let collected = collected.lock().unwrap();
    let expected: Vec<u64> = (0..100).collect();
    assert_eq!(*collected, expected);
}

#[test]
fn three_stage_pipeline_producer_doubler_consumer() {
    let collected = Arc::new(Mutex::new(Vec::new()));

    // Channel 1: producer -> doubler
    let ch1 = SpscChannel::<u64>::new(1024);
    let tx1 = ch1.sender().unwrap();
    let rx1 = ch1.receiver().unwrap();

    // Channel 2: doubler -> consumer
    let ch2 = SpscChannel::<u64>::new(1024);
    let tx2 = ch2.sender().unwrap();
    let rx2 = ch2.receiver().unwrap();

    // Consumer actor
    let consumer = Actor::new(
        CollectorHandler {
            collected: collected.clone(),
        },
        |_| {},
    );
    let consumer_handle = consumer.activate().unwrap();

    // Forward ch2 -> consumer actor
    let forward2 = std::thread::spawn(move || {
        loop {
            match rx2.recv() {
                Ok(val) => consumer_handle.send(val).unwrap(),
                Err(_) => break,
            }
        }
        consumer_handle.deactivate().unwrap();
    });

    // Doubler actor
    let doubler = Actor::new(DoublerHandler { output: tx2 }, |_| {});
    let doubler_handle = doubler.activate().unwrap();

    // Forward ch1 -> doubler actor
    let forward1 = std::thread::spawn(move || {
        loop {
            match rx1.recv() {
                Ok(val) => doubler_handle.send(val).unwrap(),
                Err(_) => break,
            }
        }
        doubler_handle.deactivate().unwrap();
    });

    // Producer sends 1..=5
    for i in 1..=5u64 {
        tx1.send(i).unwrap();
    }
    drop(tx1); // close ch1 → forward1 exits → doubler deactivates → ch2 closes → forward2 exits → consumer deactivates

    forward1.join().unwrap();
    forward2.join().unwrap();

    let collected = collected.lock().unwrap();
    // Each value doubled: [2, 4, 6, 8, 10]
    assert_eq!(*collected, vec![2, 4, 6, 8, 10]);
}
