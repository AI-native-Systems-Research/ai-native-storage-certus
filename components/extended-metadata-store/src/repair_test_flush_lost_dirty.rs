//! Repair test for FLUSH-COALESCE (finding `flush-lost-dirty`).
//!
//! Caller A writes "a" and calls `trigger_flush`. While the worker is inside
//! that flush (after the snapshot, before `mark_flushed`), caller B writes "b"
//! and calls `trigger_flush`. When B's `trigger_flush` returns, "b" must be on
//! disk: B must not be released by A's flush (whose snapshot predates "b"), and
//! the flush must not wipe B's dirty mark.

use super::{FlushConfig, FlushManager};
use crate::block_io::BlockDeviceClient;
use crate::on_disk::Superblock;
use crate::recovery::recover_from_disk;
use crate::test_support::{heap_dma_alloc, MockBlockDevice};
use crate::ExtendedMetadataStoreComponent;
use interfaces::{IBlockDevice, IExtendedMetadataStore};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const DISK_SIZE: u64 = 16 * 1024 * 1024;
const SECTOR_SIZE: u32 = 4096;

fn connect_client(mock: &Arc<MockBlockDevice>) -> BlockDeviceClient {
    let channels = mock.connect_client().unwrap();
    let sector_size = mock.sector_size(1).unwrap();
    BlockDeviceClient::new(channels, heap_dma_alloc(), sector_size, 1, 0)
}

#[test]
fn write_during_flush_is_durable_when_its_trigger_flush_returns() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let superblock = Superblock::new(SECTOR_SIZE, num_sectors);

    // B is started from inside the first snapshot; the worker waits until B
    // has written "b", then gives B time to enter trigger_flush.
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (put_done_tx, put_done_rx) = mpsc::channel::<()>();
    let go_tx = Mutex::new(Some(go_tx));
    let first = AtomicBool::new(true);

    let c1 = comp.clone();
    let c2 = comp.clone();
    let c3 = comp.clone();
    let mgr = Arc::new(FlushManager::start(
        FlushConfig {
            interval: Duration::from_secs(3600),
            dirty_threshold: 10_000,
        },
        connect_client(&mock),
        superblock,
        Box::new(move || {
            let snap = c1.snapshot_entries();
            if first.swap(false, Ordering::SeqCst) {
                let tx = go_tx.lock().unwrap().take().unwrap();
                tx.send(()).unwrap();
                put_done_rx.recv().unwrap();
                std::thread::sleep(Duration::from_millis(100));
            }
            snap
        }),
        Box::new(move || c2.dirty_count()),
        Box::new(move |seq| c3.mark_flushed(seq)),
    ));

    let comp_b = comp.clone();
    let mgr_b = mgr.clone();
    let b = std::thread::spawn(move || {
        go_rx.recv().unwrap();
        comp_b.put("b", b"vb").unwrap();
        put_done_tx.send(()).unwrap();
        mgr_b.trigger_flush().unwrap();
        // B's trigger_flush returned: "b" must be on disk now.
        let client = connect_client(&MockBlockDevice::reboot_from(mock.shared_state()));
        let on_disk = recover_from_disk(&client).unwrap().entries;
        on_disk.iter().any(|e| e.0 == "b" && e.1 == b"vb")
    });

    comp.put("a", b"va").unwrap();
    mgr.trigger_flush().unwrap();
    let b_durable = b.join().unwrap();
    assert!(
        b_durable,
        "trigger_flush returned before a flush containing the caller's write was on disk"
    );
}

#[test]
fn write_during_flush_stays_dirty() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let superblock = Superblock::new(SECTOR_SIZE, num_sectors);

    let first = AtomicBool::new(true);
    let c1 = comp.clone();
    let c2 = comp.clone();
    let c3 = comp.clone();
    let mgr = FlushManager::start(
        FlushConfig {
            interval: Duration::from_secs(3600),
            dirty_threshold: 10_000,
        },
        connect_client(&mock),
        superblock,
        Box::new(move || {
            let snap = c1.snapshot_entries();
            if first.swap(false, Ordering::SeqCst) {
                // A write lands after the snapshot, before the flush completes.
                c1.put("late", b"x").unwrap();
            }
            snap
        }),
        Box::new(move || c2.dirty_count()),
        Box::new(move |seq| c3.mark_flushed(seq)),
    );

    comp.put("a", b"va").unwrap();
    mgr.trigger_flush().unwrap();
    assert_eq!(
        comp.dirty_count(),
        1,
        "the write made during the flush is not in it and must stay dirty"
    );
    // A later force-flush must persist it.
    mgr.trigger_flush().unwrap();
    let client = connect_client(&MockBlockDevice::reboot_from(mock.shared_state()));
    let on_disk = recover_from_disk(&client).unwrap().entries;
    assert!(
        on_disk.iter().any(|e| e.0 == "late"),
        "late write never flushed"
    );
    drop(mgr);
}
