//! The realised prefix-sharing depth histogram (spec FR-012a, FR-034a).
//!
//! # The definition is borrowed, deliberately
//!
//! A request's realised shared depth is *the length of the longest prefix of its
//! block list that also prefixes an earlier request within the same
//! `run.wss_window`* — which is, word for word, how `contracts/workload-schema.md`
//! § Fitting measures `shared_depth` from a real trace. Taking the fit
//! definition rather than inventing a plan-side one is what makes FR-057's
//! plan-versus-trace divergence a comparison of two measurements instead of two
//! definitions.
//!
//! With one qualification the fit table leaves implicit, because it measures
//! `private_depth` from turn-1 requests where the question does not arise: the
//! earlier request must belong to a **different session**. `shared_depth` is
//! defined as the depth at which *inter*-session sharing ends, and a session's own
//! later turns re-walk its whole path by construction (FR-014a) — so counting them
//! would report every multi-turn session's turn 2 as sharing its entire turn-1
//! prefix. Measured that way on the worked example the realised depth came out at
//! 796 against a configured p99 of 40: the statistic had stopped measuring sharing
//! and started measuring conversation length.
//!
//! Because a key's identity is a hash of the path to it, a matching key at depth
//! `d` implies the whole prefix above it matches too (`keys` § divergence is
//! irreversible). So the longest common prefix is simply the leading run of keys
//! already seen in the window, and the measurement is one pass with no
//! comparisons against stored paths.
//!
//! # Intended is not realised, and the gap is the finding
//!
//! `corpus.trees.shared_depth` states what a session *attempts*: it leaves the
//! trunk at depth *s*. Whether those *s* levels are shared depends on whether any
//! earlier session walked the same *s* steps, so the drawn value is an **upper
//! bound** and trunk occupancy decides whether the bound is tight. FR-012a
//! therefore requires both to be reported, as two statistics, never one — and
//! where they diverge the divergence is the result, not an error.
//!
//! This module reports only the realised side. The intended distribution is a
//! property of the document, and [`Report`](super::Report) carries it alongside.

use serde::{Deserialize, Serialize};

use super::hist::{Hist, Quantiles};
use super::{Ref, WindowTable};

/// Accumulates the realised prefix-sharing depth histogram.
#[derive(Debug, Default)]
pub struct Sharing {
    depth: Hist,
    /// Requests whose first block was already novel — no sharing at all.
    ///
    /// Held apart from the histogram's zero bucket on purpose: "shares nothing"
    /// and "shares the root and no more" are different workloads, and a single
    /// count of zeroes would conflate them.
    unshared_requests: u64,
    requests: u64,
    /// Whether the open request can still extend its common prefix. Cleared by
    /// the first novel key, because divergence is irreversible.
    prefix_open: bool,
    prefix_len: u64,
    refs_in_request: u64,
    /// The prefix length of the last request closed, for `fit`.
    last_prefix_len: u64,
}

impl Sharing {
    /// An empty accumulator.
    pub fn new() -> Sharing {
        Sharing {
            prefix_open: true,
            ..Sharing::default()
        }
    }

    /// Record one measured reference against the window it falls in.
    ///
    /// `window` must not yet contain the open request — that is what makes
    /// "already seen" mean "seen in an earlier request".
    pub fn observe(&mut self, r: &Ref, window: &WindowTable) {
        if r.request_start {
            self.flush();
        }
        self.refs_in_request += 1;
        if self.prefix_open {
            if window.seen_in_earlier_request_by_other(r.key, r.session) {
                self.prefix_len += 1;
            } else {
                self.prefix_open = false;
            }
        }
    }

    /// Close the open request, recording its shared depth.
    ///
    /// Idempotent, and implied by the next `request_start`, so a caller that
    /// only ever drives `observe` still gets every request but the last.
    pub fn end_request(&mut self) {
        self.flush();
    }

    /// The shared prefix length of the most recently closed request.
    ///
    /// Exposed so a `fit` can compute `private_depth` — the contract defines it as
    /// "turn-1 path depth − that longest common prefix" — without reimplementing the
    /// longest-common-prefix rule. Two implementations of it would put the fitted
    /// `private_depth` and the validated `shared_depth` on different definitions,
    /// which is the drift FR-021i exists to prevent.
    pub fn last_prefix_len(&self) -> u64 {
        self.last_prefix_len
    }

    fn flush(&mut self) {
        if self.refs_in_request > 0 {
            self.last_prefix_len = self.prefix_len;
            self.requests += 1;
            if self.prefix_len == 0 {
                self.unshared_requests += 1;
            } else {
                // The prefix **length**, which is what `shared_depth` is: FR-014a
                // makes a path of depth n occupy ordinals 0..n, so a document
                // asking for `shared_depth: 4` and a request realising four shared
                // leading blocks both read as 4. Reporting the deepest shared
                // *ordinal* instead would put a systematic -1 between the intended
                // and realised statistics that FR-012a asks a reader to compare.
                self.depth.add(self.prefix_len);
            }
        }
        self.refs_in_request = 0;
        self.prefix_len = 0;
        self.prefix_open = true;
    }

    /// Freeze into the serialisable form.
    pub fn finish(mut self) -> SharingReport {
        self.depth.seal();
        let sharing_requests = self.depth.count();
        SharingReport {
            requests: self.requests,
            sharing_requests,
            unshared_requests: self.unshared_requests,
            shared_fraction: if self.requests == 0 {
                None
            } else {
                Some(sharing_requests as f64 / self.requests as f64)
            },
            realised_depth: self.depth.summary(),
            depth_buckets: self.depth.buckets(),
        }
    }
}

