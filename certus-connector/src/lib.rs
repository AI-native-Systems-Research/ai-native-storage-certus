//! certus_native — PyO3 module exposing the Certus storage engine to Python.
//!
//! Provides `CertusEngine`, a Python class that wraps the Certus component stack
//! (dispatcher, dispatch-map, extent-manager, block-device, gpu-services) for
//! use by vLLM's OffloadingSpec interface.
//!
//! The Python `certus_connector` package calls into this module for:
//! - Index operations (batch_check, touch)
//! - Allocation and eviction (prepare_store, complete_store)
//! - Load pinning (prepare_load, complete_load)
//! - Async I/O (store_async, load_async, poll_completions)

mod engine;
mod keys;

use pyo3::prelude::*;
use pyo3::types::PyDict;

use engine::EngineInner;

/// Configuration for the Certus engine.
#[pyclass]
#[derive(Clone, Debug)]
pub struct CertusConfig {
    /// PCI address of the data NVMe device(s).
    pub data_pci_addrs: Vec<String>,
    /// PCI address of the metadata NVMe device.
    pub metadata_pci_addr: String,
    /// Slab size in bytes (extent allocation unit).
    pub slab_size_bytes: u64,
    /// DRAM memory-tier cache budget in bytes.
    pub dram_cache_bytes: u64,
    /// SPDK I/O queue depth.
    pub io_queue_depth: u32,
    /// GPU KV cache block size in bytes.
    pub gpu_block_size: u64,
}

/// The Certus storage engine exposed to Python.
///
/// Wraps the full Certus component stack: SPDK NVMe block devices,
/// extent managers, dispatch map, GPU DMA services, and the dispatcher.
///
/// # Usage from Python
///
/// ```python
/// import certus_native
///
/// engine = certus_native.CertusEngine({
///     "data_pci_addrs": ["0000:02:00.0"],
///     "metadata_pci_addr": "0000:01:00.0",
///     "slab_size_bytes": 131072,
///     "dram_cache_bytes": 8589934592,
///     "io_queue_depth": 128,
///     "gpu_block_size": 131072,
/// })
///
/// # Manager-level operations
/// hit_count = engine.batch_check([1, 2, 3])
/// result = engine.prepare_store([4, 5, 6])  # None if can't free space
/// if result is not None:
///     to_store, evicted = result
///     engine.complete_store([4, 5, 6], True)
///
/// # Handler-level operations
/// engine.store_async(job_id=1, gpu_block_ids=[0, 1], keys=[4, 5])
/// engine.load_async(job_id=2, gpu_block_ids=[2, 3], keys=[4, 5])
/// completions = engine.poll_completions()
///
/// engine.shutdown()
/// ```
#[pyclass]
pub struct CertusEngine {
    inner: EngineInner,
}

#[pymethods]
impl CertusEngine {
    #[new]
    fn new(config: &Bound<'_, PyDict>) -> PyResult<Self> {
        let inner = EngineInner::from_config(config)?;
        Ok(Self { inner })
    }

    // ─── Manager-level operations ───────────────────────────────────────

    /// Return the count of consecutive keys (from the start) that are cached.
    fn batch_check(&self, keys: Vec<u64>) -> PyResult<u64> {
        self.inner.batch_check(&keys)
    }

    /// Reserve DRAM slots for new keys, evicting if necessary.
    /// Returns `(keys_to_store, dram_ptrs, evicted_keys)` — `dram_ptrs[i]` is the
    /// reserved slot address for `keys_to_store[i]`, passed to `store_dma`.
    /// May store a subset (partial success); `None` only on a hard failure.
    fn prepare_store(
        &self,
        keys: Vec<u64>,
    ) -> PyResult<Option<(Vec<u64>, Vec<u64>, Vec<u64>)>> {
        self.inner.prepare_store(&keys)
    }

    /// Finalize or abort a store operation.
    fn complete_store(&self, keys: Vec<u64>, success: bool) -> PyResult<()> {
        self.inner.complete_store(&keys, success)
    }

    /// Pin blocks for reading, promote cold keys to DRAM, return
    /// (dram_ptr, size) for each key. Call complete_load when GPU DMA finishes.
    fn prepare_load(&self, keys: Vec<u64>) -> PyResult<Vec<(u64, u32)>> {
        self.inner.prepare_load(&keys)
    }

