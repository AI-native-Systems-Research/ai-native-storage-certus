//! Hardware benchmark for warm (memory-tier) and cold (SSD) dispatcher lookups.
//!
//! Exercises the full `IDispatcher::populate` + `IDispatcher::lookup` path with
//! real NVMe SSD I/O, real CUDA GPU DMA, and a real memory-tier pool.
//!
//! Requires: NVMe SSD bound to SPDK (VFIO), CUDA GPU, hugepages configured.
//! Run with:
//!   cargo bench -p dispatcher-v1 --features hardware-test --bench dispatcher_hw_benchmark

#![cfg(feature = "hardware-test")]
#![allow(clippy::arc_with_non_send_sync)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use component_core::iunknown::query;
use component_core::query_interface;
use dispatcher::DispatcherComponent;
use gpu_services::cuda_ffi;
use gpu_services::GpuServicesComponent;
use interfaces::{
    CacheKey, DispatchMapError, DispatcherConfig, DmaAllocFn, DmaBuffer, IDispatchMap,
    IDispatcher, IGpuServices, ILogger, IMemoryTier, IpcHandle, LookupResult,
};
use memory_tier::MemoryTierComponent;
use spdk_env::{ISPDKEnv, SPDKEnvComponent};

// ===========================================================================
// HwDispatchMap — tracks memory-tier pointers for correct LookupResult variant
// ===========================================================================

struct HwDmEntry {
    buffer: Arc<DmaBuffer>,
    block_offset: Option<u64>,
    mem_pointer: Option<(usize, u32)>,
    write_ref: bool,
    read_refs: u32,
}

struct HwDispatchMap {
    inner: Mutex<HashMap<CacheKey, HwDmEntry>>,
    dma_alloc: Mutex<Option<DmaAllocFn>>,
}

impl HwDispatchMap {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            dma_alloc: Mutex::new(None),
        }
    }

    fn has_block_offset(&self, key: CacheKey) -> bool {
        let inner = self.inner.lock().unwrap();
        inner
            .get(&key)
            .map(|e| e.block_offset.is_some())
            .unwrap_or(false)
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
                mem_pointer: None,
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
            Some(e) => {
                if let Some((addr, size)) = e.mem_pointer {
                    Ok(LookupResult::MemoryTier {
                        pointer: addr as *mut u8,
                        size,
                    })
                } else if let Some(offset) = e.block_offset {
                    Ok(LookupResult::BlockDevice { offset })
                } else {
                    Ok(LookupResult::Staging {
                        buffer: Arc::clone(&e.buffer),
                    })
                }
            }
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

    fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        if inner.contains_key(&key) {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        if inner.contains_key(&key) {
            Ok(4096)
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let inner = self.inner.lock().unwrap();
        inner.keys().copied().take(n).collect()
    }

    fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        // Memory-tier entries don't need a DMA buffer — the data lives in the pool.
        // Use a minimal placeholder buffer.
        let buf = DmaBuffer::new(4096, 4096, None)
            .map_err(|e| DispatchMapError::AllocationFailed(e.to_string()))?;
        inner.insert(
            key,
            HwDmEntry {
                buffer: Arc::new(buf),
                block_offset: None,
                mem_pointer: Some((pointer as usize, size)),
                write_ref: true,
                read_refs: 0,
            },
        );
        Ok(())
    }

    fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.get_mut(&key) {
            Some(e) => {
                if let Some(offset) = e.block_offset {
                    e.mem_pointer = None;
                    e.block_offset = Some(offset);
                    Ok(())
                } else {
                    Err(DispatchMapError::KeyNotFound(key))
                }
            }
            None => Err(DispatchMapError::KeyNotFound(key)),
        }
    }

    fn is_evictable(&self, _key: CacheKey) -> bool {
        false
    }

    fn recover_extent(
        &self,
        _key: CacheKey,
        _offset: u64,
        _size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }
}

// ===========================================================================
// Logger
// ===========================================================================

