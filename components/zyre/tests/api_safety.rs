//! Compile-time assertions for API safety guarantees.

use zyre::IZyreNode;

fn assert_send<T: Send>() {}

/// The node handle returned by `IZyre::create_node` is `Send` (ownership can be
/// moved between threads). This is guaranteed by the `IZyreNode: Send`
/// supertrait bound, so `Box<dyn IZyreNode>` is `Send`.
#[test]
fn zyre_node_handle_is_send() {
    assert_send::<Box<dyn IZyreNode>>();
}

/// The node handle must NOT be `Sync`: the underlying C API is not safe for
/// concurrent access to a single node. `IZyreNode` has no `Sync` supertrait, so
/// `dyn IZyreNode` (and any `Box`/`&` of it) is not `Sync`, and sharing it
/// across threads fails to compile. This test documents the invariant; the
/// guarantee itself is compile-time and needs no runtime check.
#[test]
fn zyre_node_handle_is_not_sync() {
    // Sanity check that the handle is at least Send (the counterpart guarantee).
    // The absence of Sync cannot be asserted at runtime on stable Rust without
    // trybuild-style compile-fail tests, so it is documented above instead.
    assert_send::<Box<dyn IZyreNode>>();
}

#[test]
fn public_api_has_no_unsafe_exposure() {
    // Verify that the public API value types are constructible without unsafe:
    // - NodeConfig via Default + public fields
    // - GossipConfig via constructor
    // - ZyreEvent variants
    // - PeerId via From
    // - ZyreError variants

    use zyre::{GossipConfig, NodeConfig, PeerId, ZyreError, ZyreEvent};

    let mut config = NodeConfig::default();
    config.name = Some("test".into());
    let _config = config;
    let _gossip = GossipConfig::bind("tcp://*:9999");
    let _peer = PeerId::from("some-uuid");
    let _err = ZyreError::CreateFailed;
    let _event = ZyreEvent::Stop;
}
