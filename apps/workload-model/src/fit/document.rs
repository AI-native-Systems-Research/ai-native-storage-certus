//! Assembling a fitted [`Document`] (spec FR-055, FR-055d, FR-019b).
//!
//! The last step of a fit: turn the measurements into the YAML a `plan` can read.
//! Two rules govern it, and both are about what the emitted document must *not* say.
//!
//! **A parameter that was not measured is not written.** FR-055 requires it left
//! unset, because a number in the emitted YAML is indistinguishable from a
//! measurement, and the entire value of a fitted model is that a reader can tell
//! which of its figures came from data. Where the schema itself supplies a documented
//! default the field is simply omitted and the report says so — omitting is honest,
//! writing the default's value as though it were fitted is not.
//!
//! **Two parameters cannot come from a trace at all**, and both are refused rather
//! than guessed:
//!
//! - `corpus.block_bytes`, because a trace's block size is **tokens** and the
//!   generator's is KV bytes. Converting needs the model's geometry — layers, KV
//!   heads, head dimension, dtype width — and no trace in the corpus carries it. So
//!   the caller supplies it.
//! - `topology`, because no trace carries node or GPU attribution of any kind
//!   (FR-019b). Placement is declared, never fitted, so the section is omitted
//!   entirely rather than filled with a single-node guess.

use serde::{Deserialize, Serialize};

use crate::dist::{Dist, Shape};
use crate::keys::CacheKey;
use crate::schema::{
    Arrival, ArrivalModel, Branching, Corpus, Document, MixEntry, Roots, Run, Sessions, Trees,
};
use crate::stats::FastMap;

use super::branching::FittedBranching;
use super::sessions::FittedSessions;

/// Percentile points a fitted `roots.popularity` is emitted at.
const RANK_POINTS: [f64; 8] = [0.05, 0.10, 0.25, 0.50, 0.75, 0.90, 0.99, 1.00];

/// Sessions per root, which is what `roots.popularity` is a distribution over.
///
/// `contracts/workload-schema.md` § Fitting: "`roots.popularity` | histogram of
/// sessions per root". The schema's parameter is a distribution over root *rank*, so
/// the roots are ordered by session count and the emitted distribution maps a draw
/// onto a rank — which is exactly how the generator consumes it.
#[derive(Debug, Default)]
pub struct RootPopularity {
    /// Root key of each session's first request.
    root_of_session: FastMap<u32, CacheKey>,
}

impl RootPopularity {
    /// An empty accumulator.
    pub fn new() -> RootPopularity {
        RootPopularity::default()
    }

    /// Record a session's root — the depth-0 block of its first request.
    ///
    /// A session binds to one root at birth and stays on it (FR-019a), so the first
    /// one seen is the binding and later requests cannot change it. Recording
    /// otherwise would let a session that appears twice count as two.
    pub fn observe(&mut self, session: u32, root: CacheKey) {
        self.root_of_session.entry(session).or_insert(root);
    }

    /// Distinct roots observed.
    pub fn roots(&self) -> u64 {
        self.root_of_session
            .values()
            .collect::<std::collections::BTreeSet<_>>()
            .len() as u64
    }

    /// The fitted distribution over rank, or `None` with nothing to fit.
    pub fn finish(&self) -> Option<Dist> {
        let mut per_root: FastMap<CacheKey, u64> = FastMap::default();
        for root in self.root_of_session.values() {
            *per_root.entry(*root).or_insert(0) += 1;
        }
        if per_root.is_empty() {
            return None;
        }
        let mut counts: Vec<u64> = per_root.into_values().collect();
        // Descending, so rank 1 is the most popular root — the order the schema's
        // Zipf-over-rank parameter assumes.
        counts.sort_unstable_by(|a, b| b.cmp(a));
        let total: u64 = counts.iter().sum();

        let mut steps: Vec<(f64, f64)> = Vec::new();
        let mut acc = 0u64;
        let mut next = 0usize;
        for (i, c) in counts.iter().enumerate() {
            acc += c;
            let cumulative = acc as f64 / total as f64;
            let mut wanted = false;
            while next < RANK_POINTS.len() && cumulative >= RANK_POINTS[next] {
                next += 1;
                wanted = true;
            }
            if wanted {
                // Rank is 1-based, matching `sample_u64_clamped(st, 1, roots)`.
                steps.push(((i + 1) as f64, cumulative));
            }
        }
        if let Some(last) = steps.last_mut() {
            last.1 = 1.0;
        }
        // Step points, for the same reason `fit::sessions` uses them: rank is
        // discrete and `dist::empirical` interpolates, so bare points would draw
        // ranks between the ones measured.
        let mut points: Vec<(f64, f64)> = Vec::new();
        let mut prev = 0.0f64;
        for (v, c) in steps {
            if prev > 0.0 {
                points.push((v, prev));
            }
            points.push((v, c));
            prev = c;
        }
        Some(Dist::Shaped(Shape::Empirical { points }))
    }
}

