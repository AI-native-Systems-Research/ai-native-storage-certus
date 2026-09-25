//! Mapping the plan onto Certus's mailbox: opcodes, payloads, and what is forbidden.
//!
//! This is the one place the plan's abstract operations become the shipped client's
//! actual protocol, so it is where FR-039 through FR-044 either hold or quietly do
//! not. Every opcode and payload layout below was read from
//! `lib/shmq-dispatcher/src/{wire,translate}.rs` rather than inferred.
//!
//! # The plan's kinds are not the mailbox's opcodes
//!
//! Three numbering schemes exist and they are deliberately different:
//!
//! | plan `OpKind` | mailbox opcode | payload |
//! | --- | --- | --- |
//! | `Check` | `CHECK` = 1 | `n:u32`, `key:u64 * n` |
//! | `Touch` | `TOUCH` = 2 | `promote:u8`, `n:u32`, `key:u64 * n` |
//! | `Load` | `LOOKUP` = 9 | `n:u32`, `key:u64 * n` |
//! | `Reserve` | `RESERVE` = 3 | `n:u32`, then `(key:u64, size:u32, session:u64) * n` |
//! | `Transfer` | `COPY_TO_STORE` = 4 | `n:u32`, `key:u64 * n` |
//! | `Commit` | `COMMIT_STORE` = 5 | `n:u32`, `key:u64 * n` |
//! | `Abort` | `ABORT_STORE` = 6 | `n:u32`, `key:u64 * n` |
//! | `PollEvents` | `TAKE_EVENTS` = 10 | `max:u32` |
//!
//! Keeping them separate is what lets each protocol version independently; collapsing
//! them would mean a change to one silently redefining the other.
//!
//! # `promote` is always zero, and that is a requirement not a default
//!
//! `TOUCH` and `LOOKUP` both accept a `promote` flag that makes the dispatcher call
//! `promote_to_memory_tier`. FR-043 forbids requesting promotion and FR-044 forbids
//! supplying the eviction policy with information the production client would not.
//! Promotion is a *decision*, and taking it on the client's behalf would mean measuring
//! a tiering policy this generator had partly written. So the flag is a named constant
//! set to zero, never a parameter.
//!
//! # Four opcodes must never be issued
//!
//! `PIN`, `UNPIN`, `REMOVE` and `CLEAR_MEMORY_TIER` (FR-043). The first three would let
//! the generator hold blocks the policy wanted to evict or evict blocks it wanted to
//! keep — measuring the generator's opinion rather than Certus's. `CLEAR_MEMORY_TIER` is
//! permitted exactly once, at startup, before the timed window opens.
//!
//! # Requests are split to `--batch-keys`, and nothing allocates per operation
//!
//! FR-069 makes the number of keys per request a per-run option, because it is the largest
//! measured performance lever on this path and it belongs to the client's request
//! scheduling rather than to the workload. So an operation with more keys than
//! `--batch-keys` becomes **several requests**, which is what the production client does
//! with a long prefix. [`OpStream::encode_chunk`] takes one chunk; the caller does the
//! chunking, because only the caller knows how to count what it issued.
//!
//! Every encoding borrows: key lists are written into a reused scratch buffer and handle
//! batches into a pre-built template (see [`crate::payload`]), so issuing an operation
//! allocates nothing. That is FR-038 applied to the request as well as to the payload.
//!
//! [`forbidden_opcode`] names them and [`OpStream::encode_chunk`] cannot produce them,
//! since nothing in [`OpKind`] maps to one. The test that matters is the one asserting a whole
//! run's issued opcodes contain none of them, because that is a property of the stream
//! rather than of any single call.

use std::sync::Arc;

use shmq_dispatcher::wire::{self, op};

use crate::payload::{HandleBatchTemplate, PayloadBuffer};
use workload_model::plan::OpKind;

/// `promote` value for `TOUCH` and `LOOKUP`.
///
/// Always zero. See the module docs: promotion is a decision, and taking it would make
/// this instrument measure a tiering policy it had partly authored.
pub const NO_PROMOTE: u8 = 0;

/// Maximum events to drain per `TAKE_EVENTS`.
///
/// Zero means "everything queued", which is what a client polling once per turn wants:
/// a cap would leave events to accumulate and turn a steady cost into a sawtooth.
pub const DRAIN_ALL_EVENTS: u32 = 0;

