//! One invocation record — the unit a trace file is made of.
//!
//! `contracts/trace-io.md` is normative for the field set. This module's job is the
//! mapping from a simulated turn onto it, and the mapping has three places where
//! the obvious choice is wrong.
//!
//! # A turn's *prompt* is not a turn's *reads*
//!
//! [`Turn`] separates what a turn read from what it minted, because FR-025 defines
//! the read set as the prefix *before* the turn. A trace's `full_input_blocks` is
//! the request's prompt, which is that prefix **plus** the turn's own new input —
//! the new user text is part of what gets sent. So:
//!
//! ```text
//! full_input_blocks  = reads ++ new_input
//! full_output_blocks = new_output
//! ```
//!
//! Conflating the two would understate every prompt by exactly one turn's growth
//! and, worse, would make the trace's own invariant
//! `len(full_input_blocks) * block_size == input_length` fail.
//!
//! # Lengths are in tokens, because real traces are
//!
//! `input_length` is `blocks * block_size` tokens, as every target states it. Reporting
//! blocks would have been more natural for this generator and would have broken
//! field-level comparability with the real traces the format exists to sit beside.
//!
//! # `request_end` and `partial_final_valid` are null, not zero
//!
//! A turn occupies a single virtual instant — nothing here models service time — so
//! a `request_end` equal to `request_start` would be a measurement this generator
//! did not make. And this generator mints whole blocks, so there is never a
//! trailing partial block. Both fields are **present and null**, so a reader that
//! expects them is satisfied without having to recognise which trace it is
//! (FR-056), and neither can be mistaken for a real zero (the same rule as FR-071's
//! omitted report fields).

use serde::Serialize;
use workload_model::keys::CacheKey;
use workload_model::session::{Session, Turn};

/// One invocation, ready to serialise.
///
/// Field order here is the order they appear in a JSONL line, which is fixed so
/// that two runs of the same description produce byte-identical files.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InvocationRecord {
    /// Which trace this row belongs to.
    pub trace_id: String,
    /// The session, as a string because a real trace's may not be numeric.
    pub session_id: String,
    /// 0-based position within the session.
    pub invocation_index: i64,
    /// Previous turn, or −1 at a session root.
    pub parent_invocation: i64,
    /// Virtual seconds on the run-global clock.
    pub request_start: f64,
    /// Always `None`; see the module docs.
    pub request_end: Option<f64>,
    /// Always `"start"`.
    pub timestamp_kind: &'static str,
    /// Always `true` — the clock is virtual.
    pub timestamp_is_synthetic: bool,
    /// Always `None`: this generator does not model models.
    pub model: Option<String>,
    /// Prompt length in **tokens**.
    pub input_length: i64,
    /// Generated length in **tokens**.
    pub output_length: i64,
    /// Invocation indices whose blocks this turn re-reads.
    pub reuse_from: Vec<i64>,
    /// Input keys first minted at this turn.
    pub new_input_blocks: Vec<CacheKey>,
    /// Output keys first minted at this turn.
    pub new_output_blocks: Vec<CacheKey>,
    /// The complete ordered prompt: prior prefix then this turn's new input.
    pub full_input_blocks: Vec<CacheKey>,
    /// The complete ordered generated run.
    pub full_output_blocks: Vec<CacheKey>,
    /// Always `None`; see the module docs.
    pub partial_final_valid: Option<i64>,
}

