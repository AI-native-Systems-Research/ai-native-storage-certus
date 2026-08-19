use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use interfaces::{IDispatcher, IMemoryTier};
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
    pub evictions: Arc<AtomicU64>,
    pub gpu_bytes_transferred: Arc<AtomicU64>,
}

impl ServiceCounters {
    pub fn new() -> Self {
        Self {
            populates: Arc::new(AtomicU64::new(0)),
            lookup_hits: Arc::new(AtomicU64::new(0)),
            lookup_misses: Arc::new(AtomicU64::new(0)),
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

    fn on_lookup(&self, hits: u64, misses: u64, gpu_bytes: u64) {
        self.counters.lookup_hits.fetch_add(hits, Ordering::Relaxed);
        self.counters
            .lookup_misses
            .fetch_add(misses, Ordering::Relaxed);
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

        let body = render_metrics(&*mt, &*dispatcher, &counters);

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
    counters: &ServiceCounters,
) -> String {
    let snap = mt.telemetry_snapshot();
    let used = mt.used();
    let free = mt.capacity().saturating_sub(used);
    let rw = dispatcher.read_write_stats();

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
         certus_nvme_write_ops_total {}\n",
        snap.write_lock_contentions,
        snap.read_lock_contentions,
        used,
        free,
        counters.populates.load(Ordering::Relaxed),
        counters.evictions.load(Ordering::Relaxed),
        counters.lookup_hits.load(Ordering::Relaxed),
        counters.lookup_misses.load(Ordering::Relaxed),
        counters.gpu_bytes_transferred.load(Ordering::Relaxed),
        rw.read_bytes,
        rw.write_bytes,
        rw.read_ops,
        rw.write_ops,
    )
}
