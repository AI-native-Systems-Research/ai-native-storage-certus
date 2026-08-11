//! The request-length distribution (spec FR-034a).
//!
//! Blocks per request, and bytes per request. Realised, so it carries what
//! `private_depth`, `growth_per_turn` and `shared_depth` actually produced rather
//! than what they were configured to produce — a session's path depth is a sum of
//! draws across its turns, so the length distribution is emergent even when every
//! input to it is stated exactly.
//!
//! Length matters twice over. It is the divisor between "requests per second" and
//! "blocks per second", so a throughput figure is uninterpretable without it; and
//! it is what makes a byte hit rate carry no information beyond the object hit rate
//! at constant `block_bytes` (FR-040), since then bytes are just blocks times a
//! constant.

use serde::{Deserialize, Serialize};

use super::hist::{Hist, Quantiles};
use super::Ref;

/// Accumulates the request-length distribution.
#[derive(Debug, Default)]
pub struct RequestLength {
    blocks: Hist,
    bytes: Hist,
    open_blocks: u64,
    open_bytes: u64,
}

impl RequestLength {
    /// An empty accumulator.
    pub fn new() -> RequestLength {
        RequestLength::default()
    }

    /// Record one measured reference.
    pub fn observe(&mut self, r: &Ref) {
        if r.request_start {
            self.flush();
        }
        self.open_blocks += 1;
        self.open_bytes += u64::from(r.size);
    }

    /// Close the open request. Idempotent, and implied by the next
    /// `request_start`.
    pub fn end_request(&mut self) {
        self.flush();
    }

    fn flush(&mut self) {
        if self.open_blocks > 0 {
            self.blocks.add(self.open_blocks);
            self.bytes.add(self.open_bytes);
        }
        self.open_blocks = 0;
        self.open_bytes = 0;
    }

    /// Requests closed so far.
    pub fn requests(&self) -> u64 {
        self.blocks.count()
    }

    /// Freeze into the serialisable form.
    pub fn finish(mut self) -> LengthReport {
        self.blocks.seal();
        self.bytes.seal();
        LengthReport {
            requests: self.blocks.count(),
            blocks: self.blocks.summary(),
            block_buckets: self.blocks.buckets(),
            bytes: self.bytes.summary(),
        }
    }
}

/// The request-length distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LengthReport {
    /// Measured requests.
    pub requests: u64,
    /// Blocks per request.
    pub blocks: Quantiles,
    /// The block-length histogram as `(lower, upper, count)`.
    pub block_buckets: Vec<(u64, u64, u64)>,
    /// Bytes per request.
    pub bytes: Quantiles,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};

    fn feed(requests: &[&[u32]]) -> LengthReport {
        let mut l = RequestLength::new();
        for req in requests {
            for (i, size) in req.iter().enumerate() {
                l.observe(&Ref {
                    key: CacheKey(i as u64),
                    size: *size,
                    depth: i as u32,
                    session: SessionId(0),
                    request_start: i == 0,
                    warmup: false,
                });
            }
        }
        l.end_request();
        l.finish()
    }

    #[test]
    fn length_is_blocks_per_request_and_bytes_follows_the_sizes() {
        let r = feed(&[&[100, 100, 100], &[100]]);
        assert_eq!(r.requests, 2);
        assert_eq!(r.blocks.max, Some(3));
        assert_eq!(r.blocks.min, Some(1));
        assert_eq!(r.blocks.mean, Some(2.0));
        assert_eq!(r.bytes.max, Some(300));
    }

    #[test]
    fn a_request_start_closes_the_previous_request_without_an_explicit_end() {
        // The plan format guarantees request contiguity, so a start is an end.
        let mut l = RequestLength::new();
        for (i, start) in [true, false, true, false, false].iter().enumerate() {
            l.observe(&Ref {
                key: CacheKey(i as u64),
                size: 1,
                depth: 0,
                session: SessionId(0),
                request_start: *start,
                warmup: false,
            });
        }
        assert_eq!(l.requests(), 1, "the second request is still open");
        l.end_request();
        assert_eq!(l.requests(), 2);
    }

    #[test]
    fn ending_twice_does_not_invent_an_empty_request() {
        let mut l = RequestLength::new();
        l.observe(&Ref {
            key: CacheKey(0),
            size: 1,
            depth: 0,
            session: SessionId(0),
            request_start: true,
            warmup: false,
        });
        l.end_request();
        l.end_request();
        assert_eq!(l.requests(), 1);
    }

    #[test]
    fn nothing_measured_reports_absence_rather_than_a_zero_length() {
        let r = feed(&[]);
        assert_eq!(r.requests, 0);
        assert_eq!(r.blocks.mean, None);
        assert_eq!(r.bytes.max, None);
    }

    #[test]
    fn bytes_per_request_is_blocks_times_the_constant_when_sizes_are_constant() {
        // FR-040's premise, pinned: at constant block_bytes the byte distribution
        // carries no information the block distribution does not.
        let r = feed(&[&[128; 5], &[128; 9], &[128; 2]]);
        let blocks = r.blocks.mean.unwrap();
        let bytes = r.bytes.mean.unwrap();
        assert!((bytes - blocks * 128.0).abs() < 1e-9);
    }
}
