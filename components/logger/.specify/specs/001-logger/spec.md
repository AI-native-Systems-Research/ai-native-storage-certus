# Feature Specification: Logger Component

**Feature Branch**: `001-logger`
**Created**: 2026-07-08
**Status**: Backfilled
**Source**: Generated from existing implementation

## Backfill Notice

> This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The Logger component provides structured logging for the Certus storage system. It implements the `ILogger` interface from the shared `interfaces` crate, enabling other components to consume logging via dependency-injected receptacles. Output is configurable: by default, log messages are written to stderr with ANSI color codes (when a TTY is detected); alternatively, a file output mode writes in append mode without color.

Log messages include an ISO 8601 (RFC 3339) timestamp with millisecond precision in UTC, a severity level tag, and the message text. The active log level threshold is controlled by the `RUST_LOG` environment variable using conventions compatible with `env_logger`. Messages below the configured threshold are suppressed with an early return. The component is thread-safe, using a `Mutex`-protected writer to guarantee non-interleaved output under concurrent access.

## User Scenarios & Testing

### User Story 1 - Console Logging (Priority: P1)

As a Certus component developer, I want to emit structured log messages to stderr with automatic color coding, so that I can observe system behavior during development and debugging.

**Acceptance Scenarios**:

- **Given** a `LoggerComponent` created with `new_default()`, **when** `info("system ready")` is called, **then** a line is written to stderr containing an ISO 8601 timestamp, "INFO", and "system ready".
- **Given** a terminal (TTY) attached to stderr, **when** any log method is called, **then** the level tag is wrapped in ANSI color codes (red for Error, orange for Warn, green for Info, cyan for Debug).
- **Given** stderr is not a TTY (e.g., piped), **when** any log method is called, **then** no ANSI escape sequences appear in the output.

### User Story 2 - File Logging (Priority: P1)

As a system operator, I want to direct log output to a file in append mode, so that I can retain a persistent log record without color codes polluting the file.

**Acceptance Scenarios**:

- **Given** `LoggerComponent::new_with_file("/tmp/app.log")` is called, **when** messages are logged, **then** the file is created (if absent) or appended to (if present) with plain-text log lines (no ANSI codes).
- **Given** an invalid file path (e.g., `/nonexistent/dir/file.log`), **when** `new_with_file` is called, **then** an `io::Error` is returned.

### User Story 3 - Log Level Filtering (Priority: P1)

As a developer, I want to control log verbosity via the `RUST_LOG` environment variable, so that I can suppress low-priority messages in production while retaining full detail in development.

**Acceptance Scenarios**:

- **Given** `RUST_LOG=warn`, **when** `info("...")` or `debug("...")` is called, **then** no output is produced.
- **Given** `RUST_LOG=warn`, **when** `warn("...")` or `error("...")` is called, **then** both messages appear in output.
- **Given** `RUST_LOG` is unset, **when** any log method is called, **then** the threshold defaults to `Info` (Info, Warn, Error shown; Debug suppressed).
- **Given** `RUST_LOG=trace`, **when** `debug("...")` is called, **then** the message appears (trace maps to Debug level).

### User Story 4 - Component Integration via Receptacle (Priority: P1)

As a Certus component author, I want to declare an `ILogger` receptacle in my component and bind it to a `LoggerComponent` at runtime, so that I can use dependency-injected logging without coupling to a concrete implementation.

**Acceptance Scenarios**:

- **Given** a consumer component with an `ILogger` receptacle, **when** `connect_receptacle_raw("logger", &*logger_comp)` is called, **then** the binding succeeds and `logger.get()` returns a usable `ILogger` reference.
- **Given** a `LoggerComponent`, **when** `query_interface!(component, ILogger)` is invoked, **then** an `Arc<dyn ILogger + Send + Sync>` is returned.

### User Story 5 - Concurrent Logging (Priority: P2)

As a multi-threaded application, I want to share a single `LoggerComponent` across threads, so that all threads log through one writer without interleaved or corrupted lines.

**Acceptance Scenarios**:

- **Given** 4 threads each logging 100 messages concurrently, **when** all threads complete, **then** exactly 400 complete, non-interleaved lines are present in the output.

## Requirements

### Functional Requirements