struct BenchLogger;
impl ILogger for BenchLogger {
    fn error(&self, msg: &str) {
        eprintln!("[ERROR] {msg}");
    }
    fn warn(&self, msg: &str) {
        eprintln!("[WARN] {msg}");
    }
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

// ===========================================================================
// Benchmark infrastructure
// ===========================================================================

const WARMUP_ITERS: usize = 5;
const MEASURED_ITERS: usize = 50;
const MEMORY_TIER_POOL_SIZE: usize = 512 * 1024 * 1024; // 512 MiB

struct BenchResult {
    label: String,
    total_bytes: usize,
    times_us: Vec<f64>,
}

impl BenchResult {
    fn report(&self) {
        let n = self.times_us.len() as f64;
        let mean = self.times_us.iter().sum::<f64>() / n;
        let mut sorted = self.times_us.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min = sorted[0];
        let p50 = sorted[(sorted.len() as f64 * 0.5) as usize];
        let p99 = sorted[((sorted.len() as f64 * 0.99) as usize).min(sorted.len() - 1)];
        let max = sorted[sorted.len() - 1];
        let throughput_mbs = (self.total_bytes as f64 / (1024.0 * 1024.0)) / (mean / 1_000_000.0);

        println!(
            "  {:>24} | {:>8} KiB | mean {:>9.1} us | min {:>9.1} us | p50 {:>9.1} us | p99 {:>9.1} us | max {:>9.1} us | {:>8.1} MB/s",
            self.label,
            self.total_bytes / 1024,
            mean, min, p50, p99, max, throughput_mbs
        );
    }
}

// ===========================================================================
// GPU memory helpers
// ===========================================================================

unsafe fn gpu_alloc(size: usize) -> Result<*mut std::ffi::c_void, String> {
    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let err = unsafe { cuda_ffi::cudaMalloc(&mut ptr, size) };
    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaMalloc({} bytes): {}",
            size,
            cuda_ffi::cuda_error_string(err)
        ));
    }
    // Fill with pattern via host→device memcpy.
    let pattern = vec![0xA5u8; size];
    let err = unsafe {
        cuda_ffi::cudaMemcpy(
            ptr,
            pattern.as_ptr() as *const std::ffi::c_void,
            size,
            cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
        )
    };
    if err != cuda_ffi::CUDA_SUCCESS {
        unsafe { cuda_ffi::cudaFree(ptr) };
        return Err(format!(
            "cudaMemcpy (fill pattern): {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }
    Ok(ptr)
}

unsafe fn gpu_free(ptr: *mut std::ffi::c_void) {
    unsafe { cuda_ffi::cudaFree(ptr) };
}

// ===========================================================================
// Setup
// ===========================================================================

fn setup_dispatcher(
    pci_addrs: &[String],
) -> Result<
    (
        Arc<dyn IDispatcher + Send + Sync>,
        Arc<HwDispatchMap>,
        Arc<dyn IMemoryTier + Send + Sync>,
        Arc<DispatcherComponent>,
        Arc<dyn IGpuServices + Send + Sync>,
    ),
    String,
> {
    // SPDK env
    let spdk_env_comp = SPDKEnvComponent::new_default();
    let ienv = query::<dyn ISPDKEnv + Send + Sync>(&*spdk_env_comp)
        .ok_or("ISPDKEnv query failed")?;
    ienv.init()
        .map_err(|e| format!("SPDK init: {e}"))?;

    let devices = ienv.devices();
    if devices.is_empty() {
        return Err("no NVMe devices found — ensure devices are bound to VFIO".into());
    }
    let actual_addrs: Vec<String> = if pci_addrs.is_empty() {
        devices.iter().map(|d| d.address.to_string()).collect()
    } else {
        pci_addrs.to_vec()
    };
    eprintln!("  NVMe devices: {:?}", actual_addrs);

    // GPU services (real CUDA)
    let gpu_comp = GpuServicesComponent::new_default();
    let igpu = query_interface!(gpu_comp, IGpuServices)
        .ok_or("IGpuServices query failed")?;
    igpu.initialize()
        .map_err(|e| format!("GPU services init: {e}"))?;
    let gpu_devices = igpu.get_devices().unwrap_or_default();
    eprintln!("  GPU: {} device(s)", gpu_devices.len());
    if let Some(d) = gpu_devices.first() {
        eprintln!("    [0] {} ({} MiB)", d.name, d.memory_bytes / (1024 * 1024));
    }

    // Memory tier (real mmap pool)
    let mt_comp = MemoryTierComponent::new_default();
    let imt = query_interface!(mt_comp, IMemoryTier)
        .ok_or("IMemoryTier query failed")?;
    imt.initialize(MEMORY_TIER_POOL_SIZE)
        .map_err(|e| format!("memory-tier init: {e:?}"))?;
    eprintln!(
        "  Memory-tier: {} MiB pool",
        MEMORY_TIER_POOL_SIZE / (1024 * 1024)
    );

    // Dispatcher component
    let dm = Arc::new(HwDispatchMap::new());
    let dispatcher = DispatcherComponent::new_default();

    dispatcher
        .dispatch_map
        .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
        .map_err(|e| format!("connect dispatch_map: {e}"))?;
    dispatcher
        .logger
        .connect(Arc::new(BenchLogger) as Arc<dyn ILogger + Send + Sync>)
        .map_err(|e| format!("connect logger: {e}"))?;
    dispatcher
        .gpu_services
        .connect(Arc::clone(&igpu) as Arc<dyn IGpuServices + Send + Sync>)
        .map_err(|e| format!("connect gpu_services: {e}"))?;
    dispatcher
        .spdk_env
        .connect(Arc::clone(&ienv) as Arc<dyn ISPDKEnv + Send + Sync>)
        .map_err(|e| format!("connect spdk_env: {e}"))?;
    dispatcher
        .memory_tier
        .connect(Arc::clone(&imt) as Arc<dyn IMemoryTier + Send + Sync>)
        .map_err(|e| format!("connect memory_tier: {e}"))?;

    let d: Arc<dyn IDispatcher + Send + Sync> =
        query_interface!(dispatcher, IDispatcher).ok_or("IDispatcher query failed")?;

    // Use first device for metadata only; remaining device(s) for data.
    // This avoids using a small metadata NVMe as a data drive (its slab_size
    // may be too small for large extents).
    let (metadata_addr, data_addrs) = if actual_addrs.len() > 1 {
        (actual_addrs[0].clone(), actual_addrs[1..].to_vec())
    } else {
        (actual_addrs[0].clone(), actual_addrs.clone())
    };
    let config = DispatcherConfig {
        metadata_pci_addr: metadata_addr,
        data_pci_addrs: data_addrs,
        max_cache_entries: 0,
        ..Default::default()
    };
    d.initialize(config)
        .map_err(|e| format!("dispatcher init: {e:?}"))?;

    Ok((d, dm, imt, dispatcher, igpu))
}

// ===========================================================================
// Warm lookup benchmark
// ===========================================================================

fn bench_warm_lookup(
    d: &Arc<dyn IDispatcher + Send + Sync>,
    dm: &Arc<HwDispatchMap>,
    gpu: &Arc<dyn IGpuServices + Send + Sync>,
    size: usize,
) -> Result<BenchResult, String> {
    let label = format!("warm_{}KiB", size / 1024);

    // Allocate GPU source buffer (for populate) and destination buffer (for lookup).
    let gpu_src = unsafe { gpu_alloc(size)? };
    let gpu_dst = unsafe { gpu_alloc(size)? };

    let key: CacheKey = 0xBEEF_0000 + (size as u64);

    // Populate: GPU → memory-tier (+ background SSD write).
    let src_handle = IpcHandle {
        address: gpu_src as *mut u8,
        size: size as u32,
    };
    d.populate(key, src_handle)
        .map_err(|e| format!("populate: {e:?}"))?;

    // Wait for background write to complete (so entry has block_offset and is stable).
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !dm.has_block_offset(key) {
        if Instant::now() > deadline {
            eprintln!("    warning: background write did not complete in 5s, proceeding anyway");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Warm lookup: entry is in memory-tier (MemoryTier variant).
    let dst_ptr = gpu_dst as *mut u8;
    let dst_size = size as u32;

    // Warmup
    for _ in 0..WARMUP_ITERS {
        let h = IpcHandle { address: dst_ptr, size: dst_size };
        d.lookup_async(key, h).map_err(|e| format!("warmup lookup: {e:?}"))?;
    }

    // Measured iterations: use lookup_async + targeted stream_synchronize.
    let mut times_us = Vec::with_capacity(MEASURED_ITERS);
    for _ in 0..MEASURED_ITERS {
        let h = IpcHandle { address: dst_ptr, size: dst_size };
        let start = Instant::now();
        let stream = d.lookup_async(key, h).map_err(|e| format!("lookup: {e:?}"))?;
        if !stream.0.is_null() {
            gpu.stream_synchronize(stream).map_err(|e| format!("sync: {e}"))?;
        }
        times_us.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }

    // Cleanup: remove entry, free GPU memory.
    let _ = d.remove(key);
    unsafe {
        gpu_free(gpu_src);
        gpu_free(gpu_dst);
    }

    Ok(BenchResult {
        label,
        total_bytes: size,
        times_us,
    })
}

// ===========================================================================
// Cold lookup benchmark (SSD read path)
// ===========================================================================

fn bench_cold_lookup(
    d: &Arc<dyn IDispatcher + Send + Sync>,
    dm: &Arc<HwDispatchMap>,
    mt: &Arc<dyn IMemoryTier + Send + Sync>,
    size: usize,
) -> Result<BenchResult, String> {
    let label = format!("cold_{}KiB", size / 1024);

    let gpu_src = unsafe { gpu_alloc(size)? };
    let gpu_dst = unsafe { gpu_alloc(size)? };

    let key: CacheKey = 0xDEAD_0000 + (size as u64);

    // Populate: GPU → memory-tier + background SSD write.
    let src_handle = IpcHandle {
        address: gpu_src as *mut u8,
        size: size as u32,
    };
    d.populate(key, src_handle)
        .map_err(|e| format!("populate key={key}: {e:?}"))?;

    // Wait for background write to complete (5s is ample for NVMe write-through).
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !dm.has_block_offset(key) {
        if Instant::now() > deadline {
            let _ = d.remove(key);
            unsafe { gpu_free(gpu_src); gpu_free(gpu_dst); }
            return Err("background write did not complete (extent too large for slab?)".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Evict from memory-tier → BlockDevice-only state.
    // Must remove from real memory-tier too, so promote_and_serve can re-insert.
    let _ = mt.remove(key);
    dm.convert_memory_tier_to_block(key)
        .map_err(|e| format!("convert_memory_tier_to_block: {e:?}"))?;

    let dst_ptr = gpu_dst as *mut u8;
    let dst_size = size as u32;

    // Warmup (promote_and_serve re-inserts into memory-tier, so re-evict each time).
    for _ in 0..WARMUP_ITERS {
        let _ = mt.remove(key);
        let _ = dm.convert_memory_tier_to_block(key);
        let h = IpcHandle { address: dst_ptr, size: dst_size };
        d.lookup(key, h).map_err(|e| format!("warmup cold lookup: {e:?}"))?;
        unsafe { cuda_ffi::cudaDeviceSynchronize() };
    }

    // Measured iterations: each re-evicts to force SSD read.
    let mut times_us = Vec::with_capacity(MEASURED_ITERS);
    for iter in 0..MEASURED_ITERS {
        let _ = mt.remove(key);
        let _ = dm.convert_memory_tier_to_block(key);

        let h = IpcHandle { address: dst_ptr, size: dst_size };
        let start = Instant::now();
        d.lookup(key, h).map_err(|e| format!("cold lookup iter {iter}: {e:?}"))?;
        unsafe { cuda_ffi::cudaDeviceSynchronize() };
        times_us.push(start.elapsed().as_secs_f64() * 1_000_000.0);
    }

    // Cleanup
    let _ = d.remove(key);
    unsafe {
        gpu_free(gpu_src);
        gpu_free(gpu_dst);
    }

    Ok(BenchResult {
        label,
        total_bytes: size,
        times_us,
    })
}

// ===========================================================================
// Main
// ===========================================================================

fn main() {
    extern "C" {
        fn atexit(cb: extern "C" fn()) -> i32;
        fn _exit(status: i32) -> !;
    }
    extern "C" fn exit_hook() {
        unsafe { _exit(0) };
    }
    unsafe { atexit(exit_hook) };

    // CUDA init
    let mut device_count: std::os::raw::c_int = 0;
    let err = unsafe { cuda_ffi::cudaGetDeviceCount(&mut device_count) };
    if err != cuda_ffi::CUDA_SUCCESS || device_count == 0 {
        eprintln!("FATAL: no CUDA GPU available");
        std::process::exit(1);
    }
    let err = unsafe { cuda_ffi::cudaSetDevice(0) };
    if err != cuda_ffi::CUDA_SUCCESS {
        eprintln!(
            "FATAL: cudaSetDevice(0): {}",
            cuda_ffi::cuda_error_string(err)
        );
        std::process::exit(1);
    }

    eprintln!("Initializing hardware stack...");
    let (d, dm, mt, _comp, igpu) = match setup_dispatcher(&[]) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };

    let sizes: &[usize] = &[
        128 * 1024,       // 128 KiB (single MDTS chunk)
        512 * 1024,       // 512 KiB
        1024 * 1024,      // 1 MiB
        4 * 1024 * 1024,  // 4 MiB
        16 * 1024 * 1024, // 16 MiB
    ];

    println!();
    println!("=== Dispatcher HW Benchmark: Warm & Cold Lookup ===");
    println!("  warmup:   {} iterations", WARMUP_ITERS);
    println!("  measured: {} iterations", MEASURED_ITERS);
    println!("  pool:     {} MiB", MEMORY_TIER_POOL_SIZE / (1024 * 1024));
    println!();

    let mut results = Vec::new();

    // Run each size: cold first (requires SSD write-through), then warm.
    // Running both for the same key in sequence avoids extent manager pressure.
    for &size in sizes {
        // --- Cold lookup ---
        eprint!("  benchmarking {} KiB (cold) ... ", size / 1024);
        match bench_cold_lookup(&d, &dm, &mt, size) {
            Ok(r) => {
                eprintln!("done");
                results.push(r);
            }
            Err(e) => {
                eprintln!("FAILED: {e}");
            }
        }

        // --- Warm lookup ---
        eprint!("  benchmarking {} KiB (warm) ... ", size / 1024);
        match bench_warm_lookup(&d, &dm, &igpu, size) {
            Ok(r) => {
                eprintln!("done");
                results.push(r);
            }
            Err(e) => {
                eprintln!("FAILED: {e}");
            }
        }
    }

    // --- Summary ---
    println!();
    println!("--- Results Summary ---");
    for r in &results {
        r.report();
    }

    println!();
}
