//! Integration tests for IExtentManager API operations.
//!
//! All tests exercise the full component stack:
//! `IExtentManager` → `ExtentManagerComponentV1` → `BlockDevice` → `MockBlockDevice`

use std::sync::Arc;

use component_core::iunknown::query;
use extent_manager::test_support::{
    create_test_component, create_uninit_component, heap_dma_alloc_fn, MockBlockDevice,
};
use extent_manager::ExtentMetadata;
use interfaces::IExtentManagerAdmin;
use interfaces::{IBlockDevice, IExtentManager};

/// Default test geometry: 10,000 blocks, 2 size classes (128K, 256K), 100 slots each.
fn default_component() -> (
    Arc<extent_manager::ExtentManagerComponentV1>,
    Arc<MockBlockDevice>,
) {
    create_test_component(10_000, &[131072, 262144], &[100, 50])
}

// T010: create and lookup
#[test]
fn api_create_and_lookup() {
    let (comp, _mock) = default_component();

    let meta_bytes = comp
        .create_extent(1, 0, "test.dat", 0, false)
        .expect("create_extent failed");
    let meta = ExtentMetadata::from_bytes(&meta_bytes).unwrap();

    assert_eq!(meta.key, 1);
    assert_eq!(meta.size_class, 0);
    assert_eq!(meta.filename.as_deref(), Some("test.dat"));
    assert_eq!(comp.extent_count(), 1);

    let lookup_bytes = comp.lookup_extent(1).expect("lookup_extent failed");
    let lookup_meta = ExtentMetadata::from_bytes(&lookup_bytes).unwrap();
    assert_eq!(meta.key, lookup_meta.key);
    assert_eq!(meta.offset_blocks, lookup_meta.offset_blocks);
}

// T011: create and remove
#[test]
fn api_create_and_remove() {
    let (comp, _mock) = default_component();

    comp.create_extent(1, 0, "", 0, false).unwrap();
    assert_eq!(comp.extent_count(), 1);

    comp.remove_extent(1).unwrap();
    assert_eq!(comp.extent_count(), 0);

    // Lookup after removal should fail.
    let err = comp.lookup_extent(1).unwrap_err();
    assert!(err.to_string().contains("key not found"));
}

// T012: duplicate key error
#[test]
fn api_duplicate_key_error() {
    let (comp, _mock) = default_component();

    comp.create_extent(42, 0, "", 0, false).unwrap();
    let err = comp.create_extent(42, 0, "", 0, false).unwrap_err();
    assert!(err.to_string().contains("duplicate key"));
}

// T013: key not found error
#[test]
fn api_key_not_found_error() {
    let (comp, _mock) = default_component();

    let err = comp.lookup_extent(999).unwrap_err();
    assert!(err.to_string().contains("key not found"));

    let err = comp.remove_extent(999).unwrap_err();
    assert!(err.to_string().contains("key not found"));
}

// T014: invalid size class error
#[test]
fn api_invalid_size_class_error() {
    let (comp, _mock) = default_component();

    // Only size classes 0 and 1 are configured.
    let err = comp.create_extent(1, 99, "", 0, false).unwrap_err();
    assert!(err.to_string().contains("invalid size class"));
}

// T015: out of space error
#[test]
fn api_out_of_space_error() {
    // Small capacity: only 2 slots in class 0.
    let (comp, _mock) = create_test_component(10_000, &[131072], &[2]);

    comp.create_extent(1, 0, "", 0, false).unwrap();
    comp.create_extent(2, 0, "", 0, false).unwrap();

    let err = comp.create_extent(3, 0, "", 0, false).unwrap_err();
    assert!(err.to_string().contains("out of space"));
}

// T016: not initialized error
#[test]
fn api_not_initialized_error() {
    let (comp, _mock) = create_uninit_component(10_000);

    let err = comp.create_extent(1, 0, "", 0, false).unwrap_err();
    assert!(err.to_string().contains("not initialized"));
}

// T017: device too small error
#[test]
fn api_device_too_small_error() {
    // 2 blocks is way too small for any configuration.
    let mock = Arc::new(MockBlockDevice::new(2));
    let comp = extent_manager::ExtentManagerComponentV1::new_default();

    let ibd: Arc<dyn IBlockDevice + Send + Sync> = mock.clone();
    comp.block_device.connect(ibd).unwrap();
    let admin =
        query::<dyn IExtentManagerAdmin + Send + Sync>(&*comp).expect("IExtentManagerAdmin query");
    admin.set_dma_alloc(heap_dma_alloc_fn());

    let err = admin.initialize(vec![131072], vec![100], 1).unwrap_err();
    assert!(err.to_string().contains("too small"));
}

