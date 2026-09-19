//! The operation plan: the boundary between simulation and execution.
//!
//! A plan is a value. It is a pure function of the description and the seed, and
//! it is what both execution paths consume — the live path drives it into a Certus
//! node, the emit path writes it to a file — so FR-072's "the plan does not vary
//! with batch size or lane count" holds because there is nothing in here that
//! could vary with them.
//!
//! # What a turn becomes
//!
//! One turn produces the operations the production client would issue for it
//! (FR-039), in this order:
//!
//! 1. **`Check`** over the turn's whole prefix — which blocks are present.
//! 2. **`Touch`** over the same keys: the **reference report**, with no promotion
//!    requested (FR-041).
//! 3. **`Load`** over the same keys, for whatever the check found.
//! 4. **`Reserve` → `Transfer` → `Commit`** over the turn's new **input** blocks.
//! 5. The same three over its new **output** blocks.
//! 6. **`PollEvents`**, because the production client polls and it costs the
//!    server real work (FR-042).
//!
//! # `Touch` is separate from `Check`, and leaving it out was a real defect
//!
//! An earlier version of this module had no `Touch`, on the reading that a `Check`
//! *is* the reference report. It is not. In the shipped client they are different
//! calls against different opcodes: `batch_check` asks which blocks are present,
//! while `touch` is documented as "update eviction ordering for the given keys" —
//! and `certus_offload_manager.py` invokes it as its own step.
//!
//! Omitting it does not lose an operation so much as lose a *signal*: the eviction
//! policy would never learn that a read happened, so recency-based policies would
//! be scored against a workload in which nothing is ever recently used. For an
//! instrument whose stated purpose is evaluating that policy, that is the worst
//! available bug, and nothing in the emitted trace or the plan's own invariants
//! would have shown it. FR-044 — "MUST NOT withhold information the production
//! client would supply" — is exactly this requirement.
//!
//! Input and output get **separate store sequences** rather than one covering
//! both: a real engine stores the prompt's new blocks when prefill completes and
//! the generated blocks as decode produces them, so a single sequence would
//! understate the number of round trips a turn costs.
//!
//! There is no single-shot store, because the production client has none (FR-040).
//! [`OpKind::Abort`] exists because the executor needs it for the failure branch of
//! a commit, and **the planner never emits it** — which branch is taken is a
//! run-time outcome and cannot be in a plan that must be reproducible.
//!
//! # What the plan cannot know, and why that is not a defect
//!
//! `Touch` and `Load` carry the same keys as their `Check`. They have to: which of them were
//! present is a run-time outcome, and two sessions racing to mint the same shared
//! prefix means the answer legitimately differs between runs at a fixed seed
//! (FR-035, FR-036). So the plan states the **candidate** set and the executor
//! narrows it. A plan that recorded hits would be a plan that could not be
//! reproduced.
//!
//! # Ordering
//!
//! Totally ordered by virtual time, ties broken on session id (FR-035). The
//! simulation delivers turns in exactly that order, so a plan is built by
//! appending and never by sorting — which matters because an emit run streams and
//! cannot hold the plan to sort it at the end. [`OperationPlan::check_ordered`]
//! asserts the property rather than trusting it.
//!
//! # Keys live in one arena
//!
//! Every operation's keys are a range into a single [`Vec`], not a `Vec` of its
//! own. A turn's `Check` carries its whole prefix, so per-operation vectors would
//! put an allocation on a path whose size already grows quadratically with session
//! length. `Check` and `Load` share one range, since they cover the same keys.
//!
//! # Examples
//!
//! ```
//! use workload_model::description::WorkloadDescription;
//! use workload_model::plan::{OpKind, OperationPlan};
//! use workload_model::sim::Simulation;
//!
//! let yaml = r#"
//! version: 1
//! blocks: {tokens: 16, bytes: 32768}
//! shared_classes:
//!   manual:
//!     length: {constant: 4}
//!     lifetime: {constant: .inf}
//! session_classes:
//!   chat:
//!     pool: {size: {exact: 2}}
//!     uses: [{class: manual, count: {constant: 1}}]
//!     turns: {constant: 3}
//!     input_growth: {constant: 2}
//!     output_growth: {constant: 1}
//!     think_time: {constant: 5}
//! "#;
//! let description: WorkloadDescription = yaml.parse().unwrap();
//! let mut sim = Simulation::new(&description, 7, 1).unwrap();
//!
//! let mut plan = OperationPlan::default();
//! sim.run_until(100.0, &mut |session, turn| plan.record_turn(session, turn));
//!
//! assert!(plan.len() > 0);
//! plan.check_ordered().unwrap();
//! // Every turn checks before it loads, and stores through reserve/transfer/commit.
//! assert!(plan.operations().iter().any(|o| o.kind() == OpKind::Check));
//! assert!(plan.operations().iter().any(|o| o.kind() == OpKind::Commit));
//! assert!(!plan.operations().iter().any(|o| o.kind() == OpKind::Abort));
//!
//! // The canonical form is what byte-identity is asserted against (SC-003).
//! let bytes = plan.to_canonical_bytes();
//! assert_eq!(&bytes[..8], b"CERTUSPL");
//! ```

