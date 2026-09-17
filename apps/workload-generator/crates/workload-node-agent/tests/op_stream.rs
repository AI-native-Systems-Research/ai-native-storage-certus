//! The issued operation stream, checked against a mock mailbox (T051).
//!
//! # Where the expected answer comes from, and why that matters
//!
//! The obvious version of this test writes down the sequence
//! `Check, Touch, Load, Reserve, Transfer, Commit, Poll` and asserts the encoder produces
//! it. That test cannot fail for any reason worth knowing: both sides come from the code
//! under test, so it passes equally well when the mapping is wrong. This project has
//! already been bitten by exactly that three times.
//!
//! So the mock here is a **protocol checker, not a recorder**, and every rule it enforces
//! is sourced from something outside this crate:
//!
//! | Rule | Source |
//! | --- | --- |
//! | `commit_store` needs a pending write for the key | `IDispatcher` contract: returns `KeyNotFound` if no pending write exists |
//! | `size` must be non-zero | `IDispatcher` contract: returns `InvalidParameter` |
//! | no duplicate keys within one request | `check_duplicate_keys` in `shmq-dispatcher::translate` — a hard `OpError` |
//! | a store is reserve → transfer → commit | FR-040 |
//! | `promote` is always 0 | FR-043, FR-044 |
//! | no `PIN`/`UNPIN`/`REMOVE`/`CLEAR_MEMORY_TIER`/`POPULATE`/`FLUSH_TO_SSD` | FR-043 |
//! | every response carries one byte per key | the `Writer::with_capacity(n)` + `w.u8` loops in `translate.rs` |
//!
//! The mock decodes with `shmq_dispatcher::wire::Reader` — the server's own reader — so a
//! payload the real server would reject fails here too. It was worth building: running the
//! real server is what found that `op_lookup` takes a handle batch rather than a key list,
//! and a mock that shared the encoder's assumptions would have agreed with the bug.
//!
//! # What it cannot check
//!
//! That Certus *does* the right thing with a well-formed request. This is a test of the
//! stream the generator emits, not of the cache. FR-036 also makes hit/miss outcomes
//! non-reproducible, so the mock reports misses deterministically rather than modelling a
//! cache: what is being asserted is the shape and order of what we ask for.
//!
//! # A cache outcome is never a violation
//!
//! **We do not know when Certus will evict anything**, and we must not.  So the mock never
//! treats an absent key as an error: storing a block and later finding it gone is eviction
//! working, and it is the behaviour under measurement. Nor is a repeat store a fault — the
//! spec anticipates two sessions racing to mint the same shared prefix, both missing and
//! both storing. The only things recorded as violations are things the *generator* did:
//! a malformed payload, a forbidden opcode, a duplicate key inside one request, or a store
//! whose reserve/transfer/commit sequence was not followed.
//!
//! This distinction is why the client counts per-key results rather than failing on them.
//! A run in which every reserve came back 0 because the cache was full is a run worth
//! knowing about, but it is not a broken generator.

use std::collections::{HashMap, HashSet};

use shmq_dispatcher::wire::{self, op};
use workload_model::description::WorkloadDescription;
use workload_model::plan::{OpKind, OperationPlan};
use workload_model::sim::Simulation;
use workload_node_agent::opstream::{forbidden_opcode, Encoding, OpStream, TurnSplit};
use workload_node_agent::payload::HandleBatchTemplate;

/// A workload with shared prefixes, several turns and growth, so the plan contains every
/// operation kind and long enough key lists to exercise chunking.
const DESCRIPTION: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 6}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 5}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 6}
    input_growth: {constant: 3}
    output_growth: {constant: 2}
    think_time: {constant: 4}
"#;

const BLOCK_BYTES: u32 = 32768;

fn plan_of(seed: u64, span: f64) -> OperationPlan {
    let d: WorkloadDescription = DESCRIPTION.parse().unwrap();
    let mut sim = Simulation::new(&d, seed).unwrap();
    let mut plan = OperationPlan::default();
    sim.run_until(span, &mut |s, t| plan.record_turn(s, t));
    plan
}

/// A template with no device behind it: the protocol is bytes, so this needs no GPU.
fn template(max_keys: usize) -> HandleBatchTemplate {
    let mut handle = [0u8; 64];
    for (i, b) in handle.iter_mut().enumerate() {
        *b = i as u8;
    }
    HandleBatchTemplate::new(&handle, 0, 0, BLOCK_BYTES, max_keys)
}

