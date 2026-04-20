# example-helloworld

A reference component demonstrating the Certus component framework. Shows how to define an interface, implement a component, and use the actor model for message-driven concurrency.

## What It Demonstrates

- **Interface definition** with `define_interface!` — the `IGreeter` trait
- **Component definition** with `define_component!` — `HelloWorldComponent` providing `IGreeter`
- **Receptacle wiring** — optional `ILogger` receptacle for structured logging
- **Actor model** — `GreeterHandler` processes `GreetRequest` messages on a dedicated thread, counting greetings and printing them to stdout

## Public API

### IGreeter Interface

```rust
define_interface! {
    pub IGreeter {
        fn greeting_prefix(&self) -> &str;
    }
}
```

### HelloWorldComponent

- Provides: `IGreeter`
- Receptacles: `logger` (`ILogger`, optional)
- Version: `"0.1.0"`
- `greeting_prefix()` returns `"Hello"`

### GreeterHandler (Actor)

Handles `GreetRequest { name: String }` messages. Each message prints a greeting to stdout with a running count.

- `GreeterHandler::new()` — create without logging
- `GreeterHandler::with_logger(logger)` — create with an `Arc<dyn ILogger + Send + Sync>` for structured logging

## Build

```bash
cargo build -p example-helloworld
```

## Test

```bash
cargo test -p example-helloworld
```

To see `println!`/`eprintln!` output and log messages during tests:

```bash
RUST_LOG=debug cargo test -p example-helloworld -- --nocapture
```

## Usage

See `apps/helloworld-mainline/` for a full example that instantiates this component, queries its interface, wires up the actor, and sends messages.

## Source Layout

```
src/
  lib.rs    Component definition, IGreeter impl, GreeterHandler actor
```
