//! The fidelity loop: **hit rate against cache size**, the synthetic plan measured
//! beside the trace it was fitted from.
//!
//! # Why this measurement exists
//!
//! Every statistic `fit` gates on is a **marginal** — one axis of the reference
//! stream, collapsed. `SC-002` asserts that matching the reuse-distance CDF means
//! "LRU hit rate agrees at every capacity under test", and `SC-010a` calls that CDF
//! a capacity-free object that *encodes* the achievable hit-rate curve. Both are
//! claims about an implication, and neither has ever been checked. The FR-057c floor
//! work gives specific reason to doubt them: reuse distance's whole discriminating
//! band on `tau2_airline` is 0.007–0.012 wide while the tolerance gating it is 0.02,
//! so a model can sit inside tolerance while being further from the trace than a
//! *different workload* is. A curve comparison answers the question the marginals
//! only gesture at: run both streams through one cache at one capacity and see
//! whether they convert reuse into hits at the same rate.
//!
//! # Why this is not the deferred cache simulator
//!
//! The spec's Out of Scope entry defers cache simulation for three reasons, and none
//! of them is crossed here:
//!
//! 1. *A workload must not be specified in terms of a consumer's internals
//!    (FR-018a).* Nothing in the schema, in `workload-model`, or in any fitted
//!    parameter changes. `workload-model` still depends on no `interfaces`, no
//!    `IEvictionPolicy` and no policy component, and still has no concept of a tier,
//!    a cache, memory or disk. This module is in the *tool*, and what it produces is
//!    a statement about a **fit**, not a property of a workload.
//! 2. *A realistic simulator would have to share Certus's evolving replacement code.*
//!    It does share it, deliberately and in the consumer-side direction: the policy
//!    is a real component consumed through `IEvictionPolicy` and named in the report.
//!    Nothing is reimplemented here, so there is no second copy to drift. If the
//!    policy changes, this number changes — which is correct for a measurement taken
//!    through a named instrument.
//! 3. *A disk tier has nowhere to live except real disks.* There is no tier here at
//!    all. The cache is a bounded set of blocks, capacity counted in blocks, and the
//!    only quantity reported is a hit **fraction**. No latency, no bandwidth, no
//!    queueing, no promotion, no byte accounting — the three things that made a
//!    simulated SSD's error unbounded are all absent.
//!
//! So the instrument is the existing `apps/eviction-replay-benchmark` simulator, used
//! as-is on both arms. Using one implementation for both is not tidiness: two would
//! make a curve comparison a comparison of two definitions of a hit — the same
//! failure `workload_model::stats` exists to prevent for the marginals.
//!
//! # The two decisions that decide whether the comparison means anything
//!
//! * **Order.** A cache is recency-sensitive, so both arms are replayed in **arrival
//!   order** — the trace in its own invocation order, the plan in emission order
//!   (`t_ns` is non-decreasing). Deliberately *not* the session-contiguous order the
//!   segment census needs: grouping a session's requests together would hand both
//!   arms a locality no consumer ever sees. A trace whose order is file order rather
//!   than chronological (FR-055d) makes the curve order-dependent, and the report
//!   says so rather than quietly comparing.
//! * **Session identity.** `BlockSemantics::session_id` is what a lineage-aware
//!   policy consumes, and `eviction-policy-session-lists` chains a session's blocks
//!   and evicts only leaves. The trace's session is one conversation, all its turns;
//!   the plan's `session_id` is the same thing. So the mapping is session→session.
//!   It is emphatically **not** `root_index`: a shared-prefix family is a set of
//!   *different* conversations that happen to share a trunk, and feeding it would
//!   give the synthetic arm lineage chains the real arm has no counterpart for —
//!   producing a plausible curve from an incomparable pair.

use component_core::query_interface;
use eviction_replay_benchmark::replay::{Op, Trace as SimTrace};
use eviction_replay_benchmark::sim::{simulate, SimStats};
use interfaces::IEvictionPolicy;
use workload_model::plan::{Generator, PlanEvent};
use workload_model::schema::Document;

