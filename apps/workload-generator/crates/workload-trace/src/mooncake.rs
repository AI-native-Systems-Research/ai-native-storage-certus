//! The Mooncake trace format — the one standard-format export (`research.md` D8).
//!
//! `contracts/trace-interop.md` is normative, and every property below was read from
//! `kvcache-ai/Mooncake` @ `main:FAST25-release/traces/conversation_trace.jsonl`
//! rather than recalled. Four fields per line, one line per request:
//!
//! ```text
//! {"timestamp": 0, "input_length": 6758, "output_length": 500,
//!  "hash_ids": [0, 1, 2, ..., 13]}
//! ```
//!
//! # Why this format and not the one we emit
//!
//! It is the only public block-level format that keeps both things a cache workload
//! needs: **per-request rows on a run-global clock**, so cross-session interleaving
//! survives, and **globally scoped identifiers**, so cross-session reuse can be
//! expressed at all. Verified on 2 328 upstream rows: `hash_ids` range 0..44 683 with
//! 44 684 distinct values — perfectly dense, global to the file.
//!
//! # Three conversions, each with a reason
//!
//! - **Time.** Ours is virtual seconds, theirs is **milliseconds**. Their corpus
//!   quantises to a 3 000 ms tick, but that is a property of *their* corpus and not of
//!   the format, so we write true millisecond values off the virtual clock.
//! - **Lengths.** Tokens, as ours are. Their invariant is
//!   `len(hash_ids) == ceil(input_length / block_size)`; our blocks are whole, so the
//!   division is exact and ceil, floor and length agree. Upstream's ceil exists because
//!   a real prompt does not end on a block boundary.
//! - **Identifiers.** Theirs are dense integers; ours are chained `u64` keys. So the
//!   conversion **renumbers**, and that is the one place this writer can be wrong in a
//!   way that looks like success — see below.
//!
//! # The mistake that would look like success
//!
//! Renumbering **per session** instead of across the whole output produces a file that
//! parses, loads, replays and reports plausible numbers — with every trace of
//! cross-session reuse silently gone, because two sessions sharing an object would be
//! given different identifiers for it. FR-078 exists for this one failure. The map here
//! is therefore built once for the whole writer, never reset, and
//! `tests/mooncake.rs` asserts that two sessions which shared an instance still share
//! identifiers afterwards.
//!
//! # What the conversion drops, declared rather than discovered (FR-077)
//!
//! - **Session grouping.** There is no `session_id` field. Sessions survive only as the
//!   prefix structure of `hash_ids`, recoverable up to ambiguity and not at all where
//!   two sessions share a prefix.
//! - **`partial_final_valid`.** Their ceil convention gives the trailing block an
//!   identifier and records nothing about how full it is. **Unrecoverable on import**,
//!   so a round trip through this format is lossy and must not be used as a determinism
//!   check.
//! - **Block geometry.** The format carries **no block-size field** — upstream's 512 is
//!   implicit. A file written from a description with a different block size is
//!   therefore only interpretable by a consumer told what it is, which is why the
//!   converter reports the block size it used instead of leaving it to be inferred.
//! - Think time, TTFT and service time, none of which this generator models anyway.
//!
//! # Examples
//!
//! [`convert_jsonl`] takes emitted trace rows and writes Mooncake lines. Note what
//! the renumbering does with the shared prefix — it is the whole point of the format
//! being global rather than per session:
//!
//! ```
//! use workload_trace::mooncake::convert_jsonl;
//!
//! // Two turns of one session: the second re-reads the first's prompt.
//! let trace = concat!(
//!     r#"{"request_start":0.0,"full_input_blocks":[91,92],"full_output_blocks":[93]}"#, "\n",
//!     r#"{"request_start":1.5,"full_input_blocks":[91,92,93],"full_output_blocks":[94]}"#, "\n",
//! );
//!
//! let mut out = Vec::new();
//! let stats = convert_jsonl(trace.as_bytes(), &mut out, 16).unwrap();
//! assert_eq!(stats.records, 2);
//! assert_eq!(stats.distinct_ids, 3); // 91, 92, 93 renumbered to 0, 1, 2
//!
//! let text = String::from_utf8(out).unwrap();
//! let lines: Vec<&str> = text.lines().collect();
//! assert_eq!(lines[0], r#"{"timestamp":0,"input_length":32,"output_length":16,"hash_ids":[0,1]}"#);
//! // The prefix keeps its identifiers, so the reuse is still visible.
//! assert_eq!(lines[1], r#"{"timestamp":1500,"input_length":48,"output_length":16,"hash_ids":[0,1,2]}"#);
//!
//! // And the conversion says what it gave up, rather than leaving it to be found.
//! assert!(stats.declared_losses().iter().any(|l| l.contains("16 tokens")));
//! ```

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

