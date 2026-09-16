//! T062c — the Mooncake conversion, checked against the upstream invariants and
//! against the one property that would otherwise fail silently.
//!
//! # The assertion that matters
//!
//! Renumbering identifiers **per session** instead of across the whole output yields a
//! file that parses, satisfies every structural invariant upstream states, loads,
//! replays, and reports plausible cache numbers — with all cross-session reuse gone.
//! FR-078 exists for that single failure.
//!
//! So `two_sessions_that_shared_an_instance_still_share_identifiers` is the test this
//! file is for. Everything else here is the structural conformance that a reader of
//! upstream's traces would expect, and none of it would catch that bug.
//!
//! The upstream figures the conformance checks are drawn from were read from
//! `kvcache-ai/Mooncake` @ `main:FAST25-release/traces/conversation_trace.jsonl`, 2 328
//! rows, and are recorded in `contracts/trace-interop.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufWriter;

use tempfile::TempDir;
use workload_model::description::WorkloadDescription;
use workload_model::sim::Simulation;
use workload_trace::jsonl::JsonlWriter;
use workload_trace::mooncake::{convert_jsonl, MooncakeWriter};
use workload_trace::record::InvocationRecord;

const BLOCK_SIZE: u64 = 16;

/// One shared class of exactly **one** immortal instance, so every session
/// necessarily draws it and cross-session sharing is guaranteed to exist — the
/// condition the load-bearing test needs in order to be able to fail.
fn description() -> WorkloadDescription {
    r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 5}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 6}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 4}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 8}
"#
    .parse()
    .unwrap()
}

/// Emit and convert, returning (mooncake rows, the trace's own rows).
fn convert(
    seed: u64,
    span: f64,
    dir: &std::path::Path,
) -> (Vec<serde_json::Value>, Vec<InvocationRecord>) {
    let d = description();

    let jsonl_path = dir.join("trace.jsonl");
    let mut sim = Simulation::new(&d, seed).unwrap();
    let file = std::fs::File::create(&jsonl_path).unwrap();
    let mut writer = JsonlWriter::new(BufWriter::new(file), "mc", BLOCK_SIZE);
    let mut records = Vec::new();
    sim.run_until(span, &mut |s, t| {
        writer.write(s, t).unwrap();
        records.push(InvocationRecord::from_turn("mc", s, t, BLOCK_SIZE));
    });
    writer.finish().unwrap();

    let mut out: Vec<u8> = Vec::new();
    let input = std::io::BufReader::new(std::fs::File::open(&jsonl_path).unwrap());
    convert_jsonl(input, &mut out, BLOCK_SIZE).unwrap();
    let rows = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (rows, records)
}

