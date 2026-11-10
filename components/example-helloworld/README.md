# example-helloworld

## Summary

A demo component illustrating core Certus component framework patterns. It defines an `IGreeter` interface via `define_interface!`, implements it in `HelloWorldComponent` via `define_component!`, and provides a `GreeterHandler` actor that processes `GreetRequest` messages on a dedicated thread. An optional `ILogger` receptacle demonstrates component wiring for structured logging.

## Architecture

The component uses the actor model for message-driven concurrency:

- `HelloWorldComponent` provides the `IGreeter` interface and declares an `ILogger` receptacle.
- `GreeterHandler` implements `ActorHandler<GreetRequest>`, running on its own thread. It prints greetings and optionally logs via the wired `ILogger`.
- Messages are sent to the actor through a channel handle obtained from `Actor::simple(...).activate()`.

See `apps/helloworld-mainline/` for a full application that wires up and drives this component.

## Build

```bash
cargo build -p example-helloworld
```

## Test

```bash
cargo test -p example-helloworld
```
