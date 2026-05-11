//! EngineInner — wires the Certus component stack and implements the
//! operations exposed by the CertusEngine PyO3 class.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use component_core::query_interface;
use interfaces::{
    CacheKey, DispatcherConfig, DmaAllocFn, DmaBuffer, FormatParams, IBlockDevice,
    IBlockDeviceAdmin, IDispatchMap, IDispatcher, IExtentManager, IGpuServices, ILogger, IpcHandle,
    LookupResult, PciAddress,
};

use crate::keys;

// ─── Transfer job tracking ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum JobKind {
    Store,
    Load,
}

#[allow(dead_code)]
struct TransferJob {
    kind: JobKind,
    keys: Vec<CacheKey>,
    gpu_block_ids: Vec<u64>,
    completed: AtomicBool,
    success: AtomicBool,
}

fn parse_pci_addr(s: &str) -> Result<PciAddress, String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return Err(format!("expected domain:bus:dev.func, got '{s}'"));
    }
    let domain = u32::from_str_radix(parts[0], 16).map_err(|_| format!("invalid domain '{}'", parts[0]))?;
    let bus = u8::from_str_radix(parts[1], 16).map_err(|_| format!("invalid bus '{}'", parts[1]))?;
    let dev_func: Vec<&str> = parts[2].split('.').collect();
    if dev_func.len() != 2 {
        return Err(format!("invalid dev.func '{}'", parts[2]));
    }
    let dev = u8::from_str_radix(dev_func[0], 16).map_err(|_| format!("invalid dev '{}'", dev_func[0]))?;
    let func = u8::from_str_radix(dev_func[1], 16).map_err(|_| format!("invalid func '{}'", dev_func[1]))?;
    Ok(PciAddress { domain, bus, dev, func })
}

// ─── EngineInner ───────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct EngineInner {
    dispatcher: Arc<dyn IDispatcher + Send + Sync>,
    dispatch_map: Arc<dyn IDispatchMap + Send + Sync>,
    gpu_services: Arc<dyn IGpuServices + Send + Sync>,
    gpu_block_size: u64,
    jobs: Mutex<HashMap<u64, Arc<TransferJob>>>,
    next_internal_id: AtomicU64,
    initialized: AtomicBool,
}

