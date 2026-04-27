use interfaces::{ExtentManagerError, FormatParams, IExtentManager};

use extent_manager_v2::test_support::create_test_component;

const DISK_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB
const METADATA_DISK_SIZE: u64 = 16 * 1024 * 1024; // 16 MiB
const SECTOR_SIZE: u32 = 4096;
const SLAB_SIZE: u64 = 1024 * 1024; // 1 MiB
const MAX_EXTENT_SIZE: u32 = 65536;
const METADATA_ALIGNMENT: u64 = 1048576; // 1 MiB

fn format_params() -> FormatParams {
    FormatParams {
        data_disk_size: DISK_SIZE,
        slab_size: SLAB_SIZE,
        max_extent_size: MAX_EXTENT_SIZE,
        sector_size: SECTOR_SIZE,
        region_count: 4,
        metadata_alignment: METADATA_ALIGNMENT,
    }
}

fn setup() -> std::sync::Arc<extent_manager_v2::ExtentManagerV2> {
    let (component, _data_mock, _metadata_mock) = create_test_component(DISK_SIZE, METADATA_DISK_SIZE);
    component.format(format_params()).expect("format");
    component
}

// ============================================================
// User Story 1: Reserve, Write, and Publish a File (T014)
// ============================================================

#[test]
fn reserve_publish_lookup_round_trip() {
    let c = setup();
    let handle = c.reserve_extent(42, 4096).expect("reserve");
    assert_eq!(handle.key(), 42);
    assert!(handle.extent_size() >= 4096);

    let extent = handle.publish().expect("publish");
    assert_eq!(extent.key, 42);

    let looked_up = c.lookup_extent(42).expect("lookup");
    assert_eq!(looked_up.key, 42);
    assert_eq!(looked_up.offset, extent.offset);
    assert_eq!(looked_up.size, extent.size);
}

#[test]
fn multiple_distinct_keys() {
    let c = setup();

    for key in [1, 2, 3, 100, u64::MAX] {
        let handle = c.reserve_extent(key, 4096).expect("reserve");
        handle.publish().expect("publish");
    }

    for key in [1, 2, 3, 100, u64::MAX] {
        let ext = c.lookup_extent(key).expect("lookup");
        assert_eq!(ext.key, key);
    }
}

#[test]
fn duplicate_key_at_publish() {
    let c = setup();

    let h1 = c.reserve_extent(42, 4096).expect("reserve first");
    let h2 = c.reserve_extent(42, 4096).expect("reserve second");

    h1.publish().expect("first publish succeeds");

    match h2.publish() {
        Err(ExtentManagerError::DuplicateKey(k)) => assert_eq!(k, 42),
        other => panic!("expected DuplicateKey, got: {other:?}"),
    }

    let ext = c.lookup_extent(42).expect("lookup");
    assert_eq!(ext.key, 42);
}

#[test]
fn key_zero_is_valid() {
    let c = setup();
    let handle = c.reserve_extent(0, 4096).expect("reserve key 0");
    handle.publish().expect("publish key 0");
    let ext = c.lookup_extent(0).expect("lookup key 0");
    assert_eq!(ext.key, 0);
}

#[test]
fn key_max_is_valid() {
    let c = setup();
    let handle = c.reserve_extent(u64::MAX, 4096).expect("reserve key MAX");
    handle.publish().expect("publish key MAX");
    let ext = c.lookup_extent(u64::MAX).expect("lookup key MAX");
    assert_eq!(ext.key, u64::MAX);
}

// ============================================================
// User Story 1: OutOfSpace (T015)
// ============================================================

