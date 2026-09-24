//! Projection onto the shape `apps/eviction-replay-benchmark` reads.
//!
//! `research.md` D1 settled this: the simulator does not read the emitted schema, and
//! teaching it to would make this feature's correctness depend on a change in another
//! app. So the projection lives here, and the conversion is a pure one — it needs no
//! information the trace lacks.
//!
//! # The shape, and the two things about it that matter
//!
//! `{chat_id, parent_chat_id, timestamp, turn, type, input_length, output_length,
//! hash_ids}` — the format's full documented shape. `apps/eviction-replay-benchmark`
//! reads four of those and ignores the rest, which is a fact about that consumer, not
//! about the format.
//!
//! **`chat_id` must be unique across the whole file, not per session.** The loader
//! keeps a single `chat_id -> root` map, so two sessions both numbering their turns
//! from zero would collide and be merged into one conversation. Turn identity here is
//! therefore a running counter over emitted rows, with each session remembering the
//! counter of its previous turn.
//!
//! **`hash_ids` is the turn's prompt followed by the blocks it generated.** The
//! simulator counts every listed key as one cache access, and both are accesses: a turn
//! *reads* its prompt and *stores* its output.
//!
//! This corrects an earlier version that wrote the prompt alone, on the argument that a
//! turn's output reappears in the next turn's prompt so no key is lost. Two things
//! survive that argument. A real deployment inserts a generated block **when it is
//! generated** — vLLM's offloading connector calls `prepare_store` after each forward
//! pass (`knowledge/kv_IO_pattern.md`) — so prompt-only moved every store one turn
//! later than it happens. And the **final turn of every session** generates output that
//! is never read again, so prompt-only omitted it entirely while the real cache still
//! held it. The result understated capacity pressure and eviction opportunity, which is
//! precisely what this file exists to measure.
//!
//! # What the projection deliberately drops
//!
//! Session identity as such, which references are reads versus stores, and how full a
//! trailing partial block was. Recorded because FR-077 requires a conversion to say what
//! it drops, and because a converted file must never be mistaken for the trace it came
//! from.
//!
//! **Virtual time is NOT dropped**, though it used to be. The format carries
//! `timestamp`, `turn`, `input_length` and `output_length` alongside the four fields
//! `apps/eviction-replay-benchmark` reads, and that consumer ignoring them is no reason
//! for us not to write them: another reader of the same format can use them, and a file
//! that omits them is a subset of one consumer's needs rather than a Qwen-Bailian trace.
//!
//! # The failure mode this projection has to avoid
//!
//! The loader derives a session by walking `parent_chat_id` to a root, so a wrong
//! chain **still loads**. It does not error — it silently collapses every session
//! into one, or splits one into many, and a lineage-aware eviction policy then scores
//! against a workload nobody described. That is what `tests/qwen.rs` checks, and
//! it is why the parent link is asserted here rather than assumed.
//!
//! # Examples
//!
//! [`QwenWriter`] projects records as the simulation produces them. `chat_id` is a
//! **turn**, globally unique, and `parent_chat_id` links it to the previous turn of the
//! same session — the loader reconstructs a conversation by walking to the root.
//!
//! ```
//! use workload_trace::qwen::QwenWriter;
//! use workload_trace::record::InvocationRecord;
//!
//! fn rec(session: &str, index: i64, at: f64, input: Vec<u64>, output: Vec<u64>) -> InvocationRecord {
//!     InvocationRecord {
//!         trace_id: "demo".to_string(),
//!         session_id: session.to_string(),
//!         invocation_index: index,
//!         parent_invocation: index - 1,
//!         request_start: at,
//!         request_end: None,
//!         timestamp_kind: "virtual",
//!         timestamp_is_synthetic: true,
//!         model: None,
//!         input_length: input.len() as i64 * 16,
//!         output_length: output.len() as i64 * 16,
//!         reuse_from: Vec::new(),
//!         new_input_blocks: Vec::new(),
//!         new_output_blocks: Vec::new(),
//!         full_input_blocks: input,
//!         full_output_blocks: output,
//!         partial_final_valid: None,
//!     }
//! }
//!
//! let mut out = Vec::new();
//! let stats = {
//!     let mut w = QwenWriter::new(&mut out);
//!     w.write_record(&rec("a", 0, 0.0, vec![91, 92], vec![93])).unwrap();
//!     w.write_record(&rec("a", 1, 1.0, vec![91, 92, 93], vec![94])).unwrap();
//!     w.finish().unwrap()
//! };
//! assert_eq!(stats.records, 2);
//! assert_eq!(stats.sessions, 1);
//! ```

