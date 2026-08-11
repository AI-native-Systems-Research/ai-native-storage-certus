//! T038–T041: what the plan statistics claim, checked against something
//! independent of the code that produces them.
//!
//! Integration tests rather than unit tests because each is about the whole
//! pipeline — document in, statistic out. A unit test on the reuse-distance
//! accumulator can hold while the corpus model, the session model or the plan
//! writer make the stream something other than what the document asked for, and
//! it is the composition that SC-005 is a claim about.
//!
//! **SC-005** is the headline: the reuse-distance CDF reported for a pure-Zipf
//! plan matches the *analytic* reuse-distance distribution of the
//! independent-reference model. That closed form is derived here from the
//! popularity law alone, with no reference to how the generator works, so the
//! check spans the popularity sampler, key derivation, the session model, the plan
//! writer and the statistic itself.

use workload_model::plan::{flags, Generator, PlanEvent};
use workload_model::schema::Document;
use workload_model::stats::{Ref, Report, Statistics};

// ---------------------------------------------------------------------------
// The analytic side: a closed form for the independent-reference model.
// ---------------------------------------------------------------------------

/// The rank pmf the generator's Zipf sampler realises.
///
/// `dist::zipf` inverts a *continuous* approximation to the discrete Zipf CDF and
/// floors the result, so rank `k` gets the mass the true density puts on
/// `[k, k+1)`: `p_k = (H(k+1) - H(k)) / H(n)` with `H` the antiderivative of
/// `x^-s`. That is exactly integrable, so the realised pmf has a closed form even
/// though it is not the discrete Zipf pmf.
///
/// Derived from the sampler's *documented* transform rather than measured from its
/// output, so this is a statement about the law and not a restatement of the
/// samples. `the_realised_root_popularity_matches_the_analytic_pmf` checks the
/// sampler against it separately, which is what keeps the reuse-distance
/// comparison below from being circular: if the popularity law were wrong, that
/// test fails and says so, rather than this one failing for an unclear reason.
fn zipf_pmf(s: f64, n: u64) -> Vec<f64> {
    let h = |x: f64| {
        if (s - 1.0).abs() < 1e-9 {
            x.ln()
        } else {
            (x.powf(1.0 - s) - 1.0) / (1.0 - s)
        }
    };
    let hn = h(n as f64);
    (1..=n)
        .map(|k| {
            // The top rank is reachable only at u == 1, which never occurs, so
            // its mass is zero and the rest sum to one.
            (h((k + 1) as f64).min(hn) - h(k as f64)) / hn
        })
        .collect()
}