use std::io::{self, Write};
use std::ops::Range;

use crate::keys::CacheKey;
use crate::session::{Session, Turn};
use crate::{Error, Result};

/// Magic at the head of a canonical plan, so a truncated or foreign file is
/// rejected rather than misread.
const MAGIC: &[u8; 8] = b"CERTUSPL";

/// Canonical-serialisation version. A change to the layout is a new version, never
/// an edit — the same rule as the key derivation, and for the same reason: an edit
/// produces a file that still loads and means something different.
pub const CANONICAL_VERSION: u16 = 1;

/// What an operation asks Certus to do.
///
/// The discriminants are the wire `op_kind` values in
/// `contracts/node-agent-wire.md`, so the plan and the wire cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum OpKind {
    /// Learn which blocks are present.
    Check = 0,
    /// Report the reference, updating eviction ordering, **without** requesting
    /// promotion (FR-041, FR-043). The signal a recency policy runs on.
    Touch = 1,
    /// Fetch blocks the check found.
    Load = 2,
    /// First step of a store: ask for space (FR-040).
    Reserve = 3,
    /// Second step: move the payload.
    Transfer = 4,
    /// Third step, success branch.
    Commit = 5,
    /// Third step, failure branch. **Never planned** — the executor chooses it.
    Abort = 6,
    /// Poll for cache events, which the production client does (FR-042).
    PollEvents = 7,
}

impl OpKind {
    /// The wire `op_kind` byte.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Every kind, in wire order. Useful for exhaustiveness in tests and reports.
    pub fn all() -> [OpKind; 8] {
        [
            OpKind::Check,
            OpKind::Touch,
            OpKind::Load,
            OpKind::Reserve,
            OpKind::Transfer,
            OpKind::Commit,
            OpKind::Abort,
            OpKind::PollEvents,
        ]
    }
}

/// One operation, with its keys held as a range into the plan's arena.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    at_bits: u64,
    session: u64,
    node: u16,
    kind: OpKind,
    keys: Range<usize>,
}

impl Operation {
    /// Virtual time the operation happens at.
    pub fn at(&self) -> f64 {
        f64::from_bits(self.at_bits)
    }

    /// The session that issues it, which is also the ordering tie-break.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// Node placement at the time of the operation.
    ///
    /// Always 0 for now: placement and migration are User Story 3. The field is
    /// present from the start because it is in the canonical serialisation, and
    /// adding it later would invalidate every recorded plan digest.
    pub fn node(&self) -> u16 {
        self.node
    }

    /// What it asks for.
    pub fn kind(&self) -> OpKind {
        self.kind
    }

    /// How many keys it carries.
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }
}

