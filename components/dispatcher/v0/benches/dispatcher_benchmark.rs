use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use component_core::query_interface;
use dispatcher::io_segmenter::segment_io;
use dispatcher::DispatcherComponentV0;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaAllocFn, DmaBuffer, GpuDeviceInfo,
    GpuDmaBuffer, GpuIpcHandle, IDispatchMap, IDispatcher, IGpuServices, ILogger, IpcHandle,
    LookupResult,
};

// ===========================================================================
// Mock infrastructure (staging-only, no hardware)
// ===========================================================================

unsafe extern "C" fn dma_free(ptr: *mut std::ffi::c_void) {
    unsafe { libc::free(ptr) };
}

fn alloc_dma_buffer(size: usize) -> Arc<DmaBuffer> {
    let sz = size.max(4096);
    let aligned_sz = sz.next_multiple_of(4096);
    let ptr = unsafe { libc::aligned_alloc(4096, aligned_sz) };
    assert!(!ptr.is_null());
    unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, aligned_sz) };
    let buf = unsafe { DmaBuffer::from_raw(ptr, aligned_sz, dma_free, -1) }.unwrap();
    Arc::new(buf)
}

struct BenchEntry {
    buffer: Arc<DmaBuffer>,
    block_offset: Option<u64>,
    write_ref: bool,
    read_refs: u32,
}

struct BenchDispatchMap {
    inner: Mutex<HashMap<CacheKey, BenchEntry>>,
}

impl BenchDispatchMap {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }
}

