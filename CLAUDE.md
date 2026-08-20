# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Certus is a generative domain-specific filesystem for inferencing workloads, built on a COM-inspired Rust component framework. Components are developed independently with low coupling and integrated via typed interfaces and receptacles. This design keeps LLM context windows small by scoping work to one component plus its bindings.

**Platform**: Linux only (tested on RHEL/Fedora). Rust stable, edition 2021, MSRV 1.75.

## Build Commands

```bash
# Default build (excludes SPDK-dependent crates)
cargo build

# Full workspace build (requires SPDK pre-built at deps/spdk-build/)
cargo build --workspace

# Build specific SPDK crate
cargo build -p spdk-sys
cargo build -p block-device-spdk-nvme
cargo build -p extent-manager
```

## Test Commands

```bash
cargo test --all                        # All default-member tests (single-threaded in CI)
cargo test -p extent-manager            # SPDK crate tests (run without hardware via mocks)
cargo test -p component-framework       # Framework tests only
cargo test --all -- --test-threads 1    # CI runs single-threaded
```

## Lint and Format

```bash
cargo fmt --check
cargo clippy -- -D warnings     # Warnings are errors
cargo doc --no-deps             # Must be warning-free
```

## Benchmarks

Criterion-based. Run with `cargo bench` or target a specific suite:

```bash
cargo bench --bench channel_spsc_benchmark
cargo bench -p extent-manager --bench create_benchmark
```

## SPDK Setup (for hardware-dependent crates)

```bash
deps/install_deps.sh            # System packages (sudo, RHEL/Fedora)
pip install -r deps/requirements.txt
deps/build_spdk.sh              # Clone, build, install to deps/spdk-build/
```

Requires kernel boot params for IOMMU and hugepages, and `memlock` set to unlimited. See README.md for details.

## Architecture

### Workspace Layout

- **`lib/component-framework/`** — Core framework: `component-core` (traits, actor, channels, NUMA), `component-macros` (proc macros for `define_interface!`/`define_component!`), `component-framework` (facade re-export). Has its own CLAUDE.md.
- **`components/interfaces/`** — Shared interface trait definitions (`IBlockDevice`, `IExtentManager`, `ILogger`, `IGreeter`, `ISPDKEnv`, `IEvictionPolicy`, `IRemoteLookup`). SPDK-dependent interfaces gated behind `features = ["spdk"]`.
- **`components/block-device-spdk-nvme/`** — NVMe block device driver via SPDK userspace. Actor-per-controller model with shared-memory client channels. Has its own CLAUDE.md.
- **`components/extent-manager/`** — Fixed-size extent allocator with crash-consistent on-disk layout (superblock + bitmap + records, 4KiB atomic writes). Has its own CLAUDE.md.
- **`lib/spdk-sys/`** — Raw FFI bindings to SPDK C libraries (bindgen-generated).
- **`components/spdk-env/`** — Safe Rust wrapper around SPDK environment init. Has its own CLAUDE.md.
- **`components/eviction-policy-lru/`** — LRU eviction policy with multi-pool support. Used by memory-tier and dispatch-map for cache eviction decisions.
- **`components/remote-lookup/`** — Placeholder for remote cache lookups to other Certus nodes. Dispatcher forwards local misses here. Has its own CLAUDE.md.
- **`components/example-helloworld/`, `components/console-logger/`** — Example components.
- **`apps/`** — Integrated applications (`helloworld-mainline`, `iops-benchmark`, `extent-benchmark`).
- **`deps/`** — SPDK source and build scripts.
- **`knowledge/`** — Internal wiki (component architecture, SPDK notes).

### Component Model

Components use `define_component!` and `define_interface!` macros. Every component implements `IUnknown` for runtime interface discovery. Dependencies are declared as **receptacles** (typed slots) and wired via first-party or third-party binding (`bind()` by name). Actors run on dedicated OS threads with lock-free channel communication.

### Default vs SPDK Members

The workspace `default-members` excludes SPDK crates (`spdk-sys`, `spdk-env`, `block-device-spdk-nvme`, `extent-manager`, `iops-benchmark`, `extent-benchmark`). A plain `cargo build`/`cargo test --all` works without SPDK. SPDK crates must be built explicitly with `-p`.

## CI

GitHub Actions (`.github/workflows/rust.yml`): builds and tests default members on `ubuntu-latest` with single-threaded test execution.

## Coding Conventions

- `rustfmt` default formatting, `clippy` with `-D warnings`.
- Public APIs require doc comments with runnable examples; `cargo doc --no-deps` must be warning-free.
- Performance-sensitive code must have Criterion benchmarks.
- Unsafe code requires `// SAFETY:` justification comments.
- Component CLAUDE.md files exist in sub-crates — read them when working in those directories.
