//! The reactive rule, in **one** place: offer a turn's path, load what is resident, store
//! what is absent.
//!
//! # Why this is a module and not a loop inside the local path
//!
//! Two execution paths reach a Certus mailbox: the generator's own on a local node, and the
//! node agent's on a remote one. FR-072 promises that the same description and seed produce
//! the same workload whichever path runs it, and that promise is only checkable if there is
//! one implementation of the rule to check. Two would be free to drift — a fix applied to
//! the local path and forgotten on the agent would show up as a local/remote difference
//! that looked like a property of the network.
//!
//! So the agent depends on this crate rather than reimplementing the sequence. That
//! dependency arrow is the wrong way round for a daemon, and it is accepted deliberately:
//! see `contracts/node-agent-wire.md`.
//!
//! # The sequence, and why it is this and not something shorter
//!
//! Per turn, against the local mailbox:
//!
//! 1. `CHECK` the whole path, root of the prefix through the end of the new growth.
//! 2. `TOUCH` then `LOOKUP` whatever came back **resident**. Touch is the reference report
//!    (FR-041) and is a separate call from the check in the shipped client; omitting it
//!    would score every recency policy against a workload in which nothing is ever recently
//!    used.
//! 3. `RESERVE` whatever came back **absent**, and keep its per-key answer.
//! 4. `COPY_TO_STORE` then `COMMIT_STORE` only the keys the reserve **granted**. A transfer
//!    DMAs a block into the slot a reservation produced, so writing for a declined key
//!    sends a payload nowhere and then fails its commit for want of a pending write.
//!
//! A `PENDING` key is neither loaded nor stored: another lane's store is in flight, and
//! `translate.rs` is explicit that the client only reserves what the check reported absent.
//!
//! # This is the only place latency and bandwidth are measured
//!
//! Whatever talks to the mailbox is the only thing that can time a `LOOKUP` or count a block
//! that moved, so the counters and the per-operation histograms are gathered here and
//! nowhere else. Because both paths run this code, a local figure and a remote figure mean
//! the same thing — had each path counted for itself, a difference between them would be
//! unattributable between the cache and the instrument.
//!
//! Timing is **per request, never per key**: a batch of 64 keys is one round trip, so a
//! per-key clock would measure the same interval 64 times and put instrumentation on the
//! per-key path (FR-038, FR-070).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use shm_queue::Client;
use shmq_dispatcher::wire::{self, op};
use workload_model::plan::OpKind;
use workload_wire::frame::Counters;

use crate::opstream::{forbidden_opcode, Encoding, OpStream, TurnSplit};
use crate::payload::{HandleBatchTemplate, PayloadBuffer};

/// Spins before a response poll sleeps.
pub const SPIN_ITERS: u32 = 20_000;

/// Per-attempt wait before retrying a response poll.
pub const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(50);

/// Overall deadline for one request.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(30);

/// Largest latency the histogram records, in microseconds.
const LATENCY_CEILING_US: u64 = 60_000_000;

/// What one turn did, as the wire reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnResult {
    /// Path keys the cache held.
    pub resident: u32,
    /// Path keys another lane was already storing.
    pub pending: u32,
    /// Path keys absent, and so offered for storing.
    pub missing: u32,
    /// Of those, the ones `RESERVE` granted.
    pub granted: u32,
    /// Blocks whose payload came from the cache.
    pub blocks_read: u32,
    /// Blocks whose payload went to it.
    pub blocks_written: u32,
    /// Operations skipped for want of a GPU payload buffer.
    pub skipped_needing_gpu: u32,
    /// Keys those skipped operations would have moved.
    pub skipped_keys: u32,
}

/// Runs turns against one mailbox channel, and measures what happened.
///
/// One per lane: it owns an [`OpStream`] with a per-lane payload slot, and reusable buffers
/// so a turn allocates nothing.
#[derive(Debug)]
pub struct TurnExecutor {
    stream: OpStream,
    split: TurnSplit,
    counters: Counters,
    latency: Histogram<u64>,
    by_op: BTreeMap<u32, Histogram<u64>>,
    batch_keys: usize,
    block_bytes: u64,
    states: Vec<u8>,
    granted: Vec<u8>,
    stored: Vec<u64>,
    skipped_needing_gpu: u64,
    skipped_keys: u64,
}

