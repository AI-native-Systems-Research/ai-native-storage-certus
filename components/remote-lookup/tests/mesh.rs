//! Multi-node, in-process protocol tests over the mock seams (research
//! Decision 8): N ≥ 4 back-to-back `remote-lookup` instances discover each other
//! over the **real** `zyre` component (gossip on TCP loopback) and exercise the
//! full SHOUT → KEY_RESPONSE → RDMA_REQUEST → RDMA_STATUS protocol, single-flight,
//! completion/timeout, and peer-exit teardown. Only the NIC (RDMA
//! initiator/responder) and local state (memory-tier/dispatch-map/dispatcher) are
//! mocked, via `remote_lookup::seams`.
//!
//! The `#[ignore]`d hardware loopback variant (real zyre + single-host RDMA
//! loopback) is gated behind the `rdma` feature and added at T034.
//!
//! Assertions target timing-robust invariants only; determinism comes from a
//! discovery barrier ([`TestMesh::await_discovery`]) plus, in later protocol
//! tests, app-level reply delays scripted into each node's mock server.

use std::net::TcpListener;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use component_core::query_interface;
use interfaces::{
    Endpoint, GossipConfig, IDispatchMap, IDispatcher, IMemoryTier, IRemoteLookup,
    IRemoteLookupRdmaInitiator, IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin, IZyre,
    LookupConfig, PushStatus,
};
use remote_lookup::seams::{
    MockDispatchMap, MockDispatcher, MockInitiator, MockMemoryTier, MockResponder, NodeWorld,
};
use remote_lookup::RemoteLookupComponent;
use zyre::ZyreComponent;

/// Process-wide lock serialising `TestMesh` lifetimes. Each mesh spins several
/// real zyre nodes that share the process-wide czmq context; running multiple
/// meshes concurrently (the default `cargo test` behaviour within a binary)
/// races that global state and aborts. Holding this for a mesh's lifetime lets
/// `cargo test` pass without `--test-threads 1`. Poison is ignored (a panicking
/// test should not wedge the rest).
fn mesh_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Grab an ephemeral TCP port by binding to `:0` and immediately releasing it.
/// There is a small TOCTOU window before zyre rebinds it, acceptable for tests.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// An in-process mesh of `remote-lookup` instances wired to one real zyre
/// provider via gossip over TCP loopback, each backed by a scriptable
/// [`NodeWorld`] and mock RDMA seams.
struct TestMesh {
    /// The `remote-lookup` instances, index-aligned with `worlds`. Declared
    /// first so they drop (joining their actor threads) before the zyre
    /// provider.
    nodes: Vec<Arc<RemoteLookupComponent>>,
    /// Per-node scriptable local state, index-aligned with `nodes`.
    worlds: Vec<NodeWorld>,
    /// Kept alive for the mesh's lifetime (nodes also hold `IZyre` clones).
    _zyre: Arc<ZyreComponent>,
    /// Serialises mesh lifetimes across concurrently-run tests.
    _guard: MutexGuard<'static, ()>,
    group: String,
}

impl TestMesh {
    /// Build an `n`-node mesh with the default per-node config.
    fn new(n: usize) -> Self {
        Self::new_with(n, |_cfg| {})
    }

