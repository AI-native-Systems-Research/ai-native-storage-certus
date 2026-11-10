//! Repair test for FORCE-FLUSH-DURABLE (finding `failed-flush-reported-durable`).
//!
//! With the documented trigger (`move || flush_manager.trigger_flush()`)
//! installed, a `force_flush()` whose disk write fails must return
//! `StorageError`, not `Ok`: the data is not durable.

use super::{FlushConfig, FlushManager};
use crate::block_io::BlockDeviceClient;
use crate::on_disk::Superblock;
use crate::test_support::{heap_dma_alloc, FaultConfig, MockBlockDevice};
use crate::ExtendedMetadataStoreComponent;
use interfaces::{ExtendedMetadataStoreError, IBlockDevice, IExtendedMetadataStore};
use std::sync::Arc;
use std::time::Duration;

const DISK_SIZE: u64 = 16 * 1024 * 1024;
const SECTOR_SIZE: u32 = 4096;

fn wire(mock: &Arc<MockBlockDevice>) -> Arc<ExtendedMetadataStoreComponent> {
    let comp = ExtendedMetadataStoreComponent::new_default();
    let channels = mock.connect_client().unwrap();
    let sector_size = mock.sector_size(1).unwrap();
    let client = BlockDeviceClient::new(channels, heap_dma_alloc(), sector_size, 1, 0);
    let (c1, c2, c3) = (comp.clone(), comp.clone(), comp.clone());
    let mgr = FlushManager::start(
        FlushConfig {
            // Only explicit flushes during the test.
            interval: Duration::from_secs(3600),
            dirty_threshold: 1_000_000,
        },
        client,
        Superblock::new(SECTOR_SIZE, DISK_SIZE / SECTOR_SIZE as u64),
        Box::new(move || c1.snapshot_entries()),
        Box::new(move || c2.dirty_count()),
        Box::new(move |seq| c3.mark_flushed(seq)),
    );
    comp.attach_flush_trigger(Box::new(move || mgr.trigger_flush()));
    comp
}

#[test]
fn force_flush_reports_failed_disk_write() {
    // Every device write fails.
    let mock = MockBlockDevice::with_fault_config(
        DISK_SIZE,
        FaultConfig {
            fail_after_n_writes: Some(0),
        },
    );
    let comp = wire(&mock);
    comp.put("a", b"1").unwrap();
    let res = comp.force_flush();
    assert!(
        matches!(res, Err(ExtendedMetadataStoreError::StorageError(_))),
        "force_flush() must return StorageError when the flush fails, got {res:?}"
    );
    assert!(
        comp.dirty_count() > 0,
        "a failed flush must leave the write dirty"
    );
}

#[test]
fn force_flush_ok_on_healthy_device() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = wire(&mock);
    comp.put("a", b"1").unwrap();
    assert_eq!(comp.force_flush(), Ok(()));
    assert_eq!(comp.dirty_count(), 0);
    // Nothing dirty: still Ok.
    assert_eq!(comp.force_flush(), Ok(()));
}
