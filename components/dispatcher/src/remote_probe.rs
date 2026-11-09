//! Opt-in timing probe for the remote-lookup delivery path.
//!
//! Splits a remote batch's wall-clock into the three stages that matter for
//! deciding where the next round of optimization belongs:
//!
//! - **fetch** — `IRemoteLookup::batch_lookup`, i.e. everything from the zyre
//!   request through the peer's RDMA write landing in local DRAM.
//! - **lookup** — the per-key `IDispatchMap::lookup` calls, which is where the
//!   dispatch-map mutex and (via `ep.touch`) the LRU pool mutex are paid.
//! - **h2d** — `serve_memory_tier_to_gpu`: the gpu-services state mutex plus the
//!   `cudaMemcpyAsync` launch.
//! - **sync** — the single batched `stream_synchronize` that waits for those
//!   copies to complete.
//!
//! Without this split a flat throughput number is uninterpretable: it cannot
//! distinguish "delivery was never the bottleneck" from "the change did not
//! take". `PipelineMetrics` cannot answer it either — nothing calls
//! `set_pipeline_metrics` in the `certus-server-yaml` binary the multi-node
//! benchmark actually runs, so those histograms are dead there.
//!
//! Splitting `lookup` from `h2d` is what separates a lock-contention story from
//! a CUDA-launch-overhead one. They are the same order of magnitude a priori —
//! six mutex acquisitions per key versus one `cudaMemcpyAsync` launch — and only
//! one of them is fixed by sharding a lock.
//!
//! **Read the percentages, not the per-key microseconds.** The per-key figures
//! are a sum over concurrently-executing batches, so with N requests in flight
//! they overstate real per-key latency by roughly N (measured: 192-221 µs/key
//! reported against 11.55 µs/key actual at 4 workers x 4 inflight). The ratios
//! are unaffected, which is why the line labels them as summed.
//!
//! Cost when enabled is five `Instant::now()` calls and a few relaxed atomic
//! adds per *batch* (not per key), against a batch that takes on the order of a
//! millisecond. Off by default; set `CERTUS_LOG_REMOTE_DELIVERY` to a truthy
//! value (`1`/`true`/`yes`/`on`) to turn it on. Output goes out at `warn` level
//! because the benchmark harness runs the servers at `RUST_LOG=warn` and raising
//! that to `info` distorts the measurement it is trying to take.

use std::sync::atomic::{AtomicU64, Ordering};

/// Batches per emitted summary. Aggregating keeps the log off the hot path
/// while still giving many samples over a benchmark run.
const REPORT_EVERY: u64 = 256;

static BATCHES: AtomicU64 = AtomicU64::new(0);
static KEYS: AtomicU64 = AtomicU64::new(0);
static FETCH_US: AtomicU64 = AtomicU64::new(0);
static LOOKUP_US: AtomicU64 = AtomicU64::new(0);
static H2D_US: AtomicU64 = AtomicU64::new(0);
static SYNC_US: AtomicU64 = AtomicU64::new(0);

/// Whether the probe is switched on. Read from the environment once.
pub(crate) fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("CERTUS_LOG_REMOTE_DELIVERY")
            .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "on"))
            .unwrap_or(false)
    })
}

/// Accumulate one remote batch, returning a summary line every
/// [`REPORT_EVERY`] batches.
///
/// Counters are process-global rather than per-component: the dispatcher is a
/// singleton in every profile that binds remote-lookup, and keeping them out of
/// the `define_component!` field list avoids touching its positional
/// constructor at every call site for a diagnostic.
pub(crate) fn record(
    keys: u64,
    fetch_us: u64,
    lookup_us: u64,
    h2d_us: u64,
    sync_us: u64,
) -> Option<String> {
    KEYS.fetch_add(keys, Ordering::Relaxed);
    FETCH_US.fetch_add(fetch_us, Ordering::Relaxed);
    LOOKUP_US.fetch_add(lookup_us, Ordering::Relaxed);
    H2D_US.fetch_add(h2d_us, Ordering::Relaxed);
    SYNC_US.fetch_add(sync_us, Ordering::Relaxed);

    if BATCHES.fetch_add(1, Ordering::Relaxed) + 1 < REPORT_EVERY {
        return None;
    }

    // Take the window's totals and start the next one. Batches completing on
    // other threads can land either side of these swaps, so the window edges
    // are approximate — irrelevant for the stage *ratio*, which is the only
    // thing this is used to read.
    let batches = BATCHES.swap(0, Ordering::Relaxed);
    let keys = KEYS.swap(0, Ordering::Relaxed);
    let fetch = FETCH_US.swap(0, Ordering::Relaxed);
    let lookup = LOOKUP_US.swap(0, Ordering::Relaxed);
    let h2d = H2D_US.swap(0, Ordering::Relaxed);
    let sync = SYNC_US.swap(0, Ordering::Relaxed);

    let total = (fetch + lookup + h2d + sync) as f64;
    let share = |v: u64| {
        if total == 0.0 {
            0.0
        } else {
            100.0 * v as f64 / total
        }
    };
    let per_key = |v: u64| {
        if keys == 0 {
            0.0
        } else {
            v as f64 / keys as f64
        }
    };

    Some(format!(
        "remote-delivery: {batches} batches / {keys} keys | \
         fetch {:.1}% lookup {:.1}% h2d {:.1}% sync {:.1}% | \
         summed-per-key (concurrent batches overlap, ratios only): \
         fetch {:.1}us lookup {:.1}us h2d {:.1}us sync {:.1}us total {:.1}us",
        share(fetch),
        share(lookup),
        share(h2d),
        share(sync),
        per_key(fetch),
        per_key(lookup),
        per_key(h2d),
        per_key(sync),
        per_key(fetch + lookup + h2d + sync),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing is emitted until a full window has accumulated, and the summary
    /// then reports the window's key count and stage shares.
    #[test]
    fn reports_once_per_window() {
        for i in 1..REPORT_EVERY {
            assert!(
                record(2, 100, 40, 20, 40).is_none(),
                "batch {i} should not emit yet"
            );
        }
        let line = record(2, 100, 40, 20, 40).expect("window boundary should emit");
        assert!(line.contains(&format!("{REPORT_EVERY} batches")), "{line}");
        assert!(
            line.contains(&format!("{} keys", REPORT_EVERY * 2)),
            "{line}"
        );
        // Per batch: fetch 100, lookup 40, h2d 20, sync 40 => total 200.
        assert!(line.contains("fetch 50.0%"), "{line}");
        assert!(line.contains("lookup 20.0%"), "{line}");
        assert!(line.contains("h2d 10.0%"), "{line}");
        assert!(line.contains("sync 20.0%"), "{line}");
        // 100us of fetch over 2 keys per batch => 50us summed-per-key.
        assert!(line.contains("fetch 50.0us"), "{line}");
        // Counters reset, so the next window starts over.
        assert!(record(1, 1, 1, 1, 1).is_none());
    }
}
