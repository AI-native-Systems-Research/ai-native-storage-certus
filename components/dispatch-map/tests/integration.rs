//! Integration tests for the dispatch map component.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use component_core::query_interface;
use component_framework::define_component;
use dispatch_map::DispatchMapComponent;
use interfaces::{
    DispatchMapError, Extent, ExtentKey, ExtentManagerError, FormatParams, IDispatchMap,
    IEvictionPolicy, IExtentManager, LookupResult, WriteHandle,
};

use dispatch_map::DispatchMapState;

// ---------------------------------------------------------------------------
// Mock helpers
// ---------------------------------------------------------------------------

fn setup_component() -> Arc<DispatchMapComponent> {
    let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
        query_interface!(ep_comp, IEvictionPolicy).unwrap();

    let c = DispatchMapComponent::new(DispatchMapState::new());
    c.eviction_policy.connect(ep).unwrap();
    c
}

/// Helper: create a memory-tier entry with a leaked buffer for the given key.
fn create_entry(dm: &Arc<dyn IDispatchMap + Send + Sync>, key: u64) {
    let ptr = Box::into_raw(vec![0u8; 4096].into_boxed_slice()) as *mut u8;
    dm.create_memory_tier_entry(key, ptr, 4096).unwrap();
}

// ---------------------------------------------------------------------------
// T014: Multi-threaded concurrent access
// ---------------------------------------------------------------------------

