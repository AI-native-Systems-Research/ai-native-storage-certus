//! The human rendering of a plan report (spec FR-048).
//!
//! In the library rather than in the binary because `certus-workload report` and
//! `certus-trace validate` both print these statistics, and two renderers would
//! give the same quantity two names — which is the same drift FR-021i keeps the
//! statistics themselves out of the binaries to avoid.
//!
//! Three conventions the layout is built on:
//!
//! - An absent value prints `--`, never `0`. FR-012's failure mode is a configured
//!   or defaulted number read as a measured one, and a zero standing in for "not
//!   measured" is exactly that.
//! - Every quantity says whether it is **realised** or **intended**, because
//!   FR-012a forbids presenting one as the other.
//! - Byte counts print a human scale and the exact count, since one is for reading
//!   and the other for arithmetic.

use std::fmt::Write as _;

use super::hist::Quantiles;
use super::report::Report;

/// Depths printed in full before the table elides its middle.
const DEPTH_HEAD: usize = 24;
/// Depths printed at the tail of an elided table.
const DEPTH_TAIL: usize = 8;

/// A byte count as a human scale and the exact figure.
fn bytes(n: u128) -> String {
    const UNITS: [(&str, u128); 5] = [
        ("PiB", 1 << 50),
        ("TiB", 1 << 40),
        ("GiB", 1 << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
    ];
    for (name, scale) in UNITS {
        if n >= scale {
            return format!("{:.2} {name} ({n})", n as f64 / scale as f64);
        }
    }
    format!("{n} B")
}

/// An optional count, or `--`.
fn opt_u64(v: Option<u64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "--".into())
}

/// An optional float at `p` decimals, or `--`.
fn opt_f64(v: Option<f64>, p: usize) -> String {
    v.map(|v| format!("{v:.p$}")).unwrap_or_else(|| "--".into())
}

/// A percentage, or `--`.
fn opt_pct(v: Option<f64>) -> String {
    v.map(|v| format!("{:.2}%", v * 100.0))
        .unwrap_or_else(|| "--".into())
}

/// One quantile row.
fn quantiles(label: &str, q: &Quantiles) -> String {
    format!(
        "  {label:<14} p50 {:>12}  p90 {:>12}  p99 {:>12}  p99.9 {:>12}  max {:>12}  mean {:>14}\n",
        opt_u64(q.p50),
        opt_u64(q.p90),
        opt_u64(q.p99),
        opt_u64(q.p999),
        opt_u64(q.max),
        opt_f64(q.mean, 2),
    )
}

