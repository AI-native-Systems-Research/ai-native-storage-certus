//! The realised working-set size over `run.wss_window` (spec FR-034a, FR-009h).
//!
//! The distinct keys — and distinct bytes — a window of `run.wss_window`
//! **requests** touches. This is the number a consumer divides its own capacity
//! by, and publishing it is what lets a workload be stated without ever naming a
//! cache: "a quarter of the working set" is a claim the reader assembles from this
//! figure and its own hardware, not one the workload makes.
//!
//! # The window is a request count, not a duration
//!
//! FR-009h, and not a matter of taste. Under `closed_loop` arrival times depend on
//! the system's own response, so a time window is unknowable at plan time; under
//! `open_loop` it drifts whenever the schedule slips, which is the very thing
//! FR-061 exists to report. A count is exact in both cases, so the working-set
//! size stays a property of the plan rather than of the run that consumed it.
//!
//! # Tumbling windows, and what that costs
//!
//! Windows are consecutive and non-overlapping. A *sliding* window would need
//! every reference in the window resident — at the default of 240 000 requests and
//! a realistic path length that is tens of millions of references — so the
//! reported maximum is the maximum over tumbling windows, which is a **lower
//! bound** on the sliding-window maximum. The mean is unaffected by the choice.
//!
//! A run shorter than one window still gets a figure: the final partial window is
//! reported with the request count it actually reached and marked partial, because
//! a working-set size measured over 12 000 requests is useful as long as nobody
//! reads it as one measured over 240 000.

use serde::{Deserialize, Serialize};

use super::WindowTable;

/// Accumulates the realised working-set size.
#[derive(Debug, Default)]
pub struct WorkingSet {
    window_requests: u64,
    windows: Vec<WindowObservation>,
}

/// One window's realised working set.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowObservation {
    /// Requests the window actually contained.
    pub requests: u64,
    /// References the window contained.
    pub references: u64,
    /// Distinct keys touched.
    pub distinct_keys: u64,
    /// Summed entry size over those distinct keys.
    pub distinct_bytes: u128,
    /// Bytes referenced, counting repeats.
    pub referenced_bytes: u128,
    /// Whether the window closed short of `wss_window` requests.
    pub partial: bool,
}

impl WorkingSet {
    /// An accumulator for windows of `window_requests` requests.
    pub fn new(window_requests: u64) -> WorkingSet {
        WorkingSet {
            window_requests,
            windows: Vec::new(),
        }
    }

    /// Fold one closed window in.
    pub fn close_window(&mut self, window: &WindowTable) {
        if window.requests() == 0 {
            return;
        }
        self.windows.push(WindowObservation {
            requests: window.requests(),
            references: window.references(),
            distinct_keys: window.distinct_keys(),
            distinct_bytes: window.distinct_bytes(),
            referenced_bytes: window.bytes(),
            partial: window.requests() < self.window_requests,
        });
    }

    /// Freeze into the serialisable form.
    pub fn finish(self) -> WorkingSetReport {
        let full: Vec<&WindowObservation> = self.windows.iter().filter(|w| !w.partial).collect();
        // Summaries come from complete windows where there are any, because a
        // partial window's working set is smaller for a reason that has nothing to
        // do with the workload. Where there are none, the partial window is all
        // there is and is reported as such rather than withheld.
        let basis: Vec<&WindowObservation> = if full.is_empty() {
            self.windows.iter().collect()
        } else {
            full
        };
        let n = basis.len() as f64;
        let mean_keys = if basis.is_empty() {
            None
        } else {
            Some(basis.iter().map(|w| w.distinct_keys as f64).sum::<f64>() / n)
        };
        let mean_bytes = if basis.is_empty() {
            None
        } else {
            Some(basis.iter().map(|w| w.distinct_bytes as f64).sum::<f64>() / n)
        };
        WorkingSetReport {
            window_requests: self.window_requests,
            windows: self.windows.len() as u64,
            complete_windows: self.windows.iter().filter(|w| !w.partial).count() as u64,
            mean_distinct_keys: mean_keys,
            max_distinct_keys: basis.iter().map(|w| w.distinct_keys).max(),
            mean_distinct_bytes: mean_bytes,
            max_distinct_bytes: basis.iter().map(|w| w.distinct_bytes).max(),
            observations: self.windows,
        }
    }
}