impl TurnExecutor {
    /// An executor for one lane.
    ///
    /// # Errors
    ///
    /// If the latency histogram cannot be built.
    pub fn new(block_bytes: u32, batch_keys: usize) -> Result<Self, String> {
        Ok(Self {
            stream: OpStream::new(block_bytes, batch_keys),
            split: TurnSplit::default(),
            counters: Counters::default(),
            latency: Histogram::<u64>::new_with_bounds(1, LATENCY_CEILING_US, 3)
                .map_err(|e| format!("cannot build the latency histogram: {e}"))?,
            by_op: BTreeMap::new(),
            batch_keys,
            block_bytes: u64::from(block_bytes),
            states: Vec::new(),
            granted: Vec::new(),
            stored: Vec::new(),
            skipped_needing_gpu: 0,
            skipped_keys: 0,
        })
    }

    /// Attach the GPU payload buffer, making the data-moving operations issuable.
    pub fn with_payload(mut self, buffer: Arc<PayloadBuffer>, slot: usize) -> Self {
        self.stream = self.stream.with_payload(buffer, slot);
        self
    }

    /// Attach a request template with no device behind it, for testing the stream.
    pub fn with_template(
        mut self,
        template: HandleBatchTemplate,
        buffer: Option<Arc<PayloadBuffer>>,
        slot: usize,
    ) -> Self {
        self.stream = self.stream.with_template(template, buffer, slot);
        self
    }

    /// What the mailbox-facing code counted. Reporting only.
    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// Latency across every opcode together.
    pub fn latency(&self) -> &Histogram<u64> {
        &self.latency
    }

    /// Latency per opcode, because an aggregate describes the operation mix (FR-066a).
    pub fn latency_by_op(&self) -> &BTreeMap<u32, Histogram<u64>> {
        &self.by_op
    }

    /// Operations skipped for want of a GPU payload buffer, and the keys they would have moved.
    pub fn skipped(&self) -> (u64, u64) {
        (self.skipped_needing_gpu, self.skipped_keys)
    }

    /// Whether the data-moving operations can be issued at all.
    pub fn can_move_data(&self) -> bool {
        self.stream.can_move_data()
    }

    /// Run one turn: offer `path`, load what is resident, store what is absent.
    ///
    /// # Errors
    ///
    /// If a request fails, returns a non-OK status, or names a forbidden opcode.
    pub fn run_turn(
        &mut self,
        client: &Client,
        channel: usize,
        session: u64,
        path: &[u64],
        poll_events: bool,
    ) -> Result<TurnResult, String> {
        let before = self.counters;
        let skipped_before = (self.skipped_needing_gpu, self.skipped_keys);

        self.states.clear();
        let mut states = std::mem::take(&mut self.states);
        let check = self.issue(client, channel, op::CHECK, path, 0, Some(&mut states));
        self.states = states;
        check?;
        // Taken apart so the split does not borrow `self` while `issue` needs it mutably.
        let mut split = std::mem::take(&mut self.split);
        split.split(path, &self.states);

        for opcode in [op::TOUCH, op::LOOKUP] {
            let resident = split.resident().to_vec();
            self.issue(client, channel, opcode, &resident, 0, None)?;
        }

        let missing = split.missing().to_vec();
        let mut granted = std::mem::take(&mut self.granted);
        granted.clear();
        let reserve = self.issue(
            client,
            channel,
            op::RESERVE,
            &missing,
            session,
            Some(&mut granted),
        );
        self.granted = granted;
        reserve?;

        let mut stored = std::mem::take(&mut self.stored);
        granted_keys(&missing, &self.granted, &mut stored);
        let granted_count = stored.len() as u32;
        let mut store_result = Ok(());
        for opcode in [op::COPY_TO_STORE, op::COMMIT_STORE] {
            store_result = self.issue(client, channel, opcode, &stored, session, None);
            if store_result.is_err() {
                break;
            }
        }
        // Put the buffer back whether or not the store succeeded, so a failing turn does not
        // leave the executor without its reusable allocations.
        self.stored = stored;
        store_result?;

        self.counters.check_pending += split.pending();
        let pending = split.pending() as u32;
        let resident = split.resident().len() as u32;
        let missing_count = missing.len() as u32;
        self.split = split;

        // The poll is not keyed, and how often it happens is the plan's decision.
        if poll_events {
            self.issue(client, channel, op::TAKE_EVENTS, &[], 0, None)?;
        }

        let moved = self.counters;
        Ok(TurnResult {
            resident,
            pending,
            missing: missing_count,
            granted: granted_count,
            blocks_read: (moved.lookup_hits - before.lookup_hits) as u32,
            blocks_written: ((moved.transfers_attempted - before.transfers_attempted)
                .saturating_sub(moved.transfers_declined - before.transfers_declined))
                as u32,
            skipped_needing_gpu: (self.skipped_needing_gpu - skipped_before.0) as u32,
            skipped_keys: (self.skipped_keys - skipped_before.1) as u32,
        })
    }