use crate::read::NormalisedInvocation;

/// Which policy component to measure both arms through.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Policy {
    /// Recency-only LRU (`eviction-policy-lru`).
    Lru,
    /// Session-lineage, leaf-only eviction (`eviction-policy-session-lists`).
    SessionLists,
    /// Both, side by side.
    Both,
}

impl Policy {
    /// The components this selection expands to.
    fn kinds(self) -> &'static [Kind] {
        match self {
            Policy::Lru => &[Kind::Lru],
            Policy::SessionLists => &[Kind::SessionLists],
            Policy::Both => &[Kind::Lru, Kind::SessionLists],
        }
    }
}

/// One policy component.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Kind {
    Lru,
    SessionLists,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Lru => "lru",
            Kind::SessionLists => "session-lists",
        }
    }

    /// A fresh component instance per run, so no state crosses cache sizes or arms.
    fn run(self, trace: &SimTrace, cache_size: usize) -> SimStats {
        match self {
            Kind::Lru => {
                let comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
                let ep = query_interface!(comp, IEvictionPolicy)
                    .expect("eviction-policy-lru provides IEvictionPolicy");
                simulate(&*ep, trace, cache_size)
            }
            Kind::SessionLists => {
                let comp =
                    eviction_policy_session_lists::EvictionPolicySessionListsComponent::new_default(
                    );
                let ep = query_interface!(comp, IEvictionPolicy)
                    .expect("eviction-policy-session-lists provides IEvictionPolicy");
                simulate(&*ep, trace, cache_size)
            }
        }
    }
}

/// The real arm: one op per invocation, in the trace's own order.
///
/// Takes the invocation slice rather than the [`crate::read::Trace`] so that the
/// mapping is testable without a manifest, and so that the *only* thing this
/// function can depend on is what a trace and a plan both have.
pub fn arm_from_trace(invocations: &[NormalisedInvocation]) -> SimTrace {
    let mut ops = Vec::with_capacity(invocations.len());
    for inv in invocations {
        if inv.blocks.is_empty() {
            continue;
        }
        ops.push(Op {
            method: "request".into(),
            keys: inv.blocks.iter().map(|k| k.0).collect(),
            session_id: u64::from(inv.session.0),
        });
    }
    finish(ops)
}

/// The synthetic arm: one op per plan request, in emission order.
///
/// Warmup requests are dropped, and the count returned. A warmup window is a
/// property of a measured run rather than of a workload (FR-045) and a trace has
/// none, so replaying the plan's would charge the synthetic arm compulsory misses
/// the real arm never pays. Dropping the requests entirely — rather than replaying
/// them and starting the measurement warm — is the mirror of a trace, which begins
/// cold. Fitted documents declare no warmup, so this is a no-op in practice and a
/// guard against a document that does.
pub fn arm_from_events(events: &[PlanEvent]) -> (SimTrace, u64) {
    use workload_model::plan::record::flags;
    use workload_model::trace::requests;

    let mut ops = Vec::new();
    let mut warmup = 0u64;
    for r in requests(events) {
        if r.first().is_some_and(|e| e.has(flags::WARMUP)) {
            warmup += 1;
            continue;
        }
        let Some(first) = r.first() else { continue };
        ops.push(Op {
            method: "request".into(),
            keys: r.iter().map(|e| e.key.0).collect(),
            session_id: u64::from(first.session_id.0),
        });
    }
    (finish(ops), warmup)
}

/// Fill in the two derived totals a [`SimTrace`] carries.
fn finish(ops: Vec<Op>) -> SimTrace {
    let mut distinct = std::collections::HashSet::new();
    let mut refs = 0usize;
    for op in &ops {
        refs += op.keys.len();
        for &k in &op.keys {
            distinct.insert(k);
        }
    }
    SimTrace {
        ops,
        distinct_keys: distinct.len(),
        total_key_refs: refs,
    }
}

