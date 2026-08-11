//! The interchange trace: one schema, two containers, one definition.
//!
//! `contracts/trace-io.md` is both the format `certus-trace fit` reads from real
//! traces and the format `certus-workload emit` writes, which is what makes a
//! generated workload substitutable for a real one as input to any third-party
//! tool. The record types therefore live **here** rather than in either binary:
//! an emitter and a reader with separate definitions of the same record would
//! drift, and the FR-058a round trip — emit, read back, refit — would then be
//! comparing two formats rather than checking one.
//!
//! ## Why the full encoding
//!
//! The contract carries two mutually exclusive block encodings, and a generated
//! trace uses the **full** one (`source_class: pre_hashed`): the generator knows
//! every block id it minted, so expressing reuse as a delta against ancestors
//! would be a compression of information it has, not an honest statement of what
//! it knows. The full encoding **includes** the trailing partial block and gives
//! its valid token count in `partial_final_valid`, where the delta encoding
//! excludes it — the trap the contract records, and the reason both invariants
//! below are asserted in the tests rather than assumed:
//!
//! ```text
//! (input_length - partial_final_valid) % block_size == 0
//! len(full_input_blocks) == (input_length - partial_final_valid) / block_size + 1
//! ```
//!
//! ## What a generated trace does not claim
//!
//! `output_length` is 0 and `full_output_blocks` is empty, because the plan makes
//! **no input/output distinction**: a plan event is a block reference and nothing
//! more (`contracts/plan-format.md`). Declaring some fraction of a request's
//! blocks as "output" would be inventing an attribution the plan does not carry.
//! A reader recovers a session's growth per turn from successive
//! `full_input_blocks` lengths, which is the same information without the
//! invention. `field_status` says so per field, so a consumer learns it by
//! reading the manifest rather than by knowing which tool wrote the file.
//!
//! `reuse_from` is left empty deliberately: the contract is explicit that it is
//! intra-session compression only and that genuine sharing appears as two
//! sessions listing the same global block id. A reader treating it as the sharing
//! signal would conclude a generated trace has no cross-session sharing at all,
//! which would be wrong — and the same wrong conclusion it would draw about a
//! real trace.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::keys::CacheKey;
use crate::plan::record::{flags, PlanEvent};

/// Tokens per block when none is stated.
///
/// vLLM's own default. `block_size` counts **tokens**, not bytes, and the
/// generator's `block_bytes` is a payload size — the two are independent, which
/// is why this is a separate parameter rather than derived from one.
pub const DEFAULT_BLOCK_SIZE_TOKENS: u32 = 16;

/// One invocation: an LLM request, and the block list it references.
///
/// Field names and types are those of `contracts/trace-io.md` § The invocation
/// record, spelled exactly, because a third-party reader matches on them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invocation {
    /// Which trace this row belongs to.
    pub trace_id: String,
    /// Conversation or agent run. Nullable in the contract; never null here,
    /// since the generator always knows the session.
    pub session_id: Option<String>,
    /// 0-based position within the session.
    pub invocation_index: i64,
    /// Predecessor, or −1.
    pub parent_invocation: i64,
    /// Predecessors when there is more than one. Always empty: fan-in is a
    /// scheduling dependency rather than prefix reuse, and the generator does not
    /// emit it (spec Out of Scope).
    pub parent_invocations: Vec<i64>,
    /// Seconds from the trace's time origin.
    pub request_start: Option<f64>,
    /// Absent: a plan carries when a request is issued, not when it finished.
    /// Inventing an end would be inventing a latency.
    pub request_end: Option<f64>,
    /// `start` or `submission`.
    pub timestamp_kind: String,
    /// Always true for a generated trace, so it is never mistaken for measured.
    pub timestamp_is_synthetic: bool,
    /// Nullable; a generated trace names no model.
    pub model: Option<String>,
    /// Tokens, not blocks and not bytes.
    pub input_length: i64,
    /// 0: the plan makes no input/output distinction.
    pub output_length: i64,
    /// Invocation indices whose blocks this one re-reads. Empty by design; see
    /// the module note.
    pub reuse_from: Vec<i64>,
    /// Empty under the full encoding.
    pub new_input_blocks: Vec<i64>,
    /// Empty under the full encoding.
    pub new_output_blocks: Vec<i64>,
    /// The complete ordered input block list.
    pub full_input_blocks: Vec<i64>,
    /// Empty; see the module note.
    pub full_output_blocks: Vec<i64>,
    /// Valid tokens in the trailing partial block.
    pub partial_final_valid: i64,
}