/// What the mock saw, and what it objected to.
#[derive(Debug, Default)]
struct Mailbox {
    /// Keys with a reservation outstanding, mirroring the server's `pending_stores`.
    pending: HashMap<u64, u32>,
    /// Keys whose store completed.
    committed: HashSet<u64>,
    /// Keys ever transferred, so a commit-before-transfer is visible.
    transferred: HashSet<u64>,
    /// Requests seen, by opcode.
    counts: HashMap<u32, u64>,
    /// Keys carried, by opcode.
    keys_seen: HashMap<u32, u64>,
    /// Every key any request named.
    all_keys: HashSet<u64>,
    /// Reserves for a key already reserved — a legitimate mint race, counted not flagged.
    repeat_reserves: u64,
    /// Protocol violations, each a reason. Empty is the only acceptable outcome.
    ///
    /// A violation is something **the generator** did wrong: a malformed payload, a
    /// forbidden opcode, a duplicate key within one request, a store out of sequence. It
    /// is never a cache outcome. Whether a key is present is Certus's decision — it may
    /// have been evicted at any time — so a miss, a failed reserve on a full cache, or a
    /// re-store of a racing prefix are all normal and none of them appear here.
    violations: Vec<String>,
}

impl Mailbox {
    /// Handle one request, returning the response body the real server would.
    fn request(&mut self, opcode: u32, payload: &[u8]) -> Vec<u8> {
        *self.counts.entry(opcode).or_default() += 1;

        if let Some(why) = forbidden_opcode(opcode) {
            self.violations
                .push(format!("issued a forbidden opcode {opcode}: {why}"));
            return Vec::new();
        }

        match opcode {
            op::CHECK => {
                let keys = self.read_keys(opcode, payload);
                // Answers from the mock's own state, exactly as `op_check` does: a
                // reservation in flight is PENDING, a committed key is RESIDENT, anything
                // else MISS. Modelling this is what lets the reactive executor be tested —
                // a mock that always said MISS would store on every turn and never
                // exercise the resident path.
                keys.iter()
                    .map(|k| {
                        if self.pending.contains_key(k) {
                            wire::check_state::PENDING
                        } else if self.committed.contains(k) {
                            wire::check_state::RESIDENT
                        } else {
                            wire::check_state::MISS
                        }
                    })
                    .collect()
            }
            op::TOUCH => {
                let mut r = wire::Reader::new(payload);
                let promote = r.u8().expect("TOUCH carries a promote byte");
                if promote != 0 {
                    self.violations.push(format!(
                        "TOUCH requested promotion (promote={promote}), which FR-043 \
                         forbids: promotion is a decision, and taking it would measure a \
                         tiering policy the generator had partly written"
                    ));
                }
                let keys = self.read_keys_from(opcode, &mut r);
                vec![1u8; keys.len()]
            }
            op::RESERVE => {
                let mut r = wire::Reader::new(payload);
                let n = r.u32().expect("RESERVE carries a count") as usize;
                let mut keys = Vec::with_capacity(n);
                let mut out = Vec::with_capacity(n);
                for _ in 0..n {
                    let key = r.u64().expect("a key");
                    let size = r.u32().expect("a size");
                    let _session = r.u64().expect("a session");
                    // IDispatcher: InvalidParameter if size is 0.
                    if size == 0 {
                        self.violations
                            .push(format!("RESERVE for key {key} asked for 0 bytes"));
                    }
                    if size != BLOCK_BYTES {
                        self.violations.push(format!(
                            "RESERVE for key {key} asked for {size} bytes, not the \
                             description's {BLOCK_BYTES} per block"
                        ));
                    }
                    // NOT a violation if the key is already present or already
                    // reserved. `prepare_store` answers `AlreadyExists`, but the spec
                    // anticipates exactly that: two sessions racing to mint the same
                    // shared prefix both miss and both store, so a repeat reserve is a
                    // legitimate outcome of the workload rather than a client defect. The
                    // server reports it as a per-key 0, not a failed request.
                    self.repeat_reserves += u64::from(self.pending.contains_key(&key));
                    self.pending.insert(key, size);
                    keys.push(key);
                    out.push(1u8);
                }
                self.record(opcode, &keys);
                out
            }
            op::COPY_TO_STORE | op::LOOKUP => {
                let keys = self.read_handle_batch(opcode, payload);
                let mut out = Vec::with_capacity(keys.len());
                for key in &keys {
                    if opcode == op::LOOKUP && !self.committed.contains(key) {
                        // A load of a key the cache does not hold. Under the reactive rule
                        // this should never happen: the executor loads only what CHECK
                        // reported resident, so a load of an absent key means it acted on
                        // a decision the cache never gave it.
                        self.violations.push(format!(
                            "LOOKUP for key {key} that was never committed: the executor \
                             must load only what CHECK reported RESIDENT"
                        ));
                    }
                    if opcode == op::COPY_TO_STORE {
                        // FR-040: a transfer belongs between a reserve and a commit.
                        if !self.pending.contains_key(key) {
                            self.violations.push(format!(
                                "COPY_TO_STORE for key {key} with no reservation \
                                 outstanding: FR-040 requires reserve, transfer, commit"
                            ));
                        }
                        self.transferred.insert(*key);
                        out.push(1u8);
                    } else {
                        // A lookup is a read; every key a miss, deterministically.
                        out.push(0u8);
                    }
                }
                out
            }
            op::COMMIT_STORE => {
                let keys = self.read_keys(opcode, payload);
                let mut out = Vec::with_capacity(keys.len());
                for key in &keys {
                    // IDispatcher: KeyNotFound if no pending write exists. The server
                    // reports that per key as a 0 rather than failing the request.
                    match self.pending.remove(key) {
                        None => {
                            // In the mock every reservation is granted, so a missing
                            // pending write means the generator never reserved this key —
                            // a sequence defect. Against a real server a 0 here may
                            // instead mean the reserve failed on a full cache, which is
                            // not our fault and is counted rather than flagged.
                            self.violations.push(format!(
                                "COMMIT_STORE for key {key} that this run never reserved \
                                 (the mock grants every reservation, so this is a \
                                 sequence defect and not a full cache)"
                            ));
                            out.push(0u8);
                        }
                        Some(_) => {
                            if !self.transferred.contains(key) {
                                self.violations.push(format!(
                                    "COMMIT_STORE for key {key} that was never \
                                     transferred: FR-040's sequence was not followed"
                                ));
                            }
                            self.committed.insert(*key);
                            out.push(1u8);
                        }
                    }
                }
                out
            }
            op::ABORT_STORE => {
                let keys = self.read_keys(opcode, payload);
                for key in &keys {
                    if self.pending.remove(key).is_none() {
                        self.violations
                            .push(format!("ABORT_STORE for key {key} with no pending write"));
                    }
                }
                vec![1u8; keys.len()]
            }
            op::TAKE_EVENTS => {
                let mut r = wire::Reader::new(payload);
                let max = r.u32().expect("TAKE_EVENTS carries a cap");
                if max != 0 {
                    self.violations.push(format!(
                        "TAKE_EVENTS capped at {max}: a cap lets events accumulate and \
                         turns a steady per-turn cost into a sawtooth"
                    ));
                }
                // No events queued.
                0u32.to_le_bytes().to_vec()
            }
            other => {
                self.violations
                    .push(format!("unknown opcode {other} reached the mailbox"));
                Vec::new()
            }
        }
    }

