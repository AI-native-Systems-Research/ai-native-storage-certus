# Certus

Certus is a generative domain-specific filesystem for inferencing workloads. The implementation is based on the integration of components that somewhat supports a paradigm of **independent extensibilty** - that is, components can be developed separately and later integrated into the final solution.  This approach also helps to reduce the required LLM context window by limiting it to the component being developed and other components it must bind to (note: components should have low coupling and only bind to a few other components).

## Claude and Spec-Kit

Install Claude with:

```
curl -fsSL https://claude.ai/install.sh | bash
```

Install spec-kit with:

```
uv tool install specify-cli --from git+https://github.com/github/spec-kit.git@v0.5.1
```

Install spec-kit-sync extension:

```
specify extension add spec-kit-async --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip
```

## Quick Start

### Prerequisites

- Rust toolchain (edition 2021, MSRV 1.75)
- Linux (tested on RHEL/Fedora)
- `memlock` set to unlimited in `/etc/security/limits.conf` (required for SPDK hugepage and VFIO usage):
  ```
  * hard memlock unlimited
  * soft memlock unlimited
  ```
  Log out and back in after making this change for it to take effect.


- Kernel boot parameters for IOMMU and hugepages must be configured (one-time setup, requires reboot):
  ```bash
  # For Intel:
  components/spdk-env/scripts/add_kernel_options.sh
  # This runs: grubby --update-kernel=ALL --args="intel_iommu=on iommu=pt default_hugepagesz=1G hugepagesz=1G hugepages=4"
  ```
  **AMD architectures** require `amd_iommu=on` instead of `intel_iommu=on`:
  ```bash
  grubby --update-kernel=ALL --args="amd_iommu=on iommu=pt default_hugepagesz=1G hugepagesz=1G hugepages=4"
  ```
  Reboot after applying kernel parameter changes.

### Building without SPDK

The SPDK crates (`spdk-sys`, `spdk-env`, `spdk-simple-block-device`) are excluded from
the default workspace members, so a plain `cargo build` works without any SPDK dependencies:

```bash
cargo build            # Build default (non-SPDK) workspace members
```

### Building with SPDK

To build the SPDK-dependent crates you must first build and install SPDK:

```bash
# 1. Install system dependencies (requires sudo, RHEL/Fedora)
deps/install_deps.sh

# 2. Install Python build dependencies
pip install -r deps/requirements.txt

# 3. Clone, build, and install SPDK to deps/spdk-build/
deps/build_spdk.sh
```

Then build individual SPDK crates explicitly:

```bash
cargo build -p spdk-sys
cargo build -p spdk-env
cargo build -p spdk-simple-block-device
```

Or build everything (default + SPDK members) at once:

```bash
cargo build --workspace
```

### Running Tests

```bash
# Run tests for default (non-SPDK) crates
cargo test --all

# Run tests for a specific SPDK crate (requires SPDK built)
cargo test -p spdk-sys
cargo test -p spdk-env
cargo test -p spdk-simple-block-device
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
cargo bench --bench channel_latency_benchmark
cargo bench --bench numa_latency_benchmark
cargo bench --bench numa_throughput_benchmark
```

After a run, open the Criterion HTML report:

```bash
firefox --no-remote components/component-framework/crates/component-framework/target/criterion/report/index.html
```

### Running Examples

Component-framework examples:

```bash
cargo run --example basic
cargo run --example wiring
cargo run --example introspection
cargo run --example binding
cargo run --example actor_ping_pong
cargo run --example actor_pipeline
cargo run --example actor_fan_in
cargo run --example actor_log
cargo run --example numa_pinning
```

SPDK block-device examples (require real NVMe hardware bound to `vfio-pci` with hugepages configured):

```bash
# Bind NVMe devices to vfio-pci
sudo deps/spdk/scripts/setup.sh

# Allocate hugepages
sudo sh -c 'echo 1024 > /proc/sys/vm/nr_hugepages'

# Run examples
cargo run -p spdk-simple-block-device --example basic_io     # Component-based I/O
cargo run -p spdk-simple-block-device --example actor_io      # Actor-based I/O
cargo run -p spdk-simple-block-device --example iops_bench    # IOPS benchmark
```

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

After running benchmarks, Criterion generates an HTML report with distribution plots, violin charts, and regression analysis. Open the top-level report index in a browser:

```bash
# Open the Criterion report (after running cargo bench)
firefox --no-remote components/component-framework/crates/component-framework/target/criterion/report/index.html
```

Each benchmark suite has its own sub-report with detailed plots including PDF distribution, iteration times, and comparison against previous runs. Navigate from the index page or access individual reports directly:

```bash
# Example: open the SPSC channel benchmark report
firefox --no-remote ./components/component-framework/crates/component-framework/target/criterion/spsc_throughput_u64/report/index.html
```

If you are using a remote connection in VS Code, you can use the Live Preview extension to open the remote report files.