/// Per-block-size counts. A reader uses `invocations` to tell a full trace from a
/// sample, so it must be the true count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockStats {
    /// Distinct sessions.
    pub sessions: u64,
    /// Rows.
    pub invocations: u64,
    /// Distinct block ids.
    pub unique_blocks: u64,
}

/// The trace manifest: what makes a trace self-describing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceManifest {
    /// Identifier shared by every row.
    pub trace_id: String,
    /// `pre_hashed` for generated data: the full encoding is the honest one.
    pub source_class: String,
    /// `rolling_prefix`, required for structural fitting — and true of the
    /// generator's keys by construction (spec FR-008).
    pub id_semantics: String,
    /// Tokens per block.
    pub block_size: u32,
    /// Every blocking this trace carries.
    pub block_sizes_available: Vec<u32>,
    /// `synthetic`, so a generated trace is never read as production.
    pub provenance: String,
    /// Always true here.
    pub timestamp_is_synthetic: bool,
    /// Per-field `native` | `reconstructed` | `unavailable`. A reader consults
    /// this **before** fitting, because a field this trace does not carry must be
    /// refused rather than defaulted.
    pub field_status: HashMap<String, String>,
    /// Capability summary: `B` roles, `R` reuse, `T` timing, `V` token counts.
    /// `P` is deliberately absent — the contract records that its meaning is not
    /// established and that readers must not depend on it.
    pub supports: String,
    /// The role vocabulary. Empty: block roles need source text, which a
    /// generated trace has none of.
    pub role_codes: HashMap<String, String>,
    /// Keyed by block size as a string, matching the on-disk layout.
    pub block_stats: HashMap<String, BlockStats>,
}

impl TraceManifest {
    /// The manifest for a generated trace.
    pub fn synthetic(trace_id: &str, block_size: u32, stats: BlockStats) -> TraceManifest {
        let mut field_status = HashMap::new();
        for f in [
            "session_id",
            "invocation_index",
            "request_start",
            "input_length",
            "full_input_blocks",
            "partial_final_valid",
        ] {
            field_status.insert(f.to_string(), "native".to_string());
        }
        // Named as unavailable rather than omitted: FR-055 requires `fit` to
        // refuse a parameter whose source field is unavailable, which it can only
        // do if the manifest says which those are.
        for f in [
            "request_end",
            "model",
            "output_length",
            "full_output_blocks",
            "new_input_blocks",
            "new_output_blocks",
            "reuse_from",
            "parent_invocations",
            "block_roles",
        ] {
            field_status.insert(f.to_string(), "unavailable".to_string());
        }
        let mut block_stats = HashMap::new();
        block_stats.insert(block_size.to_string(), stats);
        TraceManifest {
            trace_id: trace_id.to_string(),
            source_class: "pre_hashed".to_string(),
            id_semantics: "rolling_prefix".to_string(),
            block_size,
            block_sizes_available: vec![block_size],
            provenance: "synthetic".to_string(),
            timestamp_is_synthetic: true,
            field_status,
            // Timing and token counts are real; reuse structure is present as
            // shared global ids; roles are not.
            supports: "RTV".to_string(),
            role_codes: HashMap::new(),
            block_stats,
        }
    }

    /// Serialise to pretty JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Turns plan events into invocation records.
///
/// Block ids are **dense integers in mint order**, as the contract requires, so
/// the emitter assigns them on first appearance. That costs a map over distinct
/// keys, which is why it is the *emitter's* cost and not generation's: a bounded
/// file is what a block budget buys (FR-021d), and generation itself stays
/// bounded by the live session population (FR-010).
#[derive(Debug)]
pub struct Emitter {
    trace_id: String,
    block_size: u32,
    ids: HashMap<CacheKey, i64>,
    next_id: i64,
    time_origin_ns: u64,
    sessions: std::collections::HashSet<u32>,
    invocations: u64,
}

impl Emitter {
    /// An emitter for `trace_id`, with `block_size` tokens per block.
    pub fn new(trace_id: &str, block_size: u32, time_origin_ns: u64) -> Emitter {
        Emitter {
            trace_id: trace_id.to_string(),
            block_size: block_size.max(1),
            ids: HashMap::new(),
            next_id: 0,
            time_origin_ns,
            sessions: std::collections::HashSet::new(),
            invocations: 0,
        }
    }

