# Certus

Certus is a generative domain-specific filesystem for inferencing workloads. The implementation is based on the integration of components that somewhat supports a paradigm of **independent extensibilty** - that is, components can be developed separately and later integrated into the final solution.  This approach also helps to reduce the required LLM context window by limiting it to the component being developed and other components it must bind to (note: components should have low coupling and only bind to a few other components).

## Component Framework

The core infrastructure is a Rust component framework inspired by COM (Component Object Model) principles.
It provides a structured way to define, discover, connect, and manage software components at runtime
through standardized interfaces, with first-class support for the actor model, high-performance
lock-free channels, and NUMA-aware execution.

### Key Concepts

- **Interfaces** are declared with `define_interface!` and expose capabilities as trait objects queryable at runtime.
- **Components** are declared with `define_component!` and implement one or more interfaces. Every component implements `IUnknown` for runtime discovery and connection.
- **Receptacles** are typed slots that let components declare required dependencies, wired either directly or through a `ComponentRegistry` with string-based `bind()`.
- **Actors** run on dedicated OS threads, communicate through lock-free channels, and integrate with the component model via `ISender<M>`.
- **Channels** include built-in SPSC and MPSC lock-free implementations, plus adapters for crossbeam, kanal, rtrb, and tokio.
- **NUMA** support provides topology discovery, thread pinning, and NUMA-local memory allocation.

For full API documentation, see [components/component-framework/README.md](components/component-framework/README.md).

### Building

```bash
cargo build           # Build all workspace members
cargo test --all      # Run all tests (unit + integration + doc)
cargo doc --no-deps   # Build documentation
```

### Running Examples

Examples live in `components/component-framework/examples/` and cover the major framework features:

```bash
# Basic component definition and interface querying
cargo run --example basic

# Connect two components via receptacles
cargo run --example wiring

# Enumerate provided interfaces and receptacles at runtime
cargo run --example introspection

# Registry-based creation and third-party binding
cargo run --example binding

# Two actors exchanging messages through SPSC channels
cargo run --example actor_ping_pong

# Three-stage actor pipeline: producer -> processor -> consumer
cargo run --example actor_pipeline

# Multiple producers feeding a single consumer via MPSC
cargo run --example actor_fan_in

# Built-in logging actor with level filtering and file output
cargo run --example actor_log

# NUMA topology discovery, thread pinning, and cross-node latency
cargo run --example numa_pinning
```

### Running Benchmarks

Benchmarks use [Criterion](https://github.com/bheisler/criterion.rs) and live in
`components/component-framework/crates/component-framework/benches/`.

```bash
# Run all benchmarks
cargo bench

# Run a specific benchmark suite
cargo bench --bench channel_spsc_benchmark
cargo bench --bench channel_mpsc_benchmark
cargo bench --bench actor_latency
cargo bench --bench numa_latency_benchmark
cargo bench --bench query_interface
```

Available benchmark suites:

| Suite | What it measures |
|-------|-----------------|
| `query_interface` | Interface map lookup (hit and miss) |
| `receptacle` | Receptacle connect and get latency |
| `method_dispatch` | Indirect dispatch through `Arc<dyn Trait>` |
| `registry` | Registry register, create, and list |
| `binding` | First-party and third-party wiring |
| `component_ref` | ComponentRef creation and clone overhead |
| `channel_throughput` | SPSC and MPSC message throughput |
| `channel_spsc_benchmark` | SPSC throughput across all backends and payload sizes |
| `channel_mpsc_benchmark` | MPSC throughput with varying producer counts |
| `channel_latency_benchmark` | Per-message round-trip latency |
| `actor_latency` | Actor activation time and message round-trip |
| `numa_latency_benchmark` | Same-node vs cross-node SPSC latency |
| `numa_throughput_benchmark` | Same-node vs cross-node SPSC throughput |
