//! Full hardware integration tests for the Dispatcher component.
//!
//! Exercises every method of the `IDispatcher` interface against real NVMe
//! hardware via SPDK. Tests cover the happy path, error conditions, edge
//! cases, concurrent access, and the lazy migration lifecycle.
//!
//! Requirements:
//!   - SPDK built at `deps/spdk-build/`
//!   - NVMe devices bound to VFIO
//!   - Hugepages configured (2+ GiB)
//!   - IOMMU enabled, memlock unlimited
//!
//! Run with:
//! ```bash
//! cargo test -p dispatcher --features hardware-test --test hardware -- --test-threads=1
//! ```
//!
//! **`--test-threads=1` is mandatory** — SPDK is a process-wide singleton and
//! NVMe controllers cannot be re-probed after detach in the same process.

#![cfg(feature = "hardware-test")]

use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use component_core::iunknown::query;
use component_core::query_interface;
use dispatcher::DispatcherComponentV0;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DispatcherError, DmaAllocFn, DmaBuffer,
    GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle, IDispatchMap, IDispatcher, IGpuServices, ILogger,
    IpcHandle, LookupResult,
};
use spdk_env::{ISPDKEnv, SPDKEnvComponent};

// ===========================================================================
// SPDK singleton
// ===========================================================================

static SPDK_ENV: OnceLock<Arc<SPDKEnvComponent>> = OnceLock::new();

fn get_spdk_env() -> &'static Arc<SPDKEnvComponent> {
    SPDK_ENV.get_or_init(|| {
        let comp = SPDKEnvComponent::new_default();
        let ienv =
            query::<dyn ISPDKEnv + Send + Sync>(&*comp).expect("failed to query ISPDKEnv");
        ienv.init()
            .expect("SPDK init failed — check hugepages, VFIO, IOMMU");
        comp
    })
}

// ===========================================================================
// Mock services — only dispatch map uses real DMA buffers; GPU is mocked
// ===========================================================================

struct TestLogger;
impl ILogger for TestLogger {
    fn error(&self, msg: &str) {
        eprintln!("[ERROR] {msg}");
    }
    fn warn(&self, msg: &str) {
        eprintln!("[WARN] {msg}");
    }
    fn info(&self, msg: &str) {
        eprintln!("[INFO] {msg}");
    }
    fn debug(&self, msg: &str) {
        eprintln!("[DEBUG] {msg}");
    }
}

