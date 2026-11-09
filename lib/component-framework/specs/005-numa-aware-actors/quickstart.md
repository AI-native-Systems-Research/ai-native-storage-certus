# Quickstart: NUMA-Aware Actor Thread Pinning

Pin actor threads to specific CPUs and NUMA nodes for latency-sensitive workloads.

## Prerequisites

1. Linux system with 2+ NUMA nodes (use `numactl --hardware` to verify)
2. Project built with `cargo build`

## Basic Usage: Pin an Actor to a CPU

```rust
use component_core::actor::{Actor, ActorHandler};
use component_core::numa::CpuSet;

struct MyHandler;
impl ActorHandler<String> for MyHandler {
    fn handle(&mut self, msg: String) {
        println!("Processing: {msg}");
    }
}

// Pin actor to CPU 0
let affinity = CpuSet::from_cpu(0).unwrap();
let actor = Actor::new(MyHandler, |_| {})
    .with_cpu_affinity(affinity);

let handle = actor.activate().unwrap();
// Actor thread is now running on CPU 0
```

## Discover NUMA Topology

```rust
use component_core::numa::NumaTopology;

let topology = NumaTopology::discover().unwrap();
println!("NUMA nodes: {}", topology.node_count());

for node in topology.nodes() {
    println!("Node {}: CPUs {:?}", node.id(), node.cpus().iter().collect::<Vec<_>>());
}
```

## Pin Actors to Same NUMA Node

```rust
let topology = NumaTopology::discover().unwrap();
let node0 = topology.node(0).unwrap();
let cpus: Vec<usize> = node0.cpus().iter().collect();

// Pin producer to first CPU on node 0
let producer = Actor::new(ProducerHandler, |_| {})
    .with_cpu_affinity(CpuSet::from_cpu(cpus[0]).unwrap());

// Pin consumer to second CPU on node 0
let consumer = Actor::new(ConsumerHandler, |_| {})
    .with_cpu_affinity(CpuSet::from_cpu(cpus[1]).unwrap());
```

## NUMA-Local Channel Allocation

```rust
use component_core::channel::mpsc::MpscChannel;

// Allocate channel buffer on NUMA node 0
let channel: MpscChannel<String> = MpscChannel::new_numa(1024, 0);
```

## Run NUMA Benchmarks

```bash
# Run all NUMA benchmarks
cargo bench --bench numa_latency_benchmark
cargo bench --bench numa_throughput_benchmark

# View HTML report
open target/criterion/report/index.html
```

## Run NUMA Pinning Example

```bash
cargo run --example numa_pinning
```

Output:
```
NUMA Topology:
  Node 0: CPUs [0, 1, 2, ...]
  Node 1: CPUs [16, 17, 18, ...]

Same-node round-trip latency: ~150ns
Cross-node round-trip latency: ~350ns
```

## Change Affinity Between Activations

```rust
let actor = Actor::new(MyHandler, |_| {});

// First run on CPU 0
actor.set_cpu_affinity(CpuSet::from_cpu(0).unwrap()).unwrap();
let handle = actor.activate().unwrap();
handle.deactivate().unwrap();

// Second run on CPU 4
actor.set_cpu_affinity(CpuSet::from_cpu(4).unwrap()).unwrap();
let handle = actor.activate().unwrap();
handle.deactivate().unwrap();
```

## Error Handling

```rust
// Invalid CPU ID
let result = CpuSet::from_cpu(9999);
assert!(result.is_err()); // NumaError::CpuOutOfRange

// Empty CPU set
let empty = CpuSet::new();
// Passing empty set to actor will fail at activate()

// Change affinity while running
let handle = actor.activate().unwrap();
let result = actor.set_cpu_affinity(CpuSet::from_cpu(1).unwrap());
assert!(result.is_err()); // ActorError::AlreadyActive
```
