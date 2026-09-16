//! The JSONL writer: one record per invocation, written as its turn happens.
//!
//! # It streams, and that is the point
//!
//! A record is complete the moment its turn is simulated, so this writer takes one
//! and hands it to the sink. Nothing is buffered and nothing is held to be sorted
//! at the end, which is what lets an emit run of any span work in bounded memory.
//!
//! That is not a free property of the schema — it is a consequence of the record
//! being one *invocation* rather than one session. A session-grouped format cannot
//! write a line until the session ends, so emitting it would mean holding every
//! in-flight session; `contracts/trace-interop.md` records where that was measured.
//!
//! # Statistics are accumulated on the way past
//!
//! The manifest needs session, invocation and distinct-key counts, and it is written
//! last (FR-073). Counting distinct keys means keeping a set, which is the one
//! unbounded structure in the writer — bounded by the run's distinct key space, which
//! the projection already reports. Recorded rather than hidden, because it is the
//! thing that would fail first on a very long run.
//!
//! # Examples
//!
//! ```no_run
//! use std::fs::File;
//! use std::io::BufWriter;
//! use workload_trace::jsonl::JsonlWriter;
//!
//! let file = BufWriter::new(File::create("part-0.jsonl").unwrap());
//! let mut writer = JsonlWriter::new(file, "demo", 16);
//! // ... writer.write(session, turn)? per turn ...
//! let stats = writer.finish().unwrap();
//! assert_eq!(stats.invocations, 0);
//! ```

use std::collections::BTreeSet;
use std::io::{self, Write};

use workload_model::keys::CacheKey;
use workload_model::session::{Session, Turn};

use crate::manifest::BlockStats;
use crate::record::InvocationRecord;

/// Writes invocation records as newline-delimited JSON.
#[derive(Debug)]
pub struct JsonlWriter<W: Write> {
    sink: W,
    trace_id: String,
    block_size: u64,
    sessions: BTreeSet<u64>,
    distinct_keys: BTreeSet<CacheKey>,
    invocations: u64,
    /// Last row per session, for the append-only row check.
    previous: std::collections::HashMap<String, InvocationRecord>,
    verify: bool,
}

impl<W: Write> JsonlWriter<W> {
    /// A writer for one container file.
    ///
    /// Row verification is **on** by default. FR-058 requires every emitted row to
    /// satisfy the schema's invariants, and the check costs a comparison against the
    /// previous row of the same session — cheap next to serialising the row at all,
    /// and the only place the append-only property can be caught at the moment it is
    /// written rather than after the fact.
    pub fn new(sink: W, trace_id: &str, block_size: u64) -> Self {
        Self {
            sink,
            trace_id: trace_id.to_string(),
            block_size,
            sessions: BTreeSet::new(),
            distinct_keys: BTreeSet::new(),
            invocations: 0,
            previous: std::collections::HashMap::new(),
            verify: true,
        }
    }

    /// Turn row verification off.
    ///
    /// Only for a benchmark measuring the writer itself. A run that writes
    /// unverified rows cannot claim FR-058, so this is not a performance knob for
    /// ordinary use.
    pub fn without_verification(mut self) -> Self {
        self.verify = false;
        self
    }

    /// Write one turn's record.
    ///
    /// # Errors
    ///
    /// If serialisation or the sink fails, or — when verification is on — if the row
    /// violates an invariant the trace's own manifest declares.
    pub fn write(&mut self, session: &Session, turn: &Turn) -> io::Result<()> {
        let record = InvocationRecord::from_turn(&self.trace_id, session, turn, self.block_size);
        if self.verify {
            let previous = self.previous.get(&record.session_id);
            record.check(self.block_size, previous).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("emitted row violates the trace schema: {e}"),
                )
            })?;
        }

        self.sessions.insert(session.id());
        self.distinct_keys.extend(&record.full_input_blocks);
        self.distinct_keys.extend(&record.full_output_blocks);
        self.invocations += 1;

        let line = serde_json::to_string(&record)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.sink.write_all(line.as_bytes())?;
        self.sink.write_all(b"\n")?;

        if self.verify {
            self.previous.insert(record.session_id.clone(), record);
        }
        Ok(())
    }

    /// Flush and return the counts the manifest needs.
    ///
    /// # Errors
    ///
    /// If the flush fails.
    pub fn finish(mut self) -> io::Result<BlockStats> {
        self.sink.flush()?;
        Ok(BlockStats {
            sessions: self.sessions.len() as u64,
            invocations: self.invocations,
            unique_blocks: self.distinct_keys.len() as u64,
        })
    }

    /// Counts so far, without consuming the writer.
    pub fn stats(&self) -> BlockStats {
        BlockStats {
            sessions: self.sessions.len() as u64,
            invocations: self.invocations,
            unique_blocks: self.distinct_keys.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workload_model::description::WorkloadDescription;
    use workload_model::sim::Simulation;

    fn description() -> WorkloadDescription {
        r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 3}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 4}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 4}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 10}