/// What encoding an operation produced.
///
/// The payload is **borrowed** from the encoder's own reused buffers, so nothing is
/// allocated per operation. It is valid until the next call.
#[derive(Debug, PartialEq, Eq)]
pub enum Encoding<'a> {
    /// Ready to issue.
    Ready {
        /// The mailbox opcode.
        opcode: u32,
        /// The payload, in the layout `translate.rs` expects.
        payload: &'a [u8],
    },
    /// Nothing to issue: only [`OpKind::Abort`], which the planner never emits.
    NotPlanned,
    /// **Needs a GPU IPC handle**, which this build has no payload buffer for.
    ///
    /// `LOOKUP` and `COPY_TO_STORE` do not take key lists. `op_lookup` reads a *handle
    /// batch* — `(key, IpcHandle regions)` pairs — because a load DMAs into a GPU
    /// buffer, and a store copies out of one. Discovered by running against a live
    /// server: reading the opcode table suggested a key list and the server answered
    /// "truncated: need 64 bytes at offset 4, have 32".
    ///
    /// A run that skipped these silently would report a throughput for an operation
    /// stream missing its two data-moving operations, so the driver counts them and the
    /// report says the run was partial. Attaching a [`crate::payload::PayloadBuffer`] with
    /// [`OpStream::with_payload`] makes them `Ready`; without one — no GPU on the node —
    /// they are still counted and declared.
    NeedsGpuPayload {
        /// Which opcode it would have been.
        opcode: u32,
        /// How many keys it would have carried.
        keys: usize,
    },
}

/// Whether an opcode is one this generator must never issue (FR-043).
///
/// `CLEAR_MEMORY_TIER` is included: it is permitted exactly once at startup, which is
/// not part of the operation stream and is issued by its own path, outside the timed
/// window (FR-046).
pub fn forbidden_opcode(opcode: u32) -> Option<&'static str> {
    match opcode {
        op::PIN => Some("PIN: pinning would hold blocks the eviction policy wanted to evict"),
        op::UNPIN => Some("UNPIN: only meaningful after a pin, which is itself forbidden"),
        op::REMOVE => {
            Some("REMOVE: evicting on the policy's behalf measures our opinion, not Certus's")
        }
        op::CLEAR_MEMORY_TIER => {
            Some("CLEAR_MEMORY_TIER: permitted once at startup only, outside the timed window")
        }
        op::POPULATE => Some("POPULATE: not an operation the production client issues"),
        op::FLUSH_TO_SSD => {
            Some("FLUSH_TO_SSD: a tiering decision the production client does not make")
        }
        _ => None,
    }
}

/// How a turn's path divides once the cache has answered.
///
/// # A turn offers its whole path, root to the end of the new growth
///
/// A real prefix-caching client cannot know what it must store. It offers every key from
/// the root of the prefix through the end of the turn's new growth, and then:
///
/// * what came back **resident** it touches and loads — the blocks it does not have to
///   recompute;
/// * what came back **missing** it stores — reserve, transfer, commit.
///
/// So the operations a turn issues are a function of the cache's state, not of the
/// description alone. That is what `spec.md`'s race note describes (two sessions racing to
/// mint the same shared prefix both miss and both store), and it is what lets a block
/// **evicted mid-run be stored again**. With a fixed operation list nothing re-stores an
/// evicted block, so a run's hit rate could only ever decay — and shared prefix blocks,
/// which no turn mints, were never stored at all.
///
/// # `PENDING` is neither loaded nor stored
///
/// `op_check` answers `PENDING` when another lane has reserved the key and its store is in
/// flight. `translate.rs` is explicit that "the client only reserves keys that Check
/// reported absent", so re-storing a pending key would duplicate a store already under way,
/// and loading it would race the writer. It is counted and skipped.
#[derive(Debug, Default, Clone)]
pub struct TurnSplit {
    resident: Vec<u64>,
    missing: Vec<u64>,
    pending: u64,
}

impl TurnSplit {
    /// Divide `path` by the `CHECK` states in `states`, reusing the buffers.
    ///
    /// `states` is one byte per key in `path`, in order — `wire::check_state`. A short
    /// `states` leaves the remaining keys untouched rather than guessing, because guessing
    /// would either invent a hit or invent a store.
    pub fn split(&mut self, path: &[u64], states: &[u8]) {
        self.resident.clear();
        self.missing.clear();
        self.pending = 0;
        for (key, state) in path.iter().zip(states.iter()) {
            match *state {
                wire::check_state::RESIDENT => self.resident.push(*key),
                wire::check_state::PENDING => self.pending += 1,
                _ => self.missing.push(*key),
            }
        }
    }