    /// Unpin blocks after load DMA completes.
    fn complete_load(&self, keys: Vec<u64>) -> PyResult<()> {
        self.inner.complete_load(&keys)
    }

    /// Update LRU ordering for the given keys.
    fn touch(&self, keys: Vec<u64>) -> PyResult<()> {
        self.inner.touch(&keys)
    }

    /// Cumulative SSD I/O counters across all data drives, as
    /// `(read_ops, read_bytes, read_latency_ns_sum, write_ops, write_bytes,
    /// write_latency_ns_sum)`. Monotonic; take deltas for a per-round measurement
    /// (mean latency = Δlatency_sum / Δops). Zero unless certus_native was built
    /// with the `telemetry` feature.
    fn read_write_stats(&self) -> (u64, u64, u64, u64, u64, u64) {
        self.inner.read_write_stats()
    }

    /// Cumulative cache-level counters as
    /// `(mem_tier_hits, ssd_hits, misses, mem_tier_evictions)`: load blocks
    /// served from DRAM, load blocks resolved from NVMe (each an SSD read), load
    /// blocks not present, and blocks demoted out of the DRAM memory tier under
    /// capacity pressure. Monotonic; take deltas for a per-round measurement.
    /// Comparing `mem_tier_hits` to `ssd_hits` shows what fraction of loads were
    /// served from DRAM vs SSD; `mem_tier_evictions` tracks store-path pressure.
    fn cache_stats(&self) -> (u64, u64, u64, u64) {
        self.inner.cache_stats()
    }

    /// Live DRAM memory-tier occupancy as `(used_bytes, capacity_bytes)`.
    fn mem_tier_usage(&self) -> (u64, u64) {
        self.inner.mem_tier_usage()
    }

    /// Cumulative completed store entries (net of removals).
    fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    /// Current live dispatch-map entry counts `(total, memory_tier, block_device)`.
    fn index_stats(&self) -> (u64, u64, u64) {
        self.inner.index_stats()
    }

    // ─── Handler-level operations ───────────────────────────────────────

    /// Submit async GPU→DRAM→NVMe transfer (legacy — use store_dma instead).
    fn store_async(&self, job_id: u64, gpu_block_ids: Vec<u64>, keys: Vec<u64>) -> PyResult<bool> {
        self.inner.store_async(job_id, &gpu_block_ids, &keys)
    }

    /// Raw GPU→DRAM async DMA. Takes pre-reserved destination pointers from
    /// prepare_store. No key lookup — cannot fail with KeyNotFound.
    fn store_dma(&self, job_id: u64, gpu_block_ids: Vec<u64>, dst_ptrs: Vec<u64>) -> PyResult<bool> {
        self.inner.store_dma(job_id, &gpu_block_ids, &dst_ptrs)
    }

    /// Submit async NVMe/DRAM→GPU transfer (legacy — use load_dma instead).
    fn load_async(&self, job_id: u64, gpu_block_ids: Vec<u64>, keys: Vec<u64>) -> PyResult<bool> {
        self.inner.load_async(job_id, &gpu_block_ids, &keys)
    }

    /// Raw DRAM→GPU async DMA. Takes pre-computed source pointers from prepare_load.
    fn load_dma(&self, job_id: u64, gpu_block_ids: Vec<u64>, src_ptrs: Vec<u64>) -> PyResult<bool> {
        self.inner.load_dma(job_id, &gpu_block_ids, &src_ptrs)
    }

    /// Poll for completed transfers. Returns list of (job_id, success).
    fn poll_completions(&self) -> PyResult<Vec<(u64, bool)>> {
        self.inner.poll_completions()
    }

    /// Block until a specific job completes.
    fn wait_job(&self, job_id: u64) -> PyResult<()> {
        self.inner.wait_job(job_id)
    }

    /// Set the GPU KV cache base pointer and stride (called after vLLM allocates GPU tensors).
    fn set_gpu_base_ptr(&mut self, ptr: u64, stride: u64) {
        self.inner.set_gpu_base_ptr(ptr, stride);
    }

    /// Shut down the engine, releasing all resources.
    fn shutdown(&self) -> PyResult<()> {
        self.inner.shutdown()
    }

}

/// Python module definition.
#[pymodule]
fn certus_native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<CertusEngine>()?;
    m.add_class::<CertusConfig>()?;
    Ok(())
}
