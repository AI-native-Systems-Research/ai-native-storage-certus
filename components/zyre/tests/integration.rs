//! Integration tests for zyre node discovery and messaging.
//!
//! These tests require the zyre C libraries to be pre-built at
//! `deps/zyre-build/`. They exercise real network I/O on localhost.
//!
//! # Why these tests serialize themselves
//!
//! The underlying czmq `zsys` layer keeps process-global state (notably a
//! socket counter that guards `zsys_set_thread_name_prefix`, plus fixed UDP
//! beacon ports). The default `cargo test` harness runs each test on its own
//! thread within a single process, so creating zyre nodes in two tests at once
//! aborts the process (`assert (s_open_sockets == 0)` -> SIGABRT) and races on
//! network ports. Every test here therefore takes `SERIAL_LOCK` for its whole
//! body, which reproduces single-threaded execution regardless of how the
//! harness is invoked. The guard is declared first so it is released last,
//! after each node's `Drop` has torn down its sockets.

use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use component_core::query_interface;
use zyre::{GossipConfig, IZyre, NodeConfig, ZyreComponent, ZyreEvent};

/// Process-global lock serializing the socket-creating integration tests.
static SERIAL_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the serial lock, recovering from poisoning so that a panicking
/// (failing) test does not cascade into spurious failures in the others.
fn serialize() -> MutexGuard<'static, ()> {
    SERIAL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn two_nodes_discover_and_shout() {
    let _guard = serialize();

    let comp = ZyreComponent::new();
    let izyre = query_interface!(comp, IZyre).expect("query IZyre");

    let mut config_a = NodeConfig::default();
    config_a.name = Some("node-a".into());
    let mut config_b = NodeConfig::default();
    config_b.name = Some("node-b".into());

    let mut node_a = izyre.create_node(config_a).expect("create node A");
    let mut node_b = izyre.create_node(config_b).expect("create node B");

    node_a.start().expect("start A");
    node_b.start().expect("start B");

    node_a.join("test-group").expect("A join");
    node_b.join("test-group").expect("B join");

    // Allow time for UDP beacon discovery
    thread::sleep(Duration::from_millis(500));

    // Node A shouts a message to the group
    let payload = b"hello from A";
    node_a
        .shout("test-group", payload)
        .expect("A shout to group");

    // Node B should receive ENTER events and the SHOUT
    let mut received_shout = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);

    while std::time::Instant::now() < deadline {
        match node_b.try_recv() {
            Ok(Some(ZyreEvent::Shout { message, group, .. })) => {
                if group == "test-group" && message == payload {
                    received_shout = true;
                    break;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("recv error: {e}"),
        }
    }

    assert!(received_shout, "node B should receive shout from node A");
}

#[test]
fn two_nodes_whisper() {
    let _guard = serialize();

    let comp = ZyreComponent::new();
    let izyre = query_interface!(comp, IZyre).expect("query IZyre");

    let mut config_a = NodeConfig::default();
    config_a.name = Some("whisper-a".into());
    let mut config_b = NodeConfig::default();
    config_b.name = Some("whisper-b".into());

    let mut node_a = izyre.create_node(config_a).expect("create node A");
    let mut node_b = izyre.create_node(config_b).expect("create node B");

    node_a.start().expect("start A");
    node_b.start().expect("start B");

    // Wait for discovery
    thread::sleep(Duration::from_millis(500));

    // Find node A's peer ID as seen by node B
    let mut peer_a_id = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match node_b.try_recv() {
            Ok(Some(ZyreEvent::Enter { peer, name, .. })) if name == "whisper-a" => {
                peer_a_id = Some(peer);
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("recv error: {e}"),
        }
    }
    let peer_a_id = peer_a_id.expect("B should discover A");

    // B whispers to A
    let payload = b"secret message";
    node_b.whisper(&peer_a_id, payload).expect("B whisper to A");

    // A receives the whisper
    let mut received_whisper = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        match node_a.try_recv() {
            Ok(Some(ZyreEvent::Whisper { message, .. })) => {
                if message == payload {
                    received_whisper = true;
                    break;
                }
            }
            Ok(Some(_)) => continue,
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("recv error: {e}"),
        }
    }

    assert!(
        received_whisper,
        "node A should receive whisper from node B"
    );
}

#[test]
fn gossip_discovery() {
    let _guard = serialize();

    let comp = ZyreComponent::new();
    let izyre = query_interface!(comp, IZyre).expect("query IZyre");

    // Each node needs its own unique data endpoint, distinct from the shared
    // gossip hub endpoint (19876).
    let mut config_a = NodeConfig::default();
    config_a.name = Some("gossip-a".into());
    config_a.endpoint = Some("tcp://127.0.0.1:19877".into());
    config_a.gossip = Some(GossipConfig::bind("tcp://127.0.0.1:19876"));

    let mut config_b = NodeConfig::default();
    config_b.name = Some("gossip-b".into());
    config_b.endpoint = Some("tcp://127.0.0.1:19878".into());
    config_b.gossip = Some(GossipConfig::connect("tcp://127.0.0.1:19876"));

    let mut node_a = izyre.create_node(config_a).expect("create gossip node A");
    let mut node_b = izyre.create_node(config_b).expect("create gossip node B");

    node_a.start().expect("start gossip A");
    node_b.start().expect("start gossip B");

    // Wait for gossip discovery (may take slightly longer than beacon)
    let mut discovered = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match node_b.try_recv() {
            Ok(Some(ZyreEvent::Enter { name, .. })) if name == "gossip-a" => {
                discovered = true;
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => panic!("recv error: {e}"),
        }
    }

    assert!(discovered, "node B should discover node A via gossip");
}
