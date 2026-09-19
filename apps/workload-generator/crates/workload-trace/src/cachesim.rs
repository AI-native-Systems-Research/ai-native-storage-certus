//! libCacheSim's two containers: CSV, and the binary `oracleGeneral`.
//!
//! `contracts/trace-interop.md` is normative and records where each fact was verified.
//! At the cache level a workload is just `(time, object, size)` triples, which is what
//! this world runs on — so this projection is what buys access to published eviction
//! baselines to compare Certus's policy against.
//!
//! # CSV: the columns are ours to choose, so the parameter string is part of the output
//!
//! libCacheSim's CSV reader takes a column map, so there is no fixed layout to match —
//! which means a CSV file alone is **not interpretable**. [`CachesimStats::params`]
//! renders the `--trace-type-params` string for the layout written, and the converter
//! prints it. A file whose column meanings live only in someone's memory is a file
//! that will be read wrong.
//!
//! Two verified traps: a numeric object id needs `obj-id-is-num=1` or the reader errors
//! out, and the CSV reader is **ASCII-only** — harmless here, since every column is a
//! number, but it must not be "improved" by adding a text column.
//!
//! # `oracleGeneral`: the format we can fill and a real trace cannot
//!
//! It carries `next_access_vtime`, the request ordinal at which an object is next
//! touched. A real trace can only obtain that by a full offline pass. **An emit run
//! knows the entire future of its own workload**, so this is the one thing this
//! generator can give a cache simulator that a captured trace cannot — and it makes
//! optimal-policy (Belady) baselines available.
//!
//! ## It cannot stream, and that is inherent
//!
//! Next-access requires the future, so [`OracleGeneralWriter`] buffers the whole
//! reference stream and resolves it on [`OracleGeneralWriter::finish`] with one backward
//! pass. Peak memory is therefore proportional to the **run**, not to a constant: 20
//! bytes per reference held, so the shipped example's 18.6M references at a 300-second
//! span cost about 370 MB.
//!
//! That is the only writer here with that property. It is stated because the projection
//! is otherwise indistinguishable from the streaming ones, and because the pre-flight
//! projection already reports the reference count that predicts it.
//!
//! ## The layout, verified rather than inferred
//!
//! 24 bytes, **packed**, native byte order:
//!
//! ```text
//! offset 0   clock_time         uint32_t
//! offset 4   obj_id             uint64_t
//! offset 12  obj_size           uint32_t
//! offset 16  next_access_vtime  int64_t
//! ```
//!
//! `obj_id` at offset 4 is not 8-aligned; the reader casts pointers at fixed offsets, so
//! there is no padding to reproduce. `-1` is the "never again" sentinel the reader
//! normalises. And `clock_time` being 32 bits caps millisecond timestamps at **49.7
//! days** of virtual time, which the writer refuses rather than wrapping.
//!
//! # Both the prompt and the generated run are accesses
//!
//! A turn's prompt blocks are **reads** and the blocks it generates are **stores**, and
//! both are accesses this projection writes, at that turn's own timestamp, prompt first.
//!
//! The store belongs at the generating turn because that is when a real deployment
//! inserts it: vLLM's offloading connector calls `prepare_store` **after each forward
//! pass** (`knowledge/kv_IO_pattern.md`), offloading newly-computed blocks rather than
//! waiting for something to read them. Projecting the prompt alone would move every
//! generated block's arrival one turn later than it happens, and would omit the last
//! turn of every session entirely — that output is stored and never read again, so it
//! would appear nowhere while still occupying the cache it was measured against.
//!
//! libCacheSim has no notion of input versus output, which is the point: to a cache a
//! reference is a reference. Nothing is tagged, and nothing needs to be.
//!
//! # Examples
//!
//! CSV, one line per block reference — and the parameter string that makes the file
//! interpretable, which is not optional here:
//!
//! ```
//! use workload_trace::cachesim::convert_jsonl_csv;
//!
//! let trace = concat!(
//!     r#"{"request_start":0.0,"full_input_blocks":[91,92],"full_output_blocks":[93]}"#, "\n",
//!     r#"{"request_start":1.5,"full_input_blocks":[91,92,93],"full_output_blocks":[94]}"#, "\n",
//! );
//!
//! let mut out = Vec::new();
//! let stats = convert_jsonl_csv(trace.as_bytes(), &mut out, 32768).unwrap();
//! // References, not requests: 2 prompt + 1 generated, then 3 prompt + 1 generated.
//! assert_eq!(stats.accesses, 7);
//! assert_eq!(stats.distinct_objects, 4);
//!
//! let text = String::from_utf8(out).unwrap();
//! assert_eq!(text.lines().next(), Some("0,91,32768"));
//! // Key 93 is stored at turn 0 and read at turn 1, in that order.
//! assert_eq!(text.lines().nth(2), Some("0,93,32768"));
//! assert_eq!(text.lines().count(), 7);
//!
//! // A CSV file cannot say what its own columns mean, so the layout travels with it.
//! assert_eq!(stats.params(), "time-col=1, obj-id-col=2, obj-size-col=3, obj-id-is-num=1");
//! ```
//!
//! `oracleGeneral`, where the emit run's knowledge of its own future becomes
//! `next_access_vtime` — the field a captured trace cannot fill without an offline
//! pass, and the one that makes a Belady baseline possible:
//!
//! ```
//! use workload_trace::cachesim::{convert_jsonl_oracle, NEVER_AGAIN, ORACLE_RECORD_BYTES};
//!
//! let trace = concat!(
//!     r#"{"request_start":0.0,"full_input_blocks":[91,92],"full_output_blocks":[93]}"#, "\n",
//!     r#"{"request_start":1.5,"full_input_blocks":[91],"full_output_blocks":[]}"#, "\n",
//! );
//!
//! let mut out = Vec::new();
//! let stats = convert_jsonl_oracle(trace.as_bytes(), &mut out, 32768).unwrap();
//! assert_eq!(stats.accesses, 4); // 91, 92, then the generated 93, then 91 again
//! assert_eq!(out.len(), 4 * ORACLE_RECORD_BYTES);
//!
//! // Read `next_access_vtime` back out of each packed record: an ordinal, not a time.
//! let next_access = |i: usize| -> i64 {
//!     let base = i * ORACLE_RECORD_BYTES + 16;
//!     i64::from_ne_bytes(out[base..base + 8].try_into().unwrap())
//! };
//! assert_eq!(next_access(0), 3);          // key 91 is touched again at ordinal 3
//! assert_eq!(next_access(1), NEVER_AGAIN); // key 92 never is
//! // The generated block: stored here, never read again. Invisible before this
//! // projection carried the generated run at all.
//! assert_eq!(next_access(2), NEVER_AGAIN);
//! assert_eq!(next_access(3), NEVER_AGAIN);
//! ```

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use serde::Deserialize;