    /// Build an `n`-node mesh. Node 0 binds the gossip hub; the rest connect to
    /// it. Every node gets a distinct data-mailbox endpoint. All instances are
    /// initialized and joined to a shared group. `tweak` customizes each node's
    /// [`LookupConfig`] before `initialize` (e.g. to set `caller_wait` or
    /// `connection_teardown_timeout` for the decoupling/teardown tests).
    fn new_with(n: usize, tweak: impl Fn(&mut LookupConfig)) -> Self {
        assert!(n >= 2, "a mesh needs at least two nodes");
        let guard = mesh_lock();
        let group = "mesh".to_string();

        let zyre_comp = ZyreComponent::new();
        let izyre: Arc<dyn IZyre + Send + Sync> =
            query_interface!(zyre_comp, IZyre).expect("zyre provides IZyre");

        let hub_endpoint = format!("tcp://127.0.0.1:{}", free_port());

        let mut nodes = Vec::with_capacity(n);
        let mut worlds = Vec::with_capacity(n);

        for i in 0..n {
            let comp = RemoteLookupComponent::new_default();
            let world = NodeWorld::with_default_pool();
            // Give each node a distinct advertised RDMA endpoint so warm-at-
            // discovery (and any endpoint-sensitive assertion) is meaningful.
            world.set_endpoint(Endpoint {
                ip: "127.0.0.1".to_string(),
                port: 6000 + i as u16,
            });

            // Wire the real zyre provider and the mock seams.
            comp.zyre.connect(Arc::clone(&izyre)).expect("connect zyre");
            comp.memory_tier
                .connect(Arc::new(MockMemoryTier::new(world.clone()))
                    as Arc<dyn IMemoryTier + Send + Sync>)
                .expect("connect memory_tier");
            comp.dispatch_map
                .connect(Arc::new(MockDispatchMap::new(world.clone()))
                    as Arc<dyn IDispatchMap + Send + Sync>)
                .expect("connect dispatch_map");
            comp.dispatcher
                .connect(Arc::new(MockDispatcher::new(world.clone()))
                    as Arc<dyn IDispatcher + Send + Sync>)
                .expect("connect dispatcher");
            comp.initiator
                .connect(Arc::new(MockInitiator::new(world.clone()))
                    as Arc<dyn IRemoteLookupRdmaInitiator + Send + Sync>)
                .expect("connect initiator");
            let (responder, responder_admin) = MockResponder::new(world.clone());
            comp.responder
                .connect(Arc::new(responder) as Arc<dyn IRemoteLookupRdmaResponder + Send + Sync>)
                .expect("connect responder");
            comp.responder_admin
                .connect(Arc::new(responder_admin)
                    as Arc<dyn IRemoteLookupRdmaResponderAdmin + Send + Sync>)
                .expect("connect responder_admin");

            // Gossip discovery: node 0 binds the hub, the rest connect.
            let discovery = if i == 0 {
                GossipConfig::bind(hub_endpoint.clone())
            } else {
                GossipConfig::connect(hub_endpoint.clone())
            };
            let mut cfg = LookupConfig {
                group: group.clone(),
                discovery: Some(discovery),
                node_endpoint: Some(format!("tcp://127.0.0.1:{}", free_port())),
                // Shorten the deadline so timeout-path tests finish quickly.
                op_deadline: Duration::from_millis(200),
                ..Default::default()
            };
            tweak(&mut cfg);

            comp.initialize(cfg).expect("initialize node");
            nodes.push(comp);
            worlds.push(world);
        }

        Self {
            nodes,
            worlds,
            _zyre: zyre_comp,
            _guard: guard,
            group,
        }
    }
}

impl Drop for TestMesh {
    fn drop(&mut self) {
        // Two-phase teardown (see `RemoteLookupComponent::signal_shutdown`): tell
        // every actor to stop polling its zyre node, let the poll loops observe
        // it and release their nodes, then join. This prevents one node being
        // destroyed while another actor is mid-`try_recv` on the shared czmq
        // context (which trips a zpoller assertion at process exit).
        for n in &self.nodes {
            n.signal_shutdown();
        }
        std::thread::sleep(Duration::from_millis(50));
        for n in &self.nodes {
            n.shutdown();
        }
    }
}

impl TestMesh {
    /// Block until every node sees all `n-1` peers, or `timeout` elapses.
    /// Returns `true` if the full mesh formed.
    fn await_discovery(&self, timeout: Duration) -> bool {
        let need = self.nodes.len() - 1;
        let deadline = Instant::now() + timeout;
        loop {
            if self.nodes.iter().all(|c| c.peers_seen() >= need) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn four_node_mesh_discovers_over_gossip() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "all four nodes should discover each other over gossip within 15s; \
         per-node peer counts: {:?}",
        mesh.nodes
            .iter()
            .map(|c| c.peers_seen())
            .collect::<Vec<_>>()
    );

    // The scriptable worlds are index-aligned and start empty.
    assert_eq!(mesh.worlds.len(), 4);
    assert!(!mesh.worlds[0].contains(1));

    // The group is joined and the actor round-trip works live: a placeholder
    // batch_lookup still terminates (all NotFound until the protocol lands).
    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(1, 4096), (2, 4096)]);
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.is_err()));

    assert_eq!(mesh.group, "mesh");
}

#[test]
fn memory_hit_is_fetched_from_peer() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Only node 1 holds key 42 (4096 bytes) in its memory tier.
    mesh.worlds[1].with_memory(42, 4096);

    // Node 0 asks the group; it should fetch key 42 from node 1 over the
    // (mock) RDMA path and publish it locally.
    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(42, 4096)]);

    assert_eq!(results, vec![Ok(())], "key 42 should be satisfied");
    assert!(
        mesh.worlds[0].is_memory_resident(42),
        "node 0 should have published key 42 to its dispatch map"
    );
}

