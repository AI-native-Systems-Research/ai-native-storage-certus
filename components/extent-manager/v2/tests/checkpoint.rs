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

    for k in 1..=10u64 {
        c.lookup_extent(k).expect("lookup after checkpoint");
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

    c.lookup_extent(1).expect("key 1 still present");
    c.lookup_extent(2).expect("key 2 present");
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

    for k in 1..=100u64 {
        let ext = c2.lookup_extent(k).expect(&format!("lookup key {k}"));
        assert_eq!(ext.key, k);
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

    for k in 1..=5u64 {
        c2.lookup_extent(k).expect(&format!("checkpointed key {k}"));
    }

    for k in 6..=10u64 {
        match c2.lookup_extent(k) {
            Err(ExtentManagerError::KeyNotFound(_)) => {}
            other => panic!("expected KeyNotFound for uncheckpointed key {k}, got: {other:?}"),
        }
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

    // Read superblock to find the active copy and corrupt it
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

    for k in 1..=5u64 {
        c2.lookup_extent(k)
            .expect(&format!("key {k} from previous checkpoint"));
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

        c.remove_extent(1).unwrap();

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

    let recovered = c2.lookup_extent(1).unwrap();
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

    c.remove_extent(1).unwrap();
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
