//! T034 — integrated single-host RDMA loopback test (`#[ignore]`d, `rdma`
//! feature).
//!
//! This is the *only* test that exercises the whole remote fill end-to-end on
//! real hardware: two `remote-lookup` instances, each wired to the **real**
//! `remote-lookup-rdma-initiator` and `remote-lookup-rdma-responder` components
//! over a single host's RDMA NIC (RoCE/IB loopback), with only the local state
//! (memory-tier / dispatch-map / dispatcher) mocked via `seams.rs`. The mock
//! memory-tier hands out real, registerable pool pointers (`pool_info`/`peek`),
//! so the real responder can `ibv_reg_mr` the pool and the real initiator can
//! RDMA-write from it. Discovery still runs over the real `zyre` component.
//!
//! Purpose: confirm that with warm-at-discovery a cold cache fill actually
//! *succeeds* on hardware (the mock layer can never catch an rkey/endpoint/MR
//! mismatch), and to surface the initiator's per-phase connect telemetry so the
//! `op_deadline` / `phase1_timeout` defaults can be re-tuned against measured
//! `rdma_cm` latency (build additionally with `--features
//! remote-lookup-rdma-initiator/telemetry` to log the breakdown).
//!
//! # Running (on a host with an active RDMA device)
//!
//! ```bash
//! # Auto-detect the first active RDMA device:
//! cargo test -p remote-lookup --features rdma -- --ignored rdma_loopback
//! # Or pin the RoCE IPv4 both responders bind (distinct ephemeral ports):
//! CERTUS_RDMA_TEST_IP=<roce-ip> \
//!   cargo test -p remote-lookup --features rdma -- --ignored rdma_loopback
//! ```
//!
//! Requires the RDMA stack the real components need (rdma-core, an active
//! device with a routable IPv4, `memlock` unlimited). Without a NIC the test is
//! skipped (it is `#[ignore]`d); the surrounding crate still builds.
#![cfg(feature = "rdma")]

use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use component_core::query_interface;
use interfaces::{
    GossipConfig, IDispatchMap, IDispatcher, IMemoryTier, IRemoteLookup,
    IRemoteLookupRdmaInitiator, IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin, IZyre,
    LookupConfig,
};
use remote_lookup::seams::{MockDispatchMap, MockDispatcher, MockMemoryTier, NodeWorld};
use remote_lookup::RemoteLookupComponent;
use remote_lookup_rdma_initiator::RemoteLookupRdmaInitiatorComponent;
use remote_lookup_rdma_responder::RemoteLookupRdmaResponderComponent;
use zyre::ZyreComponent;

/// Grab an ephemeral TCP port from the OS (for the gossip hub and each node's
/// ZRE mailbox). Binding to a literal `:0` in a `GossipConfig` would leave the
/// connecting node with nothing concrete to dial, so we resolve real ports up
/// front — mirroring `mesh.rs`.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// One node's live objects, kept alive for the test's duration.
struct RdmaNode {
    comp: Arc<RemoteLookupComponent>,
    world: NodeWorld,
    // Real RDMA components: held so their actor/accept threads stay alive.
    _initiator: Arc<RemoteLookupRdmaInitiatorComponent>,
    _responder: Arc<RemoteLookupRdmaResponderComponent>,
}

