# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is the `block-device-spdk-nvme` component of the **Certus** project — a generative domain-specific filesystem for inferencing workloads. This component provides a high-performance NVMe block device driver using SPDK for direct userspace NVMe controller access.

Certus uses a component-based architecture inspired by COM, where components are developed independently and later integrated. The parent repo is a Rust workspace rooted at `../../Cargo.toml`.

## Build and Test Commands

```bash
cargo fmt --check              # Check formatting
cargo clippy -- -D warnings    # Lint (warnings are errors)
cargo test --all               # Unit + integration + doc tests
cargo test --doc               # Doc tests only
cargo bench                    # Criterion benchmarks (latency/throughput at varying queue depths)
cargo doc --no-deps            # Build documentation (must be warning-free)
```

**CI gate** (all must pass before merge):
```bash
cargo fmt --check && cargo clippy -- -D warnings && cargo test --all && cargo doc --no-deps && cargo bench --no-run
```

## Architecture

### Actor Model

Each component instance runs an actor with a dedicated service thread pinned to a core in the same NUMA zone as the NVMe controller device. The actor thread polls all attached client channels.

### Client Channels

Each client gets two shared-memory channels:
- **Ingress channel**: client sends command messages to the actor
- **Callback channel**: actor sends asynchronous completion notifications back to the client

Use a fast SPSC channel (e.g., crossbeam bounded to 64 slots) for testing and benchmarking.

### NVMe Controller Binding

Each component instance is associated with a single NVMe controller device, attached and initialized at instantiation. The component is namespace-aware and can probe, create, format, and delete NVMe namespaces.

### Key Interfaces

- **IBlockDevice** (provided interface): creating/connecting channels, device info (capacity, max queue depth, IO queue count, max transfer size, block size, NUMA id, NVMe version), telemetry API
- **ILogger** (receptacle): debug logging via dependency injection; use `LoggerComponent` for testing

### Messaging API

Public messaging covers:
- Sync and async read/write (namespace id, DmaBuffer, LBA offset, timeout)
- Async operations with timeout and abort support; completions via callback channel
- Write zeros, batch operations, controller hardware reset
- Namespace management (probe, create, format, delete)

### DMA Buffers

Clients provide memory for read/write as `DmaBuffer` structs (defined in `spdk_types.rs`). Since clients are in-process, `Arc` references can be passed in messages to avoid copies.

### Telemetry Feature

Compile with `cargo build --features telemetry` to collect min/max/mean IO latencies, total operation count, and mean throughput. The telemetry API returns an error when the feature is not enabled.

### NVMe IO Queue Exploitation

The component exploits different NVMe IO queues with different queue depths to minimize latency for a given batch size.

## Dependencies

- **component-framework** (`../component-framework/`): interface/receptacle/binding definitions
- **spdk-env** (`../spdk-env/`): SPDK environment initialization
- **spdk-sys** (`../spdk-sys/`): SPDK FFI bindings
- **interfaces** (`../interfaces/`): shared interface trait definitions (IBlockDevice, ILogger)

## Constitution (Key Rules)

The project constitution lives at `.specify/memory/constitution.md`. Core mandates:

1. **Correctness First** — Every public API must have unit tests. Unsafe code must be justified (`// SAFETY:` comment) and tested.
2. **Comprehensive Testing** — Unit, integration, and doc tests mandatory. TDD preferred. `cargo test --all` must pass with zero failures.
3. **Performance Accountability** — Performance-sensitive APIs must have Criterion benchmarks in `benches/`. Regressions must be justified or fixed.
4. **Documentation as Contract** — Every public type/function/method must have doc comments with runnable examples. `cargo doc --no-deps` must be warning-free.
5. **Maintainability** — Minimal public API surface. `cargo fmt` + `cargo clippy` enforced. Single-responsibility modules.
6. **Component-Framework Conformance** — Actor model, shared-memory channels, receptacle-based dependencies per `components/component-framework` methodology.

**Platform**: Linux only. Rust stable toolchain (edition 2021, MSRV 1.75+).

## Speckit Workflow

Available slash commands for spec-driven development:

- `/speckit-constitution` — Define/update project principles
- `/speckit-specify` — Create/update feature spec from natural language
- `/speckit-clarify` — Identify underspecified areas in spec
- `/speckit-plan` — Generate implementation plan
- `/speckit-tasks` — Generate dependency-ordered tasks
- `/speckit-implement` — Execute implementation plan
- `/speckit-analyze` — Cross-artifact consistency check
- `/speckit-drift` — Analyze drift between specs and code

## Active Technologies
- Rust stable (edition 2021, MSRV 1.75) + component-framework (actor, channels, NUMA, IUnknown), (001-spdk-nvme-block-device)
- Direct NVMe via SPDK (userspace, zero-copy) (001-spdk-nvme-block-device)
- Rust stable (edition 2021, MSRV 1.75) + block-device-spdk-nvme v2, component-framework, spdk-env, interfaces, example-logger, clap (CLI argument parsing), rand (random LBA generation) (002-iops-benchmark)
- N/A (benchmark tool, no persistent state) (002-iops-benchmark)

## Recent Changes
- 001-spdk-nvme-block-device: Added Rust stable (edition 2021, MSRV 1.75) + component-framework (actor, channels, NUMA, IUnknown),
