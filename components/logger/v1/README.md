# logger

A simple `ILogger` implementation for the Certus storage system. Provides console and file-based logging with configurable log levels, ANSI colorized output, and timestamped messages. Built with the component framework using `define_component!` and `define_interface!`.

## Summary

`LoggerComponentV1` is a logging component that other Certus components consume via the `ILogger` interface (declared as a receptacle). It supports:

- Console output (stderr) with ANSI color when a TTY is detected
- File output (append mode, no color)
- Log level filtering via `RUST_LOG` environment variable (error, warn, info, debug, trace)
- Timestamped messages in ISO 8601 format

### ILogger Interface

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

### Constructors

- `new_default()` -- Console logger (stderr), color when TTY detected
- `new_with_file(path)` -- File logger (append mode, no color)
- `new_with_writer(writer, level, use_color)` -- Custom writer for testing

### Environment Variables

| Variable | Effect | Default |
|----------|--------|---------|
| `RUST_LOG` | Set log level: error, warn, info, debug, trace | info |

## Structure

```
src/
  lib.rs                LoggerComponentV1 definition, ILogger impl, LogLevel, LoggerState
tests/
  integration.rs        Component framework integration tests (IUnknown, receptacles, concurrency)
benches/
  log_throughput.rs     Criterion throughput benchmarks
```

## Build & Test

### Build

```bash
cargo build -p logger
```

### Test

```bash
cargo test -p logger
```

### Lint

```bash
cargo fmt -p logger --check
cargo clippy -p logger -- -D warnings
```

### Benchmarks

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
