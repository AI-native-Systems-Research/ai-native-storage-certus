//! Projection onto the shape `apps/eviction-replay-benchmark` reads.
//!
//! `research.md` D1 settled this: the simulator does not read the emitted schema, and
//! teaching it to would make this feature's correctness depend on a change in another
//! app. So the projection lives here, and the conversion is a pure one — it needs no
//! information the trace lacks.
//!
//! # The shape, and the two things about it that matter
//!
//! Four fields: `{chat_id, parent_chat_id, hash_ids, type}`.
//!
//! **`chat_id` must be unique across the whole file, not per session.** The loader
//! keeps a single `chat_id -> root` map, so two sessions both numbering their turns
//! from zero would collide and be merged into one conversation. Turn identity here is
//! therefore a running counter over emitted rows, with each session remembering the
//! counter of its previous turn.
//!
//! **`hash_ids` is the turn's prompt, not its whole chain.** The simulator counts
//! every listed key as one cache access, and a turn *reads* its prompt while it
//! *stores* its output. No key is lost by leaving output out: a turn's output is part
//! of the next turn's prompt, so it appears then. The exception is the final turn's
//! output, which is stored and never read again — correctly absent from a trace of
//! accesses.
//!
//! # What the projection deliberately drops
//!
//! Virtual time, session identity as such, the input/output distinction, and the
//! trailing-partial-block convention. The simulator models a cache-access sequence,
//! not a workload; those fields have no reader there. Recorded because FR-077 requires
//! a conversion to say what it drops, and because a converted file must never be
//! mistaken for the trace it came from.
//!
//! # The failure mode this projection has to avoid
//!
//! The loader derives a session by walking `parent_chat_id` to a root, so a wrong
//! chain **still loads**. It does not error — it silently collapses every session
//! into one, or splits one into many, and a lineage-aware eviction policy then scores
//! against a workload nobody described. That is what `tests/simulator.rs` checks, and
//! it is why the parent link is asserted here rather than assumed.
//!
//! # Examples
//!
//! Two interleaved sessions, each numbering its turns from zero. `chat_id` is a
//! counter over emitted rows, so they do not collide, and each session's chain is
//! rooted at −1:
//!
//! ```
//! use workload_trace::simulator::convert_jsonl;
//!
//! let trace = concat!(
//!     r#"{"session_id":"a","invocation_index":0,"parent_invocation":-1,"full_input_blocks":[1,2]}"#, "\n",
//!     r#"{"session_id":"b","invocation_index":0,"parent_invocation":-1,"full_input_blocks":[3,4]}"#, "\n",
//!     r#"{"session_id":"a","invocation_index":1,"parent_invocation":0,"full_input_blocks":[1,2,5]}"#, "\n",
//! );
//!
//! let mut out = Vec::new();
//! let stats = convert_jsonl(trace.as_bytes(), &mut out).unwrap();
//! assert_eq!(stats.records, 3);
//! assert_eq!(stats.sessions, 2);
//! assert_eq!(stats.distinct_keys, 5);
//!
//! let text = String::from_utf8(out).unwrap();
//! let lines: Vec<&str> = text.lines().collect();
//! assert_eq!(lines[0], r#"{"chat_id":0,"parent_chat_id":-1,"hash_ids":[1,2],"type":"request"}"#);
//! assert_eq!(lines[1], r#"{"chat_id":1,"parent_chat_id":-1,"hash_ids":[3,4],"type":"request"}"#);
//! // Session a's second turn points at chat_id 0, not at the row before it.
//! assert_eq!(lines[2], r#"{"chat_id":2,"parent_chat_id":0,"hash_ids":[1,2,5],"type":"request"}"#);
//! ```
//!
//! A chain that does not hold together is refused rather than silently reshaped:
//!
//! ```
//! use workload_trace::simulator::convert_jsonl;
//!
//! // Claims a parent, but the converter has no earlier turn for this session.
//! let orphan = r#"{"session_id":"a","invocation_index":3,"parent_invocation":2,"full_input_blocks":[1]}"#;
//! let mut out = Vec::new();
//! let err = convert_jsonl(orphan.as_bytes(), &mut out).unwrap_err();
//! assert!(err.to_string().contains("silently reshaped"));
//! ```

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