impl InvocationRecord {
    /// Build a record from a live session and one of its turns.
    ///
    /// `block_size` is tokens per block, from the description's `blocks.tokens`.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::description::WorkloadDescription;
    /// use workload_model::sim::Simulation;
    /// use workload_trace::record::InvocationRecord;
    ///
    /// let yaml = r#"
    /// version: 1
    /// blocks: {tokens: 16, bytes: 32768}
    /// shared_classes:
    ///   m: {length: {constant: 3}, lifetime: {constant: .inf}}
    /// session_classes:
    ///   c:
    ///     pool: {size: {exact: 1}}
    ///     uses: [{class: m, count: {constant: 1}}]
    ///     turns: {constant: 2}
    ///     input_growth: {constant: 2}
    ///     output_growth: {constant: 1}
    ///     think_time: {constant: 1}
    /// "#;
    /// let d: WorkloadDescription = yaml.parse().unwrap();
    /// let mut sim = Simulation::new(&d, 1, 1).unwrap();
    /// let mut rows = Vec::new();
    /// sim.run_until(50.0, &mut |s, t| {
    ///     rows.push(InvocationRecord::from_turn("demo", s, t, 16));
    /// });
    ///
    /// let first = &rows[0];
    /// // The prompt is the prefix read plus this turn's own new input.
    /// assert_eq!(first.full_input_blocks.len(), 3 + 2);
    /// assert_eq!(first.input_length, (3 + 2) * 16);
    /// assert_eq!(first.full_output_blocks.len(), 1);
    /// assert_eq!(first.parent_invocation, -1);
    /// assert!(first.request_end.is_none());
    /// ```
    pub fn from_turn(trace_id: &str, session: &Session, turn: &Turn, block_size: u64) -> Self {
        let index = turn.index() as i64;
        let reads = session.reads_of(turn);
        let new_input = session.new_input_of(turn);
        let new_output = session.new_output_of(turn);

        let mut full_input = Vec::with_capacity(reads.len() + new_input.len());
        full_input.extend_from_slice(reads);
        full_input.extend_from_slice(new_input);

        Self {
            trace_id: trace_id.to_string(),
            session_id: session.id().to_string(),
            invocation_index: index,
            parent_invocation: index - 1,
            request_start: turn.at(),
            request_end: None,
            timestamp_kind: "start",
            timestamp_is_synthetic: true,
            model: None,
            input_length: full_input.len() as i64 * block_size as i64,
            output_length: new_output.len() as i64 * block_size as i64,
            // Append-only, so every earlier turn of this session contributed blocks
            // this one re-reads. A fan-in model would need `parent_invocations`
            // instead, which is out of scope.
            reuse_from: (0..index).collect(),
            new_input_blocks: new_input.to_vec(),
            new_output_blocks: new_output.to_vec(),
            full_input_blocks: full_input,
            full_output_blocks: new_output.to_vec(),
            partial_final_valid: None,
        }
    }

    /// Check the per-row invariants `contracts/trace-io.md` states (FR-058).
    ///
    /// Takes the previous row of the *same session* so the append-only property can
    /// be checked, which is the one invariant a single row cannot express.
    ///
    /// # Errors
    ///
    /// Naming the invariant and the row, because a trace that violates its own
    /// declared schema is worse than one that declares less.
    pub fn check(
        &self,
        block_size: u64,
        previous: Option<&InvocationRecord>,
    ) -> Result<(), String> {
        let at = |what: &str| {
            format!(
                "session {} turn {}: {what}",
                self.session_id, self.invocation_index
            )
        };

        if self.full_input_blocks.len() as i64 * block_size as i64 != self.input_length {
            return Err(at(&format!(
                "{} input blocks at {block_size} tokens each is not input_length {}",
                self.full_input_blocks.len(),
                self.input_length
            )));
        }
        if self.full_output_blocks.len() as i64 * block_size as i64 != self.output_length {
            return Err(at("output blocks do not match output_length"));
        }
        if self.partial_final_valid.is_some() {
            return Err(at(
                "partial_final_valid is set, but blocks are always whole",
            ));
        }
        if self.request_end.is_some() {
            return Err(at("request_end is set, but no service time is modelled"));
        }

        // New blocks are the tail of their full lists.
        let n = self.new_input_blocks.len();
        if self.full_input_blocks.len() < n
            || self.full_input_blocks[self.full_input_blocks.len() - n..]
                != self.new_input_blocks[..]
        {
            return Err(at("new_input_blocks is not the tail of full_input_blocks"));
        }
        if self.full_output_blocks != self.new_output_blocks {
            return Err(at("full_output_blocks differs from new_output_blocks"));
        }

        match previous {
            None => {
                if self.invocation_index != 0 || self.parent_invocation != -1 {
                    return Err(at("a session's first row must be index 0 with parent -1"));
                }
            }
            Some(p) => {
                if self.invocation_index != p.invocation_index + 1 {
                    return Err(at("invocation_index is not consecutive"));
                }
                if self.parent_invocation != p.invocation_index {
                    return Err(at("parent_invocation does not name the previous turn"));
                }
                if self.request_start < p.request_start {
                    return Err(at("request_start went backwards within a session"));
                }
                // Append-only: this prompt begins with the previous prompt followed
                // by the previous output (FR-025). The single most valuable row
                // check, because it is what makes the trace's reuse structure real.
                let mut expected =
                    Vec::with_capacity(p.full_input_blocks.len() + p.full_output_blocks.len());
                expected.extend_from_slice(&p.full_input_blocks);
                expected.extend_from_slice(&p.full_output_blocks);
                if self.full_input_blocks.len() < expected.len()
                    || self.full_input_blocks[..expected.len()] != expected[..]
                {
                    return Err(at(
                        "the prompt does not begin with the previous prompt followed \
                         by the previous output; the chain is not append-only",
                    ));
                }
            }
        }
        Ok(())
    }
}