/// Fractions of the working set the default sweep visits.
///
/// Geometric over three decades, because a hit-rate curve is flat at both ends and
/// all of its shape is in the middle; a linear sweep spends most of its runs where
/// nothing happens. The last point is the whole working set, where the cache cannot
/// evict — see [`unbounded_hit_rate`], which the run at that size must reproduce.
const SWEEP: [f64; 6] = [1.0 / 1024.0, 1.0 / 256.0, 1.0 / 64.0, 1.0 / 16.0, 0.25, 1.0];

/// The cache sizes to sweep, in blocks and ascending.
///
/// Derived from the **trace's** working set, not each arm's own, so that both arms
/// are asked the same operational question: at this many blocks, what fraction of
/// references hit? Sizing each arm by its own working set would compare two
/// different capacities and call the difference fidelity.
pub fn sizes(working_set: usize, explicit: &[usize]) -> Vec<usize> {
    if !explicit.is_empty() {
        let mut v: Vec<usize> = explicit.iter().copied().filter(|&n| n >= 1).collect();
        v.sort_unstable();
        v.dedup();
        return v;
    }
    let mut v: Vec<usize> = SWEEP
        .iter()
        .map(|f| ((working_set as f64 * f).round() as usize).max(1))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Hit rate an unbounded cache would report: every reference after the first hits.
///
/// The arithmetic ceiling of any curve, and worth printing on its own because it
/// **decomposes** the comparison. It is a function of reuse *quantity* alone
/// (`1 - distinct/references`), so if the two arms disagree here the synthetic
/// stream has the wrong amount of reuse and no capacity can hide it; if they agree
/// here and differ below, the reuse quantity is right and its *arrangement* — which
/// references land close enough together to survive — is wrong. The marginals cannot
/// separate those two failures, and they want different fixes.
pub fn unbounded_hit_rate(t: &SimTrace) -> f64 {
    if t.total_key_refs == 0 {
        return 0.0;
    }
    (t.total_key_refs - t.distinct_keys) as f64 / t.total_key_refs as f64
}

/// Generate the plan a document describes, into memory.
///
/// Regenerated here rather than buffered during the fit's search for the same reason
/// `print_structure_diff` regenerates: only the winning candidate is measured, and
/// holding every event of every candidate would cost hundreds of megabytes per
/// iteration.
fn generate(doc: &Document) -> Result<Vec<PlanEvent>, String> {
    let mut g = Generator::new(doc).map_err(|e| e.to_string())?;
    let mut plan: Vec<PlanEvent> = Vec::new();
    let mut chunk: Vec<PlanEvent> = Vec::new();
    while !g.is_done() {
        chunk.clear();
        if g.fill(&mut chunk) == 0 {
            break;
        }
        plan.extend_from_slice(&chunk);
    }
    Ok(plan)
}

/// One swept point: the same capacity, both arms, one policy.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    /// Capacity in blocks.
    pub cache_size: usize,
    /// Trace-arm hit rate in `[0, 1]`.
    pub trace: f64,
    /// Synthetic-arm hit rate in `[0, 1]`.
    pub plan: f64,
}

impl Point {
    /// Synthetic minus trace, in percentage points. Positive means the synthetic
    /// stream is *easier* to cache than the workload it claims to model.
    pub fn delta_points(&self) -> f64 {
        (self.plan - self.trace) * 100.0
    }
}

/// How much the **trace's own** hit rate moves across the sweep, in percentage points.
///
/// What capacity buys on this workload, and therefore the yardstick a gap has to be read
/// against.
fn trace_span(points: &[Point]) -> f64 {
    let (lo, hi) = points.iter().fold((f64::MAX, f64::MIN), |(l, h), p| {
        (l.min(p.trace * 100.0), h.max(p.trace * 100.0))
    });
    if points.is_empty() {
        0.0
    } else {
        hi - lo
    }
}

/// Span below which the curve carries no information and the ceiling must be read instead.
///
/// One percentage point: at that flatness the sweep's endpoints are within the width of the
/// numbers printed, so a ratio against the span is arithmetic on noise.
const FLAT_CURVE_POINTS: f64 = 1.0;