/// How a plan is built from turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanOptions {
    /// Emit a `PollEvents` every this many turns, per session. `1` means every
    /// turn.
    ///
    /// **This is an assumption, not a measured fact.** FR-042 requires polling and
    /// FR-039 requires the stream to be what the production client would emit, but
    /// the client's exact cadence is not recorded anywhere in this repository. One
    /// poll per request loop is the conservative reading; the knob exists so that
    /// being wrong costs a flag rather than a redesign.
    pub poll_events_every: u64,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self {
            poll_events_every: 1,
        }
    }
}

/// A totally ordered sequence of cache operations in virtual time.
#[derive(Debug, Clone, Default)]
pub struct OperationPlan {
    options: PlanOptions,
    keys: Vec<CacheKey>,
    ops: Vec<Operation>,
    turns: u64,
}

impl OperationPlan {
    /// An empty plan with the given options.
    pub fn with_options(options: PlanOptions) -> Self {
        Self {
            options,
            ..Self::default()
        }
    }

    /// The operations, in order.
    pub fn operations(&self) -> &[Operation] {
        &self.ops
    }

    /// Operations in the plan.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Whether nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Turns recorded.
    pub fn turns(&self) -> u64 {
        self.turns
    }

    /// Total key references across every operation — the figure the projection
    /// reports separately from minted keys (FR-073).
    ///
    /// Summed over operations, **not** the size of the key arena. `Check` and
    /// `Load` share one range as a storage optimisation, and that must not change
    /// the reported number: a key mentioned by two operations crosses the wire
    /// twice. Use [`Self::interned_keys`] for the arena's own size.
    ///
    /// Per session this grows **quadratically** in turn count, because every turn
    /// checks its whole prefix. Note it does *not* grow quadratically with a run's
    /// span: longer sessions mean proportionally fewer of them, so the total over a
    /// fixed span is linear in turn count.
    pub fn key_references(&self) -> usize {
        self.ops.iter().map(|o| o.keys.len()).sum()
    }

    /// Keys stored in the arena, which is less than [`Self::key_references`]
    /// wherever operations share a range.
    pub fn interned_keys(&self) -> usize {
        self.keys.len()
    }

    /// The keys one operation carries, in prefix order.
    pub fn keys_of(&self, op: &Operation) -> &[CacheKey] {
        &self.keys[op.keys.clone()]
    }

    /// Record one turn's operations.
    ///
    /// Call this from [`Simulation::run_until`](crate::sim::Simulation::run_until)'s
    /// callback: the session must still be alive for its keys to be readable.
    pub fn record_turn(&mut self, session: &Session, turn: &Turn) {
        let at_bits = turn.at().to_bits();
        let id = session.id();

        // Check and Load share one range: same keys, and a turn's prefix is the
        // largest key list in the plan.
        let reads = session.reads_of(turn);
        if !reads.is_empty() {
            let range = self.intern(reads);
            self.push(at_bits, id, OpKind::Check, range.clone());
            // The reference report. Separate from the check because the shipped
            // client's is — see the module docs on why omitting it was a defect.
            self.push(at_bits, id, OpKind::Touch, range.clone());
            self.push(at_bits, id, OpKind::Load, range);
        }

        // Two store sequences, input then output — a real engine stores the
        // prompt's new blocks at the end of prefill and the generated ones as decode
        // produces them.
        for keys in [session.new_input_of(turn), session.new_output_of(turn)] {
            if keys.is_empty() {
                continue;
            }
            let range = self.intern(keys);
            for kind in [OpKind::Reserve, OpKind::Transfer, OpKind::Commit] {
                self.push(at_bits, id, kind, range.clone());
            }
        }

        self.turns += 1;
        let every = self.options.poll_events_every.max(1);
        if (turn.index() as u64 + 1) % every == 0 {
            self.push(at_bits, id, OpKind::PollEvents, 0..0);
        }
    }

