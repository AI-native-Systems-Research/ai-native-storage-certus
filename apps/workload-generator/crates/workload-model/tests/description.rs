//! Description parsing and validation tests (T018).
//!
//! The centrepiece is that **the shipped example validates**. That file is
//! normative for the schema, so if it stopped parsing the schema would have
//! drifted from its own specification, and nothing else in this crate would
//! notice.

use std::path::PathBuf;

use workload_model::description::{
    Population, RankBy, SharedPoolSpec, WorkloadDescription, MAX_DISCARDED_MASS,
};

/// The normative example, found relative to this crate rather than the cwd, so
/// the test works under `cargo test` from anywhere in the workspace.
fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/001-synthetic-workload-generator/contracts/workload-input.example.yml")
}

fn example() -> WorkloadDescription {
    WorkloadDescription::from_path(example_path())
        .expect("the shipped example must parse; it is normative for the schema")
}

// ---------------------------------------------------------------------------
// The shipped example
// ---------------------------------------------------------------------------

#[test]
fn the_shipped_example_parses_and_validates() {
    let d = example();
    let report = d
        .validate()
        .unwrap_or_else(|e| panic!("the normative example must validate:\n{e}"));

    assert_eq!(d.version, 1);
    assert_eq!(d.blocks.tokens, 16);
    assert_eq!(d.blocks.bytes, 65536);
    assert_eq!(d.shared_classes.len(), 5);
    assert_eq!(d.session_classes.len(), 2);

    // Declaration order is the class_id space, so it is asserted, not assumed.
    let names: Vec<&str> = d.shared_classes.iter().map(|(_, n, _)| n).collect();
    assert_eq!(
        names,
        [
            "system_prompt1",
            "system_prompt2",
            "tool",
            "short_document",
            "long_document"
        ]
    );
    assert_eq!(d.shared_classes.index_of("system_prompt1"), Some(0));
    assert_eq!(d.shared_classes.index_of("long_document"), Some(4));

    assert!(report.refusals().is_empty());
    // The report must actually say something: this example truncates several
    // distributions, so a silent report would mean FR-003 is not wired in.
    assert!(
        !report.effective().is_empty(),
        "no effective values reported, yet the example truncates several \
         distributions — FR-003 reporting is not wired in"
    );
}

#[test]
fn the_example_reports_long_documents_effective_count() {
    // `doc_analysis` draws long_document from exponential(mean 5, min 1), and
    // the pool of 20 is the implied maximum. This is the figure FR-003 reports
    // and FR-004 gates on, and it is the mean of the ROUNDED variable because a
    // count is integral — 5.1435, not the continuous 5.565.
    let d = example();
    let report = d.validate().unwrap();
    let (path, eff) = report
        .effective()
        .iter()
        .find(|(p, _)| p.contains("doc_analysis") && p.contains("long_document"))
        .expect("no effective value reported for the long_document count");

    assert!(path.contains("uses"), "unexpected path {path}");
    assert_eq!(eff.requested_mean, 5.0);
    assert!(
        (eff.mean - 5.143_508).abs() < 1e-5,
        "effective count mean {} at pool 20",
        eff.mean
    );
    assert!(
        (eff.discarded - 0.018_316).abs() < 1e-5,
        "discarded {} at pool 20",
        eff.discarded
    );
    assert!(
        eff.discarded < MAX_DISCARDED_MASS,
        "pool 20 must pass the 5% gate"
    );
}

#[test]
fn shrinking_that_pool_to_ten_is_refused_naming_both_means() {
    // The example's own comment records why the pool was raised from 10: at 10
    // the cap discards 13.5% of the count's mass. The refusal must name the
    // requested mean and the effective one, or an author cannot tell how far
    // off they are.
    let text = std::fs::read_to_string(example_path()).unwrap();
    let shrunk = text.replace(
        "      rank_by: recency\n      selection: {exponential: {mean: 6}}",
        "      rank_by: recency\n      selection: {exponential: {mean: 6}}\n      # shrunk",
    );
    // Change long_document's pool from 20 to 10, and only that one.
    let shrunk = shrunk.replace("      size: 20\n", "      size: 10\n");
    assert_ne!(shrunk, text, "the edit did not apply");

    let d: WorkloadDescription = shrunk.parse().unwrap();
    let err = d
        .validate()
        .expect_err("a cap discarding 13.5% of the mass must be refused")
        .to_string();

    assert!(
        err.contains("13.5"),
        "does not name the discarded share:\n{err}"
    );
    assert!(
        err.contains('5'),
        "does not name the requested mean:\n{err}"
    );
    assert!(
        err.contains("3.95"),
        "does not name the effective mean:\n{err}"
    );
    assert!(
        err.contains("long_document"),
        "does not name the offending class:\n{err}"
    );
}