/// `P(reuse distance <= d)` under the independent-reference model, exactly.
///
/// For a reference to item `i`, let `G` be the number of references strictly
/// between it and the previous reference to `i`. Under IRM, looking backwards,
/// each preceding position is item `i` independently with probability `p_i`, so
/// `G` is geometric: `P(G = g) = p_i (1 - p_i)^g`. Given `G = g`, those references
/// are iid over the other items with `q_j = p_j / (1 - p_i)`, and the reuse
/// distance is the number of *distinct* items among them.
///
/// # Why this is inclusion–exclusion and not a Poisson-binomial
///
/// The obvious route — item `j` appears with probability `1 - (1 - q_j)^g`, so
/// convolve those Bernoullis — is wrong, and wrong in a way that looks right. The
/// appearance indicators are **negatively correlated**: the `g` draws are shared
/// between them, so one item appearing crowds out the others. That product form is
/// the classical independence approximation to the occupancy problem, and it
/// agreed with measurement to better than 0.005 from distance 3 upward while
/// being wrong by 0.043 at distance 0 — where it assigns positive probability to
/// "`g >= 1` draws happened and no item appeared", an impossible event.
///
/// The exact route runs over subsets. For `A` a set of other items,
/// `P(appeared ⊆ A | G = g) = q_A^g` with `q_A = Σ_{j∈A} q_j`, and the geometric
/// mixture of that has a closed form:
///
/// ```text
/// F(A) = Σ_g p_i (1 - p_i)^g q_A^g = p_i / (1 - (1 - p_i) q_A)
/// ```
///
/// Möbius inversion over the subset lattice then gives
/// `P(appeared = A) = Σ_{B⊆A} (-1)^(|A|-|B|) F(B)`, and summing over `|A| <= d`
/// and collecting terms by `B`:
///
/// ```text
/// P(D <= d) = Σ_{|B| <= d} F(B) · Σ_{u=0}^{d-|B|} (-1)^u C(M-|B|, u)
/// ```
///
/// with `M` the number of other items. Two values check the algebra by hand:
/// `F(∅) = p_i` gives `P(D = 0) = Σ_i p_i²`, and `F(everything) = 1` gives
/// `P(D <= M) = 1`. Both are asserted below.
///
/// Exponential in the support, which is why the fixture keeps the support small:
/// the point of this closed form is to be an independent statement of the law, not
/// to scale.
fn irm_reuse_distance_cdf(p: &[f64]) -> Vec<f64> {
    let n = p.len();
    let m = n - 1; // other items, from any one item's point of view
    let mut cdf = vec![0.0f64; m + 1];

    // coef[b][d] = Σ_{u=0}^{d-b} (-1)^u C(m-b, u), for b <= d.
    let mut binom = vec![vec![0.0f64; m + 1]; m + 1];
    for r in 0..=m {
        binom[r][0] = 1.0;
        for c in 1..=r {
            binom[r][c] = binom[r - 1][c - 1] + binom[r - 1][c];
        }
    }
    let coef = |b: usize, d: usize| -> f64 {
        (0..=(d - b))
            .map(|u| if u % 2 == 0 { 1.0 } else { -1.0 } * binom[m - b][u])
            .sum()
    };

    for (i, &pi) in p.iter().enumerate() {
        if pi <= 0.0 {
            continue;
        }
        let others: Vec<f64> = p
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, &pj)| pj / (1.0 - pi))
            .collect();
        debug_assert_eq!(others.len(), m);
        let mut per_i = vec![0.0f64; m + 1];
        for mask in 0u32..(1u32 << m) {
            let mut q = 0.0f64;
            let b = mask.count_ones() as usize;
            for (j, &qj) in others.iter().enumerate() {
                if mask & (1 << j) != 0 {
                    q += qj;
                }
            }
            let f = pi / (1.0 - (1.0 - pi) * q);
            for (d, slot) in per_i.iter_mut().enumerate().skip(b) {
                *slot += f * coef(b, d);
            }
        }
        for d in 0..=m {
            cdf[d] += pi * per_i[d];
        }
    }
    cdf
}

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

/// Roots in the pure-Zipf fixture. The top rank carries no mass (see
/// [`zipf_pmf`]), so the usable support is one less.
///
/// Small on purpose: [`irm_reuse_distance_cdf`] is exponential in the support, and
/// twelve usable roots give twelve CDF points every one of which is an exact
/// bucket bound, so the comparison carries no bucketing error at all.
const ZIPF_ROOTS: u64 = 13;
/// The Zipf exponent under test.
const ZIPF_S: f64 = 0.9;

/// A plan that is nothing but a Zipf draw over roots.
///
/// `shared_depth` and `private_depth` both zero, one turn, no growth: every
/// request is a single block, and that block is the session's root. So consecutive
/// requests are independent draws from `roots.popularity` — an
/// independent-reference model, which is the one workload shape with a closed-form
/// reuse-distance distribution.
///
/// Deliberately degenerate. The schema's own guidance calls a corpus that mints no
/// keys below the trunk a workload not worth measuring, and it is right: nothing
/// here would tell you anything about a cache. It is exactly the right shape for
/// validating the statistic, which is a different job.
fn zipf_doc(seed: u64, requests: u64) -> Document {
    let y = format!(
        r#"
version: 1
seed: {seed}
requests: {requests}
corpus:
  block_bytes: {{dist: const, value: 4096}}
  trees:
    roots: {{count: {ZIPF_ROOTS}, popularity: {{dist: zipf, s: {ZIPF_S}}}}}
    shared_depth: {{dist: const, value: 0}}
    branching: 1.0
    branch_skew: 0.0
workload:
  arrival: {{model: open_loop, rate: 10000/s, burstiness: 1.0}}
  sessions:
    turns: {{dist: const, value: 1}}
    think_time: {{dist: const, value: 0.0}}
    private_depth: {{dist: const, value: 0}}
    growth_per_turn: {{dist: const, value: 0}}
  mix:
    - {{weight: 1.0}}
run:
  mode: plan
  wss_window: {requests}
"#
    );
    Document::from_yaml(&y).expect("fixture must parse")
}

