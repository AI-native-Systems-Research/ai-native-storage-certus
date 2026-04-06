Component Framework — quickstart
================================

This repository contains a small COM-style component framework. The core
actor component is provided by the `component-core` crate; actors own a
dedicated thread and process messages sequentially.

Quickstart — Actor
-------------------

Create a handler by implementing `ActorHandler<M>` for your message type `M`.

```rust
use component_core::actor::{Actor, ActorHandler};
use std::sync::{Arc, Mutex};

struct Accumulator {
    sum: Arc<Mutex<i64>>,
}

impl ActorHandler<i64> for Accumulator {
    fn handle(&mut self, msg: i64) {
        *self.sum.lock().unwrap() += msg;
    }
}

let sum = Arc::new(Mutex::new(0i64));
let actor = Actor::new(
    Accumulator { sum: sum.clone() },
    |panic_info| eprintln!("Actor panicked: {panic_info:?}"),
);

// Spawn the actor thread and get a handle
let handle = actor.activate().unwrap();

// Send messages
for i in 1..=10 {
    handle.send(i).unwrap();
}

// Deactivate and join thread
handle.deactivate().unwrap();

assert_eq!(*sum.lock().unwrap(), 55);
```

Querying `ISender<M>` via `IUnknown`
-----------------------------------

Actors implement `IUnknown` and expose an `ISender<M>` interface so other
components may obtain a sender without holding an `ActorHandle`:

```rust
use component_core::actor::{Actor, ActorHandler};
use component_core::channel::ISender;
use component_core::iunknown::{IUnknown, query};
use std::sync::{Arc, Mutex};

struct Logger { log: Arc<Mutex<Vec<String>>> }
impl ActorHandler<String> for Logger {
    fn handle(&mut self, msg: String) { self.log.lock().unwrap().push(msg); }
}

let actor = Actor::new(Logger { log: Arc::new(Mutex::new(Vec::new())) }, |_| {});

// Query ISender<String> via IUnknown
let sender: Arc<dyn ISender<String> + Send + Sync> =
    query::<dyn ISender<String> + Send + Sync>(&actor).unwrap();

let handle = actor.activate().unwrap();
sender.send("hello".into()).unwrap();
handle.deactivate().unwrap();
```

Examples
--------

See `examples/` for runnable examples such as `actor_ping_pong.rs` and
`actor_log.rs`.

Development
-----------

- Formatting: `cargo fmt`
- Linting: `cargo clippy -- -D warnings`
- Tests: `cargo test --all`

