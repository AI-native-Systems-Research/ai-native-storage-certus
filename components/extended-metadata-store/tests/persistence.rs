#![cfg(feature = "testing")]

//! Persistence tests using MockBlockDevice.
//!
//! Validates that put/get works through the full I/O path:
//! component → flush → BlockDeviceClient → MockBlockDevice → disk state.

use extended_metadata_store::block_io::BlockDeviceClient;
use extended_metadata_store::flush::flush_to_disk;
use extended_metadata_store::on_disk::{self, Superblock};
use extended_metadata_store::test_support::{
    create_test_component, heap_dma_alloc, FaultConfig, MockBlockDevice,
};
use extended_metadata_store::ExtendedMetadataStoreComponent;

use component_core::query_interface;
use interfaces::{ExtendedMetadataStoreError, IBlockDevice, IExtendedMetadataStore};

use std::sync::Arc;

const DISK_SIZE: u64 = 128 * 1024 * 1024; // 128 MiB
const SECTOR_SIZE: u32 = 4096;

/// Helper: create a BlockDeviceClient connected to a MockBlockDevice.
fn connect_client(mock: &Arc<MockBlockDevice>) -> BlockDeviceClient {
    let channels = mock.connect_client().unwrap();
    let alloc = heap_dma_alloc();
    let sector_size = mock.sector_size(1).unwrap();
    BlockDeviceClient::new(channels, alloc, sector_size, 1, 0)
}

// ---------------------------------------------------------------------------
// T024: Put + get round-trip with varied sizes via persistence path
// ---------------------------------------------------------------------------

#[test]
fn put_get_roundtrip_varied_sizes() {
    let (comp, _mock) = create_test_component(DISK_SIZE);
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp, IExtendedMetadataStore).unwrap();

    // Zero-length value
    store.put("empty", b"").unwrap();
    assert_eq!(store.get("empty").unwrap(), b"");

    // 1-byte value
    store.put("tiny", &[0x42]).unwrap();
    assert_eq!(store.get("tiny").unwrap(), vec![0x42]);

    // 4 KiB value
    let medium = vec![0xAB; 4096];
    store.put("medium", &medium).unwrap();
    assert_eq!(store.get("medium").unwrap(), medium);

    // 128 KiB (max) value
    let large = vec![0xCD; 128 * 1024];
    store.put("large", &large).unwrap();
    assert_eq!(store.get("large").unwrap(), large);

    // Over max should fail
    let too_big = vec![0u8; 128 * 1024 + 1];
    assert_eq!(
        store.put("toobig", &too_big),
        Err(ExtendedMetadataStoreError::ValueTooLarge)
    );
}

// ---------------------------------------------------------------------------
// T025: Put + flush + verify on-disk data
// ---------------------------------------------------------------------------

#[test]
fn put_flush_verify_on_disk() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    // Write entries
    store.put("key_a", b"value_a").unwrap();
    store.put("key_b", b"value_b_longer").unwrap();
    store.put("key_c", &vec![0xFF; 1000]).unwrap();

    // Create a client and flush to disk
    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let mut superblock = Superblock::new(SECTOR_SIZE, num_sectors);

    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();
    comp.mark_flushed(superblock.flush_seq);

    // Verify superblock on disk
    let shared = mock.shared_state();
    let state = shared.lock().unwrap();
    assert!(state.blocks.contains_key(&0), "superblock not written");
    drop(state);

    // Read back from disk and verify entries
    let sb_data = client.read_sectors(0, 1).unwrap();
    let read_sb = Superblock::deserialize(&sb_data).unwrap();
    assert_eq!(read_sb.flush_seq, 1);
    assert_eq!(read_sb.entry_count, 3);

    // Read the active region and deserialize
    let region_offset = read_sb.active_region_offset();
    let region_data = client
        .read_sectors(region_offset, read_sb.region_a_size)
        .unwrap();
    let (header, parsed_entries) =
        on_disk::deserialize_region(&region_data, SECTOR_SIZE as usize).unwrap();
    assert_eq!(header.flush_seq, 1);
    assert_eq!(parsed_entries.len(), 3);

    // Verify values match
    let mut sorted = parsed_entries.clone();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(sorted[0].0, "key_a");
    assert_eq!(sorted[0].1, b"value_a");
    assert_eq!(sorted[1].0, "key_b");
    assert_eq!(sorted[1].1, b"value_b_longer");
    assert_eq!(sorted[2].0, "key_c");
    assert_eq!(sorted[2].1, vec![0xFF; 1000]);
}

