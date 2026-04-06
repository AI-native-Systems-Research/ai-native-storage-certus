//! Integration tests for MPSC channel.

use component_framework::channel::mpsc::MpscChannel;
use component_framework::channel::ChannelError;
use std::thread;

#[test]
fn mpsc_multiple_senders_allowed() {
    let ch = MpscChannel::<u32>::new(16);
    let _tx1 = ch.sender().unwrap();
    let _tx2 = ch.sender().unwrap();
    let _tx3 = ch.sender().unwrap();
}

#[test]
fn mpsc_second_receiver_rejected() {
    let ch = MpscChannel::<u32>::new(4);
    let _rx = ch.receiver().unwrap();
    assert!(matches!(
        ch.receiver().unwrap_err(),
        ChannelError::BindingRejected { .. }
    ));
}

#[test]
fn mpsc_concurrent_multi_producer_delivery() {
    let ch = MpscChannel::<u64>::new(4096);
    let rx = ch.receiver().unwrap();

    let mut handles = vec![];
    for pid in 0..8u64 {
        let tx = ch.sender().unwrap();
        handles.push(thread::spawn(move || {
            for i in 0..10_000u64 {
                tx.send(pid * 10_000 + i).unwrap();
            }
        }));
    }

    let consumer = thread::spawn(move || {
        let mut received = Vec::with_capacity(80_000);
        loop {
            match rx.recv() {
                Ok(val) => received.push(val),
                Err(ChannelError::Closed) => break,
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        }
        received
    });

    for h in handles {
        h.join().unwrap();
    }
    drop(ch);

    let received = consumer.join().unwrap();
    assert_eq!(received.len(), 80_000);
}

#[test]
fn mpsc_closure_when_all_senders_dropped() {
    let ch = MpscChannel::<u32>::new(4);
    let tx1 = ch.sender().unwrap();
    let tx2 = ch.sender().unwrap();
    let rx = ch.receiver().unwrap();

    tx1.send(1).unwrap();
    tx2.send(2).unwrap();

    drop(tx1);
    drop(tx2);
    drop(ch);

    let mut msgs = vec![];
    loop {
        match rx.recv() {
            Ok(v) => msgs.push(v),
            Err(ChannelError::Closed) => break,
            Err(e) => panic!("unexpected: {e:?}"),
        }
    }

    msgs.sort();
    assert_eq!(msgs, vec![1, 2]);
}

#[test]
fn mpsc_receiver_rebind_after_disconnect() {
    let ch = MpscChannel::<u32>::new(4);
    {
        let _rx = ch.receiver().unwrap();
    }
    let _rx2 = ch.receiver().unwrap();
}
