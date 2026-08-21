# component-framework

**Crate**: `component-framework`
**Path**: `lib/component-framework/crates/component-framework/`
**Version**: 0.1.0

## Description

Facade crate that re-exports everything from `component-core` and `component-macros` as a single dependency. Provides a `prelude` module and backwards-compatible `declare_interface!` / `declare_component!` macro aliases.

Components that depend on the framework typically add `component-framework` (or `component-core` directly) to their `Cargo.toml`.

## Benchmarks

Includes Criterion benchmarks for:
- `query_interface` dispatch
- `receptacle` connect/get
- `method_dispatch`
- `registry` operations
- `binding` overhead
- `component_ref` operations
- Channel throughput (SPSC, MPSC, crossbeam, kanal)
- Actor latency
- NUMA latency and throughput

## Interfaces Provided

None (facade crate).

## Receptacles

None.