/// The realised prefix-sharing depth histogram.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharingReport {
    /// Measured requests.
    pub requests: u64,
    /// Requests that shared at least their first block.
    pub sharing_requests: u64,
    /// Requests whose very first block was novel in the window.
    pub unshared_requests: u64,
    /// Sharing requests as a fraction of all of them.
    pub shared_fraction: Option<f64>,
    /// The realised shared depth — a prefix **length** in blocks, on the same
    /// scale as the configured `shared_depth`. Over sharing requests only, so its
    /// support starts at 1; requests that shared nothing are
    /// `unshared_requests`.
    pub realised_depth: Quantiles,
    /// The histogram as `(lower, upper, count)`, ascending.
    pub depth_buckets: Vec<(u64, u64, u64)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};

    /// Feed requests as key lists through a window, as the report does.
    ///
    /// Each request gets its own session, so these exercise the inter-session
    /// sharing the statistic is about. `same_session` covers the other case.
    fn feed(requests: &[&[u64]]) -> SharingReport {
        let mut w = WindowTable::new();
        let mut s = Sharing::new();
        for (req_index, req) in requests.iter().enumerate() {
            for (i, k) in req.iter().enumerate() {
                let r = Ref {
                    key: CacheKey(*k),
                    size: 8,
                    depth: i as u32,
                    session: SessionId(req_index as u32),
                    request_start: i == 0,
                    warmup: false,
                };
                s.observe(&r, &w);
                w.observe(&r);
            }
            s.end_request();
            w.end_request();
        }
        s.finish()
    }

    #[test]
    fn the_first_request_shares_nothing_because_nothing_preceded_it() {
        let r = feed(&[&[1, 2, 3]]);
        assert_eq!(r.requests, 1);
        assert_eq!(r.unshared_requests, 1);
        assert_eq!(r.sharing_requests, 0);
        assert_eq!(r.shared_fraction, Some(0.0));
    }

    #[test]
    fn a_repeated_path_shares_to_its_full_depth() {
        let r = feed(&[&[1, 2, 3], &[1, 2, 3]]);
        assert_eq!(r.sharing_requests, 1);
        assert_eq!(r.realised_depth.max, Some(3), "three shared blocks");
    }

    #[test]
    fn sharing_stops_at_the_first_divergence_and_does_not_resume() {
        // Divergence is irreversible, so a later coincidental match must not
        // extend the common prefix. If it did, realised sharing would exceed
        // what the key model can produce.
        let r = feed(&[&[1, 2, 3, 4], &[1, 2, 99, 4]]);
        assert_eq!(r.realised_depth.max, Some(2), "two blocks, then divergence");
    }

    #[test]
    fn a_request_sharing_only_its_root_is_not_the_same_as_one_sharing_nothing() {
        // The conflation FR-012a's separate counts exist to prevent: a length of 0
        // is not a bucket, it is `unshared_requests`.
        let r = feed(&[&[1, 2], &[1, 9], &[7, 8]]);
        assert_eq!(r.sharing_requests, 1);
        assert_eq!(
            r.unshared_requests, 2,
            "the first request and the novel one"
        );
        assert_eq!(
            r.realised_depth.max,
            Some(1),
            "sharing one block is sharing; the histogram's support starts at 1"
        );
    }

    #[test]
    fn a_window_reset_makes_earlier_requests_invisible_again() {
        // The window is part of the definition: counted over a whole run, a
        // configuration could "achieve" sharing merely by running longer.
        let mut w = WindowTable::new();
        let mut s = Sharing::new();
        let push = |s: &mut Sharing, w: &mut WindowTable, session: u32, keys: &[u64]| {
            for (i, k) in keys.iter().enumerate() {
                let r = Ref {
                    key: CacheKey(*k),
                    size: 8,
                    depth: i as u32,
                    session: SessionId(session),
                    request_start: i == 0,
                    warmup: false,
                };
                s.observe(&r, w);
                w.observe(&r);
            }
            s.end_request();
            w.end_request();
        };
        push(&mut s, &mut w, 1, &[1, 2]);
        w.reset();
        push(&mut s, &mut w, 2, &[1, 2]);
        let r = s.finish();
        assert_eq!(r.sharing_requests, 0, "the window forgot the first request");
        assert_eq!(r.unshared_requests, 2);
    }

    #[test]
    fn a_sessions_own_later_turns_are_not_sharing() {
        // The bug this pins: a session's turn 2 re-walks its whole turn-1 path, so
        // counting same-session prefixes made the realised depth a measure of
        // conversation length rather than of inter-session sharing.
        let mut w = WindowTable::new();
        let mut s = Sharing::new();
        for turn in 0..3 {
            let path: Vec<u64> = (0..10 + turn).collect();
            for (i, k) in path.iter().enumerate() {
                let r = Ref {
                    key: CacheKey(*k),
                    size: 8,
                    depth: i as u32,
                    session: SessionId(7),
                    request_start: i == 0,
                    warmup: false,
                };
                s.observe(&r, &w);
                w.observe(&r);
            }
            s.end_request();
            w.end_request();
        }
        let r = s.finish();
        assert_eq!(r.requests, 3);
        assert_eq!(
            r.unshared_requests, 3,
            "one session's three turns share nothing with anyone else"
        );
        assert_eq!(r.sharing_requests, 0);
    }

    #[test]
    fn every_request_is_counted_exactly_once() {
        let r = feed(&[&[1], &[1], &[1], &[2, 3], &[2, 3]]);
        assert_eq!(r.requests, 5);
        assert_eq!(r.sharing_requests + r.unshared_requests, r.requests);
    }
}
