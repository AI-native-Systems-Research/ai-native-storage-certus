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

## Benchmarks

```bash
cargo bench -p logger
```

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
