# Quickstart: Actor Model with Channel Components

**Feature**: 003-actor-channels | **Date**: 2026-03-31

## Scenario 1: Simple Actor with SPSC Channel

Two actors exchange messages through an SPSC channel — a producer sends integers, a consumer prints them.

```rust
use component_framework::*;

// Define message handler for the consumer
struct PrintHandler;

impl ActorHandler<u64> for PrintHandler {
    fn handle(&mut self, msg: u64) {
        println!("Received: {msg}");
    }
}

fn main() {
    // Create an SPSC channel (default capacity 1024)
    let channel = SpscChannel::<u64>::with_default_capacity();

    // Get sender and receiver endpoints
    let tx = channel.sender().unwrap();
    let rx = channel.receiver().unwrap();

    // Create and activate the consumer actor
    let consumer = Actor::new(PrintHandler, |panic_info| {
        eprintln!("Actor panicked: {panic_info:?}");
    });
    // The actor's inbound channel is the receiver
    let handle = consumer.activate().unwrap();

    // Producer sends messages (could also be an actor)
    for i in 0..10 {
        tx.send(i).unwrap();
    }

    // Clean shutdown
    drop(tx); // Close the channel
    handle.deactivate().unwrap();
}
```

## Scenario 2: Actor Pipeline (Producer -> Processor -> Consumer)

Three-stage pipeline using two SPSC channels.

```rust
use component_framework::*;

struct DoubleProcessor {
    output: Sender<u64>,
}

impl ActorHandler<u64> for DoubleProcessor {
    fn handle(&mut self, msg: u64) {
        self.output.send(msg * 2).unwrap();
    }
}

struct CollectorHandler {
    collected: Vec<u64>,
}

impl ActorHandler<u64> for CollectorHandler {
    fn handle(&mut self, msg: u64) {
        self.collected.push(msg);
    }
    fn on_stop(&mut self) {
        println!("Collected {} items: {:?}", self.collected.len(), self.collected);
    }
}

fn main() {
    // Channel 1: producer -> processor
    let ch1 = SpscChannel::<u64>::with_default_capacity();
    let tx1 = ch1.sender().unwrap();
    let rx1 = ch1.receiver().unwrap();

    // Channel 2: processor -> consumer
    let ch2 = SpscChannel::<u64>::with_default_capacity();
    let tx2 = ch2.sender().unwrap();
    let rx2 = ch2.receiver().unwrap();

    // Wire processor: reads from ch1, writes to ch2
    let processor = Actor::new(
        DoubleProcessor { output: tx2 },
        |e| eprintln!("Processor panic: {e:?}"),
    );
    let proc_handle = processor.activate().unwrap();

    // Wire consumer: reads from ch2
    let consumer = Actor::new(
        CollectorHandler { collected: vec![] },
        |e| eprintln!("Consumer panic: {e:?}"),
    );
    let cons_handle = consumer.activate().unwrap();

    // Producer sends
    for i in 1..=5 {
        tx1.send(i).unwrap();
    }

    // Shutdown cascade
    drop(tx1);
    proc_handle.deactivate().unwrap();
    cons_handle.deactivate().unwrap();
}
```

## Scenario 3: Fan-In with MPSC Channel

Multiple producers send to a single consumer through an MPSC channel.

```rust
use component_framework::*;
use std::thread;

fn main() {
    let channel = MpscChannel::<String>::with_default_capacity();
    let rx = channel.receiver().unwrap();

    // Spawn 3 producer threads, each with a cloned sender
    let mut handles = vec![];
    for id in 0..3 {
        let tx = channel.sender().unwrap(); // Each call succeeds (MPSC)
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                tx.send(format!("producer-{id}: msg-{i}")).unwrap();
            }
        }));
    }

    // Consumer collects all 300 messages
    let consumer = thread::spawn(move || {
        let mut count = 0;
        loop {
            match rx.recv() {
                Ok(_msg) => count += 1,
                Err(ChannelError::Closed) => break,
                Err(e) => panic!("Unexpected: {e:?}"),
            }
        }
        assert_eq!(count, 300);
        println!("Received all {count} messages");
    });

    for h in handles {
        h.join().unwrap();
    }
    // All senders dropped → channel closes → consumer exits
    consumer.join().unwrap();
}
```

## Scenario 4: Binding Enforcement

Demonstrates SPSC topology rejection.

```rust
use component_framework::*;

fn main() {
    let channel = SpscChannel::<u64>::with_default_capacity();

    // First sender/receiver — OK
    let _tx1 = channel.sender().unwrap();
    let _rx1 = channel.receiver().unwrap();

    // Second sender — REJECTED
    let result = channel.sender();
    assert!(result.is_err());
    // ChannelError::BindingRejected { reason: "SPSC channel already has a sender" }

    // Second receiver — REJECTED
    let result = channel.receiver();
    assert!(result.is_err());
}
```

## Scenario 5: Third-Party Binding via Registry

Actors and channels wired by string names.

```rust
use component_framework::*;
use std::any::Any;
use std::sync::Arc;

fn main() {
    let registry = ComponentRegistry::new();

    // Register actor and channel factories
    registry.register("producer-actor", |_: Option<&dyn Any>| {
        // Returns a ComponentRef wrapping the actor component
        Ok(ComponentRef::from(/* ... */))
    }).unwrap();

    registry.register("spsc-u64", |_: Option<&dyn Any>| {
        Ok(ComponentRef::from(SpscChannel::<u64>::with_default_capacity()))
    }).unwrap();

    // Create by name
    let producer = registry.create("producer-actor", None).unwrap();
    let channel = registry.create("spsc-u64", None).unwrap();

    // Bind by string names
    bind(&*channel, "ISender", &*producer, "output").unwrap();
}
```

## Integration Test Pattern

```rust
#[test]
fn spsc_channel_delivers_messages_in_order() {
    let ch = SpscChannel::<u32>::new(64);
    let tx = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    for i in 0..64 {
        tx.send(i).unwrap();
    }

    for i in 0..64 {
        assert_eq!(rx.recv().unwrap(), i);
    }
}

#[test]
fn mpsc_channel_no_message_loss() {
    let ch = MpscChannel::<u32>::new(1024);
    let rx = ch.receiver().unwrap();
    let mut handles = vec![];

    for producer_id in 0..8 {
        let tx = ch.sender().unwrap();
        handles.push(std::thread::spawn(move || {
            for i in 0..10_000 {
                tx.send(producer_id * 10_000 + i).unwrap();
            }
        }));
    }

    let consumer = std::thread::spawn(move || {
        let mut count = 0;
        loop {
            match rx.recv() {
                Ok(_) => count += 1,
                Err(_) => break,
            }
        }
        count
    });

    for h in handles {
        h.join().unwrap();
    }
    // Drop all senders (already moved into threads and dropped)
    drop(ch); // drops the channel's internal sender tracking

    assert_eq!(consumer.join().unwrap(), 80_000);
}
```