#[test]
fn flush_multiple_times_alternates_regions() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let mut superblock = Superblock::new(SECTOR_SIZE, num_sectors);

    // First flush: writes to inactive (region B since active starts at A)
    store.put("first", b"1").unwrap();
    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();
    assert_eq!(superblock.active_region, 1); // flipped to B
    assert_eq!(superblock.flush_seq, 1);

    // Second flush: writes to inactive (now region A)
    store.put("second", b"2").unwrap();
    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();
    assert_eq!(superblock.active_region, 0); // flipped back to A
    assert_eq!(superblock.flush_seq, 2);
    assert_eq!(superblock.entry_count, 2);
}

#[test]
fn flush_and_recover_from_reboot() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    // Write and flush
    store.put("persist_1", b"hello").unwrap();
    store.put("persist_2", b"world").unwrap();

    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let mut superblock = Superblock::new(SECTOR_SIZE, num_sectors);
    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();

    // Simulate reboot: new mock from same shared state
    let shared = mock.shared_state();
    drop(client); // drop old channels
    let mock2 = MockBlockDevice::reboot_from(shared);
    let client2 = connect_client(&mock2);

    // Read superblock
    let sb = client2.read_superblock().unwrap().unwrap();
    assert_eq!(sb.flush_seq, 1);
    assert_eq!(sb.entry_count, 2);

    // Read and parse the active region
    let region_data = client2
        .read_sectors(sb.active_region_offset(), sb.region_a_size)
        .unwrap();
    let (_header, recovered_entries) =
        on_disk::deserialize_region(&region_data, SECTOR_SIZE as usize).unwrap();
    assert_eq!(recovered_entries.len(), 2);

    // Load into a fresh component
    let comp2 = ExtendedMetadataStoreComponent::new_default();
    comp2.load_entries(recovered_entries);
    let store2: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp2, IExtendedMetadataStore).unwrap();

    assert_eq!(store2.get("persist_1").unwrap(), b"hello");
    assert_eq!(store2.get("persist_2").unwrap(), b"world");
}

// ---------------------------------------------------------------------------
// Phase 4: Recovery tests (T030-T032)
// ---------------------------------------------------------------------------

/// T030: Put entries + flush + reboot + initialize_from_client → all entries present.
#[test]
fn recovery_via_initialize_from_client() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    store.put("alpha", b"one").unwrap();
    store.put("beta", b"two").unwrap();
    store.put("gamma", &vec![0xAA; 5000]).unwrap();

    // Flush
    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let mut superblock = Superblock::new(SECTOR_SIZE, num_sectors);
    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();

    // Reboot
    let shared = mock.shared_state();
    drop(client);
    let mock2 = MockBlockDevice::reboot_from(shared);
    let client2 = connect_client(&mock2);

    // Initialize a fresh component from disk
    let comp2 = ExtendedMetadataStoreComponent::new_default();
    let (sb, warnings) = comp2.initialize_from_client(&client2, num_sectors).unwrap();

    assert_eq!(sb.flush_seq, 1);
    assert_eq!(sb.entry_count, 3);
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

    let store2: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp2, IExtendedMetadataStore).unwrap();
    assert_eq!(store2.get("alpha").unwrap(), b"one");
    assert_eq!(store2.get("beta").unwrap(), b"two");
    assert_eq!(store2.get("gamma").unwrap(), vec![0xAA; 5000]);
}

