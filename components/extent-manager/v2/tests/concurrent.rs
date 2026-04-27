use std::sync::Arc;
use std::thread;

use interfaces::{FormatParams, IExtentManager};

use extent_manager_v2::test_support::create_test_component;

const DISK_SIZE: u64 = 256 * 1024 * 1024;
const METADATA_DISK_SIZE: u64 = 16 * 1024 * 1024;
const SECTOR_SIZE: u32 = 4096;
const SLAB_SIZE: u64 = 1024 * 1024;
const MAX_ELEMENT_SIZE: u32 = 65536;
const METADATA_PADDING: u64 = 1048576;

fn format_params() -> FormatParams {
    FormatParams {
        slab_size: SLAB_SIZE,
        max_element_size: MAX_ELEMENT_SIZE,
        sector_size: SECTOR_SIZE,
        region_count: 8,
        metadata_padding: METADATA_PADDING,
    }
}

#[test]
fn concurrent_reserve_publish_lookup() {
    let (c, _data_mock, _metadata_mock) = create_test_component(DISK_SIZE, METADATA_DISK_SIZE);
    c.format(format_params()).expect("format");

    let c = Arc::new(c);
    let num_threads = 8;
    let ops_per_thread = 100;

    let mut handles = Vec::new();
    for t in 0..num_threads {
        let c = Arc::clone(&c);
        handles.push(thread::spawn(move || {
            for i in 0..ops_per_thread {
                let key = (t * ops_per_thread + i) as u64;
                let h = c.reserve_extent(key, 4096).expect("reserve");
                h.publish().expect("publish");

                let ext = c.lookup_extent(key).expect("lookup");
                assert_eq!(ext.key, key);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread join");
    }

    let extents = c.get_extents();
    assert_eq!(extents.len(), num_threads * ops_per_thread);
}

#[test]
fn concurrent_reserve_abort() {
    let (c, _data_mock, _metadata_mock) = create_test_component(DISK_SIZE, METADATA_DISK_SIZE);
    c.format(format_params()).expect("format");

    let c = Arc::new(c);
    let num_threads = 8;
    let ops_per_thread = 100;

    let mut handles = Vec::new();
    for t in 0..num_threads {
        let c = Arc::clone(&c);
        handles.push(thread::spawn(move || {
            for i in 0..ops_per_thread {
                let key = (t * ops_per_thread + i) as u64;
                let h = c.reserve_extent(key, 4096).expect("reserve");
                if i % 2 == 0 {
                    h.publish().expect("publish");
                } else {
                    h.abort();
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread join");
    }

    let extents = c.get_extents();
    assert_eq!(extents.len(), num_threads * ops_per_thread / 2);
}

#[test]
fn concurrent_publish_remove() {
    let (c, _data_mock, _metadata_mock) = create_test_component(DISK_SIZE, METADATA_DISK_SIZE);
    c.format(format_params()).expect("format");

    for k in 0..800u64 {
        let h = c.reserve_extent(k, 4096).expect("reserve");
        h.publish().expect("publish");
    }

    let c = Arc::new(c);
    let mut handles = Vec::new();

    for t in 0..8u64 {
        let c = Arc::clone(&c);
        handles.push(thread::spawn(move || {
            for i in 0..100u64 {
                let key = t * 100 + i;
                c.remove_extent(key).expect("remove");
            }
        }));
    }

    for h in handles {
        h.join().expect("thread join");
    }

    assert!(c.get_extents().is_empty());
}