    fn read_keys(&mut self, opcode: u32, payload: &[u8]) -> Vec<u64> {
        let mut r = wire::Reader::new(payload);
        self.read_keys_from(opcode, &mut r)
    }

    fn read_keys_from(&mut self, opcode: u32, r: &mut wire::Reader) -> Vec<u64> {
        let n = r.u32().expect("a key count") as usize;
        let mut keys = Vec::with_capacity(n);
        for _ in 0..n {
            keys.push(r.u64().expect("a key"));
        }
        self.record(opcode, &keys);
        keys
    }

    /// Decode a handle batch exactly as `decode_handle_batch` does, including its
    /// `handle_idx` range check.
    fn read_handle_batch(&mut self, opcode: u32, payload: &[u8]) -> Vec<u64> {
        let mut r = wire::Reader::new(payload);
        let n_handles = r.u32().expect("n_handles") as usize;
        if n_handles == 0 {
            self.violations
                .push(format!("opcode {opcode} sent an empty handle table"));
        }
        for _ in 0..n_handles {
            let _h = r.handle().expect("a 64-byte handle");
            let _dev = r.i32().expect("a device ordinal");
        }
        let n_entries = r.u32().expect("n_entries") as usize;
        let mut keys = Vec::with_capacity(n_entries);
        for _ in 0..n_entries {
            let key = r.u64().expect("a key");
            let nreg = r.u16().expect("a region count") as usize;
            if nreg == 0 {
                self.violations
                    .push(format!("key {key} named no region to transfer into"));
            }
            for _ in 0..nreg {
                let idx = r.u32().expect("a handle_idx");
                let _offset = r.u64().expect("an offset");
                let size = r.u32().expect("a size");
                if idx as usize >= n_handles {
                    self.violations.push(format!(
                        "region handle_idx {idx} out of range (n_handles={n_handles}), \
                         which the dispatcher refuses"
                    ));
                }
                if size != BLOCK_BYTES {
                    self.violations.push(format!(
                        "key {key} named a {size}-byte region, not the description's \
                         {BLOCK_BYTES}"
                    ));
                }
            }
            keys.push(key);
        }
        self.record(opcode, &keys);
        keys
    }

    fn record(&mut self, opcode: u32, keys: &[u64]) {
        // `check_duplicate_keys`: a duplicate within one request is a hard OpError.
        let mut seen = HashSet::with_capacity(keys.len());
        for key in keys {
            if !seen.insert(*key) {
                self.violations.push(format!(
                    "duplicate key {key} within one opcode-{opcode} request, which the \
                     dispatcher answers with an error rather than a per-key result"
                ));
            }
            self.all_keys.insert(*key);
        }
        *self.keys_seen.entry(opcode).or_default() += keys.len() as u64;
    }

    fn count(&self, opcode: u32) -> u64 {
        self.counts.get(&opcode).copied().unwrap_or(0)
    }

    fn keys(&self, opcode: u32) -> u64 {
        self.keys_seen.get(&opcode).copied().unwrap_or(0)
    }
}