/// The sentence that says how to read `worst` — as a multiple of the span, or not at all.
///
/// A gap in percentage points is meaningless without the span of the trace's own curve
/// (FR-057d). Measured on `wildchat`, the corpus's **smallest** absolute gap (2.5 points)
/// is its **largest** by span, because capacity moves its hit rate by only 0.8 points end
/// to end — so the ratio is what compares across traces. Below [`FLAT_CURVE_POINTS`] the
/// ratio is meaningless too, and the reader is sent to the unbounded ceiling instead: on a
/// flat curve the question is not what capacity buys but whether the stream carries the
/// right amount of reuse at all.
fn span_note(span: f64, worst: f64) -> String {
    if span < FLAT_CURVE_POINTS {
        format!(
            "the trace's own hit rate moves only {span:.1} points across this whole sweep, \
             so\n                   capacity buys it almost nothing and the gap is a LEVEL \
             offset — judge it on hit@ws, not on this curve"
        )
    } else {
        format!(
            "the trace's own curve spans {span:.1} points, so the worst gap is {:.2}x what \
             capacity\n                   buys on this workload — the figure to compare across \
             traces, since points do not compare",
            worst / span
        )
    }
}

/// Sweep one policy over `at` on both arms.
fn sweep(kind: Kind, arms: (&SimTrace, &SimTrace), at: &[usize]) -> Vec<Point> {
    at.iter()
        .map(|&n| Point {
            cache_size: n,
            trace: kind.run(arms.0, n).hit_rate(),
            plan: kind.run(arms.1, n).hit_rate(),
        })
        .collect()
}