// ---------------------------------------------------------------------------
// Schema shape
// ---------------------------------------------------------------------------

const MINIMAL: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 65536}
shared_classes:
  sp:
    length:   {constant: 10}
    lifetime: {constant: inf}
session_classes:
  chat:
    uses:
      - class: sp
        count: {constant: 1}
    turns:         {constant: 4}
    input_growth:  {constant: 5}
    output_growth: {constant: 2}
    think_time:    {exponential: {mean: 10}}
    pool: {size: {exact: 100}}
"#;

#[test]
fn a_minimal_description_validates() {
    let d: WorkloadDescription = MINIMAL.parse().unwrap();
    d.validate().unwrap();
}

#[test]
fn an_absent_pool_is_one_immortal_instance() {
    // Not the Poisson default. If it were, this description would be refused,
    // because a Poisson birth rate of size/E[lifetime] is undefined for an
    // infinite lifetime — and the shipped example's two system_prompt classes
    // are written exactly this way.
    let d: WorkloadDescription = MINIMAL.parse().unwrap();
    let sp = d.shared_classes.get("sp").unwrap();
    assert_eq!(sp.pool, SharedPoolSpec::default());
    assert_eq!(sp.pool.size, Population::Exact(1));
    d.validate().unwrap();
}

#[test]
fn a_bare_integer_pool_size_means_poisson() {
    let d: WorkloadDescription = MINIMAL
        .replace("pool: {size: {exact: 100}}", "pool: {size: 100}")
        .parse()
        .unwrap();
    assert_eq!(
        d.session_classes.get("chat").unwrap().pool.size,
        Population::Poisson(100)
    );
}

#[test]
fn rank_by_parses_both_forms_and_defaults_to_slot() {
    let with = MINIMAL.replace(
        "    lifetime: {constant: inf}",
        "    lifetime: {exponential: {mean: 100}}\n    pool: {size: {exact: 4}, rank_by: recency}",
    );
    let d: WorkloadDescription = with.parse().unwrap();
    assert_eq!(
        d.shared_classes.get("sp").unwrap().pool.rank_by,
        Some(RankBy::Recency)
    );
    // Absent, it is reported as defaulted rather than silently chosen.
    let without = MINIMAL.replace(
        "    lifetime: {constant: inf}",
        "    lifetime: {exponential: {mean: 100}}\n    pool: {size: {exact: 4}}",
    );
    let d: WorkloadDescription = without.parse().unwrap();
    assert_eq!(d.shared_classes.get("sp").unwrap().pool.rank_by, None);
    let report = d.validate().unwrap();
    assert!(
        report
            .notes()
            .iter()
            .any(|n| n.contains("rank_by defaulted")),
        "the defaulted rank_by was not reported: {:?}",
        report.notes()
    );
}