/// Chunk a key list the way the driver does, yielding one empty chunk for a keyless
/// operation so that a poll still produces a request.
fn chunks_of(keys: &[u64], n: usize) -> Vec<&[u64]> {
    if keys.is_empty() {
        return vec![&[]];
    }
    keys.chunks(n).collect()
}

/// Drive a whole plan through the reactive executor into the mock.
///
/// This mirrors what `live::consume` does and shares the decision with it via
/// [`TurnSplit`]: offer the turn's whole path root-to-growth with `CHECK`, then touch and
/// load what came back resident and store what came back absent (FR-072a). The mock answers
/// `CHECK` from its own committed and pending sets, so a warm path really does stop being
/// stored — a mock that always said MISS would store on every turn and never exercise the
/// resident branch at all.
fn drive(plan: &OperationPlan, batch_keys: usize, with_gpu: bool) -> Mailbox {
    let mut stream = OpStream::new(BLOCK_BYTES, batch_keys);
    if with_gpu {
        stream = stream.with_template(template(batch_keys), None, 0);
    }
    let mut mailbox = Mailbox::default();
    let mut split = TurnSplit::default();
    let mut path: Vec<u64> = Vec::new();
    let mut states: Vec<u8> = Vec::new();

    // One turn at a time, because the reactive rule is per turn. A turn is the run of
    // operations sharing one (session, virtual instant), which is how `record_turn` emits.
    let ops = plan.operations();
    let mut i = 0usize;
    while i < ops.len() {
        let id = (ops[i].session(), ops[i].at().to_bits());
        let mut j = i;
        while j < ops.len() && (ops[j].session(), ops[j].at().to_bits()) == id {
            j += 1;
        }
        let group = &ops[i..j];

        // The path: root of the prefix through the end of the new growth. Check names the
        // prefix and Reserve the growth; Touch/Load and Transfer/Commit only repeat them.
        path.clear();
        let mut seen = HashSet::new();
        for op in group {
            if matches!(op.kind(), OpKind::Check | OpKind::Reserve) {
                for key in plan.keys_of(op) {
                    if seen.insert(*key) {
                        path.push(*key);
                    }
                }
            }
        }

        states.clear();
        issue(
            &mut mailbox,
            &mut stream,
            OpKind::Check,
            &path,
            0,
            Some(&mut states),
        );
        split.split(&path, &states);
        for kind in [OpKind::Touch, OpKind::Load] {
            issue(&mut mailbox, &mut stream, kind, split.resident(), 0, None);
        }
        let session = group[0].session();
        for kind in [OpKind::Reserve, OpKind::Transfer, OpKind::Commit] {
            issue(
                &mut mailbox,
                &mut stream,
                kind,
                split.missing(),
                session,
                None,
            );
        }
        if group.iter().any(|o| o.kind() == OpKind::PollEvents) {
            issue(&mut mailbox, &mut stream, OpKind::PollEvents, &[], 0, None);
        }
        i = j;
    }
    mailbox
}

/// Issue one kind over `keys`, chunked, collecting response bytes when asked.
fn issue(
    mailbox: &mut Mailbox,
    stream: &mut OpStream,
    kind: OpKind,
    keys: &[u64],
    session: u64,
    mut collect: Option<&mut Vec<u8>>,
) {
    if keys.is_empty() && kind != OpKind::PollEvents {
        return;
    }
    let batch_keys = stream.batch_keys();
    for chunk in chunks_of(keys, batch_keys) {
        match stream.encode_chunk(kind, chunk, session).unwrap() {
            Encoding::Ready { opcode, payload } => {
                let body = mailbox.request(opcode, payload);
                if let Some(out) = collect.as_deref_mut() {
                    out.extend_from_slice(&body);
                }
            }
            Encoding::NeedsGpuPayload { .. } | Encoding::NotPlanned => {}
        }
    }
}

#[test]
fn a_whole_run_commits_no_protocol_violation() {
    // The central assertion. Every rule the mock enforces comes from the IDispatcher
    // contract, from `translate.rs`, or from a numbered requirement — see the module docs.
    let plan = plan_of(11, 300.0);
    let mailbox = drive(&plan, 64, true);
    assert!(
        mailbox.violations.is_empty(),
        "the operation stream violated the protocol:\n  {}",
        mailbox.violations.join("\n  ")
    );
    // And it must have done enough to be worth asserting about.
    assert!(
        mailbox.count(op::CHECK) > 10,
        "only {} checks: too small a run to test",
        mailbox.count(op::CHECK)
    );
}