/// What the caller must supply because no trace carries it.
#[derive(Debug, Clone)]
pub struct Supplied {
    /// `corpus.block_bytes`. Tokens are not bytes and no trace carries the model
    /// geometry that would convert them.
    pub block_bytes: Dist,
    /// Requests per second, for a trace whose timestamps cannot supply one.
    pub rate_per_s: Option<f64>,
    /// The window every windowed statistic was measured over.
    pub wss_window_requests: u64,
    /// The seed the emitted document carries.
    ///
    /// A property of the *sample*, not of the workload, so it is the caller's to fix
    /// and a fit does not invent one that would look measured.
    pub seed: u64,
}

/// A fitted document, and everything a reader needs to judge it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FittedDocument {
    /// The document, ready to serialise.
    #[serde(skip)]
    pub document: Option<Document>,
    /// Parameters left unset, and why.
    pub unset: Vec<String>,
    /// Everything the measurements themselves qualify.
    pub caveats: Vec<String>,
    /// Requests behind the fit.
    pub requests: u64,
    /// Sessions behind the fit.
    pub sessions: u64,
}

/// Why a document could not be assembled.
#[derive(Debug)]
pub enum FitError {
    /// A measurement the schema requires came back empty.
    Unmeasured(&'static str),
    /// The trace has no usable timestamps and no rate was supplied.
    NoArrivalRate,
}

impl std::fmt::Display for FitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FitError::Unmeasured(what) => write!(
                f,
                "{what} could not be measured from this trace, and the schema requires it. \
                 Refusing rather than emitting a default, which in the output YAML would be \
                 indistinguishable from a measurement (FR-055)"
            ),
            FitError::NoArrivalRate => write!(
                f,
                "the trace has no usable timestamps, so the arrival rate is not measurable. \
                 Supply one, or fit a trace whose `request_start` is native: an invented rate \
                 would change the plan's whole time axis while looking fitted"
            ),
        }
    }
}

impl std::error::Error for FitError {}

