//! Integration tests for the dispatch map component.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use component_core::query_interface;
use component_framework::define_component;
use dispatch_map::DispatchMapComponentV0;
use interfaces::{
    DispatchMapError, DmaAllocFn, DmaBuffer, Extent, ExtentKey, ExtentManagerError, FormatParams,
    IDispatchMap, IExtentManager, LookupResult, WriteHandle,
};

use dispatch_map::DispatchMapState;

// ---------------------------------------------------------------------------
// Mock helpers
// ---------------------------------------------------------------------------

fn mock_dma_alloc() -> DmaAllocFn {
    Arc::new(|size, _align, _numa| {
        let layout = std::alloc::Layout::from_size_align(size, 4096).unwrap();
        // SAFETY: Test-only allocation with valid layout.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("allocation failed".into());
        }
        // SAFETY: ptr is valid heap memory from alloc_zeroed.
        unsafe {
            DmaBuffer::from_raw(
                ptr as *mut std::ffi::c_void,
                size,
                mock_free as unsafe extern "C" fn(*mut std::ffi::c_void),
                -1,
            )
        }
        .map_err(|e| e.to_string())
    })
}

unsafe extern "C" fn mock_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        // SAFETY: test-only dealloc matching mock_dma_alloc.
        unsafe {
            std::alloc::dealloc(
                ptr as *mut u8,
                std::alloc::Layout::from_size_align_unchecked(1, 1),
            );
        }
    }
}

fn setup_component() -> Arc<DispatchMapComponentV0> {
    let c = DispatchMapComponentV0::new(DispatchMapState::new());
    let dm = query_interface!(c, IDispatchMap).unwrap();
    dm.set_dma_alloc(mock_dma_alloc());
    c
}

// ---------------------------------------------------------------------------
// T014: Multi-threaded concurrent access
// ---------------------------------------------------------------------------

#[test]
fn multiple_readers_concurrent() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let dm = Arc::clone(&dm);
            thread::spawn(move || {
                dm.take_read(1).unwrap();
                thread::sleep(Duration::from_millis(10));
                dm.release_read(1).unwrap();
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn writer_blocks_until_readers_release() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();

    dm.take_read(1).unwrap();

    let dm2 = Arc::clone(&dm);
    let writer = thread::spawn(move || {
        dm2.take_write(1).unwrap();
        dm2.release_write(1).unwrap();
    });

    thread::sleep(Duration::from_millis(50));
    dm.release_read(1).unwrap();

    writer.join().unwrap();
}

#[test]
fn writer_timeout_with_active_readers() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();

    dm.take_read(1).unwrap();

    let dm2 = Arc::clone(&dm);
    let writer = thread::spawn(move || {
        let result = dm2.take_write(1);
        assert!(matches!(result, Err(DispatchMapError::Timeout(1))));
    });

    writer.join().unwrap();
    dm.release_read(1).unwrap();
}

#[test]
fn lookup_blocks_on_active_writer() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // write_ref is 1 from create_staging

    let dm2 = Arc::clone(&dm);
    let reader = thread::spawn(move || {
        let result = dm2.lookup(1).unwrap();
        assert!(matches!(result, LookupResult::Staging { .. }));
        dm2.release_read(1).unwrap();
    });

    thread::sleep(Duration::from_millis(50));
    dm.release_write(1).unwrap();

    reader.join().unwrap();
}

// ---------------------------------------------------------------------------
// Locking correctness
// ---------------------------------------------------------------------------

#[test]
fn writer_blocks_on_another_writer() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // write_ref=1 from create_staging

    let dm2 = Arc::clone(&dm);
    let second_writer = thread::spawn(move || dm2.take_write(1));

    thread::sleep(Duration::from_millis(10));
    // First writer still held — second must timeout.
    dm.release_write(1).unwrap();

    let result = second_writer.join().unwrap();
    // Second writer either succeeded (released in time) or timed out.
    // With 100ms timeout and 10ms sleep, it should succeed.
    assert!(result.is_ok());
}

#[test]
fn second_writer_times_out_while_first_holds() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // write_ref=1 — never release

    let dm2 = Arc::clone(&dm);
    let second_writer = thread::spawn(move || dm2.take_write(1));

    let result = second_writer.join().unwrap();
    assert!(matches!(result, Err(DispatchMapError::Timeout(1))));

    dm.release_write(1).unwrap();
}

