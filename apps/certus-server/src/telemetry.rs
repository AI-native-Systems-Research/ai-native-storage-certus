//! OpenTelemetry metrics exporter for Certus server.
//!
//! Exports dispatcher operation metrics via OTLP (gRPC) to an
//! OpenTelemetry Collector or any OTLP-compatible backend (Grafana Cloud,
//! Prometheus with OTLP receiver, etc.).

use std::sync::Arc;
use std::time::Duration;

use opentelemetry::metrics::{Counter, Histogram, Meter, MeterProvider};
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::Resource;

/// Holds all OTel metric instruments for the dispatcher service.
#[derive(Clone)]
pub struct Metrics {
    pub ops_total: Counter<u64>,
    pub ops_errors: Counter<u64>,
    pub op_duration_us: Histogram<f64>,
    pub batch_size: Histogram<u64>,
    pub entries_cleared: Counter<u64>,
    pub jobs_flushed: Counter<u64>,
    pub pipeline: PipelineStageMetrics,
    _provider: Arc<SdkMeterProvider>,
}

/// OTel histogram instruments for internal pipeline stages.
#[derive(Clone)]
pub struct PipelineStageMetrics {
    cold_ssd_read_us: Histogram<f64>,
    cold_gpu_dma_us: Histogram<f64>,
    cold_stream_sync_us: Histogram<f64>,
    cold_total_us: Histogram<f64>,
    cold_prep_us: Histogram<f64>,
    cold_finalize_us: Histogram<f64>,
    hot_gpu_dma_us: Histogram<f64>,
    populate_gpu_d2h_us: Histogram<f64>,
    populate_alloc_us: Histogram<f64>,
    populate_total_us: Histogram<f64>,
}

impl Metrics {
    /// Initialize the OTLP metrics pipeline.
    ///
    /// `endpoint` is the OTLP gRPC target (e.g. "http://localhost:4317").
    /// `service_name` identifies this instance in dashboards.
    pub fn init(endpoint: &str, service_name: &str) -> Result<Self, String> {
        use opentelemetry_otlp::MetricExporter;
        use opentelemetry_sdk::metrics::PeriodicReader;
        use opentelemetry_sdk::runtime::Tokio;

        let exporter = MetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .map_err(|e| format!("failed to create OTLP exporter: {e}"))?;

        let reader = PeriodicReader::builder(exporter, Tokio)
            .with_interval(Duration::from_secs(10))
            .build();

        let resource = Resource::new(vec![KeyValue::new(
            "service.name",
            service_name.to_string(),
        )]);

        let provider = SdkMeterProvider::builder()
            .with_reader(reader)
            .with_resource(resource)
            .build();

        let meter: Meter = provider.meter("certus-server");

        let ops_total = meter
            .u64_counter("certus.dispatcher.ops.total")
            .with_description("Total dispatcher operations")
            .build();

        let ops_errors = meter
            .u64_counter("certus.dispatcher.ops.errors")
            .with_description("Total dispatcher operation errors")
            .build();

        let op_duration_us = meter
            .f64_histogram("certus.dispatcher.op.duration_us")
            .with_description("Dispatcher operation duration in microseconds")
            .build();

        let batch_size = meter
            .u64_histogram("certus.dispatcher.batch.size")
            .with_description("Number of entries per batch request")
            .build();

        let entries_cleared = meter
            .u64_counter("certus.dispatcher.entries_cleared")
            .with_description("Total memory-tier entries cleared")
            .build();

        let jobs_flushed = meter
            .u64_counter("certus.dispatcher.jobs_flushed")
            .with_description("Total background jobs flushed to SSD")
            .build();

        // Record zero so metrics exist in Prometheus immediately.
        entries_cleared.add(0, &[]);
        jobs_flushed.add(0, &[]);

        let pipeline = PipelineStageMetrics {
            cold_ssd_read_us: meter
                .f64_histogram("certus.pipeline.cold.ssd_read_us")
                .with_description("NVMe read completion wait (aggregated per batch, µs)")
                .build(),
            cold_gpu_dma_us: meter
                .f64_histogram("certus.pipeline.cold.gpu_dma_us")
                .with_description("GPU async DMA enqueue time (aggregated per batch, µs)")
                .build(),
            cold_stream_sync_us: meter
                .f64_histogram("certus.pipeline.cold.stream_sync_us")
                .with_description("GPU stream synchronization time (aggregated per batch, µs)")
                .build(),
            cold_total_us: meter
                .f64_histogram("certus.pipeline.cold.total_us")
                .with_description("Total cold pipeline execution (µs)")
                .build(),
            cold_prep_us: meter
                .f64_histogram("certus.pipeline.cold.prep_us")
                .with_description("Memory-tier eviction + slot insert (µs)")
                .build(),
            cold_finalize_us: meter
                .f64_histogram("certus.pipeline.cold.finalize_us")
                .with_description("Dispatch-map re-registration after promote (µs)")
                .build(),
            hot_gpu_dma_us: meter
                .f64_histogram("certus.pipeline.hot.gpu_dma_us")
                .with_description("Hot-path memory-tier → GPU DMA + sync (µs)")
                .build(),
            populate_gpu_d2h_us: meter
                .f64_histogram("certus.pipeline.populate.gpu_d2h_us")
                .with_description("Populate GPU D2H DMA copy (µs)")
                .build(),
            populate_alloc_us: meter
                .f64_histogram("certus.pipeline.populate.alloc_us")
                .with_description("Populate memory-tier allocation + eviction (µs)")
                .build(),
            populate_total_us: meter
                .f64_histogram("certus.pipeline.populate.total_us")
                .with_description("Total populate operation (µs)")
                .build(),
        };

        Ok(Self {
            ops_total,
            ops_errors,
            op_duration_us,
            batch_size,
            entries_cleared,
            jobs_flushed,
            pipeline,
            _provider: Arc::new(provider),
        })
    }

