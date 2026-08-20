//! Ping-pong example: two actors exchanging messages through SPSC channels.
//!
//! Demonstrates basic actor lifecycle and bidirectional communication.
//! Uses `split()`, `pipe()`, and `Actor::simple()` for concise wiring.

use component_framework::actor::{pipe, Actor, ActorHandler};
use component_framework::channel::spsc::SpscChannel;
use component_framework::channel::Sender;
use component_framework::{define_component, define_interface};

// Small demo: declare a trivial interface + component so this example
// demonstrates the macros without changing actor behavior.
define_interface! {
    pub IExamplePingPong {
        fn example(&self) -> u32;
    }
}

define_component! {
    pub ExamplePingPong {
        version: "0.1.0",
        provides: [IExamplePingPong],
    }
}

impl IExamplePingPong for ExamplePingPong {
    fn example(&self) -> u32 {
        42
    }
}
use std::sync::{Arc, Mutex};

/// Message exchanged between ping and pong actors.
#[derive(Debug)]
enum PingPong {
    Ping(u32),
    Pong(u32),
}

/// The Ping actor sends Ping(n) and expects Pong(n) back.
/// The Ping actor sends Ping(n) and expects Pong(n) back.
struct PingHandler {
    reply_tx: Sender<PingPong>,
    received: Arc<Mutex<Vec<u32>>>,
}

impl ActorHandler<PingPong> for PingHandler {
    fn handle(&mut self, msg: PingPong) {
        match msg {
            PingPong::Pong(n) => {
                println!("  Ping received Pong({n})");
                self.received.lock().unwrap().push(n);
            }
            PingPong::Ping(n) => {
                println!("  Ping got unexpected Ping({n}), replying with Pong");
                let _ = self.reply_tx.send(PingPong::Pong(n));
            }
        }
    }
}

/// The Pong actor receives Ping(n) and replies with Pong(n).
/// The Pong actor receives Ping(n) and replies with Pong(n).
struct PongHandler {
    reply_tx: Sender<PingPong>,
}

impl ActorHandler<PingPong> for PongHandler {
    fn handle(&mut self, msg: PingPong) {
        match msg {
            PingPong::Ping(n) => {
                println!("  Pong received Ping({n}), replying with Pong({n})");
                let _ = self.reply_tx.send(PingPong::Pong(n));
            }
            PingPong::Pong(n) => {
                println!("  Pong got unexpected Pong({n})");
            }
        }
    }
}

fn main() {
    println!("=== Actor Ping-Pong Example ===\n");

    let rounds = 5;
    let received = Arc::new(Mutex::new(Vec::new()));

    // Channel: Ping -> Pong (carries Ping messages)
    let (tx_to_pong, rx_from_ping) = SpscChannel::<PingPong>::new(16).split().unwrap();

    // Channel: Pong -> Ping (carries Pong replies)
    let (tx_to_ping, rx_from_pong) = SpscChannel::<PingPong>::new(16).split().unwrap();

    // Create a dummy sender for PingHandler (it only receives Pongs).
    let dummy_ch = SpscChannel::<PingPong>::new(16);
    let dummy_tx = dummy_ch.sender().unwrap();

    // Create and activate actors
    let pong = Actor::simple(PongHandler {
        reply_tx: tx_to_ping,
    });
    let pong_handle = pong.activate().unwrap();

    let ping = Actor::simple(PingHandler {
        reply_tx: dummy_tx,
        received: received.clone(),
    });
    let ping_handle = ping.activate().unwrap();

    // Pipe channels to actors — replaces manual forwarder threads
    let fwd_to_pong = pipe(rx_from_ping, pong_handle);
    let fwd_to_ping = pipe(rx_from_pong, ping_handle);

    // Send initial ping messages
    for i in 1..=rounds {
        println!("Sending Ping({i})");
        tx_to_pong.send(PingPong::Ping(i)).unwrap();
    }

    // Wait for all pongs to be received
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Shutdown: drop sender to close channel -> pipe deactivates actors
    drop(tx_to_pong);
    fwd_to_pong.join().unwrap();
    fwd_to_ping.join().unwrap();

    let received = received.lock().unwrap();
    println!(
        "\nPing received {}/{rounds} pong replies: {received:?}",
        received.len()
    );
    println!("\n=== Done ===");
}