#[test]
fn concurrent_same_key_lookups_issue_one_rdma() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Node 1 holds key 99. Delay its serve so the first fetch stays in-flight
    // while the second lookup's KEY_RESPONSE is processed — that second op must
    // attach as a single-flight follower rather than issue its own RDMA.
    mesh.worlds[1].with_memory(99, 4096);
    mesh.worlds[1].set_serve_delay(Duration::from_millis(60));

    let node0 = Arc::clone(&mesh.nodes[0]);
    let handle = {
        let n = Arc::clone(&node0);
        std::thread::spawn(move || {
            let rl: Arc<dyn IRemoteLookup + Send + Sync> =
                query_interface!(n, IRemoteLookup).unwrap();
            rl.batch_lookup(&[(99, 4096)])
        })
    };
    let rl: Arc<dyn IRemoteLookup + Send + Sync> = query_interface!(node0, IRemoteLookup).unwrap();
    let r_main = rl.batch_lookup(&[(99, 4096)]);
    let r_thread = handle.join().expect("lookup thread panicked");

    assert_eq!(r_main, vec![Ok(())], "main lookup should be satisfied");
    assert_eq!(
        r_thread,
        vec![Ok(())],
        "concurrent lookup should be satisfied"
    );
    assert_eq!(
        mesh.worlds[1].push_count(99),
        1,
        "single-flight: key 99 should be fetched over RDMA exactly once"
    );
}

#[test]
fn slot_survives_timeout_while_peer_live_then_reclaimed_on_late_status() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Node 1 holds key 55 but serves far slower than the op deadline (200ms),
    // so node 0's operation times out with the fetch still in flight.
    mesh.worlds[1].with_memory(55, 4096);
    mesh.worlds[1].set_serve_delay(Duration::from_millis(400));

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(55, 4096)]);
    assert_eq!(
        results,
        vec![Err(interfaces::RemoteLookupError::NotFound)],
        "op should time out"
    );

    // SC-005: the landing slot exposed to the still-live peer must NOT be
    // reclaimed on the timeout — a late one-sided write could still land.
    assert!(
        mesh.worlds[0].has_reservation(55),
        "slot must survive the timeout while the peer is a live member"
    );

    // Once the peer finishes serving, its RDMA_STATUS arrives late (the op is
    // gone) and the orphaned slot is reclaimed.
    let mut reclaimed = false;
    for _ in 0..80 {
        if !mesh.worlds[0].has_reservation(55) {
            reclaimed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        reclaimed,
        "a late RDMA_STATUS should reclaim the orphaned landing slot"
    );
}

#[test]
fn failed_fetch_retries_alternate_peer() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Node 1 advertises key 77 in memory but fails to serve it, and serves
    // slowly so its failure lands after node 2's reply is cached. Node 2 holds
    // 77 and serves fine, but replies a little later — so node 0 tries node 1
    // first, fails, then retries the cached alternate (node 2).
    mesh.worlds[1].with_memory(77, 4096);
    mesh.worlds[1].force_push(77, PushStatus::KeyNotFound);
    mesh.worlds[1].set_serve_delay(Duration::from_millis(80));
    mesh.worlds[2].with_memory(77, 4096);
    mesh.worlds[2].set_reply_delay(Duration::from_millis(20));

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(77, 4096)]);

    assert_eq!(results, vec![Ok(())], "should succeed via retry to node 2");
    assert_eq!(
        mesh.worlds[1].push_count(77),
        1,
        "node 1 was tried once and failed"
    );
    assert_eq!(mesh.worlds[2].push_count(77), 1, "node 2 served the retry");
}

#[test]
fn retry_cap_exhausted_returns_not_found() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Nodes 1..=3 all advertise key 88 in memory but all fail to serve it. Node 0
    // should try alternates and eventually give up with NotFound.
    for i in 1..=3 {
        mesh.worlds[i].with_memory(88, 4096);
        mesh.worlds[i].force_push(88, PushStatus::KeyNotFound);
    }

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(88, 4096)]);

    assert_eq!(
        results,
        vec![Err(interfaces::RemoteLookupError::NotFound)],
        "all serves fail → NotFound after retries"
    );
    let total: usize = (1..=3).map(|i| mesh.worlds[i].push_count(88)).sum();
    assert!(
        (2..=3).contains(&total),
        "should retry alternates (each peer served at most once), total serves = {total}"
    );
}

#[test]
fn disk_only_hit_is_promoted_and_served() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Node 1 holds key 33 on disk only. Phase-1 (memory) finds nothing; the
    // Phase-2 disk re-scan fetches from node 1, whose dispatcher promotes the
    // key to memory and serves it (US4).
    mesh.worlds[1].with_disk(33, 4096, 0x1000);

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(33, 4096)]);

    assert_eq!(
        results,
        vec![Ok(())],
        "disk-only key should be promoted and served"
    );
    assert!(
        mesh.worlds[0].is_memory_resident(33),
        "node 0 should have published the fetched key"
    );
    // The serving node promoted the key from disk to memory.
    assert!(mesh.worlds[1].is_memory_resident(33));
}

