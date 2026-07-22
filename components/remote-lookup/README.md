# remote-lookup

## Summary

`remote-lookup` resolves cache keys that miss locally by fetching them from other
Certus nodes. It implements `IRemoteLookup`: the dispatcher forwards any entries
not found in the local memory tier or on SSD to `batch_lookup`, which locates the
value on a peer and pulls it in over RDMA.

Coordination runs over **zyre** (group SHOUT for discovery of who holds a key,
directed WHISPER for the exchange); the value itself moves out of band over
**RDMA** as a one-sided write into a landing slot the requester reserves in its
own memory tier. The heavy RDMA work is delegated to the sibling
`remote-lookup-rdma-initiator` / `remote-lookup-rdma-responder` components; this
crate owns the protocol and state machine. See
`specs/002-remote-lookup-rdma/` for the full design.

## Protocol

For each `batch_lookup(&[(CacheKey, u32)])` (key + expected size):

1. **KEY_QUERY** (SHOUT) — ask the group which peers hold each `(key, size)`.
2. **KEY_RESPONSE** (WHISPER) — each peer classifies every key against its
   dispatch map: in memory / on disk / not available (a size mismatch is a
   miss). Phase 1 greedily fetches memory hits; disk holders are cached.
3. **RDMA_REQUEST** (WHISPER) — the requester reserves a private landing slot
   (`memory_tier.insert`) and asks the holder to write the value into it,
   advertising its own responder endpoint + pool rkey + slot address.
4. **RDMA_STATUS** (WHISPER) — after the one-sided write completes, the holder
   reports per-key success/failure. On success the requester **publishes** the
   slot to its dispatch map (publish-on-success — nothing is exposed to local
   readers until the data has landed).

Additional behavior:

- **Single-flight** — concurrent lookups of the same key issue one RDMA; the
  rest ride along and complete together.
- **Retry** — a failed fetch re-targets an alternate cached holder (memory
  preferred, then disk), bounded by `max_retry_rounds`.
- **Phase-2 disk fallback** — keys satisfiable only from a peer's disk tier are
  fetched once the memory phase stalls; the serving peer promotes the value
  (`dispatcher.promote_to_memory_tier`) before writing it.
- **Completion / timeout** — an operation finalizes when every key is satisfied,
  when every expected peer has replied with nothing left in flight, or when
  `op_deadline` elapses (unsatisfied keys return `NotFound`, and the caller's
  layer may recompute).
- **Teardown-before-reclaim** — a landing slot exposed to a peer is never freed
  on a timeout while that peer is still live; it is reclaimed on a late
  RDMA_STATUS, or when the peer exits after the responder confirms the queue
  pair is torn down (memory-safety invariant SC-005).

All mutable state lives on a single actor poll-loop thread that owns the zyre
node; callers submit work over an MPSC channel and block on a per-operation
one-shot.

## Interface (`IRemoteLookup`)

| Method | Description |
|--------|-------------|
| `initialize(LookupConfig) -> Result<(), RemoteLookupError>` | Create/join the zyre node, bring up the responder, spawn the actor. Idempotent. |
| `batch_lookup(&[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>>` | Resolve each `(key, size)` from peers; positional results (`Ok(())` = published locally). |
| `join_cluster(&str) -> Result<(), RemoteLookupError>` | Join an additional zyre group. |
| `leave_cluster() -> Result<(), RemoteLookupError>` | Leave the configured group. |

## Configuration (`LookupConfig`)

`group`, `quorum_pct`, `phase1_timeout`, `op_deadline` (default 50 ms),
`max_retry_rounds`, `max_keys_per_query`, `bind_ip`, `actor_cpu`, and the
discovery knobs `discovery: Option<GossipConfig>` (`None` = UDP beacon, `Some` =
gossip) + `node_endpoint`. Derives `Default`.

## Receptacles

| Name | Interface | Purpose |
|------|-----------|---------|
| `zyre` | `IZyre` | Discovery + signalling (creates the node). |
| `dispatch_map` | `IDispatchMap` | Classify/serve keys; publish fetched values. |
| `memory_tier` | `IMemoryTier` | Reserve landing slots for inbound RDMA writes. |
| `dispatcher` | `IDispatcher` | Promote disk-only keys before serving (US4). |
| `initiator` | `IRemoteLookupRdmaInitiator` | One-sided RDMA writes to a peer. |
| `responder` | `IRemoteLookupRdmaResponder` | Local pool registration + endpoint/rkey. |
| `responder_admin` | `IRemoteLookupRdmaResponderAdmin` | Responder lifecycle/config. |
| `logger` | `ILogger` | Diagnostic logging. |

## Usage

```rust
use component_core::query_interface;
use interfaces::{IRemoteLookup, LookupConfig};
use remote_lookup::RemoteLookupComponent;

let comp = RemoteLookupComponent::new_default();
let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
    query_interface!(comp, IRemoteLookup).unwrap();

// Without receptacles wired, initialize reports a transport error.
assert!(rl.initialize(LookupConfig::default()).is_err());
```

Wiring all receptacles (zyre + the RDMA/local-state providers) and calling
`initialize` brings the node up; the integrating mainline must
`Receptacle::disconnect()` one side of the `dispatcher-p2p ⇄ remote-lookup` Arc
cycle at teardown (see `specs/002-remote-lookup-rdma/quickstart.md`).

## Build & Test

```bash
cargo build -p remote-lookup
cargo test  -p remote-lookup      # 18 unit + 10 mesh + 2 doc tests
cargo clippy -p remote-lookup --all-targets -- -D warnings
cargo doc   -p remote-lookup --no-deps
```

The multi-node integration tests in `tests/mesh.rs` spin up several **real**
zyre nodes in-process (gossip over TCP loopback) with the NIC and local state
mocked via `remote_lookup::seams` (research Decision 8). `build.rs` embeds an
rpath to the pre-built zyre libraries (`deps/zyre-build/`), so no
`LD_LIBRARY_PATH` is needed. The `#[ignore]`d hardware loopback variant is gated
behind the `rdma` feature.

**Note:** this crate enables `interfaces/spdk` (it consumes `IDispatchMap` /
`IDispatcher`, currently SPDK-gated), so it is outside the workspace
`default-members` and needs SPDK at `deps/spdk-build/`. See root `Cargo.toml` and
`specs/002-remote-lookup-rdma/research.md` Decision 10.
