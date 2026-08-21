//! NUMA Pinning Example
//!
//! Demonstrates topology discovery, actor thread pinning, and NUMA-local
//! channel communication. Measures round-trip latency for same-node and
//! cross-node actor pairs.
//!
//! # Running
//!
//! ```sh
//! cargo run --example numa_pinning
//! ```

use component_core::actor::{Actor, ActorHandler};
use component_core::channel::spsc::SpscChannel;
use component_core::numa::{CpuSet, NumaTopology};
use component_framework::{define_component, define_interface};

define_interface! {
    pub IExampleNuma {
        fn id(&self) -> u32;
    }
}

define_component! {
    pub ExampleNuma {
        version: "0.1.0",
        provides: [IExampleNuma],
    }
}

impl IExampleNuma for ExampleNuma {
    fn id(&self) -> u32 {
        0
    }
}
use std::sync::{Arc, Mutex};
use std::time::Instant;

const WARMUP: usize = 100;
const ITERATIONS: usize = 10_000;

/// Measure round-trip latency between two CPUs.
fn measure_roundtrip(cpu_send: usize, cpu_recv: usize) -> Option<std::time::Duration> {
    // Forward channel: main -> echo actor
    let fwd = SpscChannel::<u64>::new(64);
    let fwd_tx = fwd.sender().ok()?;
    let fwd_rx = fwd.receiver().ok()?;

    // Reply channel: echo actor -> main
    let reply = SpscChannel::<u64>::new(64);
    let reply_tx = reply.sender().ok()?;
    let reply_rx = reply.receiver().ok()?;

    let reply_tx_shared = Arc::new(Mutex::new(Some(reply_tx)));

    // The echo actor receives from fwd and sends back on reply.
    // We can't easily use Actor with a custom channel, so we'll use raw threads
    // to demonstrate the pinning concept directly.
    let reply_tx_clone = reply_tx_shared.clone();
    let receiver_thread = std::thread::spawn(move || {
        // Pin to recv CPU.
        let cs = CpuSet::from_cpu(cpu_recv).unwrap();
        component_core::numa::set_thread_affinity(&cs).unwrap();

        let tx_guard = reply_tx_clone.lock().unwrap();
        let tx = tx_guard.as_ref().unwrap();

        // Process messages until channel closes.
        while let Ok(msg) = fwd_rx.recv() {
            let _ = tx.send(msg);
        }
    });

    // Pin sender thread (current).
    let cs = CpuSet::from_cpu(cpu_send).unwrap();
    if component_core::numa::set_thread_affinity(&cs).is_err() {
        eprintln!("  Warning: could not pin to CPU {cpu_send}");
    }

    // Warmup
    for i in 0..WARMUP as u64 {
        fwd_tx.send(i).unwrap();
        reply_rx.recv().unwrap();
    }

    // Timed iterations
    let start = Instant::now();
    for i in 0..ITERATIONS as u64 {
        fwd_tx.send(i).unwrap();
        reply_rx.recv().unwrap();
    }
    let elapsed = start.elapsed();

    // Clean up
    drop(fwd_tx);
    receiver_thread.join().ok();

    Some(elapsed)
}

fn main() {
    println!("=== NUMA Pinning Example ===\n");

    // 1. Discover topology
    let topo = NumaTopology::discover().expect("Failed to discover NUMA topology");
    println!("NUMA Topology:");
    println!("  Nodes: {}", topo.node_count());
    for node in topo.nodes() {
        let cpus: Vec<usize> = node.cpus().iter().collect();
        println!("  Node {}: CPUs {:?}", node.id(), cpus);
        if !node.distances().is_empty() {
            println!("    Distances: {:?}", node.distances());
        }
    }
    println!();

    // 2. Same-node latency
    let node0 = topo.node(0).expect("No node 0");
    let cpus0: Vec<usize> = node0.cpus().iter().take(2).collect();

    if cpus0.len() >= 2 {
        println!(
            "Same-node test: CPU {} <-> CPU {} (both on node 0)",
            cpus0[0], cpus0[1]
        );
        match measure_roundtrip(cpus0[0], cpus0[1]) {
            Some(elapsed) => {
                let per_msg = elapsed / ITERATIONS as u32;
                println!(
                    "  {} round-trips in {:.2?} ({:.0?}/msg)\n",
                    ITERATIONS, elapsed, per_msg
                );
            }
            None => println!("  Failed to run same-node test\n"),
        }
    } else {
        println!("Only 1 CPU on node 0 — cannot run same-node test\n");
    }

    // 3. Cross-node latency (if available)
    if topo.node_count() >= 2 {
        let node1 = topo.node(1).expect("No node 1");
        let cpu0 = cpus0[0];
        let cpu1 = node1.cpus().iter().next().expect("Node 1 has no CPUs");

        println!(
            "Cross-node test: CPU {} (node 0) <-> CPU {} (node 1)",
            cpu0, cpu1
        );
        match measure_roundtrip(cpu0, cpu1) {
            Some(elapsed) => {
                let per_msg = elapsed / ITERATIONS as u32;
                println!(
                    "  {} round-trips in {:.2?} ({:.0?}/msg)\n",
                    ITERATIONS, elapsed, per_msg
                );
            }
            None => println!("  Failed to run cross-node test\n"),
        }
    } else {
        println!("Single NUMA node — cross-node test skipped\n");
    }

    // 4. Demonstrate Actor with CPU affinity
    println!("Actor with CPU affinity:");
    let first_cpu = topo.online_cpus().iter().next().unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));

    struct CpuReporter {
        observed: Arc<Mutex<Vec<usize>>>,
    }
    impl ActorHandler<()> for CpuReporter {
        fn handle(&mut self, _msg: ()) {
            if let Ok(cpus) = component_core::numa::get_thread_affinity() {
                *self.observed.lock().unwrap() = cpus.iter().collect();
            }
        }
    }

    let actor = Actor::new(
        CpuReporter {
            observed: observed.clone(),
        },
        |_| {},
    )
    .with_cpu_affinity(CpuSet::from_cpu(first_cpu).unwrap());

    let handle = actor.activate().unwrap();
    handle.send(()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    handle.deactivate().unwrap();

    let cpus = observed.lock().unwrap();
    println!(
        "  Actor pinned to CPU {}, observed affinity: {:?}",
        first_cpu, *cpus
    );
    println!("\nDone.");
}
