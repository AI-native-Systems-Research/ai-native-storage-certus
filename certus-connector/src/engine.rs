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
    CacheKey, DispatcherConfig, DmaAllocFn, DmaBuffer, GpuStream, IDispatchMap, IDispatcher,
    IEvictionPolicy, IGpuServices, ILogger, IpcHandle, LookupResult,
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
    stream: Mutex<Option<GpuStream>>,
}

// SAFETY: GpuStream wraps a CUDA stream pointer which is thread-safe to poll/sync from any thread.
unsafe impl Send for TransferJob {}
unsafe impl Sync for TransferJob {}

// ─── EngineInner ───────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct EngineInner {
    dispatcher: Arc<dyn IDispatcher + Send + Sync>,
    dispatch_map: Arc<dyn IDispatchMap + Send + Sync>,
    gpu_services: Arc<dyn IGpuServices + Send + Sync>,
    gpu_block_size: u64,
    gpu_base_ptr: u64,
    max_cache_entries: usize,
    eviction_watermark: usize,
    entry_count: AtomicU64,
    jobs: Mutex<HashMap<u64, Arc<TransferJob>>>,
    next_internal_id: AtomicU64,
    initialized: AtomicBool,
}

impl EngineInner {
    /// Construct from a Python config dict.
    ///
    /// Instantiates and wires all Certus components:
    /// - SPDKEnvComponent (environment init)
    /// - GpuServicesComponent (CUDA init)
    /// - DispatchMapComponent (key→location index)
    /// - DispatcherComponent (orchestration)
    pub fn from_config(config: &Bound<'_, PyDict>) -> PyResult<Self> {
        let data_pci_addrs: Vec<String> = config
            .get_item("data_pci_addrs")?
            .ok_or_else(|| PyRuntimeError::new_err("missing 'data_pci_addrs'"))?
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

        let numa_node: i32 = config
            .get_item("numa_node")?
            .and_then(|v| v.extract().ok())
            .unwrap_or(-1);

        let gpu_base_ptr: u64 = config
            .get_item("gpu_base_ptr")?
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
        let log_comp = logger::LoggerComponent::new_default();
        let log: Arc<dyn ILogger + Send + Sync> = query_interface!(log_comp, ILogger)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query ILogger"))?;

        // --- Initialize GPU services ---
        let gpu_comp = gpu_services::GpuServicesComponent::new_default();
        let gpu: Arc<dyn IGpuServices + Send + Sync> = query_interface!(gpu_comp, IGpuServices)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query IGpuServices"))?;
        gpu.initialize()
            .map_err(|e| PyRuntimeError::new_err(format!("GPU init failed: {e}")))?;

        // --- Create eviction policy ---
        let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
        let eviction_policy: Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(ep_comp, IEvictionPolicy)
                .ok_or_else(|| PyRuntimeError::new_err("failed to query IEvictionPolicy"))?;

        // --- Create dispatch map (no persistence — starts fresh each time) ---
        let dm_comp =
            dispatch_map::DispatchMapComponent::new(dispatch_map::DispatchMapState::default());
        dm_comp
            .logger
            .connect(Arc::clone(&log) as Arc<dyn ILogger + Send + Sync>)
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire logger for dispatch map: {e}")))?;
        dm_comp
            .eviction_policy
            .connect(Arc::clone(&eviction_policy))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire eviction_policy for dispatch map: {e}")))?;
        let dm: Arc<dyn IDispatchMap + Send + Sync> = query_interface!(dm_comp, IDispatchMap)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query IDispatchMap"))?;
        let numa_opt = if numa_node >= 0 { Some(numa_node as i32) } else { None };
        let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
            DmaBuffer::new(size, align, numa_opt).map_err(|e| e.to_string())
        });
        dm.set_dma_alloc(dma_alloc);
        dm.initialize()
            .map_err(|e| PyRuntimeError::new_err(format!("DispatchMap init failed: {e}")))?;

        // --- Create dispatcher ---
        let disp_comp = dispatcher::DispatcherComponent::new_default();
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
                data_pci_addrs,
                max_cache_entries,
                eviction_threshold,
                format_on_init: true,
                ..Default::default()
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Dispatcher init failed: {e}")))?;

        let eviction_watermark =
            (max_cache_entries as f64 * eviction_threshold) as usize;

        Ok(Self {
            dispatcher,
            dispatch_map: dm,
            gpu_services: gpu,
            gpu_block_size,
            gpu_base_ptr,
            max_cache_entries,
            eviction_watermark,
            entry_count: AtomicU64::new(0),
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

    /// Allocate space for new keys, evicting LRU entries if necessary.
    /// Returns (keys_to_store, evicted_keys), or None if eviction cannot
    /// free enough space.
    pub fn prepare_store(&self, keys: &[u64]) -> PyResult<Option<(Vec<u64>, Vec<u64>)>> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);
        let mut to_store = Vec::new();
        let protected: std::collections::HashSet<CacheKey> =
            cache_keys.iter().copied().collect();

        for (i, key) in cache_keys.iter().enumerate() {
            match self.dispatcher.check(*key) {
                Ok(true) => {}
                Ok(false) | Err(_) => {
                    to_store.push(keys[i]);
                }
            }
        }

        if to_store.is_empty() {
            return Ok(Some((vec![], vec![])));
        }

        let current_count = self.entry_count.load(Ordering::Acquire) as usize;
        let after_store = current_count + to_store.len();
        let evicted = if after_store > self.eviction_watermark {
            let needed = after_store - self.eviction_watermark;
            let candidates = self.dispatch_map.oldest_keys(usize::MAX);
            let mut evicted_keys: Vec<u64> = Vec::new();
            for candidate in candidates {
                if evicted_keys.len() >= needed {
                    break;
                }
                if protected.contains(&candidate) {
                    continue;
                }
                match self.dispatcher.remove(candidate) {
                    Ok(()) => {
                        self.entry_count.fetch_sub(1, Ordering::Release);
                        evicted_keys.push(candidate);
                    }
                    Err(_) => continue,
                }
            }
            if evicted_keys.len() < needed {
                return Ok(None);
            }
            evicted_keys
        } else {
            vec![]
        };

        Ok(Some((to_store, evicted)))
    }

    /// Finalize or abort a store operation.
    pub fn complete_store(&self, keys: &[u64], success: bool) -> PyResult<()> {
        self.ensure_init()?;
        if !success {
            let cache_keys = keys::to_cache_keys(keys);
            for key in &cache_keys {
                if self.dispatcher.remove(*key).is_ok() {
                    self.entry_count.fetch_sub(1, Ordering::Release);
                }
            }
        }
        Ok(())
    }

    /// Update LRU ordering for the given keys.
    pub fn touch(&self, keys: &[u64]) -> PyResult<()> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);
        for key in &cache_keys {
            let _ = self.dispatcher.touch(*key);
        }
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
    ///
    /// Issues all D2H DMA copies via `populate_async` without blocking.
    /// Completion is detected by `poll_completions` via `stream_query`.
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
            stream: Mutex::new(None),
        });

        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(job_id, Arc::clone(&job));
        }

        // Issue async DMA for each block. All copies share the same CUDA stream
        // so a single stream_query covers all of them.
        let mut last_stream: Option<GpuStream> = None;
        let mut all_ok = true;
        for (i, key) in cache_keys.iter().enumerate() {
            let block_id = gpu_block_ids[i];
            let gpu_ptr = self.gpu_base_ptr + block_id * self.gpu_block_size;

            let handle = IpcHandle {
                address: gpu_ptr as *mut u8,
                size: self.gpu_block_size as u32,
            };

            match self.dispatcher.populate_async(*key, handle) {
                Ok(stream) => {
                    if !stream.0.is_null() {
                        last_stream = Some(stream);
                    } else {
                        // Null stream = completed synchronously (fallback path).
                        self.entry_count.fetch_add(1, Ordering::Release);
                    }
                }
                Err(e) => {
                    let msg = format!("{e:?}");
                    if msg.contains("already exists") || msg.contains("AlreadyExists") {
                        continue;
                    }
                    eprintln!("[certus] store_async populate_async failed key={key} block_id={block_id}: {msg}");
                    all_ok = false;
                    break;
                }
            }
        }

        if !all_ok || last_stream.is_none() {
            // Either failed or all completed synchronously.
            job.completed.store(true, Ordering::Release);
            job.success.store(all_ok, Ordering::Release);
        } else {
            // DMA is in-flight — store the stream for poll_completions.
            *job.stream.lock().unwrap() = last_stream;
        }

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
            stream: Mutex::new(None),
        });

        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(job_id, Arc::clone(&job));
        }

        // Execute load: for each block, create an IpcHandle pointing at the
        // absolute GPU device address and call dispatcher.lookup().
        let mut all_ok = true;
        for (i, key) in cache_keys.iter().enumerate() {
            let block_id = gpu_block_ids[i];
            let gpu_ptr = self.gpu_base_ptr + block_id * self.gpu_block_size;

            let handle = IpcHandle {
                address: gpu_ptr as *mut u8,
                size: self.gpu_block_size as u32,
            };

            if let Err(e) = self.dispatcher.lookup(*key, handle) {
                eprintln!("[certus] load_async lookup failed key={key} block_id={block_id}: {e:?}");
                all_ok = false;
                break;
            }
        }

        job.completed.store(true, Ordering::Release);
        job.success.store(all_ok, Ordering::Release);

        Ok(all_ok)
    }

    /// Poll for completed transfers. Returns list of (job_id, success).
    ///
    /// For store jobs with in-flight DMA, polls the CUDA stream via
    /// `stream_query`. On completion, calls `populate_finalize` for each
    /// key to register entries in the dispatch-map and enqueue SSD writes.
    pub fn poll_completions(&self) -> PyResult<Vec<(u64, bool)>> {
        self.ensure_init()?;
        let mut completions = Vec::new();
        let mut jobs = self.jobs.lock().unwrap();

        // First pass: check in-flight store jobs for stream completion.
        let in_flight_ids: Vec<u64> = jobs
            .iter()
            .filter(|(_, job)| !job.completed.load(Ordering::Acquire))
            .map(|(id, _)| *id)
            .collect();

        for id in &in_flight_ids {
            let job = jobs.get(id).unwrap();
            let stream_opt = { job.stream.lock().unwrap().take() };

            if let Some(stream) = stream_opt {
                match self.gpu_services.stream_query(stream) {
                    Ok(true) => {
                        // DMA complete — finalize all keys.
                        let mut all_ok = true;
                        for key in &job.keys {
                            match self.dispatcher.populate_finalize(*key) {
                                Ok(()) => {
                                    self.entry_count.fetch_add(1, Ordering::Release);
                                }
                                Err(e) => {
                                    eprintln!("[certus] populate_finalize failed key={key}: {e:?}");
                                    all_ok = false;
                                }
                            }
                        }
                        job.completed.store(true, Ordering::Release);
                        job.success.store(all_ok, Ordering::Release);
                    }
                    Ok(false) => {
                        // Still in-flight — put the stream back.
                        *job.stream.lock().unwrap() = Some(stream);
                    }
                    Err(e) => {
                        eprintln!("[certus] stream_query failed: {e}");
                        job.completed.store(true, Ordering::Release);
                        job.success.store(false, Ordering::Release);
                    }
                }
            }
        }

        // Second pass: collect all completed jobs.
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
    ///
    /// For in-flight store jobs, synchronizes the CUDA stream (blocking)
    /// then calls `populate_finalize` for each key.
    pub fn wait_job(&self, job_id: u64) -> PyResult<()> {
        self.ensure_init()?;

        let jobs = self.jobs.lock().unwrap();
        let job = match jobs.get(&job_id) {
            Some(j) => Arc::clone(j),
            None => return Ok(()),
        };
        drop(jobs);

        if job.completed.load(Ordering::Acquire) {
            return Ok(());
        }

        // Take the stream and block on it.
        let stream_opt = { job.stream.lock().unwrap().take() };

        if let Some(stream) = stream_opt {
            if let Err(e) = self.gpu_services.stream_synchronize(stream) {
                eprintln!("[certus] wait_job stream_synchronize failed: {e}");
                job.completed.store(true, Ordering::Release);
                job.success.store(false, Ordering::Release);
                return Ok(());
            }

            // DMA complete — finalize all keys.
            let mut all_ok = true;
            for key in &job.keys {
                match self.dispatcher.populate_finalize(*key) {
                    Ok(()) => {
                        self.entry_count.fetch_add(1, Ordering::Release);
                    }
                    Err(e) => {
                        eprintln!("[certus] wait_job populate_finalize failed key={key}: {e:?}");
                        all_ok = false;
                    }
                }
            }
            job.completed.store(true, Ordering::Release);
            job.success.store(all_ok, Ordering::Release);
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
            .map_err(|e| PyRuntimeError::new_err(format!("store_host_bytes commit failed: {e}")))?;

        self.entry_count.fetch_add(1, Ordering::Release);
        Ok(())
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
