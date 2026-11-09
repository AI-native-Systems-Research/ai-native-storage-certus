# Research: Logger Component

**Date**: 2026-04-17

## R1: Timestamp Library

**Decision**: Use `chrono` crate for ISO 8601 timestamp formatting.
**Rationale**: `chrono` is the de facto Rust standard for datetime
handling, supports `Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)`
for compact ISO 8601 with millisecond precision. Lightweight for our use
(only formatting, no parsing needed).
**Alternatives considered**:
- `std::time::SystemTime` + manual formatting — more code, error-prone
  leap second handling, no built-in ISO 8601 formatter.
- `time` crate — viable but `chrono` is more widely used in the Rust
  ecosystem and already battle-tested.

## R2: ANSI Colorization Approach

**Decision**: Hand-rolled ANSI escape codes (no external crate).
**Rationale**: Only 4 colors needed (red=error, yellow=warn, green=info,
cyan=debug). A small helper function with `\x1b[31m` etc. is simpler
than adding a dependency. TTY detection uses `libc::isatty(2)` (stderr fd).
**Alternatives considered**:
- `colored` crate — adds a dependency for trivial use case.
- `termcolor` crate — overkill for 4 static color mappings.
- `ansi_term` crate — unmaintained.

## R3: Thread Safety Strategy

**Decision**: `Mutex<Box<dyn Write + Send>>` wrapping the output writer.
**Rationale**: Ensures atomic line writes without interleaving. The mutex
is held only for the duration of a single `write_all` + `flush` call
(microseconds). For a logging component, contention is negligible.
The writer is type-erased (`Box<dyn Write + Send>`) to support both
stderr and File behind the same interface.
**Alternatives considered**:
- Lock-free ring buffer — complex, not justified for a logger.
- Per-thread writers — loses ordering guarantees.
- `parking_lot::Mutex` — faster under contention but adds a dependency;
  `std::sync::Mutex` is sufficient.

## R4: RUST_LOG Parsing Scope

**Decision**: Global level filtering only (e.g., `RUST_LOG=info`).
Module-level filtering (e.g., `RUST_LOG=mymod=debug`) is out of scope.
**Rationale**: The ILogger interface methods (`error`, `warn`, `info`,
`debug`) accept only a message string — there is no module/target
parameter. Module-level filtering would require a different interface
design. Global level filtering matches the spec and user expectations.
Level names are parsed case-insensitively. "trace" maps to debug since
ILogger has no trace method.
**Alternatives considered**:
- Full env_logger compatibility — would require adding a target/module
  parameter to every ILogger method, breaking the simple interface design.

## R5: Configuration Mechanism

**Decision**: Construction-time configuration. `LoggerComponentV1::new()`
creates a console logger (default). A separate constructor or builder
method creates a file logger. Configuration is immutable after
construction.
**Rationale**: Matches the component-framework pattern where components
are configured at creation and then used via their interface. Runtime
switching between console/file adds complexity with no clear use case.
The `define_component!` macro generates `new()` returning `Arc<Self>` —
we add a `new_with_file(path)` associated function for file mode.
**Alternatives considered**:
- Runtime-switchable output — adds interior mutability complexity for
  the output destination selection on top of the write mutex.
- Builder pattern — overkill for two options (console vs file).

## R6: Log Format

**Decision**: `{timestamp} {LEVEL} {message}\n`
Example: `2026-04-17T14:30:00.123Z INFO server started`
**Rationale**: Compact, machine-parseable, grep-friendly. No brackets
or pipes to reduce visual noise. Level is uppercase and fixed-width
(padded to 5 chars: `ERROR`, `WARN `, `INFO `, `DEBUG`) for alignment.
**Alternatives considered**:
- Bracketed format `[timestamp] [LEVEL] message` — more visual noise.
- Key-value structured format — overkill for a simple logger.