#[test]
fn every_store_follows_reserve_then_transfer_then_commit() {
    // FR-040. The mock rejects a transfer with no reservation and a commit with no pending
    // write, so an out-of-order stream fails above; this asserts the sequence actually
    // *happened* rather than that nothing was caught, and that it balanced.
    let plan = plan_of(12, 300.0);
    let mailbox = drive(&plan, 64, true);
    assert!(mailbox.violations.is_empty(), "{:?}", mailbox.violations);
    assert!(mailbox.count(op::RESERVE) > 0, "nothing was ever stored");
    assert_eq!(
        mailbox.keys(op::RESERVE),
        mailbox.keys(op::COPY_TO_STORE),
        "every reserved key must be transferred"
    );
    assert_eq!(
        mailbox.keys(op::RESERVE),
        mailbox.keys(op::COMMIT_STORE),
        "every reserved key must be committed"
    );
    assert!(
        mailbox.pending.is_empty(),
        "{} reservations were left outstanding, which the server reclaims after 30s as \
         stale — a generator that leaks them measures a server doing cleanup work no \
         production client would cause",
        mailbox.pending.len()
    );
    // Committed keys can be fewer than committed *references*: two sessions may mint the
    // same shared prefix and both store it, which the spec anticipates.
    assert!(mailbox.committed.len() as u64 <= mailbox.keys(op::COMMIT_STORE));
    assert!(
        !mailbox.committed.is_empty(),
        "nothing was committed, so the sequence was never exercised"
    );
}

#[test]
fn the_reference_report_is_issued_so_recency_policies_see_a_recent_workload() {
    // The defect this suite exists to keep fixed: the plan once had no Touch at all.
    // `engine.touch` is documented "Update eviction ordering for the given keys" and is a
    // separate call from `batch_check`, so omitting it scores every recency policy against
    // a workload in which nothing is ever recently used (FR-041).
    let plan = plan_of(13, 300.0);
    let mailbox = drive(&plan, 64, true);
    assert!(
        mailbox.count(op::TOUCH) > 0,
        "no TOUCH was issued: recency policies would be scored against a workload where \
         nothing is ever recently used"
    );
    // Under the reactive rule TOUCH covers the *resident* subset, not the whole path —
    // touching a key the cache does not hold reorders nothing. The invariant that carries
    // the recency signal is that every key we load, we also report: a load without a touch
    // would read a block while leaving the policy believing it was never used.
    assert_eq!(
        mailbox.keys(op::TOUCH),
        mailbox.keys(op::LOOKUP),
        "every loaded key must also be touched"
    );
    assert!(
        mailbox.keys(op::TOUCH) < mailbox.keys(op::CHECK),
        "the whole path was touched, so the resident split is not being applied"
    );
}

#[test]
fn events_are_polled_because_the_production_client_polls() {
    // FR-042. A client that never drains events leaves the server queueing them, which
    // changes the server's own behaviour over a long run.
    let plan = plan_of(14, 300.0);
    let mailbox = drive(&plan, 64, true);
    assert!(
        mailbox.count(op::TAKE_EVENTS) > 0,
        "events were never polled"
    );
}

#[test]
fn no_forbidden_operation_reaches_the_mailbox() {
    // FR-043 as a property of the stream. The mock records a violation for any of the six,
    // so this asserts over a long run rather than over one call.
    let plan = plan_of(15, 600.0);
    let mailbox = drive(&plan, 64, true);
    for opcode in [
        op::PIN,
        op::UNPIN,
        op::REMOVE,
        op::CLEAR_MEMORY_TIER,
        op::POPULATE,
        op::FLUSH_TO_SSD,
    ] {
        assert_eq!(
            mailbox.count(opcode),
            0,
            "opcode {opcode} was issued: {}",
            forbidden_opcode(opcode).unwrap_or("")
        );
    }
    assert!(mailbox.violations.is_empty(), "{:?}", mailbox.violations);
}

#[test]
fn batch_keys_changes_the_requests_but_not_the_workload() {
    // FR-069 with FR-072: request scheduling is the client's, the workload is the
    // description's. This is the property that was silently untrue while --batch-keys was
    // parsed but never plumbed through to the live path.
    let plan = plan_of(16, 300.0);
    let wide = drive(&plan, 4096, true);
    let narrow = drive(&plan, 4, true);

    let wide_requests: u64 = wide.counts.values().sum();
    let narrow_requests: u64 = narrow.counts.values().sum();
    assert!(
        narrow_requests > wide_requests,
        "a smaller --batch-keys must produce more requests ({narrow_requests} vs \
         {wide_requests})"
    );

    // Same keys, same operations, whatever the batching.
    for opcode in [
        op::CHECK,
        op::TOUCH,
        op::LOOKUP,
        op::RESERVE,
        op::COPY_TO_STORE,
        op::COMMIT_STORE,
    ] {
        assert_eq!(
            wide.keys(opcode),
            narrow.keys(opcode),
            "opcode {opcode} carried a different number of keys under a different \
             --batch-keys, so the option changed the workload"
        );
    }
    assert_eq!(
        wide.all_keys, narrow.all_keys,
        "the set of keys touched changed with the batch size"
    );
    assert!(narrow.violations.is_empty(), "{:?}", narrow.violations);
}

