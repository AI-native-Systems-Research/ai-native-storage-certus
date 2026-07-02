# Data Model: Zyre Rust Bindings

**Date**: 2026-07-01  
**Feature**: 001-zyre-bindings

## Entities

### ZyreNode

The primary handle representing a single zyre peer on the network.

| Field | Type | Description |
|-------|------|-------------|
| ptr | `*mut zyre_t` | Owned raw pointer to the C node (internal) |
| started | `bool` | Whether `start()` has been called |
| uuid | `String` | Node's UUID (set after creation) |

**Lifecycle**: Created → Configured → Started → Running → Stopped (via drop)

**Ownership**: Exclusive. Not Clone, not Copy. Implements `Send` (can be moved between threads). Does NOT implement `Sync`.

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
- `message` in Whisper/Shout is the first frame payload (single-frame API). Multi-frame variant carries `Vec<Vec<u8>>`.
- `headers` in Enter is a snapshot — headers do not update after initial discovery.

---

### NodeConfig

Configuration for constructing ZyreNode instances.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| name | `Option<String>` | None (auto-generated) | Human-readable node name |
| headers | `HashMap<String, String>` | empty | Metadata shared during discovery |
| port | `Option<u16>` | None (5670) | UDP beacon port |
| interface | `Option<String>` | None (all interfaces) | Network interface for beaconing |
| evasive_timeout_ms | `u32` | 5000 | Time before peer marked evasive |
| expired_timeout_ms | `u32` | 30000 | Time before peer marked expired |
| beacon_interval_ms | `u32` | 1000 | Beacon broadcast interval |
| gossip | `Option<GossipConfig>` | None | If set, use gossip instead of beacon |

**Validation rules**:
- `evasive_timeout_ms` must be > 0
- `expired_timeout_ms` must be > `evasive_timeout_ms`
- `port` if Some, must be > 0
- `name` if Some, must be non-empty

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
[Created] --start()--> [Running] --stop()/drop--> [Stopped]
    |                      |
    |                      +--recv()--> [Running] (returns ZyreEvent)
    |                      +--shout()/whisper()--> [Running]
    |                      +--join()/leave()--> [Running]
    |
    +-- (any operation except start) --> Error::NotStarted
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
    └── creates → ZyreNode (via NodeConfig)
                      ├── receives → ZyreEvent
                      ├── identifies peers via → PeerId
                      └── configured by → NodeConfig / GossipConfig
```