use crate::record::InvocationRecord;

/// One line of a Mooncake trace.
///
/// Field order is upstream's, so a reader that has seen their files sees ours the same
/// way — and so two runs of ours are byte-comparable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MooncakeRecord {
    /// Milliseconds on the run-global clock, non-decreasing.
    pub timestamp: i64,
    /// Prompt length in tokens.
    pub input_length: i64,
    /// Generated length in tokens.
    pub output_length: i64,
    /// The prompt's blocks, as dense identifiers global to the file.
    pub hash_ids: Vec<i64>,
}

/// The fields the projection needs from a trace row.
#[derive(Debug, Clone, Deserialize)]
struct Row {
    request_start: f64,
    full_input_blocks: Vec<u64>,
    full_output_blocks: Vec<u64>,
}

/// What a conversion produced, and what it had to give up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MooncakeStats {
    /// Rows written.
    pub records: u64,
    /// Distinct identifiers issued, which is also the next identifier.
    pub distinct_ids: u64,
    /// Total identifier references across all rows.
    pub references: u64,
    /// Tokens per block, which the format cannot carry and a consumer must be told.
    pub block_size: u64,
}

impl MooncakeStats {
    /// The losses this conversion incurred, for a report to print (FR-077).
    pub fn declared_losses(&self) -> Vec<String> {
        vec![
            "session grouping: the format has no session_id, so sessions survive only \
             as the prefix structure of hash_ids"
                .to_string(),
            "partial_final_valid: upstream's ceil convention records nothing about how \
             full a trailing block is, so this is unrecoverable on import and a round \
             trip is lossy"
                .to_string(),
            format!(
                "block geometry: the format carries no block-size field, so a consumer \
                 must be told this file's is {} tokens",
                self.block_size
            ),
            "the generated run: output_length survives as a token count, but the output \
             keys themselves are not referenced, so a consumer cannot see them cached"
                .to_string(),
            "think time, TTFT and service time: not modelled by this generator and not \
             representable here"
                .to_string(),
        ]
    }
}

/// Writes the Mooncake projection.
#[derive(Debug)]
pub struct MooncakeWriter<W: Write> {
    sink: W,
    block_size: u64,
    /// Our chained key to their dense identifier, in first-appearance order.
    ///
    /// Built once for the whole writer and **never reset per session** — that is
    /// FR-078, and the reason is in the module docs.
    dense: HashMap<u64, i64>,
    next_id: i64,
    stats: MooncakeStats,
    last_timestamp: i64,
}

impl<W: Write> MooncakeWriter<W> {
    /// A writer for one Mooncake file.
    ///
    /// `block_size` is tokens per block, from the description's `blocks.tokens`.
    pub fn new(sink: W, block_size: u64) -> Self {
        Self {
            sink,
            block_size,
            dense: HashMap::new(),
            next_id: 0,
            stats: MooncakeStats {
                block_size,
                ..Default::default()
            },
            last_timestamp: i64::MIN,
        }
    }

    /// Project and write one invocation record.
    ///
    /// # Errors
    ///
    /// If the row's timestamp goes backwards — upstream's `timestamp` is
    /// non-decreasing, and a reader that sorts on it would silently reorder the
    /// workload — or if serialisation or the sink fails.
    pub fn write_record(&mut self, record: &InvocationRecord) -> io::Result<()> {
        self.write_parts(
            record.request_start,
            &record.full_input_blocks,
            &record.full_output_blocks,
        )
    }