use crate::record::InvocationRecord;

/// One record in the simulator's format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SimulatorRecord {
    /// Turn identity, unique across the whole file.
    pub chat_id: i64,
    /// The previous turn of the same session, or −1 at a session root.
    pub parent_chat_id: i64,
    /// The prompt's blocks, in prefix order. Each is one cache access.
    pub hash_ids: Vec<u64>,
    /// Request type, retained by the loader for reporting only.
    #[serde(rename = "type")]
    pub request_type: &'static str,
}

/// The fields the projection needs from a trace row.
///
/// A reader of its own rather than deserialising [`InvocationRecord`]: the projection
/// needs four fields out of seventeen, and a narrow reader cannot be broken by a
/// change to a field it does not use.
#[derive(Debug, Clone, Deserialize)]
struct Row {
    session_id: String,
    invocation_index: i64,
    parent_invocation: i64,
    full_input_blocks: Vec<u64>,
}

/// Statistics that let a conversion be checked against the run that produced it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimulatorStats {
    /// Rows written.
    pub records: u64,
    /// Rows dropped for having no blocks, which the loader would skip anyway.
    pub dropped_empty: u64,
    /// Distinct sessions, which must match the trace's own count.
    pub sessions: u64,
    /// Distinct keys, which must match the emit report's.
    pub distinct_keys: u64,
    /// Total key references.
    pub key_references: u64,
}

impl SimulatorStats {
    /// The losses this projection incurred, for a report to print (FR-077).
    ///
    /// The dropped-empty entry appears only when something was dropped: a declaration that
    /// names a loss of zero rows is noise, and noise is what stops these being read.
    pub fn declared_losses(&self) -> Vec<String> {
        let mut losses = vec![
            "virtual time: there is no timestamp field, so the records carry their order \
             and nothing else — arrival rate, think time and realised concurrency are gone"
                .to_string(),
            "session identity: session_id becomes a chat_id/parent_chat_id chain, so the \
             grouping survives as that chain but the trace's own session names do not"
                .to_string(),
            "the generated run: only the prompt's blocks become hash_ids, so output keys \
             are never accessed"
                .to_string(),
            "partial_final_valid and block geometry: the format carries neither, so a \
             trailing partial block is indistinguishable from a full one"
                .to_string(),
        ];
        if self.dropped_empty > 0 {
            losses.push(format!(
                "{} invocation{} with no blocks: the loader skips them, so they are dropped \
                 rather than written and the file's row count is short by that many",
                self.dropped_empty,
                if self.dropped_empty == 1 { "" } else { "s" }
            ));
        }
        losses
    }
}

/// Writes the simulator's projection.
#[derive(Debug)]
pub struct SimulatorWriter<W: Write> {
    sink: W,
    next_chat_id: i64,
    /// Last chat id per session, which is the next turn's parent.
    last: HashMap<String, i64>,
    distinct: std::collections::BTreeSet<u64>,
    stats: SimulatorStats,
}

impl<W: Write> SimulatorWriter<W> {
    /// A writer for one simulator file.
    pub fn new(sink: W) -> Self {
        Self {
            sink,
            next_chat_id: 0,
            last: HashMap::new(),
            distinct: Default::default(),
            stats: SimulatorStats::default(),
        }
    }

    /// Project and write one invocation record.
    ///
    /// # Errors
    ///
    /// If serialisation or the sink fails.
    pub fn write_record(&mut self, record: &InvocationRecord) -> io::Result<()> {
        self.write_parts(
            &record.session_id,
            record.invocation_index,
            record.parent_invocation,
            &record.full_input_blocks,
        )
    }