#[test]
fn writer_waits_for_all_readers_to_drain() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();

    // Acquire 3 read refs.
    dm.take_read(1).unwrap();
    dm.take_read(1).unwrap();
    dm.take_read(1).unwrap();

    let dm2 = Arc::clone(&dm);
    let writer = thread::spawn(move || dm2.take_write(1));

    // Release readers one at a time; writer should still be blocked after
    // the first two releases (read_ref > 0).
    thread::sleep(Duration::from_millis(5));
    dm.release_read(1).unwrap();
    thread::sleep(Duration::from_millis(5));
    dm.release_read(1).unwrap();
    thread::sleep(Duration::from_millis(5));
    dm.release_read(1).unwrap();

    let result = writer.join().unwrap();
    assert!(result.is_ok());
    dm.release_write(1).unwrap();
}

#[test]
fn take_read_times_out_with_active_writer() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // write_ref=1 — never release

    let dm2 = Arc::clone(&dm);
    let reader = thread::spawn(move || dm2.take_read(1));

    let result = reader.join().unwrap();
    assert!(matches!(result, Err(DispatchMapError::Timeout(1))));

    dm.release_write(1).unwrap();
}

#[test]
fn lookup_times_out_with_active_writer() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // write_ref=1 — never release

    let dm2 = Arc::clone(&dm);
    let reader = thread::spawn(move || dm2.lookup(1));

    let result = reader.join().unwrap();
    assert!(matches!(result, Err(DispatchMapError::Timeout(1))));

    dm.release_write(1).unwrap();
}

#[test]
fn independent_keys_do_not_interfere() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // key 1 has write_ref=1
    let _ = dm.create_staging(2, 1).unwrap();
    dm.release_write(2).unwrap();

    // Reading key 2 must not block on key 1's writer.
    let dm2 = Arc::clone(&dm);
    let reader = thread::spawn(move || {
        dm2.take_read(2).unwrap();
        dm2.release_read(2).unwrap();
    });
    reader.join().unwrap();

    // Writing key 2 must not block on key 1's writer.
    let dm3 = Arc::clone(&dm);
    let writer = thread::spawn(move || {
        dm3.take_write(2).unwrap();
        dm3.release_write(2).unwrap();
    });
    writer.join().unwrap();

    dm.release_write(1).unwrap();
}

#[test]
fn downgrade_unblocks_pending_readers() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // write_ref=1

    let dm2 = Arc::clone(&dm);
    let reader = thread::spawn(move || {
        let result = dm2.lookup(1).unwrap();
        assert!(matches!(result, LookupResult::Staging { .. }));
        dm2.release_read(1).unwrap();
    });

    thread::sleep(Duration::from_millis(10));
    // Downgrade write → read; pending lookup should unblock.
    dm.downgrade_reference(1).unwrap();

    reader.join().unwrap();
    dm.release_read(1).unwrap();
}

#[test]
fn downgrade_still_blocks_writers() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    dm.downgrade_reference(1).unwrap();
    // Now read_ref=1, write_ref=0.

    let dm2 = Arc::clone(&dm);
    let writer = thread::spawn(move || dm2.take_write(1));

    let result = writer.join().unwrap();
    // Writer must timeout because read_ref > 0.
    assert!(matches!(result, Err(DispatchMapError::Timeout(1))));

    dm.release_read(1).unwrap();
}

#[test]
fn sequential_writers_succeed() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();

    for _ in 0..5 {
        dm.take_write(1).unwrap();
        dm.release_write(1).unwrap();
    }
}

#[test]
fn reader_succeeds_immediately_after_writer_releases() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // write_ref=1

    let dm2 = Arc::clone(&dm);
    let reader = thread::spawn(move || {
        dm2.take_read(1).unwrap();
        dm2.release_read(1).unwrap();
    });

    thread::sleep(Duration::from_millis(10));
    dm.release_write(1).unwrap();

    reader.join().unwrap();
}

#[test]
fn remove_blocked_by_active_read_ref() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();
    dm.take_read(1).unwrap();

    let err = dm.remove(1);
    assert!(matches!(err, Err(DispatchMapError::ActiveReferences(1))));

    dm.release_read(1).unwrap();
    dm.remove(1).unwrap();
}