- **FR-001**: The component shall implement the `ILogger` interface with four methods: `error`, `warn`, `info`, and `debug`.
- **FR-002**: Each log line shall contain an ISO 8601 / RFC 3339 timestamp with millisecond precision in UTC (trailing `Z`).
- **FR-003**: Each log line shall contain a fixed-width severity tag: `ERROR`, `WARN `, `INFO `, or `DEBUG`.
- **FR-004**: Log level threshold shall be read from the `RUST_LOG` environment variable at construction time. Unrecognized or missing values default to `Info`.
- **FR-005**: The string "trace" shall map to the `Debug` level. The strings "warn" and "warning" shall both map to `Warn`.
- **FR-006**: Messages with severity below the configured threshold shall be suppressed (no output, early return).
- **FR-007**: Default output shall be to stderr.
- **FR-008**: When stderr is a TTY (detected via `libc::isatty`), ANSI color codes shall be applied to the level tag: red (`\x1b[31m`) for Error, orange (`\x1b[38;5;208m`) for Warn, green (`\x1b[32m`) for Info, cyan (`\x1b[36m`) for Debug. A reset code (`\x1b[0m`) shall follow each tag.
- **FR-009**: When stderr is not a TTY, or when output is to a file, no ANSI escape sequences shall be emitted.
- **FR-010**: `new_with_file(path)` shall open the specified path in create+append mode and return `io::Result<Arc<Self>>`. File output never uses color.
- **FR-011**: `new_with_writer(writer, level, use_color)` shall accept an arbitrary `Box<dyn Write + Send>` for testing and custom output targets.

### Non-Functional Requirements

- **NFR-001**: Thread safety -- the writer shall be protected by a `Mutex`, guaranteeing atomic per-line writes and preventing interleaved output under concurrent access.
- **NFR-002**: Performance -- filtered-out messages (below threshold) shall incur minimal overhead (no allocation, no formatting, no I/O). Criterion benchmarks shall verify this.
- **NFR-003**: Compatibility -- log level parsing shall be case-insensitive and accept `env_logger`-compatible level names (error, warn, warning, info, debug, trace).
- **NFR-004**: Platform -- Linux only. Uses `libc::isatty` for TTY detection.
- **NFR-005**: Documentation -- all public types, functions, and methods shall have doc comments with runnable examples. `cargo doc --no-deps` shall produce zero warnings.
- **NFR-006**: The component shall use the `define_component!` macro and expose `IUnknown` for runtime interface discovery (version reporting, provided interfaces listing, receptacle binding).

## Key Entities

| Entity | Description |
|--------|-------------|
| `LoggerComponent` | The component struct, created via `define_component!`. Holds `LoggerState` and provides `ILogger`. |
| `LoggerState` | Internal state: a `Mutex<Box<dyn Write + Send>>` writer, a `LogLevel` threshold, and a `use_color` flag. |
| `LogLevel` | Enum with variants `Error`, `Warn`, `Info`, `Debug` (ordered most-to-least severe). Supports `Ord` for threshold comparison. |
| `ILogger` | Trait interface (defined in `interfaces` crate) with `error`, `warn`, `info`, `debug` methods. |
| `IUnknown` | Base trait providing `query_interface`, `version`, `provided_interfaces`, and `receptacles`. Auto-implemented by `define_component!`. |

## Dependencies

| Dependency | Purpose |
|------------|---------|
| `component-framework` | Provides `define_component!` macro facade |
| `component-core` | Core traits (`IUnknown`, `query_interface!`) |
| `component-macros` | Proc macros for interface/component definition |
| `interfaces` | Shared crate defining the `ILogger` trait |
| `chrono` (0.4, `clock` feature) | ISO 8601 timestamp formatting |
| `libc` (0.2) | `isatty` for TTY detection |
| `criterion` (dev, 0.5) | Benchmarking framework |

## Success Criteria

- **SC-001**: `cargo test -p logger` passes all unit, integration, and doc tests with zero failures.
- **SC-002**: `cargo fmt -p logger --check` reports no formatting issues.
- **SC-003**: `cargo clippy -p logger -- -D warnings` produces no warnings.
- **SC-004**: `cargo doc -p logger --no-deps` builds without warnings.
- **SC-005**: `cargo bench -p logger --no-run` compiles benchmarks successfully.
- **SC-006**: Concurrent logging test (4 threads x 100 messages) produces exactly 400 non-interleaved lines.
- **SC-007**: File output contains no ANSI escape sequences.

## Implementation Notes

- The component uses `Arc<Self>` return types from constructors to support shared ownership across threads and receptacle bindings.
- `LogLevel` implements `PartialOrd`/`Ord` with `Error < Warn < Info < Debug`, so filtering uses `level > self.state.level` to suppress messages less severe than the threshold (higher numeric values are less severe).
- The `Mutex` around the writer ensures atomic writes but introduces contention under high concurrency. For the Certus use case (operational logging, not high-frequency data path), this is acceptable.
- The `new_with_writer` constructor exists primarily for testing (allows capturing output to a `Vec<u8>`) but is public for any custom output scenario.
- Writer errors (`write_all`, `flush`) are silently ignored (using `let _ = ...`) to avoid panicking on I/O failures during logging -- a deliberate design choice for a logging component.
