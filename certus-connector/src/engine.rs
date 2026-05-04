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
    CacheKey, DispatcherConfig, IDispatchMap, IDispatcher, IGpuServices, IpcHandle,
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

        // --- Initialize SPDK environment ---
        let spdk_comp = spdk_env::SPDKEnvComponent::new_default();
        let spdk_iface = query_interface!(spdk_comp, spdk_env::ISPDKEnv)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query ISPDKEnv"))?;
        spdk_iface
            .init()
            .map_err(|e| PyRuntimeError::new_err(format!("SPDK init failed: {e}")))?;

        // --- Initialize GPU services ---
        let gpu_comp = gpu_services::GpuServicesComponentV0::new();
        let gpu: Arc<dyn IGpuServices + Send + Sync> =
            query_interface!(gpu_comp, IGpuServices)
                .ok_or_else(|| PyRuntimeError::new_err("failed to query IGpuServices"))?;
        gpu.initialize()
            .map_err(|e| PyRuntimeError::new_err(format!("GPU init failed: {e}")))?;

        // --- Create dispatch map ---
        let dm_comp = dispatch_map::DispatchMapComponentV0::new(
            dispatch_map::DispatchMapState::default(),
        );
        let dm: Arc<dyn IDispatchMap + Send + Sync> = query_interface!(dm_comp, IDispatchMap)
            .ok_or_else(|| PyRuntimeError::new_err("failed to query IDispatchMap"))?;
        dm.initialize()
            .map_err(|e| PyRuntimeError::new_err(format!("DispatchMap init failed: {e}")))?;

        // --- Create dispatcher ---
        let disp_comp = dispatcher::DispatcherComponentV0::new_default();
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

    // ─── Handler-level operations ──────────────────────────────────────

    /// Submit async GPU→DRAM→NVMe transfer (store).
    pub fn store_async(
        &self,
        job_id: u64,
        gpu_block_ids: &[u64],
        keys: &[u64],
    ) -> PyResult<bool> {
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
    pub fn load_async(
        &self,
        job_id: u64,
        gpu_block_ids: &[u64],
        keys: &[u64],
    ) -> PyResult<bool> {
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