impl EngineInner {
    /// Construct from a Python config dict.
    ///
    /// Instantiates and wires all Certus components:
    /// - SPDKEnvComponent (environment init)
    /// - GpuServicesComponentV0 (CUDA init)
    /// - DispatchMapComponentV0 (key→location index)
    /// - DispatcherComponentV0 (orchestration)
    pub fn from_config(config: &Bound<'_, PyDict>) -> PyResult<Self> {
        let data_pci_addrs: Vec<String> = config
            .get_item("data_pci_addrs")?
            .ok_or_else(|| PyRuntimeError::new_err("missing 'data_pci_addrs'"))?
            .extract()?;

        let metadata_pci_addr: String = config
            .get_item("metadata_pci_addr")?
            .ok_or_else(|| PyRuntimeError::new_err("missing 'metadata_pci_addr'"))?
            .extract()?;

        let gpu_block_size: u64 = config
            .get_item("gpu_block_size")?
            .ok_or_else(|| PyRuntimeError::new_err("missing 'gpu_block_size'"))?
            .extract()?;

        let slab_size_bytes: u64 = config
            .get_item("slab_size_bytes")?
            .and_then(|v| v.extract().ok())
            .unwrap_or(131072);

        let dram_cache_bytes: u64 = config
            .get_item("dram_cache_bytes")?
            .and_then(|v| v.extract().ok())
            .unwrap_or(0);

        let eviction_threshold: f64 = config
            .get_item("eviction_threshold")?
            .and_then(|v| v.extract().ok())
            .unwrap_or(0.8);

        let max_cache_entries: usize = if slab_size_bytes > 0 && dram_cache_bytes > 0 {
            (dram_cache_bytes / slab_size_bytes) as usize
        } else {
            10000
        };

        // --- Initialize SPDK environment ---
        let spdk_comp = spdk_env::SPDKEnvComponent::new_default();
        let spdk_iface = query_interface!(spdk_comp, spdk_env::ISPDKEnv)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query ISPDKEnv"))?;
        spdk_iface
            .init()
            .map_err(|e| PyRuntimeError::new_err(format!("SPDK init failed: {e}")))?;

        // --- Create logger ---
        let log_comp = logger::LoggerComponentV1::new_default();
        let log: Arc<dyn ILogger + Send + Sync> = query_interface!(log_comp, ILogger)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query ILogger"))?;

        // --- Initialize GPU services ---
        let gpu_comp = gpu_services::GpuServicesComponentV0::new_default();
        let gpu: Arc<dyn IGpuServices + Send + Sync> = query_interface!(gpu_comp, IGpuServices)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query IGpuServices"))?;
        gpu.initialize()
            .map_err(|e| PyRuntimeError::new_err(format!("GPU init failed: {e}")))?;

        // --- Create metadata block device ---
        let meta_dev = block_device_spdk_nvme_v2::BlockDeviceSpdkNvmeComponentV2::new_default();
        meta_dev
            .logger
            .connect(Arc::clone(&log))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire logger for metadata device: {e}")))?;
        meta_dev
            .spdk_env
            .connect(Arc::clone(&spdk_iface))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire spdk_env for metadata device: {e}")))?;
        let meta_admin: Arc<dyn IBlockDeviceAdmin + Send + Sync> =
            query_interface!(meta_dev, IBlockDeviceAdmin)
                .ok_or_else(|| PyRuntimeError::new_err("failed to query IBlockDeviceAdmin for metadata device"))?;
        let pci = parse_pci_addr(&metadata_pci_addr)
            .map_err(|e| PyRuntimeError::new_err(format!("invalid metadata PCI address '{metadata_pci_addr}': {e}")))?;
        meta_admin.set_pci_address(pci);
        meta_admin
            .initialize()
            .map_err(|e| PyRuntimeError::new_err(format!("metadata block device init failed: {e}")))?;
        let meta_ibd: Arc<dyn IBlockDevice + Send + Sync> =
            query_interface!(meta_dev, IBlockDevice)
                .ok_or_else(|| PyRuntimeError::new_err("failed to query IBlockDevice for metadata device"))?;

        // --- Create extent manager for metadata device ---
        let meta_em = extent_manager_v2::ExtentManagerV2::new_inner();
        let numa_node = meta_ibd.numa_node();
        let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
            DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
        });
        meta_em.set_dma_alloc(dma_alloc);
        meta_em
            .logger
            .connect(Arc::clone(&log) as Arc<dyn ILogger + Send + Sync>)
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire logger for metadata extent manager: {e}")))?;
        use component_core::binding::bind;
        bind(
            &*meta_dev,
            "IBlockDevice",
            &*meta_em as &dyn component_core::IUnknown,
            "metadata_device",
        )
        .map_err(|e| PyRuntimeError::new_err(format!("failed to bind metadata block device to extent manager: {e}")))?;
        let meta_iem: Arc<dyn IExtentManager + Send + Sync> =
            query_interface!(meta_em, IExtentManager)
                .ok_or_else(|| PyRuntimeError::new_err("failed to query IExtentManager for metadata device"))?;
        let sector_size = meta_ibd.block_size();
        let num_sectors = meta_ibd.num_sectors(1).unwrap_or(0);
        let data_disk_size = num_sectors * sector_size as u64;
        let defaults = FormatParams::default();
        meta_iem
            .format(FormatParams {
                data_disk_size,
                sector_size,
                ..defaults
            })
            .map_err(|e| PyRuntimeError::new_err(format!("metadata extent manager format failed: {e}")))?;

        // --- Create dispatch map, wire extent manager, initialize ---
        let dm_comp =
            dispatch_map::DispatchMapComponentV0::new(dispatch_map::DispatchMapState::default());
        dm_comp
            .logger
            .connect(Arc::clone(&log) as Arc<dyn ILogger + Send + Sync>)
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire logger for dispatch map: {e}")))?;
        dm_comp
            .extent_manager
            .connect(Arc::clone(&meta_iem))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire extent_manager to dispatch map: {e}")))?;
        let dm: Arc<dyn IDispatchMap + Send + Sync> = query_interface!(dm_comp, IDispatchMap)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query IDispatchMap"))?;
        dm.initialize()
            .map_err(|e| PyRuntimeError::new_err(format!("DispatchMap init failed: {e}")))?;

        // --- Create dispatcher ---
        let disp_comp = dispatcher::DispatcherComponentV0::new_default();
        disp_comp
            .logger
            .connect(Arc::clone(&log) as Arc<dyn ILogger + Send + Sync>)
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire logger for dispatcher: {e}")))?;
        disp_comp
            .dispatch_map
            .connect(Arc::clone(&dm))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to bind dispatch_map: {e}")))?;
        disp_comp
            .gpu_services
            .connect(Arc::clone(&gpu))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to bind gpu_services: {e}")))?;
        disp_comp
            .spdk_env
            .connect(Arc::clone(&spdk_iface))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to bind spdk_env: {e}")))?;

        let dispatcher: Arc<dyn IDispatcher + Send + Sync> =
            query_interface!(disp_comp, IDispatcher)
                .ok_or_else(|| PyRuntimeError::new_err("failed to query IDispatcher"))?;

        dispatcher
            .initialize(DispatcherConfig {
                metadata_pci_addr,
                data_pci_addrs,
                block_device_version: interfaces::BlockDeviceVersion::V2,
                extent_manager_version: interfaces::ExtentManagerVersion::V2,
                max_cache_entries,
                eviction_threshold,
                format_on_init: true,
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Dispatcher init failed: {e}")))?;

        Ok(Self {
            dispatcher,
            dispatch_map: dm,
            gpu_services: gpu,
            gpu_block_size,
            jobs: Mutex::new(HashMap::new()),
            next_internal_id: AtomicU64::new(0),
            initialized: AtomicBool::new(true),
        })
    }

    fn ensure_init(&self) -> PyResult<()> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(PyRuntimeError::new_err("engine not initialized"));
        }
        Ok(())
    }

    // ─── Manager-level operations ──────────────────────────────────────

    /// Return count of consecutive keys (from the start) that are cached.
    pub fn batch_check(&self, keys: &[u64]) -> PyResult<u64> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);
        let mut count: u64 = 0;
        for key in &cache_keys {
            match self.dispatcher.check(*key) {
                Ok(true) => count += 1,
                Ok(false) => break,
                Err(_) => break,
            }
        }
        Ok(count)
    }

    /// Allocate space for new keys, evicting if necessary.
    /// Returns (keys_to_store, evicted_keys).
    ///
    /// Current implementation: all keys that don't already exist need storing.
    /// Eviction is handled internally by the extent manager when out of space.
    pub fn prepare_store(&self, keys: &[u64]) -> PyResult<(Vec<u64>, Vec<u64>)> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);
        let mut to_store = Vec::new();
        let mut evicted = Vec::new();

        for (i, key) in cache_keys.iter().enumerate() {
            match self.dispatcher.check(*key) {
                Ok(true) => {
                    // Already cached, skip
                }
                Ok(false) | Err(_) => {
                    to_store.push(keys[i]);
                }
            }
        }

        // TODO: When extent manager signals OutOfSpace during actual store,
        // implement LRU eviction by removing oldest entries from dispatch_map.
        // For now, evicted is always empty — the dispatcher handles allocation
        // failures at populate time.
        let _ = &mut evicted;

        Ok((to_store, evicted))
    }

    /// Finalize or abort a store operation.
    pub fn complete_store(&self, keys: &[u64], success: bool) -> PyResult<()> {
        self.ensure_init()?;
        if !success {
            let cache_keys = keys::to_cache_keys(keys);
            for key in &cache_keys {
                let _ = self.dispatcher.remove(*key);
            }
        }
        Ok(())
    }

    /// Update LRU ordering for the given keys.
    ///
    /// Currently a no-op — dispatch-map doesn't track access order yet.
    /// When LRU eviction is implemented, this will bump the keys.
    pub fn touch(&self, keys: &[u64]) -> PyResult<()> {
        self.ensure_init()?;
        let _cache_keys = keys::to_cache_keys(keys);
        // TODO: Update LRU ordering in dispatch-map
        Ok(())
    }

    /// Pin blocks for reading (protect from eviction) and return their
    /// storage offsets. Assumes all keys are already stored and ready.
    ///
    /// Uses `dispatch_map.lookup()` which atomically increments `read_ref`
    /// and returns the block location. Blocks with `read_ref > 0` cannot
    /// be evicted or removed.
    /// Caller MUST call `complete_load` when DMA is done.
    pub fn prepare_load(&self, keys: &[u64]) -> PyResult<Vec<u64>> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);
        let mut offsets = Vec::with_capacity(cache_keys.len());

        for (i, key) in cache_keys.iter().enumerate() {
            match self.dispatch_map.lookup(*key) {
                Ok(LookupResult::BlockDevice { offset }) => {
                    offsets.push(offset);
                }
                Ok(LookupResult::MemoryTier { .. }) => {
                    offsets.push(*key);
                }
                Ok(LookupResult::Staging { .. }) => {
                    offsets.push(*key);
                }
                Ok(LookupResult::NotExist) => {
                    // Rollback: release reads we already took
                    for prev_key in &cache_keys[..i] {
                        let _ = self.dispatch_map.release_read(*prev_key);
                    }
                    return Err(PyRuntimeError::new_err(format!(
                        "prepare_load: key {key} not found"
                    )));
                }
                Ok(LookupResult::MismatchSize) => {
                    for prev_key in &cache_keys[..i] {
                        let _ = self.dispatch_map.release_read(*prev_key);
                    }
                    return Err(PyRuntimeError::new_err(format!(
                        "prepare_load: key {key} size mismatch"
                    )));
                }
                Err(e) => {
                    // Rollback: release reads we already took
                    for prev_key in &cache_keys[..i] {
                        let _ = self.dispatch_map.release_read(*prev_key);
                    }
                    return Err(PyRuntimeError::new_err(format!(
                        "prepare_load: lookup failed for key {key}: {e:?}"
                    )));
                }
            }
        }

        Ok(offsets)
    }

    /// Unpin blocks after load DMA completes. Decrements `read_ref` so
    /// blocks become eligible for eviction again.
    pub fn complete_load(&self, keys: &[u64]) -> PyResult<()> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);

        for key in &cache_keys {
            self.dispatch_map.release_read(*key).map_err(|e| {
                PyRuntimeError::new_err(format!(
                    "complete_load: release_read failed for key {key}: {e:?}"
                ))
            })?;
        }

        Ok(())
    }

    // ─── Handler-level operations ──────────────────────────────────────

    /// Submit async GPU→DRAM→NVMe transfer (store).
    pub fn store_async(&self, job_id: u64, gpu_block_ids: &[u64], keys: &[u64]) -> PyResult<bool> {
        self.ensure_init()?;

        if gpu_block_ids.len() != keys.len() {
            return Err(PyRuntimeError::new_err(
                "gpu_block_ids and keys must have same length",
            ));
        }

        let cache_keys = keys::to_cache_keys(keys);

        let job = Arc::new(TransferJob {
            kind: JobKind::Store,
            keys: cache_keys.clone(),
            gpu_block_ids: gpu_block_ids.to_vec(),
            completed: AtomicBool::new(false),
            success: AtomicBool::new(false),
        });

        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(job_id, Arc::clone(&job));
        }

        // Execute store: for each block, create an IpcHandle pointing at the
        // GPU memory region and call dispatcher.populate().
        let mut all_ok = true;
        for (i, key) in cache_keys.iter().enumerate() {
            let block_id = gpu_block_ids[i];
            let offset = block_id * self.gpu_block_size;

            // IpcHandle points to GPU memory at the computed offset.
            // The dispatcher will DMA from this address into its staging buffer.
            let handle = IpcHandle {
                address: offset as *mut u8,
                size: self.gpu_block_size as u32,
            };

            if let Err(_e) = self.dispatcher.populate(*key, handle) {
                all_ok = false;
                break;
            }
        }

        job.completed.store(true, Ordering::Release);
        job.success.store(all_ok, Ordering::Release);

        Ok(all_ok)
    }

    /// Submit async NVMe/DRAM→GPU transfer (load).
    pub fn load_async(&self, job_id: u64, gpu_block_ids: &[u64], keys: &[u64]) -> PyResult<bool> {
        self.ensure_init()?;

        if gpu_block_ids.len() != keys.len() {
            return Err(PyRuntimeError::new_err(
                "gpu_block_ids and keys must have same length",
            ));
        }

        let cache_keys = keys::to_cache_keys(keys);

        let job = Arc::new(TransferJob {
            kind: JobKind::Load,
            keys: cache_keys.clone(),
            gpu_block_ids: gpu_block_ids.to_vec(),
            completed: AtomicBool::new(false),
            success: AtomicBool::new(false),
        });

        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(job_id, Arc::clone(&job));
        }

        // Execute load: for each block, create an IpcHandle pointing at the
        // destination GPU memory and call dispatcher.lookup().
        let mut all_ok = true;
        for (i, key) in cache_keys.iter().enumerate() {
            let block_id = gpu_block_ids[i];
            let offset = block_id * self.gpu_block_size;

            let handle = IpcHandle {
                address: offset as *mut u8,
                size: self.gpu_block_size as u32,
            };

            if let Err(_e) = self.dispatcher.lookup(*key, handle) {
                all_ok = false;
                break;
            }
        }

        job.completed.store(true, Ordering::Release);
        job.success.store(all_ok, Ordering::Release);

        Ok(all_ok)
    }

    /// Poll for completed transfers. Returns list of (job_id, success).
    pub fn poll_completions(&self) -> PyResult<Vec<(u64, bool)>> {
        self.ensure_init()?;
        let mut completions = Vec::new();
        let mut jobs = self.jobs.lock().unwrap();

        let completed_ids: Vec<u64> = jobs
            .iter()
            .filter(|(_, job)| job.completed.load(Ordering::Acquire))
            .map(|(id, _)| *id)
            .collect();

        for id in completed_ids {
            if let Some(job) = jobs.remove(&id) {
                completions.push((id, job.success.load(Ordering::Acquire)));
            }
        }

        Ok(completions)
    }

    /// Block until a specific job completes.
    pub fn wait_job(&self, job_id: u64) -> PyResult<()> {
        self.ensure_init()?;
        // Jobs complete synchronously in the current implementation,
        // so this is effectively a lookup + remove.
        let jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get(&job_id) {
            if !job.completed.load(Ordering::Acquire) {
                drop(jobs);
                // Spin-wait (will be replaced with condvar when async I/O lands)
                loop {
                    let jobs = self.jobs.lock().unwrap();
                    if let Some(job) = jobs.get(&job_id) {
                        if job.completed.load(Ordering::Acquire) {
                            break;
                        }
                    } else {
                        break;
                    }
                    drop(jobs);
                    std::thread::sleep(std::time::Duration::from_micros(100));
                }
            }
        }
        Ok(())
    }

    /// Store bytes from a host buffer directly (no GPU DMA). For testing only.
    /// Uses dispatcher.prepare_store()+commit_store() to write directly into
    /// the DMA buffer and flush to NVMe without going through CUDA.
    pub fn store_host_bytes(&self, key: u64, data: &[u8]) -> PyResult<()> {
        self.ensure_init()?;
        let dma_buf = self.dispatcher
            .prepare_store(key, data.len() as u32)
            .map_err(|e| PyRuntimeError::new_err(format!("store_host_bytes prepare failed: {e}")))?;

        // Copy data into the DMA buffer directly.
        // SAFETY: dma_buf is a valid DMA allocation covering at least data.len() bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), dma_buf.as_ptr() as *mut u8, data.len());
        }

        self.dispatcher
            .commit_store(key)
            .map_err(|e| PyRuntimeError::new_err(format!("store_host_bytes commit failed: {e}")))
    }

    /// Read bytes from the dispatch map's staging buffer for a key (no GPU DMA).
    /// For testing only — verifies data was written correctly before background
    /// NVMe migration moves it off the staging buffer.
    pub fn load_host_bytes(&self, key: u64, size: usize) -> PyResult<Vec<u8>> {
        self.ensure_init()?;
        // Read directly from the dispatch map staging buffer, bypassing GPU DMA.
        let result = self.dispatch_map
            .lookup(key)
            .map_err(|e| PyRuntimeError::new_err(format!("load_host_bytes lookup failed: {e}")))?;

        use interfaces::LookupResult;
        match result {
            LookupResult::Staging { buffer } => {
                let copy_len = size.min(buffer.len());
                let mut out = vec![0u8; size];
                // SAFETY: buffer is a valid DMA allocation; out is a valid heap allocation.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        buffer.as_ptr() as *const u8,
                        out.as_mut_ptr(),
                        copy_len,
                    );
                }
                let _ = self.dispatch_map.release_read(key);
                Ok(out)
            }
            LookupResult::MemoryTier { pointer, size: entry_size } => {
                let copy_len = size.min(entry_size as usize);
                let mut out = vec![0u8; size];
                // SAFETY: pointer is a valid memory-tier slot for entry_size bytes.
                unsafe {
                    std::ptr::copy_nonoverlapping(pointer, out.as_mut_ptr(), copy_len);
                }
                let _ = self.dispatch_map.release_read(key);
                Ok(out)
            }
            LookupResult::BlockDevice { .. } => {
                let _ = self.dispatch_map.release_read(key);
                Err(PyRuntimeError::new_err(
                    "key already migrated to NVMe — use load_async for block device reads",
                ))
            }
            LookupResult::NotExist => Err(PyRuntimeError::new_err(
                format!("key {key} not found in dispatch map"),
            )),
            LookupResult::MismatchSize => {
                let _ = self.dispatch_map.release_read(key);
                Err(PyRuntimeError::new_err("size mismatch on lookup"))
            }
        }
    }

    /// Shut down the engine, releasing all resources.
    pub fn shutdown(&self) -> PyResult<()> {
        if !self.initialized.swap(false, Ordering::AcqRel) {
            return Ok(());
        }

        self.dispatcher
            .shutdown()
            .map_err(|e| PyRuntimeError::new_err(format!("dispatcher shutdown failed: {e}")))?;

        self.gpu_services
            .shutdown()
            .map_err(|e| PyRuntimeError::new_err(format!("GPU shutdown failed: {e}")))?;

        Ok(())
    }
}
