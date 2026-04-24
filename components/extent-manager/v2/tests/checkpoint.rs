use std::sync::Arc;

use interfaces::{ExtentManagerError, FormatParams, IBlockDevice, IExtentManager, ILogger};

use extent_manager_v2::test_support::{
    create_test_component, heap_dma_alloc, MockBlockDevice, MockLogger,
};

const DISK_SIZE: u64 = 64 * 1024 * 1024;
const SECTOR_SIZE: u32 = 4096;
const SLAB_SIZE: u32 = 1024 * 1024;
const MAX_ELEMENT_SIZE: u32 = 65536;
const METADATA_BLOCK_SIZE: u32 = 131072;

fn format_params() -> FormatParams {
    FormatParams {
        slab_size: SLAB_SIZE,
        max_element_size: MAX_ELEMENT_SIZE,
        metadata_block_size: METADATA_BLOCK_SIZE,
        sector_size: SECTOR_SIZE,
        region_count: 4,
    }
}

// ============================================================
// User Story 4: Checkpoint Metadata to Disk (T026)
// ============================================================

#[test]
fn checkpoint_persists_extents() {
    let (c, _mock) = create_test_component(DISK_SIZE);
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
    let (c, _mock) = create_test_component(DISK_SIZE);
    c.format(format_params()).expect("format");

    c.checkpoint().expect("first checkpoint (noop)");
    c.checkpoint().expect("second checkpoint (noop)");
}

#[test]
fn two_sequential_checkpoints() {
    let (c, _mock) = create_test_component(DISK_SIZE);
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
    let mock = Arc::new(MockBlockDevice::new(DISK_SIZE));
    let shared = mock.shared_state();

    {
        let c = extent_manager_v2::MetadataManager::new_inner();
        c.block_device
            .connect(mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
            .unwrap();
        c.logger
            .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
            .unwrap();
        c.set_dma_alloc(heap_dma_alloc());
        c.format(format_params()).expect("format");
    }

    let mock2 = Arc::new(MockBlockDevice::reboot_from(&shared));
    let c2 = extent_manager_v2::MetadataManager::new_inner();
    c2.block_device
        .connect(mock2 as Arc<dyn IBlockDevice + Send + Sync>)
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
    let mock = Arc::new(MockBlockDevice::new(DISK_SIZE));
    let shared = mock.shared_state();

    {
        let c = extent_manager_v2::MetadataManager::new_inner();
        c.block_device
            .connect(mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
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

    let mock2 = Arc::new(MockBlockDevice::reboot_from(&shared));
    let c2 = extent_manager_v2::MetadataManager::new_inner();
    c2.block_device
        .connect(mock2 as Arc<dyn IBlockDevice + Send + Sync>)
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
    let mock = Arc::new(MockBlockDevice::new(DISK_SIZE));
    let shared = mock.shared_state();

    {
        let c = extent_manager_v2::MetadataManager::new_inner();
        c.block_device
            .connect(mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
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

    let mock2 = Arc::new(MockBlockDevice::reboot_from(&shared));
    let c2 = extent_manager_v2::MetadataManager::new_inner();
    c2.block_device
        .connect(mock2 as Arc<dyn IBlockDevice + Send + Sync>)
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
fn corrupt_primary_falls_back_to_previous() {
    let mock = Arc::new(MockBlockDevice::new(DISK_SIZE));
    let shared = mock.shared_state();

    {
        let c = extent_manager_v2::MetadataManager::new_inner();
        c.block_device
            .connect(mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
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

    // Read the superblock to find current_index_lba and corrupt it
    {
        let state = shared.lock().unwrap();
        let sb_data = state.blocks.get(&0).cloned().unwrap_or_default();
        let sb = extent_manager_v2::superblock::Superblock::deserialize(&sb_data).unwrap();
        drop(state);

        let mut state = shared.lock().unwrap();
        if let Some(block) = state.blocks.get_mut(&sb.current_index_lba) {
            block[0] ^= 0xFF;
            block[1] ^= 0xFF;
        }
    }

    let mock2 = Arc::new(MockBlockDevice::reboot_from(&shared));
    let c2 = extent_manager_v2::MetadataManager::new_inner();
    c2.block_device
        .connect(mock2 as Arc<dyn IBlockDevice + Send + Sync>)
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
fn invalid_magic_returns_error() {
    let mock = Arc::new(MockBlockDevice::new(DISK_SIZE));
    {
        let shared = mock.shared_state();
        let mut state = shared.lock().unwrap();
        let mut bad_sb = vec![0u8; 4096];
        bad_sb[0..8].copy_from_slice(&0xDEADu64.to_le_bytes());
        state.blocks.insert(0, bad_sb);
    }

    let c = extent_manager_v2::MetadataManager::new_inner();
    c.block_device
        .connect(mock as Arc<dyn IBlockDevice + Send + Sync>)
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
