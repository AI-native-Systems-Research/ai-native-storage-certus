use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use interfaces::{IDispatcher, IMemoryTier, IRemoteLookup};
use shmq_dispatcher::{FetchStats, Fetcher, TranslatorObserver};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Cumulative per-op counters exposed to the metrics/telemetry layer.
/// All fields are monotonic; take deltas across two reads to compute rates.
/// Populated by [`CountersObserver`], which the shmq dispatcher calls on each op.
#[derive(Clone)]
pub(crate) struct ServiceCounters {
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
    pub(crate) fn new() -> Self {
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
pub(crate) struct CountersObserver {
    counters: ServiceCounters,
}

impl CountersObserver {
    pub(crate) fn new(counters: ServiceCounters) -> Self {
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

pub(crate) async fn serve_metrics(
    port: u16,
    mt: Arc<dyn IMemoryTier + Send + Sync>,
    dispatcher: Arc<dyn IDispatcher + Send + Sync>,
    remote_lookup: Arc<dyn IRemoteLookup + Send + Sync>,
    counters: ServiceCounters,
    fetcher: Option<Fetcher>,
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

        let fetch = fetcher.as_ref().map(Fetcher::stats);
        let body = render_metrics(&*mt, &*dispatcher, &*remote_lookup, &counters, fetch);

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
    fetch: Option<FetchStats>,
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
    let dram = dram_hits(tier.lookup_hits_dram, fetch.as_ref());

    let mut body = format!(
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
         # HELP certus_peer_pin_hold_us_max Longest single responder pin-batch lifetime, microseconds\n\
         # TYPE certus_peer_pin_hold_us_max gauge\n\
         certus_peer_pin_hold_us_max {}\n\
         # HELP certus_oldest_sampled Oldest memory-tier keys sampled -- the window eviction scans\n\
         # TYPE certus_oldest_sampled gauge\n\
         certus_oldest_sampled {}\n\
         # HELP certus_oldest_persisted Of those, how many are demotable (have an ssd_offset)\n\
         # TYPE certus_oldest_persisted gauge\n\
         certus_oldest_persisted {}\n\
         # HELP certus_store_backpressure_events_total Reserve retries while the memory tier was momentarily full\n\
         # TYPE certus_store_backpressure_events_total counter\n\
         certus_store_backpressure_events_total {}\n\
         # HELP certus_store_drops_on_full_total Stores dropped after the backpressure budget elapsed\n\
         # TYPE certus_store_drops_on_full_total counter\n\
         certus_store_drops_on_full_total {}\n\
         # HELP certus_store_already_resident_total Reserves refused because the key was already cached, not for want of space\n\
         # TYPE certus_store_already_resident_total counter\n\
         certus_store_already_resident_total {}\n",
        snap.write_lock_contentions,
        snap.read_lock_contentions,
        used,
        free,
        counters.populates.load(Ordering::Relaxed),
        counters.evictions.load(Ordering::Relaxed),
        counters.lookup_hits.load(Ordering::Relaxed),
        counters.lookup_misses.load(Ordering::Relaxed),
        counters.lookup_errors.load(Ordering::Relaxed),
        dram,
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
        serve.peer_pin_hold_us_max,
        tier.oldest_sampled,
        tier.oldest_persisted,
        tier.store_backpressure_events,
        tier.store_drops_on_full,
        tier.store_already_resident,
    );
    if let Some(f) = fetch {
        body.push_str(&render_fetch(&f));
    }
    body
}

/// Local memory-tier hits, net of fetched blocks' first serves.
///
/// The dispatcher sees a fetched block as an ordinary memory-tier entry and counts
/// its first serve as `Dram`; the translator then re-attributes exactly that serve as
/// `Fetched` (it only ever re-marks a `Dram`). Subtracting keeps the route partition
/// `dram + ssd + fetched + remote = lookup_hits`, so a fetch never reads as a local
/// hit (dispatcher spec 002 `contracts/served-by.md`).
#[must_use]
pub(crate) fn dram_hits(dispatcher_dram: u64, fetch: Option<&FetchStats>) -> u64 {
    dispatcher_dram.saturating_sub(fetch.map_or(0, |f| f.lookup_hits_fetched))
}

/// The shm-queue `FETCH` counters (`contracts/shmq-fetch.md`). Rendered only by a
/// server that fetches, so a profile without a transfer endpoint shows no fetch
/// series at all rather than zeros that look like an idle, working path.
fn render_fetch(f: &FetchStats) -> String {
    let rows: [(&str, &str, &str, u64); 14] = [
        (
            "lookup_hits_fetched_total",
            "counter",
            "Lookups that served a block fetched from a peer, its first serve",
            f.lookup_hits_fetched,
        ),
        (
            "fetch_requests_total",
            "counter",
            "FETCH ops received",
            f.requests,
        ),
        (
            "fetch_keys_requested_total",
            "counter",
            "Keys FETCH ops named",
            f.keys_requested,
        ),
        (
            "fetch_keys_started_total",
            "counter",
            "Keys accepted into a fetch run",
            f.keys_started,
        ),
        (
            "fetch_not_started_total",
            "counter",
            "Keys refused at once: already resident or pending",
            f.not_started,
        ),
        (
            "fetch_hits_total",
            "counter",
            "Blocks published from a peer",
            f.hits,
        ),
        (
            "fetch_bytes_total",
            "counter",
            "Bytes of blocks published from a peer",
            f.bytes,
        ),
        (
            "fetch_misses_total",
            "counter",
            "Fetched keys the peer does not hold",
            f.misses,
        ),
        (
            "fetch_failed_total",
            "counter",
            "Fetched keys lost to a fault: transport, deadline, refusal",
            f.failed,
        ),
        (
            "fetch_truncated_total",
            "counter",
            "Started keys never fetched: after a miss or a refused reservation",
            f.truncated,
        ),
        (
            "fetch_window_waits_total",
            "counter",
            "Fetch windows the global in-flight cap held back",
            f.window_waits,
        ),
        (
            "fetched_unused_total",
            "counter",
            "Fetched blocks evicted or aged out before any lookup served them",
            f.fetched_unused,
        ),
        (
            "fetch_keys_pending",
            "gauge",
            "Keys in a fetch run now",
            f.keys_pending,
        ),
        (
            "fetch_marks_live",
            "gauge",
            "Fetched blocks not yet served, still marked",
            f.marks_live,
        ),
    ];
    let mut out = String::new();
    for (name, kind, help, v) in rows {
        let _ = write!(
            out,
            "# HELP certus_{name} {help}\n# TYPE certus_{name} {kind}\ncertus_{name} {v}\n"
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fetched block's first serve is the dispatcher's `Dram` re-attributed: it
    /// must leave the DRAM count, or every remote fetch reads as a local hit.
    /// Measured on hardware before this: 739 fetched serves, all in DRAM.
    #[test]
    fn fetched_serves_leave_the_dram_count() {
        let f = FetchStats {
            lookup_hits_fetched: 739,
            ..Default::default()
        };
        assert_eq!(dram_hits(1000, Some(&f)), 261);
        assert_eq!(dram_hits(739, Some(&f)), 0);
        assert_eq!(dram_hits(1000, None), 1000);
    }

    /// Every counter the contract names is exported, under its contract name.
    #[test]
    fn every_fetch_counter_is_rendered() {
        let f = FetchStats {
            requests: 1,
            keys_requested: 2,
            keys_started: 3,
            not_started: 4,
            hits: 5,
            bytes: 6,
            misses: 7,
            failed: 8,
            truncated: 9,
            window_waits: 10,
            lookup_hits_fetched: 11,
            fetched_unused: 12,
            keys_pending: 13,
            marks_live: 14,
        };
        let text = render_fetch(&f);
        for line in [
            "certus_fetch_requests_total 1",
            "certus_fetch_keys_requested_total 2",
            "certus_fetch_keys_started_total 3",
            "certus_fetch_not_started_total 4",
            "certus_fetch_hits_total 5",
            "certus_fetch_bytes_total 6",
            "certus_fetch_misses_total 7",
            "certus_fetch_failed_total 8",
            "certus_fetch_truncated_total 9",
            "certus_fetch_window_waits_total 10",
            "certus_lookup_hits_fetched_total 11",
            "certus_fetched_unused_total 12",
            "certus_fetch_keys_pending 13",
            "certus_fetch_marks_live 14",
        ] {
            assert!(
                text.lines().any(|l| l == line),
                "missing {line:?} in\n{text}"
            );
        }
        assert_eq!(text.matches("# TYPE").count(), 14);
    }
}