/// Every event of a plan, in order.
fn events(d: &Document) -> Vec<PlanEvent> {
    let mut g = Generator::new(d).expect("generator");
    let mut out = Vec::new();
    let mut chunk = Vec::new();
    while !g.is_done() {
        chunk.clear();
        if g.fill(&mut chunk) == 0 {
            break;
        }
        out.extend_from_slice(&chunk);
    }
    out
}

/// The report over a whole plan.
fn report(d: &Document, window: u64) -> Report {
    let mut s = Statistics::new(window);
    for e in &events(d) {
        s.push(&Ref::from(e));
    }
    s.finish()
}

/// The measured object-distance CDF, conditional on the distance being finite —
/// the same conditioning the analytic form uses.
fn measured_finite_cdf(r: &Report, upto: usize) -> Vec<f64> {
    let total = r.reuse_distance.objects.count;
    (0..upto)
        .map(|d| {
            let n: u64 = r
                .reuse_distance
                .object_buckets
                .iter()
                .filter(|(lo, _, _)| *lo <= d as u64)
                .map(|(_, _, c)| *c)
                .sum();
            n as f64 / total as f64
        })
        .collect()
}

// ---------------------------------------------------------------------------
// T038 / SC-005.
// ---------------------------------------------------------------------------

#[test]
fn the_pure_zipf_fixture_really_is_an_independent_reference_model() {
    // The premise the analytic comparison rests on. If the fixture produced more
    // than one block per request, or keys below the roots, the closed form would
    // be describing a different process and the next test would be meaningless —
    // so the premise is asserted rather than assumed.
    let ev = events(&zipf_doc(1, 20_000));
    assert!(!ev.is_empty());
    for e in &ev {
        assert_eq!(e.depth, 0, "a pure-Zipf request is one root block");
        assert!(e.has(flags::REQUEST_START) && e.has(flags::REQUEST_END));
        assert!(!e.has(flags::WARMUP), "no warmup window is configured");
    }
    let distinct: std::collections::BTreeSet<_> = ev.iter().map(|e| e.key).collect();
    assert!(
        distinct.len() <= ZIPF_ROOTS as usize,
        "{} distinct keys exceeds the root count",
        distinct.len()
    );
    assert!(distinct.len() >= 10, "too few roots drawn to be a test");
}

#[test]
fn the_realised_root_popularity_matches_the_analytic_pmf() {
    // Checked separately from the reuse-distance comparison so that a wrong
    // popularity law fails *here*, naming itself, instead of showing up as an
    // unexplained divergence in a much more derived statistic.
    let ev = events(&zipf_doc(7, 400_000));
    let mut counts = vec![0u64; ZIPF_ROOTS as usize];
    let mut by_key: std::collections::BTreeMap<u64, usize> = std::collections::BTreeMap::new();
    for e in &ev {
        let n = by_key.len();
        let idx = *by_key.entry(e.key.0).or_insert(n);
        counts[idx.min(ZIPF_ROOTS as usize - 1)] += 1;
    }
    // Ranks are recovered by frequency, since the mapping from rank to key is a
    // hash and carries no order.
    counts.sort_unstable_by(|a, b| b.cmp(a));
    let total: u64 = counts.iter().sum();
    let mut expect = zipf_pmf(ZIPF_S, ZIPF_ROOTS);
    expect.sort_unstable_by(|a, b| b.partial_cmp(a).unwrap());

    for (rank, (got, want)) in counts.iter().zip(expect.iter()).enumerate() {
        let got = *got as f64 / total as f64;
        // Absolute tolerance: the sampling error on a 400k-draw multinomial is
        // under 0.001 at every rank, so 0.005 is loose by a wide margin while
        // still catching the ~20% error a discrete-Zipf pmf would show at rank 1.
        assert!(
            (got - want).abs() < 0.005,
            "rank {} realised {got:.5} vs analytic {want:.5}",
            rank + 1
        );
    }
}