    fn write_parts(&mut self, at: f64, input: &[u64], output: &[u64]) -> io::Result<()> {
        if input.is_empty() {
            // A request with no prompt is not a request. Upstream has no such rows and
            // a reader would divide by nothing to recover the block count.
            return Ok(());
        }
        let timestamp = (at * 1000.0).round() as i64;
        if timestamp < self.last_timestamp {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "timestamp {timestamp} ms follows {}: upstream's timestamp is \
                     non-decreasing, and a reader sorting on it would silently reorder \
                     the workload",
                    self.last_timestamp
                ),
            ));
        }
        self.last_timestamp = timestamp;

        let mut hash_ids = Vec::with_capacity(input.len());
        for key in input {
            let id = match self.dense.get(key) {
                Some(id) => *id,
                None => {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.dense.insert(*key, id);
                    id
                }
            };
            hash_ids.push(id);
        }

        self.stats.records += 1;
        self.stats.references += hash_ids.len() as u64;
        self.stats.distinct_ids = self.next_id as u64;

        let out = MooncakeRecord {
            timestamp,
            input_length: input.len() as i64 * self.block_size as i64,
            output_length: output.len() as i64 * self.block_size as i64,
            hash_ids,
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
    pub fn finish(mut self) -> io::Result<MooncakeStats> {
        self.sink.flush()?;
        Ok(self.stats)
    }
}

