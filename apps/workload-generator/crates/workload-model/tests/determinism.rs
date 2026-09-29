//! T032 — the plan is byte-identical at a fixed seed and differs at another.
//!
//! T037 — audit of the six cross-cutting invariants tabulated at the end of
//! `data-model.md`. Each is named here with the test that actually holds it, so a
//! reader can check the mapping rather than assume it, and so an invariant that
//! loses its test becomes visible.
//!
//! | Invariant (`data-model.md`) | Requirement | Test that holds it |
//! | --- | --- | --- |
//! | Plan byte-identical across repeats, batch sizes, lane counts | FR-072, SC-003 | `a_fixed_seed_gives_byte_identical_plans` and `a_different_seed_gives_a_different_plan`, below, for the repeat half. Batch size and lane count belong to the live path, so that half is held by `workload-node-agent/tests/op_stream.rs::batch_keys_changes_the_requests_but_not_the_workload`. |
//! | Integral draws uniform at the endpoints, not half-weighted | FR-009 | `tests/distribution.rs::integral_uniform_gives_every_value_equal_weight`, with `integral_draws_round_rather_than_truncate_toward_zero` alongside it |
//! | Residual-life seeding gives flat churn from `t = 0` | FR-015 | `tests/pool.rs` — all four tests, with the exponential control the one that guards the sampler |
//! | Keys identical across toolchains and machines | FR-029 | `tests/keys.rs::splitmix64_matches_the_normative_vectors` and `chain_vectors_are_pinned`. **Held for keys.** Keys are integer-only splitmix64, so they are identical everywhere and remote hits cannot be affected. A plan *digest* can still differ across machines, because `f64::exp`/`ln` are not bit-identical across libm versions and a draw landing within an ULP of a rounding boundary could round the other way — **accepted, with no work planned**, for the reasons in `special.rs`'s header. Compare statistics rather than digests across boxes. |
//! | Nested-not-divergent chains for overlapping sets | FR-028 | `tests/session.rs` — six tests, including the named `{0,1}` vs `{0,1,4}` case |
//! | Emit report omits live-only fields | FR-071 | `crates/workload-gen/tests/emit_determinism.rs::the_report_omits_every_live_only_field` — absent fields rather than zeroed ones, which is the whole claim. |
//!
//! One of the six is only partly held, and it says which half sits where rather than
//! claiming the row outright. That is the point of writing the audit down: an audit that
//! reported six of six without naming a test would be describing the table.

use workload_model::description::WorkloadDescription;
use workload_model::plan::OperationPlan;
use workload_model::sim::Simulation;

/// A description exercising every mechanism that could leak nondeterminism: two
/// shared classes with different population forms, a fluctuating session class,
/// and non-degenerate distributions.
fn description() -> WorkloadDescription {
    r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 8}
    lifetime: {constant: .inf}
  docs:
    length: {normal: {mean: 6, sigma: 2, min: 1}}
    lifetime: {exponential: {mean: 300}}
    pool: {size: {poisson: 12}}
session_classes:
  chat:
    pool: {size: {poisson: 15}}
    uses:
      - {class: manual, count: {constant: 1}}
      - {class: docs, count: {uniform: {min: 1, max: 3}}}
    turns: {normal: {mean: 5, sigma: 2, min: 1}}
    input_growth: {constant: 2}
    output_growth: {uniform: {min: 1, max: 3}}
    think_time: {exponential: {mean: 12}}
"#
    .parse()
    .unwrap()
}

fn plan_bytes(seed: u64, until: f64) -> Vec<u8> {
    let d = description();
    let mut sim = Simulation::new(&d, seed, 1).unwrap();
    let mut plan = OperationPlan::default();
    sim.run_until(until, &mut |s, t| plan.record_turn(s, t));
    plan.check_ordered().expect("the plan must be ordered");
    plan.to_canonical_bytes()
}

