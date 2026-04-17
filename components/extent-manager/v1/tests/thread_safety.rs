use extent_manager::test_support::create_test_component;
use extent_manager::ExtentManagerComponentV1;
use interfaces::{IExtentManager, IExtentManagerAdmin};
use std::sync::Arc;
use std::thread;

const SLAB_SIZE: u32 = 128 * 4096;
const TOTAL_SIZE: u64 = 100 * 128 * 4096;

#[test]
fn concurrent_creates_no_duplicates() {
    let (comp, _mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");

    let handles: Vec<_> = (0..4)
        .map(|t| {
            let comp: Arc<ExtentManagerComponentV1> = Arc::clone(&comp);
            thread::spawn(move || {
                for i in 0..100u64 {
                    let key = t * 1000 + i;
                    comp.create_extent(key, 131072, "", 0, false)
                        .expect("create");
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(comp.extent_count(), 400);
}

#[test]
fn concurrent_lookups() {
    let (comp, _mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");

    for i in 0..50u64 {
        comp.create_extent(i, 131072, "", 0, false).expect("create");
    }

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let comp: Arc<ExtentManagerComponentV1> = Arc::clone(&comp);
            thread::spawn(move || {
                for i in 0..50u64 {
                    comp.lookup_extent(i).expect("lookup");
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn concurrent_create_and_lookup() {
    let (comp, _mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");

    for i in 0..100u64 {
        comp.create_extent(i, 131072, "", 0, false).expect("create");
    }

    let comp_writer: Arc<ExtentManagerComponentV1> = Arc::clone(&comp);
    let writer = thread::spawn(move || {
        for i in 100..200u64 {
            comp_writer
                .create_extent(i, 131072, "", 0, false)
                .expect("create");
        }
    });

    let comp_reader: Arc<ExtentManagerComponentV1> = Arc::clone(&comp);
    let reader = thread::spawn(move || {
        for i in 0..100u64 {
            comp_reader.lookup_extent(i).expect("lookup");
        }
    });

    writer.join().unwrap();
    reader.join().unwrap();
    assert_eq!(comp.extent_count(), 200);
}

#[test]
fn extent_count_concurrent() {
    let (comp, _mock) = create_test_component();
    comp.initialize(TOTAL_SIZE, SLAB_SIZE, 1).expect("init");

    let comp2: Arc<ExtentManagerComponentV1> = Arc::clone(&comp);
    let writer = thread::spawn(move || {
        for i in 0..500u64 {
            comp2
                .create_extent(i, 131072, "", 0, false)
                .expect("create");
        }
    });

    let comp3: Arc<ExtentManagerComponentV1> = Arc::clone(&comp);
    let counter = thread::spawn(move || {
        let mut max_seen = 0u64;
        for _ in 0..1000 {
            let count = comp3.extent_count();
            assert!(count >= max_seen);
            max_seen = count;
        }
    });

    writer.join().unwrap();
    counter.join().unwrap();
    assert_eq!(comp.extent_count(), 500);
}
