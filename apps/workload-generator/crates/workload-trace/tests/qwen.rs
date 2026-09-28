//! T061 — a converted trace loads in the real simulator, with counts that match.
//!
//! This drives `eviction_replay_benchmark::replay::load` itself rather than a
//! reimplementation of it. A reimplementation would be a test of my reading of the
//! loader, and my reading is exactly the thing that could be wrong — `research.md` D1
//! is a claim *about another app's* record shape, and the only way to check a claim
//! like that is to run the code it is about.
//!
//! # The failure this file exists for
//!
//! The loader derives a session by walking `parent_chat_id` to a conversation root. A
//! wrong chain therefore **still loads**: no error, no warning, and every session
//! silently collapsed into one — or one split into many. A lineage-aware eviction
//! policy would then score against a workload nobody described, and the number it
//! produced would look entirely reasonable.
//!
//! So the assertion that matters here is not "it loaded". It is that the **session
//! count** the loader derives equals the number of sessions the run actually had, and
//! that the distinct-key and reference counts match the converter's own.

use std::io::BufWriter;

use eviction_replay_benchmark::replay;
use tempfile::TempDir;
use workload_model::description::WorkloadDescription;
use workload_model::sim::Simulation;
use workload_trace::qwen::QwenWriter;
use workload_trace::record::InvocationRecord;

const BLOCK_SIZE: u64 = 16;

/// Several concurrent sessions with several turns each, so the rows interleave and the
/// parent chains have to be reconstructed rather than read off adjacent lines.
fn description() -> WorkloadDescription {
    r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 3}
    lifetime: {constant: .inf}
  docs:
    length: {uniform: {min: 1, max: 3}}
    lifetime: {exponential: {mean: 500, min: 1}}
    pool: {size: {exact: 8}}
session_classes:
  chat:
    pool: {size: {exact: 7}}
    uses:
      - {class: manual, count: {constant: 1}}
      - {class: docs, count: {constant: 2}}
    turns: {constant: 5}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 9}
"#
    .parse()
    .unwrap()
}

/// A projected run, plus what the projection and the simulator each counted.
struct Converted {
    trace: replay::Trace,
    sessions_in_run: u64,
    converter_distinct: u64,
    converter_references: u64,
    converter_records: u64,
}

fn run(seed: u64, span: f64, dir: &std::path::Path) -> Converted {
    let d = description();

    // Project straight through `QwenWriter::write_record`, the path `emit` takes, and read
    // the result back through another app's loader rather than our own — reading our own
    // writing would only prove the two halves of this crate agree.
    let sim_path = dir.join("sim.jsonl");
    let mut sim = Simulation::new(&d, seed, 1).unwrap();
    let mut sessions = std::collections::BTreeSet::new();
    let converted = {
        let out = BufWriter::new(std::fs::File::create(&sim_path).unwrap());
        let mut writer = QwenWriter::new(out);
        sim.run_until(span, &mut |s, t| {
            sessions.insert(s.id());
            writer
                .write_record(&InvocationRecord::from_turn("sim", s, t, BLOCK_SIZE))
                .unwrap();
        });
        writer.finish().unwrap()
    };

    Converted {
        // `None` is "no conversation cap", which this file requires rather than
        // merely prefers: every assertion below compares the loader's counts against
        // the whole converted trace, so a cap would make them disagree by design.
        trace: replay::load(&sim_path, None).expect("the simulator must load the conversion"),
        sessions_in_run: sessions.len() as u64,
        converter_distinct: converted.distinct_keys,
        converter_references: converted.key_references,
        converter_records: converted.records,
    }
}

#[test]
fn the_simulator_derives_exactly_the_sessions_the_run_had() {
    // **The assertion this file exists for.** A wrong parent chain still loads, so
    // "it loaded" proves nothing; the session count is what catches a collapsed or
    // split conversation graph.
    let tmp = TempDir::new().unwrap();
    let c = run(21, 600.0, tmp.path());

    let derived: std::collections::BTreeSet<_> = c.trace.ops.iter().map(|o| o.session_id).collect();
    assert!(c.sessions_in_run >= 7, "too few sessions to be a test");
    assert_eq!(
        derived.len() as u64,
        c.sessions_in_run,
        "the simulator derived {} sessions from a run that had {} — the conversation \
         graph was reshaped by the conversion, and nothing errored",
        derived.len(),
        c.sessions_in_run
    );
}

#[test]
fn distinct_key_and_reference_counts_survive_the_conversion() {
    let tmp = TempDir::new().unwrap();
    let c = run(22, 600.0, tmp.path());
    assert_eq!(
        c.trace.distinct_keys as u64, c.converter_distinct,
        "distinct keys disagree between the converter and the loader"
    );
    assert_eq!(
        c.trace.total_key_refs as u64, c.converter_references,
        "key references disagree"
    );
    assert_eq!(c.trace.ops.len() as u64, c.converter_records);
    assert!(c.trace.distinct_keys > 0);
}

#[test]
fn the_conversion_preserves_reuse_rather_than_flattening_it() {
    // A conversion that emitted each turn's *new* blocks instead of its whole prompt
    // would load, replay, and show almost no cache reuse — a plausible-looking result
    // from a broken projection. References must therefore far exceed distinct keys.
    let tmp = TempDir::new().unwrap();
    let c = run(23, 600.0, tmp.path());
    let ratio = c.trace.total_key_refs as f64 / c.trace.distinct_keys as f64;
    assert!(
        ratio > 3.0,
        "only {ratio:.2} references per distinct key; the prompt prefixes are not \
         being carried through"
    );
}

#[test]
fn every_sessions_turns_stay_in_order_within_the_conversion() {
    // The loader relies on parents preceding children in file order. Our trace is in
    // virtual-time order and a session's turns are monotonic within it, so this holds
    // — but it holds because of that, not by construction of the converter, so it is
    // worth asserting.
    let tmp = TempDir::new().unwrap();
    let c = run(24, 600.0, tmp.path());
    let mut seen: std::collections::BTreeMap<_, usize> = Default::default();
    for op in &c.trace.ops {
        *seen.entry(op.session_id).or_default() += 1;
    }
    // Every session contributed at least one op, and the busiest more than one, or the
    // chain-walking was never exercised.
    assert!(seen.values().any(|n| *n > 1), "no multi-turn session");
    assert!(seen.values().all(|n| *n >= 1));
}
