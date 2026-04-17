# example-helloworld

A reference component demonstrating the Certus component framework. Shows how to define an interface, implement a component, and use the actor model for message-driven concurrency.

## What It Demonstrates

- **Interface definition** with `define_interface!` — the `IGreeter` trait
- **Component definition** with `define_component!` — `HelloWorldComponent` providing `IGreeter`
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
- Version: `"0.1.0"`
- `greeting_prefix()` returns `"Hello"`

### GreeterHandler (Actor)

Handles `GreetRequest { name: String }` messages. Each message prints a greeting to stdout with a running count. Logs actor start/stop to stderr.

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

`RUST_LOG` controls the logger's level threshold (`error`, `warn`, `info`, `debug`). `--nocapture` prevents the test harness from swallowing stdout/stderr.

## Usage

See `apps/helloworld-mainline/` for a full example that instantiates this component, queries its interface, wires up the actor, and sends messages.
