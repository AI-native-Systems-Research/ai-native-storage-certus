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

```rust
use zyre::{NodeConfig, ZyreNode, ZyreEvent};

// Create and start a node
let config = NodeConfig::builder()
    .name("my-node")
    .header("service", "storage")
    .build();

let mut node = ZyreNode::new(config).unwrap();
node.start().unwrap();
node.join("my-group").unwrap();

// Send a message
node.shout("my-group", b"hello world").unwrap();

// Multi-frame message
node.shout_multi("my-group", &[b"header", b"payload"]).unwrap();

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

// The IZyre interface provides a health-check ping
let comp = ZyreComponent::new_default();
let iface: Arc<dyn IZyre + Send + Sync> = query_interface!(comp, IZyre).unwrap();
assert_eq!(iface.ping().unwrap(), "pong");
```

## Gossip Discovery

For networks without UDP broadcast:

```rust
use zyre::{NodeConfig, GossipConfig};

// Server node (binds the gossip endpoint)
let server_config = NodeConfig::builder()
    .name("gossip-server")
    .gossip(GossipConfig::bind("tcp://*:9999"))
    .build();

// Client node (connects to the gossip server)
let client_config = NodeConfig::builder()
    .name("gossip-client")
    .gossip(GossipConfig::connect("tcp://server-host:9999"))
    .build();
```