#[test]
fn the_reuse_distance_cdf_matches_the_analytic_zipf_distribution() {
    // SC-005. The comparison is a Kolmogorov–Smirnov distance evaluated at every
    // integer distance, which for distances under 32 are exact bucket bounds, so
    // no part of the gap is a bucketing artefact.
    //
    // The tolerance is 0.02, chosen here and **provisional**: FR-057a's per
    // statistic defaults are owed by task T075 and are not derived yet.
    //
    // The margin is wide. Measured 2026-08-11 the KS distance is **0.0013**, at
    // distance 7 — fifteen times inside the tolerance, and close to the 0.0022
    // sampling error 1.36/sqrt(400 000) would give for iid samples. So the
    // tolerance is loose enough not to be flaky and still an order of magnitude
    // tighter than the divergences the two derivation errors found while writing
    // this produced: 0.043 from the Poisson-binomial form, and 0.45 from the
    // generator's off-by-one path length.
    const TOLERANCE: f64 = 0.02;

    // Only the ranks that carry mass take part: the top rank's is zero, and a
    // zero-probability item is not one of the "others" any distance can count.
    let pmf: Vec<f64> = zipf_pmf(ZIPF_S, ZIPF_ROOTS)
        .into_iter()
        .filter(|p| *p > 0.0)
        .collect();
    let analytic = irm_reuse_distance_cdf(&pmf);
    let m = analytic.len();

    // The two hand-checkable values of the closed form, asserted before it is
    // used as a reference: an arithmetic slip in the inclusion–exclusion would
    // otherwise show up as a divergence blamed on the generator.
    let sum_sq: f64 = pmf.iter().map(|p| p * p).sum();
    assert!(
        (analytic[0] - sum_sq).abs() < 1e-9,
        "P(D = 0) should be the collision probability {sum_sq:.6}, got {:.6}",
        analytic[0]
    );
    assert!(
        (analytic[m - 1] - 1.0).abs() < 1e-9,
        "the CDF should reach 1 at the full support, got {:.9}",
        analytic[m - 1]
    );

    let r = report(&zipf_doc(20_260_811, 400_000), 400_000);
    let measured = measured_finite_cdf(&r, m);

    let mut worst = (0usize, 0.0f64);
    for d in 0..m {
        let gap = (measured[d] - analytic[d]).abs();
        if gap > worst.1 {
            worst = (d, gap);
        }
    }
    assert!(
        worst.1 < TOLERANCE,
        "KS distance {:.4} at distance {} exceeds {TOLERANCE}\n  measured {:?}\n  analytic {:?}",
        worst.1,
        worst.0,
        measured
            .iter()
            .map(|v| format!("{v:.4}"))
            .collect::<Vec<_>>(),
        analytic
            .iter()
            .map(|v| format!("{v:.4}"))
            .collect::<Vec<_>>(),
    );

    // And the distribution is genuinely spread over the support rather than
    // concentrated, which is what makes the agreement above informative.
    assert!(measured[0] < 0.35, "distance 0 should not dominate");
    assert!(measured[m - 2] > 0.95, "the support should be covered");
}

// ---------------------------------------------------------------------------
// T039 / FR-034a, FR-034, FR-034b.
// ---------------------------------------------------------------------------

#[test]
fn the_floor_is_the_miss_rate_at_unbounded_capacity_over_a_real_plan() {
    // The identity holds by construction in the accumulators; this checks it
    // survives a real plan, where warmup, multi-block requests and repeated keys
    // all interact.
    let r = report(&sharing_doc(3, "12s", 0.0), 20_000);
    let floor = r.floor.per_object.expect("a measured floor");
    let unbounded_miss = 1.0 - r.reuse_distance.fraction_within_objects(u64::MAX / 2);
    assert!(
        (floor - unbounded_miss).abs() < 1e-12,
        "floor {floor} != unbounded-capacity miss rate {unbounded_miss}"
    );
    assert!(floor > 0.0 && floor < 1.0, "floor {floor} is degenerate");
}