#[test]
fn remove_blocked_by_active_write_ref() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    // write_ref=1

    let err = dm.remove(1);
    assert!(matches!(err, Err(DispatchMapError::ActiveReferences(1))));

    dm.release_write(1).unwrap();
    dm.remove(1).unwrap();
}

#[test]
fn concurrent_readers_and_writer_on_different_keys() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();

    for k in 1..=4 {
        let _ = dm.create_staging(k, 1).unwrap();
        dm.release_write(k).unwrap();
    }

    let handles: Vec<_> = (1..=4)
        .map(|k| {
            let dm = Arc::clone(&dm);
            thread::spawn(move || {
                if k % 2 == 0 {
                    dm.take_read(k).unwrap();
                    thread::sleep(Duration::from_millis(5));
                    dm.release_read(k).unwrap();
                } else {
                    dm.take_write(k).unwrap();
                    thread::sleep(Duration::from_millis(5));
                    dm.release_write(k).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn lookup_acquires_read_ref() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();

    let _ = dm.lookup(1).unwrap();
    // lookup implicitly took a read ref, so take_write must timeout
    let dm2 = Arc::clone(&dm);
    let writer = thread::spawn(move || dm2.take_write(1));
    let result = writer.join().unwrap();
    assert!(matches!(result, Err(DispatchMapError::Timeout(1))));

    dm.release_read(1).unwrap();
}

// ---------------------------------------------------------------------------
// T027: Recovery with mock IExtentManager
// ---------------------------------------------------------------------------

define_component! {
    pub MockExtentManagerComponent {
        version: "0.1.0",
        provides: [IExtentManager],
        receptacles: {},
        fields: {
            extents: Vec<Extent>,
        },
    }
}

impl IExtentManager for MockExtentManagerComponent {
    fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

    fn format(&self, _params: FormatParams) -> Result<(), ExtentManagerError> {
        Ok(())
    }

    fn initialize(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }

    fn reserve_extent(
        &self,
        _key: ExtentKey,
        _size: u32,
    ) -> Result<WriteHandle, ExtentManagerError> {
        Err(ExtentManagerError::OutOfSpace)
    }

    fn lookup_extent(&self, _key: ExtentKey) -> Result<Extent, ExtentManagerError> {
        Err(ExtentManagerError::KeyNotFound(0))
    }

    fn get_extents(&self) -> Vec<Extent> {
        self.extents.clone()
    }

    fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent)) {
        for e in &self.extents {
            cb(e);
        }
    }

    fn remove_extent(&self, _key: ExtentKey) -> Result<(), ExtentManagerError> {
        Ok(())
    }

    fn checkpoint(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }
}

#[test]
fn recovery_populated() {
    use component_core::iunknown::IUnknown;

    let extents = vec![
        Extent {
            key: 10,
            size: 4,
            offset: 0,
        },
        Extent {
            key: 20,
            size: 8,
            offset: 16384,
        },
        Extent {
            key: 30,
            size: 2,
            offset: 32768,
        },
    ];
    let em = MockExtentManagerComponent::new(extents);

    let c = DispatchMapComponentV0::new(DispatchMapState::new());
    c.connect_receptacle_raw("extent_manager", &*em)
        .expect("bind extent_manager");

    let dm = query_interface!(c, IDispatchMap).unwrap();
    dm.set_dma_alloc(mock_dma_alloc());
    dm.initialize().unwrap();

    for key in [10, 20, 30] {
        let result = dm.lookup(key).unwrap();
        assert!(
            matches!(result, LookupResult::BlockDevice { .. }),
            "expected BlockDevice for key {key}"
        );
        dm.release_read(key).unwrap();
    }

    let result = dm.lookup(99).unwrap();
    assert!(matches!(result, LookupResult::NotExist));
}

#[test]
fn recovery_empty() {
    use component_core::iunknown::IUnknown;

    let em = MockExtentManagerComponent::new(vec![]);

    let c = DispatchMapComponentV0::new(DispatchMapState::new());
    c.connect_receptacle_raw("extent_manager", &*em)
        .expect("bind extent_manager");

    let dm = query_interface!(c, IDispatchMap).unwrap();
    dm.set_dma_alloc(mock_dma_alloc());
    dm.initialize().unwrap();

    let result = dm.lookup(1).unwrap();
    assert!(matches!(result, LookupResult::NotExist));
}
