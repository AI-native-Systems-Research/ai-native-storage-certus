use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use interfaces::{IDispatcher, IMemoryTier, IRemoteLookup};
use opentelemetry::metrics::MeterProvider;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::Resource;

use crate::metrics::ServiceCounters;

pub struct OtelMetrics {
    _provider: Arc<SdkMeterProvider>,
}

impl OtelMetrics {
    pub fn init(
        endpoint: &str,
        service_name: &str,
        memory_tier: Arc<dyn IMemoryTier + Send + Sync>,
        dispatcher: Arc<dyn IDispatcher + Send + Sync>,
        remote_lookup: Arc<dyn IRemoteLookup + Send + Sync>,
        counters: ServiceCounters,
    ) -> Result<Self, String> {
        use opentelemetry_otlp::MetricExporter;
        use opentelemetry_sdk::metrics::PeriodicReader;
        use opentelemetry_sdk::runtime::Tokio;

        let exporter = MetricExporter::builder()
            .with_http()
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

        // Service-level counters (rates computed by collector)
        let c = counters.populates.clone();
        meter
            .u64_observable_counter("certus.populates_total")
            .with_description("Total successful populate operations")
            .with_callback(move |counter| {
                counter.observe(c.load(Ordering::Relaxed), &[]);
            })
            .build();

        let c = counters.evictions.clone();
        meter
            .u64_observable_counter("certus.evictions_total")
            .with_description("Total eviction events")
            .with_callback(move |counter| {
                counter.observe(c.load(Ordering::Relaxed), &[]);
            })
            .build();

        let c = counters.lookup_hits.clone();
        meter
            .u64_observable_counter("certus.lookup_hits_total")
            .with_description("Total successful lookup operations")
            .with_callback(move |counter| {
                counter.observe(c.load(Ordering::Relaxed), &[]);
            })
            .build();

        let c = counters.lookup_misses.clone();
        meter
            .u64_observable_counter("certus.lookup_misses_total")
            .with_description("Total lookup misses (key not found)")
            .with_callback(move |counter| {
                counter.observe(c.load(Ordering::Relaxed), &[]);
            })
            .build();

        let c = counters.gpu_bytes_transferred.clone();
        meter
            .u64_observable_counter("certus.gpu_bytes_transferred_total")
            .with_description("Total bytes transferred to GPU via lookup")
            .with_callback(move |counter| {
                counter.observe(c.load(Ordering::Relaxed), &[]);
            })
            .build();

        let c = counters.lookup_errors.clone();
        meter
            .u64_observable_counter("certus.lookup_errors_total")
            .with_description("Lookups neither served nor shown absent (FR-024)")
            .with_callback(move |counter| {
                counter.observe(c.load(std::sync::atomic::Ordering::Relaxed), &[]);
            })
            .build();

        // Remote-lookup attribution (spec 002 Phase 1). Requester-side: what this
        // node obtained FROM peers, not what peers asked OF it.
        //
        // These are the numbers that say whether forwarding a local miss to a peer
        // earns what it costs. `certus.lookup_hits_total` cannot answer that -- it
        // counts every served key without distinguishing the source, which is why
        // two cluster measurements established remote lookup's cost and neither
        // could establish its benefit.
        // The local half of the route partition. With remote_lookup_hits these three
        // sum to the served count, so a dashboard can show where hits come from rather
        // than only how many there were.
        let d = Arc::clone(&dispatcher);
        meter
            .u64_observable_counter("certus.lookup_hits_dram_total")
            .with_description("Lookups served from the local memory tier")
            .with_callback(move |counter| {
                counter.observe(d.tier_event_stats().lookup_hits_dram, &[]);
            })
            .build();

        let d = Arc::clone(&dispatcher);
        meter
            .u64_observable_counter("certus.lookup_hits_ssd_total")
            .with_description("Lookups served by reading a local data drive")
            .with_callback(move |counter| {
                counter.observe(d.tier_event_stats().lookup_hits_ssd, &[]);
            })
            .build();

        let d = Arc::clone(&dispatcher);
        meter
            .u64_observable_counter("certus.remote_lookup_hits_total")
            .with_description("Lookups a peer served, via remote lookup (requester side)")
            .with_callback(move |counter| {
                counter.observe(d.tier_event_stats().remote_lookup_hits, &[]);
            })
            .build();

        let d = Arc::clone(&dispatcher);
        meter
            .u64_observable_counter("certus.remote_lookup_misses_total")
            .with_description("Keys forwarded to a peer that no peer held")
            .with_callback(move |counter| {
                counter.observe(d.tier_event_stats().remote_lookup_misses, &[]);
            })
            .build();

        // Responder-side, the opposite direction from the pair above: what peers cause
        // THIS node to do. `peer_triggered_promotions` is the cold-serve signal -- how
        // much of this node's own disk work exists only because peers asked. It is the
        // instrument for the hypothesis that serving peers is what drives local store
        // declines, and it replaces a per-key REMOTE_SSD attribution that was withdrawn
        // for naming something it could not deliver (see dispatcher spec 002).
        let r = Arc::clone(&remote_lookup);
        meter
            .u64_observable_counter("certus.peer_served_keys_total")
            .with_description("Keys this node served to peers' remote lookups")
            .with_callback(move |counter| {
                counter.observe(r.serve_stats().peer_served_keys, &[]);
            })
            .build();

        let r = Arc::clone(&remote_lookup);
        meter
            .u64_observable_counter("certus.peer_triggered_promotions_total")
            .with_description("Keys this node read from its own disk to serve a peer")
            .with_callback(move |counter| {
                counter.observe(r.serve_stats().peer_triggered_promotions, &[]);
            })
            .build();

        // NVMe I/O counters
        let d = Arc::clone(&dispatcher);
        meter
            .u64_observable_counter("certus.nvme.read_bytes_total")
            .with_description("Total bytes read from NVMe drives")
            .with_callback(move |counter| {
                counter.observe(d.read_write_stats().read_bytes, &[]);
            })
            .build();

        let d = Arc::clone(&dispatcher);
        meter
            .u64_observable_counter("certus.nvme.write_bytes_total")
            .with_description("Total bytes written to NVMe drives")
            .with_callback(move |counter| {
                counter.observe(d.read_write_stats().write_bytes, &[]);
            })
            .build();

        let d = Arc::clone(&dispatcher);
        meter
            .u64_observable_counter("certus.nvme.read_ops_total")
            .with_description("Total NVMe read operations")
            .with_callback(move |counter| {
                counter.observe(d.read_write_stats().read_ops, &[]);
            })
            .build();

        let d = Arc::clone(&dispatcher);
        meter
            .u64_observable_counter("certus.nvme.write_ops_total")
            .with_description("Total NVMe write operations")
            .with_callback(move |counter| {
                counter.observe(d.read_write_stats().write_ops, &[]);
            })
            .build();

        Ok(Self {
            _provider: Arc::new(provider),
        })
    }
}