#[test]
fn no_statistic_takes_a_capacity_and_no_report_contains_a_hit_rate() {
    // FR-034 and FR-034b as a structural check rather than a promise. The whole
    // accumulator is constructed from a *request count* — there is no capacity
    // argument to pass — and the serialised report is searched for the vocabulary
    // of the things this crate must not publish.
    let r = report(&sharing_doc(4, "12s", 0.0), 20_000);
    let json = r.to_json().expect("json");
    for forbidden in [
        "capacity",
        "hit_rate",
        "hit rate",
        "eviction",
        "belady",
        "opt_hit",
        "promotion",
        "tier",
    ] {
        assert!(
            !json.contains(forbidden),
            "the report published {forbidden:?}, which needs a model of the consumer's internals"
        );
    }
    // The floor is present, since it is the part of the story that survives
    // without a capacity.
    assert!(json.contains("compulsory_misses"));
}

// ---------------------------------------------------------------------------
// T040 / FR-036, FR-062.
// ---------------------------------------------------------------------------

#[test]
fn the_same_plan_consumed_twice_yields_the_same_stream_digest() {
    // FR-036 is the generator's whole contribution to a comparison's validity: two
    // arms can be *proven* to have seen the identical stream, whoever ran them.
    let a = digest_of(&sharing_doc(11, "8s", 0.0));
    let b = digest_of(&sharing_doc(11, "8s", 0.0));
    assert_eq!(a, b, "the same document and seed must digest identically");
    assert!(a.starts_with("blake3:"));
    assert!(Report::refuse_unless_same_stream(&a, &b).is_ok());
}

#[test]
fn a_comparison_between_arms_with_differing_digests_is_refused() {
    // FR-062. A different seed is a different sample of the same workload — a
    // perfectly legitimate plan, and precisely the case where a hit-rate
    // comparison would look reasonable and mean nothing.
    let a = digest_of(&sharing_doc(11, "8s", 0.0));
    let b = digest_of(&sharing_doc(12, "8s", 0.0));
    assert_ne!(a, b);
    let e = Report::refuse_unless_same_stream(&a, &b).expect_err("must refuse");
    assert_eq!(e.requirement, "FR-062");
    assert!(e.message.contains("did not consume the same key sequence"));
}

/// The stream digest a plan encodes.
fn digest_of(d: &Document) -> String {
    use workload_model::plan::digest::StreamDigest;
    let mut sd = StreamDigest::new();
    for e in &events(d) {
        sd.push(e.key);
    }
    sd.finish()
}

// ---------------------------------------------------------------------------
// T041: a scan-shaped mixture entry, and the bimodality it puts in the CDF.
// ---------------------------------------------------------------------------

/// A workload with a shared trunk and, optionally, a scan-shaped mixture entry.
///
/// `scan_weight` 0.0 is the control. The scan entry is long-document ingest: one
/// very deep private path per session, re-read on a second turn after a long
/// think. Its re-reads therefore sit at a reuse distance of everything the rest of
/// the workload referenced during that think, which is orders of magnitude beyond
/// the distance at which the shared trunk is re-read.
fn sharing_doc(seed: u64, duration: &str, scan_weight: f64) -> Document {
    let sharing_weight = 1.0 - scan_weight;
    let scan = if scan_weight > 0.0 {
        format!(
            "    - {{weight: {scan_weight}, turns: {{dist: const, value: 2}}, \
             think_time: {{dist: const, value: 2.0}}, \
             private_depth: {{dist: const, value: 400}}, \
             growth_per_turn: {{dist: const, value: 0}}}}\n"
        )
    } else {
        String::new()
    };
    let y = format!(
        r#"
version: 1
seed: {seed}
duration: {duration}
corpus:
  block_bytes: {{dist: const, value: 131072}}
  trees:
    roots: {{count: 4, popularity: {{dist: zipf, s: 0.9}}}}
    shared_depth: {{dist: const, value: 3}}
    branching: 1.0
    branch_skew: 0.5
workload:
  arrival: {{model: open_loop, rate: 500/s, burstiness: 1.0}}
  sessions:
    turns: {{dist: const, value: 4}}
    think_time: {{dist: const, value: 0.02}}
    private_depth: {{dist: const, value: 2}}
    growth_per_turn: {{dist: const, value: 1}}
  mix:
    - {{weight: {sharing_weight}}}
{scan}run:
  mode: plan
  wss_window: 20000
"#
    );
    Document::from_yaml(&y).expect("fixture must parse")
}