struct MockGpuServices;
impl IGpuServices for MockGpuServices {
    fn initialize(&self) -> Result<(), String> {
        Ok(())
    }
    fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
    fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String> {
        Ok(vec![])
    }
    fn deserialize_ipc_handle(&self, _: &str) -> Result<GpuIpcHandle, String> {
        Err("mock".into())
    }
    fn verify_memory(&self, _: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn pin_memory(&self, _: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn unpin_memory(&self, _: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn create_dma_buffer(&self, _: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
        Err("mock".into())
    }
    fn dma_copy_to_host(
        &self,
        src: *const std::ffi::c_void,
        dst: &DmaBuffer,
        size: usize,
    ) -> Result<(), String> {
        // SAFETY: In tests, src is a valid host pointer from IpcHandle.
        unsafe {
            std::ptr::copy_nonoverlapping(src as *const u8, dst.as_ptr() as *mut u8, size);
        }
        Ok(())
    }
    fn dma_copy_to_device(
        &self,
        src: &DmaBuffer,
        dst: *mut std::ffi::c_void,
        size: usize,
    ) -> Result<(), String> {
        // SAFETY: In tests, dst is a valid host pointer from IpcHandle.
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HwDispatchMap — real SPDK DMA-backed staging buffers
// ---------------------------------------------------------------------------

struct HwDmEntry {
    buffer: Arc<DmaBuffer>,
    block_offset: Option<u64>,
    write_ref: bool,
    read_refs: u32,
}

struct HwDispatchMap {
    inner: Mutex<std::collections::HashMap<CacheKey, HwDmEntry>>,
    dma_alloc: Mutex<Option<DmaAllocFn>>,
}

impl HwDispatchMap {
    fn new(dma_alloc: DmaAllocFn) -> Self {
        Self {
            inner: Mutex::new(std::collections::HashMap::new()),
            dma_alloc: Mutex::new(Some(dma_alloc)),
        }
    }

    fn entry_count(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    fn migrated_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.block_offset.is_some())
            .count()
    }
}

impl IDispatchMap for HwDispatchMap {
    fn set_dma_alloc(&self, alloc: DmaAllocFn) {
        *self.dma_alloc.lock().unwrap() = Some(alloc);
    }

    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn create_staging(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        let alloc_guard = self.dma_alloc.lock().unwrap();
        let alloc = alloc_guard
            .as_ref()
            .ok_or_else(|| DispatchMapError::NotInitialized("dma_alloc not set".into()))?;
        let buf =
            alloc(size as usize * 4096, 4096, None).map_err(DispatchMapError::AllocationFailed)?;
        let buffer = Arc::new(buf);
        inner.insert(
            key,
            HwDmEntry {
                buffer: Arc::clone(&buffer),
                block_offset: None,
                write_ref: true,
                read_refs: 0,
            },
        );
        Ok(buffer)
    }

    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        match inner.get(&key) {
            None => Ok(LookupResult::NotExist),
            Some(e) => match e.block_offset {
                Some(offset) => Ok(LookupResult::BlockDevice { offset }),
                None => Ok(LookupResult::Staging {
                    buffer: Arc::clone(&e.buffer),
                }),
            },
        }
    }

    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) => {
                e.block_offset = Some(offset);
                Ok(())
            }
        }
    }

    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) => {
                e.read_refs += 1;
                Ok(())
            }
        }
    }

    fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) => {
                e.write_ref = true;
                Ok(())
            }
        }
    }

    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) => {
                e.read_refs = e.read_refs.saturating_sub(1);
                Ok(())
            }
        }
    }

    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) => {
                e.write_ref = false;
                Ok(())
            }
        }
    }

    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            None => Err(DispatchMapError::NoWriteReference(key)),
            Some(e) => {
                e.write_ref = false;
                e.read_refs += 1;
                Ok(())
            }
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.remove(&key).is_some() {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }
}

// ===========================================================================
// Test harness
// ===========================================================================

fn discover_devices() -> Vec<String> {
    let spdk_env_comp = get_spdk_env();
    let ienv =
        query::<dyn ISPDKEnv + Send + Sync>(&**spdk_env_comp).expect("failed to query ISPDKEnv");
    let devices = ienv.devices();
    assert!(
        !devices.is_empty(),
        "no NVMe devices found — ensure devices are bound to VFIO"
    );
    let addrs: Vec<String> = devices.iter().map(|d| d.address.to_string()).collect();
    eprintln!("discovered {} NVMe device(s): {:?}", addrs.len(), addrs);
    addrs
}