/// Assemble a document from the fitted parts.
///
/// `chronological` says whether the trace's order was real; where it was not, every
/// order-dependent parameter is marked as such rather than presented as measured
/// (FR-055d).
pub fn assemble(
    branching: &FittedBranching,
    sessions: &FittedSessions,
    roots: &RootPopularity,
    supplied: &Supplied,
    requests: u64,
    chronological: bool,
) -> Result<FittedDocument, FitError> {
    let turns = sessions
        .turns
        .clone()
        .ok_or(FitError::Unmeasured("turns"))?;
    let private_depth = sessions
        .private_depth
        .clone()
        .ok_or(FitError::Unmeasured("private_depth"))?;
    let shared_depth = sessions
        .shared_depth
        .clone()
        .ok_or(FitError::Unmeasured("shared_depth"))?;
    let popularity = roots
        .finish()
        .ok_or(FitError::Unmeasured("roots.popularity"))?;

    let mut unset = Vec::new();
    let mut caveats = branching.caveats();
    caveats.extend(sessions.caveats());

    // think_time: measured where timestamps allow, and a hard stop otherwise, since
    // the schema requires it and a guess would set the plan's whole time axis.
    let think_time = match sessions.think_time.clone() {
        Some(t) => t,
        None => return Err(FitError::Unmeasured("think_time")),
    };

    // growth_per_turn is only measurable from a session with more than one turn.
    let growth_per_turn = match sessions.growth_per_turn.clone() {
        Some(g) => g,
        None => {
            unset.push(
                "growth_per_turn: no session in this trace had a second turn, so there is no \
                 depth increment to measure. The emitted model is one-shot, which is what the \
                 trace showed"
                    .to_string(),
            );
            Dist::Scalar(0.0)
        }
    };

    // The arrival rate: measured from the trace's own span where it has one.
    let rate = match supplied.rate_per_s {
        Some(r) => r,
        None => return Err(FitError::NoArrivalRate),
    };

    unset.push(
        "corpus.trees.branch_skew: its fitting procedure is an open derivation \
         (research.md § Open derivations), so it is omitted and takes the schema's documented \
         default rather than a value that would read as fitted"
            .to_string(),
    );
    unset.push(
        "corpus.trees.churn: a trace of ordinary length cannot distinguish a rotated shared \
         prefix from one that was never shared, so churn is not fittable and is left off \
         entirely"
            .to_string(),
    );
    unset.push(
        "topology: no trace carries node or GPU attribution of any kind, so placement, \
         self_affinity and replication are declared rather than fitted (FR-019b)"
            .to_string(),
    );
    unset.push(
        "workload.mix: emitted as a single arm. Decomposing a trace into a weighted mixture \
         is not something any measurement here identifies, and inventing arms would put \
         structure in the model that the trace does not evidence"
            .to_string(),
    );

    if !chronological {
        caveats.push(
            "the trace had no usable timestamps, so its order was file order: every \
             order-dependent measurement above — realised sharing, and through it \
             private_depth — is order-dependent rather than measured (FR-055d)"
                .to_string(),
        );
    }

    let document = Document {
        version: 1,
        seed: supplied.seed,
        extends: None,
        duration: None,
        requests: Some(requests),
        blocks: None,
        unbounded: None,
        corpus: Corpus {
            block_bytes: supplied.block_bytes.clone(),
            trees: Trees {
                roots: Roots {
                    count: branching.roots.max(1) as u32,
                    popularity,
                },
                shared_depth,
                branching: if branching.segments.is_empty() {
                    // No fitted segment means the uncensored prefix held no width
                    // change: a flat trunk, which is a measurement rather than an
                    // absence.
                    Branching::Uniform(1.0)
                } else {
                    Branching::Profile(branching.segments.clone())
                },
                branch_skew: crate::schema::default_branch_skew(),
                churn: None,
            },
        },
        workload: crate::schema::Workload {
            arrival: Arrival {
                model: ArrivalModel::OpenLoop,
                rate: Some(format!("{rate:.3}/s")),
                burstiness: None,
                concurrency: None,
            },
            sessions: Sessions {
                turns,
                think_time,
                private_depth,
                growth_per_turn,
                spawn: None,
            },
            mix: vec![MixEntry {
                weight: 1.0,
                turns: None,
                think_time: None,
                private_depth: None,
                growth_per_turn: None,
            }],
            drift: None,
        },
        topology: None,
        run: Run {
            mode: "plan".to_string(),
            endpoint_template: None,
            batch_size: None,
            workers: None,
            inflight: None,
            gpu_buffer: None,
            // A fitted document carries no warmup: how long to warm is a property of
            // the run the operator is about to do, and rule 15b will tell them if
            // what they choose is too short.
            warmup: None,
            warm_connections: None,
            wss_window: Some(serde_yaml::Value::Number(
                supplied.wss_window_requests.into(),
            )),
            clock_skew_bound: None,
            emit_trace: None,
        },
        sweep: None,
    };

    Ok(FittedDocument {
        document: Some(document),
        unset,
        caveats,
        requests,
        sessions: sessions.sessions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fit::sessions::SessionShapes;
    use crate::stats::hist::Hist;
    use crate::stats::sharing::SharingReport;

    fn sharing_with(depths: &[u64]) -> SharingReport {
        let mut h = Hist::new();
        for d in depths {
            h.add(*d);
        }
        h.seal();
        SharingReport {
            requests: depths.len() as u64,
            sharing_requests: h.count(),
            unshared_requests: 0,
            shared_fraction: Some(1.0),
            realised_depth: h.summary(),
            depth_buckets: h.buckets(),
        }
    }

    fn fitted_sessions() -> FittedSessions {
        let mut s = SessionShapes::new();
        for session in 0..40u32 {
            s.observe(session, 0, 30, 18, Some(0.0));
            s.observe(session, 1, 40, 18, Some(2.0));
        }
        s.finish(&sharing_with(&[18, 18, 18, 4, 40]))
    }

    fn fitted_branching() -> FittedBranching {
        FittedBranching {
            root_boundary_depth: 0,
            roots: 12,
            retention_at_boundary: 1.0,
            segments: vec![crate::schema::Segment {
                from_depth: 0,
                fanout: 1.05,
                skew: None,
                churn_half_life: None,
            }],
            segment_occupancy: vec![8.0],
            fitted_to_depth: 40,
            observed_to_depth: 40,
            censored_ratios: 0,
            is_lower_bound: true,
        }
    }

    fn roots_with(sessions_per_root: &[u64]) -> RootPopularity {
        let mut r = RootPopularity::new();
        let mut session = 0u32;
        for (root, n) in sessions_per_root.iter().enumerate() {
            for _ in 0..*n {
                r.observe(session, CacheKey(root as u64));
                session += 1;
            }
        }
        r
    }

    fn supplied() -> Supplied {
        Supplied {
            block_bytes: Dist::Scalar(131_072.0),
            rate_per_s: Some(2000.0),
            wss_window_requests: 20_000,
            seed: 7,
        }
    }

    #[test]
    fn a_fitted_document_parses_back_as_a_document() {
        // The whole point: what `fit` writes must be what `plan` reads.
        let f = assemble(
            &fitted_branching(),
            &fitted_sessions(),
            &roots_with(&[40, 20, 10, 5]),
            &supplied(),
            80,
            true,
        )
        .expect("should assemble");
        let yaml = f.document.as_ref().unwrap().to_yaml().expect("serialise");
        let back = Document::from_yaml(&yaml).expect("a fitted document must parse");
        assert_eq!(back.version, 1);
        assert_eq!(back.requests, Some(80));
        assert_eq!(back.corpus.trees.roots.count, 12);
    }

    #[test]
    fn a_fitted_document_passes_its_own_validation() {
        // FR-055a: `fit` must fail rather than emit a combination the generator
        // cannot realise, so the document it emits has to survive the schema.
        let f = assemble(
            &fitted_branching(),
            &fitted_sessions(),
            &roots_with(&[40, 20, 10, 5]),
            &supplied(),
            80,
            true,
        )
        .unwrap();
        let doc = f.document.unwrap();
        let report = crate::schema::validate::validate(&doc);
        let rejections: Vec<String> = report
            .rejections()
            .map(|r| format!("[{}] {}", r.rule, r.message))
            .collect();
        assert!(
            rejections.is_empty(),
            "a fitted document was rejected: {}",
            rejections.join("; ")
        );
    }

    #[test]
    fn the_unfittable_parameters_are_named_rather_than_filled_in() {
        let f = assemble(
            &fitted_branching(),
            &fitted_sessions(),
            &roots_with(&[10, 5]),
            &supplied(),
            30,
            true,
        )
        .unwrap();
        let unset = f.unset.join("\n");
        for expected in ["branch_skew", "churn", "topology", "workload.mix"] {
            assert!(unset.contains(expected), "{expected} not named:\n{unset}");
        }
        // And no topology section was invented.
        assert!(f.document.unwrap().topology.is_none());
    }

    #[test]
    fn a_trace_without_a_rate_is_refused_rather_than_given_one() {
        let mut s = supplied();
        s.rate_per_s = None;
        let e = assemble(
            &fitted_branching(),
            &fitted_sessions(),
            &roots_with(&[10]),
            &s,
            10,
            true,
        )
        .expect_err("must refuse");
        assert!(e
            .to_string()
            .contains("would change the plan's whole time axis"));
    }

    #[test]
    fn file_order_is_carried_into_the_caveats() {
        // FR-055d: an order-dependent measurement must not read as a measured one.
        let f = assemble(
            &fitted_branching(),
            &fitted_sessions(),
            &roots_with(&[10]),
            &supplied(),
            10,
            false,
        )
        .unwrap();
        assert!(f
            .caveats
            .iter()
            .any(|c| c.contains("order-dependent rather than measured")));
    }

    #[test]
    fn root_popularity_ranks_roots_by_their_session_count() {
        // Rank 1 is the most popular, which is the order the schema's parameter
        // assumes. With 60 of 100 sessions on one root, the median draw is rank 1.
        let r = roots_with(&[60, 20, 10, 10]);
        assert_eq!(r.roots(), 4);
        let d = r.finish().expect("fitted");
        assert_eq!(d.quantile(0.5), Some(1.0));
        assert_eq!(d.quantile(1.0), Some(4.0));
    }

    #[test]
    fn a_session_seen_twice_counts_once_toward_its_root() {
        // A session binds to one root at birth (FR-019a), so a later request cannot
        // move it and must not double its weight.
        let mut r = RootPopularity::new();
        r.observe(1, CacheKey(10));
        r.observe(1, CacheKey(10));
        r.observe(1, CacheKey(99));
        assert_eq!(r.roots(), 1);
    }

    #[test]
    fn a_missing_required_measurement_refuses_and_names_itself() {
        let empty = SessionShapes::new().finish(&sharing_with(&[]));
        let e = assemble(
            &fitted_branching(),
            &empty,
            &roots_with(&[10]),
            &supplied(),
            10,
            true,
        )
        .expect_err("must refuse");
        assert!(e.to_string().contains("turns"), "{e}");
        assert!(e.to_string().contains("FR-055"), "{e}");
    }
}