/// Wire one node: real initiator + responder (sharing the node's world-backed
/// mock memory tier) and mock dispatch-map/dispatcher, then initialize it into
/// `group` with gossip discovery over `hub`. `bind_ip` is the RoCE IPv4 (empty
/// ⇒ the responder auto-detects the first active device).
fn build_node(
    izyre: &Arc<dyn IZyre + Send + Sync>,
    group: &str,
    node_endpoint: String,
    discovery: GossipConfig,
    bind_ip: String,
) -> RdmaNode {
    let world = NodeWorld::with_default_pool();

    // Real initiator, reading source bytes from this node's (real) pool.
    let initiator = RemoteLookupRdmaInitiatorComponent::new_default();
    initiator
        .memory_tier
        .connect(Arc::new(MockMemoryTier::new(world.clone())) as Arc<dyn IMemoryTier + Send + Sync>)
        .expect("initiator memory_tier");
    let initiator_if: Arc<dyn IRemoteLookupRdmaInitiator + Send + Sync> =
        query_interface!(Arc::clone(&initiator), IRemoteLookupRdmaInitiator).unwrap();

    // Real responder, registering this node's pool as the inbound landing MR.
    let responder = RemoteLookupRdmaResponderComponent::new_default();
    responder
        .memory_tier
        .connect(Arc::new(MockMemoryTier::new(world.clone())) as Arc<dyn IMemoryTier + Send + Sync>)
        .expect("responder memory_tier");
    let responder_if: Arc<dyn IRemoteLookupRdmaResponder + Send + Sync> =
        query_interface!(Arc::clone(&responder), IRemoteLookupRdmaResponder).unwrap();
    let responder_admin: Arc<dyn IRemoteLookupRdmaResponderAdmin + Send + Sync> =
        query_interface!(Arc::clone(&responder), IRemoteLookupRdmaResponderAdmin).unwrap();

    // The remote-lookup node itself: real zyre + real RDMA + mock local state.
    let comp = RemoteLookupComponent::new_default();
    comp.zyre.connect(Arc::clone(izyre)).expect("zyre");
    comp.memory_tier
        .connect(Arc::new(MockMemoryTier::new(world.clone())) as Arc<dyn IMemoryTier + Send + Sync>)
        .expect("memory_tier");
    comp.dispatch_map
        .connect(
            Arc::new(MockDispatchMap::new(world.clone())) as Arc<dyn IDispatchMap + Send + Sync>
        )
        .expect("dispatch_map");
    comp.dispatcher
        .connect(Arc::new(MockDispatcher::new(world.clone())) as Arc<dyn IDispatcher + Send + Sync>)
        .expect("dispatcher");
    comp.initiator.connect(initiator_if).expect("initiator");
    comp.responder.connect(responder_if).expect("responder");
    comp.responder_admin
        .connect(responder_admin)
        .expect("responder_admin");

    comp.initialize(LookupConfig {
        group: group.to_string(),
        discovery: Some(discovery),
        node_endpoint: Some(node_endpoint),
        bind_ip,
        // Real connects run long; give the op room well past a cold connect.
        op_deadline: Duration::from_secs(10),
        phase1_timeout: Duration::from_secs(3),
        ..Default::default()
    })
    .expect("initialize node");

    RdmaNode {
        comp,
        world,
        _initiator: initiator,
        _responder: responder,
    }
}

#[test]
#[ignore = "requires an active RDMA device (single-host loopback)"]
fn rdma_loopback_memory_hit_is_filled_over_real_rdma() {
    let group = "mesh-rdma";
    let bind_ip = std::env::var("CERTUS_RDMA_TEST_IP").unwrap_or_default();

    let zyre_comp = ZyreComponent::new();
    let izyre: Arc<dyn IZyre + Send + Sync> = query_interface!(zyre_comp, IZyre).unwrap();
    let hub = format!("tcp://127.0.0.1:{}", free_port());

    // Node 0 binds the gossip hub; node 1 connects to it. Distinct ZRE mailbox
    // endpoints; both responders bind the same RoCE IP on distinct ephemeral
    // ports (the co-resident case).
    let node0 = build_node(
        &izyre,
        group,
        format!("tcp://127.0.0.1:{}", free_port()),
        GossipConfig::bind(hub.clone()),
        bind_ip.clone(),
    );
    let node1 = build_node(
        &izyre,
        group,
        format!("tcp://127.0.0.1:{}", free_port()),
        GossipConfig::connect(hub),
        bind_ip,
    );

    // Discovery barrier: both nodes must see each other before the lookup.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if node0.comp.peers_seen() >= 1 && node1.comp.peers_seen() >= 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        node0.comp.peers_seen() >= 1 && node1.comp.peers_seen() >= 1,
        "nodes did not discover each other"
    );

    // Node 1 holds key 0xC0DE in memory at 4 KiB; node 0 fetches it. The fill
    // travels the full real path: SHOUT → KEY_RESPONSE → RDMA_REQUEST →
    // node 1's initiator RDMA-writes into node 0's pool → RDMA_STATUS.
    let (key, size) = (0xC0DEu64, 4096u32);
    node1.world.with_memory(key, size);

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&node0.comp), IRemoteLookup).unwrap();
    let start = Instant::now();
    let results = rl.batch_lookup(&[(key, size)]);
    let elapsed = start.elapsed();

    eprintln!("rdma_loopback fill took {elapsed:?}");
    assert_eq!(
        results,
        vec![Ok(())],
        "memory-resident key should be filled over real RDMA loopback"
    );
    // The value must have landed in node 0's published cache.
    assert!(
        node0.world.contains(key),
        "filled value not published locally"
    );

    // Two-phase teardown (mirrors `mesh.rs`): signal both actors to stop polling
    // the shared czmq context and release their zyre nodes, let the poll loops
    // observe it, then join. Signalling both before joining either prevents one
    // node being destroyed while the other actor is mid-`try_recv` on the shared
    // context (which trips a zpoller assertion at process exit).
    node0.comp.signal_shutdown();
    node1.comp.signal_shutdown();
    std::thread::sleep(Duration::from_millis(50));
    node0.comp.shutdown();
    node1.comp.shutdown();
}
