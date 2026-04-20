# logger

A logging component for the Certus storage system. Provides console and file-based logging with configurable log levels, ANSI colorized output, and timestamped messages. Built with the component framework using `define_component!` and `define_interface!`.

## ILogger Interface

```rust
define_interface! {
    pub ILogger {
        fn error(&self, msg: &str);
        fn warn(&self, msg: &str);
        fn info(&self, msg: &str);
        fn debug(&self, msg: &str);
    }
}
```

Defined in the shared `interfaces` crate.

## Public API

### LoggerComponentV1

- **Provides**: `ILogger`
- **Version**: `"0.1.0"`
- `new_default()` — Console logger (stderr), color when TTY detected
- `new_with_file(path)` — File logger (append mode, no color)
- `new_with_writer(writer, level, use_color)` — Custom writer for testing

### LogLevel

Enum: `Error`, `Warn`, `Info`, `Debug`. Parsed from `RUST_LOG` environment variable (case-insensitive). "trace" maps to Debug. Invalid/missing defaults to Info.

## Build

```bash
cargo build -p logger
```

## Test

```bash
cargo test -p logger
```

### Test Suites

| Location | Coverage |
|----------|----------|
| `src/lib.rs` (unit) | Log level ordering, parsing, display, message formatting, level filtering, ANSI color output, file output, file creation/error |
| `tests/integration.rs` | IUnknown query, version, provided interfaces, receptacle binding, concurrent logging (4 threads x 100 messages) |

## Benchmarks

Criterion-based benchmarks using a null writer:

```bash
cargo bench -p logger
```

| Benchmark | Description |
|-----------|-------------|
| `log_info` | Single info message throughput (no color) |
| `log_info_colored` | Single info message throughput (with ANSI color) |
| `log_filtered_out` | Cost of a filtered-out message (level below threshold) |
| `log_concurrent_4_threads` | 4 threads x 100 messages concurrently |

## Usage

### Console logging (default)

```rust
use logger::LoggerComponentV1;
use interfaces::ILogger;
use component_core::query_interface;

let component = LoggerComponentV1::new_default();
let log = query_interface!(component, ILogger).unwrap();
log.info("server started");
```

### File logging

```rust
let component = LoggerComponentV1::new_with_file("/tmp/app.log").unwrap();
let log = query_interface!(component, ILogger).unwrap();
log.info("logging to file");
```

### Component wiring

```rust
let logger_comp = LoggerComponentV1::new_default();
consumer.connect_receptacle_raw("logger", &*logger_comp).unwrap();
```

## Environment Variables

| Variable | Effect | Default |
|----------|--------|---------|
| `RUST_LOG` | Set log level: error, warn, info, debug, trace | info |

## Log Format

```
2026-04-17T14:30:00.123Z INFO  server started
2026-04-17T14:30:00.124Z ERROR disk failure
```

Console output includes ANSI colors when stderr is a terminal. File output never contains ANSI escape codes.

## Source Layout

```
src/
  lib.rs                LoggerComponentV1 definition, ILogger impl, LogLevel, LoggerState
tests/
  integration.rs        Component framework integration tests
benches/
  log_throughput.rs     Criterion throughput benchmarks
```