/// Measure and print the curve comparison for a fitted document against its source.
///
/// Returns nothing rather than a summary scalar, deliberately. Nothing gates on this
/// yet — FR-057c's rule is that a statistic must be justified against an achievable
/// floor first, and this one has no floor measured — and a returned number no caller
/// reads is the same defect as a fitted parameter nothing consumes: it looks
/// load-bearing while being ignored. The per-policy mean and worst are printed, which
/// is what a reader and a corpus sweep both actually consume.
pub fn print_cache_curve(
    invocations: &[NormalisedInvocation],
    chronological: bool,
    doc: &Document,
    explicit_sizes: &[usize],
    policy: Policy,
) -> Result<(), String> {
    let trace_arm = arm_from_trace(invocations);
    let (plan_arm, warmup) = arm_from_events(&generate(doc)?);
    if trace_arm.ops.is_empty() || plan_arm.ops.is_empty() {
        return Err("cache curve: an arm has no key-bearing requests".into());
    }
    let at = sizes(trace_arm.distinct_keys, explicit_sizes);

    println!(
        "\n  cache curve — hit rate against capacity, the SYNTHETIC plan beside its source trace\n         \
         The four gated statistics are marginals; what a KV cache does is convert reuse into hits\n         \
         at a capacity. SC-002 asserts that matching the reuse-distance CDF makes LRU hit rate\n         \
         agree at every capacity — this measures that implication instead of assuming it. Both arms\n         \
         go through the same policy COMPONENT in arrival order, so a hit means one thing.\n         \
         hit@ws is the unbounded-cache ceiling (1 - distinct/refs): it depends on how MUCH reuse a\n         \
         stream has and not on where it sits, so a gap there is a different defect from a gap below."
    );
    println!(
        "    {:<10} {:>10} {:>12} {:>12} {:>9}",
        "arm", "requests", "accesses", "working set", "hit@ws"
    );
    for (label, arm) in [("trace", &trace_arm), ("synthetic", &plan_arm)] {
        println!(
            "    {:<10} {:>10} {:>12} {:>12} {:>8.1}%",
            label,
            arm.ops.len(),
            arm.total_key_refs,
            arm.distinct_keys,
            unbounded_hit_rate(arm) * 100.0
        );
    }
    let scale = plan_arm.total_key_refs as f64 / trace_arm.total_key_refs.max(1) as f64;
    println!(
        "    the synthetic arm carries {scale:.3}x the trace's references and {:.3}x its working \
         set;\n    the sweep is sized from the TRACE's working set, so both arms answer the same \
         capacity question",
        plan_arm.distinct_keys as f64 / trace_arm.distinct_keys.max(1) as f64
    );
    if warmup > 0 {
        println!(
            "    note: {warmup} warmup requests were not replayed — a trace has no warmup window, \
             so replaying the plan's would charge it misses the trace never pays"
        );
    }
    if !chronological {
        println!(
            "    ORDER-DEPENDENT (FR-055d): this trace carries no usable timestamps, so its arm is \
             replayed in\n    FILE order. A cache is recency-sensitive, so this curve is a property \
             of that order as much as of the workload"
        );
    }

    println!(
        "\n    {:<14} {:>10} {:>9} {:>9} {:>9} {:>9}",
        "policy", "cache", "frac ws", "hit% T", "hit% S", "delta"
    );
    for kind in policy.kinds() {
        let points = sweep(*kind, (&trace_arm, &plan_arm), &at);
        let mut sum = 0.0;
        let mut kind_worst = (0.0f64, 0usize);
        for p in &points {
            let d = p.delta_points();
            sum += d.abs();
            if d.abs() > kind_worst.0 {
                kind_worst = (d.abs(), p.cache_size);
            }
            println!(
                "    {:<14} {:>10} {:>9.4} {:>8.1}% {:>8.1}% {:>+8.1}",
                kind.label(),
                p.cache_size,
                p.cache_size as f64 / trace_arm.distinct_keys.max(1) as f64,
                p.trace * 100.0,
                p.plan * 100.0,
                d
            );
        }
        println!(
            "    {:<14} mean |delta| {:.1} points, worst {:.1} points at cache {}",
            kind.label(),
            sum / points.len() as f64,
            kind_worst.0,
            kind_worst.1
        );
        println!(
            "    {:<14} {}",
            kind.label(),
            span_note(trace_span(&points), kind_worst.0)
        );
    }
    println!(
        "    NOT GATED: FR-057c's rule is that a statistic must be justified against a measured\n    \
         achievable floor before it may gate, and this curve has none yet — two halves of one real\n    \
         trace have to be swept the same way first. Read it as evidence, not as a verdict."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use workload_model::keys::{CacheKey, SessionId};
    use workload_model::plan::record::flags;

    /// A plan event, with only the fields this module reads set meaningfully.
    fn ev(key: u64, session: u32, request: u32, f: u8) -> PlanEvent {
        PlanEvent {
            t_ns: u64::from(request),
            key: CacheKey(key),
            size: 4096,
            request_id: request,
            session_id: SessionId(session),
            depth: 0,
            turn: 1,
            node: 0,
            mix_index: 0,
            flags: f,
        }
    }

    fn inv(session: u32, turn: u32, blocks: &[u64]) -> NormalisedInvocation {
        NormalisedInvocation {
            session: SessionId(session),
            turn,
            request_start: None,
            blocks: blocks.iter().map(|&k| CacheKey(k)).collect(),
        }
    }

    #[test]
    fn the_synthetic_arm_keeps_the_plan_s_own_arrival_order() {
        // Two sessions interleaved, which is what a plan emits and what a cache sees.
        // If this were regrouped by session the second request's keys would be adjacent
        // to the first's, and every recency-sensitive number below would change.
        let events = vec![
            ev(10, 1, 0, flags::REQUEST_START),
            ev(11, 1, 0, flags::REQUEST_END),
            ev(20, 2, 1, flags::REQUEST_START),
            ev(21, 2, 1, flags::REQUEST_END),
            ev(10, 1, 2, flags::REQUEST_START),
            ev(12, 1, 2, flags::REQUEST_END),
        ];
        let (arm, warmup) = arm_from_events(&events);
        assert_eq!(warmup, 0);
        assert_eq!(arm.ops.len(), 3);
        assert_eq!(arm.ops[0].keys, vec![10, 11]);
        assert_eq!(arm.ops[1].keys, vec![20, 21]);
        assert_eq!(arm.ops[2].keys, vec![10, 12]);
        assert_eq!(
            arm.ops.iter().map(|o| o.session_id).collect::<Vec<_>>(),
            vec![1, 2, 1],
            "session identity is the conversation, carried through unchanged"
        );
        assert_eq!(arm.total_key_refs, 6);
        assert_eq!(arm.distinct_keys, 5);
    }

    #[test]
    fn warmup_requests_are_dropped_and_counted() {
        let events = vec![
            ev(1, 1, 0, flags::REQUEST_START | flags::WARMUP),
            ev(2, 1, 0, flags::REQUEST_END | flags::WARMUP),
            ev(1, 1, 1, flags::REQUEST_START),
            ev(2, 1, 1, flags::REQUEST_END),
        ];
        let (arm, warmup) = arm_from_events(&events);
        assert_eq!(warmup, 1);
        assert_eq!(arm.ops.len(), 1);
        // The warmup pass left nothing resident: its keys are first touches here, so the
        // arm begins cold exactly as a trace does.
        assert_eq!(arm.total_key_refs, 2);
        assert_eq!(arm.distinct_keys, 2);
    }

    #[test]
    fn the_trace_arm_carries_blocks_and_sessions_through_in_trace_order() {
        let invs = vec![inv(7, 0, &[1, 2, 3]), inv(8, 0, &[]), inv(7, 1, &[1, 2, 4])];
        let arm = arm_from_trace(&invs);
        assert_eq!(
            arm.ops.len(),
            2,
            "an invocation with no blocks is not an op"
        );
        assert_eq!(arm.ops[0].keys, vec![1, 2, 3]);
        assert_eq!(arm.ops[1].keys, vec![1, 2, 4]);
        assert!(arm.ops.iter().all(|o| o.session_id == 7));
        assert_eq!(arm.total_key_refs, 6);
        assert_eq!(arm.distinct_keys, 4);
    }

    #[test]
    fn the_sweep_is_geometric_ascending_and_ends_at_the_whole_working_set() {
        let v = sizes(4096, &[]);
        assert_eq!(v, vec![4, 16, 64, 256, 1024, 4096]);
        assert!(v.windows(2).all(|w| w[0] < w[1]));
        // A tiny working set collapses the low end onto 1 rather than onto 0, and the
        // duplicates are removed rather than run twice.
        let small = sizes(3, &[]);
        assert_eq!(small, vec![1, 3]);
        assert!(!small.contains(&0));
        // An explicit list wins, is sorted, and drops a zero capacity the simulator
        // would assert on.
        assert_eq!(sizes(4096, &[64, 0, 8, 64]), vec![8, 64]);
    }

    #[test]
    fn at_the_whole_working_set_the_curve_reaches_the_unbounded_ceiling() {
        // The identity that ties the two halves of the report together: a cache holding
        // the whole working set cannot evict, so its hit rate must be exactly
        // 1 - distinct/refs. Measured through the real component, so it also checks that
        // the arm and the simulator agree about what an access is.
        let events = vec![
            ev(1, 1, 0, flags::REQUEST_START),
            ev(2, 1, 0, flags::REQUEST_END),
            ev(1, 1, 1, flags::REQUEST_START),
            ev(2, 1, 1, flags::REQUEST_END),
            ev(3, 2, 2, flags::REQUEST_START),
            ev(1, 2, 2, flags::REQUEST_END),
        ];
        let (arm, _) = arm_from_events(&events);
        assert_eq!(arm.distinct_keys, 3);
        assert_eq!(arm.total_key_refs, 6);
        let s = Kind::Lru.run(&arm, arm.distinct_keys);
        assert_eq!(s.evictions, 0);
        assert!(
            (s.hit_rate() - unbounded_hit_rate(&arm)).abs() < 1e-12,
            "hit rate {} against ceiling {}",
            s.hit_rate(),
            unbounded_hit_rate(&arm)
        );
        // And a cache of one block cannot hold a working set of three, so it must be
        // strictly worse — the curve has somewhere to rise from.
        assert!(Kind::Lru.run(&arm, 1).hit_rate() < s.hit_rate());
    }

    #[test]
    fn a_gap_is_reported_against_the_span_and_a_flat_curve_is_sent_to_the_ceiling() {
        let pts = |v: &[f64]| -> Vec<Point> {
            v.iter()
                .enumerate()
                .map(|(i, &t)| Point {
                    cache_size: i + 1,
                    trace: t,
                    plan: t,
                })
                .collect()
        };
        // airline's real shape: 6.5% at the smallest capacity to 95.3% at the largest.
        let span = trace_span(&pts(&[0.065, 0.545, 0.921, 0.952, 0.953, 0.953]));
        assert!((span - 88.8).abs() < 0.05, "span was {span}");
        let note = span_note(span, 7.5);
        assert!(note.contains("0.08x"), "{note}");

        // wildchat's real shape: 73.8% to 74.6%, a 0.8-point span. The absolute gap is the
        // SMALLEST in the corpus and the ratio would be the largest, so the note must send
        // the reader to the ceiling rather than print 3.13x as if it were a curve result.
        let flat = trace_span(&pts(&[0.738, 0.742, 0.745, 0.746, 0.746, 0.746]));
        assert!(flat < FLAT_CURVE_POINTS, "span was {flat}");
        let flat_note = span_note(flat, 2.5);
        assert!(flat_note.contains("LEVEL offset"), "{flat_note}");
        assert!(flat_note.contains("hit@ws"), "{flat_note}");
        assert!(
            !flat_note.contains('x'),
            "no ratio on a flat curve: {flat_note}"
        );
    }

    #[test]
    fn a_lineage_policy_reads_the_session_the_arm_supplies() {
        // The mapping decision is load-bearing only if the policy actually reads it, so
        // assert that it does. The two arms below are the SAME key stream and differ only
        // in how sessions are attributed: one long conversation, versus one conversation
        // per request. That is exactly the difference between feeding `session_id` and
        // feeding something coarser or finer, so the error is measurable rather than
        // stylistic.
        //
        // Six distinct blocks then a revisit of the first two, through a 3-block cache:
        // * under ONE session the blocks form one chain and only its newest member is a
        //   leaf, so the two oldest are pinned by their descendants and the revisit hits;
        // * under a session per request every block is its own chain and therefore always
        //   a leaf, so the policy degenerates to recency and the revisit misses.
        let stream: Vec<u64> = vec![1, 2, 3, 4, 5, 6, 1, 2];
        let arm = |session_of: &dyn Fn(u32) -> u32| {
            let events: Vec<PlanEvent> = stream
                .iter()
                .enumerate()
                .map(|(r, &k)| {
                    ev(
                        k,
                        session_of(r as u32),
                        r as u32,
                        flags::REQUEST_START | flags::REQUEST_END,
                    )
                })
                .collect();
            arm_from_events(&events).0
        };
        let one_session = arm(&|_| 1);
        let per_request = arm(&|r| r);
        assert_eq!(one_session.total_key_refs, per_request.total_key_refs);
        assert_eq!(one_session.distinct_keys, per_request.distinct_keys);
        assert_eq!(
            Kind::Lru.run(&one_session, 3).hits,
            Kind::Lru.run(&per_request, 3).hits,
            "a recency-only policy cannot see the session"
        );
        assert_eq!(
            Kind::SessionLists.run(&one_session, 3).hits,
            2,
            "the two oldest blocks are interior to the chain, so the revisit hits"
        );
        assert_eq!(
            Kind::SessionLists.run(&per_request, 3).hits,
            0,
            "singleton chains are all leaves, so the revisit finds them evicted"
        );
    }
}