#[test]
fn the_stream_is_deterministic_from_description_and_seed() {
    // FR-072. Two independent drives of the same seed must issue the same thing, or none
    // of the numbers above can be reproduced from a report.
    let a = drive(&plan_of(17, 200.0), 64, true);
    let b = drive(&plan_of(17, 200.0), 64, true);
    assert_eq!(a.counts, b.counts);
    assert_eq!(a.keys_seen, b.keys_seen);
    assert_eq!(a.all_keys, b.all_keys);

    let c = drive(&plan_of(18, 200.0), 64, true);
    assert_ne!(
        a.all_keys, c.all_keys,
        "two different seeds produced the same keys, so the seed does nothing"
    );
}

#[test]
fn without_a_payload_buffer_the_data_moving_operations_are_absent_not_malformed() {
    // The control-path-only mode (`--no-payload`). It must issue *nothing* for the two,
    // rather than a key-list LOOKUP the server would reject — which is the bug the real
    // server caught with "truncated: need 64 bytes at offset 4, have 32".
    let plan = plan_of(19, 300.0);
    let without = drive(&plan, 64, false);
    assert_eq!(without.count(op::LOOKUP), 0);
    assert_eq!(without.count(op::COPY_TO_STORE), 0);
    // The control path still ran, and a store with no transfer must then be visible as a
    // violation rather than passing quietly: it is a partial run, and the report says so.
    assert!(without.count(op::CHECK) > 0);
    assert!(
        without
            .violations
            .iter()
            .any(|v| v.contains("never transferred")),
        "a store with no transfer should be detectable; got {:?}",
        without.violations
    );

    let with = drive(&plan, 64, true);
    assert!(
        with.count(op::LOOKUP) > 0,
        "loads must be issued with a buffer"
    );
    assert!(with.count(op::COPY_TO_STORE) > 0);
    assert!(with.violations.is_empty(), "{:?}", with.violations);
}

#[test]
fn the_mock_can_actually_fail() {
    // A protocol checker that cannot object is decoration. Three violations are provoked
    // deliberately, because every assertion above is only worth its ability to fail.
    let mut m = Mailbox::default();

    // A commit with no reservation: IDispatcher answers KeyNotFound.
    let mut w = wire::Writer::with_capacity(12);
    w.u32(1);
    w.u64(42);
    m.request(op::COMMIT_STORE, &w.into_bytes());
    assert!(
        m.violations.iter().any(|v| v.contains("never reserved")),
        "got {:?}",
        m.violations
    );

    // A TOUCH that asks for promotion: FR-043.
    let mut m2 = Mailbox::default();
    let mut w = wire::Writer::with_capacity(13);
    w.u8(1);
    w.u32(1);
    w.u64(7);
    m2.request(op::TOUCH, &w.into_bytes());
    assert!(
        m2.violations.iter().any(|v| v.contains("promotion")),
        "got {:?}",
        m2.violations
    );

    // A forbidden opcode.
    let mut m3 = Mailbox::default();
    m3.request(op::PIN, &[]);
    assert!(
        m3.violations.iter().any(|v| v.contains("forbidden")),
        "got {:?}",
        m3.violations
    );

    // And a duplicate key within one request, which the dispatcher refuses outright.
    let mut m4 = Mailbox::default();
    let mut w = wire::Writer::with_capacity(20);
    w.u32(2);
    w.u64(5);
    w.u64(5);
    m4.request(op::CHECK, &w.into_bytes());
    assert!(
        m4.violations.iter().any(|v| v.contains("duplicate key")),
        "got {:?}",
        m4.violations
    );
}

#[test]
fn every_response_carries_one_byte_per_key() {
    // The shape the server actually returns: `Writer::with_capacity(n)` then one `w.u8`
    // per key, in `op_reserve`, `op_commit_store`, `op_copy_to_store` and `op_lookup`.
    // The generator ignored these bytes, so a run in which every reserve *failed* reported
    // full throughput; asserting the shape here is what makes the client's new check
    // meaningful rather than hopeful.
    let mut m = Mailbox::default();
    let mut w = wire::Writer::with_capacity(4 + 3 * 20);
    w.u32(3);
    for key in [1u64, 2, 3] {
        w.u64(key);
        w.u32(BLOCK_BYTES);
        w.u64(99);
    }
    let response = m.request(op::RESERVE, &w.into_bytes());
    assert_eq!(response.len(), 3, "one result byte per key");
    assert!(response.iter().all(|b| *b == 1));
    assert!(m.violations.is_empty(), "{:?}", m.violations);
}