    /// The dense id for `key`, minting one if this is its first appearance.
    fn id_of(&mut self, key: CacheKey) -> i64 {
        let next = &mut self.next_id;
        *self.ids.entry(key).or_insert_with(|| {
            let id = *next;
            *next += 1;
            id
        })
    }

    /// Convert one request's events — contiguous, `REQUEST_START` first — into a
    /// record.
    ///
    /// The `request` slice is exactly what a consumer gets by scanning to
    /// `REQUEST_END`, which is the contiguity guarantee being used rather than
    /// merely relied upon.
    pub fn request(&mut self, request: &[PlanEvent]) -> Option<Invocation> {
        let first = request.first()?;
        let blocks: Vec<i64> = request.iter().map(|e| self.id_of(e.key)).collect();
        let last_key = request.last()?.key;
        // Deterministic from the trailing key, so re-emitting the same plan gives
        // the same file. In 1..=block_size, since the full encoding includes the
        // partial block and a valid count of 0 would mean it should not be there.
        let pfv = 1 + (last_key.0 % u64::from(self.block_size)) as i64;
        let input_length = (blocks.len() as i64 - 1) * i64::from(self.block_size) + pfv;
        self.sessions.insert(first.session_id.0);
        self.invocations += 1;
        Some(Invocation {
            trace_id: self.trace_id.clone(),
            session_id: Some(first.session_id.0.to_string()),
            invocation_index: i64::from(first.turn.saturating_sub(1)),
            parent_invocation: i64::from(first.turn) - 2,
            parent_invocations: Vec::new(),
            request_start: Some((first.t_ns.saturating_sub(self.time_origin_ns)) as f64 / 1e9),
            request_end: None,
            timestamp_kind: "submission".to_string(),
            timestamp_is_synthetic: true,
            model: None,
            input_length,
            output_length: 0,
            reuse_from: Vec::new(),
            new_input_blocks: Vec::new(),
            new_output_blocks: Vec::new(),
            full_input_blocks: blocks,
            full_output_blocks: Vec::new(),
            partial_final_valid: pfv,
        })
    }

    /// What was emitted, for the manifest a reader checks a trace's completeness
    /// against.
    pub fn stats(&self) -> BlockStats {
        BlockStats {
            sessions: self.sessions.len() as u64,
            invocations: self.invocations,
            unique_blocks: self.ids.len() as u64,
        }
    }

