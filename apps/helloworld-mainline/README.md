# helloworld-mainline

Minimal example application demonstrating the Certus component framework end-to-end. Instantiates a component, queries its interface, wires up an actor, and sends messages.

## What It Does

1. Creates a `HelloWorldComponent` instance
2. Queries the `IGreeter` interface via `IUnknown`
3. Creates a `GreeterHandler` actor (runs on a dedicated thread)
4. Sends four greeting messages: "World", "Rust", "Certus", "Actors"
5. Deactivates the actor and exits

## Build

```bash
cargo build -p helloworld-mainline
```

## Run

```bash
cargo run -p helloworld-mainline
```

## Test

```bash
cargo test -p helloworld-mainline
```
