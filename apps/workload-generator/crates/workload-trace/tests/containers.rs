//! T055 — the two containers yield identical records, and every row satisfies the
//! invariants the trace's own manifest declares.
//!
//! FR-055 says "two containers holding identical records", and SC-004 asserts it.
//! That is not a property either writer can check on its own: each is convincing in
//! isolation, and the failure mode is a field that means something slightly
//! different in one of them — a list order, a null encoded as zero, a length in the
//! wrong unit. So this reads the parquet back and compares it to the JSONL, field by
//! field, rather than comparing the two writers' inputs.
//!
//! The whole file is behind the `parquet` feature, because half the comparison does
//! not exist without it. Run with:
//!
//! ```text
//! cargo test -p workload-trace --features parquet
//! ```
//!
//! Without the feature the JSONL side is still covered by `src/jsonl.rs`'s own
//! tests, so nothing is silently unchecked in a default build — but the
//! *equivalence* claim is only tested with the feature on, and CI must therefore run
//! it that way. Recorded here because a feature-gated test that nobody enables is
//! indistinguishable from no test.

#![cfg(feature = "parquet")]

use std::sync::Arc;

use arrow_array::{
    Array, BooleanArray, Float64Array, Int64Array, ListArray, StringArray, UInt64Array,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use workload_model::description::WorkloadDescription;
use workload_model::sim::Simulation;
use workload_trace::jsonl::JsonlWriter;
use workload_trace::manifest::{BlockStats, Manifest};
use workload_trace::parquet::ParquetWriter;
use workload_trace::record::InvocationRecord;

const BLOCK_SIZE: u64 = 16;

/// A description with enough shape to catch a per-field mistake: several concurrent
/// sessions so rows interleave, more than one turn so the append-only chain is
/// exercised, and both growth kinds non-trivial.
fn description() -> (WorkloadDescription, &'static str) {
    let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 3}
    lifetime: {constant: .inf}
  docs:
    length: {uniform: {min: 1, max: 4}}
    lifetime: {exponential: {mean: 400, min: 1}}
    pool: {size: {exact: 6}}
session_classes:
  chat:
    pool: {size: {exact: 5}}
    uses:
      - {class: manual, count: {constant: 1}}
      - {class: docs, count: {uniform: {min: 1, max: 2}}}
    turns: {constant: 4}
    input_growth: {constant: 2}
    output_growth: {uniform: {min: 1, max: 3}}
    think_time: {constant: 7}
"#;
    (yaml.parse().unwrap(), yaml)
}

/// Emit the same run into both containers and return (jsonl rows, parquet rows).
fn emit_both(
    seed: u64,
    span: f64,
) -> (
    Vec<InvocationRecord>,
    Vec<InvocationRecord>,
    BlockStats,
    BlockStats,
) {
    let (d, _) = description();

    // JSONL. Records are rebuilt from the serialised lines rather than kept from the
    // writer, so a serialisation bug is inside the comparison rather than outside it.
    let mut json_bytes: Vec<u8> = Vec::new();
    let mut sim = Simulation::new(&d, seed).unwrap();
    let mut jw = JsonlWriter::new(&mut json_bytes, "equiv", BLOCK_SIZE);
    sim.run_until(span, &mut |s, t| jw.write(s, t).expect("valid row"));
    let json_stats = jw.finish().unwrap();
    let json_rows: Vec<InvocationRecord> = String::from_utf8(json_bytes)
        .unwrap()
        .lines()
        .map(parse_line)
        .collect();

    // Parquet, from a fresh simulation at the same seed.
    let mut pq_bytes: Vec<u8> = Vec::new();
    let mut sim = Simulation::new(&d, seed).unwrap();
    let mut pw = ParquetWriter::new(&mut pq_bytes, "equiv", BLOCK_SIZE).unwrap();
    sim.run_until(span, &mut |s, t| pw.write(s, t).expect("valid row"));
    let pq_stats = pw.finish().unwrap();
    let pq_rows = read_parquet(&pq_bytes);

    (json_rows, pq_rows, json_stats, pq_stats)
}