// T018: multiple size classes
#[test]
fn api_multiple_size_classes() {
    let (comp, _mock) = default_component();

    // Create one extent in each size class.
    let meta0_bytes = comp.create_extent(1, 0, "", 0, false).unwrap();
    let meta0 = ExtentMetadata::from_bytes(&meta0_bytes).unwrap();
    assert_eq!(meta0.size_class, 0);

    let meta1_bytes = comp.create_extent(2, 1, "", 0, false).unwrap();
    let meta1 = ExtentMetadata::from_bytes(&meta1_bytes).unwrap();
    assert_eq!(meta1.size_class, 1);

    assert_eq!(comp.extent_count(), 2);

    // Offsets should be different (different regions).
    assert_ne!(meta0.offset_blocks, meta1.offset_blocks);
}

// T019: filename and CRC round-trip
#[test]
fn api_filename_and_crc() {
    let (comp, _mock) = default_component();

    let meta_bytes = comp
        .create_extent(1, 0, "model.bin", 0xDEADBEEF, true)
        .unwrap();
    let meta = ExtentMetadata::from_bytes(&meta_bytes).unwrap();

    assert_eq!(meta.filename.as_deref(), Some("model.bin"));
    assert_eq!(meta.data_crc, Some(0xDEADBEEF));

    // Verify round-trip through lookup.
    let lookup_bytes = comp.lookup_extent(1).unwrap();
    let lookup = ExtentMetadata::from_bytes(&lookup_bytes).unwrap();
    assert_eq!(lookup.filename.as_deref(), Some("model.bin"));
    assert_eq!(lookup.data_crc, Some(0xDEADBEEF));
}

// T020: initialize and reopen
#[test]
fn api_initialize_and_reopen() {
    let num_blocks = 10_000u64;
    let sizes = &[131072u32, 262144];
    let slots = &[100u32, 50];

    // Phase 1: initialize and populate.
    let mock = Arc::new(MockBlockDevice::new(num_blocks));
    let comp = extent_manager::ExtentManagerComponentV1::new_default();

    let ibd: Arc<dyn IBlockDevice + Send + Sync> = mock.clone();
    comp.block_device.connect(ibd).unwrap();
    let admin =
        query::<dyn IExtentManagerAdmin + Send + Sync>(&*comp).expect("IExtentManagerAdmin query");
    admin.set_dma_alloc(heap_dma_alloc_fn());
    admin.initialize(sizes.to_vec(), slots.to_vec(), 1).unwrap();

    comp.create_extent(10, 0, "file_a.dat", 0, false).unwrap();
    comp.create_extent(20, 0, "file_b.dat", 0xCAFE, true)
        .unwrap();
    comp.create_extent(30, 1, "", 0, false).unwrap();
    assert_eq!(comp.extent_count(), 3);

    // Save block storage handle, drop component.
    let blocks = mock.blocks();
    drop(comp);

    // Phase 2: reopen from same storage.
    let mock2 = Arc::new(MockBlockDevice::reboot_from(&blocks, num_blocks));
    let comp2 = extent_manager::ExtentManagerComponentV1::new_default();

    let ibd2: Arc<dyn IBlockDevice + Send + Sync> = mock2;
    comp2.block_device.connect(ibd2).unwrap();
    let admin2 =
        query::<dyn IExtentManagerAdmin + Send + Sync>(&*comp2).expect("IExtentManagerAdmin query");
    admin2.set_dma_alloc(heap_dma_alloc_fn());

    let stats = admin2.open(1).unwrap();
    assert_eq!(stats.extents_loaded, 3);
    assert_eq!(stats.orphans_cleaned, 0);

    // Verify all extents recovered.
    assert_eq!(comp2.extent_count(), 3);

    let meta10 = ExtentMetadata::from_bytes(&comp2.lookup_extent(10).unwrap()).unwrap();
    assert_eq!(meta10.filename.as_deref(), Some("file_a.dat"));

    let meta20 = ExtentMetadata::from_bytes(&comp2.lookup_extent(20).unwrap()).unwrap();
    assert_eq!(meta20.data_crc, Some(0xCAFE));

    comp2.lookup_extent(30).unwrap();
}