    /// Payload bytes read and written so far.
    ///
    /// A block is read on a `LOOKUP` hit and written on an accepted `COPY_TO_STORE`, and no
    /// control operation moves a byte — which is why bandwidth can only come from these
    /// counters (FR-066).
    pub fn bytes(&self) -> (u64, u64) {
        let written = self
            .counters
            .transfers_attempted
            .saturating_sub(self.counters.transfers_declined);
        (
            self.counters.lookup_hits * self.block_bytes,
            written * self.block_bytes,
        )
    }

    /// Issue one opcode over `keys`, split into `--batch-keys` requests.
    ///
    /// `collect` gathers the per-key response bytes in order across every chunk, which is
    /// what makes a `CHECK` answer usable as a decision over the whole path rather than per
    /// chunk. An empty `keys` issues nothing unless the opcode is keyless.
    fn issue(
        &mut self,
        client: &Client,
        channel: usize,
        opcode: u32,
        keys: &[u64],
        session: u64,
        mut collect: Option<&mut Vec<u8>>,
    ) -> Result<(), String> {
        if let Some(why) = forbidden_opcode(opcode) {
            return Err(format!("refusing to issue a forbidden operation: {why}"));
        }
        let keyless = opcode == op::TAKE_EVENTS;
        if keys.is_empty() && !keyless {
            return Ok(());
        }
        let kind = kind_of(opcode);
        let mut start = 0usize;
        loop {
            let end = (start + self.batch_keys).min(keys.len());
            let chunk = &keys[start..end];
            let (op_code, payload) = match self.stream.encode_chunk(kind, chunk, session)? {
                Encoding::Ready { opcode, payload } => (opcode, payload),
                Encoding::NotPlanned => break,
                Encoding::NeedsGpuPayload { keys: n, .. } => {
                    // No payload buffer: counted and declared, never silently dropped.
                    self.skipped_needing_gpu += 1;
                    self.skipped_keys += n as u64;
                    start = end;
                    if start >= keys.len() {
                        break;
                    }
                    continue;
                }
            };
            let started = Instant::now();
            let (status, body) = client
                .request(
                    channel,
                    op_code,
                    payload,
                    SPIN_ITERS,
                    ATTEMPT_TIMEOUT,
                    REQUEST_DEADLINE,
                )
                .map_err(|e| format!("shmq request (op {op_code}) failed: {e}"))?;
            if status != wire::STATUS_OK {
                return Err(format!(
                    "op {op_code} returned an error status: {}",
                    String::from_utf8_lossy(&body)
                ));
            }
            let micros = started
                .elapsed()
                .as_micros()
                .clamp(1, LATENCY_CEILING_US as u128) as u64;
            self.record(op_code, micros)?;
            self.counters.requests += 1;
            self.counters.key_references += chunk.len() as u64;
            count_results(op_code, &body, &mut self.counters);
            if let Some(out) = collect.as_deref_mut() {
                out.extend_from_slice(&body);
            }
            start = end;
            if start >= keys.len() {
                break;
            }
        }
        Ok(())
    }

