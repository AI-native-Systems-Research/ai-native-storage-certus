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
use zyre::{GossipConfig, IZyre, NodeConfig, ZyreComponent, ZyreError, ZyreEvent};

/// Process-global lock serializing the socket-creating integration tests.
static SERIAL_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the serial lock, recovering from poisoning so that a panicking
/// (failing) test does not cascade into spurious failures in the others.
fn serialize() -> MutexGuard<'static, ()> {
    SERIAL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Multiplier applied to the discovery/receive deadlines for slow environments.
/// Under valgrind everything runs ~20-40x slower, which would blow past the
/// wall-clock deadlines below; set e.g. `ZYRE_TEST_TIMEOUT_SCALE=40` so the
/// memory-safety run (see `run-valgrind.sh`) still completes. Defaults to 1.
fn timeout_scale() -> u32 {
    std::env::var("ZYRE_TEST_TIMEOUT_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1)
}

/// A base duration scaled by [`timeout_scale`].
fn scaled(base: Duration) -> Duration {
    base * timeout_scale()
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

    // A shout only reaches peers already in the group, so keep shouting until
    // B has discovered A and joined — a single pre-timed shout races discovery
    // (and loses that race under valgrind's slowdown).
    let payload = b"hello from A";
    let mut received_shout = false;
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(5));
    while std::time::Instant::now() < deadline {
        node_a
            .shout("test-group", payload)
            .expect("A shout to group");
        // Drain everything currently queued for B this round.
        loop {
            match node_b.try_recv() {
                Ok(Some(ZyreEvent::Shout { message, group, .. }))
                    if group == "test-group" && message == payload =>
                {
                    received_shout = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(e) => panic!("recv error: {e}"),
            }
        }
        if received_shout {
            break;
        }
        thread::sleep(Duration::from_millis(50));
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

    // Find node A's peer ID as seen by node B (discovery happens as we poll).
    let mut peer_a_id = None;
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(5));
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

    // B re-whispers to A until A receives it (robust against the return path
    // still wiring up, and against valgrind slowdown).
    let payload = b"secret message";
    let mut received_whisper = false;
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(5));
    while std::time::Instant::now() < deadline {
        node_b.whisper(&peer_a_id, payload).expect("B whisper to A");
        loop {
            match node_a.try_recv() {
                Ok(Some(ZyreEvent::Whisper { message, .. })) if message == payload => {
                    received_whisper = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(e) => panic!("recv error: {e}"),
            }
        }
        if received_whisper {
            break;
        }
        thread::sleep(Duration::from_millis(50));
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
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(10));
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

#[test]
fn stop_delivers_terminal_stop_event() {
    let _guard = serialize();

    let comp = ZyreComponent::new();
    let izyre = query_interface!(comp, IZyre).expect("query IZyre");

    let mut config = NodeConfig::default();
    config.name = Some("stopper".into());
    let mut node = izyre.create_node(config).expect("create node");
    node.start().expect("start");

    // `stop()` enqueues a terminal Stop sentinel on the inbox; the node stays
    // drainable until it is consumed.
    node.stop();

    // Drain (non-blocking) until we observe the terminal Stop event. With no
    // peers, Stop is the only queued message, but tolerate stray events.
    let mut saw_stop = false;
    let deadline = std::time::Instant::now() + scaled(Duration::from_secs(5));
    while std::time::Instant::now() < deadline {
        match node.try_recv() {
            Ok(Some(ZyreEvent::Stop)) => {
                saw_stop = true;
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => panic!("unexpected recv error before Stop: {e}"),
        }
    }
    assert!(saw_stop, "stop() should deliver a terminal ZyreEvent::Stop");

    // Once the sentinel is consumed the stream is terminal: recv() reports
    // Stopped, and try_recv() yields Ok(None) (never blocking on the now
    // producerless inbox).
    assert!(
        matches!(node.recv(), Err(ZyreError::Stopped)),
        "recv after Stop should return Stopped"
    );
    assert!(
        matches!(node.try_recv(), Ok(None)),
        "try_recv after Stop should return Ok(None)"
    );
}

/// SC-001: two nodes discover each other and exchange a round-trip message
/// within 2 seconds on localhost.
///
/// The clock starts before the nodes start and stops when A receives B's reply
/// to A's message (A -> B ping, B -> A pong). We re-send each side's message
/// while waiting so discovery latency is covered without a fixed pre-sleep.
///
/// This test asserts a real wall-clock bound, so it is meaningless (and would
/// fail) under valgrind's slowdown; it skips itself when `ZYRE_TEST_TIMEOUT_SCALE`
/// is set above 1.
#[test]
fn round_trip_within_two_seconds() {
    let _guard = serialize();

    if timeout_scale() > 1 {
        eprintln!("skipping round_trip_within_two_seconds under ZYRE_TEST_TIMEOUT_SCALE > 1");
        return;
    }

    let comp = ZyreComponent::new();
    let izyre = query_interface!(comp, IZyre).expect("query IZyre");

    let mut config_a = NodeConfig::default();
    config_a.name = Some("rt-a".into());
    let mut config_b = NodeConfig::default();
    config_b.name = Some("rt-b".into());

    let mut node_a = izyre.create_node(config_a).expect("create A");
    let mut node_b = izyre.create_node(config_b).expect("create B");

    let start = std::time::Instant::now();
    node_a.start().expect("start A");
    node_b.start().expect("start B");
    node_a.join("rt").expect("A join");
    node_b.join("rt").expect("B join");

    let deadline = start + Duration::from_secs(2);
    let mut b_saw_ping = false;
    let mut a_saw_pong = false;

    while std::time::Instant::now() < deadline && !a_saw_pong {
        if !b_saw_ping {
            node_a.shout("rt", b"ping").expect("A ping");
        } else {
            node_b.shout("rt", b"pong").expect("B pong");
        }

        // Drain B until it sees the ping, then drain A until it sees the pong.
        if !b_saw_ping {
            while let Ok(Some(ev)) = node_b.try_recv() {
                if matches!(ev, ZyreEvent::Shout { ref message, .. } if message == b"ping") {
                    b_saw_ping = true;
                    break;
                }
            }
        }
        if b_saw_ping {
            while let Ok(Some(ev)) = node_a.try_recv() {
                if matches!(ev, ZyreEvent::Shout { ref message, .. } if message == b"pong") {
                    a_saw_pong = true;
                    break;
                }
            }
        }

        thread::sleep(Duration::from_millis(10));
    }

    let elapsed = start.elapsed();
    assert!(
        a_saw_pong,
        "round-trip did not complete within 2s (elapsed {elapsed:?})"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "round-trip took {elapsed:?}, exceeding the SC-001 2s bound"
    );
}
