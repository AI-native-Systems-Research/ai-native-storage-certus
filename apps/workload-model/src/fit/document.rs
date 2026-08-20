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
    Arrival, ArrivalModel, Branching, Corpus, Document, MixEntry, RootTurn1, Roots, Run, Sessions,
    Trees,
};
use crate::stats::FastMap;

use super::branching::FittedBranching;
use super::sessions::FittedSessions;

/// Ranks a fitted `roots.popularity` is emitted at before ranks are grouped.
///
/// One step per rank up to this many, so **every** rank in the support is reachable.
/// It replaced eight percentile points (0.05, 0.10, 0.25, 0.50, 0.75, 0.90, 0.99, 1.00)
/// on 2026-08-14, which was a readability budget standing in for an accuracy one and cost
/// far more than readability: a step CDF's steps have zero width, so `dist::empirical`
/// could only ever return one of the point *values themselves*. For a real trace's
/// histogram those were ranks {1, 6, 38, 132, 153} — a fitted model with **five**
/// populated roots however many `roots.count` claimed. Measured with the real generator
/// over 1.2M sessions.
///
/// The corpus's largest shared-root count is 249, so grouping never engages on it.
const RANK_MAX_STEPS: usize = 4096;

/// Sessions per root, which is what `roots.popularity` is a distribution over.
///
/// `contracts/workload-schema.md` § Fitting: "`roots.popularity` | histogram of
/// sessions per root". The schema's parameter is a distribution over root *rank*, so
/// the roots are ordered by session count and the emitted distribution maps a draw
/// onto a rank — which is exactly how the generator consumes it.
#[derive(Debug, Default)]
pub struct RootPopularity {
    /// Per session: its lowest-numbered turn, that turn's root key, and its path length.
    ///
    /// The path length rides along with the root binding so that the per-root turn-1 table and
    /// `popularity` are measured over one population in one order. They were computed apart
    /// once — `roots.count`, the popularity support and the realised root layer disagreed three
    /// ways — and a level attached to the wrong rank is that failure in a form no total would
    /// reveal.
    root_of_session: FastMap<u32, (u32, CacheKey, u64)>,
}

impl RootPopularity {
    /// An empty accumulator.
    pub fn new() -> RootPopularity {
        RootPopularity::default()
    }

    /// Record a session's root — the depth-0 block of its first request — and that request's
    /// path length.
    ///
    /// A session binds to one root at birth and stays on it (FR-019a), so later requests cannot
    /// change the binding. The **lowest-numbered turn** wins rather than the first one seen,
    /// because a disordered trace's first arrival can be mid-conversation and its path length is
    /// then not a turn-1 quantity — the same trap already recorded for `private_depth`. The root
    /// key is invariant across a session's turns (turn n+1 re-reads turn n's blocks, so
    /// `blocks[0]` is the same key), so taking both from one turn cannot disagree with taking the
    /// root from the first arrival, and it keeps this measurement aligned with `root_path_eta2`.
    pub fn observe(&mut self, session: u32, turn: u32, root: CacheKey, path_blocks: u64) {
        let e = self
            .root_of_session
            .entry(session)
            .or_insert((u32::MAX, root, path_blocks));
        if turn <= e.0 {
            *e = (turn, root, path_blocks);
        }
    }

    /// Distinct roots observed, shared or not.
    pub fn roots(&self) -> u64 {
        self.root_of_session
            .values()
            .map(|(_, root, _)| root)
            .collect::<std::collections::BTreeSet<_>>()
            .len() as u64
    }

