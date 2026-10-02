use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use interfaces::{IDispatcher, IMemoryTier, IRemoteLookup};
use shmq_dispatcher::TranslatorObserver;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Cumulative per-op counters exposed to the metrics/telemetry layer.
/// All fields are monotonic; take deltas across two reads to compute rates.
/// Populated by [`CountersObserver`], which the shmq dispatcher calls on each op.
#[derive(Clone)]
pub struct ServiceCounters {
    pub populates: Arc<AtomicU64>,
    pub lookup_hits: Arc<AtomicU64>,
    pub lookup_misses: Arc<AtomicU64>,
    /// Lookups that could be neither served nor shown absent: a GPU handle that
    /// would not open, or a dispatcher error other than `KeyNotFound`.
    ///
    /// Exists so that hits + misses + errors accounts for every entry a client asked
    /// for (FR-024). Before it, both classes were dropped and the accounting was
    /// silently short -- which is how `lookup_misses` came to read 0 for so long
    /// without anyone noticing the totals did not add up.
    pub lookup_errors: Arc<AtomicU64>,
    pub evictions: Arc<AtomicU64>,
    pub gpu_bytes_transferred: Arc<AtomicU64>,
}

impl ServiceCounters {
    pub fn new() -> Self {
        Self {
            populates: Arc::new(AtomicU64::new(0)),
            lookup_hits: Arc::new(AtomicU64::new(0)),
            lookup_misses: Arc::new(AtomicU64::new(0)),
            lookup_errors: Arc::new(AtomicU64::new(0)),
            evictions: Arc::new(AtomicU64::new(0)),
            gpu_bytes_transferred: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// Bridges the transport-agnostic [`TranslatorObserver`] hook to
/// [`ServiceCounters`], so the shmq server keeps the same Prometheus/OTel
/// counters (`certus_populates_total`, `certus_lookup_hits_total`, …) that the
/// former gRPC service maintained. Installed via `Translator::with_observer`.
pub struct CountersObserver {
    counters: ServiceCounters,
}

impl CountersObserver {
    pub fn new(counters: ServiceCounters) -> Self {
        Self { counters }
    }
}

impl TranslatorObserver for CountersObserver {
    fn on_populate(&self, succeeded: u64) {
        self.counters
            .populates
            .fetch_add(succeeded, Ordering::Relaxed);
    }

    fn on_lookup(&self, hits: u64, misses: u64, errors: u64, gpu_bytes: u64) {
        self.counters.lookup_hits.fetch_add(hits, Ordering::Relaxed);
        self.counters
            .lookup_misses
            .fetch_add(misses, Ordering::Relaxed);
        self.counters
            .lookup_errors
            .fetch_add(errors, Ordering::Relaxed);
        self.counters
            .gpu_bytes_transferred
            .fetch_add(gpu_bytes, Ordering::Relaxed);
    }

    fn on_evictions(&self, count: u64) {
        self.counters.evictions.fetch_add(count, Ordering::Relaxed);
    }
}

pub async fn serve_metrics(
    port: u16,
    mt: Arc<dyn IMemoryTier + Send + Sync>,
    dispatcher: Arc<dyn IDispatcher + Send + Sync>,
    remote_lookup: Arc<dyn IRemoteLookup + Send + Sync>,
    counters: ServiceCounters,
) {
    let listener = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("metrics: failed to bind port {port}: {e}");
            return;
        }
    };
    eprintln!("metrics: listening on 0.0.0.0:{port}");

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(_) => continue,
        };

        let body = render_metrics(&*mt, &*dispatcher, &*remote_lookup, &counters);

        let (reader, mut writer) = stream.split();
        let mut buf_reader = BufReader::new(reader);
        let mut request_line = String::new();
        if buf_reader.read_line(&mut request_line).await.is_err() {
            continue;
        }

        let response = if request_line.starts_with("GET /metrics") {
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
        } else {
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        };

        let _ = writer.write_all(response.as_bytes()).await;
    }
}

