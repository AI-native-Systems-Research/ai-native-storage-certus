//! Simulated power-failure tests for crash-consistency guarantees.
//!
//! These tests use `FaultConfig` to fail writes at precise points during
//! the two-phase create protocol (record write → bitmap persist), then
//! verify correct behavior on re-open via `open()`.

use std::sync::Arc;

use component_core::iunknown::query;
use extent_manager::test_support::{create_test_component, heap_dma_alloc_fn, MockBlockDevice};
use extent_manager::ExtentMetadata;
use interfaces::IExtentManagerAdmin;
use interfaces::{IBlockDevice, IExtentManager};

/// Helper to create and initialize a component returning the mock handle.
fn init_component(
    num_blocks: u64,
    sizes: &[u32],
    slots: &[u32],
) -> (
    Arc<extent_manager::ExtentManagerComponentV1>,
    Arc<MockBlockDevice>,
) {
    create_test_component(num_blocks, sizes, slots)
}

/// Helper to reopen from existing block storage.
fn reopen_component(
    blocks: &Arc<std::sync::Mutex<std::collections::HashMap<u64, [u8; 4096]>>>,
    num_blocks: u64,
) -> (
    Arc<extent_manager::ExtentManagerComponentV1>,
    Arc<MockBlockDevice>,
    interfaces::RecoveryResult,
) {
    let mock2 = Arc::new(MockBlockDevice::reboot_from(blocks, num_blocks));
    let comp2 = extent_manager::ExtentManagerComponentV1::new_default();

    let ibd2: Arc<dyn IBlockDevice + Send + Sync> = mock2.clone();
    comp2.block_device.connect(ibd2).unwrap();
    let admin =
        query::<dyn IExtentManagerAdmin + Send + Sync>(&*comp2).expect("IExtentManagerAdmin query");
    admin.set_dma_alloc(heap_dma_alloc_fn());
    let stats = admin.open(1).unwrap();
    (comp2, mock2, stats)
}

// T021: orphan after record write succeeds but bitmap fails
#[test]
fn crash_orphan_after_record_write() {
    let num_blocks = 10_000u64;
    let (comp, mock) = init_component(num_blocks, &[131072], &[100]);

    // After initialization, the superblock + bitmap writes are done.
    // Now configure fault injection: allow exactly the record write
    // (1 write), then fail the bitmap persist.
    {
        let fc_handle = mock.fault_config();
        let mut fc = fc_handle.lock().unwrap();
        fc.fail_after_n_writes = Some(1);
    }

    // create_extent does: (1) write record block, (2) persist bitmap block.
    // With fail_after_n_writes=1: write #1 (record) succeeds, write #2 (bitmap) fails.
    let result = comp.create_extent(1, 0, "", 0, false);
    assert!(result.is_err(), "bitmap write should have failed");

    // Save block storage and reopen.
    let blocks = mock.blocks();
    drop(comp);

    let (comp2, _mock2, stats) = reopen_component(&blocks, num_blocks);

    // The record exists on disk but bitmap bit is not set → orphan.
    assert!(
        stats.orphans_cleaned >= 1,
        "expected orphan cleanup, got: {stats:?}"
    );
    // The extent should NOT be in the index after recovery.
    assert_eq!(comp2.extent_count(), 0);
}

// T022: consistency after bitmap fail on remove
#[test]
fn crash_consistency_after_bitmap_fail_on_remove() {
    let num_blocks = 10_000u64;
    let (comp, mock) = init_component(num_blocks, &[131072], &[100]);

    // Successfully create an extent first.
    comp.create_extent(1, 0, "test.dat", 0, false).unwrap();
    assert_eq!(comp.extent_count(), 1);

    // Now set fault to fail the bitmap persist during remove.
    // remove_extent does: (1) bitmap clear + persist. One write.
    {
        let fc_handle = mock.fault_config();
        let mut fc = fc_handle.lock().unwrap();
        fc.fail_all_writes = true;
    }

    let result = comp.remove_extent(1);
    assert!(result.is_err(), "bitmap write during remove should fail");

    // Save and reopen.
    let blocks = mock.blocks();
    drop(comp);

    let (comp2, _mock2, stats) = reopen_component(&blocks, num_blocks);

    // The bitmap bit is still set (write failed), so the extent should be recovered.
    assert_eq!(stats.extents_loaded, 1);
    assert_eq!(comp2.extent_count(), 1);

    // Verify the extent is still there with correct metadata.
    let meta_bytes = comp2.lookup_extent(1).unwrap();
    let meta = ExtentMetadata::from_bytes(&meta_bytes).unwrap();
    assert_eq!(meta.key, 1);
}

