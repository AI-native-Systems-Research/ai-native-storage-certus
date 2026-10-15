//! Repair test for FLUSH-BG-TRIGGERS (finding `flush-threshold-unimplemented`).
//!
//! With a timer interval far longer than the test, reaching
//! `FlushConfig::dirty_threshold` must start a background flush by itself -
//! whether the threshold is reached before the manager starts or while its
//! worker is already asleep.

use super::{FlushConfig, FlushManager};
use crate::block_io::BlockDeviceClient;
use crate::on_disk::Superblock;
use crate::test_support::{heap_dma_alloc, MockBlockDevice};
use crate::ExtendedMetadataStoreComponent;
use interfaces::{IBlockDevice, IExtendedMetadataStore};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DISK_SIZE: u64 = 16 * 1024 * 1024;
const SECTOR_SIZE: u32 = 4096;
const THRESHOLD: u64 = 3;

fn connect_client(mock: &Arc<MockBlockDevice>) -> BlockDeviceClient {
    let channels = mock.connect_client().unwrap();
    let sector_size = mock.sector_size(1).unwrap();
    BlockDeviceClient::new(channels, heap_dma_alloc(), sector_size, 1, 0)
}

fn start(comp: &Arc<ExtendedMetadataStoreComponent>, mock: &Arc<MockBlockDevice>) -> FlushManager {
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let (c1, c2, c3) = (comp.clone(), comp.clone(), comp.clone());
    FlushManager::start(
        FlushConfig {
            // The timer never fires during the test.
            interval: Duration::from_secs(3600),
            dirty_threshold: THRESHOLD,
        },
        connect_client(mock),
        Superblock::new(SECTOR_SIZE, num_sectors),
        Box::new(move || c1.snapshot_entries()),
        Box::new(move || c2.dirty_count()),
        Box::new(move |seq| c3.mark_flushed(seq)),
    )
}

fn put_n(comp: &ExtendedMetadataStoreComponent, n: u64) {
    for i in 0..n {
        comp.put(&format!("k{i}"), &[i as u8]).unwrap();
    }
}

fn wait_flushed(mgr: &FlushManager, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if mgr.completed_seq() >= 1 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    mgr.completed_seq() >= 1
}

#[test]
fn threshold_reached_before_start_flushes_without_timer() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    put_n(&comp, THRESHOLD);
    let mgr = start(&comp, &mock);
    assert!(
        wait_flushed(&mgr, Duration::from_secs(2)),
        "dirty count reached the threshold before start, but no flush happened within 2 s"
    );
    drop(mgr);
}

#[test]
fn threshold_reached_while_worker_sleeps_flushes_without_timer() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let mgr = start(&comp, &mock);
    // Let the worker go to sleep first.
    std::thread::sleep(Duration::from_millis(50));
    put_n(&comp, THRESHOLD);
    assert!(
        wait_flushed(&mgr, Duration::from_secs(2)),
        "dirty count reached the threshold while the worker slept, but no flush happened within 2 s"
    );
    assert_eq!(comp.dirty_count(), 0, "flushed mutations must be cleared");
    drop(mgr);
}

#[test]
fn below_threshold_does_not_flush_before_timer() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let mgr = start(&comp, &mock);
    put_n(&comp, THRESHOLD - 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(mgr.completed_seq(), 0, "flushed below the threshold");
    drop(mgr);
}