#[test]
fn disk_promotion_failure_yields_not_found() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Node 1 holds key 44 on disk but its promotion is scripted to fail, so the
    // serve reports the key no longer available and the op ends NotFound.
    mesh.worlds[1].with_disk(44, 4096, 0x2000);
    mesh.worlds[1].fail_promote(44);

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(44, 4096)]);

    assert_eq!(
        results,
        vec![Err(interfaces::RemoteLookupError::NotFound)],
        "failed promotion → NotFound"
    );
}

#[test]
fn canonical_four_node_scenario() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    let (k1, k2, k3, sz) = (101u64, 102u64, 103u64, 4096u32);

    // Requester = node 0 (holds nothing).
    // Node 1 ("node2"): k1,k2 on disk, k3 in memory — but k3 is evicted before it
    // can be served (reports memory in KEY_RESPONSE, KeyNoLongerAvailable on serve).
    mesh.worlds[1].with_disk(k1, sz, 0x1000);
    mesh.worlds[1].with_disk(k2, sz, 0x2000);
    mesh.worlds[1].with_memory(k3, sz);
    mesh.worlds[1].evict_before_serve(k3);
    // Node 2 ("node3"): all three on disk; replies a little later.
    mesh.worlds[2].with_disk(k1, sz, 0x3000);
    mesh.worlds[2].with_disk(k2, sz, 0x4000);
    mesh.worlds[2].with_disk(k3, sz, 0x5000);
    mesh.worlds[2].set_reply_delay(Duration::from_millis(15));
    // Node 3 ("node4"): k1,k3 in memory only; replies later still (< op_deadline).
    mesh.worlds[3].with_memory(k1, sz);
    mesh.worlds[3].with_memory(k3, sz);
    mesh.worlds[3].set_reply_delay(Duration::from_millis(30));

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(k1, sz), (k2, sz), (k3, sz)]);

    // Timing-robust invariants (research Decision 8): every key is satisfied and
    // published locally, regardless of exact routing.
    assert_eq!(
        results,
        vec![Ok(()), Ok(()), Ok(())],
        "all three keys should be satisfied"
    );
    for k in [k1, k2, k3] {
        assert!(
            mesh.worlds[0].is_memory_resident(k),
            "node 0 should have published key {k}"
        );
    }
    // k3's only fast memory holder is node 1, which evicts before serving — so it
    // is tried there first and fails (then re-satisfied from node 3's memory).
    assert_eq!(
        mesh.worlds[1].push_count(k3),
        1,
        "k3 was requested from node 1 (the immediate memory holder) first"
    );
    // k2 is disk-only everywhere, so it is served by a disk holder that promoted it.
    assert!(
        mesh.worlds[1].is_memory_resident(k2) || mesh.worlds[2].is_memory_resident(k2),
        "k2 was promoted to memory at whichever disk holder served it"
    );
}

#[test]
fn warms_connections_to_discovered_peers() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Each node advertises its RDMA responder endpoint in its zyre ENTER header;
    // on discovery every node dispatches an off-loop warm-connect to each peer's
    // endpoint, so the cold RDMA connect is paid before the first serve rather
    // than on the poll loop mid-request (connect-hardening). The warm is
    // asynchronous (poll loop → worker → mock initiator), so poll briefly.
    let want = |i: usize| -> Vec<String> {
        (0..4)
            .filter(|&j| j != i)
            .map(|j| format!("127.0.0.1:{}", 6000 + j))
            .collect()
    };
    let mut ok = false;
    for _ in 0..100 {
        if (0..4).all(|i| {
            let warms = mesh.worlds[i].warms();
            want(i).iter().all(|e| warms.contains(e))
        }) {
            ok = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        ok,
        "each node should warm an initiator connection to every peer's endpoint"
    );
}

#[test]
fn phase1_timeout_triggers_disk_fallback_without_waiting_for_slow_peer() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Node 1 holds key 61 on disk and replies promptly. Node 2 is a laggard
    // (slow to classify/reply). The Phase-1 timeout must fire the disk fetch from
    // node 1 well before node 2 replies and well before op_deadline — so the key
    // is satisfied early rather than stalling on the slow peer (FR-010).
    let (k, sz) = (61u64, 4096u32);
    mesh.worlds[1].with_disk(k, sz, 0x9000);
    mesh.worlds[2].set_reply_delay(Duration::from_millis(150));

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let start = Instant::now();
    let results = rl.batch_lookup(&[(k, sz)]);
    let elapsed = start.elapsed();

    assert_eq!(results, vec![Ok(())], "disk key satisfied via Phase-2");
    assert!(
        elapsed < Duration::from_millis(120),
        "Phase-1 timeout should drive the disk fetch early (op_deadline 200ms, \
         slow peer 150ms); took {elapsed:?}"
    );
}

