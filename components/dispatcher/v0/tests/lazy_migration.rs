//! Integration tests for lazy migration of staging buffers to SSD/NVMe.
//!
//! Verifies that after `populate()`, the background writer migrates entries
//! from staging (DMA buffer) to block-device state, and that subsequent
//! lookups and checks still succeed on migrated entries.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use component_core::query_interface;
use dispatcher::DispatcherComponentV0;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaAllocFn, DmaBuffer, GpuDeviceInfo,
    GpuDmaBuffer, GpuIpcHandle, IDispatchMap, IDispatcher, IGpuServices, ILogger, IpcHandle,
    LookupResult,
};

// ---------------------------------------------------------------------------
// Mock infrastructure
// ---------------------------------------------------------------------------

unsafe extern "C" fn dma_free(ptr: *mut std::ffi::c_void) {
    unsafe { libc::free(ptr) };
}

fn alloc_dma_buffer(size: usize) -> Arc<DmaBuffer> {
    let sz = size.max(4096);
    let aligned_sz = sz.next_multiple_of(4096);
    let ptr = unsafe { libc::aligned_alloc(4096, aligned_sz) };
    assert!(!ptr.is_null(), "aligned_alloc failed for {aligned_sz} bytes");
    unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, aligned_sz) };
    let buf = unsafe { DmaBuffer::from_raw(ptr, aligned_sz, dma_free, -1) }.unwrap();
    Arc::new(buf)
}

struct MockEntry {
    buffer: Arc<DmaBuffer>,
    block_offset: Option<u64>,
    write_ref: bool,
    read_refs: u32,
}

struct MockDmInner {
    entries: HashMap<CacheKey, MockEntry>,
}

struct MockDispatchMap {
    inner: Mutex<MockDmInner>,
}

impl MockDispatchMap {
    fn new() -> Self {
        Self {
            inner: Mutex::new(MockDmInner {
                entries: HashMap::new(),
            }),
        }
    }

    fn migrated_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .entries
            .values()
            .filter(|e| e.block_offset.is_some())
            .count()
    }

    fn entry_count(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }
}

impl IDispatchMap for MockDispatchMap {
    fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn create_staging(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        let buffer = alloc_dma_buffer(size as usize * 4096);
        inner.entries.insert(
            key,
            MockEntry {
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
        match inner.entries.get(&key) {
            None => Ok(LookupResult::NotExist),
            Some(entry) => match entry.block_offset {
                Some(offset) => Ok(LookupResult::BlockDevice { offset }),
                None => Ok(LookupResult::Staging {
                    buffer: Arc::clone(&entry.buffer),
                }),
            },
        }
    }

    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.block_offset = Some(offset);
                Ok(())
            }
        }
    }

    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.read_refs += 1;
                Ok(())
            }
        }
    }

    fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.write_ref = true;
                Ok(())
            }
        }
    }

    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.read_refs = entry.read_refs.saturating_sub(1);
                Ok(())
            }
        }
    }

    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) => {
                entry.write_ref = false;
                Ok(())
            }
        }
    }

    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::NoWriteReference(key)),
            Some(entry) => {
                entry.write_ref = false;
                entry.read_refs += 1;
                Ok(())
            }
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.remove(&key).is_some() {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }
}

struct MockLogger;

impl ILogger for MockLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
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
    fn deserialize_ipc_handle(&self, _base64_payload: &str) -> Result<GpuIpcHandle, String> {
        Err("mock: not implemented".into())
    }
    fn verify_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn pin_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn unpin_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
        Ok(())
    }
    fn create_dma_buffer(&self, _handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
        Err("mock: not implemented".into())
    }
    fn dma_copy_to_host(
        &self,
        src: *const std::ffi::c_void,
        dst: &DmaBuffer,
        size: usize,
    ) -> Result<(), String> {
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
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn setup() -> (Arc<DispatcherComponentV0>, Arc<MockDispatchMap>) {
    let dm = Arc::new(MockDispatchMap::new());
    let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
    let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
    let c = DispatcherComponentV0::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger.connect(logger).unwrap();
    c.gpu_services.connect(gpu).unwrap();

    let d = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        metadata_pci_addr: "0000:01:00.0".to_string(),
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
    })
    .unwrap();

    (c, dm)
}

fn make_handle(buf: &mut [u8]) -> IpcHandle {
    IpcHandle {
        address: buf.as_mut_ptr(),
        size: buf.len() as u32,
    }
}

// ---------------------------------------------------------------------------
// Lazy migration tests
// ---------------------------------------------------------------------------

#[test]
fn staging_entry_migrates_to_block_device_on_drain() {
    let (c, dm) = setup();
    let d = query_interface!(c, IDispatcher).unwrap();

    let mut buf = vec![0u8; 4096];
    d.populate(1, make_handle(&mut buf)).unwrap();

    assert_eq!(dm.migrated_count(), 0, "should still be in staging before drain");

    d.shutdown().unwrap();

    assert_eq!(dm.migrated_count(), 1, "entry should be migrated after bg writer drains");
}

#[test]
fn multiple_entries_all_migrate() {
    let (c, dm) = setup();
    let d = query_interface!(c, IDispatcher).unwrap();

    for key in 0..10u64 {
        let mut buf = vec![0u8; 8192];
        d.populate(key, make_handle(&mut buf)).unwrap();
    }

    assert_eq!(dm.entry_count(), 10);

    d.shutdown().unwrap();

    assert_eq!(dm.migrated_count(), 10, "all entries should migrate");
}

#[test]
fn lookup_succeeds_after_migration() {
    let (c, _dm) = setup();
    let d = query_interface!(c, IDispatcher).unwrap();

    let mut buf = vec![0u8; 4096];
    d.populate(42, make_handle(&mut buf)).unwrap();

    // Drain bg writer
    d.shutdown().unwrap();

    // Re-initialize to allow lookups
    d.initialize(DispatcherConfig {
        metadata_pci_addr: "0000:01:00.0".to_string(),
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
    })
    .unwrap();

    let mut buf2 = vec![0u8; 4096];
    assert!(
        d.lookup(42, make_handle(&mut buf2)).is_ok(),
        "lookup should succeed on migrated entry"
    );

    d.shutdown().unwrap();
}

#[test]
fn check_finds_migrated_entry() {
    let (c, _dm) = setup();
    let d = query_interface!(c, IDispatcher).unwrap();

    let mut buf = vec![0u8; 4096];
    d.populate(7, make_handle(&mut buf)).unwrap();

    d.shutdown().unwrap();

    d.initialize(DispatcherConfig {
        metadata_pci_addr: "0000:01:00.0".to_string(),
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
    })
    .unwrap();

    assert_eq!(d.check(7).unwrap(), true, "migrated entry should be discoverable");
    d.shutdown().unwrap();
}

#[test]
fn concurrent_populates_all_migrate() {
    let (c, dm) = setup();

    let handles: Vec<_> = (0..4)
        .map(|t| {
            let comp = Arc::clone(&c);
            thread::spawn(move || {
                let d = query_interface!(comp, IDispatcher).unwrap();
                for i in 0..5 {
                    let key: u64 = t * 1000 + i;
                    let mut buf = vec![0u8; 4096];
                    d.populate(key, make_handle(&mut buf)).unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }

    let d = query_interface!(c, IDispatcher).unwrap();
    d.shutdown().unwrap();

    assert_eq!(dm.entry_count(), 20);
    assert_eq!(
        dm.migrated_count(),
        20,
        "all concurrently populated entries should migrate"
    );
}