fn render_metrics(
    mt: &dyn IMemoryTier,
    dispatcher: &dyn IDispatcher,
    remote_lookup: &dyn IRemoteLookup,
    counters: &ServiceCounters,
) -> String {
    let snap = mt.telemetry_snapshot();
    let used = mt.used();
    let free = mt.capacity().saturating_sub(used);
    let rw = dispatcher.read_write_stats();
    // Remote-lookup attribution (spec 002 Phase 1). Comes from the dispatcher's own
    // tier counters, not from `ServiceCounters`: the dispatcher is the only place that
    // knows, per key, whether a peer served it.
    let tier = dispatcher.tier_event_stats();
    // Responder-side, and the opposite direction from `tier.remote_lookup_*` above:
    // what peers caused THIS node to do. Kept adjacent so the naming contrast is
    // visible at the one place a reader compares them.
    let serve = remote_lookup.serve_stats();

    format!(
        "# HELP certus_memory_tier_write_lock_contentions_total Write-lock contention events\n\
         # TYPE certus_memory_tier_write_lock_contentions_total counter\n\
         certus_memory_tier_write_lock_contentions_total {}\n\
         # HELP certus_memory_tier_read_lock_contentions_total Read-lock contention events\n\
         # TYPE certus_memory_tier_read_lock_contentions_total counter\n\
         certus_memory_tier_read_lock_contentions_total {}\n\
         # HELP certus_memory_tier_used_bytes Bytes currently allocated\n\
         # TYPE certus_memory_tier_used_bytes gauge\n\
         certus_memory_tier_used_bytes {}\n\
         # HELP certus_memory_tier_free_bytes Bytes available for allocation\n\
         # TYPE certus_memory_tier_free_bytes gauge\n\
         certus_memory_tier_free_bytes {}\n\
         # HELP certus_populates_total Total successful populate operations\n\
         # TYPE certus_populates_total counter\n\
         certus_populates_total {}\n\
         # HELP certus_evictions_total Total eviction events\n\
         # TYPE certus_evictions_total counter\n\
         certus_evictions_total {}\n\
         # HELP certus_lookup_hits_total Total successful lookup operations\n\
         # TYPE certus_lookup_hits_total counter\n\
         certus_lookup_hits_total {}\n\
         # HELP certus_lookup_misses_total Total lookup misses (key not found)\n\
         # TYPE certus_lookup_misses_total counter\n\
         certus_lookup_misses_total {}\n\
         # HELP certus_lookup_errors_total Lookups neither served nor shown absent\n\
         # TYPE certus_lookup_errors_total counter\n\
         certus_lookup_errors_total {}\n\
         # HELP certus_lookup_hits_dram_total Lookups served from the local memory tier\n\
         # TYPE certus_lookup_hits_dram_total counter\n\
         certus_lookup_hits_dram_total {}\n\
         # HELP certus_lookup_hits_ssd_total Lookups served by reading a local data drive\n\
         # TYPE certus_lookup_hits_ssd_total counter\n\
         certus_lookup_hits_ssd_total {}\n\
         # HELP certus_remote_lookup_hits_total Lookups a peer served (requester side)\n\
         # TYPE certus_remote_lookup_hits_total counter\n\
         certus_remote_lookup_hits_total {}\n\
         # HELP certus_remote_lookup_misses_total Keys forwarded to a peer that no peer held\n\
         # TYPE certus_remote_lookup_misses_total counter\n\
         certus_remote_lookup_misses_total {}\n\
         # HELP certus_peer_served_keys_total Keys this node served to peers' remote lookups\n\
         # TYPE certus_peer_served_keys_total counter\n\
         certus_peer_served_keys_total {}\n\
         # HELP certus_peer_triggered_promotions_total Keys this node read from its own disk to serve a peer\n\
         # TYPE certus_peer_triggered_promotions_total counter\n\
         certus_peer_triggered_promotions_total {}\n\
         # HELP certus_gpu_bytes_transferred_total Total bytes transferred to GPU\n\
         # TYPE certus_gpu_bytes_transferred_total counter\n\
         certus_gpu_bytes_transferred_total {}\n\
         # HELP certus_nvme_read_bytes_total Total bytes read from NVMe\n\
         # TYPE certus_nvme_read_bytes_total counter\n\
         certus_nvme_read_bytes_total {}\n\
         # HELP certus_nvme_write_bytes_total Total bytes written to NVMe\n\
         # TYPE certus_nvme_write_bytes_total counter\n\
         certus_nvme_write_bytes_total {}\n\
         # HELP certus_nvme_read_ops_total Total NVMe read operations\n\
         # TYPE certus_nvme_read_ops_total counter\n\
         certus_nvme_read_ops_total {}\n\
         # HELP certus_nvme_write_ops_total Total NVMe write operations\n\
         # TYPE certus_nvme_write_ops_total counter\n\
         certus_nvme_write_ops_total {}\n\
         # HELP certus_evictions_blocked_by_pin_total Eviction candidates skipped because a read pin was held\n\
         # TYPE certus_evictions_blocked_by_pin_total counter\n\
         certus_evictions_blocked_by_pin_total {}\n\
         # HELP certus_evictions_blocked_unpersisted_total Eviction candidates skipped because write-through had not landed\n\
         # TYPE certus_evictions_blocked_unpersisted_total counter\n\
         certus_evictions_blocked_unpersisted_total {}\n\
         # HELP certus_eviction_scans_exhausted_total Clean-eviction scans that freed nothing, so the store had to backpressure\n\
         # TYPE certus_eviction_scans_exhausted_total counter\n\
         certus_eviction_scans_exhausted_total {}\n\
         # HELP certus_peer_pins_held Read pins the responder holds for peers right now (gauge, not monotonic)\n\
         # TYPE certus_peer_pins_held gauge\n\
         certus_peer_pins_held {}\n\
         # HELP certus_peer_pins_taken_total Keys ever pinned on behalf of peers\n\
         # TYPE certus_peer_pins_taken_total counter\n\
         certus_peer_pins_taken_total {}\n\
         # HELP certus_peer_pin_hold_us_total Summed lifetime of released responder pin batches, microseconds\n\
         # TYPE certus_peer_pin_hold_us_total counter\n\
         certus_peer_pin_hold_us_total {}\n\
         # HELP certus_peer_pin_hold_us_max Longest single responder pin-batch lifetime, microseconds\n\
         # TYPE certus_peer_pin_hold_us_max gauge\n\
         certus_peer_pin_hold_us_max {}\n\
         # HELP certus_oldest_sampled Oldest memory-tier keys sampled -- the window eviction scans\n\
         # TYPE certus_oldest_sampled gauge\n\
         certus_oldest_sampled {}\n\
         # HELP certus_oldest_persisted Of those, how many are demotable (have an ssd_offset)\n\
         # TYPE certus_oldest_persisted gauge\n\
         certus_oldest_persisted {}\n",
        snap.write_lock_contentions,
        snap.read_lock_contentions,
        used,
        free,
        counters.populates.load(Ordering::Relaxed),
        counters.evictions.load(Ordering::Relaxed),
        counters.lookup_hits.load(Ordering::Relaxed),
        counters.lookup_misses.load(Ordering::Relaxed),
        counters.lookup_errors.load(Ordering::Relaxed),
        tier.lookup_hits_dram,
        tier.lookup_hits_ssd,
        tier.remote_lookup_hits,
        tier.remote_lookup_misses,
        serve.peer_served_keys,
        serve.peer_triggered_promotions,
        counters.gpu_bytes_transferred.load(Ordering::Relaxed),
        rw.read_bytes,
        rw.write_bytes,
        rw.read_ops,
        rw.write_ops,
        tier.evictions_blocked_by_pin,
        tier.evictions_blocked_unpersisted,
        tier.eviction_scans_exhausted,
        serve.peer_pins_held,
        serve.peer_pins_taken,
        serve.peer_pin_hold_us_total,
        serve.peer_pin_hold_us_max,
        tier.oldest_sampled,
        tier.oldest_persisted,
    )
}