    /// Record a completed operation.
    pub fn record_op(&self, op: &str, count: u64, errors: u64, duration_us: f64) {
        let attrs = [KeyValue::new("op", op.to_string())];
        self.ops_total.add(count, &attrs);
        if errors > 0 {
            self.ops_errors.add(errors, &attrs);
        }
        self.op_duration_us.record(duration_us, &attrs);
        self.batch_size.record(count, &attrs);
    }
}

impl dispatcher::PipelineMetrics for PipelineStageMetrics {
    fn record_cold_ssd_read(&self, drive: usize, duration_us: f64) {
        let attrs = [KeyValue::new("drive", drive as i64)];
        self.cold_ssd_read_us.record(duration_us, &attrs);
    }

    fn record_cold_gpu_dma(&self, duration_us: f64) {
        self.cold_gpu_dma_us.record(duration_us, &[]);
    }

    fn record_cold_stream_sync(&self, duration_us: f64) {
        self.cold_stream_sync_us.record(duration_us, &[]);
    }

    fn record_cold_total(&self, drive: usize, duration_us: f64) {
        let attrs = [KeyValue::new("drive", drive as i64)];
        self.cold_total_us.record(duration_us, &attrs);
    }

    fn record_cold_prep(&self, duration_us: f64) {
        self.cold_prep_us.record(duration_us, &[]);
    }

    fn record_cold_finalize(&self, duration_us: f64) {
        self.cold_finalize_us.record(duration_us, &[]);
    }

    fn record_hot_gpu_dma(&self, duration_us: f64) {
        self.hot_gpu_dma_us.record(duration_us, &[]);
    }

    fn record_populate_gpu_d2h(&self, duration_us: f64) {
        self.populate_gpu_d2h_us.record(duration_us, &[]);
    }

    fn record_populate_alloc(&self, duration_us: f64) {
        self.populate_alloc_us.record(duration_us, &[]);
    }

    fn record_populate_total(&self, duration_us: f64) {
        self.populate_total_us.record(duration_us, &[]);
    }
}