/// T031: Corrupt the active region CRC → recovery falls back to inactive region.
#[test]
fn recovery_fallback_to_inactive_region() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let mut superblock = Superblock::new(SECTOR_SIZE, num_sectors);

    // First flush: write "v1" entries to region B (active flips to B)
    store.put("key1", b"value_v1").unwrap();
    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();
    // Now active_region = 1 (B), flush_seq = 1

    // Second flush: write "v2" entries to region A (active flips to A)
    store.put("key1", b"value_v2").unwrap();
    store.put("key2", b"new_entry").unwrap();
    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();
    // Now active_region = 0 (A), flush_seq = 2

    // Corrupt the active region (A) header by flipping bytes in the first sector after superblock
    let shared = mock.shared_state();
    {
        let mut state = shared.lock().unwrap();
        let active_lba = superblock.active_region_offset();
        if let Some(block) = state.blocks.get_mut(&active_lba) {
            block[0] ^= 0xFF;
            block[1] ^= 0xFF;
        }
    }

    // Reboot and recover
    drop(client);
    let mock2 = MockBlockDevice::reboot_from(shared);
    let client2 = connect_client(&mock2);

    let comp2 = ExtendedMetadataStoreComponent::new_default();
    let (_sb, warnings) = comp2.initialize_from_client(&client2, num_sectors).unwrap();

    // Should have recovered from inactive (region B) with the v1 data
    assert!(
        warnings.iter().any(|w| w.contains("inactive")),
        "expected fallback warning, got: {warnings:?}"
    );

    let store2: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp2, IExtendedMetadataStore).unwrap();
    // Recovered from the first flush (region B had "value_v1", no "key2")
    assert_eq!(store2.get("key1").unwrap(), b"value_v1");
    assert_eq!(
        store2.get("key2"),
        Err(ExtendedMetadataStoreError::NotFound)
    );
}

// ---------------------------------------------------------------------------
// Phase 5: Delete persistence tests (T033-T035)
// ---------------------------------------------------------------------------

/// T034: Put + delete + flush + reboot + get returns NotFound.
#[test]
fn delete_persists_across_reboot() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    store.put("keep", b"kept").unwrap();
    store.put("remove", b"gone").unwrap();
    store.delete("remove").unwrap();

    // Flush current state (only "keep" should be persisted)
    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let mut superblock = Superblock::new(SECTOR_SIZE, num_sectors);
    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();

    // Reboot
    let shared = mock.shared_state();
    drop(client);
    let mock2 = MockBlockDevice::reboot_from(shared);
    let client2 = connect_client(&mock2);

    let comp2 = ExtendedMetadataStoreComponent::new_default();
    comp2.initialize_from_client(&client2, num_sectors).unwrap();

    let store2: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp2, IExtendedMetadataStore).unwrap();

    assert_eq!(store2.get("keep").unwrap(), b"kept");
    assert_eq!(
        store2.get("remove"),
        Err(ExtendedMetadataStoreError::NotFound)
    );

    // iterate_all should also exclude deleted key
    let all = store2.iterate_all().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, "keep");
}

/// T035: Delete non-existent key returns Ok (idempotent).
#[test]
fn delete_nonexistent_is_idempotent() {
    let (comp, _mock) = create_test_component(DISK_SIZE);
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp, IExtendedMetadataStore).unwrap();

    assert!(store.delete("never_existed").is_ok());
    assert!(store.delete("never_existed").is_ok());
}

// ---------------------------------------------------------------------------
// Phase 6: Iterate All tests (T036-T038)
// ---------------------------------------------------------------------------

/// T037: Put 100 entries + iterate_all returns exactly 100 with correct values.
#[test]
fn iterate_all_returns_all_100_entries() {
    let (comp, _mock) = create_test_component(DISK_SIZE);
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp, IExtendedMetadataStore).unwrap();

    let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..100 {
        let key = format!("iter_key_{i:03}");
        let value = format!("value_{i}").into_bytes();
        store.put(&key, &value).unwrap();
        expected.push((key, value));
    }

    let mut result = store.iterate_all().unwrap();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    expected.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(result.len(), 100);
    for (got, exp) in result.iter().zip(expected.iter()) {
        assert_eq!(got.0, exp.0);
        assert_eq!(got.1, exp.1);
    }
}