#[test]
fn a_fixed_seed_gives_byte_identical_plans() {
    // SC-003. Repeated three times rather than twice: two identical runs could
    // both be wrong in the same way if something cached, and a third makes an
    // accidental pass less likely.
    let a = plan_bytes(4242, 1_000.0);
    let b = plan_bytes(4242, 1_000.0);
    let c = plan_bytes(4242, 1_000.0);
    assert!(!a.is_empty(), "the plan is empty, so this proves nothing");
    assert_eq!(a, b);
    assert_eq!(b, c);
}

#[test]
fn a_different_seed_gives_a_different_plan() {
    // The half that catches a seed which is not actually wired through — a failure
    // that looks *exactly* like determinism and would otherwise pass every test
    // above.
    let base = plan_bytes(4242, 1_000.0);
    for seed in [4243u64, 0, 1, u64::MAX] {
        let other = plan_bytes(seed, 1_000.0);
        assert_ne!(
            base, other,
            "seed {seed} produced the same plan as 4242; the seed is not reaching \
             every draw"
        );
    }
}

#[test]
fn every_substream_responds_to_the_seed() {
    // Sharper than the test above. Each concern draws from its own named
    // substream, so a seed reaching only *some* of them would still change the
    // plan — and would still be a defect, because the unreached parts would be
    // fixed across every run of every experiment.
    //
    // Checked through observable consequences: instance lengths come from the
    // pools substream, turn counts and think times from the sessions substream,
    // and growth from the sim substream.
    let d = description();
    let observe = |seed: u64| {
        let mut sim = Simulation::new(&d, seed, 1).unwrap();
        let mut shapes = Vec::new();
        sim.run_until(600.0, &mut |s, t| {
            shapes.push((
                s.shared_len(),
                s.turns_total(),
                t.at().to_bits(),
                t.new_output_len(),
            ));
        });
        shapes
    };
    let a = observe(7);
    let b = observe(8);
    assert!(a.len() > 20 && b.len() > 20);

    // Each of the four coordinates must differ *somewhere* between the two runs.
    let differs = |f: fn(&(usize, usize, u64, usize)) -> u64| {
        let xa: Vec<u64> = a.iter().map(f).collect();
        let xb: Vec<u64> = b.iter().map(f).collect();
        xa != xb
    };
    assert!(differs(|x| x.0 as u64), "shared lengths never varied");
    assert!(differs(|x| x.1 as u64), "turn counts never varied");
    assert!(differs(|x| x.2), "turn times never varied");
    assert!(differs(|x| x.3 as u64), "output growth never varied");
}

#[test]
fn the_span_determines_the_plan_and_nothing_else_does() {
    // A run to 1000 must be a prefix of a run to 2000 at the same seed. If it is
    // not, something depends on the total span — a lookahead, a buffer size, an
    // end-of-run flush — and the plan would then not be a function of description
    // and seed alone.
    let short = plan_bytes(99, 500.0);
    let long = plan_bytes(99, 2_000.0);
    assert!(long.len() > short.len(), "the longer run added nothing");

    // The header carries an operation count, so the bytes are not a literal
    // prefix; compare the operations instead.
    let d = description();
    let ops_of = |until: f64| {
        let mut sim = Simulation::new(&d, 99, 1).unwrap();
        let mut plan = OperationPlan::default();
        sim.run_until(until, &mut |s, t| plan.record_turn(s, t));
        plan.operations()
            .iter()
            .map(|o| (o.at().to_bits(), o.session(), o.kind(), o.key_count()))
            .collect::<Vec<_>>()
    };
    let a = ops_of(500.0);
    let b = ops_of(2_000.0);
    assert_eq!(
        &b[..a.len()],
        &a[..],
        "a longer run is not an extension of a shorter one at the same seed"
    );
}

#[test]
fn the_canonical_header_pins_the_format_version() {
    // A format change must be a new version rather than an edit, or every recorded
    // digest silently comes to mean something else.
    let bytes = plan_bytes(1, 100.0);
    assert_eq!(&bytes[..8], b"CERTUSPL");
    assert_eq!(
        u16::from_le_bytes([bytes[8], bytes[9]]),
        workload_model::plan::CANONICAL_VERSION
    );
}