    fn write_parts(
        &mut self,
        session_id: &str,
        invocation_index: i64,
        parent_invocation: i64,
        blocks: &[u64],
    ) -> io::Result<()> {
        if blocks.is_empty() {
            // The loader skips these, so writing them would make the file's row count
            // disagree with what the simulator sees.
            self.stats.dropped_empty += 1;
            return Ok(());
        }
        let chat_id = self.next_chat_id;
        self.next_chat_id += 1;

        let previous = self.last.get(session_id).copied();
        // A row claiming a parent must have one on record, and a row claiming to be a
        // root must not. Either mismatch would still *load* — it would silently
        // reshape the conversation graph — so it is checked rather than trusted.
        let parent_chat_id = match (parent_invocation, previous) {
            (p, Some(prev)) if p >= 0 => prev,
            (p, None) if p < 0 => -1,
            (p, prev) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "session {session_id} turn {invocation_index} declares \
                         parent_invocation {p} but the converter has {} for it; the \
                         conversation graph would be silently reshaped",
                        prev.map_or("no previous turn".to_string(), |c| format!("chat_id {c}"))
                    ),
                ));
            }
        };
        self.last.insert(session_id.to_string(), chat_id);

        for k in blocks {
            self.distinct.insert(*k);
        }
        self.stats.records += 1;
        self.stats.key_references += blocks.len() as u64;

        let out = SimulatorRecord {
            chat_id,
            parent_chat_id,
            hash_ids: blocks.to_vec(),
            request_type: "request",
        };
        let line = serde_json::to_string(&out)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.sink.write_all(line.as_bytes())?;
        self.sink.write_all(b"\n")?;
        Ok(())
    }

    /// Flush and return the statistics.
    ///
    /// # Errors
    ///
    /// If the flush fails.
    pub fn finish(mut self) -> io::Result<SimulatorStats> {
        self.sink.flush()?;
        self.stats.sessions = self.last.len() as u64;
        self.stats.distinct_keys = self.distinct.len() as u64;
        Ok(self.stats)
    }
}