use std::collections::HashMap;
use std::io::{self, Write};

use serde::Serialize;

use crate::record::InvocationRecord;

/// One record in the Qwen-Bailian format.
///
/// Field order follows the format's own documented example, so a reader that has seen
/// a captured file sees ours the same way — and two runs of ours stay byte-comparable.
///
/// `PartialEq` without `Eq`, because `timestamp` is an `f64`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QwenRecord {
    /// Turn identity, unique across the whole file.
    ///
    /// **Not a session id**, despite the name: one `chat_id` is one *request*. A
    /// conversation is the chain reached by following [`Self::parent_chat_id`], and its
    /// root's `chat_id` is what the loader uses as the session identity.
    pub chat_id: i64,
    /// The previous turn of the same session, or −1 at a session root.
    ///
    /// A parent *turn*, never a parent session. The format can express a tree — a real
    /// captured file records a regenerated answer as a branch — but this generator's sessions
    /// are append-only (`record.rs` sets `parent_invocation = index - 1`), so every
    /// chain we write is strictly linear.
    pub parent_chat_id: i64,
    /// Virtual seconds on the run-global clock, as the format writes it (e.g. `61.1`).
    pub timestamp: f64,
    /// 0-based turn index within the conversation, which is `invocation_index`.
    pub turn: i64,
    /// Request type, retained by the loader for reporting only.
    #[serde(rename = "type")]
    pub request_type: &'static str,
    /// Prompt length in tokens.
    pub input_length: i64,
    /// Generated length in tokens.
    pub output_length: i64,
    /// The turn's prompt blocks in prefix order, then the blocks it generated. Each is
    /// one cache access; the format does not distinguish reads from stores.
    pub hash_ids: Vec<u64>,
}

/// The fields the projection needs from a trace row.
///
/// One turn's inputs to the projection.
///
/// A struct rather than eight positional arguments, which is both over clippy's limit
/// and the shape in which a `parent_invocation` ends up where an `invocation_index` was
/// meant.
struct Parts<'a> {
    session_id: &'a str,
    invocation_index: i64,
    parent_invocation: i64,
    request_start: f64,
    input_length: i64,
    output_length: i64,
    input: &'a [u64],
    output: &'a [u64],
}

/// Statistics that let a conversion be checked against the run that produced it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QwenStats {
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

