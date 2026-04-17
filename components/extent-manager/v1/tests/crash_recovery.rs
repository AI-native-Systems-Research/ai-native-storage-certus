use extent_manager::test_support::{
    create_test_component, heap_dma_alloc, FaultConfig, MockBlockDevice,
};
use interfaces::{IBlockDevice, IExtentManager, IExtentManagerAdmin};
use std::sync::Arc;

const SLAB_SIZE: u32 = 128 * 4096;
const TOTAL_SIZE: u64 = 100 * 128 * 4096;

fn reopen(
    shared: &Arc<std::sync::Mutex<extent_manager::test_support::MockState>>,
) -> Arc<extent_manager::ExtentManagerComponentV1> {
    let mock2 = MockBlockDevice::reboot_from(shared);
    let mock2 = Arc::new(mock2);
    let comp2 = extent_manager::ExtentManagerComponentV1::new_default();
    comp2
        .block_device
        .connect(mock2 as Arc<dyn IBlockDevice>)
        .expect("connect");
    comp2.set_dma_alloc(heap_dma_alloc());
    comp2.open(1).expect("open");
    comp2
}

#[test]
fn clean_shutdown_recovery() {
    let (comp, mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");
    comp.create_extent(1, 131072, "file.dat", 0xAB, true)
        .expect("create");
    comp.create_extent(2, 131072, "", 0, false).expect("create");

    let shared = mock.shared_state();
    drop(comp);

    let comp2 = reopen(&shared);
    assert_eq!(comp2.extent_count(), 2);
    comp2.lookup_extent(1).expect("key 1");
    comp2.lookup_extent(2).expect("key 2");
}

#[test]
fn crash_during_create_before_bitmap_write() {
    let (comp, mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");
    comp.create_extent(1, 131072, "", 0, false)
        .expect("first create ok");

    // For the second create_extent, crash after the record write but before bitmap write.
    // The slab is already allocated so create_extent does: 1 write (record) + 1 write (bitmap).
    // fail_after_n_writes=1 means the record write succeeds but bitmap write fails.
    mock.set_fault_config(FaultConfig {
        fail_after_n_writes: Some(1),
        ..Default::default()
    });

    let _ = comp.create_extent(2, 131072, "", 0, false);

    let shared = mock.shared_state();
    drop(comp);

    let comp2 = reopen(&shared);
    assert_eq!(comp2.extent_count(), 1);
    comp2.lookup_extent(1).expect("key 1 survives");
    assert!(comp2.lookup_extent(2).is_err());
}

#[test]
fn crash_during_remove_before_zero_block() {
    let (comp, mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");
    comp.create_extent(1, 131072, "", 0, false).expect("create");

    // Remove: phase 1 = clear bitmap + write bitmap block,
    // phase 2 = zero the record block.
    // Crash after bitmap write but before zeroing the record
    mock.set_fault_config(FaultConfig {
        fail_after_n_writes: Some(1),
        ..Default::default()
    });

    let _ = comp.remove_extent(1);

    let shared = mock.shared_state();
    drop(comp);

    let comp2 = reopen(&shared);
    assert_eq!(comp2.extent_count(), 0);
}

#[test]
fn recovery_with_multiple_size_classes() {
    let (comp, mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");
    comp.create_extent(1, 131072, "a", 0, false).expect("c1");
    comp.create_extent(2, 262144, "b", 0, false).expect("c2");
    comp.create_extent(3, 131072, "c", 0, false).expect("c3");

    let shared = mock.shared_state();
    drop(comp);

    let comp2 = reopen(&shared);
    assert_eq!(comp2.extent_count(), 3);
    comp2.lookup_extent(1).expect("1");
    comp2.lookup_extent(2).expect("2");
    comp2.lookup_extent(3).expect("3");
}

#[test]
fn open_empty_device() {
    let (comp, mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");

    let shared = mock.shared_state();
    drop(comp);

    let comp2 = reopen(&shared);
    assert_eq!(comp2.extent_count(), 0);
}

#[test]
fn create_remove_create_then_recover() {
    let (comp, mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");
    comp.create_extent(1, 131072, "", 0, false)
        .expect("create 1");
    comp.remove_extent(1).expect("remove 1");
    comp.create_extent(2, 131072, "", 0, false)
        .expect("create 2");

    let shared = mock.shared_state();
    drop(comp);

    let comp2 = reopen(&shared);
    assert_eq!(comp2.extent_count(), 1);
    assert!(comp2.lookup_extent(1).is_err());
    comp2.lookup_extent(2).expect("2");
}