#[test]
fn declaration_order_is_preserved_and_duplicates_refused() {
    let two = MINIMAL.replace(
        "session_classes:",
        "  zz:\n    length:   {constant: 1}\n    lifetime: {constant: inf}\nsession_classes:",
    );
    let d: WorkloadDescription = two.parse().unwrap();
    assert_eq!(d.shared_classes.index_of("sp"), Some(0));
    assert_eq!(d.shared_classes.index_of("zz"), Some(1));

    let dup = MINIMAL.replace(
        "session_classes:",
        "  sp:\n    length:   {constant: 1}\n    lifetime: {constant: inf}\nsession_classes:",
    );
    assert!(
        dup.parse::<WorkloadDescription>().is_err(),
        "a duplicate class name was accepted, which would shift every later class_id"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_version_is_refused() {
    let d: WorkloadDescription = MINIMAL.replace("version: 1", "version: 7").parse().unwrap();
    let e = d.validate().unwrap_err().to_string();
    assert!(e.contains("version 7"), "{e}");
}

#[test]
fn a_uses_entry_naming_an_undeclared_class_is_refused() {
    let d: WorkloadDescription = MINIMAL
        .replace("      - class: sp", "      - class: nonexistent")
        .parse()
        .unwrap();
    let e = d.validate().unwrap_err().to_string();
    assert!(e.contains("nonexistent"), "{e}");
    assert!(e.contains("not declared"), "{e}");
}

#[test]
fn poisson_with_an_unbounded_lifetime_is_refused_and_names_the_fix() {
    // The birth rate is size / E[lifetime], which is undefined for an infinite
    // lifetime. The refusal must point at `exact`, since that is the form the
    // author almost certainly wanted (FR-017: minted once, never turns over).
    let d: WorkloadDescription = MINIMAL
        .replace(
            "    lifetime: {constant: inf}",
            "    lifetime: {constant: inf}\n    pool: {size: {poisson: 50}}",
        )
        .parse()
        .unwrap();
    let e = d.validate().unwrap_err().to_string();
    assert!(e.contains("finite lifetime"), "{e}");
    assert!(e.contains("exact: 50"), "does not name the fix:\n{e}");
}

#[test]
fn exact_with_an_unbounded_lifetime_is_accepted() {
    // The other half of the claim above: an immortal pool of a fixed size is
    // exactly FR-017's mint-once case and must not be refused.
    let d: WorkloadDescription = MINIMAL
        .replace(
            "    lifetime: {constant: inf}",
            "    lifetime: {constant: inf}\n    pool: {size: {exact: 50}}",
        )
        .parse()
        .unwrap();
    d.validate().unwrap();
}

#[test]
fn a_tuning_or_host_field_is_refused_anywhere() {
    // FR-005: one description must be portable across clusters unchanged, so
    // nothing host-specific or tuning-related may appear in it.
    for bad in [
        (
            "blocks: {tokens: 16, bytes: 65536}",
            "blocks: {tokens: 16, bytes: 65536, lanes: 8}",
        ),
        ("version: 1", "version: 1\nnodes: [node5, node7]"),
        ("version: 1", "version: 1\nseed: 42"),
        ("version: 1", "version: 1\nshm_path: /dev/shm/certus"),
        (
            "    pool: {size: {exact: 100}}",
            "    pool: {size: {exact: 100}, gain: 0.5}",
        ),
    ] {
        let text = MINIMAL.replace(bad.0, bad.1);
        assert!(
            text.parse::<WorkloadDescription>().is_err(),
            "accepted a tuning/host field: {}",
            bad.1
        );
    }
}

#[test]
fn an_empty_session_class_set_is_refused() {
    let text = "version: 1\nblocks: {tokens: 16, bytes: 65536}\n\
                shared_classes: {}\nsession_classes: {}";
    let d: WorkloadDescription = text.parse().unwrap();
    let e = d.validate().unwrap_err().to_string();
    assert!(e.contains("no workload"), "{e}");
}

#[test]
fn every_refusal_is_collected_rather_than_only_the_first() {
    // One run should surface every problem in a file. Reporting one per run
    // makes fixing a description an iterative guessing game.
    let d: WorkloadDescription = MINIMAL
        .replace("      - class: sp", "      - class: missing1")
        .replace(
            "    turns:         {constant: 4}",
            "    turns:         {constant: 0}",
        )
        .parse()
        .unwrap();
    let e = d.validate().unwrap_err().to_string();
    assert!(e.contains("missing1"), "{e}");
    assert!(e.contains("turns"), "{e}");
    assert!(
        e.matches("  - ").count() >= 2,
        "only one refusal was reported:\n{e}"
    );
}

// ---------------------------------------------------------------------------
// The salt-encodable ceilings become load-time refusals, not mid-run panics
// ---------------------------------------------------------------------------

#[test]
fn a_pool_beyond_the_encodable_instance_count_is_refused() {
    let d: WorkloadDescription = MINIMAL
        .replace(
            "    lifetime: {constant: inf}",
            "    lifetime: {exponential: {mean: 100}}\n    pool: {size: {exact: 70000000}}",
        )
        .parse()
        .unwrap();
    let e = d.validate().unwrap_err().to_string();
    assert!(e.contains("67108864"), "{e}");
}

#[test]
fn a_session_longer_than_one_stream_can_encode_is_refused() {
    // turns x input_growth is the worst-case ordinal in a session's input
    // stream. Refusing at load turns what would be a panic part-way through a
    // run — destroying its output — into an error before anything is issued.
    let d: WorkloadDescription = MINIMAL
        .replace(
            "    turns:         {constant: 4}",
            "    turns:         {constant: 20000000}",
        )
        .parse()
        .unwrap();
    let e = d.validate().unwrap_err().to_string();
    assert!(e.contains("16777216"), "{e}");
}

#[test]
fn the_report_renders_for_a_clean_description() {
    let d: WorkloadDescription = MINIMAL.parse().unwrap();
    let r = d.validate().unwrap();
    let rendered = r.render();
    assert!(!rendered.is_empty());
}
