//! Realised trunk width and occupancy per depth (spec FR-012, FR-034a).
//!
//! Trunk shape is **emergent**. A document states a *fanout*; the width it
//! produces, `w(0) = roots.count` and `w(d+1) = w(d) × fanout(d+1)`, is only
//! knowable after generating — which is exactly why FR-012 requires the report to
//! carry the realised value rather than let a reader assume the configured one.
//!
//! # Width
//!
//! `w(d) = distinct keys at depth d`, taken verbatim from how
//! `contracts/workload-schema.md` § Fitting measures `branching`. That definition
//! counts *every* key at a depth, private descents included, because a trace
//! cannot tell a shared node from a private one and a plan-side definition that
//! could would not be comparable with it.
//!
//! # Occupancy, and which way it is biased
//!
//! Occupancy is sessions per distinct path: `distinct (key, session) pairs at
//! depth d / distinct keys at depth d`, within one `run.wss_window`. It is the
//! quantity that decides whether configured sharing is realisable at all — when
//! occupancy at the sharing depth falls below 1, sessions land on virgin trunk and
//! realised sharing collapses far below the drawn `shared_depth` while the
//! configuration still looks entirely reasonable.
//!
//! The measured value is a **lower bound** on trunk occupancy, and the direction
//! is worth being explicit about: private paths have occupancy exactly 1 and sit
//! in the denominator, so a depth that is mostly private reads near 1 however
//! well-occupied its trunk is. The stream carries no trunk/private label — a
//! single-session key could be either a private descent or an unoccupied trunk
//! path — so no honest classification is available, and inventing one would put a
//! guess where FR-012 asks for a measurement. `shared_key_fraction` is published
//! alongside so a reader can see how much of a depth the bound is averaging over.
//!
//! # Why per window and not per run
//!
//! FR-009h: the window is a request count, and it is part of the definition.
//! Occupancy counted over a whole run would let a configuration "achieve" sharing
//! by running longer, which is not a physical effect. Windows here are
//! consecutive and non-overlapping, and each depth's occupancy is **pooled** over
//! the windows that contained it — summed numerator over summed denominator, so a
//! window holding more of that depth's keys counts for more, rather than a mean of
//! per-window ratios in which a window holding one key would count as much as one
//! holding ten thousand.

use serde::{Deserialize, Serialize};

use super::{KeyTable, WindowTable};

/// Accumulates realised width and occupancy per depth.
#[derive(Debug, Default)]
pub struct Trunk {
    /// Per depth: summed per-window occupancy numerator, denominator, and the
    /// number of windows that saw the depth at all.
    windows: Vec<DepthAccum>,
    window_count: u64,
}

#[derive(Debug, Default, Clone, Copy)]
struct DepthAccum {
    session_pairs: u64,
    keys: u64,
    shared_keys: u64,
    windows: u64,
}

impl Trunk {
    /// An empty accumulator.
    pub fn new() -> Trunk {
        Trunk::default()
    }

    /// Fold one closed window in.
    pub fn close_window(&mut self, window: &WindowTable) {
        if window.requests() == 0 {
            return;
        }
        self.window_count += 1;
        let mut seen_depths: Vec<bool> = Vec::new();
        for (depth, sessions) in window.by_depth() {
            let d = depth as usize;
            if self.windows.len() <= d {
                self.windows.resize(d + 1, DepthAccum::default());
            }
            if seen_depths.len() <= d {
                seen_depths.resize(d + 1, false);
            }
            let a = &mut self.windows[d];
            a.keys += 1;
            a.session_pairs += u64::from(sessions);
            if sessions >= 2 {
                a.shared_keys += 1;
            }
            seen_depths[d] = true;
        }
        for (d, seen) in seen_depths.iter().enumerate() {
            if *seen {
                self.windows[d].windows += 1;
            }
        }
    }

