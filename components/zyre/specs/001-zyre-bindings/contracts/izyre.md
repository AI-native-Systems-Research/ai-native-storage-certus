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

        /// Create a new, un-started node from `config`.
        fn create_node(&self, config: NodeConfig) -> Result<Box<dyn IZyreNode>, ZyreError>;
    }
}
```

`IZyre` is a **factory**. All node operations live on the returned
[`IZyreNode`] handle; the concrete `ZyreNode` and its constructor are
crate-private in the `zyre` crate, so `create_node` is the only way to obtain a
node — outside callers cannot bypass the interface.

The value types the factory and handle exchange (`NodeConfig`, `GossipConfig`,
`ZyreEvent`, `PeerId`, `ZyreError`) and the `IZyre`/`IZyreNode` traits are all
defined in the `interfaces` crate so `IZyre` can name them without creating a
crate cycle with `zyre`. The `zyre` crate re-exports them for convenience.

## Semantics

### `ping() -> Result<String, ZyreError>`

**Preconditions**: None

**Postconditions**: Returns `Ok("pong")` when the subsystem is healthy

**Thread Safety**: `IZyre` is `Send + Sync` — safe to call from any thread.

### `create_node(config) -> Result<Box<dyn IZyreNode>, ZyreError>`

**Preconditions**:
- `config` passes validation (see `NodeConfig` validation rules in data-model.md)

**Postconditions**:
- Returns an un-started node ready for `start()`
- The node has a unique UUID assigned
- No network activity occurs until `start()` is called
- The returned node is independent — multiple nodes can coexist

**Errors**:
- `ZyreError::InvalidConfig(reason)` — config validation failed
- `ZyreError::CreateFailed` — underlying `zyre_new()` returned null (resource exhaustion)

**Thread Safety**:
- `IZyre` is `Send + Sync`; `create_node` may be called from any thread.
- The returned `Box<dyn IZyreNode>` is `Send` but NOT `Sync` — it must be used from one thread at a time (it may be moved between threads).

## IZyreNode Handle

`IZyreNode` is a plain `Send` trait (not a `define_interface!` component
interface): a component interface is `Send + Sync` with `&self`-only methods,
which would force a lock around this inherently single-threaded, `&mut self` C
resource. As a returned handle it needs no runtime interface discovery.

```rust
pub trait IZyreNode: Send {
    fn start(&mut self) -> Result<(), ZyreError>;
    fn stop(&mut self);
    fn join(&mut self, group: &str) -> Result<(), ZyreError>;
    fn leave(&mut self, group: &str) -> Result<(), ZyreError>;
    fn shout(&self, group: &str, data: &[u8]) -> Result<(), ZyreError>;
    fn whisper(&self, peer: &PeerId, data: &[u8]) -> Result<(), ZyreError>;
    fn recv(&self) -> Result<ZyreEvent, ZyreError>;
    fn try_recv(&self) -> Result<Option<ZyreEvent>, ZyreError>;
    fn uuid(&self) -> PeerId;
    fn name(&self) -> String;
    fn peers(&self) -> Vec<PeerId>;
    fn peers_by_group(&self, group: &str) -> Vec<PeerId>;
    fn own_groups(&self) -> Vec<String>;
    fn peer_groups(&self) -> Vec<String>;
    fn peer_address(&self, peer: &PeerId) -> Option<String>;
    fn peer_header_value(&self, peer: &PeerId, key: &str) -> Option<String>;
}
```

The bindings add no threads of their own; the zyre C library runs its own
discovery/beacon threads internally. The caller drives event reception by
calling `recv()` / `try_recv()`.

**Receive lifecycle**: `recv`/`try_recv` return `ZyreError::NotStarted` before
`start()`. After `stop()` (or a drop-triggered stop) the node keeps delivering
any queued events followed by a single terminal `ZyreEvent::Stop` sentinel;
once that sentinel is consumed, `recv` returns `ZyreError::Stopped` and
`try_recv` returns `Ok(None)` (it never blocks on the now-producerless inbox).
`recv` returns `ZyreError::RecvFailed` if the calling thread is interrupted.
Because the handle is `Send` but not `Sync`, this drain is single-threaded: a
node that is stopped must be stopped by whichever thread owns it — there is no
separate stop handle for signalling a node parked in another thread's `recv`.

## Consumer Pattern

```rust
use component_core::query_interface;
use zyre::{IZyre, NodeConfig, ZyreComponent, ZyreEvent};

// Obtain the factory and create a node
let comp = ZyreComponent::new();
let izyre = query_interface!(comp, IZyre).unwrap();

let mut config = NodeConfig::default();
config.name = Some("my-certus-node".into());
config.headers.insert("role".into(), "storage".into());

let mut node = izyre.create_node(config)?;
node.start()?;
node.join("certus-cluster")?;

// Send a message to the group
node.shout("certus-cluster", b"hello from node")?;

// Process events until this node decides to leave, then stop and drain:
// stop() enqueues a terminal Stop sentinel, so the same loop exits cleanly
// once it is observed (any events already queued arrive before it).
let mut running = true;
while running {
    match node.recv()? {
        ZyreEvent::Shout { peer, group, .. } => {
            println!("Got message from {peer} in {group}");
            node.stop(); // begin shutdown; keep draining until Stop
        }
        ZyreEvent::Exit { peer, .. } => {
            println!("Peer {peer} left");
        }
        ZyreEvent::Stop => running = false, // terminal end-of-stream sentinel
        _ => {}
    }
}
// recv() would now return Err(ZyreError::Stopped); the node is cleaned up on drop.
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
