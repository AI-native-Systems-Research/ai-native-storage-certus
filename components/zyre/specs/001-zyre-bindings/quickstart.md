# Quickstart: Zyre Rust Bindings

## Prerequisites

1. Linux (RHEL/Fedora) with cmake, pkg-config, and a C compiler installed
2. Rust stable (>= 1.75)
3. libclang (for bindgen)

## Build Dependencies

From the workspace root:

```bash
# Install system prerequisites (cmake, pkg-config, libtool, etc.)
deps/install_zyre_deps.sh

# Clone and build libzmq, czmq, zyre into deps/zyre-build/
deps/build_zyre.sh
```

This produces:
- `deps/zyre-build/lib/` — shared and static libraries
- `deps/zyre-build/include/` — C headers for bindgen
- `deps/zyre-build/share/pkgconfig/` — pkg-config files

## Build the Crate

```bash
cargo build -p zyre
```

The build script (`build.rs`) automatically:
1. Finds headers in `deps/zyre-build/include/`
2. Generates FFI bindings via bindgen
3. Links against libraries in `deps/zyre-build/lib/`

## Run Tests

```bash
cargo test -p zyre
```

Integration tests start two nodes on localhost and verify discovery + messaging.

## Basic Usage

Nodes are created through the `IZyre` factory; operations live on the returned
`Box<dyn IZyreNode>` handle.

```rust
use component_core::query_interface;
use zyre::{IZyre, NodeConfig, ZyreComponent, ZyreEvent};

// Obtain the IZyre factory from the component
let comp = ZyreComponent::new();
let izyre = query_interface!(comp, IZyre).unwrap();

// Configure and create a node
let mut config = NodeConfig::default();
config.name = Some("my-node".into());
config.headers.insert("service".into(), "storage".into());

let mut node = izyre.create_node(config).unwrap();
node.start().unwrap();
node.join("my-group").unwrap();

// Send a message (single frame, bounded only by memory — serialize any
// structure you need into the byte payload)
node.shout("my-group", b"hello world").unwrap();

// Receive (blocking)
let event = node.recv().unwrap();

// Non-blocking poll
if let Some(event) = node.try_recv().unwrap() {
    // handle event
}

// Peer introspection
let peers = node.peers();
let groups = node.own_groups();

// Node is stopped and freed when dropped
drop(node);
```

## Component Interface (health check)

```rust
use interfaces::IZyre;
use component_core::query_interface;
use zyre::ZyreComponent;
use std::sync::Arc;

// The IZyre interface also provides a health-check ping
let comp = ZyreComponent::new();
let iface: Arc<dyn IZyre + Send + Sync> = query_interface!(comp, IZyre).unwrap();
assert_eq!(iface.ping().unwrap(), "pong");
```

## Gossip Discovery

For networks without UDP broadcast:

```rust
use zyre::{GossipConfig, NodeConfig};

// Server node (binds the gossip endpoint)
let mut server_config = NodeConfig::default();
server_config.name = Some("gossip-server".into());
server_config.endpoint = Some("tcp://server-host:9998".into());
server_config.gossip = Some(GossipConfig::bind("tcp://*:9999"));

// Client node (connects to the gossip server)
let mut client_config = NodeConfig::default();
client_config.name = Some("gossip-client".into());
client_config.endpoint = Some("tcp://client-host:9998".into());
client_config.gossip = Some(GossipConfig::connect("tcp://server-host:9999"));
```
