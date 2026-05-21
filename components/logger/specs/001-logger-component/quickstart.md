# Quickstart: Logger Component

## Prerequisites

- Rust stable toolchain (MSRV 1.75)
- Linux (RHEL/Fedora)
- Workspace root `Cargo.toml` updated with logger member

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

## Usage: Console Logging (default)

```rust
use logger::LoggerComponentV1;
use interfaces::ILogger;
use component_core::query_interface;

// Create a console logger (reads RUST_LOG for level filtering)
let component = LoggerComponentV1::new();

// Query the ILogger interface
let logger = query_interface!(component, ILogger).unwrap();

// Log at various levels
logger.error("disk failure detected");
logger.warn("high latency on pool 3");
logger.info("server started on port 8080");
logger.debug("cache hit ratio: 0.95");
```

Output (with RUST_LOG=info):
```text
2026-04-17T14:30:00.123Z ERROR disk failure detected
2026-04-17T14:30:00.124Z WARN  high latency on pool 3
2026-04-17T14:30:00.124Z INFO  server started on port 8080
```
(debug message suppressed because debug < info threshold)

## Usage: File Logging

```rust
use logger::LoggerComponentV1;
use interfaces::ILogger;
use component_core::query_interface;

let component = LoggerComponentV1::new_with_file("/tmp/app.log").unwrap();
let logger = query_interface!(component, ILogger).unwrap();

logger.info("logging to file");
// Output written to /tmp/app.log without ANSI color codes
```

## Usage: Component Wiring

```rust
use logger::LoggerComponentV1;

// Create logger component
let logger_comp = LoggerComponentV1::new();

// Bind to another component's ILogger receptacle
consumer_component.connect_receptacle_raw("logger", &*logger_comp).unwrap();
```

## Environment Variables

| Variable | Effect | Default |
|----------|--------|---------|
| RUST_LOG | Set log level: error, warn, info, debug, trace | info |

Level names are case-insensitive. "trace" maps to debug.

## CI Gate

```bash
cargo fmt -p logger --check \
  && cargo clippy -p logger -- -D warnings \
  && cargo test -p logger \
  && cargo doc -p logger --no-deps \
  && cargo bench -p logger --no-run
```
