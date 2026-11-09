# zyre

**Crate**: `zyre`
**Path**: `components/zyre/`
**Version**: 0.1.0

## Description

Safe Rust bindings for the zyre C library. Provides zero-configuration peer discovery and group messaging over local area networks. Wraps the C API with RAII resource management, typed events, and configuration structs. Used by the `remote-lookup` component for cluster peer discovery and signalling.

## Component Definition

```
ZyreComponent {
    version: "0.1.0",
    provides: [IZyre],
    receptacles: {},
}
```

## Interface Definition

```rust
define_interface! {
    pub IZyre {
        fn ping(&self) -> Result<String, ZyreError>;
        fn create_node(&self, config: NodeConfig) -> Result<Box<dyn IZyreNode>, ZyreError>;
    }
}
```

`IZyreNode` is a plain trait (not `define_interface!`), returned as `Box<dyn IZyreNode>`:

| Method | Description |
|--------|-------------|
| `start()` | Start the zyre node (begins discovery) |
| `stop()` | Stop the node |
| `join(group)` | Join a named group |
| `leave(group)` | Leave a group |
| `shout(group, data)` | Broadcast to all group members |
| `whisper(peer, data)` | Send directly to one peer |
| `recv()` | Blocking receive of next event |
| `try_recv()` | Non-blocking receive |
| `uuid()` | This node's PeerId |
| `name()` | This node's name |
| `peers()` | All known peers |
| `peers_by_group(group)` | Peers in a specific group |
| `own_groups()` | Groups this node has joined |
| `peer_groups()` | Groups known from peers |
| `peer_address(peer)` | Network address of a peer |
| `peer_header_value(peer, key)` | Peer's advertised header value |

## Receptacles

None.

## Key Types

- `NodeConfig` — name, port, interface, evasive/expired timeouts, headers, gossip config
- `GossipConfig` — `bind(endpoint)` or `connect(endpoint)` for gossip discovery
- `PeerId(String)` — unique node UUID
- `ZyreEvent` — Enter, Exit, Join, Leave, Shout, Whisper, Stop (with peer/group/data accessors)
- `ZyreError` — `InvalidConfig(String)`, `NodeError(String)`