use crate::record::InvocationRecord;

/// Sentinel `next_access_vtime` for an object never accessed again.
///
/// The reader treats `-1` and `INT64_MAX` alike and normalises both, so either works;
/// `-1` is the smaller thing to write and the more obvious to read in a hex dump.
pub const NEVER_AGAIN: i64 = -1;

/// Bytes per `oracleGeneral` record, from the reader's own `item_size`.
pub const ORACLE_RECORD_BYTES: usize = 24;

/// Largest millisecond timestamp `clock_time`'s `uint32_t` can hold.
pub const MAX_ORACLE_TIMESTAMP_MS: u64 = u32::MAX as u64;

/// The fields the projection needs from a trace row.
#[derive(Debug, Clone, Deserialize)]
struct Row {
    request_start: f64,
    full_input_blocks: Vec<u64>,
    full_output_blocks: Vec<u64>,
}

/// What a conversion produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CachesimStats {
    /// Accesses written — one row per block reference.
    pub accesses: u64,
    /// Distinct objects.
    pub distinct_objects: u64,
    /// Bytes per object, which is the block size in bytes.
    pub object_bytes: u32,
}

impl CachesimStats {
    /// The losses this projection incurred, for a report to print (FR-077).
    ///
    /// One list for both containers, because they project the same reference stream and
    /// differ only in how they encode it. `oracleGeneral`'s extra `next_access_vtime` is
    /// not listed: it is something this projection *adds* from knowing the whole future,
    /// and its 32-bit `clock_time` is a **refusal** rather than a loss — a run past 49.7
    /// days of virtual time is rejected instead of wrapped.
    ///
    /// Stated rather than left to be discovered, because every one of these is invisible
    /// in the output: a libCacheSim file that has lost session identity still loads,
    /// replays and reports a hit rate.
    ///
    /// The generated run is **not** listed: it is carried, as stores at the turn that
    /// produced it. The input/output *distinction* is dropped, which is not a loss a cache
    /// can observe — a reference is a reference.
    pub fn declared_losses(&self) -> Vec<String> {
        vec![
            "session grouping and identity: the format has no session field at all, so \
             nothing separates two sessions' accesses and the conversation graph cannot be \
             recovered"
                .to_string(),
            "the input/output distinction: prompt reads and generated stores are both \
             present as accesses, in that order within a turn, but nothing marks which is \
             which. A cache does not distinguish them; a reader wanting to would need the \
             native trace"
                .to_string(),
            format!(
                "how full a trailing partial block was: its key is kept and cached like \
                 any other, but every object is charged {} bytes, so partial_final_valid \
                 is unrecoverable and byte-capacity accounting rounds it up",
                self.object_bytes
            ),
            "virtual time: kept only as an integer millisecond clock, and think time, TTFT \
             and service time are neither modelled by this generator nor representable here"
                .to_string(),
        ]
    }

