# Quickstart: component-framework

## Prerequisites

- Rust stable toolchain (1.75+)
- Linux (x86_64)
- `cargo` (included with Rust)

## Build

```bash
# Clone and enter the workspace
cd component-framework

# Build all crates
cargo build --workspace

# Run all tests (unit + integration + doc tests)
cargo test --all

# Run benchmarks
cargo bench

# Build documentation
cargo doc --no-deps --open
```

## Define an Interface

Create an interface that can be shared across crates without exposing
implementations:

```rust
use component_framework::define_interface;

define_interface! {
    ILogger {
        fn log(&self, level: u8, message: &str);
        fn flush(&self);
    }
}
```

This generates a `ILogger` trait with `Send + Sync + 'static` bounds
and associated `InterfaceInfo` metadata.

## Define a Component

Create a component that provides `ILogger` and is introspectable via
`IUnknown`:

```rust
use component_framework::{define_component, define_interface};

define_interface! {
    ILogger {
        fn log(&self, level: u8, message: &str);
        fn flush(&self);
    }
}

define_component! {
    ConsoleLogger {
        version: "1.0.0",
        provides: [ILogger],
    }
}

impl ILogger for ConsoleLogger {
    fn log(&self, level: u8, message: &str) {
        println!("[{level}] {message}");
    }
    fn flush(&self) {}
}
```

## Query Interfaces

Use `IUnknown` to discover what a component provides:

```rust
use component_framework::{IUnknown, query};
use std::sync::Arc;

let logger = Arc::new(ConsoleLogger::new());

// Query for ILogger
let ilogger: Arc<dyn ILogger + Send + Sync> =
    query::<dyn ILogger + Send + Sync>(&*logger)
    .expect("ConsoleLogger provides ILogger");

ilogger.log(1, "Hello from queried interface");

// Enumerate all provided interfaces
for info in logger.provided_interfaces() {
    println!("Provides: {}", info.name);
}
// Output: Provides: ILogger
//         Provides: IUnknown
```

## Wire Receptacles

Connect a component's required interface to a provider:

```rust
use component_framework::{define_component, Receptacle};

define_interface! {
    IStorage {
        fn read(&self, key: &str) -> Option<Vec<u8>>;
    }
}

define_component! {
    CachingProxy {
        version: "1.0.0",
        provides: [],
        receptacles: {
            backend: IStorage,
        },
    }
}

// Create components
let storage = Arc::new(MyStorageImpl::new());
let proxy = Arc::new(CachingProxy::new());

// Wire the receptacle
proxy.backend.connect(
    query::<dyn IStorage + Send + Sync>(&*storage).unwrap()
).expect("connect succeeds");

// Use the connected receptacle
let data = proxy.backend.get()
    .expect("connected")
    .read("key");

// Disconnect
proxy.backend.disconnect().expect("was connected");
```

## CI Validation

Run the full CI gate locally:

```bash
cargo fmt --check \
  && cargo clippy -- -D warnings \
  && cargo test --all \
  && cargo doc --no-deps
```