    /// Divide `path` by a `LOOKUP` response's `ok` bytes, reusing the buffers.
    ///
    /// One byte per key, `1` for a served key and `0` for anything else — `op_lookup` sets
    /// the flag only on success and counts `KeyNotFound` as a miss, so a zero is "not
    /// served, store it". There is no `PENDING` equivalent here: a key another lane is
    /// storing reads as a plain miss, so this mode can re-reserve it and have the reserve
    /// declined, which the granted-keys filter already handles.
    ///
    /// A short `ok` list leaves the remaining keys untouched, for the same reason
    /// [`Self::split`] does: guessing would either invent a hit or invent a store.
    pub fn split_by_lookup(&mut self, path: &[u64], ok: &[u8]) {
        self.resident.clear();
        self.missing.clear();
        self.pending = 0;
        for (key, served) in path.iter().zip(ok.iter()) {
            if *served == 1 {
                self.resident.push(*key);
            } else {
                self.missing.push(*key);
            }
        }
    }

    /// Keys to touch and load.
    pub fn resident(&self) -> &[u64] {
        &self.resident
    }

    /// Keys to reserve, transfer and commit.
    pub fn missing(&self) -> &[u64] {
        &self.missing
    }

    /// Keys another lane is already storing.
    pub fn pending(&self) -> u64 {
        self.pending
    }
}

/// Encodes plan operations as mailbox requests.
///
/// Holds the reused buffers, so an encoder is per-lane and never shared.
#[derive(Debug)]
pub struct OpStream {
    block_bytes: u32,
    batch_keys: usize,
    scratch: Vec<u8>,
    payload: Option<PayloadTarget>,
}

/// This lane's pre-built request template, and the device buffer it names.
///
/// The buffer is optional because **the wire protocol does not depend on CUDA** — a
/// handle batch is bytes, and [`HandleBatchTemplate`] builds them from a handle it is
/// given. Only stamping touches the device. That split is what lets the operation stream
/// be tested against a mock mailbox on a machine with no accelerator (T051), which is
/// where protocol mistakes are cheap to find.
#[derive(Debug)]
struct PayloadTarget {
    buffer: Option<Arc<PayloadBuffer>>,
    slot: usize,
    template: HandleBatchTemplate,
}

impl OpStream {
    /// An encoder for a description's block geometry and a run's request size.
    ///
    /// `block_bytes` is what `RESERVE` asks for per key — the cache holds bytes, and a
    /// reservation in the wrong unit would silently size the cache wrongly. `batch_keys`
    /// is FR-069's per-run option, and bounds one request's key count.
    ///
    /// Without a payload buffer, `LOOKUP` and `COPY_TO_STORE` report
    /// [`Encoding::NeedsGpuPayload`] rather than being issued; see
    /// [`OpStream::with_payload`].
    pub fn new(block_bytes: u32, batch_keys: usize) -> Self {
        assert!(batch_keys > 0, "a request must carry at least one key");
        Self {
            block_bytes,
            batch_keys,
            // Sized for the largest key list this encoder can produce, so the scratch
            // buffer never reallocates mid-run.
            scratch: Vec::with_capacity(4 + batch_keys * 20),
            payload: None,
        }
    }

    /// Attach the GPU payload buffer, making the two data-moving operations issuable.
    ///
    /// `slot` is this lane's disjoint region; see [`crate::payload`] on why that is a
    /// correctness requirement rather than tidiness.
    pub fn with_payload(self, buffer: Arc<PayloadBuffer>, slot: usize) -> Self {
        let template = buffer.template(slot);
        self.with_template(template, Some(buffer), slot)
    }

    /// Attach a pre-built request template, with or without a device buffer behind it.
    ///
    /// Without a buffer the requests are still exactly what the mailbox expects — the
    /// protocol is bytes — but no memory backs the handle, so this is for testing the
    /// operation stream rather than for a run that moves data.
    pub fn with_template(
        mut self,
        template: HandleBatchTemplate,
        buffer: Option<Arc<PayloadBuffer>>,
        slot: usize,
    ) -> Self {
        assert!(
            template.max_keys() >= self.batch_keys,
            "the payload template holds {} keys but requests carry up to {}",
            template.max_keys(),
            self.batch_keys
        );
        self.payload = Some(PayloadTarget {
            buffer,
            slot,
            template,
        });
        self
    }