#[test]
fn repeat_stores_are_the_shared_mint_race_and_never_a_session_restoring_its_own_block() {
    // Why this classification matters. A live run against a *fresh, empty* server declines
    // 60-72 reserves with `evictions[memory 0, ssd 0]` and `backpressure 0`, so nothing is
    // full: the dispatcher is answering `AlreadyExists`. A controlled comparison confirms
    // the cause — a workload with `turns: 1` (no repeat stores) declines nothing, while
    // `turns: 3` declines 60.
    //
    // That leaves two possible explanations, and they are not equally acceptable:
    //
    //   * two sessions racing to mint the same shared prefix — legitimate, and the spec
    //     anticipates it explicitly at FR-036;
    //   * one session re-storing a block it stored itself in an earlier turn — a defect,
    //     because the production client knows it already stored that block.
    //
    // The wire cannot tell them apart (a per-key 0 carries no reason), so it is settled
    // here, where the session id of every reserve is known.
    let plan = plan_of(21, 400.0);
    let mut first_owner: HashMap<u64, u64> = HashMap::new();
    let mut same_session = Vec::new();
    let mut cross_session = 0u64;
    for op in plan.operations() {
        if op.kind() != OpKind::Reserve {
            continue;
        }
        for key in plan.keys_of(op) {
            match first_owner.get(key) {
                None => {
                    first_owner.insert(*key, op.session());
                }
                Some(owner) if *owner == op.session() => same_session.push(*key),
                Some(_) => cross_session += 1,
            }
        }
    }
    assert!(
        !first_owner.is_empty(),
        "no reserves at all, so nothing was classified"
    );
    assert!(
        same_session.is_empty(),
        "{} keys were reserved twice by the SAME session (e.g. {:?}): a session re-storing \
         its own block is not the shared-prefix race, it is the generator asking Certus to \
         store something it already stored, which the production client would not do",
        same_session.len(),
        &same_session[..same_session.len().min(5)]
    );
    eprintln!(
        "reserves: {} distinct keys, {cross_session} cross-session repeats (the mint race)",
        first_owner.len()
    );
}

/// The description used for the live measurements on node2, so the numbers compare.
const LIVE_DESCRIPTION: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 4}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 4}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 3}
    input_growth: {constant: 2}
    output_growth: {constant: 1}
    think_time: {constant: 10}
"#;

fn live_plan(seed: u64, span: f64) -> OperationPlan {
    let d: WorkloadDescription = LIVE_DESCRIPTION.parse().unwrap();
    let mut sim = Simulation::new(&d, seed).unwrap();
    let mut plan = OperationPlan::default();
    sim.run_until(span, &mut |s, t| plan.record_turn(s, t));
    plan
}

/// Keys per operation kind, and the distinct read/stored key sets.
fn breakdown(plan: &OperationPlan) -> (HashMap<OpKind, u64>, HashSet<u64>, HashSet<u64>) {
    let mut per_kind: HashMap<OpKind, u64> = HashMap::new();
    let mut read = HashSet::new();
    let mut stored = HashSet::new();
    for op in plan.operations() {
        let keys = plan.keys_of(op);
        *per_kind.entry(op.kind()).or_default() += keys.len() as u64;
        match op.kind() {
            OpKind::Check | OpKind::Load => read.extend(keys.iter().copied()),
            OpKind::Reserve => stored.extend(keys.iter().copied()),
            _ => {}
        }
    }
    (per_kind, read, stored)
}

#[test]
fn keys_are_derived_from_structural_position_and_so_barely_change_with_the_seed() {
    // The question this answers: a warm-cache run with a *different* seed declined 66 of
    // 72 reserves, which is only explicable if the two runs store nearly the same keys.
    // They do. `key(parent, salt)` salts on (tag, class, instance_index, block_ordinal) —
    // all structural counters — and the seed appears nowhere in it. So the seed changes
    // *which* structural positions get used and when, not the key value at a position.
    //
    // This is not a collision: keys are 64-bit splitmix64 outputs, and the overlap is
    // exact equality of derivation inputs. It matters for experiment hygiene — a new seed
    // does NOT give a fresh key space against a server that is already warm.
    let (_, _, a) = breakdown(&live_plan(7, 60.0));
    let (_, _, b) = breakdown(&live_plan(99, 60.0));
    let shared = a.intersection(&b).count();
    let overlap = shared as f64 / a.len().max(1) as f64;
    eprintln!(
        "stored keys: seed 7 = {}, seed 99 = {}, shared = {shared} ({:.1}% of seed 7)",
        a.len(),
        b.len(),
        overlap * 100.0
    );
    assert!(
        overlap > 0.5,
        "expected heavy cross-seed key overlap from structural derivation, got {:.1}%",
        overlap * 100.0
    );
}

#[test]
fn read_references_far_outnumber_stores_which_is_why_a_cold_run_still_hits() {
    // Why a cold cache shows a non-zero hit rate: "unique keys" is true of *stores* — the
    // plan reserves each key exactly once — but reads reference those same keys again and
    // again. A session re-reads its whole growing prefix every turn, and several sessions
    // read the same shared object, so the run warms itself and later reads hit blocks that
    // earlier turns of the same run stored. Hit rate is a property of reads, not stores.
    let plan = live_plan(7, 60.0);
    let (per_kind, read, stored) = breakdown(&plan);
    let check = per_kind.get(&OpKind::Check).copied().unwrap_or(0);
    let load = per_kind.get(&OpKind::Load).copied().unwrap_or(0);
    let reserve = per_kind.get(&OpKind::Reserve).copied().unwrap_or(0);
    eprintln!(
        "per-kind key references: {per_kind:?}\n\
         read refs {} over {} distinct keys; stored refs {reserve} over {} distinct keys\n\
         read keys never stored by this run: {}",
        check + load,
        read.len(),
        stored.len(),
        read.difference(&stored).count()
    );
    assert!(
        check + load > reserve,
        "reads {} must outnumber stores {reserve} or there is no reuse to measure",
        check + load
    );
}