/// The widest run of distances over which the CDF gains almost nothing, as
/// `(low, high, gain)`.
///
/// A gap like this is what bimodality looks like on a CDF: mass on either side and
/// a stretch of distances in between that essentially nothing lands in. Measured
/// on the CDF rather than by hunting for peaks in a histogram, because the peaks
/// of a log-bucketed histogram move with the bucket widths and the CDF's plateaux
/// do not.
fn widest_plateau(cdf: &[(u64, f64)]) -> (u64, u64, f64) {
    let mut best = (0u64, 0u64, 0.0f64);
    let mut best_ratio = 1.0f64;
    for i in 0..cdf.len() {
        for j in i + 1..cdf.len() {
            let gain = cdf[j].1 - cdf[i].1;
            if gain > 0.05 {
                break;
            }
            // Only count a plateau with real mass on both sides of it.
            if cdf[i].1 < 0.15 || cdf[j].1 > 0.90 {
                continue;
            }
            let ratio = cdf[j].0.max(1) as f64 / cdf[i].0.max(1) as f64;
            if ratio > best_ratio {
                best_ratio = ratio;
                best = (cdf[i].0, cdf[j].0, gain);
            }
        }
    }
    best
}

/// The measured CDF at each non-empty bucket's upper bound.
fn cdf_points(r: &Report) -> Vec<(u64, f64)> {
    let total = r.reuse_distance.objects.count;
    let mut acc = 0u64;
    r.reuse_distance
        .object_buckets
        .iter()
        .map(|(_, hi, c)| {
            acc += c;
            (*hi, acc as f64 / total as f64)
        })
        .collect()
}

#[test]
fn a_scan_shaped_mixture_entry_puts_a_second_mode_in_the_reuse_distance_cdf() {
    // T041. The control matters as much as the case: a plateau that were present
    // without the scan entry would say nothing about the scan entry.
    let control = report(&sharing_doc(21, "20s", 0.0), 20_000);
    let mixed = report(&sharing_doc(21, "20s", 0.25), 20_000);

    let (c_lo, c_hi, _) = widest_plateau(&cdf_points(&control));
    let (m_lo, m_hi, gain) = widest_plateau(&cdf_points(&mixed));

    let control_ratio = if c_lo == 0 {
        1.0
    } else {
        c_hi as f64 / c_lo as f64
    };
    let mixed_ratio = if m_lo == 0 {
        1.0
    } else {
        m_hi as f64 / m_lo as f64
    };

    assert!(
        mixed_ratio >= 8.0,
        "expected a wide plateau with the scan entry, found {m_lo}..{m_hi} (gain {gain:.3})"
    );
    assert!(
        mixed_ratio > 4.0 * control_ratio,
        "the plateau is not attributable to the scan entry: mixed {m_lo}..{m_hi} \
         (ratio {mixed_ratio:.1}) against control {c_lo}..{c_hi} (ratio {control_ratio:.1})"
    );

    // The two modes are the two archetypes, so the mixture's request-length
    // distribution should be visibly bimodal too — a cross-check that the plateau
    // came from the mixture rather than from a timing artefact.
    let short = mixed.request_length.blocks.p50.expect("p50");
    let long = mixed.request_length.blocks.max.expect("max");
    assert!(
        long >= 20 * short.max(1),
        "the mixture's request lengths are not bimodal: p50 {short}, max {long}"
    );
}
