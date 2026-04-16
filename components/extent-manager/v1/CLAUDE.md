# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Extent Manager v1 — a fixed-size storage extent manager for the Certus storage system. Manages extent allocation, metadata persistence, and crash recovery on NVMe SSDs using 4KiB-atomic writes. Built with the component-framework using `define_component!` and `define_interface!` macros.

This is v1, a from-scratch reimplementation of v0 (at `../v0/`). The design spec lives at `info/DESIGN.md`. Use v0 as architectural reference but not as code to copy — v1 should be a clean implementation.

## Build and Test Commands

The crate is excluded from workspace `default-members` because it requires the SPDK feature and linked SPDK native libraries. Always build explicitly with `-p extent-manager`:

```bash
cargo build -p extent-manager
cargo test -p extent-manager                    # All tests (unit + integration + doc)
cargo test -p extent-manager test_name          # Run a single test by name
cargo fmt -p extent-manager --check             # Check formatting
cargo clippy -p extent-manager -- -D warnings   # Lint (warnings are errors)
cargo doc -p extent-manager --no-deps           # Build documentation
cargo bench -p extent-manager                   # Run Criterion benchmarks
cargo bench -p extent-manager --no-run          # Verify benchmarks compile
```

**CI gate** (all must pass before merge):
```bash
cargo fmt -p extent-manager --check \
  && cargo clippy -p extent-manager -- -D warnings \
  && cargo test -p extent-manager \
  && cargo doc -p extent-manager --no-deps \
  && cargo bench -p extent-manager --no-run
```

## Architecture

### Component Wiring

```
[IBlockDevice receptacle] --> ExtentManagerComponentV1 --> [IExtentManager provider]
[ILogger receptacle]      -->                          --> [IExtentManagerAdmin provider]
```

**Lifecycle**: `new_default()` → wire receptacles → `initialize(sizes, slots, ns_id)` (fresh) or `open(ns_id)` (existing with crash recovery) → use `IExtentManager` methods.

### On-Disk Layout (4KiB blocks)

- **Block 0**: Superblock (magic `0x4558544D475256_31` = "EXTMGRV1", format v1, size/slots tables, CRC-32)
- **Blocks 1+**: Bitmap region (one bit per slot per size class, one block per class minimum)
- **After bitmaps**: Extent record region (one 4KiB block per slot, CRC-32 at bytes 4092–4096)

### Crash Consistency (two-phase write)

1. Write extent record block atomically (NVMe 4KiB power-fail guarantee)
2. Flip bitmap bit and persist bitmap block atomically
3. Update in-memory index

Recovery on `open()` scans records vs bitmap bits — records with no bitmap bit are orphans (zeroed); corrupt CRC records are cleared from bitmap and zeroed.

### Key Internal Dependencies

- `component-framework`, `component-core`, `component-macros` — at `../../component-framework/crates/`
- `interfaces` — at `../../interfaces` — provides `IBlockDevice`, `IExtentManager`, `IExtentManagerAdmin`, `ILogger`, `DmaBuffer`, `ExtentManagerError`

### Feature Flags

- `default = ["spdk"]` — enables `interfaces/spdk` for `IBlockDevice`, `DmaBuffer`, SPDK types
- `testing` — exposes `test_support` module for benchmarks and external test harnesses

## Testing Conventions

All tests run without SPDK, hugepages, or NVMe hardware via `MockBlockDevice` (in-memory `HashMap<u64, [u8; 4096]>` with actor threads).

- **MockBlockDevice**: supports `reboot_from()` for restart simulation
- **FaultConfig**: `fail_after_n_writes`, `fail_lba_range`, `fail_all_writes` for crash/fault injection
- **heap_dma_alloc()**: replaces SPDK hugepage allocator in tests (4KiB-aligned heap allocation)
- **create_test_component()**: standard helper returning `(Arc<Component>, Arc<MockBlockDevice>)`

Expected test suites: unit tests in `src/*.rs`, API operation integration tests, crash recovery tests with fault injection, thread safety tests, doc tests, and Criterion benchmarks.

## Constitution (Key Rules)

1. **Correctness First** — Every public API must have unit tests and doc tests. Unsafe code must be justified and tested.
2. **Comprehensive Testing** — Unit, integration, and doc tests are mandatory. `cargo test` must pass with zero failures.
3. **Performance Accountability** — Performance-sensitive APIs must have Criterion benchmarks in `benches/`.
4. **Documentation as Contract** — Every public type/function/method must have doc comments with runnable examples. `cargo doc --no-deps` must be warning-free.
5. **Maintainability** — Minimal public API surface (`pub(crate)` preferred). `cargo fmt` + `cargo clippy` enforced.

**Platform**: Linux only. Rust stable toolchain, edition 2021, MSRV 1.75. No nightly features. No external runtime dependencies beyond the component-framework.

## Speckit Workflow

This project uses speckit v0.4.0 for spec-driven development. Feature artifacts live under `.specify/features/<feature-name>/`. Key slash commands:

- `/speckit-specify` — Create/update feature spec from natural language
- `/speckit-plan` — Generate implementation plan
- `/speckit-tasks` — Generate dependency-ordered tasks
- `/speckit-implement` — Execute implementation plan
- `/speckit-constitution` — Define/update project principles
- `/speckit-analyze` — Cross-artifact consistency check
- `/speckit-drift` — Analyze drift between specs and code