impl QwenStats {
    /// The losses this projection incurred, for a report to print (FR-077).
    ///
    /// The dropped-empty entry appears only when something was dropped: a declaration that
    /// names a loss of zero rows is noise, and noise is what stops these being read.
    pub fn declared_losses(&self) -> Vec<String> {
        let mut losses = vec![
            "session identity: session_id becomes a chat_id/parent_chat_id chain, so the \
             grouping survives as that chain but the trace's own session names do not"
                .to_string(),
            "the read/store distinction: a turn's prompt blocks and the blocks it \
             generated are both hash_ids, prompt first, with nothing marking which is \
             which. The simulator counts each as one access, which is what a cache sees"
                .to_string(),
            "how full a trailing partial block was: its key is present and accessed like \
             any other, but the format carries no block geometry, so partial_final_valid \
             cannot be recovered"
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
pub struct QwenWriter<W: Write> {
    sink: W,
    next_chat_id: i64,
    /// Last chat id per session, which is the next turn's parent.
    last: HashMap<String, i64>,
    distinct: std::collections::BTreeSet<u64>,
    stats: QwenStats,
}

impl<W: Write> QwenWriter<W> {
    /// A writer for one simulator file.
    pub fn new(sink: W) -> Self {
        Self {
            sink,
            next_chat_id: 0,
            last: HashMap::new(),
            distinct: Default::default(),
            stats: QwenStats::default(),
        }
    }

    /// Project and write one invocation record.
    ///
    /// # Errors
    ///
    /// If serialisation or the sink fails.
    pub fn write_record(&mut self, record: &InvocationRecord) -> io::Result<()> {
        self.write_parts(Parts {
            session_id: &record.session_id,
            invocation_index: record.invocation_index,
            parent_invocation: record.parent_invocation,
            request_start: record.request_start,
            input_length: record.input_length,
            output_length: record.output_length,
            input: &record.full_input_blocks,
            output: &record.full_output_blocks,
        })
    }

    fn write_parts(&mut self, parts: Parts<'_>) -> io::Result<()> {
        let Parts {
            session_id,
            invocation_index,
            parent_invocation,
            request_start,
            input_length,
            output_length,
            input,
            output,
        } = parts;
        // Prompt reads then the blocks this turn generated: both are accesses, and the
        // store belongs at the turn that produced it. See the module docs.
        let blocks: Vec<u64> = input.iter().chain(output).copied().collect();
        let blocks = blocks.as_slice();
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

        let out = QwenRecord {
            chat_id,
            parent_chat_id,
            timestamp: request_start,
            turn: invocation_index,
            request_type: "request",
            input_length,
            output_length,
            hash_ids: blocks.to_vec(),
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
    pub fn finish(mut self) -> io::Result<QwenStats> {
        self.sink.flush()?;
        self.stats.sessions = self.last.len() as u64;
        self.stats.distinct_keys = self.distinct.len() as u64;
        Ok(self.stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row with no generated blocks, so these tests' counts stay about the prompt.
    /// `row_with_output` covers the generated run.
    ///
    /// Projected through `QwenWriter`, the path `emit` takes, so what is asserted is a
    /// claim about the Qwen-Bailian shape.
    fn row(session: &str, index: i64, blocks: &[u64]) -> InvocationRecord {
        row_with_output(session, index, blocks, &[])
    }

    fn row_with_output(
        session: &str,
        index: i64,
        input: &[u64],
        output: &[u64],
    ) -> InvocationRecord {
        InvocationRecord {
            trace_id: "t".to_string(),
            session_id: session.to_string(),
            invocation_index: index,
            parent_invocation: index - 1,
            request_start: index as f64,
            request_end: None,
            timestamp_kind: "virtual",
            timestamp_is_synthetic: true,
            model: None,
            input_length: input.len() as i64 * 16,
            output_length: output.len() as i64 * 16,
            reuse_from: Vec::new(),
            new_input_blocks: Vec::new(),
            new_output_blocks: Vec::new(),
            full_input_blocks: input.to_vec(),
            full_output_blocks: output.to_vec(),
            partial_final_valid: None,
        }
    }

    /// Project records through the writer, returning the bytes and the stats.
    fn project(records: &[InvocationRecord]) -> io::Result<(Vec<u8>, QwenStats)> {
        let mut out = Vec::new();
        let stats = {
            let mut w = QwenWriter::new(&mut out);
            for r in records {
                w.write_record(r)?;
            }
            w.finish()?
        };
        Ok((out, stats))
    }

    #[test]
    fn chat_ids_are_unique_across_sessions() {
        // The failure this guards: the loader keeps ONE chat_id -> root map, so two
        // sessions numbering turns from zero would be merged into one conversation.
        let input = vec![
            row("a", 0, &[1, 2]),
            row("b", 0, &[3, 4]),
            row("a", 1, &[1, 2, 5]),
            row("b", 1, &[3, 4, 6]),
        ];
        let (out, stats) = project(&input).unwrap();
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
        let input = vec![
            row("a", 0, &[1]),
            row("a", 1, &[1, 2]),
            row("a", 2, &[1, 2, 3]),
        ];
        let (out, _) = project(&input).unwrap();
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
        let input = vec![row("a", 0, &[]), row("a", 0, &[7])];
        let (out, stats) = project(&input).unwrap();
        assert_eq!(stats.dropped_empty, 1);
        assert_eq!(stats.records, 1);
        assert_eq!(String::from_utf8(out).unwrap().lines().count(), 1);
    }

    #[test]
    fn statistics_match_the_rows_written() {
        let input = vec![row("a", 0, &[1, 2, 3]), row("a", 1, &[1, 2, 3, 4])];
        let (_, stats) = project(&input).unwrap();
        assert_eq!(stats.records, 2);
        assert_eq!(stats.key_references, 7);
        assert_eq!(stats.distinct_keys, 4);
        assert_eq!(stats.sessions, 1);
    }

    #[test]
    fn a_turns_generated_blocks_join_its_hash_ids_after_the_prompt() {
        // Both are accesses, and the store belongs at the turn that produced it. The
        // last turn's output is the case prompt-only lost completely: nothing reads it,
        // so it appeared nowhere while the real cache still held it.
        let trace = vec![
            row_with_output("a", 0, &[1, 2], &[3]),
            row_with_output("a", 1, &[1, 2, 3], &[4]),
        ];
        let (out, stats) = project(&trace).unwrap();

        assert_eq!(stats.records, 2);
        assert_eq!(stats.key_references, 3 + 4);
        assert_eq!(stats.distinct_keys, 4, "key 4 is generated and never read");

        let rows: Vec<serde_json::Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows[0]["hash_ids"], serde_json::json!([1, 2, 3]));
        assert_eq!(rows[1]["hash_ids"], serde_json::json!([1, 2, 3, 4]));
    }

    #[test]
    fn a_row_whose_only_blocks_are_generated_is_still_written() {
        // A turn with an empty prompt still stores what it generated, so it is a record
        // rather than a dropped-empty. Only a turn that touches no blocks at all is
        // dropped, because the loader skips those.
        let trace = vec![
            row_with_output("a", 0, &[], &[9]),
            row_with_output("b", 0, &[], &[]),
        ];
        let (_, stats) = project(&trace).unwrap();
        assert_eq!(stats.records, 1);
        assert_eq!(stats.dropped_empty, 1);
        assert_eq!(stats.key_references, 1);
    }

    #[test]
    fn the_declared_losses_name_what_this_shape_cannot_carry() {
        let (_, stats) = project(&[row("a", 0, &[1, 2])]).unwrap();
        let losses = stats.declared_losses();
        for expected in [
            "session identity",
            "the read/store distinction",
            "partial_final_valid",
        ] {
            assert!(
                losses.iter().any(|l| l.contains(expected)),
                "no loss names {expected:?}: {losses:?}"
            );
        }
        // Virtual time must NOT be declared lost: the format has a `timestamp` field and
        // we fill it. A stale entry here would send a reader elsewhere for something this
        // file carries.
        assert!(
            !losses.iter().any(|l| l.contains("virtual time")),
            "timestamp is carried now: {losses:?}"
        );

        // Nothing was dropped, so nothing claims to have been: a declaration that names a
        // loss of zero rows trains a reader to skip the list.
        assert_eq!(stats.dropped_empty, 0);
        assert!(
            !losses.iter().any(|l| l.contains("no blocks")),
            "a clean conversion must not report dropped rows: {losses:?}"
        );

        // And when rows *are* dropped it says so, with the count — the file's row count
        // disagreeing with the trace's is otherwise unexplained.
        let with_empty = vec![row("a", 0, &[]), row("a", 0, &[1])];
        let (_, dropped) = project(&with_empty).unwrap();
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