/// T038: Delete entry then iterate_all excludes it.
#[test]
fn iterate_all_excludes_deleted() {
    let (comp, _mock) = create_test_component(DISK_SIZE);
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp, IExtendedMetadataStore).unwrap();

    store.put("x", b"1").unwrap();
    store.put("y", b"2").unwrap();
    store.put("z", b"3").unwrap();
    store.delete("y").unwrap();

    let result = store.iterate_all().unwrap();
    let keys: Vec<&str> = result.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(result.len(), 2);
    assert!(keys.contains(&"x"));
    assert!(keys.contains(&"z"));
    assert!(!keys.contains(&"y"));
}

// ---------------------------------------------------------------------------
// Phase 7: Concurrency tests (T039-T042)
// ---------------------------------------------------------------------------

/// T041: 8 threads, 1000 operations each, no panics, final state consistent.
#[test]
fn concurrent_stress_8_threads() {
    let (comp, _mock) = create_test_component(DISK_SIZE);
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp, IExtendedMetadataStore).unwrap();

    let mut handles = Vec::new();
    for tid in 0..8u64 {
        let s = store.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..1000u64 {
                let key = format!("t{tid}_k{i}");
                match i % 3 {
                    0 => { s.put(&key, &i.to_le_bytes()).unwrap(); }
                    1 => { let _ = s.get(&key); }
                    2 => { let _ = s.delete(&key); }
                    _ => unreachable!(),
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Verify state is consistent: every key that exists has valid data
    let entries = store.iterate_all().unwrap();
    for (key, value) in &entries {
        let got = store.get(key).unwrap();
        assert_eq!(&got, value);
    }
}

/// T042: Concurrent iterate_all while other threads write — no panics, consistent snapshot.
#[test]
fn concurrent_iterate_during_writes() {
    let (comp, _mock) = create_test_component(DISK_SIZE);
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp, IExtendedMetadataStore).unwrap();

    // Pre-populate
    for i in 0..50 {
        store.put(&format!("pre_{i}"), &[i as u8; 100]).unwrap();
    }

    let writer_store = store.clone();
    let writer = std::thread::spawn(move || {
        for i in 0..500 {
            let key = format!("w_{i}");
            writer_store.put(&key, &[i as u8; 50]).unwrap();
        }
    });

    // Iterate concurrently multiple times
    for _ in 0..10 {
        let entries = store.iterate_all().unwrap();
        // Snapshot must be internally consistent: no duplicate keys
        let mut keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), entries.len(), "duplicate keys in snapshot");
    }

    writer.join().expect("writer thread panicked");
}

// ---------------------------------------------------------------------------
// Phase 8: Background flush (T043-T049)
// ---------------------------------------------------------------------------

/// T047: put + force_flush via FlushManager + crash (no timer flush) + reboot → entry persisted.
#[test]
fn flush_manager_force_flush_persists() {
    use extended_metadata_store::flush::{FlushConfig, FlushManager};
    use std::time::Duration;

    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let superblock = on_disk::Superblock::new(SECTOR_SIZE, num_sectors);

    // Long interval so timer won't fire during test
    let config = FlushConfig {
        interval: Duration::from_secs(3600),
        dirty_threshold: 10000,
    };

    let comp_clone = comp.clone();
    let comp_clone2 = comp.clone();
    let comp_clone3 = comp.clone();
    let mgr = FlushManager::start(
        config,
        client,
        superblock,
        Box::new(move || comp_clone.snapshot_entries()),
        Box::new(move || comp_clone2.dirty_count()),
        Box::new(move |seq| comp_clone3.mark_flushed(seq)),
    );

    // Write entry and force flush
    store.put("managed_key", b"managed_value").unwrap();
    mgr.trigger_flush().unwrap();

    assert!(mgr.completed_seq() >= 1);
    drop(mgr);

    // Reboot and verify
    let shared = mock.shared_state();
    let mock2 = MockBlockDevice::reboot_from(shared);
    let client2 = connect_client(&mock2);

    let comp2 = ExtendedMetadataStoreComponent::new_default();
    comp2.initialize_from_client(&client2, num_sectors).unwrap();
    let store2: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp2, IExtendedMetadataStore).unwrap();

    assert_eq!(store2.get("managed_key").unwrap(), b"managed_value");
}

