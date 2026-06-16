//! Pipeline metrics trait for external instrumentation.
//!
//! The dispatcher records timing of internal data-path stages and reports
//! them through this trait. Implementors (e.g. certus-server's OTel layer)
//! can export the data to any observability backend without coupling the
//! dispatcher crate to a specific telemetry library.

/// Records timing of internal pipeline stages.
///
/// All durations are in microseconds. Implementors should be cheap to call
/// (e.g. atomic histogram updates) as these are invoked on hot paths.
pub trait PipelineMetrics: Send + Sync {
    // --- Cold lookup (SSD → memory-tier → GPU) ---

    /// Aggregated NVMe completion wait time for one batch.
    fn record_cold_ssd_read(&self, drive: usize, duration_us: f64);

    /// Aggregated GPU async DMA enqueue time for one batch.
    fn record_cold_gpu_dma(&self, duration_us: f64);

    /// Aggregated GPU stream synchronization time for one batch.
    fn record_cold_stream_sync(&self, duration_us: f64);

    /// Total cold pipeline execution time for one batch.
    fn record_cold_total(&self, drive: usize, duration_us: f64);

    /// Memory-tier eviction + slot insertion (prep phase).
    fn record_cold_prep(&self, duration_us: f64);

    /// Dispatch-map re-registration after promotion (finalize phase).
    fn record_cold_finalize(&self, duration_us: f64);

    // --- Hot lookup (memory-tier → GPU) ---

    /// Single hot-path GPU DMA copy (H2D) + stream sync.
    fn record_hot_gpu_dma(&self, duration_us: f64);

    // --- Populate (GPU → memory-tier → SSD) ---

    /// GPU device-to-host DMA copy duration.
    fn record_populate_gpu_d2h(&self, duration_us: f64);

    /// Memory-tier allocation (including eviction) duration.
    fn record_populate_alloc(&self, duration_us: f64);

    /// Total populate operation duration.
    fn record_populate_total(&self, duration_us: f64);
}
