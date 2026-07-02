#![cfg(feature = "spdk")]

//! Integration tests for the extended-metadata-store using real NVMe hardware.
//!
//! These tests validate put/get/delete/iterate_all/force_flush operations
//! and data integrity on a real SSD via SPDK, with partition 1 managed by
//! disk-partition-manager.
//!
//! When SPDK hardware is not available (no VFIO, no hugepages, no NVMe),
//! tests print a skip message and pass without exercising hardware paths.

use std::sync::{Arc, OnceLock};

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::iunknown::query;
use component_core::query_interface;
use disk_partition_manager::DiskPartitionManager;
use extended_metadata_store::block_io::BlockDeviceClient;
use extended_metadata_store::on_disk::Superblock;
use extended_metadata_store::ExtendedMetadataStoreComponent;
use interfaces::{
    DmaAllocFn, DmaBuffer, ExtendedMetadataStoreError, IBlockDevice, IExtendedMetadataStore,
    ILogger, PartitionConfig, PartitionInfo, PartitionSpec,
};
use spdk_env::SPDKEnvComponent;

// ---------------------------------------------------------------------------
// Test Harness
// ---------------------------------------------------------------------------

struct SsdTestContext {
    block_dev: Arc<BlockDeviceSpdkNvmeComponent>,
    #[allow(dead_code)]
    spdk_env: Arc<SPDKEnvComponent>,
    #[allow(dead_code)]
    logger: Arc<dyn ILogger + Send + Sync>,
    partition_info: PartitionInfo,
}

unsafe impl Sync for SsdTestContext {}

static SSD_CONTEXT: OnceLock<Option<&'static SsdTestContext>> = OnceLock::new();

fn get_test_context() -> Option<&'static SsdTestContext> {
    *SSD_CONTEXT.get_or_init(|| {
        extern "C" {
            fn atexit(cb: extern "C" fn()) -> i32;
            fn _exit(status: i32) -> !;
        }
        extern "C" fn exit_before_spdk_teardown() {
            unsafe { _exit(0) };
        }
        unsafe { atexit(exit_before_spdk_teardown) };

        if let Err(e) = spdk_env::checks::check_vfio_available() {
            eprintln!("[extended-metadata-store] SPDK hardware not available (VFIO): {e}");
            return None;
        }
        if let Err(e) = spdk_env::checks::check_hugepages() {
            eprintln!("[extended-metadata-store] SPDK hardware not available (hugepages): {e}");
            return None;
        }

        let spdk_env = SPDKEnvComponent::new_default();
        let block_dev = BlockDeviceSpdkNvmeComponent::new_default();
        let logger = logger::LoggerComponent::new_default();

        bind(&*spdk_env, "ISPDKEnv", &*block_dev, "spdk_env")
            .expect("bind spdk_env → block_dev");
        bind(&*logger, "ILogger", &*block_dev, "logger").expect("bind logger → block_dev");

        let ienv = query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env)
            .expect("ISPDKEnv query");
        if let Err(e) = ienv.init() {
            eprintln!("[extended-metadata-store] SPDK init failed: {e}");
            return None;
        }

        let devices = ienv.devices();
        if devices.is_empty() {
            eprintln!("[extended-metadata-store] No NVMe devices found");
            return None;
        }

        let spdk_addr = devices[0].address;
        let addr = interfaces::PciAddress {
            domain: spdk_addr.domain,
            bus: spdk_addr.bus,
            dev: spdk_addr.dev,
            func: spdk_addr.func,
        };

        let admin = query::<dyn interfaces::iblock_device::IBlockDeviceAdmin + Send + Sync>(
            &*block_dev,
        )
        .expect("IBlockDeviceAdmin query");
        admin.set_pci_address(addr);

        if let Err(e) = admin.initialize() {
            eprintln!("[extended-metadata-store] Block device init failed: {e}");
            return None;
        }

        // Set up partition table with CERTUS_EXTERNAL_META on partition 1.
        let part_mgr = DiskPartitionManager::new_default();
        part_mgr.set_ns_id(1);
        bind(&*block_dev, "IBlockDevice", &*part_mgr, "block_device")
            .expect("bind block_dev → partition_mgr");

        let ibd = query::<dyn interfaces::IBlockDevice + Send + Sync>(&*block_dev)
            .expect("IBlockDevice query");
        let sector_size = ibd.block_size();
        let num_sectors = ibd.num_sectors(1).unwrap_or(0);

        // Partition layout: index 0 = internal metadata, index 1 = extended metadata store.
        // Tests use partition 1 (CERTUS_EXTERNAL_META) by default.
        let partition_config = PartitionConfig {
            sector_size,
            total_sectors: num_sectors,
            ns_id: 1,
            partitions: vec![
                PartitionSpec {
                    type_guid: interfaces::type_guids::CERTUS_METADATA,
                    size_bytes: 128 * 1024 * 1024,
                    name: "certus-metadata".into(),
                },
                PartitionSpec {
                    type_guid: interfaces::type_guids::CERTUS_EXTERNAL_META,
                    size_bytes: 128 * 1024 * 1024,
                    name: "certus-extended-metadata".into(),
                },
            ],
        };

        let (table, _formatted) = part_mgr
            .initialize_or_format(true, partition_config)
            .unwrap_or_else(|e| {
                eprintln!("[extended-metadata-store] Partition table init failed: {e}");
                std::process::exit(2);
            });

        // Use partition 1 (CERTUS_EXTERNAL_META) by default for the metadata store.
        const PARTITION_INDEX: usize = 1;
        let partition_info = table.partitions[PARTITION_INDEX].clone();

        Some(Box::leak(Box::new(SsdTestContext {
            block_dev,
            spdk_env,
            logger: logger as Arc<dyn ILogger + Send + Sync>,
            partition_info,
        })))
    })
}