/// Convert an emitted JSONL trace into the Mooncake format.
///
/// The `convert --to mooncake` entry point. Its input is the *schema*, so it works on
/// any trace in it, which is what lets a real corpus trace and a generated one be
/// pushed through the identical projection (FR-075a).
///
/// # Errors
///
/// If a line is not a trace row, or a timestamp goes backwards.
pub fn convert_jsonl<R: BufRead, W: Write>(
    input: R,
    output: W,
    block_size: u64,
) -> io::Result<MooncakeStats> {
    let mut writer = MooncakeWriter::new(output, block_size);
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
            row.request_start,
            &row.full_input_blocks,
            &row.full_output_blocks,
        )?;
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(at: f64, input: &[u64], output: &[u64]) -> String {
        format!(
            r#"{{"request_start":{at},"full_input_blocks":{input:?},"full_output_blocks":{output:?}}}"#
        )
    }

    fn lines_of(bytes: Vec<u8>) -> Vec<serde_json::Value> {
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn the_upstream_invariant_holds_on_every_row() {
        // `len(hash_ids) == ceil(input_length / block_size)`, measured on 2328/2328
        // upstream rows. Ours divides exactly because blocks are whole.
        let input = format!(
            "{}\n{}\n",
            row(0.0, &[10, 11, 12], &[20]),
            row(1.5, &[10, 11, 12, 20, 13], &[21, 22]),
        );
        let mut out = Vec::new();
        let stats = convert_jsonl(input.as_bytes(), &mut out, 16).unwrap();
        assert_eq!(stats.records, 2);
        for v in lines_of(out) {
            let n = v["hash_ids"].as_array().unwrap().len() as i64;
            let len = v["input_length"].as_i64().unwrap();
            assert_eq!(len, n * 16, "input_length is not blocks * block_size");
            assert_eq!(n, (len as f64 / 16.0).ceil() as i64);
        }
    }

    #[test]
    fn identifiers_are_dense_and_global_to_the_file() {
        // Upstream: range 0..44683 with 44684 distinct — perfectly dense. A gap or a
        // per-session restart would both break a consumer that treats an identifier as
        // an index.
        let input = format!(
            "{}\n{}\n{}\n",
            row(0.0, &[100], &[]),
            row(1.0, &[200, 201], &[]),
            row(2.0, &[100, 300], &[]),
        );
        let mut out = Vec::new();
        let stats = convert_jsonl(input.as_bytes(), &mut out, 16).unwrap();
        let all: Vec<i64> = lines_of(out)
            .iter()
            .flat_map(|v| {
                v["hash_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_i64().unwrap())
                    .collect::<Vec<_>>()
            })
            .collect();
        let distinct: std::collections::BTreeSet<&i64> = all.iter().collect();
        assert_eq!(
            distinct.len() as u64,
            stats.distinct_ids,
            "the writer's count disagrees with the file"
        );
        assert_eq!(*distinct.iter().next().unwrap(), &0, "not zero-based");
        assert_eq!(
            **distinct.iter().next_back().unwrap(),
            distinct.len() as i64 - 1,
            "identifiers are not dense: {distinct:?}"
        );
    }

    #[test]
    fn a_repeated_key_keeps_its_identifier_across_rows() {
        // The whole of reuse. If a key were renumbered on each appearance the file
        // would show no cache hits at all while looking entirely well formed.
        let input = format!(
            "{}\n{}\n",
            row(0.0, &[7, 8], &[]),
            row(1.0, &[7, 8, 9], &[])
        );
        let mut out = Vec::new();
        convert_jsonl(input.as_bytes(), &mut out, 16).unwrap();
        let rows = lines_of(out);
        let first = rows[0]["hash_ids"].as_array().unwrap();
        let second = rows[1]["hash_ids"].as_array().unwrap();
        assert_eq!(&second[..2], &first[..], "the shared prefix was renumbered");
    }

    #[test]
    fn timestamps_are_milliseconds_and_non_decreasing() {
        let input = format!("{}\n{}\n", row(0.25, &[1], &[]), row(2.5, &[1, 2], &[]));
        let mut out = Vec::new();
        convert_jsonl(input.as_bytes(), &mut out, 16).unwrap();
        let rows = lines_of(out);
        assert_eq!(rows[0]["timestamp"], 250);
        assert_eq!(rows[1]["timestamp"], 2500);
    }

    #[test]
    fn a_backwards_timestamp_is_refused() {
        // Upstream's timestamp is non-decreasing; a reader that sorts on it would
        // silently reorder the workload rather than fail.
        let input = format!("{}\n{}\n", row(5.0, &[1], &[]), row(1.0, &[2], &[]));
        let mut out = Vec::new();
        let err = convert_jsonl(input.as_bytes(), &mut out, 16).unwrap_err();
        assert!(err.to_string().contains("non-decreasing"), "got: {err}");
    }

    #[test]
    fn output_length_is_recorded_but_output_blocks_are_not_listed() {
        // Upstream's shape: `hash_ids` is the prompt. A turn's output enters the next
        // request's prefix, so nothing is lost except the final turn's — which is
        // stored and never read again.
        let input = row(0.0, &[1, 2], &[3, 4, 5]);
        let mut out = Vec::new();
        convert_jsonl(input.as_bytes(), &mut out, 16).unwrap();
        let v = &lines_of(out)[0];
        assert_eq!(v["hash_ids"].as_array().unwrap().len(), 2);
        assert_eq!(v["output_length"], 3 * 16);
    }

    #[test]
    fn the_field_order_matches_upstream() {
        // A reader that has seen their files should see ours the same way, and two runs
        // of ours must be byte-comparable.
        let mut out = Vec::new();
        convert_jsonl(row(0.0, &[1], &[2]).as_bytes(), &mut out, 16).unwrap();
        let text = String::from_utf8(out).unwrap();
        let expected = r#"{"timestamp":0,"input_length":16,"output_length":16,"hash_ids":[0]}"#;
        assert_eq!(text.trim(), expected);
    }

    #[test]
    fn the_declared_losses_name_the_block_size() {
        // The format carries no block-size field, so a file is only interpretable by a
        // consumer told what it is.
        let mut out = Vec::new();
        let stats = convert_jsonl(row(0.0, &[1], &[]).as_bytes(), &mut out, 64).unwrap();
        let losses = stats.declared_losses();
        assert!(losses.iter().any(|l| l.contains("64 tokens")), "{losses:?}");
        assert!(losses.iter().any(|l| l.contains("session grouping")));
        assert!(losses.iter().any(|l| l.contains("partial_final_valid")));
    }
}