impl Report {
    /// The human summary.
    ///
    /// Renders every FR-034a statistic. The JSON form ([`Report::to_json`]) carries
    /// the full CDF buckets and per-depth table; this view elides the middle of a
    /// deep trunk table, and says so where it does.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        self.write_provenance(&mut s);
        self.write_reuse_distance(&mut s);
        self.write_floor(&mut s);
        self.write_sharing(&mut s);
        self.write_length(&mut s);
        self.write_unique(&mut s);
        self.write_trunk(&mut s);
        self.write_working_set(&mut s);
        self.write_warnings(&mut s);
        s
    }

    fn write_provenance(&self, s: &mut String) {
        let _ = writeln!(s, "plan");
        for (label, value) in [
            ("content hash", &self.provenance.content_hash),
            ("parameter hash", &self.provenance.parameter_hash),
            ("stream digest", &self.provenance.stream_digest),
        ] {
            if let Some(v) = value {
                let _ = writeln!(s, "  {label:<14} {v}");
            }
        }
        let _ = writeln!(
            s,
            "  {:<14} {} requests, {} references, {}",
            "measured",
            self.requests,
            self.references,
            bytes(self.total_bytes)
        );
        let _ = writeln!(
            s,
            "  {:<14} {} distinct keys, {} to hold all of them",
            "key space",
            self.distinct_keys,
            bytes(self.distinct_bytes)
        );
        // FR-045: warmup is counted, and counted apart.
        let _ = writeln!(
            s,
            "  {:<14} {} requests, {} references, {} distinct keys (excluded from every \
             statistic below, but they primed the stream: a key warmup fetched is not a \
             compulsory miss)",
            "warmup", self.warmup.requests, self.warmup.references, self.warmup.distinct_keys
        );
        s.push('\n');
    }

    fn write_reuse_distance(&self, s: &mut String) {
        let r = &self.reuse_distance;
        let _ = writeln!(
            s,
            "reuse distance (realised; the primary statistic — read any capacity point off it)"
        );
        s.push_str(&quantiles("objects", &r.objects));
        s.push_str(&quantiles("bytes", &r.bytes));
        let _ = writeln!(
            s,
            "  {:<14} {} of {} references ({}) have no finite distance",
            "first touches",
            r.first_touches,
            r.references,
            opt_pct(if r.references == 0 {
                None
            } else {
                Some(r.first_touches as f64 / r.references as f64)
            })
        );
        s.push('\n');
    }

    fn write_floor(&self, s: &mut String) {
        let f = &self.floor;
        let _ = writeln!(s, "compulsory-miss floor (realised)");
        let _ = writeln!(
            s,
            "  {:<14} {}  ({} of {} references)",
            "per object",
            opt_f64(f.per_object, 4),
            f.compulsory_misses,
            f.references
        );
        let _ = writeln!(
            s,
            "  {:<14} {}  ({} of {})",
            "per byte",
            opt_f64(f.per_byte, 4),
            bytes(f.compulsory_bytes),
            bytes(f.bytes)
        );
        let _ = writeln!(
            s,
            "  {:<14} the miss rate at unbounded capacity: no capacity and no replacement \
             policy can improve on it",
            "meaning"
        );
        s.push('\n');
    }

    fn write_sharing(&self, s: &mut String) {
        let sh = &self.sharing;
        let _ = writeln!(s, "prefix sharing");
        match &self.intended_shared_depth {
            Some(i) => {
                let _ = writeln!(
                    s,
                    "  {:<14} {}  p50 {}  p90 {}  p99 {}  mean {}",
                    "intended",
                    i.shape,
                    opt_f64(i.p50, 1),
                    opt_f64(i.p90, 1),
                    opt_f64(i.p99, 1),
                    opt_f64(i.mean, 1)
                );
            }
            None => {
                let _ = writeln!(s, "  {:<14} --  (no document supplied)", "intended");
            }
        }
        s.push_str(&quantiles("realised", &sh.realised_depth));
        let _ = writeln!(
            s,
            "  {:<14} {} of {} requests shared a prefix ({}); {} shared nothing at all",
            "coverage",
            sh.sharing_requests,
            sh.requests,
            opt_pct(sh.shared_fraction),
            sh.unshared_requests
        );
        let _ = writeln!(
            s,
            "  {:<14} intended is an upper bound on realised — trunk occupancy decides whether \
             the bound is tight, so where the two diverge the divergence is the finding",
            "reading"
        );
        s.push('\n');
    }

    fn write_length(&self, s: &mut String) {
        let l = &self.request_length;
        let _ = writeln!(s, "request length (realised, over {} requests)", l.requests);
        s.push_str(&quantiles("blocks", &l.blocks));
        s.push_str(&quantiles("bytes", &l.bytes));
        s.push('\n');
    }

    fn write_unique(&self, s: &mut String) {
        let u = &self.unique_keys;
        let _ = writeln!(s, "unique keys over time (realised)");
        let _ = writeln!(
            s,
            "  {:<14} {} keys, {}",
            "distinct",
            u.distinct_keys,
            bytes(u.distinct_bytes)
        );
        // Read against the floor rather than against a threshold: linear growth
        // in distinct keys is expected here, so the useful comparison is whether
        // the tail is opening up faster than the run as a whole.
        let verdict = match (u.tail_novelty_per_reference, self.floor.per_object) {
            (None, _) => "too few requests to say".to_string(),
            (Some(v), _) if v < 0.001 => "a closed key space: the run saw all of it".to_string(),
            (Some(v), Some(f)) if v > f * 1.2 => {
                format!("above the floor of {f:.4}: the key space was still opening up")
            }
            (Some(_), Some(f)) => format!("in line with the floor of {f:.4}"),
            (Some(_), None) => "no floor to compare against".to_string(),
        };
        let _ = writeln!(
            s,
            "  {:<14} {} new keys per reference over the run's second half — {verdict}",
            "tail novelty",
            opt_f64(u.tail_novelty_per_reference, 4)
        );
        let _ = writeln!(
            s,
            "  {:<14} {} points, log-spaced (full curve in the JSON form)",
            "curve",
            u.points.len()
        );
        s.push('\n');
    }

    fn write_trunk(&self, s: &mut String) {
        let t = &self.trunk;
        let _ = writeln!(
            s,
            "trunk (realised, per depth; occupancy over {} window(s))",
            t.windows
        );
        let _ = writeln!(
            s,
            "  {:>5}  {:>12}  {:>12}  {:>12}  {:>10}  {:>10}",
            "depth", "width(run)", "shared(run)", "width(win)", "occupancy", "fanout"
        );
        let n = t.depths.len();
        let elide = n > DEPTH_HEAD + DEPTH_TAIL + 1;
        for (i, d) in t.depths.iter().enumerate() {
            if elide && i == DEPTH_HEAD {
                let _ = writeln!(
                    s,
                    "  {:>5}  ({} depths elided; the JSON form carries all of them)",
                    "...",
                    n - DEPTH_HEAD - DEPTH_TAIL
                );
            }
            if elide && i >= DEPTH_HEAD && i < n - DEPTH_TAIL {
                continue;
            }
            let fanout = t
                .realised_fanout
                .get(i)
                .copied()
                .flatten()
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "--".into());
            let _ = writeln!(
                s,
                "  {:>5}  {:>12}  {:>12}  {:>12}  {:>10}  {:>10}",
                d.depth,
                d.width_run,
                d.shared_keys_run,
                opt_f64(d.width_window_mean, 1),
                opt_f64(d.occupancy, 2),
                fanout
            );
        }
        let _ = writeln!(
            s,
            "  occupancy is a lower bound: private descents have occupancy 1 and sit in the \
             denominator, and the stream carries no trunk/private label to exclude them by"
        );
        s.push('\n');
    }

    fn write_working_set(&self, s: &mut String) {
        let w = &self.working_set;
        let _ = writeln!(
            s,
            "working set (realised, window = {} requests)",
            w.window_requests
        );
        let _ = writeln!(
            s,
            "  {:<14} {} observed, {} complete",
            "windows", w.windows, w.complete_windows
        );
        let _ = writeln!(
            s,
            "  {:<14} mean {}  max {}",
            "distinct keys",
            opt_f64(w.mean_distinct_keys, 1),
            opt_u64(w.max_distinct_keys)
        );
        let _ = writeln!(
            s,
            "  {:<14} mean {}  max {}",
            "distinct bytes",
            w.mean_distinct_bytes
                .map(|v| bytes(v as u128))
                .unwrap_or_else(|| "--".into()),
            w.max_distinct_bytes
                .map(bytes)
                .unwrap_or_else(|| "--".into())
        );
        let _ = writeln!(
            s,
            "  {:<14} windows do not overlap, so the maximum is a lower bound on the \
             sliding-window maximum",
            "note"
        );
        s.push('\n');
    }

    fn write_warnings(&self, s: &mut String) {
        if self.warnings.is_empty() {
            let _ = writeln!(s, "warnings: none");
            return;
        }
        let _ = writeln!(s, "warnings");
        for w in &self.warnings {
            let _ = writeln!(s, "  [{}] {}", w.requirement, w.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::Dist;
    use crate::keys::{CacheKey, SessionId};
    use crate::stats::report::Provenance;
    use crate::stats::{Ref, Statistics};

    fn sample(window: u64, requests: &[(u32, Vec<u64>)]) -> Report {
        let mut st = Statistics::new(window);
        for (session, path) in requests {
            for (i, k) in path.iter().enumerate() {
                st.push(&Ref {
                    key: CacheKey(*k),
                    size: 131_072,
                    depth: i as u32,
                    session: SessionId(*session),
                    request_start: i == 0,
                    warmup: false,
                });
            }
        }
        st.finish()
    }

    /// A workload with sharing, a bounded key space and plenty of re-reads —
    /// which is to say one that raises no warning, so that a test asserting the
    /// absence of warnings is testing the renderer rather than the fixture.
    fn shared_workload() -> Report {
        let reqs: Vec<(u32, Vec<u64>)> = (0..400u32)
            .map(|i| {
                (
                    i % 20,
                    vec![1, 2, u64::from(i % 5) + 10, u64::from(i % 40) + 100],
                )
            })
            .collect();
        sample(50, &reqs)
    }

    #[test]
    fn every_statistic_appears_in_the_human_form() {
        // FR-034a's enumeration is normative, so the rendering's coverage of it is
        // asserted rather than eyeballed.
        let text = shared_workload().to_text();
        for section in [
            "reuse distance",
            "compulsory-miss floor",
            "prefix sharing",
            "request length",
            "unique keys over time",
            "trunk",
            "working set",
            "distinct keys",
        ] {
            assert!(text.contains(section), "missing section {section}:\n{text}");
        }
    }

    #[test]
    fn an_absent_value_prints_as_absent_rather_than_as_zero() {
        // A 0 where a measurement is missing is FR-012's failure: a value that was
        // never realised, read as one that was.
        let empty = Statistics::new(10).finish();
        let text = empty.to_text();
        assert!(text.contains("--"), "no absence marker:\n{text}");
        assert!(
            !text.contains("0.0000"),
            "an unmeasured floor printed as a number:\n{text}"
        );
    }

    #[test]
    fn intended_and_realised_are_labelled_as_such() {
        // FR-012a: the configured value must never read as the measured one.
        let text = shared_workload()
            .with_intended_shared_depth(&Dist::Scalar(2.0))
            .to_text();
        assert!(text.contains("intended"));
        assert!(text.contains("realised"));
        assert!(text.contains("upper bound on realised"));
    }

    #[test]
    fn a_missing_document_says_so_instead_of_implying_a_configured_depth() {
        let text = shared_workload().to_text();
        assert!(text.contains("no document supplied"), "{text}");
    }

    #[test]
    fn provenance_is_printed_when_present_and_omitted_when_not() {
        let text = shared_workload().to_text();
        assert!(!text.contains("content hash"));
        let with = shared_workload()
            .with_provenance(Provenance {
                content_hash: Some("blake3:abcdef".into()),
                parameter_hash: None,
                stream_digest: Some("blake3:123456".into()),
                normalised_yaml: Some("version: 1\n".into()),
            })
            .to_text();
        assert!(with.contains("blake3:abcdef"));
        assert!(with.contains("blake3:123456"));
        assert!(!with.contains("parameter hash"), "absent, so not a row");
    }

    #[test]
    fn a_deep_trunk_table_elides_its_middle_and_says_how_much() {
        let long: Vec<u64> = (0..200u64).collect();
        let text = sample(4, &[(1, long.clone()), (2, long)]).to_text();
        assert!(text.contains("depths elided"), "{text}");
        assert!(text.contains("carries all of them"));
        // Head and tail are both present.
        assert!(text.contains("\n      0  "));
        assert!(text.contains("\n    199  "));
    }

    #[test]
    fn a_short_trunk_table_is_printed_in_full() {
        let text = sample(4, &[(1, vec![1, 2, 3]), (2, vec![1, 2, 4])]).to_text();
        assert!(!text.contains("elided"), "{text}");
    }

    #[test]
    fn byte_counts_carry_both_a_scale_and_the_exact_figure() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(2048), "2.00 KiB (2048)");
        assert_eq!(bytes(1 << 30), "1.00 GiB (1073741824)");
    }

    #[test]
    fn warnings_are_printed_with_the_requirement_they_discharge() {
        let novel: Vec<(u32, Vec<u64>)> = (0..200u32).map(|i| (i, vec![u64::from(i)])).collect();
        let text = sample(50, &novel).to_text();
        assert!(text.contains("[FR-060]"), "{text}");
        let healthy = shared_workload().to_text();
        assert!(healthy.contains("warnings: none"), "{healthy}");
    }
}
