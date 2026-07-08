# Implementation Plan: Logger

**Branch**: `001-logger` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The Logger component provides structured, colored console and file-based logging for the Certus storage system. It implements the `ILogger` interface (four severity methods: error, warn, info, debug) and integrates with the component framework via `define_component!`. Log level filtering is driven by the `RUST_LOG` environment variable. Output is thread-safe via a Mutex-protected writer, and ANSI color is applied only when stderr is a TTY. The component is fully implemented and tested.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75
**Primary Dependencies**:
- `component-framework` / `component-core` / `component-macros` -- component model macros and traits
- `interfaces` -- shared crate defining `ILogger` via `define_interface!`
- `chrono` 0.4 (clock feature) -- ISO 8601 / RFC 3339 timestamp formatting
- `libc` 0.2 -- POSIX `isatty` for TTY detection on stderr
- `criterion` 0.5 (dev) -- benchmarking framework

## Architecture

### Component Layer

```
+------------------------------------------------------+
|                  Certus Components                    |
|  (extent-manager, block-device-spdk-nvme, etc.)      |
|                                                      |
|  receptacle: ILogger ----+                           |
+------------------------------------------------------+
                           |
                           | connect_receptacle_raw("logger", ...)
                           v
+------------------------------------------------------+
|              LoggerComponent (v0.1.0)                 |
|                                                      |
|  provides: [ILogger, IUnknown]                       |
|  fields:   LoggerState { writer, level, use_color }  |
+------------------------------------------------------+
                           |
              +------------+------------+
              |                         |
              v                         v
   io::stderr() (default)     File (append mode)
   with optional ANSI color   no color
```

### Internal Module Structure

```
components/logger/
  Cargo.toml                    # Crate manifest (workspace deps)
  src/
    lib.rs                      # All production code (single-file crate)
                                #   - LogLevel enum (Error/Warn/Info/Debug)
                                #   - LoggerState struct (writer + level + use_color)
                                #   - define_component!(LoggerComponent)
                                #   - ILogger impl
                                #   - Unit tests (mod tests)
  tests/
    integration.rs              # Integration tests:
                                #   - query_interface round-trip
                                #   - version / provided_interfaces introspection
                                #   - receptacle binding via TestConsumerComponent
                                #   - concurrent logging (4 threads x 100 msgs)
  benches/
    log_throughput.rs           # Criterion benchmarks:
                                #   - log_info (plain write)
                                #   - log_info_colored (ANSI formatting)
                                #   - log_filtered_out (early return path)
                                #   - log_concurrent_4_threads (contention)
  .specify/
    specs/001-logger/
      spec.md                   # Feature specification (backfilled)
      plan.md                   # This file
      tasks.md                  # Improvement tasks
```

### Data Flow

1. **Caller invokes** `logger.info("message")` (or error/warn/debug) via `ILogger` trait.
2. **Level check**: `log()` compares message level against `self.state.level`. If `level > threshold`, early return (no allocation).
3. **Timestamp**: `chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)` produces an ISO 8601 string with `Z` suffix.
4. **Format**: A single `format!` macro call assembles the line. If `use_color` is true, ANSI codes wrap the level tag.
5. **Write**: Acquires `Mutex<Box<dyn Write + Send>>`, calls `write_all` + `flush`. Errors are silently discarded (`let _ = ...`).
6. **Output reaches** stderr or a file, depending on how the component was constructed.

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Single `lib.rs` file | The component is small (~230 lines of production code). A flat module keeps navigation trivial. |
| `Mutex<Box<dyn Write + Send>>` | Guarantees atomic per-line output under concurrent access. Acceptable for operational logging (not data-path hot). |
| Silent write errors (`let _ = ...`) | A logging component must never panic or propagate errors -- it is infrastructure, not business logic. |
| `LogLevel` as `Ord` enum | Numeric ordering (Error=0 < Warn=1 < Info=2 < Debug=3) enables simple `>` comparison for filtering. |
| `RUST_LOG` parsed at construction | Level is fixed for the component's lifetime. No runtime overhead for repeated env lookups. Compatible with `env_logger` conventions. |
| `libc::isatty` for TTY detection | Direct POSIX call avoids pulling in `atty` or `is-terminal` crates. Linux-only platform. |
| `Arc<Self>` constructors | Enables shared ownership across threads and receptacle bindings without additional wrapping. |
| No `trace` level | The four-level model (error/warn/info/debug) is sufficient. `RUST_LOG=trace` maps to Debug for compatibility. |
| `new_with_writer` public API | Enables deterministic testing by injecting a `Vec<u8>` writer. Also useful for custom output targets. |

## Dependencies

| Crate | Type | Purpose |
|-------|------|---------|
| `component-framework` | workspace | `define_component!` macro facade |
| `component-core` | workspace | `IUnknown`, `query_interface!`, receptacle traits |
| `component-macros` | workspace | Proc macros powering `define_component!` / `define_interface!` |
| `interfaces` | workspace | `ILogger` trait definition (shared across all components) |
| `chrono` 0.4 | external | Timestamp formatting (clock feature only, no TZ data) |
| `libc` 0.2 | external | `isatty` syscall wrapper |
| `criterion` 0.5 | dev | Benchmarking harness |

## Testing

| Layer | Coverage |
|-------|----------|
| **Unit tests** (11) | LogLevel ordering, parsing, display; message formatting for all four levels; level filtering (suppression + pass-through); ANSI color presence/absence; file output correctness; file creation; invalid path error. |
| **Integration tests** (7) | `query_interface` round-trip; version string; provided interfaces list; receptacle binding + usage; receptacle metadata introspection; concurrent logging (4 threads x 100 msgs = 400 lines, no interleave). |
| **Doc tests** (4) | Module-level quick start; `LogLevel::from_env_str`; `new_with_file`; `new_with_writer`. |
| **Benchmarks** (4) | Plain info write; colored info write; filtered-out early return; concurrent 4-thread throughput. |

All tests run without hardware dependencies or network access. The `new_with_writer` constructor enables hermetic testing by capturing output to in-memory buffers.

## Future Considerations

- **Structured logging**: Add key-value metadata fields (e.g., component name, request ID) for machine-parseable output.
- **Async writer**: For high-throughput scenarios, a channel-backed async writer could reduce mutex contention.
- **Log rotation**: File output currently appends indefinitely; external rotation (logrotate) is assumed.
- **Dynamic level changes**: Currently fixed at construction; a `set_level` method could allow runtime adjustment without restart.
- **Tracing integration**: Bridge to the `tracing` ecosystem for span-based instrumentation if the project adopts it.