    fn record(&mut self, opcode: u32, micros: u64) -> Result<(), String> {
        self.latency
            .record(micros)
            .map_err(|e| format!("latency out of histogram range: {e}"))?;
        let entry = match self.by_op.entry(opcode) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                let h = Histogram::<u64>::new_with_bounds(1, LATENCY_CEILING_US, 3)
                    .map_err(|e| format!("cannot build a per-opcode histogram: {e}"))?;
                e.insert(h)
            }
        };
        entry
            .record(micros)
            .map_err(|e| format!("latency out of histogram range: {e}"))
    }
}

/// Keys whose `RESERVE` was granted, from the per-key answer.
///
/// A transfer DMAs a block into the slot a reservation granted, so transferring for a
/// declined key sends a payload nowhere and then fails its commit for want of a pending
/// write. Measured before this filter existed: 88 blocks written against 12 declined
/// reserves and 12 declined commits; with it, 76 written and **zero** declined commits.
///
/// A short `results` grants only what it answered for: assuming a missing byte meant success
/// would write a block Certus never reserved.
pub fn granted_keys(missing: &[u64], results: &[u8], out: &mut Vec<u64>) {
    out.clear();
    out.extend(
        missing
            .iter()
            .zip(results.iter())
            .filter(|(_, ok)| **ok == 1)
            .map(|(key, _)| *key),
    );
}

/// The plan kind that encodes as `opcode`.
///
/// The executor works in opcodes while [`OpStream`] encodes from plan kinds, so this is the
/// one place the two vocabularies meet.
pub fn kind_of(opcode: u32) -> OpKind {
    match opcode {
        op::CHECK => OpKind::Check,
        op::TOUCH => OpKind::Touch,
        op::LOOKUP => OpKind::Load,
        op::RESERVE => OpKind::Reserve,
        op::COPY_TO_STORE => OpKind::Transfer,
        op::COMMIT_STORE => OpKind::Commit,
        op::ABORT_STORE => OpKind::Abort,
        op::TAKE_EVENTS => OpKind::PollEvents,
        other => unreachable!("no plan kind encodes as opcode {other}"),
    }
}

/// Count one response's per-key results.
///
/// # These are Certus's decisions, not our failures
///
/// Every one of these opcodes answers with one byte per key. Ignoring the body — as this
/// generator once did, checking only the overall status — means a run in which *every* store
/// was declined reports full throughput: plausible numbers instead of an error.
///
/// They are counted and deliberately **not** treated as errors. We do not know when Certus
/// will evict anything: a block stored earlier and absent later is eviction working, which is
/// the behaviour under measurement, and a declined reserve is what a full tier looks like.
pub fn count_results(opcode: u32, body: &[u8], counters: &mut Counters) {
    match opcode {
        // `op_check` answers a three-valued `check_state` (MISS 0, RESIDENT 1, PENDING 2)
        // while `op_lookup` answers a binary flag. Treating `== 1` as a hit for both counted
        // every PENDING key as a miss, although `wire.rs` says `byte != 0` is the correct
        // existence test. PENDING means another lane's store is in flight: coming, not
        // absent, and never a miss.
        op::CHECK => {
            for b in body {
                match *b {
                    wire::check_state::RESIDENT => counters.check_resident += 1,
                    wire::check_state::PENDING => counters.check_pending += 1,
                    _ => counters.check_miss += 1,
                }
            }
        }
        // Binary, and a handle that failed to open is reported as 0 too, so a miss count is
        // an upper bound on true cache misses — the wire does not distinguish them.
        op::LOOKUP => {
            for b in body {
                if *b == 1 {
                    counters.lookup_hits += 1;
                } else {
                    counters.lookup_misses += 1;
                }
            }
        }
        op::RESERVE => {
            counters.reserves_attempted += body.len() as u64;
            counters.reserves_declined += body.iter().filter(|b| **b == 0).count() as u64;
        }
        op::COPY_TO_STORE => {
            counters.transfers_attempted += body.len() as u64;
            counters.transfers_declined += body.iter().filter(|b| **b == 0).count() as u64;
        }
        op::COMMIT_STORE => {
            counters.commits_attempted += body.len() as u64;
            counters.commits_declined += body.iter().filter(|b| **b == 0).count() as u64;
        }
        // A TOUCH answering 0 means the key was not there to reorder, which is eviction
        // again; TAKE_EVENTS answers with events rather than per-key results.
        _ => {}
    }
}

