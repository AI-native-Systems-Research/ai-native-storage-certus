//! Every complete example in the shipped documentation must actually work.
//!
//! Not a style check. Two defects shipped in the specs because nothing executed
//! them, and both were the kind a reader would take at face value:
//!
//! - the worked example's `warmup: 20s` was **rejected** by the schema's own rule
//!   15b, its session population needing 27.5s to reach steady state;
//! - `block_bytes: 128KiB` and `think_time: {median: 3s}` — the units the contract
//!   documents in its own § Units section — could not be **parsed** at all.
//!
//! A reader copying either would have got an error from the tool and reasonably
//! concluded the tool was broken. So the documentation's own examples are treated
//! as a test fixture: extracted from the markdown, parsed, and validated.
//!
//! Only *complete* documents are checked: a block that starts with `version:` and
//! contains no `{...}` elision. The contract is mostly section-by-section
//! fragments, which are illustrations rather than inputs and cannot be parsed
//! alone, and its top-level skeleton is a whole document with every section
//! elided — it even lists `duration`, `requests` and `blocks` together, which rule
//! 19 refuses. `{...}` is the marker that a block is showing shape rather than
//! content, so it is the discriminator rather than a hand-maintained list of which
//! blocks to skip.

use std::path::{Path, PathBuf};

use workload_model::schema::validate::validate;
use workload_model::schema::Document;

/// The spec directory, relative to this crate.
fn spec_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("specs/001-synthetic-workload-generator")
}

/// Every fenced ```yaml block in `text`.
fn yaml_blocks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match current.as_mut() {
            None => {
                if line.trim_end() == "```yaml" {
                    current = Some(String::new());
                }
            }
            Some(buf) => {
                if line.trim_end() == "```" {
                    out.push(std::mem::take(buf));
                    current = None;
                } else {
                    buf.push_str(line);
                    buf.push('\n');
                }
            }
        }
    }
    out
}

/// Every complete document in the documentation, as `(source, yaml)`.
fn documented_documents() -> Vec<(String, String)> {
    let dir = spec_dir();
    let mut out = Vec::new();
    for rel in [
        "quickstart.md",
        "contracts/workload-schema.md",
        "contracts/plan-format.md",
        "contracts/trace-io.md",
        "spec.md",
    ] {
        let path = dir.join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (i, block) in yaml_blocks(&text).into_iter().enumerate() {
            if block.trim_start().starts_with("version:") && !block.contains("{...}") {
                out.push((format!("{rel} block {i}"), block));
            }
        }
    }
    out
}

#[test]
fn every_documented_document_parses() {
    let docs = documented_documents();
    // Named sources rather than a count, so a renamed file or a changed fence says
    // what went missing instead of quietly making this test vacuous.
    for expected in ["quickstart.md", "contracts/workload-schema.md"] {
        assert!(
            docs.iter().any(|(source, _)| source.starts_with(expected)),
            "no complete example extracted from {expected}; found {:?}",
            docs.iter().map(|(s, _)| s).collect::<Vec<_>>()
        );
    }
    for (source, yaml) in &docs {
        if let Err(e) = Document::from_yaml(yaml) {
            panic!("{source} does not parse: {e}\n---\n{yaml}---");
        }
    }
}

#[test]
fn every_documented_document_passes_validation() {
    // A parsed document that the schema then rejects is worse than one that does
    // not parse: it reads as a working example right up to the point of use.
    for (source, yaml) in documented_documents() {
        let doc = match Document::from_yaml(&yaml) {
            Ok(d) => d,
            // Covered by the test above; not repeated as a second failure here.
            Err(_) => continue,
        };
        let report = validate(&doc);
        let rejections: Vec<String> = report
            .rejections()
            .map(|f| format!("[rule {}] {}", f.rule, f.message))
            .collect();
        assert!(
            rejections.is_empty(),
            "{source} is rejected by the schema it documents:\n  {}",
            rejections.join("\n  ")
        );
    }
}

#[test]
fn the_documented_unit_forms_are_the_ones_that_work() {
    // The specific forms § Units promises, checked against the parser rather than
    // against the prose. A suffix documented but unimplemented is how the last
    // defect happened.
    let doc = Document::from_yaml(
        r#"
version: 1
seed: 1
duration: 60s
corpus:
  block_bytes: 128KiB
  trees:
    roots: {count: 12, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: const, value: 4}
    branching: 1.05
workload:
  arrival: {model: open_loop, rate: 4000/s}
  sessions:
    turns: {dist: const, value: 2}
    think_time: {dist: lognormal, median: 500ms, sigma: 0.5}
    private_depth: {dist: const, value: 8}
    growth_per_turn: {dist: const, value: 4}
  mix:
    - {weight: 1.0}
run: {mode: hardware, warmup: 30s, wss_window: 240_000}
"#,
    )
    .expect("every documented unit form must parse");

    // Sizes in bytes, times in seconds, and the separator gone.
    assert_eq!(doc.corpus.block_bytes.mean(), Some(131_072.0));
    assert_eq!(
        doc.workload.sessions.think_time.quantile(0.5),
        Some(0.5),
        "500ms should be half a second, not 500 of anything"
    );
    assert_eq!(
        workload_model::schema::wss_window_requests(&doc).unwrap().0,
        240_000
    );
}

#[test]
fn a_unit_written_two_ways_is_one_document() {
    // What normalisation buys beyond parsing: identity is over the workload, not
    // over how it was punctuated, so two spellings cannot produce two arms of a
    // comparison that differ only on paper.
    let with_units = r#"
version: 1
seed: 7
requests: 100
corpus:
  block_bytes: 128KiB
  trees:
    roots: {count: 4, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: const, value: 2}
    branching: 1.0
workload:
  arrival: {model: open_loop, rate: 1000/s}
  sessions:
    turns: {dist: const, value: 1}
    think_time: {dist: const, value: 2s}
    private_depth: {dist: const, value: 2}
    growth_per_turn: {dist: const, value: 0}
  mix:
    - {weight: 1.0}
run: {mode: plan, wss_window: 10_000}
"#;
    let plain = with_units
        .replace("128KiB", "131072")
        .replace("value: 2s", "value: 2")
        .replace("10_000", "10000");
    let a = Document::from_yaml(with_units).expect("unit form must parse");
    let b = Document::from_yaml(&plain).expect("plain form must parse");
    assert_eq!(
        a.to_yaml().unwrap(),
        b.to_yaml().unwrap(),
        "the two spellings normalise to different documents, so they would hash \
         differently and read as two workloads"
    );
}

#[test]
fn a_mistyped_unit_is_refused_with_its_path() {
    // The other half of the contract's promise about units: a mistyped suffix is
    // the same class of mistake as a mistyped field name, and must not be read as
    // a bare number.
    let bad = r#"
version: 1
seed: 1
requests: 10
corpus:
  block_bytes: 128QiB
  trees:
    roots: {count: 1, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: const, value: 1}
    branching: 1.0
workload:
  arrival: {model: open_loop, rate: 10/s}
  sessions:
    turns: {dist: const, value: 1}
    think_time: {dist: const, value: 1}
    private_depth: {dist: const, value: 1}
    growth_per_turn: {dist: const, value: 0}
  mix:
    - {weight: 1.0}
run: {mode: plan}
"#;
    let e = Document::from_yaml(bad).expect_err("a bad suffix must be refused");
    let msg = e.to_string();
    assert!(msg.contains("corpus.block_bytes"), "no path in {msg:?}");
    assert!(msg.contains("128QiB"), "no offending value in {msg:?}");
}
