use extent_manager::test_support::{create_test_component, heap_dma_alloc, MockBlockDevice};
use interfaces::{ExtentManagerError, IExtentManager, IExtentManagerAdmin};
use std::sync::Arc;

const SLAB_SIZE: u32 = 128 * 4096; // 128 blocks = 512 KiB slab
const TOTAL_SIZE: u64 = 100 * 128 * 4096; // room for ~100 slabs

fn setup() -> (
    Arc<extent_manager::ExtentManagerComponentV1>,
    Arc<MockBlockDevice>,
) {
    let (comp, mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1)
        .expect("initialize");
    (comp, mock)
}

#[test]
fn initialize_creates_state() {
    let (comp, _mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1)
        .expect("initialize");
    assert_eq!(comp.extent_count(), 0);
}

#[test]
fn initialize_rejects_zero_size() {
    let (comp, _mock) = create_test_component();
    let err = comp.initialize(0, SLAB_SIZE, 1);
    assert!(err.is_err());
}

#[test]
fn initialize_rejects_tiny_slab() {
    let (comp, _mock) = create_test_component();
    let err = comp.initialize(TOTAL_SIZE, 4096, 1); // 1 block slab
    assert!(err.is_err());
}

#[test]
fn initialize_rejects_unaligned_slab() {
    let (comp, _mock) = create_test_component();
    let err = comp.initialize(TOTAL_SIZE, 5000, 1);
    assert!(err.is_err());
}

#[test]
fn create_extent_basic() {
    let (comp, _mock) = setup();
    let result = comp.create_extent(1, 131072, "test.dat", 0, false);
    assert!(result.is_ok());
    assert_eq!(comp.extent_count(), 1);
}

#[test]
fn create_extent_with_filename_and_crc() {
    let (comp, _mock) = setup();
    let result = comp.create_extent(42, 131072, "myfile.dat", 0xDEADBEEF, true);
    assert!(result.is_ok());

    let serialized = comp.lookup_extent(42).expect("lookup");
    assert!(!serialized.is_empty());
}

#[test]
fn create_extent_duplicate_key_fails() {
    let (comp, _mock) = setup();
    comp.create_extent(1, 131072, "", 0, false)
        .expect("first create");
    let err = comp.create_extent(1, 131072, "", 0, false);
    assert!(matches!(err, Err(ExtentManagerError::DuplicateKey(_))));
}

#[test]
fn create_extent_invalid_size_class_fails() {
    let (comp, _mock) = setup();
    let err = comp.create_extent(1, 999, "", 0, false);
    assert!(matches!(err, Err(ExtentManagerError::InvalidSizeClass(_))));
}

#[test]
fn lookup_extent_basic() {
    let (comp, _mock) = setup();
    let created = comp
        .create_extent(1, 131072, "hello.txt", 0xABCD, true)
        .expect("create");
    let looked_up = comp.lookup_extent(1).expect("lookup");
    assert_eq!(created, looked_up);
}

#[test]
fn lookup_extent_not_found() {
    let (comp, _mock) = setup();
    let err = comp.lookup_extent(999);
    assert!(matches!(err, Err(ExtentManagerError::KeyNotFound(_))));
}

#[test]
fn remove_extent_basic() {
    let (comp, _mock) = setup();
    comp.create_extent(1, 131072, "", 0, false).expect("create");
    assert_eq!(comp.extent_count(), 1);

    comp.remove_extent(1).expect("remove");
    assert_eq!(comp.extent_count(), 0);

    let err = comp.lookup_extent(1);
    assert!(matches!(err, Err(ExtentManagerError::KeyNotFound(_))));
}

#[test]
fn remove_extent_not_found() {
    let (comp, _mock) = setup();
    let err = comp.remove_extent(999);
    assert!(matches!(err, Err(ExtentManagerError::KeyNotFound(_))));
}

#[test]
fn remove_then_create_reuses_slot() {
    let (comp, _mock) = setup();
    comp.create_extent(1, 131072, "", 0, false)
        .expect("create 1");
    comp.remove_extent(1).expect("remove 1");
    comp.create_extent(2, 131072, "", 0, false)
        .expect("create 2");
    assert_eq!(comp.extent_count(), 1);
}