#[test]
fn the_shared_root_is_stored_by_whichever_turn_first_finds_it_missing() {
    // The regression guard for the defect that prompted FR-072a. Shared prefix blocks sit
    // at the root of every path and **no turn mints them**, so the plan's own `Reserve`
    // operations exclude them. Under a fixed operation list nothing stored them, ever:
    // cross-session prefix sharing produced no cache hits at all, and a cold and a warm run
    // reported byte-identical check results because 192 of 312 read references named 4 keys
    // nothing would ever store.
    //
    // Offering the whole path fixes it — the first turn to find them absent stores them.
    let plan = live_plan(7, 60.0);
    let (_, read, planned_stores) = breakdown(&plan);
    let never_planned: HashSet<u64> = read.difference(&planned_stores).copied().collect();
    assert!(
        !never_planned.is_empty(),
        "this description has no shared blocks, so it cannot test the property"
    );

    let mailbox = drive(&plan, 64, true);
    assert!(mailbox.violations.is_empty(), "{:?}", mailbox.violations);
    for key in &never_planned {
        assert!(
            mailbox.committed.contains(key),
            "shared root key {key} was never stored, so no session can ever hit it"
        );
    }
}

#[test]
fn a_warm_path_stops_being_stored_and_an_evicted_block_is_stored_again() {
    // Two properties of the reactive rule, both invisible under a fixed operation list.
    //
    // First: once a path is resident, later turns stop storing it. Under a fixed list every
    // turn re-stored its new growth regardless, and the server answered `AlreadyExists`.
    //
    // Second, and the reason this matters beyond tidiness: a block **evicted mid-run** is
    // stored again when a later turn finds it absent. Without that a run's hit rate can only
    // decay, and the generator would be measuring a cache it never refills.
    let plan = live_plan(7, 60.0);

    // Drive once to warm the mock, then again over the same plan.
    let mut warm = drive(&plan, 64, true);
    let stored_first = warm.keys(op::RESERVE);
    assert!(stored_first > 0, "the first pass stored nothing");
    warm.counts.clear();
    warm.keys_seen.clear();
    warm.violations.clear();

    // Re-drive the same turns against the now-warm mock: nothing should need storing.
    let plan2 = live_plan(7, 60.0);
    let ops = plan2.operations();
    let mut split = TurnSplit::default();
    let mut stream = OpStream::new(BLOCK_BYTES, 64).with_template(template(64), None, 0);
    let mut path = Vec::new();
    let mut states = Vec::new();
    let mut i = 0usize;
    while i < ops.len() {
        let id = (ops[i].session(), ops[i].at().to_bits());
        let mut j = i;
        while j < ops.len() && (ops[j].session(), ops[j].at().to_bits()) == id {
            j += 1;
        }
        path.clear();
        let mut seen = HashSet::new();
        for op in &ops[i..j] {
            if matches!(op.kind(), OpKind::Check | OpKind::Reserve) {
                for key in plan2.keys_of(op) {
                    if seen.insert(*key) {
                        path.push(*key);
                    }
                }
            }
        }
        states.clear();
        issue(
            &mut warm,
            &mut stream,
            OpKind::Check,
            &path,
            0,
            Some(&mut states),
        );
        split.split(&path, &states);
        for kind in [OpKind::Reserve, OpKind::Transfer, OpKind::Commit] {
            issue(
                &mut warm,
                &mut stream,
                kind,
                split.missing(),
                ops[i].session(),
                None,
            );
        }
        i = j;
    }
    assert_eq!(
        warm.keys(op::RESERVE),
        0,
        "a fully resident path must need no stores, but {} keys were re-stored",
        warm.keys(op::RESERVE)
    );

    // Now evict one key, as Certus may at any time, and confirm the next offer re-stores it.
    let victim = *warm
        .committed
        .iter()
        .next()
        .expect("something is committed");
    warm.committed.remove(&victim);
    let mut split = TurnSplit::default();
    let mut states2 = Vec::new();
    issue(
        &mut warm,
        &mut stream,
        OpKind::Check,
        &[victim],
        0,
        Some(&mut states2),
    );
    split.split(&[victim], &states2);
    assert_eq!(
        split.missing(),
        &[victim],
        "an evicted block must be offered for storing again"
    );
    for kind in [OpKind::Reserve, OpKind::Transfer, OpKind::Commit] {
        issue(&mut warm, &mut stream, kind, split.missing(), 1, None);
    }
    assert!(
        warm.committed.contains(&victim),
        "the evicted block was not stored again, so the hit rate could only decay"
    );
    assert!(warm.violations.is_empty(), "{:?}", warm.violations);
}
