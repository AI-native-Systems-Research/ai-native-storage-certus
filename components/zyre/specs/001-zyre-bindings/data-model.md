# Data Model: Zyre Rust Bindings

**Date**: 2026-07-01  
**Feature**: 001-zyre-bindings

> **Type location**: The value types below (`ZyreEvent`, `NodeConfig`, `GossipConfig`, `PeerId`, `ZyreError`) and the `IZyre`/`IZyreNode` traits are defined in the **`interfaces`** crate, so `IZyre::create_node` can name them without a crate cycle. The `zyre` crate re-exports them. Only the concrete `ZyreNode` (the FFI wrapper) lives in the `zyre` crate, and it is crate-private.

## Entities

### IZyreNode (handle) / ZyreNode (implementation)

`IZyreNode` is the public handle trait for a single zyre peer, returned by
`IZyre::create_node` as `Box<dyn IZyreNode>`. `ZyreNode` is its sole concrete
implementation (crate-private, FFI-owning).

| Field | Type | Description |
|-------|------|-------------|
| ptr | `*mut zyre_t` | Owned raw pointer to the C node (internal) |
| state | `Cell<State>` | Lifecycle: `Created → Running → Draining → Done` (interior mutability so `&self` recv can advance to `Done`) |

**Trait shape**: `pub trait IZyreNode: Send` — plain trait, not a
`define_interface!` component interface. `Send` but not `Sync` (matches the C
API: a node may be moved between threads but not shared). `start/stop/join/leave`
take `&mut self`; send/recv/introspection take `&self`.

**Lifecycle**: Created → Configured → Started → Running → Stopped (via drop)

**Ownership**: Exclusive. Not Clone, not Copy.

**Drop behavior**: Calls `zyre_stop()` then `zyre_destroy()`.

---

### ZyreEvent

A typed enum representing an incoming network event.

```text
enum ZyreEvent {
    Enter { peer: PeerId, name: String, headers: HashMap<String, String>, address: String },
    Exit { peer: PeerId, name: String },
    Evasive { peer: PeerId, name: String },
    Silent { peer: PeerId, name: String },
    Join { peer: PeerId, name: String, group: String },
    Leave { peer: PeerId, name: String, group: String },
    Whisper { peer: PeerId, name: String, message: Vec<u8> },
    Shout { peer: PeerId, name: String, group: String, message: Vec<u8> },
    Stop,
}
```

**Invariants**:
- Every variant except `Stop` carries a `PeerId` and peer name.
- `message` in Whisper/Shout is the payload of the (single) message frame. Payload size is bounded only by memory; there is no multi-frame representation.
- `headers` in Enter is a snapshot — headers do not update after initial discovery.
- `Stop` is the terminal event: it is delivered exactly once after `stop()` (the zyre actor enqueues a `["STOP", own-uuid, own-name]` sentinel on the inbox), and no events follow it.

**Accessors**: `ZyreEvent` provides convenience methods `peer() -> Option<&PeerId>`, `peer_name() -> Option<&str>`, and `group() -> Option<&str>` so callers can read common fields without matching every variant (all return `None` for `Stop`; `group()` returns `None` for non-group events).

---

### NodeConfig

Configuration passed to `IZyre::create_node`. A `#[non_exhaustive]` struct with
public fields and a `Default` impl (no builder). Construct via
`let mut c = NodeConfig::default(); c.name = Some(...);`. `#[non_exhaustive]`
lets future fields be added without breaking callers.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| name | `Option<String>` | None (auto-generated) | Human-readable node name |
| headers | `HashMap<String, String>` | empty | Metadata shared during discovery |
| port | `Option<u16>` | None (5670) | UDP beacon port |
| interface | `Option<String>` | None (all interfaces) | Network interface for beaconing |
| evasive_timeout_ms | `u32` | 5000 | Time before peer marked evasive |
| expired_timeout_ms | `u32` | 30000 | Time before peer marked expired |
| beacon_interval_ms | `u32` | 1000 | Beacon broadcast interval |
| endpoint | `Option<String>` | None | Node's own data endpoint (required with gossip) |
| gossip | `Option<GossipConfig>` | None | If set, use gossip instead of beacon |

**Validation** (`NodeConfig::validate`, invoked by `create_node`):
- `evasive_timeout_ms` must be > 0
- `expired_timeout_ms` must be > `evasive_timeout_ms`
- `port` if Some, must be > 0
- `name` if Some, must be non-empty
- if `gossip` is set, `endpoint` must be set and the gossip config must have at least one of `bind`/`connect`

---

### GossipConfig

Configuration for gossip-based discovery (alternative to UDP beacon).

| Field | Type | Description |
|-------|------|-------------|
| bind | `Option<String>` | Endpoint to bind as gossip server (e.g., "tcp://*:9999") |
| connect | `Vec<String>` | Endpoints to connect to as gossip client |

**Invariant**: At least one of `bind` or `connect` must be provided.

---

### PeerId

Newtype wrapping a UUID string identifying a remote peer.

| Field | Type | Description |
|-------|------|-------------|
| 0 | `String` | UUID in standard format (32 hex chars, no dashes) |

**Traits**: Clone, Debug, Display, PartialEq, Eq, Hash

---

## State Transitions

### ZyreNode Lifecycle

```text
[Created] --start()--> [Running] --stop()--> [Draining] --recv() yields Stop--> [Done]
    |                      |                       |
    |                      +--recv()/try_recv()    +--recv()/try_recv() drain the
    |                      +--shout()/whisper()       queued events, then the
    |                      +--join()/leave()          terminal Stop sentinel
    |
    +-- recv/send/join/leave before start --> Error::NotStarted

In [Draining]/[Done]: send/join/leave --> Error::Stopped.
After [Done]: recv() --> Error::Stopped; try_recv() --> Ok(None) (no further
zyre_recv is issued — the actor has exited).
Drop stops a Running node and destroys it regardless of drain progress.

stop() outside [Running] (before start(), or a repeated call while already
[Draining]/[Done]) is a silent no-op: it does not error and does not change
state. Only the [Running] --stop()--> [Draining] edge shown above issues
zyre_stop(); calling stop() is otherwise idempotent.
```

### Peer States (as observed by a running node)

```text
[Unknown] --ENTER--> [Active] --EVASIVE--> [Evasive] --SILENT--> [Silent] --EXIT--> [Gone]
                         |                                                      ^
                         +--EXIT (graceful)-------------------------------------+
```

## Relationships

```text
ZyreComponent (IZyre)
    └── create_node(NodeConfig) → Box<dyn IZyreNode>  (concrete: ZyreNode)
                      ├── receives → ZyreEvent
                      ├── identifies peers via → PeerId
                      └── configured by → NodeConfig / GossipConfig
```