    /// Freeze into the serialisable form, taking run-wide width from `keys`.
    ///
    /// Two widths are published because they answer different questions: the
    /// windowed one is the denominator of occupancy, and the run-wide one is the
    /// `branching` profile a `fit` would recover.
    pub fn finish(self, keys: &KeyTable) -> TrunkReport {
        let mut run_wide: Vec<(u64, u64)> = Vec::new();
        for (_, depth, _, shared) in keys.iter() {
            let d = depth as usize;
            if run_wide.len() <= d {
                run_wide.resize(d + 1, (0, 0));
            }
            run_wide[d].0 += 1;
            if shared {
                run_wide[d].1 += 1;
            }
        }

        let depth_count = self.windows.len().max(run_wide.len());
        let mut depths = Vec::with_capacity(depth_count);
        for d in 0..depth_count {
            let w = self.windows.get(d).copied().unwrap_or_default();
            let (run_keys, run_shared) = run_wide.get(d).copied().unwrap_or((0, 0));
            depths.push(DepthReport {
                depth: d as u32,
                width_run: run_keys,
                shared_keys_run: run_shared,
                width_window_mean: if w.windows == 0 {
                    None
                } else {
                    Some(w.keys as f64 / w.windows as f64)
                },
                occupancy: if w.keys == 0 {
                    None
                } else {
                    Some(w.session_pairs as f64 / w.keys as f64)
                },
                shared_key_fraction: if w.keys == 0 {
                    None
                } else {
                    Some(w.shared_keys as f64 / w.keys as f64)
                },
                windows: w.windows,
            });
        }

        // The realised fanout profile: the ratio of successive run-wide widths.
        // Reported rather than the configured fanout, per FR-012.
        let realised_fanout = depths
            .windows(2)
            .map(|p| {
                if p[0].width_run == 0 {
                    None
                } else {
                    Some(p[1].width_run as f64 / p[0].width_run as f64)
                }
            })
            .collect();

        TrunkReport {
            windows: self.window_count,
            depths,
            realised_fanout,
        }
    }
}

/// Realised width and occupancy at one depth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthReport {
    /// The depth described.
    pub depth: u32,
    /// Distinct keys at this depth over the whole run — the `branching` profile
    /// a `fit` recovers, private descents included.
    pub width_run: u64,
    /// Distinct keys at this depth that at least two sessions referenced. A
    /// plan-side observation with no trace-side counterpart; not part of the fit
    /// profile.
    pub shared_keys_run: u64,
    /// Mean distinct keys at this depth per window.
    pub width_window_mean: Option<f64>,
    /// Sessions per distinct key at this depth, pooled across windows. A **lower
    /// bound** on trunk occupancy: private paths sit in the denominator at 1.
    pub occupancy: Option<f64>,
    /// Fraction of this depth's windowed keys touched by two or more sessions,
    /// so a reader can see what the occupancy bound is averaging over.
    pub shared_key_fraction: Option<f64>,
    /// Windows that contained this depth at all.
    pub windows: u64,
}