// T023: recovery after clean shutdown
#[test]
fn crash_recovery_after_clean_shutdown() {
    let num_blocks = 10_000u64;
    let (comp, mock) = init_component(num_blocks, &[131072, 262144], &[100, 50]);

    comp.create_extent(10, 0, "a.dat", 0, false).unwrap();
    comp.create_extent(20, 0, "b.dat", 0xBEEF, true).unwrap();
    comp.create_extent(30, 1, "c.dat", 0, false).unwrap();
    assert_eq!(comp.extent_count(), 3);

    let blocks = mock.blocks();
    drop(comp);

    let (comp2, _mock2, stats) = reopen_component(&blocks, num_blocks);

    assert_eq!(stats.extents_loaded, 3);
    assert_eq!(stats.orphans_cleaned, 0);
    assert_eq!(stats.corrupt_records, 0);
    assert_eq!(comp2.extent_count(), 3);

    // Verify each extent's metadata.
    let m10 = ExtentMetadata::from_bytes(&comp2.lookup_extent(10).unwrap()).unwrap();
    assert_eq!(m10.filename.as_deref(), Some("a.dat"));

    let m20 = ExtentMetadata::from_bytes(&comp2.lookup_extent(20).unwrap()).unwrap();
    assert_eq!(m20.data_crc, Some(0xBEEF));

    let m30 = ExtentMetadata::from_bytes(&comp2.lookup_extent(30).unwrap()).unwrap();
    assert_eq!(m30.size_class, 1);
}

// T024: recovery statistics
#[test]
fn crash_recovery_statistics() {
    let num_blocks = 10_000u64;
    let (comp, mock) = init_component(num_blocks, &[131072], &[100]);

    // Create 5 extents.
    for key in 1..=5 {
        comp.create_extent(key, 0, "", 0, false).unwrap();
    }
    assert_eq!(comp.extent_count(), 5);

    let blocks = mock.blocks();
    drop(comp);

    let (_comp2, _mock2, stats) = reopen_component(&blocks, num_blocks);

    assert_eq!(stats.extents_loaded, 5);
    assert_eq!(stats.orphans_cleaned, 0);
    assert_eq!(stats.corrupt_records, 0);
}

// T025: corrupt superblock on open
#[test]
fn crash_corrupt_superblock_on_open() {
    let num_blocks = 10_000u64;
    let (comp, mock) = init_component(num_blocks, &[131072], &[100]);

    let blocks = mock.blocks();
    drop(comp);

    // Corrupt block 0 (superblock).
    {
        let mut blocks_guard = blocks.lock().unwrap();
        let garbage = [0xFFu8; 4096];
        blocks_guard.insert(0, garbage);
    }

    // Try to reopen — should fail with corrupt metadata error.
    let mock2 = Arc::new(MockBlockDevice::reboot_from(&blocks, num_blocks));
    let comp2 = extent_manager::ExtentManagerComponentV1::new_default();

    let ibd2: Arc<dyn IBlockDevice + Send + Sync> = mock2;
    comp2.block_device.connect(ibd2).unwrap();
    let admin2 =
        query::<dyn IExtentManagerAdmin + Send + Sync>(&*comp2).expect("IExtentManagerAdmin query");
    admin2.set_dma_alloc(heap_dma_alloc_fn());

    let err = admin2.open(1).unwrap_err();
    assert!(
        err.to_string().contains("corrupt metadata")
            || err.to_string().contains("magic")
            || err.to_string().contains("checksum"),
        "expected corruption error, got: {err}"
    );
}
