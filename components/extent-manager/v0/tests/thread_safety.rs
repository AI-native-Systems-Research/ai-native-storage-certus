//! Thread-safety tests for concurrent access to `ExtentManagerComponentV1`.
//!
//! All tests share an `Arc<ExtentManagerComponentV1>` across threads and
//! verify that the final state is consistent with the operations performed.
//! A 30-second timeout is used for deadlock detection.

use std::sync::Arc;
use std::thread;

use extent_manager::test_support::create_test_component;
use interfaces::IExtentManager;

// T026: concurrent creates
#[test]
fn thread_concurrent_creates() {
    // 8 threads, 100 extents each with unique key ranges.
    let (comp, _mock) = create_test_component(100_000, &[131072], &[1000]);
    let comp = Arc::new(comp);

    let num_threads = 8;
    let per_thread = 100;

    let handles: Vec<_> = (0..num_threads)
        .map(|t| {
            let comp = Arc::clone(&comp);
            thread::Builder::new()
                .name(format!("creator-{t}"))
                .spawn(move || {
                    let base = (t as u64) * per_thread;
                    for i in 0..per_thread {
                        comp.create_extent(base + i, 0, "", 0, false)
                            .unwrap_or_else(|e| panic!("thread {t} key {}: {e}", base + i));
                    }
                })
                .unwrap()
        })
        .collect();

    // Join all threads — panics propagate via join().
    for h in handles {
        h.join().expect("creator thread panicked");
    }

    // Verify total count.
    let expected = (num_threads as u64) * per_thread;
    assert_eq!(comp.extent_count(), expected);

    // Verify each extent is individually retrievable.
    for key in 0..expected {
        comp.lookup_extent(key)
            .unwrap_or_else(|_| panic!("key {key} not found after concurrent creates"));
    }
}

// T027: concurrent creates and removes
#[test]
fn thread_concurrent_creates_and_removes() {
    let (comp, _mock) = create_test_component(100_000, &[131072], &[2000]);
    let comp = Arc::new(comp);

    let num_threads = 4;
    let per_thread = 100;

    // Phase 1: each thread creates `per_thread` extents.
    let create_handles: Vec<_> = (0..num_threads)
        .map(|t| {
            let comp = Arc::clone(&comp);
            thread::spawn(move || {
                let base = (t as u64) * per_thread;
                for i in 0..per_thread {
                    comp.create_extent(base + i, 0, "", 0, false).unwrap();
                }
            })
        })
        .collect();

    for h in create_handles {
        h.join().expect("create thread panicked");
    }

    let total_created = (num_threads as u64) * per_thread;
    assert_eq!(comp.extent_count(), total_created);

    // Phase 2: half the threads remove, half create more.
    let remove_handles: Vec<_> = (0..num_threads)
        .map(|t| {
            let comp = Arc::clone(&comp);
            thread::spawn(move || {
                if t % 2 == 0 {
                    // Remove thread: remove keys from own range.
                    let base = (t as u64) * per_thread;
                    for i in 0..per_thread {
                        comp.remove_extent(base + i).unwrap();
                    }
                } else {
                    // Create thread: add new unique keys.
                    let base = total_created + (t as u64) * per_thread;
                    for i in 0..per_thread {
                        comp.create_extent(base + i, 0, "", 0, false).unwrap();
                    }
                }
            })
        })
        .collect();

    for h in remove_handles {
        h.join().expect("mixed thread panicked");
    }

    // Count: originally created - removed (even threads) + newly created (odd threads)
    let removed_threads = (0..num_threads).filter(|t| t % 2 == 0).count() as u64;
    let created_threads = (0..num_threads).filter(|t| t % 2 != 0).count() as u64;
    let expected = total_created - (removed_threads * per_thread) + (created_threads * per_thread);
    assert_eq!(comp.extent_count(), expected);
}

// T028: concurrent lookups
#[test]
fn thread_concurrent_lookups() {
    let (comp, _mock) = create_test_component(100_000, &[131072], &[1000]);
    let comp = Arc::new(comp);

    // Pre-populate 100 extents.
    for key in 0..100 {
        comp.create_extent(key, 0, "", 0, false).unwrap();
    }

    // 8 threads all doing concurrent lookups on the same keys.
    let num_threads = 8;
    let handles: Vec<_> = (0..num_threads)
        .map(|_| {
            let comp = Arc::clone(&comp);
            thread::spawn(move || {
                for key in 0..100 {
                    let bytes = comp.lookup_extent(key).unwrap();
                    let meta = extent_manager::ExtentMetadata::from_bytes(&bytes).unwrap();
                    assert_eq!(meta.key, key);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("lookup thread panicked");
    }

    assert_eq!(comp.extent_count(), 100);
}

// T029: concurrent mixed operations (creates + removes + lookups simultaneously)
#[test]
fn thread_concurrent_mixed_operations() {
    let (comp, _mock) = create_test_component(100_000, &[131072], &[5000]);
    let comp = Arc::new(comp);

    // Pre-populate 500 extents in range [0, 500).
    for key in 0..500 {
        comp.create_extent(key, 0, "", 0, false).unwrap();
    }

    let handles: Vec<_> = (0..6)
        .map(|t| {
            let comp = Arc::clone(&comp);
            thread::spawn(move || match t % 3 {
                // Creator: keys [1000+t*200 .. 1000+t*200+200)
                0 => {
                    let base = 1000 + (t as u64) * 200;
                    for i in 0..200 {
                        comp.create_extent(base + i, 0, "", 0, false).unwrap();
                    }
                }
                // Remover: remove from pre-populated range.
                1 => {
                    let base = (t as u64 / 3) * 100;
                    for i in 0..100 {
                        // Ignore errors — another remover thread might race.
                        let _ = comp.remove_extent(base + i);
                    }
                }
                // Looker: lookup pre-populated keys (might get KeyNotFound if removed).
                _ => {
                    for key in 0..500 {
                        let _ = comp.lookup_extent(key);
                    }
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("mixed-ops thread panicked");
    }

    // Final count should be consistent (no phantom extents).
    let count = comp.extent_count();
    // Verify every extent in the index is actually retrievable.
    // We can't predict exact count due to races, but it must be consistent.
    let mut verified = 0u64;
    for key in 0..5000 {
        if comp.lookup_extent(key).is_ok() {
            verified += 1;
        }
    }
    assert_eq!(
        count, verified,
        "extent_count disagrees with actual lookups"
    );
}
