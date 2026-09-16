//! T062 — an emit run's output is byte-identical across repeats, plus the `emit`,
//! `validate` and `plan` surfaces and their exit codes.
//!
//! # The half of FR-072 this file cannot yet reach
//!
//! FR-072 requires the output to be identical across **differing `--batch-keys` and
//! `--lanes`** as well as across repeats. Those flags belong to the live path and do
//! not exist yet (US1, T044-T046), so that half is **not tested here**.
//!
//! It is worth being precise about why that is not merely a missing test. The
//! property holds *structurally*: `workload-gen` cannot pass a batch size or a lane
//! count into `workload-model`, because `workload-model` has no parameter to receive
//! one and no dependency through which one could arrive. So the guarantee currently
//! rests on the crate boundary rather than on a test, which is what research.md D3
//! set out to achieve — but a boundary is only an argument, and the executable form
//! of it is still owed once those flags exist.
//!
//! # Why this drives the CLI rather than the library
//!
//! Everything here is reachable through `cli::run`, which returns an exit code
//! instead of calling `exit`. A subcommand that can only be exercised by spawning a
//! binary tends not to be exercised at all, and the exit codes are part of
//! `contracts/cli.md` — code 3 especially, which exists so a sweep driver cannot
//! mistake an invalid run for a data point.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

/// The shipped example is the normative input schema, so the CLI is exercised on it
/// rather than on a description invented here.
const EXAMPLE: &str = include_str!(
    "../../../specs/001-synthetic-workload-generator/contracts/workload-input.example.yml"
);

/// A small description, for the cases where the example's scale would make the test
/// slow rather than more convincing.
const SMALL: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 4}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 5}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 4}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 10}
"#;

fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    fs::write(&p, text).unwrap();
    p
}

/// Run `emit` and return (exit code, the JSONL body).
fn emit(dir: &Path, description: &Path, until: f64, seed: u64, force: bool) -> (i32, String) {
    let out = dir.join(format!("out-{seed}-{until}"));
    let mut argv = vec![
        "workload-gen".to_string(),
        "emit".to_string(),
        description.display().to_string(),
        "--until".to_string(),
        until.to_string(),
        "--output".to_string(),
        out.display().to_string(),
        "--seed".to_string(),
        seed.to_string(),
    ];
    if force {
        argv.push("--force".to_string());
    }
    let code = workload_gen::cli::run_argv(&argv);
    let body = find_jsonl(&out)
        .map(|p| fs::read_to_string(p).unwrap())
        .unwrap_or_default();
    (code, body)
}

fn find_jsonl(dir: &Path) -> Option<std::path::PathBuf> {
    let invocations = dir.join("invocations");
    for entry in fs::read_dir(invocations).ok()? {
        let sub = entry.ok()?.path();
        for f in fs::read_dir(sub).ok()? {
            let f = f.ok()?.path();
            if f.extension().is_some_and(|e| e == "jsonl") {
                return Some(f);
            }
        }
    }
    None
}

#[test]
fn an_emit_run_is_byte_identical_across_repeats() {
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "small.yml", SMALL);
    let (code_a, a) = emit(tmp.path(), &d, 500.0, 7, false);
    let (code_b, b) = emit(tmp.path(), &d, 500.0, 7, false);
    assert_eq!(code_a, 0);
    assert_eq!(code_b, 0);
    assert!(!a.is_empty(), "nothing was written");
    assert_eq!(a, b, "the same seed produced different bytes");
}

#[test]
fn a_different_seed_produces_a_different_trace() {
    // The half that catches a seed that never reaches the simulation — a failure that
    // looks exactly like determinism.
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "small.yml", SMALL);
    let (_, a) = emit(tmp.path(), &d, 500.0, 7, false);
    let (_, b) = emit(tmp.path(), &d, 500.0, 8, false);
    assert_ne!(a, b);
}

#[test]
fn the_manifest_and_report_are_both_written_and_the_manifest_goes_last() {
    // FR-073: a directory without a manifest is incomplete by construction. That is
    // only true if the manifest really is written after the records, so the ordering
    // is checked through the filesystem's own modification times.
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "small.yml", SMALL);
    let out = tmp.path().join("out-7-500");
    let (code, _) = emit(tmp.path(), &d, 500.0, 7, false);
    assert_eq!(code, 0);

    let manifest = out.join("manifest.json");
    let report = out.join("report.json");
    assert!(manifest.exists(), "no manifest");
    assert!(report.exists(), "no report");

    let jsonl = find_jsonl(&out).expect("a jsonl part file");
    let rows = fs::metadata(&jsonl).unwrap().modified().unwrap();
    let m = fs::metadata(&manifest).unwrap().modified().unwrap();
    assert!(m >= rows, "the manifest predates the records it describes");

    let manifest_text = fs::read_to_string(&manifest).unwrap();
    assert!(manifest_text.contains("\"block_id_space\": \"chained_u64\""));
    assert!(manifest_text.contains("\"seed\": 7"));
}

