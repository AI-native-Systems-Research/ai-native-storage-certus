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
    CacheKey, DispatcherConfig, GpuStream, IDispatchMap,
    IDispatcher, IEvictionPolicy, IGpuServices, ILogger, IMemoryTier, IpcHandle, LookupResult,
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
    entry_count: AtomicU64,
    jobs: Mutex<HashMap<u64, Arc<TransferJob>>>,
    next_internal_id: AtomicU64,
    initialized: AtomicBool,
    store_stream: GpuStream,
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
        dm.initialize()
            .map_err(|e| PyRuntimeError::new_err(format!("DispatchMap init failed: {e}")))?;

        let numa_opt = if numa_node >= 0 { Some(numa_node as i32) } else { None };

        // --- Create memory tier ---
        let mt_comp = memory_tier::MemoryTierComponent::new_default();
        mt_comp
            .logger
            .connect(Arc::clone(&log) as Arc<dyn ILogger + Send + Sync>)
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire logger for memory-tier: {e}")))?;
        mt_comp
            .eviction_policy
            .connect(Arc::clone(&eviction_policy))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to wire eviction_policy for memory-tier: {e}")))?;
        let memory_tier: Arc<dyn IMemoryTier + Send + Sync> =
            query_interface!(mt_comp, IMemoryTier)
                .ok_or_else(|| PyRuntimeError::new_err("failed to query IMemoryTier"))?;
        let mt_pool_size = if dram_cache_bytes > 0 {
            dram_cache_bytes as usize
        } else {
            memory_tier::DEFAULT_POOL_SIZE
        };
        memory_tier
            .initialize(mt_pool_size, numa_opt)
            .map_err(|e| PyRuntimeError::new_err(format!("MemoryTier init failed: {e}")))?;

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
        disp_comp
            .memory_tier
            .connect(Arc::clone(&memory_tier))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to bind memory_tier: {e}")))?;

        let dispatcher: Arc<dyn IDispatcher + Send + Sync> =
            query_interface!(disp_comp, IDispatcher)
                .ok_or_else(|| PyRuntimeError::new_err("failed to query IDispatcher"))?;

        let max_cache_entries: usize = if slab_size_bytes > 0 && dram_cache_bytes > 0 {
            (dram_cache_bytes / slab_size_bytes) as usize
        } else {
            10000
        };

        dispatcher
            .initialize(DispatcherConfig {
                data_pci_addrs,
                max_cache_entries,
                format_on_init: true,
                ..Default::default()
            })
            .map_err(|e| PyRuntimeError::new_err(format!("Dispatcher init failed: {e}")))?;

        let store_stream = gpu.create_stream().map_err(|e| {
            PyRuntimeError::new_err(format!("failed to create store stream: {e}"))
        })?;

        Ok(Self {
            dispatcher,
            dispatch_map: dm,
            gpu_services: gpu,
            gpu_block_size,
            gpu_base_ptr,
            entry_count: AtomicU64::new(0),
            jobs: Mutex::new(HashMap::new()),
            next_internal_id: AtomicU64::new(0),
            initialized: AtomicBool::new(true),
            store_stream,
        })
    }

    fn ensure_init(&self) -> PyResult<()> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(PyRuntimeError::new_err("engine not initialized"));
        }
        Ok(())
    }

    /// Set the GPU KV cache base pointer and stride (called after vLLM allocates GPU tensors).
    pub fn set_gpu_base_ptr(&mut self, ptr: u64, stride: u64) {
        self.gpu_base_ptr = ptr;
        if stride > 0 {
            self.gpu_block_size = stride;
        }
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

    /// Check which keys need storing and reserve DRAM for them.
    /// Returns None if DRAM allocation fails (graceful backpressure).
    pub fn prepare_store(&self, keys: &[u64]) -> PyResult<Option<(Vec<u64>, Vec<u64>)>> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);
        let mut to_store = Vec::new();
        let mut to_store_cache_keys = Vec::new();

        for (i, key) in cache_keys.iter().enumerate() {
            match self.dispatcher.check(*key) {
                Ok(true) => {}
                Ok(false) | Err(_) => {
                    to_store.push(keys[i]);
                    to_store_cache_keys.push(*key);
                }
            }
        }

        if to_store.is_empty() {
            return Ok(Some((vec![], vec![])));
        }

        // Reserve DRAM slots for all keys that need storing.
        // If any reservation fails, release all prior reservations and return None.
        let size = self.gpu_block_size as u32;
        let mut reserved = Vec::new();
        for key in &to_store_cache_keys {
            match self.dispatcher.reserve_memory(*key, size) {
                Ok(_ptr) => {
                    reserved.push(*key);
                }
                Err(_) => {
                    for rkey in &reserved {
                        let _ = self.dispatcher.release_memory(*rkey);
                    }
                    return Ok(None);
                }
            }
        }

        Ok(Some((to_store, vec![])))
    }

    /// Finalize or abort a store operation.
    ///
    /// On success: registers each key in the dispatch-map and enqueues SSD write-through.
    /// On failure: releases the reserved DRAM slots.
    pub fn complete_store(&self, keys: &[u64], success: bool) -> PyResult<()> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);
        let size = self.gpu_block_size as u32;

        if success {
            for key in &cache_keys {
                if let Err(_e) = self.dispatcher.copy_gpu_to_memory_completed(*key, size) {
                    continue;
                }
                self.entry_count.fetch_add(1, Ordering::Release);
            }
        } else {
            for key in &cache_keys {
                if self.dispatcher.remove(*key).is_ok() {
                    self.entry_count.fetch_sub(1, Ordering::Release);
                } else {
                    let _ = self.dispatcher.release_memory(*key);
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

    /// Cumulative SSD I/O counters aggregated across all data drives:
    /// `(read_ops, read_bytes, read_latency_ns_sum, write_ops, write_bytes,
    /// write_latency_ns_sum)`. Latency sums divided by the matching op count give
    /// mean per-direction latency. Zero unless the block devices were built with
    /// the `telemetry` feature.
    pub fn io_byte_stats(&self) -> (u64, u64, u64, u64, u64, u64) {
        let s = self.dispatcher.io_byte_stats();
        (
            s.read_ops,
            s.read_bytes,
            s.read_latency_ns_sum,
            s.write_ops,
            s.write_bytes,
            s.write_latency_ns_sum,
        )
    }

    /// Pin blocks for reading and return DRAM pointers for H2D DMA.
    ///
    /// For MemoryTier keys: takes read_ref, returns pointer directly.
    /// For BlockDevice keys: promotes to memory-tier (NVMe→DRAM), then
    /// takes read_ref on the new MemoryTier entry.
    ///
    /// Returns Vec<(dram_ptr, size)> for each key. Caller MUST call
    /// `complete_load` when GPU DMA is done to release read_refs.
    pub fn prepare_load(&self, keys: &[u64]) -> PyResult<Vec<(u64, u32)>> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);

        // First pass: identify which keys need NVMe→DRAM promotion.
        let mut needs_promote: Vec<CacheKey> = Vec::new();
        for key in &cache_keys {
            match self.dispatch_map.lookup(*key) {
                Ok(LookupResult::BlockDevice { .. }) => {
                    let _ = self.dispatch_map.release_read(*key);
                    needs_promote.push(*key);
                }
                Ok(_) => {
                    let _ = self.dispatch_map.release_read(*key);
                }
                Err(_) => {}
            }
        }

        // Promote cold keys (NVMe→DRAM). This is synchronous but batched
        // across drives internally.
        if !needs_promote.is_empty() {
            self.dispatcher.promote_to_memory_tier(&needs_promote);
        }

        // Second pass: take read_refs and collect DRAM pointers.
        let mut results = Vec::with_capacity(cache_keys.len());
        for (i, key) in cache_keys.iter().enumerate() {
            match self.dispatch_map.lookup(*key) {
                Ok(LookupResult::MemoryTier { pointer, size }) => {
                    results.push((pointer as u64, size));
                }
                Ok(LookupResult::BlockDevice { .. }) => {
                    // Promotion failed for this key — rollback and error.
                    let _ = self.dispatch_map.release_read(*key);
                    for prev_key in &cache_keys[..i] {
                        let _ = self.dispatch_map.release_read(*prev_key);
                    }
                    return Err(PyRuntimeError::new_err(format!(
                        "prepare_load: key {key} still on BlockDevice after promotion"
                    )));
                }
                Ok(LookupResult::NotExist) => {
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
                    for prev_key in &cache_keys[..i] {
                        let _ = self.dispatch_map.release_read(*prev_key);
                    }
                    return Err(PyRuntimeError::new_err(format!(
                        "prepare_load: lookup failed for key {key}: {e:?}"
                    )));
                }
            }
        }

        Ok(results)
    }

    /// Unpin blocks after load DMA completes. Decrements `read_ref` so
    /// blocks become eligible for eviction again.
    pub fn complete_load(&self, keys: &[u64]) -> PyResult<()> {
        self.ensure_init()?;
        let cache_keys = keys::to_cache_keys(keys);

        for key in &cache_keys {
            let _ = self.dispatch_map.release_read(*key);
        }

        Ok(())
    }

    // ─── Handler-level operations ──────────────────────────────────────

    /// Raw DRAM→GPU DMA transfer. No dispatch-map interaction.
    ///
    /// Takes pre-computed DRAM source pointers (from `prepare_load`) and
    /// issues async H2D copies. Returns immediately; completion is detected
    /// by `poll_completions` via stream synchronization.
    pub fn load_dma(&self, job_id: u64, gpu_block_ids: &[u64], src_ptrs: &[u64]) -> PyResult<bool> {
        self.ensure_init()?;

        if gpu_block_ids.len() != src_ptrs.len() {
            return Err(PyRuntimeError::new_err(
                "gpu_block_ids and src_ptrs must have same length",
            ));
        }

        let stream = self.gpu_services.create_stream().map_err(|e| {
            PyRuntimeError::new_err(format!("load_dma: create_stream failed: {e}"))
        })?;

        let mut all_ok = true;
        for (i, src_ptr) in src_ptrs.iter().enumerate() {
            let block_id = gpu_block_ids[i];
            let gpu_ptr = self.gpu_base_ptr + block_id * self.gpu_block_size;

            if let Err(e) = self.gpu_services.memcpy_h2d_async(
                *src_ptr as *const std::ffi::c_void,
                gpu_ptr as *mut std::ffi::c_void,
                self.gpu_block_size as usize,
                stream,
            ) {
                eprintln!("[certus] load_dma memcpy_h2d_async failed block_id={block_id}: {e}");
                all_ok = false;
                break;
            }
        }

        let cache_keys = gpu_block_ids.iter().map(|&id| id as CacheKey).collect();
        let completed = !all_ok;
        let job = Arc::new(TransferJob {
            kind: JobKind::Load,
            keys: cache_keys,
            gpu_block_ids: gpu_block_ids.to_vec(),
            completed: AtomicBool::new(completed),
            success: AtomicBool::new(all_ok),
            stream: Mutex::new(if all_ok { Some(stream) } else { None }),
        });

        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(job_id, job);
        }

        Ok(all_ok)
    }

    /// Submit GPU→DRAM transfer (store).
    ///
    /// DMA-copies each block from GPU into the pre-reserved DRAM slot.
    /// The slots were allocated by `prepare_store` via `reserve_memory`.
    pub fn store_async(&self, job_id: u64, gpu_block_ids: &[u64], keys: &[u64]) -> PyResult<bool> {
        self.ensure_init()?;

        if gpu_block_ids.len() != keys.len() {
            return Err(PyRuntimeError::new_err(
                "gpu_block_ids and keys must have same length",
            ));
        }

        let cache_keys = keys::to_cache_keys(keys);

        let stream = self.store_stream;

        let mut all_ok = true;
        for (i, key) in cache_keys.iter().enumerate() {
            let block_id = gpu_block_ids[i];
            let gpu_ptr = self.gpu_base_ptr + block_id * self.gpu_block_size;

            let handle = IpcHandle {
                address: gpu_ptr as *mut u8,
                size: self.gpu_block_size as u32,
            };

            // Async DMA from GPU into the DRAM slot reserved by prepare_store.
            match self.dispatcher.copy_gpu_to_memory_async(*key, handle, stream) {
                Ok(()) => {}
                Err(interfaces::DispatcherError::AlreadyExists(_)) => continue,
                Err(e) => {
                    eprintln!("[certus] store_async copy_gpu_to_memory_async failed key={key} block_id={block_id}: {e:?}");
                    all_ok = false;
                    break;
                }
            }
        }

        let completed = !all_ok;
        let job = Arc::new(TransferJob {
            kind: JobKind::Store,
            keys: cache_keys.clone(),
            gpu_block_ids: gpu_block_ids.to_vec(),
            completed: AtomicBool::new(completed),
            success: AtomicBool::new(all_ok),
            stream: Mutex::new(Some(stream)),
        });

        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(job_id, Arc::clone(&job));
        }

        Ok(all_ok)
    }

    /// Submit async NVMe/DRAM→GPU transfer (load).
    ///
    /// Issues non-blocking H2D DMA via `lookup_async`. Completion is detected
    /// by `poll_completions` via stream synchronization (same stream as stores).
    pub fn load_async(&self, job_id: u64, gpu_block_ids: &[u64], keys: &[u64]) -> PyResult<bool> {
        self.ensure_init()?;

        if gpu_block_ids.len() != keys.len() {
            return Err(PyRuntimeError::new_err(
                "gpu_block_ids and keys must have same length",
            ));
        }

        let cache_keys = keys::to_cache_keys(keys);

        // Issue async DMA for each block BEFORE inserting into the jobs map.
        let mut last_stream: Option<GpuStream> = None;
        let mut all_ok = true;
        for (i, key) in cache_keys.iter().enumerate() {
            let block_id = gpu_block_ids[i];
            let gpu_ptr = self.gpu_base_ptr + block_id * self.gpu_block_size;

            let handle = IpcHandle {
                address: gpu_ptr as *mut u8,
                size: self.gpu_block_size as u32,
            };

            match self.dispatcher.lookup_async(*key, handle) {
                Ok(stream) => {
                    if !stream.0.is_null() {
                        last_stream = Some(stream);
                    }
                }
                Err(e) => {
                    eprintln!("[certus] load_async lookup_async failed key={key} block_id={block_id}: {e:?}");
                    all_ok = false;
                    break;
                }
            }
        }

        let completed = !all_ok || last_stream.is_none();
        let job = Arc::new(TransferJob {
            kind: JobKind::Load,
            keys: cache_keys.clone(),
            gpu_block_ids: gpu_block_ids.to_vec(),
            completed: AtomicBool::new(completed),
            success: AtomicBool::new(all_ok),
            stream: Mutex::new(last_stream),
        });

        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.insert(job_id, Arc::clone(&job));
        }

        Ok(all_ok)
    }

    /// Poll for completed transfers. Returns list of (job_id, success).
    ///
    /// Both stores and loads share the same CUDA stream. A single
    /// `stream_synchronize` covers all in-flight DMA in both directions.
    /// Store jobs additionally need `copy_gpu_to_memory_completed` to register entries.
    pub fn poll_completions(&self) -> PyResult<Vec<(u64, bool)>> {
        self.ensure_init()?;
        let mut completions = Vec::new();
        let mut jobs = self.jobs.lock().unwrap();

        // Collect in-flight jobs (not yet completed).
        let in_flight_ids: Vec<u64> = jobs
            .iter()
            .filter(|(_, job)| !job.completed.load(Ordering::Acquire))
            .map(|(id, _)| *id)
            .collect();

        for id in &in_flight_ids {
            let job = jobs.get(id).unwrap();

            let stream_opt = { job.stream.lock().unwrap().take() };
            if let Some(stream) = stream_opt {
                if let Err(e) = self.gpu_services.stream_synchronize(stream) {
                    eprintln!("[certus] stream_synchronize failed: {e}");
                    job.completed.store(true, Ordering::Release);
                    job.success.store(false, Ordering::Release);
                    continue;
                }
            }

            let mut all_ok = true;

            // Store jobs: copy_gpu_to_memory_completed registers in dispatch-map.
            // Load jobs are already committed — just need the stream sync.
            if job.kind == JobKind::Store {
                let size = self.gpu_block_size as u32;
                for key in &job.keys {
                    match self.dispatcher.copy_gpu_to_memory_completed(*key, size) {
                        Ok(()) => {
                            self.entry_count.fetch_add(1, Ordering::Release);
                        }
                        Err(interfaces::DispatcherError::KeyNotFound(_)) => {}
                        Err(interfaces::DispatcherError::AlreadyExists(_)) => {}
                        Err(e) => {
                            eprintln!("[certus] copy_gpu_to_memory_completed failed key={key}: {e:?}");
                            all_ok = false;
                        }
                    }
                }
            }

            job.completed.store(true, Ordering::Release);
            job.success.store(all_ok, Ordering::Release);
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
    /// then calls `copy_gpu_to_memory_completed` for each key.
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

            let mut all_ok = true;
            if job.kind == JobKind::Store {
                let size = self.gpu_block_size as u32;
                for key in &job.keys {
                    match self.dispatcher.copy_gpu_to_memory_completed(*key, size) {
                        Ok(()) => {
                            self.entry_count.fetch_add(1, Ordering::Release);
                        }
                        Err(interfaces::DispatcherError::KeyNotFound(_)) => {}
                        Err(interfaces::DispatcherError::AlreadyExists(_)) => {}
                        Err(e) => {
                            eprintln!("[certus] wait_job copy_gpu_to_memory_completed failed key={key}: {e:?}");
                            all_ok = false;
                        }
                    }
                }
            }
            job.completed.store(true, Ordering::Release);
            job.success.store(all_ok, Ordering::Release);
        }

        Ok(())
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