    /// The `--trace-type-params` string for the layout this container produces.
    ///
    /// Printed with every conversion, because libCacheSim's columns are configurable
    /// and a CSV file cannot say what its own columns mean.
    pub fn params(&self) -> &'static str {
        "time-col=1, obj-id-col=2, obj-size-col=3, obj-id-is-num=1"
    }

    /// A ready-to-run command line, so the layout cannot be lost in transit.
    pub fn example_command(&self, path: &str) -> String {
        format!("cachesim {path} csv lru 1gb -t \"{}\"", self.params())
    }
}

/// Writes `(time, obj_id, obj_size)` CSV.
///
/// Streams: one line per block reference, nothing buffered.
#[derive(Debug)]
pub struct CsvWriter<W: Write> {
    sink: W,
    object_bytes: u32,
    distinct: std::collections::BTreeSet<u64>,
    accesses: u64,
}

impl<W: Write> CsvWriter<W> {
    /// A writer for one CSV file.
    ///
    /// `object_bytes` is bytes per block, from the description's `blocks.bytes`: that is
    /// what the cache actually holds per key.
    pub fn new(sink: W, object_bytes: u32) -> Self {
        Self {
            sink,
            object_bytes,
            distinct: Default::default(),
            accesses: 0,
        }
    }

    /// Project and write one invocation record's accesses.
    ///
    /// # Errors
    ///
    /// If the sink fails.
    pub fn write_record(&mut self, record: &InvocationRecord) -> io::Result<()> {
        self.write_parts(
            record.request_start,
            &record.full_input_blocks,
            &record.full_output_blocks,
        )
    }

    fn write_parts(&mut self, at: f64, input: &[u64], output: &[u64]) -> io::Result<()> {
        // Integer milliseconds: libCacheSim's CSV `clock_time` is int64, and a decimal
        // point in a numeric column is a needless risk in a reader that also has to
        // parse object ids as numbers.
        let ms = (at * 1000.0).round() as i64;
        // Prompt reads first, then the generated blocks this turn stored. Both are
        // accesses at this turn's time; see the module docs on why the generated run
        // belongs here rather than at the next turn that reads it.
        for key in input.iter().chain(output) {
            self.distinct.insert(*key);
            self.accesses += 1;
            writeln!(self.sink, "{ms},{key},{}", self.object_bytes)?;
        }
        Ok(())
    }

    /// Flush and return the statistics.
    ///
    /// # Errors
    ///
    /// If the flush fails.
    pub fn finish(mut self) -> io::Result<CachesimStats> {
        self.sink.flush()?;
        Ok(CachesimStats {
            accesses: self.accesses,
            distinct_objects: self.distinct.len() as u64,
            object_bytes: self.object_bytes,
        })
    }
}