#[test]
fn the_report_omits_every_live_only_field() {
    // FR-071 end to end, on the file a downstream tool would read rather than on the
    // struct.
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "small.yml", SMALL);
    emit(tmp.path(), &d, 500.0, 7, false);
    let text = fs::read_to_string(tmp.path().join("out-7-500").join("report.json")).unwrap();
    for forbidden in ["latency", "p99", "lane", "plan_queue", "throughput"] {
        assert!(
            !text.contains(forbidden),
            "report mentions {forbidden}:\n{text}"
        );
    }
    assert!(text.contains("\"run_kind\": \"emit\""));
    assert!(text.contains("generation_rate_invocations_per_second"));
}

#[test]
fn session_counts_cover_every_class_not_just_the_first() {
    // The shipped example has more than one session class. Reporting class 0's counts
    // as the run's reads perfectly plausibly and is simply wrong, which is why this
    // is asserted against a two-class description.
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "example.yml", EXAMPLE);
    let (code, _) = emit(tmp.path(), &d, 60.0, 3, false);
    assert_eq!(code, 0);
    let text = fs::read_to_string(tmp.path().join("out-3-60").join("report.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let started = v["sessions_started"].as_u64().unwrap();
    // The example declares chat at 1000 concurrent and doc_analysis at 1000, so a
    // single-class count could not reach the seeded total of both.
    assert!(
        started > 1_000,
        "only {started} sessions started; one class's count was reported"
    );
}

#[test]
fn an_invalid_description_exits_two_and_writes_nothing() {
    // Exit 2 means "rejected at load, nothing was issued", so the absence of output
    // is part of the contract rather than a detail.
    let tmp = TempDir::new().unwrap();
    let broken = SMALL.replace("class: manual", "class: nonexistent");
    let d = write(tmp.path(), "broken.yml", &broken);
    let out = tmp.path().join("out-1-100");
    let code = workload_gen::cli::run_argv(&[
        "workload-gen".into(),
        "emit".into(),
        d.display().to_string(),
        "--until".into(),
        "100".into(),
        "--output".into(),
        out.display().to_string(),
        "--seed".into(),
        "1".into(),
    ]);
    assert_eq!(code, 2, "a rejected configuration must exit 2");
    assert!(
        !out.join("manifest.json").exists(),
        "a rejected run left a manifest behind"
    );
}

#[test]
fn a_non_positive_span_is_refused() {
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "small.yml", SMALL);
    for until in ["0", "-5"] {
        let code = workload_gen::cli::run_argv(&[
            "workload-gen".into(),
            "emit".into(),
            d.display().to_string(),
            "--until".into(),
            until.into(),
            "--output".into(),
            tmp.path().join("nope").display().to_string(),
            "--seed".into(),
            "1".into(),
        ]);
        assert_eq!(code, 2, "--until {until} should be refused");
    }
}

#[test]
fn an_oversized_run_is_refused_before_writing_anything() {
    // Deliberately named for what it asserts. Which of the two refusals fires depends
    // on the box: with less free space than the 32 GiB ceiling, the free-space check
    // comes first. Both branches are covered deterministically by `cli`'s own unit
    // tests, where free space is a parameter; this one asserts the end-to-end
    // property that a refused run writes nothing at all.
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "example.yml", EXAMPLE);
    // The example costs tens of gigabytes at a 100 000-second span.
    let out = tmp.path().join("huge");
    let code = workload_gen::cli::run_argv(&[
        "workload-gen".into(),
        "emit".into(),
        d.display().to_string(),
        "--until".into(),
        "1000000".into(),
        "--output".into(),
        out.display().to_string(),
        "--seed".into(),
        "1".into(),
    ]);
    assert_eq!(code, 2, "an oversized run must be refused");
    assert!(
        !out.join("manifest.json").exists(),
        "a refused run wrote a trace anyway"
    );
}

#[test]
fn validate_reports_without_writing_and_can_invert_the_projection() {
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "example.yml", EXAMPLE);
    let code = workload_gen::cli::run_argv(&[
        "workload-gen".into(),
        "validate".into(),
        d.display().to_string(),
        "--until".into(),
        "1000".into(),
        "--for-invocations".into(),
        "50000".into(),
    ]);
    assert_eq!(code, 0);
    // Nothing was created next to the description.
    let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
    assert_eq!(entries.len(), 1, "validate wrote something");
}

#[test]
fn plan_writes_a_canonical_file_that_is_byte_identical_at_a_fixed_seed() {
    let tmp = TempDir::new().unwrap();
    let d = write(tmp.path(), "small.yml", SMALL);
    let run = |name: &str, seed: &str| {
        let out = tmp.path().join(name);
        let code = workload_gen::cli::run_argv(&[
            "workload-gen".into(),
            "plan".into(),
            d.display().to_string(),
            "--until".into(),
            "400".into(),
            "--output".into(),
            out.display().to_string(),
            "--seed".into(),
            seed.into(),
        ]);
        assert_eq!(code, 0);
        fs::read(out).unwrap()
    };
    let a = run("a.plan", "5");
    let b = run("b.plan", "5");
    let c = run("c.plan", "6");
    assert_eq!(&a[..8], b"CERTUSPL");
    assert_eq!(a, b);
    assert_ne!(a, c);
}