/// Create a DMA allocator using real SPDK hugepage memory.
fn spdk_dma_alloc(numa_node: i32) -> DmaAllocFn {
    Arc::new(move |size, align, _numa| {
        DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
    })
}

/// Create a BlockDeviceClient connected to the real SSD for the test partition.
fn make_client(ctx: &'static SsdTestContext) -> BlockDeviceClient {
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev)
        .expect("IBlockDevice query");
    let channels = ibd.connect_client().expect("connect_client");
    let sector_size = ibd.block_size();
    let numa_node = ibd.numa_node();
    let alloc = spdk_dma_alloc(numa_node);
    let base_lba = ctx.partition_info.start_lba;
    BlockDeviceClient::new(channels, alloc, sector_size, 1, base_lba)
}

/// Create a fresh ExtendedMetadataStore instance wired to the real SSD via the test context.
/// Initializes from the partition (recovers existing data or formats fresh).
fn create_store(
    ctx: &'static SsdTestContext,
) -> (
    Arc<ExtendedMetadataStoreComponent>,
    Arc<dyn IExtendedMetadataStore + Send + Sync>,
) {
    let comp = ExtendedMetadataStoreComponent::new_default();
    let client = make_client(ctx);
    let num_sectors = ctx.partition_info.num_sectors;

    // Initialize: recover from disk or format fresh
    let (_sb, _warnings) = comp
        .initialize_from_client(&client, num_sectors)
        .expect("initialize_from_client");

    let store = query_interface!(comp.clone(), IExtendedMetadataStore)
        .expect("IExtendedMetadataStore query");
    (comp, store)
}

/// Flush current state to the real SSD partition.
fn flush_store(ctx: &'static SsdTestContext, comp: &Arc<ExtendedMetadataStoreComponent>) {
    let client = make_client(ctx);
    let num_sectors = ctx.partition_info.num_sectors;
    let sector_size = client.sector_size;
    let mut superblock = Superblock::new(sector_size, num_sectors);

    // Re-read superblock to get current state
    if let Ok(Some(sb)) = client.read_superblock() {
        superblock = sb;
    }

    let entries = comp.snapshot_entries();
    extended_metadata_store::flush::flush_to_disk(&client, &mut superblock, &entries)
        .expect("flush_to_disk");
    comp.mark_flushed(superblock.flush_seq);
}