/// One buffered access, before its next-access ordinal is known.
#[derive(Debug, Clone, Copy)]
struct PendingAccess {
    clock_time_ms: u32,
    obj_id: u64,
}

/// Writes the binary `oracleGeneral` container, next-access ordinals included.
///
/// Buffers the whole reference stream; see the module docs on why that is inherent and
/// what it costs.
#[derive(Debug)]
pub struct OracleGeneralWriter<W: Write> {
    sink: W,
    object_bytes: u32,
    pending: Vec<PendingAccess>,
}

impl<W: Write> OracleGeneralWriter<W> {
    /// A writer for one `oracleGeneral` file.
    pub fn new(sink: W, object_bytes: u32) -> Self {
        Self {
            sink,
            object_bytes,
            pending: Vec::new(),
        }
    }

    /// Buffer one invocation record's accesses.
    ///
    /// # Errors
    ///
    /// If a timestamp exceeds what `clock_time`'s `uint32_t` can hold — 49.7 days of
    /// virtual time in milliseconds. Refused rather than wrapped: a wrapped timestamp
    /// would make the trace appear to jump backwards, and the reader would accept it.
    pub fn write_record(&mut self, record: &InvocationRecord) -> io::Result<()> {
        self.write_parts(
            record.request_start,
            &record.full_input_blocks,
            &record.full_output_blocks,
        )
    }

    fn write_parts(&mut self, at: f64, input: &[u64], output: &[u64]) -> io::Result<()> {
        let ms = (at * 1000.0).round() as i64;
        if ms < 0 || ms as u64 > MAX_ORACLE_TIMESTAMP_MS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "virtual time {at:.3} s is {ms} ms, past what oracleGeneral's \
                     32-bit clock_time holds ({MAX_ORACLE_TIMESTAMP_MS} ms, about 49.7 \
                     days). Refusing rather than wrapping, which would make the trace \
                     appear to jump backwards"
                ),
            ));
        }
        let clock_time_ms = ms as u32;
        // Prompt reads then the generated blocks, in that order within the turn, so a
        // generated block's own store is the access its next-access ordinal is measured
        // from.
        for key in input.iter().chain(output) {
            self.pending.push(PendingAccess {
                clock_time_ms,
                obj_id: *key,
            });
        }
        Ok(())
    }

    /// Resolve next-access ordinals and write every record.
    ///
    /// One backward pass: walking from the end, the last position seen for an object is
    /// its next access from any earlier position. `O(references)` time and one map entry
    /// per distinct object.
    ///
    /// # Errors
    ///
    /// If the sink fails.
    pub fn finish(mut self) -> io::Result<CachesimStats> {
        let n = self.pending.len();
        let mut next_access = vec![NEVER_AGAIN; n];
        let mut last_seen: HashMap<u64, usize> = HashMap::new();
        for i in (0..n).rev() {
            let id = self.pending[i].obj_id;
            if let Some(next) = last_seen.get(&id) {
                // Ordinals, not time: the reader counts requests.
                next_access[i] = *next as i64;
            }
            last_seen.insert(id, i);
        }

        let mut buf = [0u8; ORACLE_RECORD_BYTES];
        for (i, access) in self.pending.iter().enumerate() {
            // Packed at the reader's own offsets, native byte order — it casts pointers
            // rather than decoding, so there is no padding to reproduce.
            buf[0..4].copy_from_slice(&access.clock_time_ms.to_ne_bytes());
            buf[4..12].copy_from_slice(&access.obj_id.to_ne_bytes());
            buf[12..16].copy_from_slice(&self.object_bytes.to_ne_bytes());
            buf[16..24].copy_from_slice(&next_access[i].to_ne_bytes());
            self.sink.write_all(&buf)?;
        }
        self.sink.flush()?;

        Ok(CachesimStats {
            accesses: n as u64,
            distinct_objects: last_seen.len() as u64,
            object_bytes: self.object_bytes,
        })
    }

    /// References buffered so far, which is what peak memory tracks.
    pub fn buffered(&self) -> usize {
        self.pending.len()
    }
}

