//! Tokio ping-pong example: two tasks exchanging messages through
//! TokioMpscChannel components queried via IUnknown.
//!
//! Uses `tokio::task::spawn_blocking` because TokioMpscChannel's ISender /
//! IReceiver use `blocking_send` / `blocking_recv` under the hood.
//!
//! Run with: `cargo run --example tokio_ping_pong -p examples`

use component_framework::channel::tokio_mpsc::TokioMpscChannel;
use component_framework::channel::{IReceiver, ISender};
use component_framework::iunknown::query;
use component_framework::{define_component, define_interface};
use std::sync::Arc;

define_interface! {
    pub IExampleTokio {
        fn n(&self) -> u32;
    }
}

define_component! {
    pub ExampleTokio {
        version: "0.1.0",
        provides: [IExampleTokio],
    }
}

impl IExampleTokio for ExampleTokio {
    fn n(&self) -> u32 {
        3
    }
}

// Per-interface aliases for this example's Message type
type ISenderMsgObj = dyn ISender<Message> + Send + Sync;
type IReceiverMsgObj = dyn IReceiver<Message> + Send + Sync;
type SenderRef = Arc<ISenderMsgObj>;
type ReceiverRef = Arc<IReceiverMsgObj>;

#[derive(Debug)]
enum Message {
    Ping(u32),
    Pong(u32),
}

#[tokio::main]
async fn main() {
    println!("=== Tokio Ping-Pong Example ===\n");

    let rounds = 5u32;

    // Channel: ping -> pong (carries Ping messages)
    let ch_to_pong = TokioMpscChannel::<Message>::new(16);
    let tx_to_pong: SenderRef = query::<ISenderMsgObj>(&ch_to_pong).unwrap();
    let rx_from_ping: ReceiverRef = query::<IReceiverMsgObj>(&ch_to_pong).unwrap();

    // Channel: pong -> ping (carries Pong replies)
    let ch_to_ping = TokioMpscChannel::<Message>::new(16);
    let tx_to_ping: SenderRef = query::<ISenderMsgObj>(&ch_to_ping).unwrap();
    let rx_from_pong: ReceiverRef = query::<IReceiverMsgObj>(&ch_to_ping).unwrap();

    // Pong task: receives Ping(n) via ISender/IReceiver, replies with Pong(n)
    let pong_task = tokio::task::spawn_blocking(move || {
        loop {
            match rx_from_ping.recv() {
                Ok(Message::Ping(n)) => {
                    println!("  Pong received Ping({n}), replying with Pong({n})");
                    if tx_to_ping.send(Message::Pong(n)).is_err() {
                        break;
                    }
                }
                Ok(Message::Pong(n)) => {
                    println!("  Pong got unexpected Pong({n})");
                }
                Err(_) => break,
            }
        }
        println!("  Pong task exiting");
    });

    // Ping task: sends Ping(1..=rounds), collects Pong replies
    let ping_task = tokio::task::spawn_blocking(move || {
        for i in 1..=rounds {
            println!("Ping sending Ping({i})");
            tx_to_pong
                .send(Message::Ping(i))
                .expect("pong channel open");
        }

        let mut received = Vec::new();
        for _ in 0..rounds {
            match rx_from_pong.recv() {
                Ok(Message::Pong(n)) => {
                    println!("  Ping received Pong({n})");
                    received.push(n);
                }
                Ok(Message::Ping(n)) => {
                    println!("  Ping got unexpected Ping({n})");
                }
                Err(_) => break,
            }
        }
        received
    });

    let received = ping_task.await.expect("ping task panicked");

    // Drop channels so pong's recv returns Err(Closed) and it exits
    drop(ch_to_pong);
    pong_task.await.expect("pong task panicked");

    println!(
        "\nPing received {}/{rounds} pong replies: {received:?}",
        received.len()
    );
    assert_eq!(received, (1..=rounds).collect::<Vec<_>>());
    println!("\n=== Done ===");
}
