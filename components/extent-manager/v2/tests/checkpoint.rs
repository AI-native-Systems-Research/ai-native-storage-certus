use std::collections::HashSet;
use std::sync::Arc;

use interfaces::{ExtentManagerError, FormatParams, IBlockDevice, IExtentManager, ILogger};

use extent_manager_v2::test_support::{
    create_test_component, heap_dma_alloc, MockBlockDevice, MockLogger,
};

const DISK_SIZE: u64 = 64 * 1024 * 1024;
const METADATA_DISK_SIZE: u64 = 16 * 1024 * 1024;
const SECTOR_SIZE: u32 = 4096;
const SLAB_SIZE: u64 = 1024 * 1024;
const MAX_EXTENT_SIZE: u32 = 65536;
const METADATA_ALIGNMENT: u64 = 1048576;

fn format_params() -> FormatParams {
    FormatParams {
        data_disk_size: DISK_SIZE,
        slab_size: SLAB_SIZE,
        max_extent_size: MAX_EXTENT_SIZE,
        sector_size: SECTOR_SIZE,
        region_count: 4,
        metadata_alignment: METADATA_ALIGNMENT,
        instance_id: None,
        metadata_disk_ns_id: 1,
    }
}

// ============================================================
// User Story 4: Checkpoint Metadata to Disk (T026)
// ============================================================

