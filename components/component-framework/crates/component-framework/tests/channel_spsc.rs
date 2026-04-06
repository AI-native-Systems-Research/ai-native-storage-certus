//! Integration tests for SPSC channel.

use component_framework::channel::spsc::SpscChannel;
use component_framework::channel::ChannelError;
use std::thread;

#[test]
fn spsc_channel_delivers_messages_in_order() {
    let ch = SpscChannel::<u32>::new(64);
    let tx = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    for i in 0..64 {
        tx.send(i).unwrap();
    }

    for i in 0..64 {
        assert_eq!(rx.recv().unwrap(), i);
    }
}

#[test]
fn spsc_cross_thread_100k() {
    let ch = SpscChannel::<u64>::new(1024);
    let tx = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    let producer = thread::spawn(move || {
        for i in 0..100_000u64 {
            tx.send(i).unwrap();
        }
    });

    let consumer = thread::spawn(move || {
        for i in 0..100_000u64 {
            assert_eq!(rx.recv().unwrap(), i);
        }
    });

    producer.join().unwrap();
    consumer.join().unwrap();
}

#[test]
fn spsc_closure_after_drain() {
    let ch = SpscChannel::<u32>::new(4);
    let tx = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    tx.send(10).unwrap();
    tx.send(20).unwrap();
    drop(tx);

    assert_eq!(rx.recv().unwrap(), 10);
    assert_eq!(rx.recv().unwrap(), 20);
    assert_eq!(rx.recv().unwrap_err(), ChannelError::Closed);
}

#[test]
fn spsc_binding_enforcement() {
    let ch = SpscChannel::<u32>::new(4);

    let _tx = ch.sender().unwrap();
    let _rx = ch.receiver().unwrap();

    // Second sender rejected
    assert!(matches!(
        ch.sender().unwrap_err(),
        ChannelError::BindingRejected { .. }
    ));

    // Second receiver rejected
    assert!(matches!(
        ch.receiver().unwrap_err(),
        ChannelError::BindingRejected { .. }
    ));
}

#[test]
fn spsc_rebind_after_disconnect() {
    let ch = SpscChannel::<u32>::new(4);

    {
        let _tx = ch.sender().unwrap();
    } // tx dropped — slot freed

    let tx2 = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    tx2.send(42).unwrap();
    assert_eq!(rx.recv().unwrap(), 42);
}

#[test]
fn spsc_try_send_try_recv() {
    let ch = SpscChannel::<u32>::new(2);
    let tx = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    assert_eq!(rx.try_recv().unwrap_err(), ChannelError::Empty);

    tx.try_send(1).unwrap();
    tx.try_send(2).unwrap();
    assert_eq!(tx.try_send(3).unwrap_err(), ChannelError::Full);

    assert_eq!(rx.try_recv().unwrap(), 1);
    assert_eq!(rx.try_recv().unwrap(), 2);
    assert_eq!(rx.try_recv().unwrap_err(), ChannelError::Empty);
}
