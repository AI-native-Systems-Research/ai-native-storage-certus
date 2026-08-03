//! Opt-in timing probe for the remote-lookup delivery path.
//!
//! Splits a remote batch's wall-clock into the three stages that matter for
//! deciding where the next round of optimization belongs:
//!
//! - **fetch** — `IRemoteLookup::batch_lookup`, i.e. everything from the zyre
//!   request through the peer's RDMA write landing in local DRAM.
//! - **submit** — the dispatcher's own loop: per-key dispatch-map lookup plus
//!   the asynchronous H2D copy submissions.
//! - **sync** — the single batched `stream_synchronize` that waits for those
//!   copies to complete.
//!
//! Without this split a flat throughput number is uninterpretable: it cannot
//! distinguish "delivery was never the bottleneck" from "the change did not
//! take". `PipelineMetrics` cannot answer it either — nothing calls
//! `set_pipeline_metrics` in the `certus-server-yaml` binary the multi-node
//! benchmark actually runs, so those histograms are dead there.
//!
//! Cost when enabled is four `Instant::now()` calls and a few relaxed atomic
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
static SUBMIT_US: AtomicU64 = AtomicU64::new(0);
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
pub(crate) fn record(keys: u64, fetch_us: u64, submit_us: u64, sync_us: u64) -> Option<String> {
    KEYS.fetch_add(keys, Ordering::Relaxed);
    FETCH_US.fetch_add(fetch_us, Ordering::Relaxed);
    SUBMIT_US.fetch_add(submit_us, Ordering::Relaxed);
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
    let submit = SUBMIT_US.swap(0, Ordering::Relaxed);
    let sync = SYNC_US.swap(0, Ordering::Relaxed);

    let total = (fetch + submit + sync) as f64;
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
         fetch {:.1}% submit {:.1}% sync {:.1}% | \
         per key: fetch {:.1}us submit {:.1}us sync {:.1}us total {:.1}us",
        share(fetch),
        share(submit),
        share(sync),
        per_key(fetch),
        per_key(submit),
        per_key(sync),
        per_key(fetch + submit + sync),
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
                record(2, 100, 10, 40).is_none(),
                "batch {i} should not emit yet"
            );
        }
        let line = record(2, 100, 10, 40).expect("window boundary should emit");
        assert!(line.contains(&format!("{REPORT_EVERY} batches")), "{line}");
        assert!(
            line.contains(&format!("{} keys", REPORT_EVERY * 2)),
            "{line}"
        );
        // fetch 100 of 150 total per batch => ~66.7%, and 50us per key.
        assert!(line.contains("fetch 66.7%"), "{line}");
        assert!(line.contains("fetch 50.0us"), "{line}");
        // Counters reset, so the next window starts over.
        assert!(record(1, 1, 1, 1).is_none());
    }
}