    /// Append this plan's ordered key path — every distinct key it references, in first
    /// reference order — to `out`, which is cleared first.
    ///
    /// # Why the live path needs the path rather than the operations
    ///
    /// A real prefix-caching client does not know in advance what it must store. It offers
    /// the whole path from the start of the prefix to the end of the new growth, and stores
    /// whatever came back absent. Which keys those are depends on the cache, so the
    /// *operations* a turn issues cannot be fixed ahead of time — only the path can.
    ///
    /// That is why `spec.md`'s race note ("two sessions race to mint the same shared
    /// prefix, both may miss and both store") makes sense at all, and it is also what lets
    /// a block evicted mid-run be stored again. With a fixed operation list nothing ever
    /// re-stores an evicted block, so a run's hit rate can only decay.
    ///
    /// The path is taken from the `Check` and `Reserve` operations, which between them name
    /// the prefix and the growth; `Touch`/`Load` repeat the check's keys and
    /// `Transfer`/`Commit` repeat the reserve's, so including them would only duplicate.
    pub fn key_path(&self, out: &mut Vec<u64>) {
        out.clear();
        let mut seen = std::collections::HashSet::new();
        for op in &self.ops {
            if !matches!(op.kind(), OpKind::Check | OpKind::Reserve) {
                continue;
            }
            for key in self.keys_of(op) {
                if seen.insert(*key) {
                    out.push(*key);
                }
            }
        }
    }

    /// Check the ordering invariant: non-decreasing virtual time, and within one
    /// instant non-decreasing session id (FR-035).
    ///
    /// # Errors
    ///
    /// Naming the first offending pair, because a plan out of order would be read
    /// as a workload rather than as a defect.
    pub fn check_ordered(&self) -> Result<()> {
        for (i, w) in self.ops.windows(2).enumerate() {
            let (a, b) = (&w[0], &w[1]);
            if b.at() < a.at() {
                return Err(Error::new(format!(
                    "operation {} is at {} but operation {i} is at {}",
                    i + 1,
                    b.at(),
                    a.at()
                )));
            }
            if b.at() == a.at() && b.session < a.session {
                return Err(Error::new(format!(
                    "operations {i} and {} share instant {} but their sessions are \
                     out of order: {} then {}",
                    i + 1,
                    a.at(),
                    a.session,
                    b.session
                )));
            }
        }
        Ok(())
    }

    /// Write the canonical serialisation — the artifact byte-identity is asserted
    /// against (FR-060, SC-003).
    ///
    /// Layout, all integers **little-endian**:
    ///
    /// ```text
    /// magic        8 bytes  "CERTUSPL"
    /// version      u16
    /// reserved     u16      zero
    /// op_count     u64
    /// per op:  at_bits u64 | session u64 | node u16 | kind u8 | pad u8
    ///          key_count u32 | key u64 * key_count
    /// ```
    ///
    /// Explicit padding rather than a packed struct, so the format does not depend
    /// on any compiler's layout choices. `at` is written as its **bit pattern**, not
    /// as a decimal: a decimal rendering would make byte-identity depend on
    /// float formatting, which is exactly the kind of accidental dependency SC-003
    /// exists to rule out.
    ///
    /// # Errors
    ///
    /// Whatever the writer returns.
    pub fn write_canonical<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(MAGIC)?;
        w.write_all(&CANONICAL_VERSION.to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.write_all(&(self.ops.len() as u64).to_le_bytes())?;
        for op in &self.ops {
            w.write_all(&op.at_bits.to_le_bytes())?;
            w.write_all(&op.session.to_le_bytes())?;
            w.write_all(&op.node.to_le_bytes())?;
            w.write_all(&[op.kind.as_u8(), 0u8])?;
            w.write_all(&(op.keys.len() as u32).to_le_bytes())?;
            for k in &self.keys[op.keys.clone()] {
                w.write_all(&k.to_le_bytes())?;
            }
        }
        Ok(())
    }

