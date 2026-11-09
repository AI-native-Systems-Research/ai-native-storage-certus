//! Integration tests for channel binding enforcement.

use component_framework::channel::mpsc::MpscChannel;
use component_framework::channel::spsc::SpscChannel;
use component_framework::channel::{ChannelError, IReceiver, ISender};
use component_framework::iunknown::{query, IUnknown};
use std::sync::Arc;

#[test]
fn spsc_rejects_second_sender() {
    let ch = SpscChannel::<u32>::new(4);
    let _tx = ch.sender().unwrap();

    match ch.sender() {
        Err(ChannelError::BindingRejected { reason }) => {
            assert!(
                reason.contains("sender"),
                "expected sender mention: {reason}"
            );
        }
        other => panic!("expected BindingRejected, got {other:?}"),
    }
}

#[test]
fn spsc_rejects_second_receiver() {
    let ch = SpscChannel::<u32>::new(4);
    let _rx = ch.receiver().unwrap();

    match ch.receiver() {
        Err(ChannelError::BindingRejected { reason }) => {
            assert!(
                reason.contains("receiver"),
                "expected receiver mention: {reason}"
            );
        }
        other => panic!("expected BindingRejected, got {other:?}"),
    }
}

#[test]
fn mpsc_accepts_multiple_senders() {
    let ch = MpscChannel::<u32>::new(4);
    let _tx1 = ch.sender().unwrap();
    let _tx2 = ch.sender().unwrap();
    let _tx3 = ch.sender().unwrap();
    // All succeed
}

#[test]
fn mpsc_rejects_second_receiver() {
    let ch = MpscChannel::<u32>::new(4);
    let _rx = ch.receiver().unwrap();

    match ch.receiver() {
        Err(ChannelError::BindingRejected { reason }) => {
            assert!(
                reason.contains("receiver"),
                "expected receiver mention: {reason}"
            );
        }
        other => panic!("expected BindingRejected, got {other:?}"),
    }
}

#[test]
fn spsc_sender_disconnect_frees_slot_for_rebinding() {
    let ch = SpscChannel::<u32>::new(4);

    // Bind and unbind a sender
    {
        let _tx = ch.sender().unwrap();
    } // tx dropped — slot freed

    // Re-bind should succeed
    let tx2 = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    tx2.send(99).unwrap();
    assert_eq!(rx.recv().unwrap(), 99);
}

#[test]
fn spsc_receiver_disconnect_frees_slot_for_rebinding() {
    let ch = SpscChannel::<u32>::new(4);

    {
        let _rx = ch.receiver().unwrap();
    }

    let _rx2 = ch.receiver().unwrap();
}

// --- IUnknown interface query tests ---

#[test]
fn spsc_iunknown_query_send_recv() {
    let ch = SpscChannel::<u64>::new(16);
    let tx: Arc<dyn ISender<u64> + Send + Sync> =
        query::<dyn ISender<u64> + Send + Sync>(&ch).unwrap();
    let rx: Arc<dyn IReceiver<u64> + Send + Sync> =
        query::<dyn IReceiver<u64> + Send + Sync>(&ch).unwrap();

    tx.send(100).unwrap();
    tx.send(200).unwrap();
    assert_eq!(rx.recv().unwrap(), 100);
    assert_eq!(rx.recv().unwrap(), 200);
}

#[test]
fn spsc_iunknown_rejects_second_sender_query() {
    let ch = SpscChannel::<u32>::new(4);
    let _tx: Arc<dyn ISender<u32> + Send + Sync> =
        query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
    assert!(query::<dyn ISender<u32> + Send + Sync>(&ch).is_none());
}

#[test]
fn spsc_iunknown_rejects_second_receiver_query() {
    let ch = SpscChannel::<u32>::new(4);
    let _rx: Arc<dyn IReceiver<u32> + Send + Sync> =
        query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();
    assert!(query::<dyn IReceiver<u32> + Send + Sync>(&ch).is_none());
}

#[test]
fn spsc_iunknown_direct_api_blocks_query() {
    let ch = SpscChannel::<u32>::new(4);
    let _tx = ch.sender().unwrap();
    // IUnknown query for ISender should fail — already bound via direct API
    assert!(query::<dyn ISender<u32> + Send + Sync>(&ch).is_none());
}

#[test]
fn mpsc_iunknown_query_send_recv() {
    let ch = MpscChannel::<u32>::new(16);
    let tx: Arc<dyn ISender<u32> + Send + Sync> =
        query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
    let rx: Arc<dyn IReceiver<u32> + Send + Sync> =
        query::<dyn IReceiver<u32> + Send + Sync>(&ch).unwrap();

    tx.send(42).unwrap();
    assert_eq!(rx.recv().unwrap(), 42);
}

#[test]
fn mpsc_iunknown_multiple_sender_queries_succeed() {
    let ch = MpscChannel::<u32>::new(4);
    let _tx1: Arc<dyn ISender<u32> + Send + Sync> =
        query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
    // MPSC allows multiple queries for ISender
    let _tx2: Arc<dyn ISender<u32> + Send + Sync> =
        query::<dyn ISender<u32> + Send + Sync>(&ch).unwrap();
}

#[test]
fn channel_introspection() {
    let ch = SpscChannel::<u32>::new(4);
    assert_eq!(ch.provided_interfaces().len(), 2);
    assert!(ch.receptacles().is_empty());
    assert_eq!(ch.version(), "1.0.0");

    let iface_names: Vec<&str> = ch.provided_interfaces().iter().map(|i| i.name).collect();
    assert!(iface_names.contains(&"ISender"));
    assert!(iface_names.contains(&"IReceiver"));
}