    /// Tokens per block, as declared.
    pub fn block_size(&self) -> u32 {
        self.block_size
    }
}

/// Split a contiguous event slice into requests, by `REQUEST_END`.
///
/// The contract's promise from the reading side: a request is recoverable by
/// scanning forward without unbounded buffering.
pub fn requests(events: &[PlanEvent]) -> impl Iterator<Item = &[PlanEvent]> {
    let mut start = 0usize;
    let mut i = 0usize;
    std::iter::from_fn(move || {
        while i < events.len() {
            let end = events[i].has(flags::REQUEST_END);
            i += 1;
            if end {
                let s = start;
                start = i;
                return Some(&events[s..i]);
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::SessionId;
    use crate::plan::Generator;
    use crate::schema::Document;

    const DOC: &str = r#"
version: 1
seed: 4242
requests: 300
corpus:
  block_bytes: 131072
  trees:
    roots: {count: 4, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: const, value: 5}
    branching: 1.02
workload:
  arrival: {model: open_loop, rate: 2000/s}
  sessions:
    turns: {dist: const, value: 3}
    think_time: {dist: const, value: 0.05}
    private_depth: {dist: const, value: 3}
    growth_per_turn: {dist: const, value: 2}
run: {mode: jsonl, wss_window: 60000}
"#;

    fn plan() -> Vec<PlanEvent> {
        let d = Document::from_yaml(DOC).unwrap();
        let mut g = Generator::new(&d).unwrap();
        let mut all = Vec::new();
        let mut buf = Vec::new();
        while g.fill(&mut buf) > 0 {
            all.extend_from_slice(&buf);
        }
        all
    }

    fn emit_all(events: &[PlanEvent]) -> (Emitter, Vec<Invocation>) {
        let mut em = Emitter::new("t", DEFAULT_BLOCK_SIZE_TOKENS, 0);
        let recs: Vec<Invocation> = requests(events).filter_map(|r| em.request(r)).collect();
        (em, recs)
    }

    #[test]
    fn requests_are_recovered_by_scanning_to_request_end() {
        let events = plan();
        let recovered: Vec<&[PlanEvent]> = requests(&events).collect();
        assert_eq!(recovered.len(), 300);
        for r in &recovered {
            assert!(r[0].has(flags::REQUEST_START));
            assert!(r[r.len() - 1].has(flags::REQUEST_END));
            assert!(r[1..r.len() - 1].iter().all(|e| e.flags & 0b11 == 0));
        }
        // Every event is accounted for exactly once.
        assert_eq!(
            recovered.iter().map(|r| r.len()).sum::<usize>(),
            events.len()
        );
    }

    #[test]
    fn the_full_encodings_two_invariants_hold_on_every_record() {
        // The trap the contract records: the full encoding includes the trailing
        // partial block, the delta encoding excludes it, and a reader assuming
        // the wrong one is off by one block per request. Verified on every row,
        // as the contract's own rules were.
        let (em, recs) = emit_all(&plan());
        let bs = i64::from(em.block_size());
        assert!(!recs.is_empty());
        for r in &recs {
            assert_eq!((r.input_length - r.partial_final_valid) % bs, 0, "{r:?}");
            assert_eq!(
                r.full_input_blocks.len() as i64,
                (r.input_length - r.partial_final_valid) / bs + 1
            );
            assert!(r.partial_final_valid >= 1 && r.partial_final_valid <= bs);
        }
    }

    #[test]
    fn the_delta_fields_are_empty_because_the_encodings_are_exclusive() {
        // A reader detects the encoding by `full_input_blocks` non-empty versus
        // empty, so populating both would make the file undetectable.
        let (_, recs) = emit_all(&plan());
        for r in &recs {
            assert!(!r.full_input_blocks.is_empty());
            assert!(r.new_input_blocks.is_empty());
            assert!(r.new_output_blocks.is_empty());
            assert!(r.reuse_from.is_empty());
        }
    }

    #[test]
    fn block_ids_are_dense_and_in_mint_order() {
        // The contract's requirement, and what lets a reader index by id.
        let (em, recs) = emit_all(&plan());
        let mut seen: Vec<i64> = recs
            .iter()
            .flat_map(|r| r.full_input_blocks.iter().copied())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.first(), Some(&0));
        assert_eq!(seen.len() as u64, em.stats().unique_blocks);
        assert_eq!(*seen.last().unwrap(), seen.len() as i64 - 1, "not dense");
        // First appearance mints, so the first record's ids ascend from 0.
        let first = &recs[0].full_input_blocks;
        assert_eq!(*first, (0..first.len() as i64).collect::<Vec<_>>());
    }

    #[test]
    fn cross_session_sharing_survives_as_repeated_global_ids() {
        // The contract is explicit that this -- not `reuse_from` -- is the sharing
        // signal, so a generated trace has to carry it the same way a real one
        // does or `fit` would read no sharing at all.
        let (_, recs) = emit_all(&plan());
        let mut owners: HashMap<i64, std::collections::HashSet<String>> = HashMap::new();
        for r in &recs {
            let sid = r.session_id.clone().unwrap();
            for b in &r.full_input_blocks {
                owners.entry(*b).or_default().insert(sid.clone());
            }
        }
        let shared = owners.values().filter(|s| s.len() > 1).count();
        assert!(shared > 0, "no block was listed by two sessions");
    }

    #[test]
    fn a_session_is_recoverable_and_its_turns_are_ordered() {
        let (_, recs) = emit_all(&plan());
        let mut by_session: HashMap<String, Vec<&Invocation>> = HashMap::new();
        for r in &recs {
            by_session
                .entry(r.session_id.clone().unwrap())
                .or_default()
                .push(r);
        }
        let mut multi = 0;
        for rs in by_session.values() {
            let mut idx: Vec<i64> = rs.iter().map(|r| r.invocation_index).collect();
            idx.sort_unstable();
            assert_eq!(idx, (0..idx.len() as i64).collect::<Vec<_>>());
            for r in rs {
                assert_eq!(r.parent_invocation, r.invocation_index - 1);
            }
            if rs.len() > 1 {
                multi += 1;
            }
        }
        assert!(multi > 0, "no multi-turn session to check ordering on");
    }

    #[test]
    fn growth_per_turn_is_recoverable_from_successive_block_lists() {
        // What stands in for the absent output-block attribution: a reader gets
        // the same information without the emitter inventing which blocks were
        // "output".
        let (_, recs) = emit_all(&plan());
        let mut by_session: HashMap<String, Vec<&Invocation>> = HashMap::new();
        for r in &recs {
            by_session
                .entry(r.session_id.clone().unwrap())
                .or_default()
                .push(r);
        }
        let mut checked = 0;
        for rs in by_session.values() {
            let mut rs = rs.clone();
            rs.sort_by_key(|r| r.invocation_index);
            for w in rs.windows(2) {
                let grew = w[1].full_input_blocks.len() - w[0].full_input_blocks.len();
                assert_eq!(grew, 2, "growth_per_turn was configured as 2");
                // And the earlier turn's list is a prefix of the later one's.
                assert_eq!(
                    &w[1].full_input_blocks[..w[0].full_input_blocks.len()],
                    &w[0].full_input_blocks[..]
                );
                checked += 1;
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn the_manifest_declares_what_it_is_and_what_it_lacks() {
        // FR-021b, and FR-055's precondition: `fit` can only refuse an
        // unavailable field if the manifest names it as unavailable.
        let (em, _) = emit_all(&plan());
        let m = TraceManifest::synthetic("t", em.block_size(), em.stats());
        assert_eq!(m.source_class, "pre_hashed");
        assert_eq!(m.id_semantics, "rolling_prefix");
        assert_eq!(m.provenance, "synthetic");
        assert!(m.timestamp_is_synthetic);
        assert_eq!(m.field_status["full_input_blocks"], "native");
        assert_eq!(m.field_status["block_roles"], "unavailable");
        assert_eq!(m.field_status["output_length"], "unavailable");
        assert!(!m.supports.contains('P'), "P's meaning is not established");
        assert!(!m.supports.contains('B'), "roles need source text");
        assert!(m.role_codes.is_empty());
        // The count a reader uses to tell a full trace from a sample.
        assert_eq!(m.block_stats["16"].invocations, 300);
        assert!(m.block_stats["16"].sessions > 0);
    }

    #[test]
    fn a_record_serialises_with_the_contracts_own_field_names() {
        // A third-party reader matches on these, so a rename is a breaking change
        // rather than a refactor.
        let (_, recs) = emit_all(&plan());
        let line = serde_json::to_string(&recs[0]).unwrap();
        for f in [
            "trace_id",
            "session_id",
            "invocation_index",
            "parent_invocation",
            "parent_invocations",
            "request_start",
            "request_end",
            "timestamp_kind",
            "timestamp_is_synthetic",
            "model",
            "input_length",
            "output_length",
            "reuse_from",
            "new_input_blocks",
            "new_output_blocks",
            "full_input_blocks",
            "full_output_blocks",
            "partial_final_valid",
        ] {
            assert!(line.contains(&format!("\"{f}\"")), "missing {f}");
        }
        // And it round-trips, which is what the FR-021j integration test needs.
        let back: Invocation = serde_json::from_str(&line).unwrap();
        assert_eq!(back, recs[0]);
    }

    #[test]
    fn emitting_the_same_plan_twice_gives_the_identical_file() {
        let events = plan();
        let (_, a) = emit_all(&events);
        let (_, b) = emit_all(&events);
        assert_eq!(a, b);
    }

    #[test]
    fn an_absent_parent_invocations_is_empty_rather_than_unknown() {
        // The contract's JSONL note, from the writing side.
        let mut em = Emitter::new("t", 16, 0);
        let one = [PlanEvent {
            t_ns: 0,
            key: CacheKey(9),
            size: 1,
            request_id: 0,
            session_id: SessionId(0),
            depth: 0,
            turn: 1,
            node: 0,
            mix_index: 0,
            flags: flags::REQUEST_START | flags::REQUEST_END,
        }];
        let r = em.request(&one).unwrap();
        assert!(r.parent_invocations.is_empty());
        assert_eq!(r.parent_invocation, -1, "turn 1 has no predecessor");
        assert_eq!(r.invocation_index, 0);
        // One block, so the whole input is the trailing partial block.
        assert_eq!(r.full_input_blocks.len(), 1);
        assert_eq!(r.input_length, r.partial_final_valid);
    }
}