    /// The fitted root layer, or `None` with nothing to fit.
    pub fn finish(&self) -> Option<FittedRoots> {
        // Per root: sessions, and the sum and sum of squares of their turn-1 path lengths. The
        // second moment costs one multiply here and is what lets each root state its own level
        // and spread rather than borrowing the population's.
        let mut per_root: FastMap<CacheKey, (u64, f64, f64)> = FastMap::default();
        for (_, root, path) in self.root_of_session.values() {
            let e = per_root.entry(*root).or_insert((0, 0.0, 0.0));
            e.0 += 1;
            e.1 += *path as f64;
            e.2 += (*path as f64) * (*path as f64);
        }
        if per_root.is_empty() {
            return None;
        }
        let observed = per_root.len() as u32;
        let mut ranked: Vec<(CacheKey, u64, f64, f64)> = per_root
            .into_iter()
            .map(|(k, (c, s, sq))| (k, c, s, sq))
            .collect();
        // Descending by session count, so rank 1 is the most popular root — the order the schema's
        // Zipf-over-rank parameter assumes. Ties break on the root KEY, which matters now that a
        // per-root level is emitted against a rank: sorting on the count alone left equal-count
        // roots in hash order, so the rank a level belongs to would differ between two runs over
        // the same trace while every total stayed identical.
        ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0 .0.cmp(&b.0 .0)));
        // **Shared roots only**, decided 2026-08-14. `roots.count` is the shared width at
        // depth 0 — keys two or more sessions reached — so the popularity distribution
        // over it must be measured on the same population, or the two disagree and the
        // support cannot span the count. Measured on `qwen_code`, they disagreed three
        // ways at once before this: `roots.count` 603 (a *folded* boundary depth), the
        // popularity support 153 (all first-request roots, singletons included), and the
        // realised root layer 5 (the step-encoding cap above).
        //
        // A session on a singleton root shares no prefix with anything, which is a case
        // the model already cannot express — `shared_depth`'s support starts at 1 — so it
        // is counted and reported rather than silently folded into the shared population.
        let singleton_sessions: u64 = ranked
            .iter()
            .map(|(_, c, _, _)| *c)
            .filter(|c| *c < 2)
            .sum();
        // Retained on `ranked` rather than on a copy of the counts, so the per-root turn-1 table
        // below is over exactly the population `popularity` is over, at exactly its ranks.
        ranked.retain(|(_, c, _, _)| *c >= 2);
        if ranked.is_empty() {
            return None;
        }
        let counts: Vec<u64> = ranked.iter().map(|(_, c, _, _)| *c).collect();
        let count = counts.len() as u32;
        let total: u64 = counts.iter().sum();

        // One step per rank, so every rank in the support is reachable. Grouped only
        // above `RANK_MAX_STEPS`, where the alternative is a document listing a step per
        // root for a corpus with thousands of them; the corpus's largest is 249.
        let group = counts.len().div_ceil(RANK_MAX_STEPS);
        let mut steps: Vec<(f64, f64)> = Vec::new();
        let mut acc = 0u64;
        for (i, c) in counts.iter().enumerate() {
            acc += c;
            if (i + 1) % group == 0 || i + 1 == counts.len() {
                // Rank is 1-based, matching `sample_u64_clamped(st, 1, roots)`.
                steps.push(((i + 1) as f64, acc as f64 / total as f64));
            }
        }
        if let Some(last) = steps.last_mut() {
            // The support must reach `roots.count` exactly, which schema rule 8 checks:
            // a distribution stopping short leaves the roots above it unreachable, and
            // the generator records no clamp for headroom it never uses.
            last.0 = f64::from(count);
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
        Some(FittedRoots {
            popularity: Dist::Shaped(Shape::Empirical { points }),
            count,
            observed,
            sessions: self.root_of_session.len() as u64,
            singleton_sessions,
            turn1_path: self.turn1_path(&ranked),
        })
    }

    /// Each retained root's turn-1 path level and spread, plus the pooled standardised residual
    /// (FR-054j).
    ///
    /// `ranked` arrives in `popularity`'s rank order and over `popularity`'s population, so the
    /// two tables are aligned by construction rather than by a second sort agreeing with the
    /// first.
    ///
    /// `None` when no root's sessions vary at all: the shape would then be empty, and a document
    /// stating a zero spread everywhere is better served by its absence, which falls back to the
    /// population marginal.
    fn turn1_path(&self, ranked: &[(CacheKey, u64, f64, f64)]) -> Option<RootTurn1> {
        let mut level = Vec::with_capacity(ranked.len());
        let mut spread = Vec::with_capacity(ranked.len());
        let mut of_root: FastMap<CacheKey, (f64, f64)> = FastMap::default();
        for (key, c, sum, sq) in ranked {
            let n = *c as f64;
            let mean = sum / n;
            // Sample variance from the running sums, floored at zero: the algebraic form can turn
            // very slightly negative on a root whose sessions all share one path length.
            let sd = if *c > 1 {
                ((sq - sum * mean).max(0.0) / (n - 1.0)).sqrt()
            } else {
                0.0
            };
            level.push(mean);
            spread.push(sd);
            of_root.insert(*key, (mean, sd));
        }
        // The residual shape, standardised per root before pooling so that it carries shape only
        // and not the differences in scale `spread` already states. A root with no spread
        // contributes nothing: its sessions sit exactly at its level, and `0/0` is not an
        // observation of shape.
        let mut z: Vec<f64> = Vec::new();
        for (_, root, path) in self.root_of_session.values() {
            if let Some((mean, sd)) = of_root.get(root) {
                if *sd > 0.0 {
                    z.push((*path as f64 - mean) / sd);
                }
            }
        }
        if z.len() < 2 {
            return None;
        }
        z.sort_by(f64::total_cmp);
        // An empirical CDF over the standardised residual, which `Shape::Empirical` carries
        // directly: its points are real-valued, so a residual below its root's level needs no
        // offset encoding, and `Dist::sample` returns the signed value.
        let n = z.len() as f64;
        let points: Vec<(f64, f64)> = z
            .iter()
            .enumerate()
            .map(|(i, v)| (*v, (i + 1) as f64 / n))
            .collect();
        Some(RootTurn1 {
            level,
            spread,
            shape: Dist::Shaped(Shape::Empirical { points }),
        })
    }
}

