# Contract: `IRemoteLookup` (provided interface)

Crate: `interfaces` · File: `components/interfaces/src/iremote_lookup.rs` · Trait: `IRemoteLookup`

The interface `remote-lookup` provides to the dispatcher. The signature change (dropping
`IpcHandle`) is already implemented on this branch; this contract is the design-of-record for the
002 behavior behind it. **002 adds one method — `initialize(LookupConfig)`** — and a new public
`interfaces` type `LookupConfig` (this is the one `interfaces`-crate change for 002; the only
existing impl is remote-lookup's own, which 002 rewrites).

## Methods

```rust
fn initialize(&self, config: LookupConfig) -> Result<(), RemoteLookupError>;
fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Result<(), RemoteLookupError>>;
fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError>;
fn leave_cluster(&self) -> Result<(), RemoteLookupError>;
```

### `initialize(config) -> Result<(), RemoteLookupError>`

| Aspect | Contract |
|--------|----------|
| Purpose | Supply `LookupConfig` and bring the component up: spawn the actor, join the zyre group, and drive responder bring-up via the `responder_admin` receptacle (FR-025). Mirrors `IDispatcher::initialize(DispatcherConfig)`. |
| Config type | `LookupConfig` is a public `interfaces` type deriving `Default`; every field has a sensible default (FR-022). |
| YAML robustness | Configured from `apps/certus-server-yaml` via an `init_remote_lookup` hook that builds `LookupConfig { ..Default::default() }`, so adding a config field later is additive and never breaks the mainline. A profile that omits the init hook leaves the component on pure defaults. |
| Idempotency | Calling more than once returns an already-initialized error (mirrors sibling admin `initialize` semantics); safe to call before `join_cluster`/`batch_lookup`. |
| Errors | `Err(TransportError(_))` (or a dedicated variant) if responder bring-up / actor spawn fails. |

### `batch_lookup(entries) -> Vec<Result<(), RemoteLookupError>>`

| Aspect | Contract |
|--------|----------|
| Input | `&[(CacheKey, u32)]` — `(key, expected size in bytes)`. `size` is not an address. |
| Output | one positional `Result` per entry; `Ok(())` ⇒ the key is now resident in the **local memory tier** (a following local lookup hits). The dispatcher performs any DRAM→GPU delivery. |
| Errors | `Err(RemoteLookupError::NotFound)` when no peer supplied the key within the completion criteria; `Err(TransportError(_))` for a control-plane failure. |
| Blocking | blocks the calling thread until *this* operation finalizes (FR-020); other concurrent operations are unaffected. |
| Empty input | returns an empty vector immediately. |
| Single-flight | concurrent calls for the same unsatisfied `(key, size)` coalesce onto one in-flight fetch (SC-008). |

### `join_cluster(endpoint) / leave_cluster()`

Gossip-bind/connect the underlying zyre node to the cluster and join/leave the configured group
(FR-003). Idempotent; safe before/after activation.

## Provider obligations (behavioral)

- Runs as an actor on a dedicated thread owning an `IZyreNode`, its responder, and its initiator
  (FR-002).
- Never touches GPU memory; works exclusively in CPU/DRAM (spec Clarifications).
- Honors the [wire protocol](./wire-protocol.md) and its safety invariants, including
  teardown-before-reclaim on peer departure (FR-014).