/// Clear the memory tier once, before the timed window (FR-046).
///
/// Returns how many entries the server dropped. Issued on its own path rather than through
/// [`OpStream`], which refuses the opcode: clearing is setup, and a clear inside the
/// operation stream would be the generator evicting on the eviction policy's behalf.
///
/// # Errors
///
/// If the request fails or the server answers with an error status.
pub fn clear_memory_tier(client: &Client, channel: usize) -> Result<u64, String> {
    let (status, body) = client
        .request(
            channel,
            op::CLEAR_MEMORY_TIER,
            &[],
            SPIN_ITERS,
            ATTEMPT_TIMEOUT,
            REQUEST_DEADLINE,
        )
        .map_err(|e| format!("clearing the memory tier failed: {e}"))?;
    if status != wire::STATUS_OK {
        return Err(format!(
            "clearing the memory tier returned an error status: {}",
            String::from_utf8_lossy(&body)
        ));
    }
    let mut r = wire::Reader::new(&body);
    Ok(r.u64().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_granted_reservation_is_transferred() {
        // `op_reserve` answers one byte per key. A declined key has no slot, so transferring
        // it sends a payload nowhere and its commit then fails for want of a pending write.
        let missing = [10u64, 11, 12, 13];
        let mut out = Vec::new();
        granted_keys(&missing, &[1, 0, 1, 0], &mut out);
        assert_eq!(out, vec![10, 12]);
        granted_keys(&missing, &[1, 1, 1, 1], &mut out);
        assert_eq!(out, missing);
        granted_keys(&missing, &[0, 0, 0, 0], &mut out);
        assert!(out.is_empty(), "nothing reserved, so nothing may be sent");
    }

    #[test]
    fn a_short_reserve_answer_grants_only_what_it_answered_for() {
        let mut out = Vec::new();
        granted_keys(&[1, 2, 3], &[1], &mut out);
        assert_eq!(out, vec![1]);
        granted_keys(&[1, 2, 3], &[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn check_is_three_valued_and_lookup_is_binary() {
        // The defect this replaced: both were counted as "1 means hit", which recorded every
        // PENDING key as a miss.
        let mut c = Counters::default();
        count_results(
            op::CHECK,
            &[
                wire::check_state::RESIDENT,
                wire::check_state::MISS,
                wire::check_state::PENDING,
            ],
            &mut c,
        );
        assert_eq!((c.check_resident, c.check_miss, c.check_pending), (1, 1, 1));

        let mut c = Counters::default();
        count_results(op::LOOKUP, &[1, 0, 1], &mut c);
        assert_eq!((c.lookup_hits, c.lookup_misses), (2, 1));
    }

    #[test]
    fn a_decline_is_counted_with_its_denominator() {
        // A decline count without one cannot be interpreted: 12 of 12 and 12 of 500 are
        // different situations.
        let mut c = Counters::default();
        count_results(op::RESERVE, &[1, 0, 1, 1], &mut c);
        assert_eq!((c.reserves_attempted, c.reserves_declined), (4, 1));
        count_results(op::COPY_TO_STORE, &[1, 1], &mut c);
        assert_eq!((c.transfers_attempted, c.transfers_declined), (2, 0));
        count_results(op::COMMIT_STORE, &[0, 0], &mut c);
        assert_eq!((c.commits_attempted, c.commits_declined), (2, 2));
    }

    #[test]
    fn a_touch_answer_is_not_a_cache_outcome() {
        // A TOUCH answering 0 means the key was not there to reorder, which is eviction
        // rather than a hit or a miss, so it must not move the hit counters.
        let mut c = Counters::default();
        count_results(op::TOUCH, &[0, 0, 1], &mut c);
        assert_eq!(c.check_resident + c.check_miss + c.lookup_hits, 0);
    }

    #[test]
    fn every_issued_opcode_maps_to_a_plan_kind() {
        for opcode in [
            op::CHECK,
            op::TOUCH,
            op::LOOKUP,
            op::RESERVE,
            op::COPY_TO_STORE,
            op::COMMIT_STORE,
            op::ABORT_STORE,
            op::TAKE_EVENTS,
        ] {
            let _ = kind_of(opcode);
        }
    }
}
