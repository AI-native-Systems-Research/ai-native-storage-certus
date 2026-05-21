# example-helloworld

A demo component showing the Certus component framework patterns. Demonstrates interface definition, component implementation, receptacle wiring, and the actor model for message-driven concurrency.

## Summary

This component serves as a reference implementation for new Certus components. It provides the `IGreeter` interface and includes a `GreeterHandler` actor that processes greeting messages on a dedicated thread.

### What It Demonstrates

- **Interface definition** with `define_interface!` -- the `IGreeter` trait
- **Component definition** with `define_component!` -- `HelloWorldComponent` providing `IGreeter`
- **Receptacle wiring** -- optional `ILogger` receptacle for structured logging
- **Actor model** -- `GreeterHandler` processes `GreetRequest` messages on a dedicated thread

### Public API

- `HelloWorldComponent` -- provides `IGreeter`, version `"0.1.0"`, receptacle: `logger` (ILogger, optional)
- `IGreeter::greeting_prefix()` -- returns `"Hello"`
- `GreeterHandler::new()` -- create actor handler without logging
- `GreeterHandler::with_logger(logger)` -- create actor handler with an `ILogger`
- `GreetRequest { name: String }` -- message type for the actor

## Structure

```
src/
  lib.rs    Component definition, IGreeter impl, GreeterHandler actor, GreetRequest message
```

## Build & Test

### Build

```bash
cargo build -p example-helloworld
```

### Test

```bash
cargo test -p example-helloworld
```

To see output during tests:

```bash
RUST_LOG=debug cargo test -p example-helloworld -- --nocapture
```

### Usage Example

See `apps/helloworld-mainline/` for a full application that instantiates this component, queries its interface, wires up the actor, and sends messages.