/// The realised working-set size.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSetReport {
    /// The configured window, in requests.
    pub window_requests: u64,
    /// Windows observed, partial ones included.
    pub windows: u64,
    /// Windows that reached the full request count.
    pub complete_windows: u64,
    /// Mean distinct keys per window.
    pub mean_distinct_keys: Option<f64>,
    /// Largest distinct-key count in any window — a lower bound on the
    /// sliding-window maximum, since windows here do not overlap.
    pub max_distinct_keys: Option<u64>,
    /// Mean summed entry size over a window's distinct keys.
    pub mean_distinct_bytes: Option<f64>,
    /// Largest such total in any window.
    pub max_distinct_bytes: Option<u128>,
    /// Every window, in order.
    pub observations: Vec<WindowObservation>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};
    use crate::stats::Ref;

    fn window_of(keys: &[(u64, u32)], requests: u64) -> WindowTable {
        let mut w = WindowTable::new();
        let per = keys.len() as u64 / requests.max(1);
        for (i, (k, size)) in keys.iter().enumerate() {
            let r = Ref {
                key: CacheKey(*k),
                size: *size,
                depth: 0,
                session: SessionId(0),
                request_start: per == 0 || i as u64 % per == 0,
                warmup: false,
            };
            w.observe(&r);
            if per > 0 && (i as u64 + 1) % per == 0 {
                w.end_request();
            }
        }
        w.end_request();
        w
    }

    #[test]
    fn distinct_keys_and_bytes_count_each_key_once() {
        let mut ws = WorkingSet::new(4);
        let w = window_of(&[(1, 100), (1, 100), (2, 500), (2, 500)], 4);
        ws.close_window(&w);
        let r = ws.finish();
        assert_eq!(r.observations[0].distinct_keys, 2);
        assert_eq!(r.observations[0].distinct_bytes, 600);
        assert_eq!(
            r.observations[0].referenced_bytes, 1200,
            "referenced bytes count repeats; the working set does not"
        );
    }

    #[test]
    fn a_run_shorter_than_one_window_is_reported_and_marked_partial() {
        // Refusing to report would leave the worked example — twelve thousand
        // requests against a 240 000-request default — with no working-set figure
        // at all, which is worse than a labelled one.
        let mut ws = WorkingSet::new(240_000);
        ws.close_window(&window_of(&[(1, 8), (2, 8)], 2));
        let r = ws.finish();
        assert_eq!(r.windows, 1);
        assert_eq!(r.complete_windows, 0);
        assert!(r.observations[0].partial);
        assert_eq!(r.max_distinct_keys, Some(2));
    }

    #[test]
    fn summaries_ignore_a_trailing_partial_window_when_complete_ones_exist() {
        // Otherwise a run that ends mid-window would report a working set smaller
        // than the workload's, for a reason that is purely an artefact of where
        // the run stopped.
        let mut ws = WorkingSet::new(2);
        ws.close_window(&window_of(&[(1, 8), (2, 8)], 2));
        ws.close_window(&window_of(&[(3, 8), (4, 8)], 2));
        ws.close_window(&window_of(&[(5, 8)], 1));
        let r = ws.finish();
        assert_eq!(r.windows, 3);
        assert_eq!(r.complete_windows, 2);
        assert_eq!(r.mean_distinct_keys, Some(2.0));
        assert_eq!(r.max_distinct_keys, Some(2));
    }

    #[test]
    fn an_empty_window_is_not_an_observation() {
        let mut ws = WorkingSet::new(10);
        ws.close_window(&WindowTable::new());
        let r = ws.finish();
        assert_eq!(r.windows, 0);
        assert_eq!(r.mean_distinct_keys, None);
        assert_eq!(r.max_distinct_keys, None);
    }
}
