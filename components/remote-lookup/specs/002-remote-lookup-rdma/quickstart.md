# Quickstart: Remote Lookup over Zyre + RDMA

**Feature**: 002-remote-lookup-rdma

## Wiring (mainline)

`remote-lookup` is an actor that owns a zyre node, an RDMA responder (its accept/registrar side),
and an RDMA initiator (to serve peers). Bind its receptacles, then call `initialize(LookupConfig)` —
the component brings up the responder (bind IP + pool registration) and the zyre node internally.

```rust
use component_core::query_interface;
use interfaces::IRemoteLookup;
use remote_lookup::RemoteLookupComponent;

let rl = RemoteLookupComponent::new_default();
// Receptacles (all interface-only; no impl-crate or spdk coupling):
rl.zyre.connect(zyre)?;                 // IZyre factory
rl.dispatch_map.connect(dispatch_map)?; // IDispatchMap
rl.memory_tier.connect(memory_tier)?;   // IMemoryTier (pool already initialized)
rl.dispatcher.connect(dispatcher)?;     // IDispatcher (US4 disk promotion; see teardown note below)
rl.initiator.connect(initiator)?;       // IRemoteLookupRdmaInitiator  (built --features rdma)
rl.responder.connect(responder)?;       // IRemoteLookupRdmaResponder
rl.responder_admin.connect(responder_admin)?;
rl.logger.connect(logger)?;             // optional

// Configure + bring up via a single config struct (mirrors IDispatcher::initialize).
let rli = query_interface!(rl, IRemoteLookup).unwrap();
rli.initialize(LookupConfig {           // all fields default; override what the profile supplies
    bind_ip: "10.0.0.102".into(),       // RoCE IPv4 for the responder
    actor_cpu: Some(numa_cpu),          // NUMA pin (best-effort)
    ..Default::default()                // adding a knob later stays additive — YAML-robust
})?;                                    // joins the group; brings up responder + registration

// The dispatcher then calls, for entries missed locally:
let results = rli.batch_lookup(&[(key, size)]);  // Ok(()) => key now resident in the memory tier

// Teardown: break the dispatcher-p2p ⇄ remote-lookup Arc cycle before drop.
dispatcher_p2p.remote_lookup.disconnect()?;      // (or rl.dispatcher.disconnect()?) — research Decision 7
```

The mainline app must build the RDMA crates with `--features rdma` (real transport); remote-lookup
itself is SPDK- and RDMA-orthogonal at the source level.

## Test — CI (no hardware)

Two full instances in one process over **mock** zyre + mock RDMA seams exercise the whole protocol
deterministically:

```bash
cargo test -p remote-lookup            # unit + two-instance protocol tests over the mock seams
cargo clippy -p remote-lookup -- -D warnings
cargo fmt -p remote-lookup --check
cargo doc -p remote-lookup --no-deps
```

Covers: memory-hit fill (US1), answering queries (US2), serving an RDMA request (US3),
completion/timeout (US6), peer-exit teardown-before-reclaim (US7), single-flight (SC-008), and the
structural greedy-dispatch check (SC-003).

## Test — hardware (two co-resident instances, one NIC)

```bash
# Two remote-lookup instances over real localhost zyre + single-host RDMA loopback
# (distinct ephemeral ports on the one mlx5 NIC).
CERTUS_RDMA_TEST_IP=10.0.0.102 \
  cargo test -p remote-lookup --features rdma -- --ignored --test-threads=1
```

Populate `(K, S)` in instance B's memory tier, call `batch_lookup([(K, S)])` on instance A, and
assert A's memory tier then contains K (a following local lookup hits) and the positional result is
`Ok(())` (spec User Story 1 independent test, SC-001).

## Scope note

Delivery is incremental: the core (increment 1) is memory-tier hits (US1–3, 6, 7). Multi-round
retry (US5) and disk fallback (US4) follow as increment 2 — both in scope. US4's serving-node
disk→memory promotion uses the existing `IDispatcher::promote_to_memory_tier` (research Dependency
D3 resolved); it requires a `dispatcher: IDispatcher` receptacle, and the mainline must
`Receptacle::disconnect()` one side of the `dispatcher-p2p ⇄ remote-lookup` Arc cycle at teardown.
