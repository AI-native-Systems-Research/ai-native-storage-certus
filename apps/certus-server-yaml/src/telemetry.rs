use std::sync::Arc;
use std::time::Duration;

use interfaces::IMemoryTier;
use opentelemetry::metrics::MeterProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::Resource;

pub struct OtelMetrics {
    _provider: Arc<SdkMeterProvider>,
}

impl OtelMetrics {
    pub fn init(
        endpoint: &str,
        service_name: &str,
        memory_tier: Arc<dyn IMemoryTier + Send + Sync>,
    ) -> Result<Self, String> {
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

        let meter = provider.meter("certus-server-yaml");

        // Memory-tier observable gauges
        let mt = Arc::clone(&memory_tier);
        meter
            .u64_observable_gauge("certus.memory_tier.capacity_bytes")
            .with_description("Total memory-tier pool capacity in bytes")
            .with_callback(move |gauge| {
                gauge.observe(mt.capacity() as u64, &[]);
            })
            .build();

        let mt = Arc::clone(&memory_tier);
        meter
            .u64_observable_gauge("certus.memory_tier.used_bytes")
            .with_description("Bytes currently allocated in memory-tier")
            .with_callback(move |gauge| {
                gauge.observe(mt.used() as u64, &[]);
            })
            .build();

        let mt = Arc::clone(&memory_tier);
        meter
            .u64_observable_gauge("certus.memory_tier.free_bytes")
            .with_description("Bytes available for allocation in memory-tier")
            .with_callback(move |gauge| {
                let cap = mt.capacity();
                let used = mt.used();
                gauge.observe(cap.saturating_sub(used) as u64, &[]);
            })
            .build();

        // Memory-tier telemetry counters as observable counters
        let mt = Arc::clone(&memory_tier);
        meter
            .u64_observable_counter("certus.memory_tier.evictions_total")
            .with_description("Total LRU evictions from memory-tier")
            .with_callback(move |counter| {
                counter.observe(mt.telemetry_snapshot().evictions, &[]);
            })
            .build();

        let mt = Arc::clone(&memory_tier);
        meter
            .u64_observable_counter("certus.memory_tier.write_lock_contentions_total")
            .with_description("Write-lock contention events on memory-tier pool")
            .with_callback(move |counter| {
                counter.observe(mt.telemetry_snapshot().write_lock_contentions, &[]);
            })
            .build();

        let mt = Arc::clone(&memory_tier);
        meter
            .u64_observable_counter("certus.memory_tier.read_lock_contentions_total")
            .with_description("Read-lock contention events on memory-tier pool")
            .with_callback(move |counter| {
                counter.observe(mt.telemetry_snapshot().read_lock_contentions, &[]);
            })
            .build();

        Ok(Self {
            _provider: Arc::new(provider),
        })
    }
}
