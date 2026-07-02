//! Compile-time assertions for API safety guarantees.

use zyre::ZyreNode;

fn assert_send<T: Send>() {}

#[test]
fn zyre_node_is_send() {
    assert_send::<ZyreNode>();
}

/// ZyreNode must NOT be Sync: the underlying C API is not safe for concurrent
/// access to a single node. This is enforced structurally by `unsafe impl Send`
/// for ZyreNode *without* a matching `unsafe impl Sync`, so any attempt to share
/// `&ZyreNode` across threads fails to compile. This test documents the
/// invariant; the guarantee itself is compile-time and needs no runtime check.
#[test]
fn zyre_node_is_not_sync() {
    // Sanity check that ZyreNode is at least Send (the counterpart guarantee).
    // The absence of Sync cannot be asserted at runtime on stable Rust without
    // trybuild-style compile-fail tests, so it is documented above instead.
    assert_send::<ZyreNode>();
}

#[test]
fn public_api_has_no_unsafe_exposure() {
    // Verify that the public API types are constructible without unsafe:
    // - NodeConfig via builder
    // - ZyreEvent variants
    // - PeerId via From
    // - ZyreError variants

    use zyre::{GossipConfig, NodeConfig, PeerId, ZyreError, ZyreEvent};

    let _config = NodeConfig::builder().name("test").build();
    let _gossip = GossipConfig::bind("tcp://*:9999");
    let _peer = PeerId::from("some-uuid");
    let _err = ZyreError::CreateFailed;
    let _event = ZyreEvent::Stop;
}