/// Rebuild a record from one JSONL line, so the comparison goes through the
/// serialised form on both sides.
fn parse_line(line: &str) -> InvocationRecord {
    let v: serde_json::Value = serde_json::from_str(line).unwrap();
    let u64s = |k: &str| -> Vec<u64> {
        v[k].as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_u64().unwrap())
            .collect()
    };
    InvocationRecord {
        trace_id: v["trace_id"].as_str().unwrap().to_string(),
        session_id: v["session_id"].as_str().unwrap().to_string(),
        invocation_index: v["invocation_index"].as_i64().unwrap(),
        parent_invocation: v["parent_invocation"].as_i64().unwrap(),
        request_start: v["request_start"].as_f64().unwrap(),
        request_end: v["request_end"].as_f64(),
        timestamp_kind: "start",
        timestamp_is_synthetic: v["timestamp_is_synthetic"].as_bool().unwrap(),
        model: v["model"].as_str().map(str::to_string),
        input_length: v["input_length"].as_i64().unwrap(),
        output_length: v["output_length"].as_i64().unwrap(),
        reuse_from: v["reuse_from"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect(),
        new_input_blocks: u64s("new_input_blocks"),
        new_output_blocks: u64s("new_output_blocks"),
        full_input_blocks: u64s("full_input_blocks"),
        full_output_blocks: u64s("full_output_blocks"),
        partial_final_valid: v["partial_final_valid"].as_i64(),
    }
}

/// Read every row back out of a parquet buffer.
fn read_parquet(bytes: &[u8]) -> Vec<InvocationRecord> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(bytes.to_vec()))
        .unwrap()
        .build()
        .unwrap();
    let mut out = Vec::new();
    for batch in reader {
        let batch = batch.unwrap();
        let s = |name: &str| -> Arc<dyn Array> { batch.column_by_name(name).unwrap().clone() };
        let strings = |name: &str| {
            s(name)
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .clone()
        };
        let i64s = |name: &str| {
            s(name)
                .as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .clone()
        };
        let f64s = |name: &str| {
            s(name)
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .clone()
        };
        let lists = |name: &str| {
            s(name)
                .as_any()
                .downcast_ref::<ListArray>()
                .unwrap()
                .clone()
        };

        let trace_id = strings("trace_id");
        let session_id = strings("session_id");
        let invocation_index = i64s("invocation_index");
        let parent_invocation = i64s("parent_invocation");
        let request_start = f64s("request_start");
        let request_end = f64s("request_end");
        let timestamp_kind = strings("timestamp_kind");
        let synthetic = s("timestamp_is_synthetic")
            .as_any()
            .downcast_ref::<BooleanArray>()
            .unwrap()
            .clone();
        let model = strings("model");
        let input_length = i64s("input_length");
        let output_length = i64s("output_length");
        let reuse_from = lists("reuse_from");
        let new_input = lists("new_input_blocks");
        let new_output = lists("new_output_blocks");
        let full_input = lists("full_input_blocks");
        let full_output = lists("full_output_blocks");
        let partial = i64s("partial_final_valid");

        let u64_list = |a: &ListArray, i: usize| -> Vec<u64> {
            let v = a.value(i);
            v.as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .values()
                .to_vec()
        };
        let i64_list = |a: &ListArray, i: usize| -> Vec<i64> {
            let v = a.value(i);
            v.as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .values()
                .to_vec()
        };

        for i in 0..batch.num_rows() {
            out.push(InvocationRecord {
                trace_id: trace_id.value(i).to_string(),
                session_id: session_id.value(i).to_string(),
                invocation_index: invocation_index.value(i),
                parent_invocation: parent_invocation.value(i),
                request_start: request_start.value(i),
                request_end: if request_end.is_null(i) {
                    None
                } else {
                    Some(request_end.value(i))
                },
                timestamp_kind: "start",
                timestamp_is_synthetic: synthetic.value(i),
                model: if model.is_null(i) {
                    None
                } else {
                    Some(model.value(i).to_string())
                },
                input_length: input_length.value(i),
                output_length: output_length.value(i),
                reuse_from: i64_list(&reuse_from, i),
                new_input_blocks: u64_list(&new_input, i),
                new_output_blocks: u64_list(&new_output, i),
                full_input_blocks: u64_list(&full_input, i),
                full_output_blocks: u64_list(&full_output, i),
                partial_final_valid: if partial.is_null(i) {
                    None
                } else {
                    Some(partial.value(i))
                },
            });
            assert_eq!(timestamp_kind.value(i), "start");
        }
    }
    out
}