#[test]
fn multiple_size_classes() {
    let (comp, _mock) = setup();
    comp.create_extent(1, 131072, "", 0, false)
        .expect("create 128K");
    comp.create_extent(2, 262144, "", 0, false)
        .expect("create 256K");
    assert_eq!(comp.extent_count(), 2);

    comp.remove_extent(1).expect("remove 128K");
    assert_eq!(comp.extent_count(), 1);
    comp.lookup_extent(2).expect("256K still exists");
}

#[test]
fn extent_count_tracks_correctly() {
    let (comp, _mock) = setup();
    assert_eq!(comp.extent_count(), 0);

    for i in 0..10u64 {
        comp.create_extent(i, 131072, "", 0, false).expect("create");
    }
    assert_eq!(comp.extent_count(), 10);

    for i in 0..5u64 {
        comp.remove_extent(i).expect("remove");
    }
    assert_eq!(comp.extent_count(), 5);
}

#[test]
fn out_of_space() {
    let (comp, _mock) = create_test_component();
    // Tiny device: superblock (1 block) + 1 slab of 2 blocks (1 bitmap + 1 slot) = 3 blocks
    // Total = 3 blocks, slab = 2 blocks → only 1 slot available, no room for second slab
    let block_size = 4096u64;
    comp.initialize(3 * block_size, 2 * 4096, 1).expect("init");

    comp.create_extent(1, 131072, "", 0, false).expect("1");
    let err = comp.create_extent(2, 131072, "", 0, false);
    assert!(matches!(err, Err(ExtentManagerError::OutOfSpace { .. })));
}

#[test]
fn not_initialized_errors() {
    let (comp, _mock) = create_test_component();
    let err = comp.create_extent(1, 131072, "", 0, false);
    assert!(matches!(err, Err(ExtentManagerError::NotInitialized(_))));

    let err = comp.lookup_extent(1);
    assert!(matches!(err, Err(ExtentManagerError::NotInitialized(_))));

    let err = comp.remove_extent(1);
    assert!(matches!(err, Err(ExtentManagerError::NotInitialized(_))));
}

#[test]
fn open_recovers_extents() {
    let (comp, mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");
    comp.create_extent(10, 131072, "a.dat", 0, false)
        .expect("create 10");
    comp.create_extent(20, 131072, "b.dat", 0, false)
        .expect("create 20");
    assert_eq!(comp.extent_count(), 2);

    let shared = mock.shared_state();
    drop(comp);

    let mock2 = MockBlockDevice::reboot_from(&shared);
    let mock2 = Arc::new(mock2);
    let comp2 = extent_manager::ExtentManagerComponentV1::new_default();
    comp2
        .block_device
        .connect(mock2 as Arc<dyn interfaces::IBlockDevice>)
        .expect("connect");
    comp2.set_dma_alloc(heap_dma_alloc());

    let result = comp2.open(1).expect("open");
    assert_eq!(result.extents_loaded, 2);
    assert_eq!(result.orphans_cleaned, 0);
    assert_eq!(result.corrupt_records, 0);

    assert_eq!(comp2.extent_count(), 2);
    comp2.lookup_extent(10).expect("extent 10 recovered");
    comp2.lookup_extent(20).expect("extent 20 recovered");
}

#[test]
fn dynamic_slab_allocation() {
    let (comp, _mock) = setup();
    // Create extents of different size classes — each triggers a new slab
    comp.create_extent(1, 131072, "", 0, false)
        .expect("128K class");
    comp.create_extent(2, 262144, "", 0, false)
        .expect("256K class");
    comp.create_extent(3, 524288, "", 0, false)
        .expect("512K class");
    assert_eq!(comp.extent_count(), 3);
}

#[test]
fn multi_slab_same_class() {
    let (comp, _mock) = create_test_component();
    // Slab of 4 blocks: 1 bitmap + 3 slots
    // Total: enough for multiple slabs
    comp.initialize(100 * 4096, 4 * 4096, 1).expect("init");

    // Fill first slab (3 slots)
    comp.create_extent(1, 131072, "", 0, false).expect("1");
    comp.create_extent(2, 131072, "", 0, false).expect("2");
    comp.create_extent(3, 131072, "", 0, false).expect("3");

    // This should trigger a second slab allocation for the same class
    comp.create_extent(4, 131072, "", 0, false).expect("4");
    assert_eq!(comp.extent_count(), 4);
}
