use interfaces::{ExtentManagerError, FormatParams, IExtentManager};

use extent_manager_v2::test_support::create_test_component;

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

fn setup() -> std::sync::Arc<extent_manager_v2::ExtentManagerV2> {
    let (component, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
    component.format(format_params()).expect("format");
    component
}

#[test]
fn key_zero_valid() {
    let c = setup();
    let h = c.reserve_extent(0, 4096).expect("reserve key 0");
    h.publish().expect("publish key 0");
    let extents = c.get_extents();
    assert_eq!(extents.len(), 1);
    assert_eq!(extents[0].key, 0);
}

/// FREE_KEY (u64::MAX) publish is a silent no-op: succeeds but adds nothing.
#[test]
fn key_max_is_silent_discard() {
    let c = setup();
    let h = c.reserve_extent(u64::MAX, 4096).expect("reserve key MAX");
    h.publish().expect("publish key MAX returns Ok");
    assert!(c.get_extents().is_empty(), "FREE_KEY extent must not be stored");
}

#[test]
fn out_of_space_returns_error() {
    let small_disk: u64 = SLAB_SIZE + SECTOR_SIZE as u64 * 2;
    let (c, _metadata_mock) = create_test_component(METADATA_DISK_SIZE);
    c.format(FormatParams {
        data_disk_size: small_disk,
        slab_size: SLAB_SIZE,
        max_extent_size: MAX_EXTENT_SIZE,
        sector_size: SECTOR_SIZE,
        region_count: 1,
        metadata_alignment: METADATA_ALIGNMENT,
        instance_id: None,
        metadata_disk_ns_id: 1,
    })
    .expect("format");

    let slots_per_slab = SLAB_SIZE / SECTOR_SIZE as u64;
    let mut handles = Vec::new();
    for i in 0..slots_per_slab as u64 {
        handles.push(c.reserve_extent(i, SECTOR_SIZE).expect("reserve"));
    }

    match c.reserve_extent(999999, SECTOR_SIZE) {
        Err(ExtentManagerError::OutOfSpace) => {}
        other => panic!("expected OutOfSpace, got: {other:?}"),
    }
}

#[test]
fn dynamic_size_class_creation() {
    let c = setup();

    let e1 = c.reserve_extent(1, 4096).expect("reserve 4K").publish().expect("publish 4K");
    let e2 = c.reserve_extent(2, 8192).expect("reserve 8K").publish().expect("publish 8K");
    let e3 = c.reserve_extent(3, 16384).expect("reserve 16K").publish().expect("publish 16K");

    assert!(e1.size >= 4096);
    assert!(e2.size >= 8192);
    assert!(e3.size >= 16384);
}

#[test]
fn checkpoint_skip_when_not_dirty() {
    let c = setup();
    c.checkpoint().expect("noop checkpoint 1");
    c.checkpoint().expect("noop checkpoint 2");
}

#[test]
fn drop_with_outstanding_handles() {
    let c = setup();
    let _h1 = c.reserve_extent(1, 4096).expect("reserve 1");
    let _h2 = c.reserve_extent(2, 4096).expect("reserve 2");
    // Dropping handles should trigger abort without panic
}

#[test]
fn multiple_sequential_checkpoints() {
    let c = setup();

    for round in 0..5u64 {
        let key = round * 10;
        let h = c.reserve_extent(key, 4096).expect("reserve");
        h.publish().expect("publish");
        c.checkpoint().expect("checkpoint");
    }

    let keys: std::collections::HashSet<u64> = c.get_extents().iter().map(|e| e.key).collect();
    for round in 0..5u64 {
        let key = round * 10;
        assert!(keys.contains(&key), "key {key} missing after checkpoints");
    }
}
