# logger

## Summary

The `logger` crate provides a logging component for the Certus storage system. It implements the `ILogger` interface from the shared `interfaces` crate, enabling other components to consume structured logging via dependency-injected receptacles.

Output is configurable: by default, log messages are written to stderr with ANSI color codes (when a TTY is detected). Alternatively, a file output mode writes in append mode without color. Log messages include an ISO 8601 (RFC 3339) timestamp with millisecond precision, a severity level tag, and the message text. The active log level is controlled by the `RUST_LOG` environment variable using the same conventions as `env_logger`.

## Architecture

### ILogger Interface

The `ILogger` interface is defined in the `interfaces` crate and provides four severity methods:

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

### Log Levels

Levels are ordered from most to least severe: `Error`, `Warn`, `Info`, `Debug`. The `RUST_LOG` environment variable selects the threshold; messages below it are suppressed. The string "trace" maps to `Debug`. Unrecognized values default to `Info`.

| `RUST_LOG` value | Threshold |
|------------------|-----------|
| `error` | Error only |
| `warn` / `warning` | Warn and above |
| `info` (default) | Info and above |
| `debug` / `trace` | All messages |

### Output Modes

- **Console (default)** -- Writes to stderr. ANSI color codes are applied when the file descriptor is a TTY (detected via `libc::isatty`). Colors: red for Error, yellow for Warn, green for Info, cyan for Debug.
- **File** -- Opened in append mode; created if it does not exist. ANSI color codes are never emitted. Constructed via `LoggerComponent::new_with_file(path)`.

### Constructors

| Constructor | Description |
|-------------|-------------|
| `LoggerComponent::new_default()` | Console logger (stderr), color auto-detected |
| `LoggerComponent::new_with_file(path)` | File logger (append, no color) |
| `LoggerComponent::new_with_writer(writer, level, use_color)` | Custom writer (for testing) |

### Component Wiring

`LoggerComponent` is built with the `define_component!` macro, provides `ILogger`, and implements `IUnknown` for runtime interface discovery. Other components declare an `ILogger` receptacle and bind to this logger at runtime.

## Build

```bash
cargo build -p logger
```

## Test

Unit, integration, and doc tests:

```bash
cargo test -p logger
```

Lint and documentation checks:

```bash
cargo fmt -p logger --check
cargo clippy -p logger -- -D warnings
cargo doc -p logger --no-deps
```

## Benchmarks

Criterion-based throughput benchmarks using a null writer (no I/O overhead):

```bash
cargo bench -p logger
```

| Benchmark | Description |
|-----------|-------------|
| `log_info` | Single info message, no color |
| `log_info_colored` | Single info message with ANSI color formatting |
| `log_filtered_out` | Cost of a message below the level threshold (early return) |
| `log_concurrent_4_threads` | 4 threads each logging 100 messages concurrently |