#[test]
fn the_two_containers_hold_identical_records() {
    // SC-004. Compared row by row rather than as a whole, so a failure names the row
    // and the field instead of saying "the files differ".
    let (json, pq, _, _) = emit_both(11, 600.0);
    assert!(json.len() > 20, "only {} rows, too few to test", json.len());
    assert_eq!(json.len(), pq.len(), "different row counts");
    for (i, (a, b)) in json.iter().zip(&pq).enumerate() {
        assert_eq!(a, b, "row {i} differs between containers");
    }
}

#[test]
fn both_containers_report_the_same_statistics() {
    // The manifest is written from these, so a difference here would produce two
    // manifests describing the same run differently.
    let (_, _, json_stats, pq_stats) = emit_both(12, 600.0);
    assert_eq!(json_stats, pq_stats);
    assert!(json_stats.invocations > 20);
    assert!(json_stats.sessions >= 5);
    assert!(json_stats.unique_blocks > 0);
}

#[test]
fn every_row_satisfies_the_invariants_its_manifest_declares() {
    // FR-058, on every row rather than a sample — and checked against the manifest's
    // own `block_size` rather than a constant in this test, because the invariant is
    // "the trace is consistent with what it says about itself".
    let (d, text) = description();
    let (json, pq, stats, _) = emit_both(13, 600.0);
    let manifest = Manifest::new("equiv", &d, text, 13, 600.0, stats);
    assert_eq!(manifest.encoding, "full");

    for rows in [&json, &pq] {
        let mut previous: std::collections::HashMap<String, InvocationRecord> = Default::default();
        for row in rows.iter() {
            row.check(manifest.block_size, previous.get(&row.session_id))
                .expect("row must satisfy the manifest's invariants");
            previous.insert(row.session_id.clone(), row.clone());
        }
    }
}

#[test]
fn parquet_is_smaller_than_jsonl_for_the_same_records() {
    // The reason the dependency is justified at all (research.md D4): the block-list
    // columns repeat the whole prefix per row, which is what columnar compression is
    // for. If this ever stopped holding, the feature would be carrying arrow for
    // nothing.
    let (d, _) = description();
    let span = 1_500.0;

    let mut json_bytes: Vec<u8> = Vec::new();
    let mut sim = Simulation::new(&d, 14).unwrap();
    let mut jw = JsonlWriter::new(&mut json_bytes, "equiv", BLOCK_SIZE);
    sim.run_until(span, &mut |s, t| jw.write(s, t).unwrap());
    jw.finish().unwrap();

    let mut pq_bytes: Vec<u8> = Vec::new();
    let mut sim = Simulation::new(&d, 14).unwrap();
    let mut pw = ParquetWriter::new(&mut pq_bytes, "equiv", BLOCK_SIZE).unwrap();
    sim.run_until(span, &mut |s, t| pw.write(s, t).unwrap());
    pw.finish().unwrap();

    assert!(
        pq_bytes.len() < json_bytes.len(),
        "parquet {} bytes against JSONL {} — columnar compression is not paying \
         for the arrow dependency",
        pq_bytes.len(),
        json_bytes.len()
    );
}

#[test]
fn a_parquet_trace_is_byte_identical_at_a_fixed_seed() {
    // The container must not introduce nondeterminism of its own — a timestamp in
    // the footer, say, or a hash-ordered dictionary page.
    let write = |seed: u64| {
        let (d, _) = description();
        let mut bytes: Vec<u8> = Vec::new();
        let mut sim = Simulation::new(&d, seed).unwrap();
        let mut w = ParquetWriter::new(&mut bytes, "equiv", BLOCK_SIZE).unwrap();
        sim.run_until(400.0, &mut |s, t| w.write(s, t).unwrap());
        w.finish().unwrap();
        bytes
    };
    assert_eq!(write(15), write(15));
    assert_ne!(write(15), write(16));
}