"#
        .parse()
        .unwrap()
    }

    /// Emit a run into a byte buffer and return (lines, stats).
    fn emit(seed: u64, span: f64) -> (Vec<String>, BlockStats) {
        let d = description();
        let mut sim = Simulation::new(&d, seed).unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let mut writer = JsonlWriter::new(&mut buf, "test", 16);
        sim.run_until(span, &mut |s, t| {
            writer.write(s, t).expect("row must be valid")
        });
        let stats = writer.finish().unwrap();
        let text = String::from_utf8(buf).unwrap();
        (text.lines().map(str::to_string).collect(), stats)
    }

    #[test]
    fn every_row_is_one_invocation_and_parses_as_json() {
        let (lines, stats) = emit(1, 400.0);
        assert!(lines.len() > 10);
        assert_eq!(lines.len() as u64, stats.invocations);
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
            assert!(v.get("session_id").is_some());
            assert!(v.get("full_input_blocks").is_some());
        }
    }

    #[test]
    fn nulls_are_present_rather_than_fields_being_absent() {
        // FR-056: a reader learns what a trace supports by reading it. An absent
        // field would force it to guess; a null says "this trace does not have one".
        let (lines, _) = emit(2, 100.0);
        let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        for field in ["request_end", "model", "partial_final_valid"] {
            assert!(
                v.get(field).is_some_and(|x| x.is_null()),
                "{field} should be present and null"
            );
        }
    }

    #[test]
    fn lengths_are_tokens_and_match_the_block_lists_exactly() {
        // The invariant the corpus's own readers rely on, and the reason lengths are
        // in tokens rather than blocks.
        let (lines, _) = emit(3, 200.0);
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let blocks = v["full_input_blocks"].as_array().unwrap().len() as i64;
            assert_eq!(v["input_length"].as_i64().unwrap(), blocks * 16);
            let out = v["full_output_blocks"].as_array().unwrap().len() as i64;
            assert_eq!(v["output_length"].as_i64().unwrap(), out * 16);
        }
    }

    #[test]
    fn the_file_is_byte_identical_at_a_fixed_seed_and_differs_otherwise() {
        let (a, _) = emit(7, 300.0);
        let (b, _) = emit(7, 300.0);
        let (c, _) = emit(8, 300.0);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn rows_are_in_virtual_time_order_across_sessions() {
        // The streaming property: rows come out in clock order, so a reader can
        // process the file as a time series without sorting it.
        let (lines, _) = emit(4, 500.0);
        let mut previous = f64::NEG_INFINITY;
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let at = v["request_start"].as_f64().unwrap();
            assert!(at >= previous, "rows are not in virtual-time order");
            previous = at;
        }
    }

    #[test]
    fn statistics_count_sessions_invocations_and_distinct_keys() {
        let (lines, stats) = emit(5, 400.0);
        assert_eq!(stats.invocations, lines.len() as u64);
        assert!(stats.sessions >= 4, "at least the seeded generation");
        // Distinct keys must be fewer than the total references, because the whole
        // point of the format is that prefixes repeat.
        let references: usize = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["full_input_blocks"].as_array().unwrap().len()
                    + v["full_output_blocks"].as_array().unwrap().len()
            })
            .sum();
        assert!(
            (stats.unique_blocks as usize) < references,
            "distinct {} against {references} references — no reuse at all",
            stats.unique_blocks
        );
    }

    #[test]
    fn verification_rejects_a_row_that_breaks_the_append_only_chain() {
        // The check must be able to fail, or turning it on proves nothing. Built by
        // hand, because the simulation cannot produce a broken chain.
        let d = description();
        let mut sim = Simulation::new(&d, 6).unwrap();
        let mut rows: Vec<InvocationRecord> = Vec::new();
        sim.run_until(100.0, &mut |s, t| {
            rows.push(InvocationRecord::from_turn("t", s, t, 16))
        });
        // Consecutive turns *of one session*, which are not adjacent rows: with
        // several sessions live the rows interleave in virtual time, which is the
        // streaming property the writer exists for.
        let mut by_session: std::collections::BTreeMap<String, Vec<InvocationRecord>> =
            Default::default();
        for r in rows {
            by_session.entry(r.session_id.clone()).or_default().push(r);
        }
        let pair = by_session
            .values()
            .find(|v| v.len() >= 2)
            .expect("a session with two turns");
        let (first, second) = (pair[0].clone(), pair[1].clone());
        assert_eq!(first.invocation_index + 1, second.invocation_index);

        assert!(second.check(16, Some(&first)).is_ok());
        let mut broken = second.clone();
        broken.full_input_blocks[0] ^= 1;
        let err = broken.check(16, Some(&first)).unwrap_err();
        assert!(err.contains("append-only"), "unexpected error: {err}");
    }

    #[test]
    fn verification_rejects_a_length_that_does_not_match_its_blocks() {
        let d = description();
        let mut sim = Simulation::new(&d, 9).unwrap();
        let mut rows: Vec<InvocationRecord> = Vec::new();
        sim.run_until(30.0, &mut |s, t| {
            rows.push(InvocationRecord::from_turn("t", s, t, 16))
        });
        let mut row = rows[0].clone();
        row.input_length += 1;
        let err = row.check(16, None).unwrap_err();
        assert!(err.contains("input_length"), "unexpected error: {err}");
    }
}