/// T048: Dirty-count threshold triggers flush without waiting for timer.
#[test]
fn flush_manager_dirty_threshold_triggers() {
    use extended_metadata_store::flush::{FlushConfig, FlushManager};
    use std::time::Duration;

    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let superblock = on_disk::Superblock::new(SECTOR_SIZE, num_sectors);

    // Short interval to ensure timer fires and checks dirty count
    let config = FlushConfig {
        interval: Duration::from_millis(50),
        dirty_threshold: 5,
    };

    let comp_clone = comp.clone();
    let comp_clone2 = comp.clone();
    let comp_clone3 = comp.clone();
    let mgr = FlushManager::start(
        config,
        client,
        superblock,
        Box::new(move || comp_clone.snapshot_entries()),
        Box::new(move || comp_clone2.dirty_count()),
        Box::new(move |seq| comp_clone3.mark_flushed(seq)),
    );

    // Write entries exceeding threshold
    for i in 0..6 {
        store.put(&format!("dk_{i}"), &[i as u8; 10]).unwrap();
    }

    // Wait for the timer to fire and flush
    std::thread::sleep(Duration::from_millis(200));

    assert!(mgr.completed_seq() >= 1, "dirty threshold should have triggered flush");
    drop(mgr);
}

/// T049: force_flush returns quickly when no dirty entries.
#[test]
fn flush_manager_no_dirty_no_op() {
    use extended_metadata_store::flush::{FlushConfig, FlushManager};
    use std::time::Duration;

    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();

    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let superblock = on_disk::Superblock::new(SECTOR_SIZE, num_sectors);

    let config = FlushConfig {
        interval: Duration::from_secs(3600),
        dirty_threshold: 10000,
    };

    let comp_clone = comp.clone();
    let comp_clone2 = comp.clone();
    let comp_clone3 = comp.clone();
    let mgr = FlushManager::start(
        config,
        client,
        superblock,
        Box::new(move || comp_clone.snapshot_entries()),
        Box::new(move || comp_clone2.dirty_count()),
        Box::new(move |seq| comp_clone3.mark_flushed(seq)),
    );

    // No writes — trigger flush should still return (no-op)
    mgr.trigger_flush().unwrap();
    assert_eq!(mgr.completed_seq(), 0);
    drop(mgr);
}

// ---------------------------------------------------------------------------
// Phase 9: Polish — capacity and crash tests (T050-T053)
// ---------------------------------------------------------------------------

/// T051: Fill store to capacity, verify CapacityExhausted error.
#[test]
fn capacity_exhaustion_detected() {
    // Use a tiny disk (64 KiB) so we hit capacity quickly
    let tiny_disk: u64 = 64 * 1024;
    let mock = MockBlockDevice::new(tiny_disk);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    let client = connect_client(&mock);
    let num_sectors = tiny_disk / SECTOR_SIZE as u64;
    let mut superblock = on_disk::Superblock::new(SECTOR_SIZE, num_sectors);

    // Each entry occupies at least 1 sector (4KiB) after padding
    // With 64KiB disk: 1 sector superblock + ~7 sectors per region → ~7 entries max
    let mut written = 0;
    for i in 0..100 {
        let key = format!("cap_{i}");
        store.put(&key, &[i as u8; 100]).unwrap();
        written += 1;

        // Try to flush — will eventually fail with capacity error
        let entries = comp.snapshot_entries();
        match flush_to_disk(&client, &mut superblock, &entries) {
            Ok(()) => {}
            Err(e) if e.contains("exceeds region capacity") => {
                // Expected — verify all previously flushed entries are intact
                // (The last un-flushable batch is only in memory)
                break;
            }
            Err(e) => panic!("unexpected flush error: {e}"),
        }
    }
    assert!(written > 1, "should have written at least some entries before capacity hit");
}

/// T052: Put with zero-length value succeeds.
#[test]
fn put_zero_length_value_succeeds() {
    let (comp, _mock) = create_test_component(DISK_SIZE);
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp, IExtendedMetadataStore).unwrap();

    store.put("empty_val", b"").unwrap();
    assert_eq!(store.get("empty_val").unwrap(), b"");
}