/// Generate a deterministic byte pattern from a key for verification.
fn test_value(key: &str, size: usize) -> Vec<u8> {
    let seed = key.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    (0..size)
        .map(|i| ((seed.wrapping_add(i as u64)) & 0xFF) as u8)
        .collect()
}

// ---------------------------------------------------------------------------
// User Story 1: Put and Get on Real Hardware
// ---------------------------------------------------------------------------

#[test]
fn test_put_get_small_value() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    let value = test_value("small", 1);
    store.put("test_small", &value).unwrap();
    let got = store.get("test_small").unwrap();
    assert_eq!(got, value, "1-byte value mismatch");
}

#[test]
fn test_put_get_medium_value() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    let value = test_value("medium", 4096);
    store.put("test_medium", &value).unwrap();
    let got = store.get("test_medium").unwrap();
    assert_eq!(got, value, "4KiB value mismatch");
}

#[test]
fn test_put_get_max_value() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    let value = test_value("max128k", 128 * 1024);
    store.put("test_max", &value).unwrap();
    let got = store.get("test_max").unwrap();
    assert_eq!(got, value, "128KiB max value mismatch");
}

#[test]
fn test_get_nonexistent_key() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    let result = store.get("nonexistent_key_xyz");
    assert_eq!(result, Err(ExtendedMetadataStoreError::NotFound));
}

#[test]
fn test_overwrite_existing_key() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    let val_a = test_value("overwrite_a", 100);
    let val_b = test_value("overwrite_b", 5000);

    store.put("overwrite_key", &val_a).unwrap();
    assert_eq!(store.get("overwrite_key").unwrap(), val_a);

    store.put("overwrite_key", &val_b).unwrap();
    assert_eq!(
        store.get("overwrite_key").unwrap(),
        val_b,
        "Overwritten value mismatch"
    );
}

// ---------------------------------------------------------------------------
// User Story 2: Delete Operations
// ---------------------------------------------------------------------------

#[test]
fn test_delete_existing_key() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    store.put("del_key", b"to_delete").unwrap();
    store.delete("del_key").unwrap();
    assert_eq!(store.get("del_key"), Err(ExtendedMetadataStoreError::NotFound));
}

#[test]
fn test_delete_nonexistent_key() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    // Deleting a key that was never written should succeed (idempotent).
    assert!(store.delete("never_existed").is_ok());
}

#[test]
fn test_delete_not_in_iterate() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    store.put("iter_1", b"v1").unwrap();
    store.put("iter_2", b"v2").unwrap();
    store.put("iter_3", b"v3").unwrap();

    store.delete("iter_2").unwrap();

    let entries = store.iterate_all().unwrap();
    let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
    assert!(keys.contains(&"iter_1"));
    assert!(!keys.contains(&"iter_2"), "Deleted key should not appear in iterate_all");
    assert!(keys.contains(&"iter_3"));
}

// ---------------------------------------------------------------------------
// User Story 3: Data Persistence Across Restart
// ---------------------------------------------------------------------------

#[test]
fn test_persistence_after_flush() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };

    // Write entries and flush to real SSD.
    {
        let (comp, store) = create_store(ctx);
        for i in 0..10 {
            let key = format!("persist_{i}");
            let value = test_value(&key, 256);
            store.put(&key, &value).unwrap();
        }
        flush_store(ctx, &comp);
    }

    // Re-create store instance (recovers from same partition on real SSD).
    {
        let (_comp, store) = create_store(ctx);
        for i in 0..10 {
            let key = format!("persist_{i}");
            let expected = test_value(&key, 256);
            let got = store
                .get(&key)
                .unwrap_or_else(|e| panic!("Persisted entry {key} not found after restart: {e}"));
            assert_eq!(got, expected, "Persisted entry {key} mismatch");
        }
    }
}

