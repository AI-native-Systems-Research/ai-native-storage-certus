#![cfg(feature = "testing")]

//! Persistence tests using MockBlockDevice.
//!
//! Validates that put/get works through the full I/O path:
//! component → flush → BlockDeviceClient → MockBlockDevice → disk state.

use extended_metadata_store::block_io::BlockDeviceClient;
use extended_metadata_store::flush::flush_to_disk;
use extended_metadata_store::on_disk::{self, Superblock};
use extended_metadata_store::recovery;
use extended_metadata_store::test_support::{
    create_test_component, create_test_component_from_state, heap_dma_alloc, FaultConfig,
    MockBlockDevice,
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