/// Realised trunk shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrunkReport {
    /// Windows folded in.
    pub windows: u64,
    /// One entry per depth, ascending from 0.
    pub depths: Vec<DepthReport>,
    /// `width_run(d+1) / width_run(d)`, the realised fanout into each depth.
    pub realised_fanout: Vec<Option<f64>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};
    use crate::stats::Ref;

    /// Build a key table and a window from requests given as (session, path).
    fn feed(requests: &[(u32, &[u64])]) -> TrunkReport {
        let mut keys = KeyTable::new();
        let mut w = WindowTable::new();
        let mut t = Trunk::new();
        for (session, path) in requests {
            for (i, k) in path.iter().enumerate() {
                let r = Ref {
                    key: CacheKey(*k),
                    size: 8,
                    depth: i as u32,
                    session: SessionId(*session),
                    request_start: i == 0,
                    warmup: false,
                };
                keys.observe(&r);
                w.observe(&r);
            }
            w.end_request();
        }
        t.close_window(&w);
        t.finish(&keys)
    }

    #[test]
    fn width_counts_distinct_keys_at_each_depth_private_descents_included() {
        // Two sessions sharing a root and diverging: depth 0 has one key, depth 1
        // has two. The fit definition counts both children whether or not either
        // is shared.
        let r = feed(&[(1, &[1, 10]), (2, &[1, 20])]);
        assert_eq!(r.depths[0].width_run, 1);
        assert_eq!(r.depths[1].width_run, 2);
        assert_eq!(r.realised_fanout[0], Some(2.0));
    }

    #[test]
    fn occupancy_is_sessions_per_distinct_key() {
        // Four sessions on one root: occupancy 4 at depth 0.
        let r = feed(&[(1, &[1]), (2, &[1]), (3, &[1]), (4, &[1])]);
        assert_eq!(r.depths[0].occupancy, Some(4.0));
        assert_eq!(r.depths[0].shared_key_fraction, Some(1.0));
    }

    #[test]
    fn one_session_visiting_a_key_repeatedly_does_not_raise_occupancy() {
        // Occupancy counts distinct sessions, not references — otherwise a chatty
        // session would look like a crowd.
        let r = feed(&[(1, &[1]), (1, &[1]), (1, &[1])]);
        assert_eq!(r.depths[0].occupancy, Some(1.0));
        assert_eq!(r.depths[0].shared_keys_run, 0);
    }

    #[test]
    fn private_paths_drag_occupancy_toward_one_and_the_fraction_shows_it() {
        // The documented bias, pinned: depth 1 holds one shared key at occupancy
        // 2 and eight private ones at 1, so the mean is well under the trunk's
        // own occupancy and `shared_key_fraction` is what reveals that.
        let mut reqs: Vec<(u32, Vec<u64>)> = vec![(1, vec![1, 100]), (2, vec![1, 100])];
        for s in 3..11u32 {
            reqs.push((s, vec![1, 1000 + u64::from(s)]));
        }
        let borrowed: Vec<(u32, &[u64])> = reqs.iter().map(|(s, p)| (*s, p.as_slice())).collect();
        let r = feed(&borrowed);
        let d1 = &r.depths[1];
        assert_eq!(d1.width_run, 9);
        assert_eq!(d1.shared_keys_run, 1);
        let occ = d1.occupancy.unwrap();
        assert!(
            occ > 1.0 && occ < 1.2,
            "occupancy {occ} should sit just above 1"
        );
        assert!((d1.shared_key_fraction.unwrap() - 1.0 / 9.0).abs() < 1e-12);
    }

    #[test]
    fn a_depth_no_window_reached_reports_absence_rather_than_zero_occupancy() {
        let r = feed(&[(1, &[1])]);
        assert_eq!(r.depths.len(), 1);
        assert_eq!(r.depths[0].windows, 1);
        // Nothing at depth 1 at all: the vector simply ends rather than carrying
        // a 0 that would read as "no sessions there".
        assert!(r.depths.get(1).is_none());
    }

    #[test]
    fn occupancy_is_a_mean_over_windows_not_over_the_run() {
        // Two windows, each with two sessions on the same root. Counted over the
        // run the root would show four sessions; counted per window it shows two,
        // which is the physical claim.
        let mut keys = KeyTable::new();
        let mut w = WindowTable::new();
        let mut t = Trunk::new();
        let push = |keys: &mut KeyTable, w: &mut WindowTable, session: u32| {
            let r = Ref {
                key: CacheKey(1),
                size: 8,
                depth: 0,
                session: SessionId(session),
                request_start: true,
                warmup: false,
            };
            keys.observe(&r);
            w.observe(&r);
            w.end_request();
        };
        push(&mut keys, &mut w, 1);
        push(&mut keys, &mut w, 2);
        t.close_window(&w);
        w.reset();
        push(&mut keys, &mut w, 3);
        push(&mut keys, &mut w, 4);
        t.close_window(&w);
        let r = t.finish(&keys);
        assert_eq!(r.windows, 2);
        assert_eq!(r.depths[0].occupancy, Some(2.0));
        assert_eq!(r.depths[0].width_run, 1);
    }
}