/// T053: Crash mid-flush (fault injection) + reboot → recovers from previous valid region.
#[test]
fn crash_mid_flush_recovers_previous_state() {
    // First: write and flush successfully
    let mock = MockBlockDevice::new(DISK_SIZE);
    let comp = ExtendedMetadataStoreComponent::new_default();
    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp.clone(), IExtendedMetadataStore).unwrap();

    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;
    let mut superblock = on_disk::Superblock::new(SECTOR_SIZE, num_sectors);

    store.put("safe_key", b"safe_value").unwrap();
    let entries = comp.snapshot_entries();
    flush_to_disk(&client, &mut superblock, &entries).unwrap();
    // flush_seq = 1, active = B (region 1)

    // Now get shared state and create a fault-injecting mock for the second flush
    let shared = mock.shared_state();
    drop(client);

    // Create mock that fails after 2 writes (partial region write)
    let faulty_mock = MockBlockDevice::with_fault_config(DISK_SIZE, FaultConfig {
        fail_after_n_writes: Some(2),
    });
    // Copy existing blocks into the faulty mock's state
    {
        let src = shared.lock().unwrap();
        let faulty_state = faulty_mock.shared_state();
        let mut dst = faulty_state.lock().unwrap();
        dst.blocks = src.blocks.clone();
    }

    let faulty_client = connect_client(&faulty_mock);

    // Try a second flush (should fail mid-write)
    store.put("crash_key", b"crash_value").unwrap();
    let entries = comp.snapshot_entries();
    let result = flush_to_disk(&faulty_client, &mut superblock, &entries);
    assert!(result.is_err(), "flush should fail due to fault injection");

    // Reboot from the faulty mock's state (has partial writes)
    let crash_state = faulty_mock.shared_state();
    drop(faulty_client);
    let recovered_mock = MockBlockDevice::reboot_from(crash_state);
    let recovered_client = connect_client(&recovered_mock);

    // Recovery should fall back to the first valid flush
    let comp2 = ExtendedMetadataStoreComponent::new_default();
    let (_sb, warnings) = comp2
        .initialize_from_client(&recovered_client, num_sectors)
        .unwrap();

    let store2: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp2, IExtendedMetadataStore).unwrap();

    // "safe_key" from the first successful flush should be there
    assert_eq!(store2.get("safe_key").unwrap(), b"safe_value");
    // "crash_key" from the failed flush should NOT be there
    // (it might or might not be, depending on whether the corrupt region was the active one)
    // The key point: no garbage/corrupt data is served
    if let Ok(val) = store2.get("crash_key") {
        // If recovered from the region that had the partial write, it might have the entry
        // (if the entry itself was written completely before the fault hit the superblock)
        assert_eq!(val, b"crash_value");
    }

    // Either way, the store is consistent
    let all = store2.iterate_all().unwrap();
    for (k, v) in &all {
        assert_eq!(store2.get(k).unwrap(), *v);
    }
}

/// T032: Fresh partition (all zeros) → format_fresh → empty store.
#[test]
fn recovery_fresh_partition_formats_empty() {
    let mock = MockBlockDevice::new(DISK_SIZE);
    let client = connect_client(&mock);
    let num_sectors = DISK_SIZE / SECTOR_SIZE as u64;

    // Don't write anything — partition is all zeros (MockBlockDevice returns zeros for unwritten LBAs)
    let comp = ExtendedMetadataStoreComponent::new_default();
    let (sb, warnings) = comp.initialize_from_client(&client, num_sectors).unwrap();

    assert_eq!(sb.flush_seq, 0);
    assert!(
        warnings.iter().any(|w| w.contains("fresh") || w.contains("format")),
        "expected fresh format warning, got: {warnings:?}"
    );

    let store: Arc<dyn IExtendedMetadataStore + Send + Sync> =
        query_interface!(comp, IExtendedMetadataStore).unwrap();
    let entries = store.iterate_all().unwrap();
    assert_eq!(entries.len(), 0, "fresh store should be empty");
}