#[test]
fn out_of_space() {
    let small_disk: u64 = SLAB_SIZE + SECTOR_SIZE as u64 * 2;
    let (c, _data_mock, _metadata_mock) = create_test_component(small_disk, METADATA_DISK_SIZE);
    c.format(FormatParams {
        data_disk_size: small_disk,
        slab_size: SLAB_SIZE,
        max_extent_size: MAX_EXTENT_SIZE,
        sector_size: SECTOR_SIZE,
        region_count: 1,
        metadata_alignment: METADATA_ALIGNMENT,
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

// ============================================================
// User Story 2: Abort a Reservation (T016, T017)
// ============================================================

#[test]
fn explicit_abort() {
    let c = setup();
    let handle = c.reserve_extent(99, 4096).expect("reserve");
    handle.abort();

    match c.lookup_extent(99) {
        Err(ExtentManagerError::KeyNotFound(k)) => assert_eq!(k, 99),
        other => panic!("expected KeyNotFound, got: {other:?}"),
    }

    let h2 = c.reserve_extent(100, 4096).expect("reserve after abort");
    h2.publish().expect("publish after abort");
}

#[test]
fn drop_as_abort() {
    let c = setup();
    {
        let _handle = c.reserve_extent(77, 4096).expect("reserve");
    }

    match c.lookup_extent(77) {
        Err(ExtentManagerError::KeyNotFound(k)) => assert_eq!(k, 77),
        other => panic!("expected KeyNotFound, got: {other:?}"),
    }

    let h = c.reserve_extent(78, 4096).expect("reserve after drop");
    h.publish().expect("publish after drop");
}

// ============================================================
// User Story 3: Remove a Published File (T019, T020)
// ============================================================

#[test]
fn remove_published_extent() {
    let c = setup();

    let handle = c.reserve_extent(42, 4096).expect("reserve");
    handle.publish().expect("publish");

    c.remove_extent(42).expect("remove");

    match c.lookup_extent(42) {
        Err(ExtentManagerError::KeyNotFound(k)) => assert_eq!(k, 42),
        other => panic!("expected KeyNotFound, got: {other:?}"),
    }
}

#[test]
fn remove_nonexistent_key() {
    let c = setup();
    match c.remove_extent(999) {
        Err(ExtentManagerError::KeyNotFound(k)) => assert_eq!(k, 999),
        other => panic!("expected KeyNotFound, got: {other:?}"),
    }
}

#[test]
fn full_lifecycle_round_trip() {
    let c = setup();

    let h = c.reserve_extent(42, 4096).expect("reserve");
    let ext = h.publish().expect("publish");
    assert_eq!(ext.key, 42);

    let looked = c.lookup_extent(42).expect("lookup");
    assert_eq!(looked.offset, ext.offset);

    c.remove_extent(42).expect("remove");

    match c.lookup_extent(42) {
        Err(ExtentManagerError::KeyNotFound(_)) => {}
        other => panic!("expected KeyNotFound, got: {other:?}"),
    }

    let h2 = c.reserve_extent(42, 4096).expect("re-reserve same key");
    h2.publish().expect("re-publish same key");
    c.lookup_extent(42).expect("lookup after re-publish");
}

// ============================================================
// User Story 6: Enumerate All Allocated Extents (T032)
// ============================================================

#[test]
fn get_extents_returns_all_published() {
    let c = setup();

    let keys: Vec<u64> = (1..=10).collect();
    for &k in &keys {
        let h = c.reserve_extent(k, 4096).expect("reserve");
        h.publish().expect("publish");
    }

    let extents = c.get_extents();
    assert_eq!(extents.len(), 10);

    let mut found_keys: Vec<u64> = extents.iter().map(|e| e.key).collect();
    found_keys.sort();
    assert_eq!(found_keys, keys);
}

#[test]
fn get_extents_empty() {
    let c = setup();
    assert!(c.get_extents().is_empty());
}

#[test]
fn reserved_not_in_enumeration() {
    let c = setup();
    let _handle = c.reserve_extent(42, 4096).expect("reserve");
    assert!(c.get_extents().is_empty());
}

#[test]
fn for_each_extent_visits_all() {
    let c = setup();

    for k in 1..=5u64 {
        let h = c.reserve_extent(k, 4096).expect("reserve");
        h.publish().expect("publish");
    }

    let mut count = 0;
    c.for_each_extent(&mut |_ext| {
        count += 1;
    });
    assert_eq!(count, 5);
}