fn ids_of(row: &serde_json::Value) -> Vec<i64> {
    row["hash_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect()
}

#[test]
fn two_sessions_that_shared_an_instance_still_share_identifiers() {
    // **The load-bearing assertion (FR-078).** Renumbering per session would leave every
    // structural invariant intact and destroy all cross-session reuse, so this is
    // stated in terms of the trace's own keys rather than in terms of the output: two
    // rows from *different* sessions that shared a key in the source must share an
    // identifier in the conversion, and rows that shared no key must share none.
    let tmp = TempDir::new().unwrap();
    let (rows, records) = convert(31, 400.0, tmp.path());
    assert_eq!(rows.len(), records.len(), "a row was dropped");

    // Group rows by source session, and collect each session's identifier set.
    let mut by_session: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut keys_by_session: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for (row, record) in rows.iter().zip(&records) {
        by_session
            .entry(record.session_id.clone())
            .or_default()
            .extend(ids_of(row));
        keys_by_session
            .entry(record.session_id.clone())
            .or_default()
            .extend(&record.full_input_blocks);
    }
    assert!(by_session.len() >= 6, "too few sessions to be a test");

    let sessions: Vec<&String> = by_session.keys().collect();
    let mut compared = 0;
    let mut sharing_pairs = 0;
    for (i, a) in sessions.iter().enumerate() {
        for b in &sessions[i + 1..] {
            let shared_keys = keys_by_session[*a]
                .intersection(&keys_by_session[*b])
                .count();
            let shared_ids = by_session[*a].intersection(&by_session[*b]).count();
            assert_eq!(
                shared_ids, shared_keys,
                "sessions {a} and {b} shared {shared_keys} keys in the trace but \
                 {shared_ids} identifiers after conversion — renumbering has changed \
                 what the workload shares"
            );
            if shared_keys > 0 {
                sharing_pairs += 1;
            }
            compared += 1;
        }
    }
    assert!(compared >= 15, "only {compared} pairs compared");
    assert!(
        sharing_pairs > 0,
        "no pair of sessions shared anything, so this test could not have failed"
    );
}

#[test]
fn the_conversion_conforms_to_the_upstream_invariants() {
    // Structural conformance a reader of upstream's traces would expect. Note that
    // *none* of this catches per-session renumbering, which is why the test above
    // exists.
    let tmp = TempDir::new().unwrap();
    let (rows, _) = convert(32, 400.0, tmp.path());
    assert!(rows.len() > 20);

    let mut previous_ts = i64::MIN;
    let mut all: BTreeSet<i64> = BTreeSet::new();
    for row in &rows {
        // Exactly four fields, upstream's set.
        let obj = row.as_object().unwrap();
        assert_eq!(obj.len(), 4, "unexpected fields: {:?}", obj.keys());
        for f in ["timestamp", "input_length", "output_length", "hash_ids"] {
            assert!(obj.contains_key(f), "missing {f}");
        }

        // `len(hash_ids) == ceil(input_length / block_size)`, 2328/2328 upstream.
        let ids = ids_of(row);
        let input_length = row["input_length"].as_i64().unwrap();
        assert_eq!(
            ids.len() as i64,
            (input_length as f64 / BLOCK_SIZE as f64).ceil() as i64
        );
        assert!(!ids.is_empty(), "a row with no prompt");

        // Non-decreasing timestamps.
        let ts = row["timestamp"].as_i64().unwrap();
        assert!(ts >= previous_ts, "timestamps went backwards");
        previous_ts = ts;

        all.extend(&ids);
    }

    // Dense and zero-based over the whole file, as upstream's 0..44683 is.
    assert_eq!(*all.iter().next().unwrap(), 0);
    assert_eq!(*all.iter().next_back().unwrap(), all.len() as i64 - 1);
}

#[test]
fn the_prompt_prefix_structure_survives_the_conversion() {
    // The append-only chain must still be visible as a growing identifier prefix, or a
    // consumer would measure no intra-session reuse either.
    //
    // **Known weak, and recorded rather than left implied.** Renumbering from zero on
    // every row also produces 0,1,2,... prefixes, so this passes under that bug —
    // measured. It shows identifiers are assigned in prompt order; it does not show
    // reuse is preserved. `two_sessions_that_shared_an_instance_still_share_identifiers`
    // is what catches that, and it does.
    let tmp = TempDir::new().unwrap();
    let (rows, records) = convert(33, 400.0, tmp.path());

    let mut previous: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut extensions = 0;
    for (row, record) in rows.iter().zip(&records) {
        let ids = ids_of(row);
        if let Some(before) = previous.get(&record.session_id) {
            assert_eq!(
                &ids[..before.len()],
                &before[..],
                "session {} turn {} is not an extension of its predecessor",
                record.session_id,
                record.invocation_index
            );
            extensions += 1;
        }
        previous.insert(record.session_id.clone(), ids);
    }
    assert!(extensions > 10, "only {extensions} extensions checked");
}

#[test]
fn a_conversion_is_byte_identical_at_a_fixed_seed() {
    let tmp = TempDir::new().unwrap();
    let text = |seed: u64, sub: &str| {
        let dir = tmp.path().join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        let (rows, _) = convert(seed, 300.0, &dir);
        rows.iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(text(34, "a"), text(34, "b"));
    assert_ne!(text(34, "c"), text(35, "d"));
}

#[test]
fn both_entry_points_produce_the_same_bytes() {
    // FR-075a: `emit --mooncake` writes in the same pass, `convert --to mooncake` reads
    // a stored trace, and they must be one projection rather than two.
    let tmp = TempDir::new().unwrap();
    let d = description();
    let span = 300.0;

    let mut direct: Vec<u8> = Vec::new();
    let mut sim = Simulation::new(&d, 36).unwrap();
    let mut w = MooncakeWriter::new(&mut direct, BLOCK_SIZE);
    sim.run_until(span, &mut |s, t| {
        let record = InvocationRecord::from_turn("mc", s, t, BLOCK_SIZE);
        w.write_record(&record).unwrap();
    });
    let direct_stats = w.finish().unwrap();

    let jsonl_path = tmp.path().join("trace.jsonl");
    let mut sim = Simulation::new(&d, 36).unwrap();
    let file = std::fs::File::create(&jsonl_path).unwrap();
    let mut writer = JsonlWriter::new(BufWriter::new(file), "mc", BLOCK_SIZE);
    sim.run_until(span, &mut |s, t| writer.write(s, t).unwrap());
    writer.finish().unwrap();

    let mut through_file: Vec<u8> = Vec::new();
    let input = std::io::BufReader::new(std::fs::File::open(&jsonl_path).unwrap());
    let file_stats = convert_jsonl(input, &mut through_file, BLOCK_SIZE).unwrap();

    assert_eq!(
        direct_stats, file_stats,
        "the entry points disagree on counts"
    );
    assert_eq!(
        direct, through_file,
        "the entry points produced different bytes"
    );
}