    /// The canonical serialisation as bytes.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_canonical(&mut out)
            .expect("writing to a Vec cannot fail");
        out
    }

    /// A 64-bit fingerprint of the canonical form, for putting in a report.
    ///
    /// A convenience for spotting that two runs differ, **not** a cryptographic
    /// digest and not a substitute for comparing bytes: SC-003 is asserted on the
    /// bytes themselves.
    pub fn fingerprint(&self) -> u64 {
        let mut acc = 0xcbf2_9ce4_8422_2325u64;
        for b in self.to_canonical_bytes() {
            acc = crate::keys::splitmix64(acc ^ b as u64);
        }
        acc
    }

    /// Copy keys into the arena and return their range.
    fn intern(&mut self, keys: &[CacheKey]) -> Range<usize> {
        let start = self.keys.len();
        self.keys.extend_from_slice(keys);
        start..self.keys.len()
    }

    fn push(&mut self, at_bits: u64, session: u64, kind: OpKind, keys: Range<usize>) {
        self.ops.push(Operation {
            at_bits,
            session,
            // US3 sets this; see `Operation::node`.
            node: 0,
            kind,
            keys,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::description::WorkloadDescription;
    use crate::sim::Simulation;

    fn description(turns: u32, sessions: u32, input: u32, output: u32) -> WorkloadDescription {
        format!(
            r#"
version: 1
blocks: {{tokens: 16, bytes: 32768}}
shared_classes:
  manual:
    length: {{constant: 4}}
    lifetime: {{constant: .inf}}
session_classes:
  chat:
    pool: {{size: {{exact: {sessions}}}}}
    uses: [{{class: manual, count: {{constant: 1}}}}]
    turns: {{constant: {turns}}}
    input_growth: {{constant: {input}}}
    output_growth: {{constant: {output}}}
    think_time: {{constant: 5}}
"#
        )
        .parse()
        .unwrap()
    }

    fn plan_of(d: &WorkloadDescription, seed: u64, until: f64) -> OperationPlan {
        let mut sim = Simulation::new(d, seed, 1).unwrap();
        let mut plan = OperationPlan::default();
        sim.run_until(until, &mut |s, t| plan.record_turn(s, t));
        plan
    }

    #[test]
    fn a_turn_produces_the_production_clients_operation_sequence() {
        // FR-039/040/041/042 as one shape: check, load, then a reserve/transfer/
        // commit for input and another for output, then a poll.
        let d = description(2, 1, 2, 3);
        let plan = plan_of(&d, 1, 6.0);
        let kinds: Vec<OpKind> = plan.operations().iter().map(|o| o.kind()).collect();
        assert_eq!(
            &kinds[..10],
            &[
                OpKind::Check,
                OpKind::Touch,
                OpKind::Load,
                OpKind::Reserve,
                OpKind::Transfer,
                OpKind::Commit,
                OpKind::Reserve,
                OpKind::Transfer,
                OpKind::Commit,
                OpKind::PollEvents,
            ],
            "got {kinds:?}"
        );
        // Check and Load carry the same keys; the two store runs carry input then
        // output.
        let ops = plan.operations();
        // Check, Touch and Load share one key range.
        assert_eq!(plan.keys_of(&ops[0]), plan.keys_of(&ops[1]));
        assert_eq!(plan.keys_of(&ops[1]), plan.keys_of(&ops[2]));
        assert_eq!(ops[3].key_count(), 2, "input store covers input growth");
        assert_eq!(ops[6].key_count(), 3, "output store covers output growth");
        assert_eq!(ops[9].key_count(), 0, "a poll carries no keys");
    }

    #[test]
    fn no_single_shot_store_and_no_planned_abort() {
        // FR-040: the production client has neither, so neither may appear.
        let d = description(4, 6, 1, 1);
        let plan = plan_of(&d, 2, 200.0);
        assert!(!plan.is_empty());
        let mut commits = 0;
        for op in plan.operations() {
            assert_ne!(op.kind(), OpKind::Abort, "the planner emitted an Abort");
            if op.kind() == OpKind::Commit {
                commits += 1;
            }
        }
        assert!(commits > 0);
        // Every commit is preceded by a transfer and a reserve over the same keys.
        let ops = plan.operations();
        for (i, op) in ops.iter().enumerate() {
            if op.kind() == OpKind::Commit {
                assert_eq!(ops[i - 1].kind(), OpKind::Transfer);
                assert_eq!(ops[i - 2].kind(), OpKind::Reserve);
                assert_eq!(plan.keys_of(&ops[i - 1]), plan.keys_of(op));
                assert_eq!(plan.keys_of(&ops[i - 2]), plan.keys_of(op));
            }
        }
    }

    #[test]
    fn the_plan_is_ordered_by_time_then_session() {
        // FR-035. Checked by the plan's own invariant and independently here, so a
        // broken `check_ordered` cannot hide a broken plan.
        let d = description(5, 20, 1, 1);
        let plan = plan_of(&d, 3, 400.0);
        plan.check_ordered().unwrap();
        for w in plan.operations().windows(2) {
            let (a, b) = (&w[0], &w[1]);
            assert!(a.at() <= b.at());
            if a.at() == b.at() {
                assert!(a.session() <= b.session());
            }
        }
    }

    #[test]
    fn per_session_order_is_strict() {
        // Also FR-035: a session's own operations never overlap or reorder, even
        // though different sessions' may interleave freely.
        let d = description(4, 8, 2, 1);
        let plan = plan_of(&d, 4, 300.0);
        let mut last: std::collections::BTreeMap<u64, f64> = Default::default();
        for op in plan.operations() {
            if let Some(previous) = last.get(&op.session()) {
                assert!(
                    *previous <= op.at(),
                    "session {} went backwards from {previous} to {}",
                    op.session(),
                    op.at()
                );
            }
            last.insert(op.session(), op.at());
        }
        assert!(last.len() >= 8);
    }

    #[test]
    fn check_ordered_rejects_a_plan_that_is_out_of_order() {
        // The invariant must be able to fail, or asserting it proves nothing.
        let d = description(3, 4, 1, 1);
        let mut plan = plan_of(&d, 5, 100.0);
        let last = plan.ops.len() - 1;
        plan.ops.swap(0, last);
        let err = plan.check_ordered().unwrap_err().to_string();
        assert!(err.contains("is at"), "unexpected error: {err}");
    }

    #[test]
    fn the_canonical_form_is_byte_identical_at_a_fixed_seed_and_differs_otherwise() {
        // SC-003, and the half that catches a seed that is not wired through.
        let d = description(4, 6, 2, 1);
        let a = plan_of(&d, 11, 200.0).to_canonical_bytes();
        let b = plan_of(&d, 11, 200.0).to_canonical_bytes();
        let c = plan_of(&d, 12, 200.0).to_canonical_bytes();
        assert_eq!(a, b, "the same seed produced different bytes");
        assert_ne!(a, c, "a different seed produced identical bytes");
        assert_eq!(&a[..8], MAGIC);
        assert_eq!(
            u16::from_le_bytes([a[8], a[9]]),
            CANONICAL_VERSION,
            "version is not in the header"
        );
    }

    #[test]
    fn the_canonical_length_is_exactly_what_the_layout_says() {
        // A silent layout drift — a forgotten pad byte, say — would change every
        // recorded digest without changing any behaviour, so the size is pinned.
        let d = description(2, 2, 1, 1);
        let plan = plan_of(&d, 6, 20.0);
        let header = 8 + 2 + 2 + 8;
        let per_op = 8 + 8 + 2 + 2 + 4;
        // Keys are written per operation, so a shared range is written twice — the
        // reference count, not the arena size.
        let expected = header + plan.len() * per_op + plan.key_references() * 8;
        assert_eq!(plan.to_canonical_bytes().len(), expected);
        assert!(
            plan.interned_keys() < plan.key_references(),
            "Check and Load should share a range, so the arena is smaller than the \
             reference count"
        );
    }

    #[test]
    fn a_fingerprint_tracks_the_bytes() {
        let d = description(3, 4, 1, 2);
        let a = plan_of(&d, 21, 100.0);
        let b = plan_of(&d, 21, 100.0);
        let c = plan_of(&d, 22, 100.0);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    #[test]
    fn op_kind_discriminants_match_the_wire_contract() {
        // `contracts/node-agent-wire.md` lists check, touch, load, reserve,
        // transfer, commit, abort, poll-events in that order. If these drift, the
        // agent executes a different operation from the one planned.
        let wire = [0u8, 1, 2, 3, 4, 5, 6, 7];
        for (kind, want) in OpKind::all().iter().zip(wire) {
            assert_eq!(kind.as_u8(), want, "{kind:?} has the wrong wire value");
        }
    }

    #[test]
    fn one_sessions_key_references_are_exactly_the_arithmetic_of_fr_025() {
        // The quadratic figure the projection needs, asserted as a closed form
        // rather than a ratio. With a 4-block shared instance and growth 1 + 1, turn
        // k checks, touches and loads a prefix of 4 + 2k keys and stores 1 + 1
        // through three steps each, so a T-turn session references
        //
        //     sum over k < T of [3*(4 + 2k) + 6]  =  18T + 3T(T - 1)
        //
        // which is 108 at T = 4 and 312 at T = 8.
        let per_session = |turns: u32| -> usize {
            let d = description(turns, 1, 1, 1);
            let plan = plan_of(&d, 31, 4_000.0);
            let mut by_session: std::collections::BTreeMap<u64, usize> = Default::default();
            for op in plan.operations() {
                *by_session.entry(op.session()).or_default() += op.key_count();
            }
            // The seeded session has a residual turn count; a full-length one is the
            // largest.
            *by_session.values().max().expect("at least one session")
        };
        let expect = |t: usize| 18 * t + 3 * t * (t - 1);
        assert_eq!(per_session(4), expect(4), "T = 4");
        assert_eq!(per_session(8), expect(8), "T = 8");
        // Super-linear: doubling the turns more than doubles the references. Linear
        // growth would give exactly 2.0 here, and it gives 2.8.
        let ratio = per_session(8) as f64 / per_session(4) as f64;
        assert!(
            ratio > 2.2,
            "references grew only {ratio:.2}x, which is linear"
        );
    }

    #[test]
    fn a_runs_total_references_are_linear_in_turn_count_not_quadratic() {
        // Recorded because it is the mistake this test replaced. Per *session* the
        // growth is quadratic, but a fixed span holds proportionally fewer long
        // sessions, so the run total is linear — and a projection that assumed
        // otherwise would oversize every estimate.
        let total =
            |turns: u32| plan_of(&description(turns, 1, 1, 1), 31, 4_000.0).key_references();
        let ratio = total(8) as f64 / total(4) as f64;
        assert!(
            (1.1..2.0).contains(&ratio),
            "run total grew {ratio:.2}x when turns doubled; expected roughly linear"
        );
    }

    #[test]
    fn polling_can_be_thinned_without_touching_anything_else() {
        // The knob exists because the client's cadence is an assumption; changing it
        // must affect only the poll count.
        let d = description(6, 3, 1, 1);
        let mut sim = Simulation::new(&d, 41, 1).unwrap();
        let mut every_turn = OperationPlan::default();
        sim.run_until(300.0, &mut |s, t| every_turn.record_turn(s, t));

        let mut sim = Simulation::new(&d, 41, 1).unwrap();
        let mut every_third = OperationPlan::with_options(PlanOptions {
            poll_events_every: 3,
        });
        sim.run_until(300.0, &mut |s, t| every_third.record_turn(s, t));

        let polls = |p: &OperationPlan| {
            p.operations()
                .iter()
                .filter(|o| o.kind() == OpKind::PollEvents)
                .count()
        };
        assert_eq!(every_turn.turns(), every_third.turns());
        assert!(polls(&every_third) < polls(&every_turn));
        // And nothing else moved: the same non-poll operations in the same order.
        let strip = |p: &OperationPlan| -> Vec<(u64, u64, OpKind, usize)> {
            p.operations()
                .iter()
                .filter(|o| o.kind() != OpKind::PollEvents)
                .map(|o| (o.at().to_bits(), o.session(), o.kind(), o.key_count()))
                .collect()
        };
        assert_eq!(strip(&every_turn), strip(&every_third));
    }
}