#[test]
fn multiple_readers_concurrent() {
    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
    // write_ref is 1 from create_memory_tier_entry

    let dm2 = Arc::clone(&dm);
    let reader = thread::spawn(move || {
        let result = dm2.lookup(1).unwrap();
        assert!(matches!(result, LookupResult::MemoryTier { .. }));
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
    create_entry(&dm, 1);
    // write_ref=1 from create_memory_tier_entry

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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
    // key 1 has write_ref=1
    create_entry(&dm, 2);
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
    create_entry(&dm, 1);
    // write_ref=1

    let dm2 = Arc::clone(&dm);
    let reader = thread::spawn(move || {
        let result = dm2.lookup(1).unwrap();
        assert!(matches!(result, LookupResult::MemoryTier { .. }));
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
    create_entry(&dm, 1);
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
        create_entry(&dm, k);
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
    create_entry(&dm, 1);
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

    fn get_instance_id(&self) -> Result<u64, ExtentManagerError> {
        Ok(1)
    }

    fn set_checkpoint_interval(&self, _interval: Option<std::time::Duration>) {}

    fn used_bytes(&self) -> u64 {
        0
    }

    fn capacity_bytes(&self) -> u64 {
        0
    }

    fn set_metadata_base_lba(&self, _base_lba: u64) {}

    fn set_data_base_lba(&self, _base_lba: u64) {}

    fn data_base_lba(&self) -> u64 {
        0
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

    let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
        query_interface!(ep_comp, IEvictionPolicy).unwrap();

    let c = DispatchMapComponent::new(DispatchMapState::new());
    c.eviction_policy.connect(ep).unwrap();
    c.connect_receptacle_raw("extent_manager", &*em)
        .expect("bind extent_manager");

    let dm = query_interface!(c, IDispatchMap).unwrap();
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

    let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
        query_interface!(ep_comp, IEvictionPolicy).unwrap();

    let c = DispatchMapComponent::new(DispatchMapState::new());
    c.eviction_policy.connect(ep).unwrap();
    c.connect_receptacle_raw("extent_manager", &*em)
        .expect("bind extent_manager");

    let dm = query_interface!(c, IDispatchMap).unwrap();
    dm.initialize().unwrap();

    let result = dm.lookup(1).unwrap();
    assert!(matches!(result, LookupResult::NotExist));
}

// ---------------------------------------------------------------------------
// Mutual exclusion under contention
//
// These guard the atomicity of wait-then-act. `wait_for` returns the lock still
// held; if it released it and let the caller re-acquire, the predicate it waited
// on could be falsified in the gap. Both tests fail if that guard is dropped:
// they are the regression tests for a real double-writer / reader-during-write
// race, so they are written to hammer the boundary rather than probe it once.
// ---------------------------------------------------------------------------

/// Only one writer may hold a key at a time.
///
/// `take_write` assigns `write_ref = 1` rather than incrementing it, so two
/// writers leave the refcount looking perfectly valid — the violation is only
/// observable by watching how many threads are inside the critical section at
/// once, which is what the shared counter does here.
#[test]
fn take_write_is_mutually_exclusive() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const THREADS: usize = 8;
    const ITERATIONS: usize = 200;

    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    create_entry(&dm, 1);
    dm.release_write(1).unwrap();

    let inside = Arc::new(AtomicUsize::new(0));
    let violated = Arc::new(AtomicBool::new(false));
    let acquisitions = Arc::new(AtomicUsize::new(0));

    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let dm = Arc::clone(&dm);
            let inside = Arc::clone(&inside);
            let violated = Arc::clone(&violated);
            let acquisitions = Arc::clone(&acquisitions);
            thread::spawn(move || {
                for _ in 0..ITERATIONS {
                    // A timeout under contention is legitimate; only count and
                    // check the iterations that actually got the write ref.
                    if dm.take_write(1).is_err() {
                        continue;
                    }
                    acquisitions.fetch_add(1, Ordering::Relaxed);
                    if inside.fetch_add(1, Ordering::AcqRel) != 0 {
                        violated.store(true, Ordering::Relaxed);
                    }
                    std::thread::yield_now();
                    inside.fetch_sub(1, Ordering::AcqRel);
                    // A failed release means someone else already cleared the
                    // reference we were holding — the same violation seen from
                    // the other end. Record it rather than panicking, so the
                    // assertion below reports it.
                    if dm.release_write(1).is_err() {
                        violated.store(true, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    assert!(
        !violated.load(Ordering::Relaxed),
        "two threads held the write reference on key 1 at the same time"
    );
    // Guard against the test silently passing because every attempt timed out.
    assert!(
        acquisitions.load(Ordering::Relaxed) > THREADS,
        "too few successful take_write calls ({}) to have exercised the race",
        acquisitions.load(Ordering::Relaxed)
    );
}

/// A reader must never be admitted while a writer holds the key.
///
/// `lookup` waits for `write_ref == 0` and then takes a read reference. If the
/// lock is released in between, a writer can claim the entry in the gap and the
/// reader still proceeds — handing out a `Location` pointer into a slot the
/// writer believes it owns exclusively.
#[test]
fn no_reader_admitted_while_write_ref_held() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    const READERS: usize = 4;
    const ITERATIONS: usize = 300;

    let c = setup_component();
    let dm = query_interface!(c, IDispatchMap).unwrap();
    create_entry(&dm, 1);
    dm.release_write(1).unwrap();

    let writer_inside = Arc::new(AtomicBool::new(false));
    let violated = Arc::new(AtomicBool::new(false));
    let reads = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    let writer = {
        let dm = Arc::clone(&dm);
        let writer_inside = Arc::clone(&writer_inside);
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if dm.take_write(1).is_err() {
                    continue;
                }
                writer_inside.store(true, Ordering::Release);
                std::thread::yield_now();
                writer_inside.store(false, Ordering::Release);
                dm.release_write(1).unwrap();
            }
        })
    };

    let readers: Vec<_> = (0..READERS)
        .map(|_| {
            let dm = Arc::clone(&dm);
            let writer_inside = Arc::clone(&writer_inside);
            let violated = Arc::clone(&violated);
            let reads = Arc::clone(&reads);
            thread::spawn(move || {
                for _ in 0..ITERATIONS {
                    if let Ok(LookupResult::MemoryTier { .. }) = dm.lookup(1) {
                        // We hold a read reference: no writer may be inside.
                        if writer_inside.load(Ordering::Acquire) {
                            violated.store(true, Ordering::Relaxed);
                        }
                        reads.fetch_add(1, Ordering::Relaxed);
                        dm.release_read(1).unwrap();
                    }
                }
            })
        })
        .collect();

    for h in readers {
        h.join().unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();

    assert!(
        !violated.load(Ordering::Relaxed),
        "lookup handed out a read reference while a writer held key 1"
    );
    assert!(
        reads.load(Ordering::Relaxed) > READERS,
        "too few successful lookups ({}) to have exercised the race",
        reads.load(Ordering::Relaxed)
    );
}