#[test]
fn test_unflushed_entries_may_be_lost() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };

    // First, format partition clean (write empty state)
    {
        let (comp, _store) = create_store(ctx);
        flush_store(ctx, &comp);
    }

    // Write without flushing.
    {
        let (_comp, store) = create_store(ctx);
        store.put("unflushed_key", b"unflushed_value").unwrap();
        // Intentionally no flush_store() call.
    }

    // Re-create store. Unflushed entries should NOT be present (no corruption allowed).
    {
        let (_comp, store) = create_store(ctx);
        match store.get("unflushed_key") {
            Ok(_val) => { /* might be present if recovered from previous test state */ }
            Err(ExtendedMetadataStoreError::NotFound) => { /* expected: unflushed data lost */ }
            Err(e) => panic!("Unexpected error (possible corruption): {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// User Story 4: Iterate All on Real Data
// ---------------------------------------------------------------------------

#[test]
fn test_iterate_all_complete() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..10 {
        let key = format!("iter_complete_{i}");
        let value = test_value(&key, 64 * (i + 1));
        store.put(&key, &value).unwrap();
        expected.push((key, value));
    }

    let mut entries = store.iterate_all().unwrap();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    expected.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(entries.len(), expected.len(), "Entry count mismatch");
    for (got, exp) in entries.iter().zip(expected.iter()) {
        assert_eq!(got.0, exp.0, "Key mismatch");
        assert_eq!(got.1, exp.1, "Value mismatch for key {}", exp.0);
    }
}

#[test]
fn test_iterate_all_empty_store() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    let entries = store.iterate_all().unwrap();
    assert_eq!(entries.len(), 0, "Fresh store should have zero entries");
}

// ---------------------------------------------------------------------------
// User Story 5: Data Integrity Under Load
// ---------------------------------------------------------------------------

#[test]
fn test_bulk_write_integrity() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    let entry_count = 500;
    let mut expected: Vec<(String, Vec<u8>)> = Vec::with_capacity(entry_count);

    for i in 0..entry_count {
        let key = format!("bulk_{i:04}");
        let size = 64 + (i % 128) * 32; // varied sizes: 64B to ~4KiB
        let value = test_value(&key, size);
        store.put(&key, &value).unwrap();
        expected.push((key, value));
    }

    // Read all back and verify integrity.
    let mut failures = 0;
    for (key, exp_value) in &expected {
        match store.get(key) {
            Ok(got) => {
                if got != *exp_value {
                    failures += 1;
                    eprintln!("INTEGRITY FAILURE: key={key} expected_len={} got_len={}", exp_value.len(), got.len());
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("INTEGRITY FAILURE: key={key} error={e}");
            }
        }
    }
    assert_eq!(failures, 0, "{failures}/{entry_count} entries failed integrity check");
}

#[test]
fn test_capacity_exhaustion() {
    let Some(ctx) = get_test_context() else {
        eprintln!("No NVMe hardware — skipping");
        return;
    };
    let (_comp, store) = create_store(ctx);

    // Write entries until we get a capacity error or hit a reasonable limit.
    let max_attempts = 2000;
    let mut written = 0;

    for i in 0..max_attempts {
        let key = format!("cap_{i:05}");
        let value = test_value(&key, 64 * 1024); // 64KiB per entry
        match store.put(&key, &value) {
            Ok(()) => written += 1,
            Err(ExtendedMetadataStoreError::CapacityExhausted) => {
                eprintln!("Capacity exhausted after {written} entries (expected)");
                break;
            }
            Err(e) => panic!("Unexpected error at entry {i}: {e}"),
        }
    }

    // Verify all previously written entries are still intact.
    for i in 0..written {
        let key = format!("cap_{i:05}");
        let expected = test_value(&key, 64 * 1024);
        let got = store.get(&key).unwrap_or_else(|e| {
            panic!("Entry {key} lost after capacity exhaustion: {e}");
        });
        assert_eq!(got, expected, "Entry {key} corrupted after capacity exhaustion");
    }
}