impl IDispatchMap for BenchDispatchMap {
    fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn create_staging(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        let buffer = alloc_dma_buffer(size as usize * 4096);
        inner.insert(
            key,
            BenchEntry {
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

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let inner = self.inner.lock().unwrap();
        inner.keys().copied().take(n).collect()
    }
}

struct BenchLogger;
impl ILogger for BenchLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

struct BenchGpuServices;
impl IGpuServices for BenchGpuServices {
    fn initialize(&self) -> Result<(), String> { Ok(()) }
    fn shutdown(&self) -> Result<(), String> { Ok(()) }
    fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String> { Ok(vec![]) }
    fn deserialize_ipc_handle(&self, _: &str) -> Result<GpuIpcHandle, String> {
        Err("bench mock".into())
    }
    fn verify_memory(&self, _: &GpuIpcHandle) -> Result<(), String> { Ok(()) }
    fn pin_memory(&self, _: &GpuIpcHandle) -> Result<(), String> { Ok(()) }
    fn unpin_memory(&self, _: &GpuIpcHandle) -> Result<(), String> { Ok(()) }
    fn create_dma_buffer(&self, _: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
        Err("bench mock".into())
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

// ===========================================================================
// Helpers
// ===========================================================================

fn setup_dispatcher() -> (Arc<dyn IDispatcher + Send + Sync>, Arc<BenchDispatchMap>) {
    let dm = Arc::new(BenchDispatchMap::new());
    let c = DispatcherComponentV0::new_default();
    c.dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .unwrap();
    c.logger
        .connect(Arc::new(BenchLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap();
    c.gpu_services
        .connect(Arc::new(BenchGpuServices) as Arc<dyn IGpuServices + Send + Sync>)
        .unwrap();

    let d: Arc<dyn IDispatcher + Send + Sync> = query_interface!(c, IDispatcher).unwrap();
    d.initialize(DispatcherConfig {
        metadata_pci_addr: "0000:01:00.0".to_string(),
        data_pci_addrs: vec!["0000:02:00.0".to_string()],
        max_cache_entries: 0, // disable eviction for benchmarks
        ..Default::default()
    })
    .unwrap();

    (d, dm)
}

fn make_handle(buf: &mut [u8]) -> IpcHandle {
    IpcHandle {
        address: buf.as_mut_ptr(),
        size: buf.len() as u32,
    }
}

// ===========================================================================
// io_segmenter benchmarks
// ===========================================================================

fn bench_segment_io_small(c: &mut Criterion) {
    c.bench_function("segment_io_4k", |b| {
        b.iter(|| segment_io(black_box(0), black_box(4096), 131072, 4096));
    });
}

fn bench_segment_io_1m(c: &mut Criterion) {
    c.bench_function("segment_io_1m", |b| {
        b.iter(|| segment_io(black_box(0), black_box(1024 * 1024), 131072, 4096));
    });
}

// ===========================================================================
// populate benchmarks (GPU -> staging)
// ===========================================================================

fn bench_populate(c: &mut Criterion) {
    let mut group = c.benchmark_group("populate");

    let sizes: &[(u64, usize)] = &[
        (4096, 4096),
        (16384, 16 * 1024),
        (65536, 64 * 1024),
        (262144, 256 * 1024),
        (1048576, 1024 * 1024),
    ];

    for &(_id, size) in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(format!("{size}")), &size, |b, &sz| {
            let (d, _dm) = setup_dispatcher();
            let mut key_counter: u64 = 0;
            let mut buf = vec![0xA5u8; sz];

            b.iter(|| {
                let key = key_counter;
                key_counter += 1;
                d.populate(black_box(key), make_handle(&mut buf)).unwrap();
            });
        });
    }
    group.finish();
}

// ===========================================================================
// lookup benchmarks (staging -> client)
// ===========================================================================

fn bench_lookup_staging(c: &mut Criterion) {
    let mut group = c.benchmark_group("lookup_staging");

    let sizes: &[(u64, usize)] = &[
        (4096, 4096),
        (16384, 16 * 1024),
        (65536, 64 * 1024),
        (262144, 256 * 1024),
        (1048576, 1024 * 1024),
    ];

    for &(_id, size) in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(format!("{size}")), &size, |b, &sz| {
            let (d, _dm) = setup_dispatcher();

            // Pre-populate entries for lookup
            let num_entries = 100u64;
            let mut src = vec![0xBBu8; sz];
            for key in 0..num_entries {
                d.populate(key, make_handle(&mut src)).unwrap();
            }

            let mut key_idx: u64 = 0;
            let mut dst = vec![0u8; sz];

            b.iter(|| {
                let key = key_idx % num_entries;
                key_idx += 1;
                d.lookup(black_box(key), make_handle(&mut dst)).unwrap();
            });
        });
    }
    group.finish();
}

// ===========================================================================
// check benchmarks (dispatch map lookup, no data copy)
// ===========================================================================

fn bench_check(c: &mut Criterion) {
    let (d, _dm) = setup_dispatcher();

    let num_entries = 1000u64;
    let mut buf = vec![0u8; 4096];
    for key in 0..num_entries {
        d.populate(key, make_handle(&mut buf)).unwrap();
    }

    let mut group = c.benchmark_group("check");

    group.bench_function("existing_key", |b| {
        let mut key_idx: u64 = 0;
        b.iter(|| {
            let key = key_idx % num_entries;
            key_idx += 1;
            black_box(d.check(black_box(key)).unwrap());
        });
    });

    group.bench_function("nonexistent_key", |b| {
        let mut key_idx: u64 = num_entries;
        b.iter(|| {
            key_idx += 1;
            black_box(d.check(black_box(key_idx)).unwrap());
        });
    });

    group.finish();
}

// ===========================================================================
// prepare_store + cancel_store benchmarks (allocation hot path)
// ===========================================================================

fn bench_prepare_cancel(c: &mut Criterion) {
    let mut group = c.benchmark_group("prepare_cancel_store");

    let sizes: &[usize] = &[4096, 64 * 1024, 256 * 1024];

    for &size in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(format!("{size}")), &size, |b, &sz| {
            let (d, _dm) = setup_dispatcher();
            let mut key_counter: u64 = 0;

            b.iter(|| {
                let key = key_counter;
                key_counter += 1;
                let _buf = d.prepare_store(black_box(key), sz as u32).unwrap();
                d.cancel_store(black_box(key)).unwrap();
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_segment_io_small,
    bench_segment_io_1m,
    bench_populate,
    bench_lookup_staging,
    bench_check,
    bench_prepare_cancel,
);
criterion_main!(benches);
