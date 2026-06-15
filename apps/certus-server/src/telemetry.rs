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
    _provider: Arc<SdkMeterProvider>,
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

        Ok(Self {
            ops_total,
            ops_errors,
            op_duration_us,
            batch_size,
            entries_cleared,
            jobs_flushed,
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
