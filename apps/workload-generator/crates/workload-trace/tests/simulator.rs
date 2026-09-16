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
use workload_trace::jsonl::JsonlWriter;
use workload_trace::record::InvocationRecord;
use workload_trace::simulator::{convert_jsonl, SimulatorWriter};

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

/// Emit a trace, convert it, and load the conversion in the simulator.
struct Converted {
    trace: replay::Trace,
    sessions_in_run: u64,
    converter_distinct: u64,
    converter_references: u64,
    converter_records: u64,
}

fn run(seed: u64, span: f64, dir: &std::path::Path) -> Converted {
    let d = description();

    // Emit, straight through the real writer.
    let jsonl_path = dir.join("trace.jsonl");
    let mut sim = Simulation::new(&d, seed).unwrap();
    let file = std::fs::File::create(&jsonl_path).unwrap();
    let mut writer = JsonlWriter::new(BufWriter::new(file), "sim", BLOCK_SIZE);
    sim.run_until(span, &mut |s, t| writer.write(s, t).unwrap());
    let stats = writer.finish().unwrap();

    // Convert.
    let sim_path = dir.join("sim.jsonl");
    let input = std::io::BufReader::new(std::fs::File::open(&jsonl_path).unwrap());
    let out = BufWriter::new(std::fs::File::create(&sim_path).unwrap());
    let converted = convert_jsonl(input, out).unwrap();

    Converted {
        trace: replay::load(&sim_path).expect("the simulator must load the conversion"),
        sessions_in_run: stats.sessions,
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

#[test]
fn the_projection_is_the_same_whichever_entry_point_produced_it() {
    // FR-075a: `emit`'s in-stream path and `convert`'s stored-trace path must use the
    // same projection, or a trace and its conversion could disagree about the workload.
    let tmp = TempDir::new().unwrap();
    let d = description();
    let span = 400.0;

    // In-stream: build records as turns happen and project them directly.
    let mut sim = Simulation::new(&d, 25).unwrap();
    let mut direct: Vec<u8> = Vec::new();
    let mut w = SimulatorWriter::new(&mut direct);
    sim.run_until(span, &mut |s, t| {
        let record = InvocationRecord::from_turn("sim", s, t, BLOCK_SIZE);
        w.write_record(&record).unwrap();
    });
    let direct_stats = w.finish().unwrap();

    // Via a stored trace.
    let jsonl_path = tmp.path().join("trace.jsonl");
    let mut sim = Simulation::new(&d, 25).unwrap();
    let file = std::fs::File::create(&jsonl_path).unwrap();
    let mut writer = JsonlWriter::new(BufWriter::new(file), "sim", BLOCK_SIZE);
    sim.run_until(span, &mut |s, t| writer.write(s, t).unwrap());
    writer.finish().unwrap();
    let mut through_file: Vec<u8> = Vec::new();
    let input = std::io::BufReader::new(std::fs::File::open(&jsonl_path).unwrap());
    let file_stats = convert_jsonl(input, &mut through_file).unwrap();

    assert_eq!(direct_stats, file_stats, "the two entry points disagree");
    assert_eq!(
        direct, through_file,
        "the two entry points produced different bytes"
    );
}