/// Convert an emitted JSONL trace into the simulator's shape.
///
/// The `convert` entry point: its input is the *schema*, so it works on any trace in
/// it — including the real ones in the corpus, which is what makes a real workload and
/// a generated one comparable through the identical projection (FR-075a).
///
/// # Errors
///
/// If a line is not a trace row, or the parent chain does not hold together.
pub fn convert_jsonl<R: BufRead, W: Write>(input: R, output: W) -> io::Result<SimulatorStats> {
    let mut writer = SimulatorWriter::new(output);
    for (lineno, line) in input.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let row: Row = serde_json::from_str(line).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line {}: not a trace row: {e}", lineno + 1),
            )
        })?;
        writer.write_parts(
            &row.session_id,
            row.invocation_index,
            row.parent_invocation,
            &row.full_input_blocks,
        )?;
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(session: &str, index: i64, blocks: &[u64]) -> String {
        let parent = index - 1;
        format!(
            r#"{{"session_id":"{session}","invocation_index":{index},"parent_invocation":{parent},"full_input_blocks":{blocks:?}}}"#
        )
    }

    #[test]
    fn chat_ids_are_unique_across_sessions() {
        // The failure this guards: the loader keeps ONE chat_id -> root map, so two
        // sessions numbering turns from zero would be merged into one conversation.
        let input = format!(
            "{}\n{}\n{}\n{}\n",
            row("a", 0, &[1, 2]),
            row("b", 0, &[3, 4]),
            row("a", 1, &[1, 2, 5]),
            row("b", 1, &[3, 4, 6]),
        );
        let mut out = Vec::new();
        let stats = convert_jsonl(input.as_bytes(), &mut out).unwrap();
        assert_eq!(stats.records, 4);
        assert_eq!(stats.sessions, 2);

        let ids: Vec<i64> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["chat_id"]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        let distinct: std::collections::BTreeSet<&i64> = ids.iter().collect();
        assert_eq!(distinct.len(), ids.len(), "chat_ids repeat: {ids:?}");
    }

    #[test]
    fn each_session_forms_one_chain_rooted_at_minus_one() {
        let input = format!(
            "{}\n{}\n{}\n",
            row("a", 0, &[1]),
            row("a", 1, &[1, 2]),
            row("a", 2, &[1, 2, 3]),
        );
        let mut out = Vec::new();
        convert_jsonl(input.as_bytes(), &mut out).unwrap();
        let rows: Vec<serde_json::Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows[0]["parent_chat_id"], -1);
        assert_eq!(rows[1]["parent_chat_id"], rows[0]["chat_id"]);
        assert_eq!(rows[2]["parent_chat_id"], rows[1]["chat_id"]);
    }

    #[test]
    fn a_row_with_no_blocks_is_dropped_and_counted() {
        // The loader skips these, so emitting them would make the file's row count
        // disagree with what the simulator actually replays.
        let input = format!("{}\n{}\n", row("a", 0, &[]), row("a", 0, &[7]));
        let mut out = Vec::new();
        let stats = convert_jsonl(input.as_bytes(), &mut out).unwrap();
        assert_eq!(stats.dropped_empty, 1);
        assert_eq!(stats.records, 1);
        assert_eq!(String::from_utf8(out).unwrap().lines().count(), 1);
    }

    #[test]
    fn a_broken_parent_chain_is_refused_rather_than_silently_reshaped() {
        // The whole reason the link is checked: a wrong chain still LOADS. The loader
        // would collapse or split conversations and a lineage-aware policy would score
        // against a workload nobody described.
        let orphan = r#"{"session_id":"a","invocation_index":3,"parent_invocation":2,"full_input_blocks":[1]}"#;
        let mut out = Vec::new();
        let err = convert_jsonl(orphan.as_bytes(), &mut out).unwrap_err();
        assert!(
            err.to_string().contains("silently reshaped"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn a_root_that_claims_a_parent_is_refused_too() {
        let input = format!("{}\n{}\n", row("a", 0, &[1]), row("a", 0, &[1, 2]));
        // The second row claims index 0 with parent -1, but the session already has a
        // turn on record — an ambiguity, not a chain.
        let mut out = Vec::new();
        let err = convert_jsonl(input.as_bytes(), &mut out).unwrap_err();
        assert!(err.to_string().contains("silently reshaped"));
    }

    #[test]
    fn statistics_match_the_rows_written() {
        let input = format!(
            "{}\n{}\n",
            row("a", 0, &[1, 2, 3]),
            row("a", 1, &[1, 2, 3, 4]),
        );
        let mut out = Vec::new();
        let stats = convert_jsonl(input.as_bytes(), &mut out).unwrap();
        assert_eq!(stats.records, 2);
        assert_eq!(stats.key_references, 7);
        assert_eq!(stats.distinct_keys, 4);
        assert_eq!(stats.sessions, 1);
    }

    #[test]
    fn the_declared_losses_name_what_this_shape_cannot_carry() {
        let mut out = Vec::new();
        let stats =
            convert_jsonl(format!("{}\n", row("a", 0, &[1, 2])).as_bytes(), &mut out).unwrap();
        let losses = stats.declared_losses();
        for expected in [
            "virtual time",
            "session identity",
            "the generated run",
            "partial_final_valid",
        ] {
            assert!(
                losses.iter().any(|l| l.contains(expected)),
                "no loss names {expected:?}: {losses:?}"
            );
        }
        // Nothing was dropped, so nothing claims to have been: a declaration that names a
        // loss of zero rows trains a reader to skip the list.
        assert_eq!(stats.dropped_empty, 0);
        assert!(
            !losses.iter().any(|l| l.contains("no blocks")),
            "a clean conversion must not report dropped rows: {losses:?}"
        );

        // And when rows *are* dropped it says so, with the count — the file's row count
        // disagreeing with the trace's is otherwise unexplained.
        let with_empty = format!("{}\n{}\n", row("a", 0, &[]), row("a", 0, &[1]));
        let mut out2 = Vec::new();
        let dropped = convert_jsonl(with_empty.as_bytes(), &mut out2).unwrap();
        assert_eq!(dropped.dropped_empty, 1);
        assert!(
            dropped
                .declared_losses()
                .iter()
                .any(|l| l.contains("1 invocation with no blocks")),
            "{:?}",
            dropped.declared_losses()
        );
    }
}
