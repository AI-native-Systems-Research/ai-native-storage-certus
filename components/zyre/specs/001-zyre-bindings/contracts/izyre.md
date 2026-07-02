# IZyre Interface Contract

**Version**: 0.1.0  
**Component**: ZyreComponent  
**Defined in**: `components/interfaces/src/izyre.rs`

## Interface Definition

```rust
define_interface! {
    pub IZyre {
        /// Check if the zyre subsystem is available and healthy.
        fn ping(&self) -> Result<String, ZyreError>;
    }
}
```

## Semantics

### `ping() -> Result<String, ZyreError>`

**Preconditions**:
- None

**Postconditions**:
- Returns `Ok("pong")` when the subsystem is healthy

**Errors**:
- None currently defined (reserved for future health-check failures)

**Thread Safety**:
- `IZyre` (the component) is `Send + Sync` — safe to call `ping()` from any thread

## Node Creation

Node creation is performed directly via the `zyre` crate (not through `IZyre`) to avoid a circular dependency between `interfaces` and `zyre`:

```rust
use zyre::{NodeConfig, ZyreNode};

let config = NodeConfig::builder()
    .name("my-certus-node")
    .header("role", "storage")
    .header("version", "1.0")
    .evasive_timeout_ms(3000)
    .build();

let mut node = ZyreNode::new(config)?;
```

**Preconditions**:
- `config` passes validation (see NodeConfig validation rules in data-model.md)

**Postconditions**:
- Returns an un-started `ZyreNode` ready for `start()`
- The node has a unique UUID assigned
- No network activity occurs until `start()` is called
- The returned node is independent — multiple nodes can coexist

**Errors**:
- `ZyreError::InvalidConfig(reason)` — config validation failed
- `ZyreError::CreateFailed` — underlying `zyre_new()` returned null (resource exhaustion)

**Thread Safety**:
- The returned `ZyreNode` is `Send` but NOT `Sync` — must be used from one thread at a time

## Consumer Pattern

```rust
use zyre::{NodeConfig, ZyreNode, ZyreEvent};

// Build configuration
let config = NodeConfig::builder()
    .name("my-certus-node")
    .header("role", "storage")
    .header("version", "1.0")
    .evasive_timeout_ms(3000)
    .build();

// Create and start a node
let mut node = ZyreNode::new(config)?;
node.start()?;
node.join("certus-cluster")?;

// Send a message to the group
node.shout("certus-cluster", b"hello from node")?;

// Receive events
loop {
    match node.recv()? {
        ZyreEvent::Shout { peer, group, message, .. } => {
            println!("Got message from {peer} in {group}");
        }
        ZyreEvent::Exit { peer, .. } => {
            println!("Peer {peer} left");
        }
        ZyreEvent::Stop => break,
        _ => {}
    }
}
// Node automatically stopped and cleaned up when dropped
```

## Health Check Pattern

```rust
use interfaces::IZyre;
use component_core::query_interface;
use std::sync::Arc;

let iface: Arc<dyn IZyre + Send + Sync> = query_interface!(component, IZyre).unwrap();
assert_eq!(iface.ping().unwrap(), "pong");
```

## Versioning

- This is version 0.1.0 of the interface
- Breaking changes require a new interface version per Certus component model conventions