    /// Keys per request, FR-069's option.
    pub fn batch_keys(&self) -> usize {
        self.batch_keys
    }

    /// Whether the data-moving operations can be issued.
    pub fn can_move_data(&self) -> bool {
        self.payload.is_some()
    }

    /// The device buffer behind this lane's template, if there is one.
    ///
    /// Exposed so the executor can read a stamp back after a load: only the buffer knows where
    /// a block lives.
    pub fn payload_buffer(&self) -> Option<&PayloadBuffer> {
        self.payload.as_ref().and_then(|t| t.buffer.as_deref())
    }

    /// This lane's slot within the payload buffer.
    pub fn payload_slot(&self) -> usize {
        self.payload.as_ref().map_or(0, |t| t.slot)
    }

    /// Encode one chunk of one operation's keys.
    ///
    /// The caller splits an operation's keys into chunks of at most
    /// [`OpStream::batch_keys`] — see the module docs on FR-069 — and passes each in turn.
    /// An operation with no keys ([`OpKind::PollEvents`]) is passed an empty slice and
    /// still produces one request.
    ///
    /// # Errors
    ///
    /// If a key stamp fails, which only happens when stamping is enabled.
    ///
    /// # Panics
    ///
    /// If `keys` is longer than [`OpStream::batch_keys`]: the caller is responsible for
    /// splitting, and truncating here would drop key references the caller has already
    /// counted as issued.
    pub fn encode_chunk(
        &mut self,
        kind: OpKind,
        keys: &[u64],
        session: u64,
    ) -> Result<Encoding<'_>, String> {
        assert!(
            keys.len() <= self.batch_keys,
            "chunk of {} keys exceeds --batch-keys {}; the caller must split",
            keys.len(),
            self.batch_keys
        );
        // The two data-moving operations take a handle batch, not a key list.
        if matches!(kind, OpKind::Load | OpKind::Transfer) {
            let opcode = if kind == OpKind::Load {
                op::LOOKUP
            } else {
                op::COPY_TO_STORE
            };
            let Some(target) = self.payload.as_mut() else {
                return Ok(Encoding::NeedsGpuPayload {
                    opcode,
                    keys: keys.len(),
                });
            };
            // A store reads out of our buffer, so its blocks are what a stamp identifies.
            // A load overwrites them, so stamping before one would be wasted work.
            if kind == OpKind::Transfer {
                if let Some(buffer) = target.buffer.as_ref() {
                    if buffer.stamping() {
                        for (i, key) in keys.iter().enumerate() {
                            buffer.stamp_key(target.slot, i, *key)?;
                        }
                    }
                }
            }
            return Ok(Encoding::Ready {
                opcode,
                payload: target.template.stamp_keys(keys),
            });
        }

        let opcode = match kind {
            OpKind::Check => op::CHECK,
            OpKind::Touch => op::TOUCH,
            OpKind::Reserve => op::RESERVE,
            OpKind::Commit => op::COMMIT_STORE,
            OpKind::Abort => return Ok(Encoding::NotPlanned),
            OpKind::PollEvents => op::TAKE_EVENTS,
            OpKind::Load | OpKind::Transfer => unreachable!("handled above"),
        };
        self.scratch.clear();
        match kind {
            OpKind::Check | OpKind::Commit => write_keys(&mut self.scratch, keys),
            OpKind::Touch => {
                self.scratch.push(NO_PROMOTE);
                write_keys(&mut self.scratch, keys);
            }
            OpKind::Reserve => write_reserve(&mut self.scratch, keys, self.block_bytes, session),
            OpKind::PollEvents => self
                .scratch
                .extend_from_slice(&DRAIN_ALL_EVENTS.to_le_bytes()),
            OpKind::Load | OpKind::Transfer | OpKind::Abort => unreachable!("handled above"),
        }
        Ok(Encoding::Ready {
            opcode,
            payload: &self.scratch,
        })
    }

    /// Encode the abort branch of a store, which only the executor can choose.
    pub fn encode_abort(&mut self, keys: &[u64]) -> Encoding<'_> {
        self.scratch.clear();
        write_keys(&mut self.scratch, keys);
        Encoding::Ready {
            opcode: op::ABORT_STORE,
            payload: &self.scratch,
        }
    }
}