#[test]
fn total_miss_returns_not_found_within_deadline() {
    let mesh = TestMesh::new(4);
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // No node holds key 7 — every peer classifies it as not-available, so the
    // operation finalizes as NotFound (bounded by op_deadline, 200ms here).
    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let start = Instant::now();
    let results = rl.batch_lookup(&[(7, 4096)]);
    assert_eq!(results, vec![Err(interfaces::RemoteLookupError::NotFound)]);
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "should finalize promptly, took {:?}",
        start.elapsed()
    );
}

#[test]
fn caller_wait_returns_fast_then_background_op_fills_cache() {
    // The caller's patience (caller_wait) is decoupled from the operation's
    // lifetime (op_deadline). A slow fetch that outlasts caller_wait but finishes
    // within op_deadline must (a) let the caller return NotFound promptly, and
    // (b) still land and publish for the next lookup.
    let mesh = TestMesh::new_with(2, |cfg| {
        cfg.op_deadline = Duration::from_millis(2000);
        cfg.caller_wait = Some(Duration::from_millis(50));
    });
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Node 1 holds key 70 but serves slower than caller_wait (50ms) yet well
    // within op_deadline (2000ms).
    mesh.worlds[1].with_memory(70, 4096);
    mesh.worlds[1].set_serve_delay(Duration::from_millis(300));

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();

    let start = Instant::now();
    let results = rl.batch_lookup(&[(70, 4096)]);
    let waited = start.elapsed();

    // (a) Returned on caller_wait, not op_deadline (and not the 300ms serve).
    assert_eq!(results, vec![Err(interfaces::RemoteLookupError::NotFound)]);
    assert!(
        waited < Duration::from_millis(250),
        "caller should return ~caller_wait (50ms), not op_deadline; waited {waited:?}"
    );

    // (b) The operation kept running after the caller left; publish-on-success
    // makes the key resident on node 0 once the peer finishes serving.
    let mut filled = false;
    for _ in 0..100 {
        if mesh.worlds[0].is_memory_resident(70) {
            filled = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        filled,
        "background op should publish the key after the caller returned"
    );
}

#[test]
fn stuck_orphan_is_force_reclaimed_after_teardown_timeout() {
    // Backstop for a peer that neither reports a late RDMA_STATUS nor exits: the
    // orphaned landing slot must be force-reclaimed once the teardown grace
    // elapses — but only after the peer's QP is torn down (teardown-before-
    // reclaim). Without the timer this slot would leak forever.
    let mesh = TestMesh::new_with(2, |cfg| {
        cfg.op_deadline = Duration::from_millis(150);
        // Force-reclaim an orphan 200ms after the op finalizes.
        cfg.connection_teardown_timeout = Duration::from_millis(200);
    });
    assert!(
        mesh.await_discovery(Duration::from_secs(15)),
        "mesh did not form"
    );

    // Node 1 holds key 88 but "serves" far longer than the whole test window, so
    // no RDMA_STATUS ever comes back — the only reclaim path is the timer.
    mesh.worlds[1].with_memory(88, 4096);
    mesh.worlds[1].set_serve_delay(Duration::from_secs(30));

    let rl: Arc<dyn IRemoteLookup + Send + Sync> =
        query_interface!(Arc::clone(&mesh.nodes[0]), IRemoteLookup).unwrap();
    let results = rl.batch_lookup(&[(88, 4096)]);
    assert_eq!(
        results,
        vec![Err(interfaces::RemoteLookupError::NotFound)],
        "op should time out with the fetch still in flight"
    );

    // The slot must survive the op's finalize (SC-005) — no reclaim yet.
    assert!(
        mesh.worlds[0].has_reservation(88),
        "orphaned slot must survive finalize while the peer is still live"
    );

    // After the teardown grace, the timer severs the peer and reclaims the slot.
    // (Node 1 never sent a status, so only the timer can have freed it.)
    let mut reclaimed = false;
    for _ in 0..100 {
        if !mesh.worlds[0].has_reservation(88) {
            reclaimed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        reclaimed,
        "orphan should be force-reclaimed after connection_teardown_timeout"
    );
}