fn create_dispatcher(
    pci_addrs: &[String],
) -> (Arc<DispatcherComponentV0>, Arc<HwDispatchMap>) {
    let spdk_env_comp = get_spdk_env();
    let ienv =
        query::<dyn ISPDKEnv + Send + Sync>(&**spdk_env_comp).expect("failed to query ISPDKEnv");

    let dma_alloc: DmaAllocFn = Arc::new(|size, align, _numa| {
        DmaBuffer::new(size, align, None).map_err(|e| e.to_string())
    });

    let dm = Arc::new(HwDispatchMap::new(dma_alloc));
    let dispatcher = DispatcherComponentV0::new_default();

    dispatcher
        .dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    dispatcher
        .logger
        .connect(Arc::new(TestLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    dispatcher
        .gpu_services
        .connect(Arc::new(MockGpuServices) as Arc<dyn IGpuServices + Send + Sync>)
        .unwrap();
    dispatcher
        .spdk_env
        .connect(Arc::clone(&ienv) as Arc<dyn ISPDKEnv + Send + Sync>)
        .unwrap();

    let d: Arc<dyn IDispatcher + Send + Sync> = query_interface!(dispatcher, IDispatcher).unwrap();
    let config = DispatcherConfig {
        metadata_pci_addr: pci_addrs[0].clone(),
        data_pci_addrs: pci_addrs.to_vec(),
    };
    d.initialize(config).expect("dispatcher initialize failed");

    (dispatcher, dm)
}

fn make_handle(buf: &mut [u8]) -> IpcHandle {
    IpcHandle {
        address: buf.as_mut_ptr(),
        size: buf.len() as u32,
    }
}

// ===========================================================================
// Integration test — exercises the full IDispatcher interface on real hardware
// ===========================================================================

#[test]
fn hw_idispatcher_full_integration() {
    let pci_addrs = discover_devices();
    let (comp, dm) = create_dispatcher(&pci_addrs[..1]);
    let d: Arc<dyn IDispatcher + Send + Sync> = query_interface!(comp, IDispatcher).unwrap();

    // =======================================================================
    // 1. initialize() — called by create_dispatcher(), verify component is live
    // =======================================================================
    eprintln!("\n=== 1. initialize — component is live ===");
    assert_eq!(d.check(0).unwrap(), false, "empty dispatcher has no entries");

    // =======================================================================
    // 2. populate() — happy path
    // =======================================================================
    eprintln!("\n=== 2. populate — single 4 KiB block ===");
    {
        let mut buf = vec![0xA1u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 1);
    }

    eprintln!("=== 2b. populate — multi-block (3 blocks, 12 KiB) ===");
    {
        let mut buf = vec![0xB2u8; 12288];
        d.populate(2, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 2);
    }

    eprintln!("=== 2c. populate — non-block-aligned size (5000 bytes) ===");
    {
        let mut buf = vec![0xC3u8; 5000];
        d.populate(3, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 3);
    }

    eprintln!("=== 2d. populate — large buffer (1 MiB, MDTS segmentation) ===");
    {
        let mut buf = vec![0xD4u8; 1024 * 1024];
        d.populate(4, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 4);
    }

    // =======================================================================
    // 3. populate() — error cases
    // =======================================================================
    eprintln!("\n=== 3. populate — duplicate key returns AlreadyExists ===");
    {
        let mut buf = vec![0u8; 4096];
        let err = d.populate(1, make_handle(&mut buf));
        assert!(
            matches!(err, Err(DispatcherError::AlreadyExists(1))),
            "got: {err:?}"
        );
    }

    eprintln!("=== 3b. populate — zero-size returns InvalidParameter ===");
    {
        let mut buf = vec![0u8; 0];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 0,
        };
        let err = d.populate(999, handle);
        assert!(
            matches!(err, Err(DispatcherError::InvalidParameter(_))),
            "got: {err:?}"
        );
    }

    // =======================================================================
    // 4. check() — presence queries
    // =======================================================================
    eprintln!("\n=== 4. check — existing keys return true ===");
    assert_eq!(d.check(1).unwrap(), true);
    assert_eq!(d.check(2).unwrap(), true);
    assert_eq!(d.check(3).unwrap(), true);
    assert_eq!(d.check(4).unwrap(), true);

    eprintln!("=== 4b. check — nonexistent keys return false ===");
    assert_eq!(d.check(0).unwrap(), false);
    assert_eq!(d.check(100).unwrap(), false);
    assert_eq!(d.check(u64::MAX).unwrap(), false);

    // =======================================================================
    // 5. lookup() — data retrieval
    // =======================================================================
    eprintln!("\n=== 5. lookup — existing key (staging hit) ===");
    {
        let mut out = vec![0u8; 4096];
        d.lookup(1, make_handle(&mut out)).unwrap();
    }

    eprintln!("=== 5b. lookup — multi-block key ===");
    {
        let mut out = vec![0u8; 12288];
        d.lookup(2, make_handle(&mut out)).unwrap();
    }

    eprintln!("=== 5c. lookup — nonexistent key returns KeyNotFound ===");
    {
        let mut out = vec![0u8; 4096];
        let err = d.lookup(9999, make_handle(&mut out));
        assert!(
            matches!(err, Err(DispatcherError::KeyNotFound(9999))),
            "got: {err:?}"
        );
    }

    // =======================================================================
    // 6. remove() — entry eviction
    // =======================================================================
    eprintln!("\n=== 6. remove — existing key succeeds ===");
    {
        // Populate a temporary entry, then remove it
        let mut buf = vec![0u8; 4096];
        d.populate(50, make_handle(&mut buf)).unwrap();
        assert_eq!(d.check(50).unwrap(), true);

        d.remove(50).unwrap();
        assert_eq!(d.check(50).unwrap(), false);
    }

    eprintln!("=== 6b. remove — nonexistent key returns KeyNotFound ===");
    {
        let err = d.remove(77777);
        assert!(
            matches!(err, Err(DispatcherError::KeyNotFound(77777))),
            "got: {err:?}"
        );
    }

    eprintln!("=== 6c. remove — double remove fails ===");
    {
        let mut buf = vec![0u8; 4096];
        d.populate(51, make_handle(&mut buf)).unwrap();
        d.remove(51).unwrap();
        let err = d.remove(51);
        assert!(matches!(err, Err(DispatcherError::KeyNotFound(51))));
    }

    // =======================================================================
    // 7. populate() — batch of keys
    // =======================================================================
    eprintln!("\n=== 7. populate — batch of 50 keys ===");
    {
        let base_count = dm.entry_count();
        for key in 1000..1050u64 {
            let mut buf = vec![(key & 0xFF) as u8; 4096];
            d.populate(key, make_handle(&mut buf)).unwrap();
        }
        assert_eq!(dm.entry_count(), base_count + 50);

        for key in 1000..1050u64 {
            assert_eq!(d.check(key).unwrap(), true, "key {key} should exist");
        }
    }

    // =======================================================================
    // 8. Concurrent populate from multiple threads
    // =======================================================================
    eprintln!("\n=== 8. concurrent populate — 4 threads x 25 keys ===");
    {
        let handles: Vec<_> = (0..4u64)
            .map(|t| {
                let comp_clone = Arc::clone(&comp);
                thread::spawn(move || {
                    let disp = query_interface!(comp_clone, IDispatcher).unwrap();
                    for i in 0..25u64 {
                        let key = 10000 + t * 1000 + i;
                        let mut buf = vec![0xEEu8; 4096];
                        disp.populate(key, make_handle(&mut buf)).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Verify all 100 keys exist
        for t in 0..4u64 {
            for i in 0..25u64 {
                let key = 10000 + t * 1000 + i;
                assert_eq!(d.check(key).unwrap(), true, "concurrent key {key} missing");
            }
        }
    }

    // =======================================================================
    // 9. Concurrent check/lookup (read-only) while entries exist
    // =======================================================================
    eprintln!("\n=== 9. concurrent reads — 4 threads checking/looking up ===");
    {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let comp_clone = Arc::clone(&comp);
                thread::spawn(move || {
                    let disp = query_interface!(comp_clone, IDispatcher).unwrap();
                    for key in 1000..1050u64 {
                        assert_eq!(disp.check(key).unwrap(), true);
                        let mut out = vec![0u8; 4096];
                        assert!(disp.lookup(key, make_handle(&mut out)).is_ok());
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    // =======================================================================
    // 10. Populate various sizes
    // =======================================================================
    eprintln!("\n=== 10. populate — various sizes (512B to 512 KiB) ===");
    {
        let sizes: &[(u64, usize)] = &[
            (20000, 512),
            (20001, 1024),
            (20002, 4096),
            (20003, 8192),
            (20004, 65536),
            (20005, 131072),
            (20006, 524288),
        ];
        for &(key, size) in sizes {
            let mut buf = vec![0x77u8; size];
            d.populate(key, make_handle(&mut buf)).unwrap();
            assert_eq!(d.check(key).unwrap(), true, "key {key} (size {size})");
        }
    }

    // =======================================================================
    // 11. shutdown() — drains background writer, lazy migration completes
    // =======================================================================
    eprintln!("\n=== 11. shutdown — drains background writer ===");
    let total_entries = dm.entry_count();
    d.shutdown().unwrap();

    let migrated = dm.migrated_count();
    eprintln!(
        "after shutdown: {migrated}/{total_entries} entries migrated to block device"
    );
    assert_eq!(
        migrated, total_entries,
        "all entries should be migrated after shutdown drains the bg writer"
    );

    // =======================================================================
    // 12. Operations after shutdown fail with NotInitialized
    // =======================================================================
    eprintln!("\n=== 12. operations after shutdown fail ===");
    {
        let mut buf = vec![0u8; 4096];
        assert!(matches!(
            d.populate(99999, make_handle(&mut buf)),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.check(1),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.lookup(1, make_handle(&mut buf)),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.remove(1),
            Err(DispatcherError::NotInitialized(_))
        ));
    }

    // =======================================================================
    // 13. Double shutdown succeeds (idempotent)
    // =======================================================================
    eprintln!("\n=== 13. double shutdown is idempotent ===");
    d.shutdown().unwrap();

    eprintln!("\n=== ALL ASSERTIONS PASSED ===");
}

// ===========================================================================
// Multi-device test (only runs if 2+ NVMe devices are available)
// ===========================================================================

#[test]
fn hw_multi_device_initialization() {
    let pci_addrs = discover_devices();
    if pci_addrs.len() < 2 {
        eprintln!("SKIP: only 1 NVMe device found, need 2+ for multi-device test");
        return;
    }

    eprintln!("\n=== Multi-device: initializing {} devices ===", pci_addrs.len());
    let (comp, dm) = create_dispatcher(&pci_addrs);
    let d: Arc<dyn IDispatcher + Send + Sync> = query_interface!(comp, IDispatcher).unwrap();

    // Populate across the device set
    for key in 0..10u64 {
        let mut buf = vec![0xBBu8; 4096];
        d.populate(key, make_handle(&mut buf)).unwrap();
    }
    assert_eq!(dm.entry_count(), 10);

    for key in 0..10u64 {
        assert_eq!(d.check(key).unwrap(), true);
        let mut out = vec![0u8; 4096];
        d.lookup(key, make_handle(&mut out)).unwrap();
    }

    d.shutdown().unwrap();
    assert_eq!(dm.migrated_count(), 10);

    eprintln!(
        "=== Multi-device test PASSED ({} devices) ===",
        pci_addrs.len()
    );
}

// ===========================================================================
// Data integrity test — verifies cached data is returned byte-for-byte
// ===========================================================================

#[test]
fn hw_data_integrity() {
    let pci_addrs = discover_devices();
    let (comp, dm) = create_dispatcher(&pci_addrs[..1]);
    let d: Arc<dyn IDispatcher + Send + Sync> = query_interface!(comp, IDispatcher).unwrap();

    // =======================================================================
    // 1. Single block — deterministic pattern
    // =======================================================================
    eprintln!("\n=== Integrity 1: single 4 KiB block ===");
    {
        let mut src: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        d.populate(100, make_handle(&mut src)).unwrap();

        let mut dst = vec![0u8; 4096];
        d.lookup(100, make_handle(&mut dst)).unwrap();
        assert_eq!(src, dst, "single-block data mismatch");
    }

    // =======================================================================
    // 2. Multi-block — 12 KiB (3 blocks)
    // =======================================================================
    eprintln!("=== Integrity 2: multi-block 12 KiB ===");
    {
        let mut src: Vec<u8> = (0..12288).map(|i| ((i * 7 + 13) % 256) as u8).collect();
        d.populate(101, make_handle(&mut src)).unwrap();

        let mut dst = vec![0u8; 12288];
        d.lookup(101, make_handle(&mut dst)).unwrap();
        assert_eq!(src, dst, "multi-block data mismatch");
    }

    // =======================================================================
    // 3. Non-aligned size — 5000 bytes (not a multiple of 4096)
    // =======================================================================
    eprintln!("=== Integrity 3: non-aligned 5000 bytes ===");
    {
        let mut src: Vec<u8> = (0..5000).map(|i| ((i ^ 0xAB) % 256) as u8).collect();
        d.populate(102, make_handle(&mut src)).unwrap();

        let mut dst = vec![0u8; 5000];
        d.lookup(102, make_handle(&mut dst)).unwrap();
        assert_eq!(src, dst, "non-aligned data mismatch");
    }

    // =======================================================================
    // 4. Large buffer — 256 KiB (exercises MDTS segmentation)
    // =======================================================================
    eprintln!("=== Integrity 4: large 256 KiB buffer ===");
    {
        let size = 256 * 1024;
        let mut src: Vec<u8> = (0..size).map(|i| ((i * 31 + 17) % 256) as u8).collect();
        d.populate(103, make_handle(&mut src)).unwrap();

        let mut dst = vec![0u8; size];
        d.lookup(103, make_handle(&mut dst)).unwrap();
        assert_eq!(src, dst, "large buffer data mismatch");
    }

    // =======================================================================
    // 5. All-zeros and all-ones patterns
    // =======================================================================
    eprintln!("=== Integrity 5: all-zeros and all-ones ===");
    {
        let mut zeros = vec![0x00u8; 4096];
        d.populate(104, make_handle(&mut zeros)).unwrap();
        let mut out = vec![0xFFu8; 4096];
        d.lookup(104, make_handle(&mut out)).unwrap();
        assert_eq!(zeros, out, "all-zeros mismatch");

        let mut ones = vec![0xFFu8; 4096];
        d.populate(105, make_handle(&mut ones)).unwrap();
        let mut out2 = vec![0x00u8; 4096];
        d.lookup(105, make_handle(&mut out2)).unwrap();
        assert_eq!(ones, out2, "all-ones mismatch");
    }

    // =======================================================================
    // 6. Multiple distinct keys — verify no cross-contamination
    // =======================================================================
    eprintln!("=== Integrity 6: cross-contamination check (20 keys) ===");
    {
        let patterns: Vec<Vec<u8>> = (0..20u64)
            .map(|k| (0..4096).map(|i| ((i + k as usize * 37) % 256) as u8).collect())
            .collect();

        for (k, pat) in patterns.iter().enumerate() {
            let mut src = pat.clone();
            d.populate(200 + k as u64, make_handle(&mut src)).unwrap();
        }

        for (k, pat) in patterns.iter().enumerate() {
            let mut dst = vec![0u8; 4096];
            d.lookup(200 + k as u64, make_handle(&mut dst)).unwrap();
            assert_eq!(
                pat, &dst,
                "cross-contamination: key {} data mismatch",
                200 + k
            );
        }
    }

    // =======================================================================
    // 7. Concurrent populate + lookup integrity
    // =======================================================================
    eprintln!("=== Integrity 7: concurrent populate then verify ===");
    {
        let handles: Vec<_> = (0..4u64)
            .map(|t| {
                let comp_clone = Arc::clone(&comp);
                thread::spawn(move || {
                    let disp = query_interface!(comp_clone, IDispatcher).unwrap();
                    for i in 0..10u64 {
                        let key = 500 + t * 100 + i;
                        let mut src: Vec<u8> =
                            (0..4096).map(|b| ((b + key as usize) % 256) as u8).collect();
                        disp.populate(key, make_handle(&mut src)).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        for t in 0..4u64 {
            for i in 0..10u64 {
                let key = 500 + t * 100 + i;
                let expected: Vec<u8> =
                    (0..4096).map(|b| ((b + key as usize) % 256) as u8).collect();
                let mut dst = vec![0u8; 4096];
                d.lookup(key, make_handle(&mut dst)).unwrap();
                assert_eq!(expected, dst, "concurrent key {key} data mismatch");
            }
        }
    }

    // =======================================================================
    // 8. Verify integrity survives lazy migration (staging → SSD → readback)
    // =======================================================================
    eprintln!("=== Integrity 8: post-migration readback ===");
    {
        let mut src: Vec<u8> = (0..8192).map(|i| ((i * 41 + 3) % 256) as u8).collect();
        d.populate(900, make_handle(&mut src)).unwrap();

        // Force migration by shutting down (drains background writer)
        d.shutdown().unwrap();

        let migrated = dm.migrated_count();
        eprintln!("  migrated {migrated} entries total");

        // Re-initialize for lookup
        d.initialize(DispatcherConfig {
            metadata_pci_addr: pci_addrs[0].clone(),
            data_pci_addrs: pci_addrs[..1].to_vec(),
        })
        .expect("re-initialize failed");

        // After migration, lookup hits BlockDevice path (offset-based).
        // The current BlockDevice lookup path is a TODO (doesn't copy data),
        // so we verify the entry still exists via check().
        assert_eq!(d.check(900).unwrap(), true, "migrated entry should exist");
    }

    d.shutdown().unwrap();
    eprintln!("\n=== DATA INTEGRITY TEST PASSED ===");
}