fn write_keys(out: &mut Vec<u8>, keys: &[u64]) {
    out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    for k in keys {
        out.extend_from_slice(&k.to_le_bytes());
    }
}

fn write_reserve(out: &mut Vec<u8>, keys: &[u64], size: u32, session: u64) {
    out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    for k in keys {
        out.extend_from_slice(&k.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&session.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_split_has_no_pending_class_and_stores_every_zero() {
        // `op_lookup`'s answer is binary: 1 is served, anything else is "store it". There is
        // deliberately no PENDING equivalent — a key another lane is storing reads as a plain
        // miss here, gets re-reserved, and the reserve declines it, which the granted-keys
        // filter already handles. Treating a zero as anything but a store would silently drop
        // the block.
        let mut split = TurnSplit::default();
        split.split_by_lookup(&[10, 11, 12, 13], &[1, 0, 1, 0]);
        assert_eq!(split.resident(), &[10, 12]);
        assert_eq!(split.missing(), &[11, 13]);
        assert_eq!(split.pending(), 0);

        // Only 1 means served: a byte that is neither 0 nor 1 must not be read as a hit, or a
        // key that was never delivered would go unstored and unloaded.
        split.split_by_lookup(&[20, 21], &[2, 1]);
        assert_eq!(split.resident(), &[21]);
        assert_eq!(split.missing(), &[20]);

        // A short answer leaves the rest alone, exactly as `split` does: guessing would
        // either invent a hit or invent a store.
        split.split_by_lookup(&[30, 31, 32], &[1]);
        assert_eq!(split.resident(), &[30]);
        assert!(split.missing().is_empty());
    }
    use shmq_dispatcher::wire;
    use workload_model::description::WorkloadDescription;
    use workload_model::plan::{Operation, OperationPlan};
    use workload_model::sim::Simulation;

    /// Encode every operation of a plan, one chunk each, collecting `(kind, opcode)`.
    fn encode_all(plan: &OperationPlan, s: &mut OpStream) -> Vec<(OpKind, u32)> {
        let mut seen = Vec::new();
        for op in plan.operations() {
            let keys = plan.keys_of(op);
            for chunk in chunks_of(keys, s.batch_keys()) {
                match s.encode_chunk(op.kind(), chunk, op.session()).unwrap() {
                    Encoding::Ready { opcode, .. } => seen.push((op.kind(), opcode)),
                    Encoding::NeedsGpuPayload { opcode, .. } => seen.push((op.kind(), opcode)),
                    Encoding::NotPlanned => {}
                }
            }
        }
        seen
    }

    /// Chunk a key list the way the driver does, yielding one empty chunk for a keyless
    /// operation so that a poll still produces a request.
    fn chunks_of(keys: &[u64], n: usize) -> Vec<&[u64]> {
        if keys.is_empty() {
            return vec![&[]];
        }
        keys.chunks(n).collect()
    }

    /// The payload bytes for one operation of a given kind, or `None` if there is none.
    fn payload_of(plan: &OperationPlan, s: &mut OpStream, kind: OpKind) -> Option<Vec<u8>> {
        let op: &Operation = plan.operations().iter().find(|o| o.kind() == kind)?;
        let keys = plan.keys_of(op);
        let chunk = chunks_of(keys, s.batch_keys())[0];
        match s.encode_chunk(kind, chunk, op.session()).unwrap() {
            Encoding::Ready { payload, .. } => Some(payload.to_vec()),
            _ => None,
        }
    }

    fn plan_of(seed: u64, span: f64) -> OperationPlan {
        let yaml = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 3}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 3}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 3}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 5}