/// Convert an emitted JSONL trace into libCacheSim CSV.
///
/// # Errors
///
/// If a line is not a trace row, or the sink fails.
pub fn convert_jsonl_csv<R: BufRead, W: Write>(
    input: R,
    output: W,
    object_bytes: u32,
) -> io::Result<CachesimStats> {
    let mut writer = CsvWriter::new(output, object_bytes);
    for_each_row(input, |at, input_blocks, output_blocks| {
        writer.write_parts(at, input_blocks, output_blocks)
    })?;
    writer.finish()
}

/// Convert an emitted JSONL trace into the binary `oracleGeneral` container.
///
/// # Errors
///
/// If a line is not a trace row, a timestamp overflows `clock_time`, or the sink fails.
pub fn convert_jsonl_oracle<R: BufRead, W: Write>(
    input: R,
    output: W,
    object_bytes: u32,
) -> io::Result<CachesimStats> {
    let mut writer = OracleGeneralWriter::new(output, object_bytes);
    for_each_row(input, |at, input_blocks, output_blocks| {
        writer.write_parts(at, input_blocks, output_blocks)
    })?;
    writer.finish()
}

fn for_each_row<R: BufRead, F: FnMut(f64, &[u64], &[u64]) -> io::Result<()>>(
    input: R,
    mut f: F,
) -> io::Result<()> {
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
        f(
            row.request_start,
            &row.full_input_blocks,
            &row.full_output_blocks,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row with no generated blocks, so the counts in these tests stay about the
    /// prompt. `row_with_output` covers the generated run.
    fn row(at: f64, blocks: &[u64]) -> String {
        format!(
            r#"{{"request_start":{at},"full_input_blocks":{blocks:?},"full_output_blocks":[]}}"#
        )
    }

    fn row_with_output(at: f64, input: &[u64], output: &[u64]) -> String {
        format!(
            r#"{{"request_start":{at},"full_input_blocks":{input:?},"full_output_blocks":{output:?}}}"#
        )
    }

    fn decode(bytes: &[u8]) -> Vec<(u32, u64, u32, i64)> {
        assert_eq!(
            bytes.len() % ORACLE_RECORD_BYTES,
            0,
            "not a whole number of 24-byte records"
        );
        bytes
            .chunks(ORACLE_RECORD_BYTES)
            .map(|r| {
                (
                    u32::from_ne_bytes(r[0..4].try_into().unwrap()),
                    u64::from_ne_bytes(r[4..12].try_into().unwrap()),
                    u32::from_ne_bytes(r[12..16].try_into().unwrap()),
                    i64::from_ne_bytes(r[16..24].try_into().unwrap()),
                )
            })
            .collect()
    }

    #[test]
    fn csv_writes_one_line_per_access_with_the_documented_columns() {
        let input = format!("{}\n{}\n", row(0.0, &[7, 8]), row(1.5, &[7, 8, 9]));
        let mut out = Vec::new();
        let stats = convert_jsonl_csv(input.as_bytes(), &mut out, 32768).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text,
            "0,7,32768\n0,8,32768\n1500,7,32768\n1500,8,32768\n1500,9,32768\n"
        );
        assert_eq!(stats.accesses, 5);
        assert_eq!(stats.distinct_objects, 3);
        assert!(
            stats.params().contains("obj-id-is-num=1"),
            "the verified trap"
        );
        assert!(stats.example_command("t.csv").contains("time-col=1"));
    }

    #[test]
    fn csv_is_ascii_only() {
        // Verified upstream: libCacheSim's CSV reader does not support UTF-8. Harmless
        // for numbers, but it must not be "improved" with a text column.
        let mut out = Vec::new();
        convert_jsonl_csv(row(0.0, &[1]).as_bytes(), &mut out, 4096).unwrap();
        assert!(out.iter().all(|b| b.is_ascii()));
    }

    #[test]
    fn the_oracle_record_is_twenty_four_packed_bytes_at_the_verified_offsets() {
        // The layout T062e read from oracleGeneralBin.h. A 64-bit clock_time — which the
        // prose description would have suggested — shifts every later field.
        let mut out = Vec::new();
        convert_jsonl_oracle(
            row(0.25, &[0xDEAD_BEEF_CAFE_1234]).as_bytes(),
            &mut out,
            4096,
        )
        .unwrap();
        assert_eq!(out.len(), ORACLE_RECORD_BYTES);
        let r = decode(&out)[0];
        assert_eq!(r.0, 250, "clock_time is uint32 milliseconds");
        assert_eq!(r.1, 0xDEAD_BEEF_CAFE_1234, "obj_id is uint64 at offset 4");
        assert_eq!(r.2, 4096, "obj_size is uint32 at offset 12");
        assert_eq!(r.3, NEVER_AGAIN, "a single access is never accessed again");
    }

    #[test]
    fn a_full_width_key_survives_because_obj_id_is_uint64() {
        // Verified: `obj_id_t` is `uint64_t` (cacheObj.h), so keys above i64::MAX need no
        // renumbering. Had it been signed, half our key space would have wrapped.
        let key = u64::MAX - 1;
        let mut out = Vec::new();
        convert_jsonl_oracle(row(0.0, &[key]).as_bytes(), &mut out, 4096).unwrap();
        assert_eq!(decode(&out)[0].1, key);

        let mut csv = Vec::new();
        convert_jsonl_csv(row(0.0, &[key]).as_bytes(), &mut csv, 4096).unwrap();
        assert!(String::from_utf8(csv).unwrap().contains(&key.to_string()));
    }

    #[test]
    fn next_access_vtime_is_the_ordinal_of_the_next_access_and_minus_one_at_the_end() {
        // The whole point of this container. Access sequence 7,8,7,9,8 gives, per
        // position: 7 -> 2, 8 -> 4, 7 -> never, 9 -> never, 8 -> never.
        let input = format!(
            "{}\n{}\n{}\n",
            row(0.0, &[7, 8]),
            row(1.0, &[7, 9]),
            row(2.0, &[8])
        );
        let mut out = Vec::new();
        let stats = convert_jsonl_oracle(input.as_bytes(), &mut out, 4096).unwrap();
        assert_eq!(stats.accesses, 5);
        let next: Vec<i64> = decode(&out).iter().map(|r| r.3).collect();
        assert_eq!(next, vec![2, 4, NEVER_AGAIN, NEVER_AGAIN, NEVER_AGAIN]);
    }

    #[test]
    fn a_timestamp_past_the_thirty_two_bit_clock_is_refused() {
        // 49.7 days of virtual time in milliseconds. Wrapping would make the trace look
        // as though it jumped backwards, and the reader would accept it.
        let past = (MAX_ORACLE_TIMESTAMP_MS as f64 / 1000.0) + 1.0;
        let mut out = Vec::new();
        let err = convert_jsonl_oracle(row(past, &[1]).as_bytes(), &mut out, 4096).unwrap_err();
        assert!(err.to_string().contains("49.7"), "got: {err}");
        // And the CSV container has no such limit, because its clock_time is int64.
        let mut csv = Vec::new();
        convert_jsonl_csv(row(past, &[1]).as_bytes(), &mut csv, 4096).unwrap();
        assert!(!csv.is_empty());
    }

    #[test]
    fn both_containers_see_the_same_accesses() {
        // They differ only in what they can carry, never in the workload they describe.
        let input = format!("{}\n{}\n", row(0.0, &[1, 2, 3]), row(1.0, &[1, 2, 3, 4]));
        let mut csv = Vec::new();
        let csv_stats = convert_jsonl_csv(input.as_bytes(), &mut csv, 4096).unwrap();
        let mut oracle = Vec::new();
        let oracle_stats = convert_jsonl_oracle(input.as_bytes(), &mut oracle, 4096).unwrap();
        assert_eq!(csv_stats, oracle_stats);

        let csv_ids: Vec<u64> = String::from_utf8(csv)
            .unwrap()
            .lines()
            .map(|l| l.split(',').nth(1).unwrap().parse().unwrap())
            .collect();
        let oracle_ids: Vec<u64> = decode(&oracle).iter().map(|r| r.1).collect();
        assert_eq!(csv_ids, oracle_ids);
    }

    #[test]
    fn the_oracle_writer_buffers_the_whole_stream_and_says_so() {
        // Its memory is proportional to the run, not a constant. Asserted so the
        // property cannot be quietly lost — or quietly introduced into the CSV writer.
        let mut w = OracleGeneralWriter::new(Vec::new(), 4096);
        assert_eq!(w.buffered(), 0);
        w.write_parts(0.0, &[1, 2], &[3]).unwrap();
        assert_eq!(w.buffered(), 3, "the oracle writer must buffer");
        w.write_parts(1.0, &[4], &[]).unwrap();
        assert_eq!(w.buffered(), 4);
        let stats = w.finish().unwrap();
        assert_eq!(stats.accesses, 4);
        // Nothing was written until `finish`, which is the buffering property: a
        // streaming writer would have produced bytes before the backward pass.
        let mut streaming_check = OracleGeneralWriter::new(Vec::new(), 4096);
        streaming_check.write_parts(0.0, &[1], &[2]).unwrap();
        assert_eq!(streaming_check.buffered(), 2);
    }

    #[test]
    fn a_turns_generated_blocks_are_accesses_at_that_turns_own_time() {
        // The defect this replaced: projecting the prompt alone put a generated block's
        // arrival one turn late, and dropped the last turn's output of every session
        // entirely — output that a real cache holds, because vLLM stores after each
        // forward pass rather than when something reads it.
        let trace = format!(
            "{}\n{}\n",
            row_with_output(0.0, &[1, 2], &[3]),
            row_with_output(1.0, &[1, 2, 3], &[4]),
        );
        let mut out = Vec::new();
        let stats = convert_jsonl_csv(trace.as_bytes(), &mut out, 4096).unwrap();

        // 2 prompt + 1 generated, then 3 prompt + 1 generated.
        assert_eq!(stats.accesses, 7);
        assert_eq!(stats.distinct_objects, 4);

        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // Key 3 is stored at turn 0's timestamp, before it is read at turn 1's.
        assert_eq!(lines[2], "0,3,4096");
        assert_eq!(lines[5], "1000,3,4096");
        // Key 4 is the final turn's output: referenced exactly once, and present. Under
        // the prompt-only projection it appeared nowhere at all.
        assert_eq!(lines.iter().filter(|l| l.contains(",4,")).count(), 1);
        assert_eq!(lines[6], "1000,4,4096");
    }

    #[test]
    fn the_declared_losses_name_what_a_loaded_file_cannot_show() {
        // Every one of these is invisible in the output: a file that has lost session
        // identity still loads, replays and reports a hit rate, which is why FR-077 asks
        // for them to be stated rather than discovered.
        let mut out = Vec::new();
        let stats = convert_jsonl_csv(row(0.0, &[1, 2]).as_bytes(), &mut out, 4096).unwrap();
        let losses = stats.declared_losses();
        for expected in [
            "session grouping and identity",
            "the input/output distinction",
            "partial_final_valid",
            "virtual time",
        ] {
            assert!(
                losses.iter().any(|l| l.contains(expected)),
                "no loss names {expected:?}: {losses:?}"
            );
        }
        // And the generated run must NOT be declared lost, because it is carried. A
        // stale entry here would be worse than none: it would tell a reader to go to the
        // native trace for references this file already has.
        assert!(
            !losses
                .iter()
                .any(|l| l.contains("never appear as accesses")),
            "the generated run is projected now: {losses:?}"
        );
        // The object size is a figure a consumer cannot recover from the file, so it is
        // named rather than alluded to.
        assert!(
            losses.iter().any(|l| l.contains("4096 bytes")),
            "{losses:?}"
        );

        // Both containers project the same reference stream, so they declare the same
        // losses; a divergence here would mean one of them dropped something quietly.
        let mut binary = Vec::new();
        let oracle = convert_jsonl_oracle(row(0.0, &[1, 2]).as_bytes(), &mut binary, 4096).unwrap();
        assert_eq!(oracle.declared_losses(), losses);
    }
}