/// The fitted root layer: a rank distribution and the population it is over.
///
/// The two travel together because they were measured apart and disagreed — see
/// [`RootPopularity::finish`]. `count` is both `roots.count` and the support of
/// `popularity`, by construction rather than by coincidence.
#[derive(Debug, Clone)]
pub struct FittedRoots {
    /// Distribution over root rank, with support `1..=count`.
    pub popularity: Dist,
    /// Shared roots: those two or more sessions bound to. `roots.count`.
    pub count: u32,
    /// Distinct first-request roots observed, singletons included.
    pub observed: u32,
    /// Sessions that bound to any root at all.
    pub sessions: u64,
    /// Sessions whose root no other session bound to.
    pub singleton_sessions: u64,
    /// Turn-1 path level and spread per root, in `popularity`'s rank order (FR-054j).
    pub turn1_path: Option<RootTurn1>,
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
            // Both variants are FR-054b case 2, and both MUST say so. The schema requires a
            // parameter the trace was never obliged to record, which is the model asking for
            // something the data does not owe it — not a defect in the trace and not
            // something the caller can fix by passing a different flag. Leaving the
            // classification off made these two the only refusals in the taxonomy that named
            // no outcome at all, and a corpus sweep then reported them as `OK` (FR-055f):
            // silent success is the worst available reading of a refusal.
            FitError::Unmeasured(what) => write!(
                f,
                "MODEL LIMITATION (FR-054a), not a defect in the trace: `{what}` could not be \
                 measured from it and this model's schema requires it, so the binding \
                 restriction is that the schema admits no document with `{what}` absent. \
                 Refusing rather than emitting a default, which in the output YAML would be \
                 indistinguishable from a measurement (FR-055)"
            ),
            FitError::NoArrivalRate => write!(
                f,
                "MODEL LIMITATION (FR-054a), not a defect in the trace: it carries no usable \
                 timestamps, so the arrival rate is not measurable, and this model has no \
                 timeless form — `run.arrival` is required and sets the plan's whole time \
                 axis. Supply a rate explicitly, or fit a trace whose `request_start` is \
                 native: an invented rate would look fitted"
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
    segments: Option<&crate::schema::SegmentProcess>,
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
    // Still the measured distribution, and it is now used for **one** thing rather than two.
    //
    // `shared_depth` is doubly loaded, which is the sharpest evidence that it is a stand-in
    // rather than a property: `session::depth_at_turn` computes turn-1 depth as
    // `shared_depth + private_depth`, so the field is simultaneously a term in PATH LENGTH
    // and the TRUNK BOUNDARY. Since 2026-08-15 the trunk boundary is derived — a session
    // leaves the trunk when its expected cohort falls below two — and this value survives
    // only as the length term, where it is a measured quantity.
    //
    // Both roles disappear together when `{shared_depth, private_depth}` become one measured
    // turn-1 path length, which is what the trace hands over directly. Staged separately: it
    // is a 265-site change and the mechanism replacing the boundary role should be measured
    // before the refactor rides on it.
    let shared_depth = sessions
        .shared_depth
        .clone()
        .ok_or(FitError::Unmeasured("shared_depth"))?;
    let fitted_roots = roots
        .finish()
        .ok_or(FitError::Unmeasured("roots.popularity"))?;

    let mut unset = Vec::new();
    let mut caveats = branching.caveats();
    caveats.extend(sessions.caveats());

    // Sessions on a root nobody else used. They are excluded from `roots.popularity`
    // because `roots.count` is the *shared* root layer, and the model has no way to say
    // "this session shares nothing" — the same restriction `shared_depth`'s support
    // already imposes. Reported rather than folded in silently.
    if fitted_roots.singleton_sessions > 0 {
        caveats.push(format!(
            "MODEL LIMITATION (FR-054a): {} of {} sessions bound to a root no other session \
             used, and `roots.count` is the SHARED root layer ({} of {} distinct first-request \
             roots), so those sessions are not represented in `roots.popularity`. A generated \
             model puts every session on a shared root, giving them sharing the trace gave them \
             none of. A session alone on its root is ordinary workload — a one-off prompt — so \
             the gap is in the model's root layer, not in the trace",
            fitted_roots.singleton_sessions,
            fitted_roots.sessions,
            fitted_roots.count,
            fitted_roots.observed
        ));
    }
    // The shared root layer, measured a second way. The trunk report counts a depth-0 key
    // as shared when two sessions reached it at *any* invocation; a session's root is its
    // *first* request's. Real traces contain a few sessions whose invocations start at
    // different roots, so the two can differ by a handful — reported, because a reader
    // comparing `roots.count` against a width table needs to know which rule produced it.
    if branching.roots != u64::from(fitted_roots.count) {
        caveats.push(format!(
            "`roots.count` is {} — shared roots by session binding — while the trunk width at \
             depth 0 counts {} shared keys. The two rules differ: a session binds to its FIRST \
             request's root (FR-019a) whereas the width counts a key shared if two sessions \
             reached it at any invocation, and a few sessions in a real trace start at more than \
             one root. `roots.count` follows the binding, because that is the population \
             `roots.popularity` is a distribution over",
            fitted_roots.count, branching.roots
        ));
    }

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
            crate::schema::Growth::Uniform(Dist::Scalar(0.0))
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
                // `roots.count` and `roots.popularity` come from the SAME accumulator,
                // so the support spans the count by construction (schema rule 8 checks
                // it). `branching.roots` — the shared width at depth 0 from the trunk
                // report — is the same quantity measured a second way, and the two are
                // compared in a caveat rather than silently preferred: the trunk counts a
                // depth-0 key shared if two sessions reached it at *any* invocation,
                // while a session's root is its *first* request's, and a handful of
                // sessions in a real trace start at two different roots.
                roots: Roots {
                    count: fitted_roots.count.max(1),
                    popularity: fitted_roots.popularity.clone(),
                    // Stated only beside `sessions.turn1_path_length`, which is what decides
                    // whether turn-1 depth is a measured joint at all: a per-root level is a
                    // refinement of that draw and means nothing without it. So one experiment
                    // toggle governs both, and a document either states the joint or does not.
                    turn1_path: sessions
                        .turn1_path_length
                        .as_ref()
                        .and(fitted_roots.turn1_path.clone()),
                },
                shared_depth,
                // A node-level process where the census produced one, because it is the
                // only spelling that can state per-root preamble lengths and total
                // out-degree — the two things the corpus's structure is made of and a
                // per-depth profile cannot express. The width profile remains the
                // fallback, and remains what `--explain`'s width table is judged against.
                branching: match segments {
                    Some(p) => Branching::Segments(p.clone()),
                    None if branching.segments.is_empty() => {
                        // No fitted segment means the uncensored prefix held no width
                        // change: a flat trunk, which is a measurement rather than an
                        // absence.
                        Branching::Uniform(1.0)
                    }
                    None => Branching::Profile(branching.segments.clone()),
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
                // The measured joint turn-1 path length (FR-054i). `Some` only under the
                // experiment toggle, since stating it changes every generated path.
                turn1_path_length: sessions.turn1_path_length.clone(),
                turns,
                think_time,
                private_depth,
                growth_per_turn,
                // The context window, solved against the accumulation it governs
                // (FR-054c). `None` where the trace showed no saturation, which leaves
                // growth unbounded exactly as it was before this parameter existed.
                max_depth: sessions.max_depth.clone(),
                spawn: None,
            },
            mix: vec![MixEntry {
                turn1_path_length: None,
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
            roots: 12,
            retention_at_fitted_to: 1.0,
            segments: vec![crate::schema::Segment {
                from_depth: 0,
                fanout: 1.05,
                skew: None,
                churn_half_life: None,
            }],
            raw_fanouts: vec![1.05],
            segment_occupancy: vec![8.0],
            fitted_to_depth: 40,
            observed_to_depth: 40,
            censored_ratios: 0,
            is_lower_bound: true,
        }
    }

    fn roots_with(sessions_per_root: &[u64]) -> RootPopularity {
        roots_with_paths(sessions_per_root, |_, _| 100)
    }

    /// Roots with a stated turn-1 path length per session, so the FR-054j table can be checked.
    ///
    /// `path(root, i)` gives the `i`-th session on `root` its path length.
    fn roots_with_paths(
        sessions_per_root: &[u64],
        path: impl Fn(usize, u64) -> u64,
    ) -> RootPopularity {
        let mut r = RootPopularity::new();
        let mut session = 0u32;
        for (root, n) in sessions_per_root.iter().enumerate() {
            for i in 0..*n {
                r.observe(session, 1, CacheKey(root as u64), path(root, i));
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
            None,
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
        // `roots.count` is the SHARED root layer from the popularity accumulator — four
        // roots here — not the trunk report's depth-0 width (12 in `fitted_branching()`).
        // It asserted 12 until 2026-08-14, when the two came from different measurements
        // and could not be made to agree; taking both from one accumulator is what lets
        // rule 8 check that the popularity's support spans the count.
        assert_eq!(back.corpus.trees.roots.count, 4);
        match back.corpus.trees.roots.popularity.shape() {
            Shape::Empirical { points } => assert_eq!(
                points.iter().map(|(v, _)| *v).fold(0.0f64, f64::max),
                4.0,
                "the emitted support must span the emitted count"
            ),
            other => panic!("expected an empirical popularity, got {other:?}"),
        }
    }

    #[test]
    fn a_fitted_document_passes_its_own_validation() {
        // FR-055a: `fit` must fail rather than emit a combination the generator
        // cannot realise, so the document it emits has to survive the schema.
        let f = assemble(
            &fitted_branching(),
            None,
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
            None,
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
            None,
            &fitted_sessions(),
            &roots_with(&[10]),
            &s,
            10,
            true,
        )
        .expect_err("must refuse");
        // Asserts what the refusal must convey, not the sentence conveying it: the outcome
        // it classifies as, and that it says an invented rate would pass for a fitted one.
        // The previous version pinned a phrase and so failed on a reword that strengthened
        // the message.
        let msg = e.to_string();
        assert!(msg.contains("MODEL LIMITATION"), "{msg}");
        assert!(msg.contains("time axis"), "{msg}");
        assert!(msg.contains("fitted"), "{msg}");
    }

    #[test]
    fn file_order_is_carried_into_the_caveats() {
        // FR-055d: an order-dependent measurement must not read as a measured one.
        let f = assemble(
            &fitted_branching(),
            None,
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
        let f = r.finish().expect("fitted");
        assert_eq!(f.popularity.quantile(0.5), Some(1.0));
        assert_eq!(f.popularity.quantile(1.0), Some(4.0));
        assert_eq!(f.count, 4, "all four roots are shared");
        assert_eq!(f.singleton_sessions, 0);
    }

    #[test]
    fn the_turn1_table_states_each_roots_own_level_and_spread_at_its_own_rank() {
        // Rank 1 holds 40 sessions of length 100 or 120, rank 2 holds 10 of length 500 or 504.
        // The levels must land on the ranks `popularity` orders, and each spread must be that
        // root's own: the whole defect being fixed is a level that came from the population.
        let r = roots_with_paths(&[40, 10], |root, i| match (root, i % 2) {
            (0, 0) => 100,
            (0, _) => 120,
            (_, 0) => 500,
            (_, _) => 504,
        });
        let t = r.finish().expect("fitted").turn1_path.expect("a table");
        assert_eq!(t.level.len(), 2);
        assert_eq!(t.spread.len(), 2);
        assert!(
            (t.level[0] - 110.0).abs() < 1e-9,
            "rank 1 level {:?}",
            t.level
        );
        assert!(
            (t.level[1] - 502.0).abs() < 1e-9,
            "rank 2 level {:?}",
            t.level
        );
        // Sample sd of a two-point balanced set is half the gap times sqrt(n/(n-1)) → ~10.1 and
        // ~2.1, so the ratio of spreads is ~5 while the ratio of levels is ~4.6. A pooled
        // residual could not tell those apart.
        assert!(t.spread[0] > 4.0 * t.spread[1], "spreads {:?}", t.spread);
    }

    #[test]
    fn the_standardised_residual_is_shared_and_carries_no_scale() {
        // Two roots with the same shape and very different scales must contribute the SAME
        // standardised residuals, which is what makes `shape` a shape: pooling raw residuals
        // instead measured worse (KS 0.0094 against 0.0044 at 27 roots).
        let r = roots_with_paths(&[100, 100], |root, i| {
            let step = if i % 2 == 0 { 0 } else { 1 };
            if root == 0 {
                100 + step * 10
            } else {
                1000 + step * 100
            }
        });
        let t = r.finish().expect("fitted").turn1_path.expect("a table");
        // Both roots are balanced two-point sets, so every standardised residual is ±c for one
        // constant c, and the pooled shape therefore holds two values and not four. Compared to a
        // tolerance rather than bitwise: standardising 100 and 1000 divides by different sds, so
        // the two roots agree to within rounding and not to the last bit.
        let vals: Vec<f64> = match t.shape.shape() {
            Shape::Empirical { points } => points.iter().map(|(v, _)| *v).collect(),
            other => panic!("expected an empirical shape, got {other:?}"),
        };
        let mut distinct: Vec<f64> = Vec::new();
        for v in &vals {
            if !distinct.iter().any(|d| (d - v).abs() < 1e-9) {
                distinct.push(*v);
            }
        }
        assert_eq!(
            distinct.len(),
            2,
            "scale leaked into the shape: {distinct:?}"
        );
        // And it is centred: a balanced set standardises to a mean of zero.
        let mean = t.shape.mean().expect("a mean");
        assert!(mean.abs() < 1e-9, "shape not centred: {mean}");
    }

    #[test]
    fn equal_count_roots_take_a_stable_rank_so_a_level_cannot_move_between_runs() {
        // Ranks are only meaningful for a per-root level if ties break deterministically. Before
        // the key tie-break, equal-count roots sat in hash order: every total was identical and
        // the rank a level belonged to could differ run to run.
        let build = || {
            let mut r = RootPopularity::new();
            let mut session = 0u32;
            // Three roots of two sessions each, distinguishable only by their path lengths. The
            // two sessions on a root differ by 10 blocks, since a root whose sessions never vary
            // contributes no residual and the table would be absent altogether.
            for (root, path) in [(7u64, 700u64), (3, 300), (5, 500)] {
                for i in 0..2 {
                    r.observe(session, 1, CacheKey(root), path + i * 10);
                    session += 1;
                }
            }
            r.finish()
                .expect("fitted")
                .turn1_path
                .expect("a table")
                .level
        };
        let first = build();
        assert_eq!(first.len(), 3);
        for _ in 0..8 {
            assert_eq!(build(), first, "rank order is not stable across runs");
        }
        // Ordered by key, since the counts tie: keys 3, 5, 7 carry levels 305, 505, 705.
        assert_eq!(first, vec![305.0, 505.0, 705.0]);
    }

    #[test]
    fn a_root_whose_sessions_never_vary_yields_no_shape_and_so_no_table() {
        // With no within-root variation there is no residual to standardise, and a table of zero
        // spreads with an empty shape would be a parameter nothing can consume — this branch
        // exists so the document falls back to the population marginal instead.
        let r = roots_with_paths(&[10, 10], |_, _| 250);
        let f = r.finish().expect("fitted");
        assert!(
            f.turn1_path.is_none(),
            "expected no table: {:?}",
            f.turn1_path
        );
    }

    #[test]
    fn every_rank_in_the_support_is_reachable_and_the_support_spans_roots_count() {
        // The defect this shape replaced, and the reason rule 8 now checks the support.
        // Eight percentile points with zero-width steps meant `dist::empirical` could
        // only return one of the point *values*, so a 153-root histogram populated five
        // roots. Asserted by drawing: every rank must actually come out.
        let r = roots_with(&[40, 30, 20, 12, 9, 7, 6, 5, 4, 3, 2, 2]);
        let f = r.finish().expect("fitted");
        assert_eq!(f.count, 12);
        // The last point must land exactly on roots.count, or rule 8 rejects the
        // document the fit just produced.
        match f.popularity.shape() {
            Shape::Empirical { points } => {
                let top = points.iter().map(|(v, _)| *v).fold(0.0f64, f64::max);
                assert_eq!(top, 12.0, "the support must reach roots.count");
            }
            other => panic!("expected an empirical popularity, got {other:?}"),
        }
        let mut seen = [false; 12];
        let mut st = crate::rng::Stream::new(5, 5);
        for _ in 0..20_000 {
            let rank =
                f.popularity
                    .sample_u64_clamped(&mut st, 1, 12, &crate::dist::Clamps::default());
            seen[(rank - 1) as usize] = true;
        }
        let missing: Vec<usize> = seen
            .iter()
            .enumerate()
            .filter(|(_, s)| !**s)
            .map(|(i, _)| i + 1)
            .collect();
        assert!(
            missing.is_empty(),
            "ranks {missing:?} are declared but unreachable"
        );
    }

    #[test]
    fn a_root_only_one_session_used_is_excluded_and_counted() {
        // `roots.count` is the SHARED root layer, so a singleton root is not one of its
        // ranks — and the sessions on it are reported, because the model will place them
        // on a shared root and give them sharing the trace gave them none of.
        let r = roots_with(&[50, 20, 1, 1, 1]);
        let f = r.finish().expect("fitted");
        assert_eq!(f.count, 2, "only two roots had two or more sessions");
        assert_eq!(f.observed, 5, "five distinct roots were seen");
        assert_eq!(f.singleton_sessions, 3);
        assert_eq!(f.sessions, 73);
    }

    #[test]
    fn a_session_seen_twice_counts_once_toward_its_root() {
        // A session binds to one root at birth (FR-019a), so a later request cannot
        // move it and must not double its weight.
        let mut r = RootPopularity::new();
        r.observe(1, 1, CacheKey(10), 100);
        r.observe(1, 2, CacheKey(10), 140);
        r.observe(1, 3, CacheKey(99), 180);
        assert_eq!(r.roots(), 1);
    }

    #[test]
    fn a_missing_required_measurement_refuses_and_names_itself() {
        let empty = SessionShapes::new().finish(&sharing_with(&[]));
        let e = assemble(
            &fitted_branching(),
            None,
            &empty,
            &roots_with(&[10]),
            &supplied(),
            10,
            true,
        )
        .expect_err("must refuse");
        assert!(e.to_string().contains("turns"), "{e}");
        assert!(e.to_string().contains("FR-055"), "{e}");
        assert!(e.to_string().contains("MODEL LIMITATION"), "{e}");
    }

    #[test]
    fn every_refusal_names_its_fr_054b_outcome() {
        // Asserts the classification rather than the prose, so a reword cannot quietly drop
        // it. These two were the only refusals in the taxonomy carrying no outcome at all,
        // and a corpus sweep consequently reported the traces hitting them as `OK` — a
        // refusal reading as success. FR-054b case 2 covers both: a parameter whose source
        // field the trace does not carry is the model requiring something the trace was
        // never obliged to record.
        for e in [FitError::Unmeasured("think_time"), FitError::NoArrivalRate] {
            let msg = e.to_string();
            assert!(msg.contains("MODEL LIMITATION"), "{msg}");
            // Case 2 additionally requires the restriction to be named, so the message must
            // say more than which outcome it is.
            assert!(
                msg.contains("this model"),
                "must name the binding restriction: {msg}"
            );
            assert!(
                !msg.contains("CALLER INPUT") && !msg.contains("CORRUPT TRACE"),
                "exactly one outcome: {msg}"
            );
        }
    }
}