"#;
        let d: WorkloadDescription = yaml.parse().unwrap();
        let mut sim = Simulation::new(&d, seed, 1).unwrap();
        let mut plan = OperationPlan::default();
        sim.run_until(span, &mut |s, t| plan.record_turn(s, t));
        plan
    }

    #[test]
    fn a_turn_stores_what_missed_and_loads_what_was_resident() {
        // The reactive rule, from `wire::check_state`: RESIDENT is loadable, MISS must be
        // stored, PENDING is another lane's store in flight and is neither.
        use shmq_dispatcher::wire::check_state::{MISS, PENDING, RESIDENT};
        let path = [10u64, 11, 12, 13, 14];
        let states = [RESIDENT, MISS, PENDING, MISS, RESIDENT];
        let mut split = TurnSplit::default();
        split.split(&path, &states);
        assert_eq!(split.resident(), &[10, 14]);
        assert_eq!(split.missing(), &[11, 13]);
        assert_eq!(split.pending(), 1);
        // Every key is accounted for exactly once: a key both loaded and stored would be a
        // wasted store, and a key in neither would be silently dropped from the workload.
        assert_eq!(
            split.resident().len() + split.missing().len() + split.pending() as usize,
            path.len()
        );
    }

    #[test]
    fn a_pending_key_is_not_re_stored_because_another_lane_is_already_storing_it() {
        // `translate.rs`: "the client only reserves keys that Check reported absent".
        // Re-storing would duplicate a store already under way; loading would race the
        // writer. This is the case a two-valued reading of CHECK gets wrong.
        use shmq_dispatcher::wire::check_state::PENDING;
        let mut split = TurnSplit::default();
        split.split(&[7, 8], &[PENDING, PENDING]);
        assert!(
            split.missing().is_empty(),
            "a pending key must not be re-stored"
        );
        assert!(
            split.resident().is_empty(),
            "a pending key must not be loaded"
        );
        assert_eq!(split.pending(), 2);
    }

    #[test]
    fn a_cold_cache_stores_the_whole_path_including_the_shared_root() {
        // The defect this design fixes. Shared prefix blocks sit at the root of the path
        // and no turn mints them, so under a fixed operation list nothing ever stored them
        // and cross-session sharing produced no hits at all. Offering the whole path means
        // the first turn to miss on them stores them.
        use shmq_dispatcher::wire::check_state::MISS;
        let path = [1u64, 2, 3, 4];
        let mut split = TurnSplit::default();
        split.split(&path, &[MISS; 4]);
        assert_eq!(split.missing(), &path, "a cold path must be stored entire");
        assert!(split.resident().is_empty());
    }

    #[test]
    fn a_short_answer_leaves_the_rest_alone_rather_than_guessing() {
        // A truncated response must not invent a hit (which would skip a needed store) or
        // invent a miss (which would store a block the cache already has).
        use shmq_dispatcher::wire::check_state::RESIDENT;
        let mut split = TurnSplit::default();
        split.split(&[1, 2, 3], &[RESIDENT]);
        assert_eq!(split.resident(), &[1]);
        assert!(split.missing().is_empty());
        assert_eq!(split.pending(), 0);
    }

    #[test]
    fn the_split_buffers_are_reused_without_leaking_the_previous_turn() {
        use shmq_dispatcher::wire::check_state::{MISS, RESIDENT};
        let mut split = TurnSplit::default();
        split.split(&[1, 2, 3], &[MISS, MISS, MISS]);
        assert_eq!(split.missing().len(), 3);
        split.split(&[9], &[RESIDENT]);
        assert_eq!(split.missing().len(), 0, "stale misses survived");
        assert_eq!(split.resident(), &[9]);
    }

    #[test]
    fn every_kind_maps_to_the_opcode_read_from_the_dispatcher() {
        // The mapping is the whole point of this module, and a wrong entry would issue
        // a valid request that does the wrong thing — the dispatcher would accept it.
        let plan = plan_of(1, 40.0);
        let mut s = OpStream::new(32768, 64);
        let seen: std::collections::BTreeMap<OpKind, u32> =
            encode_all(&plan, &mut s).into_iter().collect();
        assert_eq!(seen[&OpKind::Check], op::CHECK);
        assert_eq!(seen[&OpKind::Touch], op::TOUCH);
        assert_eq!(seen[&OpKind::Load], op::LOOKUP);
        assert_eq!(seen[&OpKind::Reserve], op::RESERVE);
        assert_eq!(seen[&OpKind::Transfer], op::COPY_TO_STORE);
        assert_eq!(seen[&OpKind::Commit], op::COMMIT_STORE);
        assert_eq!(seen[&OpKind::PollEvents], op::TAKE_EVENTS);
        assert!(!seen.contains_key(&OpKind::Abort), "an abort was planned");
    }

    #[test]
    fn touch_never_requests_promotion() {
        // FR-043 and FR-044. `promote` is the first byte of a TOUCH payload, so this
        // reads the wire rather than trusting the constant.
        let plan = plan_of(2, 40.0);
        let mut s = OpStream::new(32768, 64);
        let payload = payload_of(&plan, &mut s, OpKind::Touch).expect("a touch");
        assert_eq!(payload[0], 0, "TOUCH asked for promotion");
    }

    #[test]
    fn a_whole_run_issues_no_forbidden_opcode() {
        // FR-043 as a property of the stream rather than of any one call. Checked
        // against the dispatcher's own opcode constants, so adding a forbidden
        // operation upstream cannot slip past by renaming.
        let plan = plan_of(3, 200.0);
        let mut s = OpStream::new(32768, 64);
        let issued = encode_all(&plan, &mut s);
        for (kind, opcode) in &issued {
            if let Some(why) = forbidden_opcode(*opcode) {
                panic!("issued a forbidden opcode {opcode} for {kind:?}: {why}");
            }
        }
        assert!(issued.len() > 50, "only {} operations", issued.len());
    }

    #[test]
    fn the_forbidden_list_names_every_opcode_it_should() {
        // A guard list that quietly lost an entry would be worse than none, so each is
        // asserted by its dispatcher constant.
        for opcode in [
            op::PIN,
            op::UNPIN,
            op::REMOVE,
            op::CLEAR_MEMORY_TIER,
            op::POPULATE,
            op::FLUSH_TO_SSD,
        ] {
            assert!(
                forbidden_opcode(opcode).is_some(),
                "opcode {opcode} is not on the forbidden list"
            );
        }
        // And the ones we do issue must not be on it.
        for opcode in [
            op::CHECK,
            op::TOUCH,
            op::RESERVE,
            op::COPY_TO_STORE,
            op::COMMIT_STORE,
            op::ABORT_STORE,
            op::LOOKUP,
            op::TAKE_EVENTS,
        ] {
            assert!(
                forbidden_opcode(opcode).is_none(),
                "opcode {opcode} is issued but marked forbidden"
            );
        }
    }

    #[test]
    fn a_key_payload_round_trips_through_the_dispatchers_own_reader() {
        // The encodings are only right if the dispatcher's reader agrees, so this
        // decodes with `wire::Reader` rather than with a hand-written parser.
        let plan = plan_of(1, 40.0);
        let mut s = OpStream::new(32768, 64);
        let op_ref = plan
            .operations()
            .iter()
            .find(|o| o.kind() == OpKind::Check)
            .expect("a check");
        let expected = plan.keys_of(op_ref).to_vec();
        let payload = payload_of(&plan, &mut s, OpKind::Check).expect("a check");
        let mut r = wire::Reader::new(&payload);
        assert_eq!(r.u32().unwrap() as usize, expected.len().min(64));
        for want in expected.iter().take(64) {
            assert_eq!(r.u64().unwrap(), *want);
        }
    }

    #[test]
    fn a_reserve_payload_carries_size_and_session_per_key() {
        // The layout `op_reserve` reads: n, then (key, size, session) per entry. A
        // reservation in the wrong unit would size the cache wrongly and nothing would
        // report it.
        let plan = plan_of(4, 40.0);
        let mut s = OpStream::new(32768, 64);
        let op_ref = plan
            .operations()
            .iter()
            .find(|o| o.kind() == OpKind::Reserve)
            .expect("a reserve");
        let session = op_ref.session();
        let expected: Vec<u64> = plan.keys_of(op_ref).iter().copied().take(64).collect();
        let payload = payload_of(&plan, &mut s, OpKind::Reserve).expect("a reserve");
        let mut r = wire::Reader::new(&payload);
        assert_eq!(r.u32().unwrap() as usize, expected.len());
        for want in expected {
            assert_eq!(r.u64().unwrap(), want);
            assert_eq!(r.u32().unwrap(), 32768, "size must be bytes per block");
            assert_eq!(r.u64().unwrap(), session, "session must be the turn's");
        }
    }

    #[test]
    fn a_poll_drains_every_queued_event_and_still_issues_with_no_keys() {
        // A cap would let events accumulate and turn a steady per-turn cost into a
        // sawtooth. And a poll carries no keys, so the chunking must still produce one
        // request for it — `[].chunks(n)` yields nothing, which would silently drop
        // every poll from the stream.
        let plan = plan_of(5, 40.0);
        let mut s = OpStream::new(32768, 64);
        let payload = payload_of(&plan, &mut s, OpKind::PollEvents).expect("a poll");
        let mut r = wire::Reader::new(&payload);
        assert_eq!(r.u32().unwrap(), DRAIN_ALL_EVENTS);
        let polls = encode_all(&plan, &mut s)
            .iter()
            .filter(|(k, _)| *k == OpKind::PollEvents)
            .count();
        assert!(polls > 0, "chunking dropped every poll");
    }

    #[test]
    fn the_two_data_moving_operations_need_a_gpu_and_say_so_without_one() {
        // Found by running against a live server, which answered LOOKUP with
        // "truncated: need 64 bytes at offset 4, have 32": `op_lookup` reads a handle
        // batch, not a key list. Counted rather than skipped, so a run cannot report a
        // throughput for a stream missing its two data-moving operations.
        let plan = plan_of(7, 40.0);
        let mut s = OpStream::new(32768, 64);
        assert!(!s.can_move_data(), "no buffer was attached");
        let mut needs = 0;
        for op in plan.operations() {
            let keys = plan.keys_of(op);
            for chunk in chunks_of(keys, 64) {
                if let Encoding::NeedsGpuPayload { opcode, keys } =
                    s.encode_chunk(op.kind(), chunk, op.session()).unwrap()
                {
                    assert!(opcode == op::LOOKUP || opcode == op::COPY_TO_STORE);
                    assert!(keys > 0);
                    needs += 1;
                }
            }
        }
        assert!(needs > 0, "no operation reported needing a GPU");
    }

    #[test]
    fn an_operation_longer_than_batch_keys_becomes_several_requests() {
        // FR-069: keys per request is a per-run option, and a long prefix is what makes
        // it bite. Before this was wired the option was accepted, reported in the run's
        // own parameters, and had no effect at all.
        let plan = plan_of(9, 400.0);
        let longest = plan
            .operations()
            .iter()
            .map(|o| plan.keys_of(o).len())
            .max()
            .unwrap_or(0);
        assert!(
            longest > 4,
            "the plan has no operation long enough to split"
        );
        let mut wide = OpStream::new(32768, 4096);
        let mut narrow = OpStream::new(32768, 4);
        let wide_n = encode_all(&plan, &mut wide).len();
        let narrow_n = encode_all(&plan, &mut narrow).len();
        assert!(
            narrow_n > wide_n,
            "a smaller --batch-keys must produce more requests ({narrow_n} vs {wide_n})"
        );
        // And every chunk must be within the limit, since the template is sized by it.
        for op in plan.operations() {
            for chunk in chunks_of(plan.keys_of(op), 4) {
                assert!(chunk.len() <= 4);
            }
        }
    }

    #[test]
    fn the_same_keys_encode_identically_whatever_the_batch_size_of_the_previous_call() {
        // The scratch buffer is reused, so a long request followed by a short one must
        // not leave the long one's tail visible. `n` bounds the read, but the returned
        // slice must agree with it.
        let mut s = OpStream::new(4096, 64);
        let long = s.encode_chunk(OpKind::Check, &[1, 2, 3, 4, 5], 0).unwrap();
        let Encoding::Ready { payload, .. } = long else {
            panic!("a check must be ready")
        };
        let long_len = payload.len();
        let short = s.encode_chunk(OpKind::Check, &[9], 0).unwrap();
        let Encoding::Ready { payload, .. } = short else {
            panic!("a check must be ready")
        };
        assert!(payload.len() < long_len, "stale bytes remained");
        let mut r = wire::Reader::new(payload);
        assert_eq!(r.u32().unwrap(), 1);
        assert_eq!(r.u64().unwrap(), 9);
    }

    #[test]
    fn abort_is_encodable_but_never_planned() {
        // The executor needs it for a failed commit; the planner must not emit it,
        // because which branch is taken is a run-time outcome.
        let plan = plan_of(6, 100.0);
        let mut s = OpStream::new(32768, 64);
        assert!(plan.operations().iter().all(|o| o.kind() != OpKind::Abort));
        let Encoding::Ready { opcode, payload } = s.encode_abort(&[1, 2]) else {
            panic!("an abort must be ready")
        };
        assert_eq!(opcode, op::ABORT_STORE);
        let mut r = wire::Reader::new(payload);
        assert_eq!(r.u32().unwrap(), 2);
    }
}