#[test]
fn checkpoint_persists_extents() {
    let (c, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
    c.format(format_params()).expect("format");

    for k in 1..=10u64 {
        let h = c.reserve_extent(k, 4096).expect("reserve");
        h.publish().expect("publish");
    }

    c.checkpoint().expect("checkpoint");

    let keys: HashSet<u64> = c.get_extents().iter().map(|e| e.key).collect();
    for k in 1..=10u64 {
        assert!(keys.contains(&k), "key {k} missing after checkpoint");
    }
}

#[test]
fn checkpoint_skips_when_clean() {
    let (c, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
    c.format(format_params()).expect("format");

    c.checkpoint().expect("first checkpoint (noop)");
    c.checkpoint().expect("second checkpoint (noop)");
}

#[test]
fn two_sequential_checkpoints() {
    let (c, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
    c.format(format_params()).expect("format");

    let h = c.reserve_extent(1, 4096).expect("reserve");
    h.publish().expect("publish");
    c.checkpoint().expect("first checkpoint");

    let h = c.reserve_extent(2, 4096).expect("reserve");
    h.publish().expect("publish");
    c.checkpoint().expect("second checkpoint");

    let keys: HashSet<u64> = c.get_extents().iter().map(|e| e.key).collect();
    assert!(keys.contains(&1), "key 1 still present");
    assert!(keys.contains(&2), "key 2 present");
}

// ============================================================
// User Story 5: Initialize and Recover Metadata from Disk (T030)
// ============================================================

#[test]
fn format_fresh_then_initialize() {
    let metadata_mock = Arc::new(MockBlockDevice::new(METADATA_DISK_SIZE));
    let metadata_shared = metadata_mock.shared_state();

    {
        let c = extent_manager_v2::ExtentManagerV2::new_inner();
        c.metadata_device
            .connect(metadata_mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
            .unwrap();
        c.logger
            .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
            .unwrap();
        c.set_dma_alloc(heap_dma_alloc());
        c.format(format_params()).expect("format");
    }

    let metadata_mock2 = Arc::new(MockBlockDevice::reboot_from(&metadata_shared));
    let c2 = extent_manager_v2::ExtentManagerV2::new_inner();
    c2.metadata_device
        .connect(metadata_mock2 as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap();
    c2.logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c2.set_dma_alloc(heap_dma_alloc());

    c2.initialize().expect("initialize fresh device");
    assert!(c2.get_extents().is_empty());
}

#[test]
fn recover_checkpointed_extents() {
    let metadata_mock = Arc::new(MockBlockDevice::new(METADATA_DISK_SIZE));
    let metadata_shared = metadata_mock.shared_state();

    {
        let c = extent_manager_v2::ExtentManagerV2::new_inner();
        c.metadata_device
            .connect(metadata_mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
            .unwrap();
        c.logger
            .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
            .unwrap();
        c.set_dma_alloc(heap_dma_alloc());
        c.format(format_params()).expect("format");

        for k in 1..=100u64 {
            let h = c.reserve_extent(k, 4096).expect("reserve");
            h.publish().expect("publish");
        }
        c.checkpoint().expect("checkpoint");
    }

    let metadata_mock2 = Arc::new(MockBlockDevice::reboot_from(&metadata_shared));
    let c2 = extent_manager_v2::ExtentManagerV2::new_inner();
    c2.metadata_device
        .connect(metadata_mock2 as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap();
    c2.logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c2.set_dma_alloc(heap_dma_alloc());

    c2.initialize().expect("initialize");

    let recovered_keys: HashSet<u64> = c2.get_extents().iter().map(|e| e.key).collect();
    for k in 1..=100u64 {
        assert!(recovered_keys.contains(&k), "key {k} not recovered");
    }
}

#[test]
fn uncheckpointed_extents_lost_after_restart() {
    let metadata_mock = Arc::new(MockBlockDevice::new(METADATA_DISK_SIZE));
    let metadata_shared = metadata_mock.shared_state();

    {
        let c = extent_manager_v2::ExtentManagerV2::new_inner();
        c.metadata_device
            .connect(metadata_mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
            .unwrap();
        c.logger
            .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
            .unwrap();
        c.set_dma_alloc(heap_dma_alloc());
        c.format(format_params()).expect("format");

        for k in 1..=5u64 {
            let h = c.reserve_extent(k, 4096).expect("reserve");
            h.publish().expect("publish");
        }
        c.checkpoint().expect("checkpoint");

        for k in 6..=10u64 {
            let h = c.reserve_extent(k, 4096).expect("reserve");
            h.publish().expect("publish");
        }
        // no checkpoint for keys 6-10
    }

    let metadata_mock2 = Arc::new(MockBlockDevice::reboot_from(&metadata_shared));
    let c2 = extent_manager_v2::ExtentManagerV2::new_inner();
    c2.metadata_device
        .connect(metadata_mock2 as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap();
    c2.logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c2.set_dma_alloc(heap_dma_alloc());
    c2.initialize().expect("initialize");

    let recovered_keys: HashSet<u64> = c2.get_extents().iter().map(|e| e.key).collect();
    assert_eq!(recovered_keys.len(), 5, "only 5 checkpointed extents should survive");
    for k in 1..=5u64 {
        assert!(recovered_keys.contains(&k), "checkpointed key {k} missing");
    }
    for k in 6..=10u64 {
        assert!(!recovered_keys.contains(&k), "uncheckpointed key {k} should be lost");
    }
}

#[test]
fn corrupt_active_falls_back_to_previous() {
    let metadata_mock = Arc::new(MockBlockDevice::new(METADATA_DISK_SIZE));
    let metadata_shared = metadata_mock.shared_state();

    {
        let c = extent_manager_v2::ExtentManagerV2::new_inner();
        c.metadata_device
            .connect(metadata_mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
            .unwrap();
        c.logger
            .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
            .unwrap();
        c.set_dma_alloc(heap_dma_alloc());
        c.format(format_params()).expect("format");

        for k in 1..=5u64 {
            let h = c.reserve_extent(k, 4096).expect("reserve");
            h.publish().expect("publish");
        }
        c.checkpoint().expect("first checkpoint");

        for k in 6..=10u64 {
            let h = c.reserve_extent(k, 4096).expect("reserve");
            h.publish().expect("publish");
        }
        c.checkpoint().expect("second checkpoint");
    }

    // Corrupt the active checkpoint copy
    {
        let state = metadata_shared.lock().unwrap();
        let sb_data = state.blocks.get(&0).cloned().unwrap_or_default();
        let sb = extent_manager_v2::superblock::Superblock::deserialize(&sb_data).unwrap();
        drop(state);

        let active_offset =
            sb.checkpoint_region_offset + sb.active_copy as u64 * sb.checkpoint_region_size;
        let active_lba = active_offset / SECTOR_SIZE as u64;

        let mut state = metadata_shared.lock().unwrap();
        if let Some(block) = state.blocks.get_mut(&active_lba) {
            block[0] ^= 0xFF;
            block[1] ^= 0xFF;
        }
    }

    let metadata_mock2 = Arc::new(MockBlockDevice::reboot_from(&metadata_shared));
    let c2 = extent_manager_v2::ExtentManagerV2::new_inner();
    c2.metadata_device
        .connect(metadata_mock2 as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap();
    c2.logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c2.set_dma_alloc(heap_dma_alloc());

    c2.initialize().expect("initialize with fallback");

    let recovered_keys: HashSet<u64> = c2.get_extents().iter().map(|e| e.key).collect();
    for k in 1..=5u64 {
        assert!(recovered_keys.contains(&k), "key {k} from previous checkpoint missing");
    }
}

#[test]
fn remove_realloc_crash_does_not_corrupt() {
    let metadata_mock = Arc::new(MockBlockDevice::new(METADATA_DISK_SIZE));
    let metadata_shared = metadata_mock.shared_state();

    let original_offset;
    {
        let c = extent_manager_v2::ExtentManagerV2::new_inner();
        c.metadata_device
            .connect(metadata_mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
            .unwrap();
        c.logger
            .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
            .unwrap();
        c.set_dma_alloc(heap_dma_alloc());
        c.format(format_params()).unwrap();

        let h = c.reserve_extent(1, SECTOR_SIZE).unwrap();
        let ext = h.publish().unwrap();
        original_offset = ext.offset;
        c.checkpoint().unwrap();

        c.remove_extent(ext.offset).unwrap();

        let h2 = c.reserve_extent(2, SECTOR_SIZE).unwrap();
        let ext2 = h2.publish().unwrap();
        assert_ne!(
            ext2.offset, original_offset,
            "removed slot must not be reused before checkpoint"
        );
    }

    let metadata_mock2 = Arc::new(MockBlockDevice::reboot_from(&metadata_shared));
    let c2 = extent_manager_v2::ExtentManagerV2::new_inner();
    c2.metadata_device
        .connect(metadata_mock2 as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap();
    c2.logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c2.set_dma_alloc(heap_dma_alloc());
    c2.initialize().unwrap();

    // After recovery the original extent (key=1) should still exist at the same offset.
    let extents = c2.get_extents();
    let recovered = extents
        .iter()
        .find(|e| e.key == 1)
        .expect("key 1 should be recovered");
    assert_eq!(recovered.offset, original_offset);
}

#[test]
fn remove_then_checkpoint_frees_slot() {
    let (c, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
    c.format(format_params()).unwrap();

    let h = c.reserve_extent(1, SECTOR_SIZE).unwrap();
    let ext = h.publish().unwrap();
    let original_offset = ext.offset;
    c.checkpoint().unwrap();

    c.remove_extent(ext.offset).unwrap();
    c.checkpoint().unwrap();

    // key 5 maps to region 1 (5 & 3 == 1), same as key 1, so the freed slot
    // in that region should be reused.
    let h2 = c.reserve_extent(5, SECTOR_SIZE).unwrap();
    let ext2 = h2.publish().unwrap();
    assert_eq!(
        ext2.offset, original_offset,
        "slot should be reused after checkpoint persisted the removal"
    );
}

#[test]
fn invalid_magic_returns_error() {
    let metadata_mock = Arc::new(MockBlockDevice::new(METADATA_DISK_SIZE));
    {
        let mock = metadata_mock.shared_state();
        let mut state = mock.lock().unwrap();
        let mut bad_sb = vec![0u8; 4096];
        bad_sb[0..8].copy_from_slice(&0xDEADu64.to_le_bytes());
        state.blocks.insert(0, bad_sb);
    }

    let c = extent_manager_v2::ExtentManagerV2::new_inner();
    c.metadata_device
        .connect(metadata_mock as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap();
    c.logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c.set_dma_alloc(heap_dma_alloc());

    match c.initialize() {
        Err(ExtentManagerError::CorruptMetadata(msg)) => {
            assert!(msg.contains("magic"));
        }
        other => panic!("expected CorruptMetadata, got: {other:?}"),
    }
}

// ============================================================
// Background checkpoint interval tests
// ============================================================

#[test]
fn background_checkpoint_fires_automatically() {
    use std::time::Duration;

    let metadata_mock = Arc::new(MockBlockDevice::new(METADATA_DISK_SIZE));
    let metadata_shared = metadata_mock.shared_state();

    {
        let c = extent_manager_v2::ExtentManagerV2::new_inner();
        c.metadata_device
            .connect(metadata_mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
            .unwrap();
        c.logger
            .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
            .unwrap();
        c.set_dma_alloc(heap_dma_alloc());
        c.format(format_params()).expect("format");

        let h = c.reserve_extent(42, 4096).expect("reserve");
        h.publish().expect("publish");

        // Trigger automatic checkpointing quickly.
        c.set_checkpoint_interval(Some(Duration::from_millis(50)));
        std::thread::sleep(Duration::from_millis(300));
        // c drops here; Drop signals the background thread and joins it.
    }

    // Recover on a fresh component — the background checkpoint must have run.
    let metadata_mock2 = Arc::new(MockBlockDevice::reboot_from(&metadata_shared));
    let c2 = extent_manager_v2::ExtentManagerV2::new_inner();
    c2.metadata_device
        .connect(metadata_mock2 as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap();
    c2.logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c2.set_dma_alloc(heap_dma_alloc());
    c2.initialize().expect("initialize");

    let extents = c2.get_extents();
    assert_eq!(extents.len(), 1);
    assert_eq!(extents[0].key, 42);
}

#[test]
fn set_checkpoint_interval_none_disables_background_checkpoints() {
    use std::time::Duration;

    let metadata_mock = Arc::new(MockBlockDevice::new(METADATA_DISK_SIZE));
    let metadata_shared = metadata_mock.shared_state();

    {
        let c = extent_manager_v2::ExtentManagerV2::new_inner();
        c.metadata_device
            .connect(metadata_mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
            .unwrap();
        c.logger
            .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
            .unwrap();
        c.set_dma_alloc(heap_dma_alloc());
        c.format(format_params()).expect("format");

        // Disable automatic checkpoints.
        c.set_checkpoint_interval(None);

        let h = c.reserve_extent(99, 4096).expect("reserve");
        h.publish().expect("publish");

        // Wait long enough that a 50ms background checkpoint would have fired.
        std::thread::sleep(Duration::from_millis(200));
        // No checkpoint should have run — extent will be lost after reboot.
    }

    let metadata_mock2 = Arc::new(MockBlockDevice::reboot_from(&metadata_shared));
    let c2 = extent_manager_v2::ExtentManagerV2::new_inner();
    c2.metadata_device
        .connect(metadata_mock2 as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap();
    c2.logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c2.set_dma_alloc(heap_dma_alloc());
    c2.initialize().expect("initialize");

    // Extent was not checkpointed, so it is lost after reboot.
    assert!(c2.get_extents().is_empty());
}
