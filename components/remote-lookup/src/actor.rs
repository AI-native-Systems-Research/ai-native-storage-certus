//! The actor: a single poll-loop thread that owns all mutable operation state.
//!
//! Each iteration drains caller submissions, inbound zyre events
//! (`IZyreNode::try_recv`), then advances per-operation deadlines, then
//! bounded-sleeps only when fully idle (research Decision 1). Callers hand work
//! in via an MPSC submission channel and block on a per-op one-shot completion.
//!
//! The actor plays both roles concurrently: as a **client** it drives its own
//! `batch_lookup`s (SHOUT KEY_QUERY → whisper RDMA_REQUEST on memory hits →
//! publish on RDMA_STATUS success), and as a **server** it answers peers'
//! KEY_QUERYs and serves their RDMA_REQUESTs (delegated to [`crate::server`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender as StdSender;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use component_core::channel::mpsc::{MpscReceiver, MpscSender};
use component_core::channel::ChannelError;
use interfaces::{
    CacheKey, ControlChannel, DispatchMapError, Endpoint, IDispatchMap, ILogger, IMemoryTier,
    IZyreNode, LookupConfig, PeerId, RemoteLookupError, ResponderCommand, ResponderEvent,
    ZyreEvent,
};

use crate::operation::{KeyState, LandingSlot, Operation, PeerReply, Phase};
use crate::server;
use crate::wire::{RdmaStatusCode, SlotDesc, WireMessage};
use crate::worker::InitiatorCmd;

/// Bounded wait for a responder `DisconnectAck` after sending `Disconnect`, so
/// `teardown_peer` cannot hang the actor if the ack is lost. This is the
/// ack-handshake bound only — distinct from `connection_teardown_timeout`, which
/// is the grace before an orphan is force-torn-down in the first place.
const DISCONNECT_ACK_TIMEOUT: Duration = Duration::from_millis(500);

/// zyre ENTER header key carrying a peer's RDMA responder endpoint (`"ip:port"`),
/// used to warm a connection to that peer at discovery time (connect-hardening).
pub(crate) const RDMA_ENDPOINT_HEADER: &str = "rl_rdma_ep";

/// Positional per-key result of a `batch_lookup`, delivered back to the blocked
/// caller over the operation's one-shot channel.
pub(crate) type BatchResult = Vec<Result<(), RemoteLookupError>>;

/// The sending half of a per-op one-shot completion channel (std MPSC used as a
/// one-shot: the actor sends the results exactly once).
pub(crate) type OneShot<T> = StdSender<T>;

/// One `batch_lookup` submission handed to the actor.
pub(crate) struct OperationRequest {
    /// Monotonic operation id allocated from the component's `op_counter`.
    pub op_id: u64,
    /// The `(key, size)` entries this operation must satisfy, in caller order.
    pub entries: Vec<(CacheKey, u32)>,
    /// One-shot completion channel back to the blocked caller.
    pub done: OneShot<BatchResult>,
}

/// Messages the actor poll-loop consumes from the MPSC submission channel.
pub(crate) enum ActorMsg {
    /// A new `batch_lookup` to run.
    Submit(OperationRequest),
    /// Join the named zyre group (from `join_cluster`).
    Join(String),
    /// Leave the current zyre group (from `leave_cluster`).
    Leave,
    /// An off-loop serve finished; whisper its RDMA_STATUS back to the requester
    /// (posted by [`crate::worker`], FR-016).
    PushComplete {
        from: PeerId,
        op_id: u64,
        statuses: Vec<(CacheKey, RdmaStatusCode)>,
    },
    /// Drain in-flight work and terminate the poll loop (on deactivate).
    Shutdown,
}

/// The sending half of the actor's MPSC submission channel.
pub(crate) type SubmitSender = MpscSender<ActorMsg>;

/// Owns the spawned actor OS thread so the component can join it on deactivate.
pub(crate) struct ActorHandle {
    join: JoinHandle<()>,
}

impl ActorHandle {
    /// Wrap a spawned actor thread's join handle.
    pub(crate) fn new(join: JoinHandle<()>) -> Self {
        Self { join }
    }

    /// Block until the actor thread exits. Callers should send
    /// [`ActorMsg::Shutdown`] first so the poll loop terminates.
    pub(crate) fn join(self) {
        let _ = self.join.join();
    }
}

/// Per-key single-flight coalescing state (T017): the operation actively
/// fetching a key, plus other operations riding along on the same fetch.
struct InFlight {
    /// The op that reserved the slot and sent the RDMA_REQUEST for this key.
    serving_op: u64,
    /// Other ops waiting on the same key (deduped — no duplicate RDMA).
    followers: Vec<u64>,
}

/// A landing slot whose operation finalized before its fetch resolved, while the
/// exposed peer was still a live member. Per SC-005 it must NOT be reclaimed on
/// the timeout (a late one-sided write could still land); it is reclaimed on a
/// late RDMA_STATUS, when the peer exits (after a responder DisconnectAck), or —
/// as a backstop against a peer that neither reports nor leaves — when its
/// `deadline` elapses (force teardown-before-reclaim in [`ActorState::tick_orphans`]).
struct Orphan {
    /// The peer the slot was exposed to (its initiator may still write here).
    peer: PeerId,
    /// Force-reclaim no earlier than this instant (finalize + `connection_teardown_timeout`).
    deadline: Instant,
}

/// Resolved receptacle handles the actor needs on the poll-loop thread. The
/// server-role handles (`initiator`, `dispatcher`) live on the off-loop worker
/// instead (see [`crate::worker`]), reached via [`ActorState::initiator_tx`].
pub(crate) struct Deps {
    pub dispatch_map: Arc<dyn IDispatchMap + Send + Sync>,
    pub memory_tier: Arc<dyn IMemoryTier + Send + Sync>,
    pub logger: Option<Arc<dyn ILogger + Send + Sync>>,
}

/// Everything `initialize` hands to the spawned actor thread.
pub(crate) struct ActorInit {
    pub node: Box<dyn IZyreNode>,
    pub deps: Deps,
    /// This node's own responder endpoint (advertised in RDMA_REQUEST/KEY_RESPONSE).
    pub local_endpoint: Endpoint,
    /// This node's pool-wide rkey (advertised in RDMA_REQUEST).
    pub local_rkey: u32,
    pub group: String,
    pub config: LookupConfig,
    pub peers_seen: Arc<AtomicUsize>,
    /// Responder control channel (Disconnect → DisconnectAck) for teardown.
    pub control: ControlChannel,
    /// Sender to the off-loop initiator worker (warm + serve dispatch).
    pub initiator_tx: StdSender<InitiatorCmd>,
}

/// Live actor state, owned solely by the poll-loop thread.
struct ActorState {
    node: Box<dyn IZyreNode>,
    deps: Deps,
    control: ControlChannel,
    own_uuid: PeerId,
    local_endpoint: Endpoint,
    local_rkey: u32,
    group: String,
    config: LookupConfig,
    peers_seen: Arc<AtomicUsize>,
    /// Active client operations by `op_id`.
    ops: HashMap<u64, Operation>,
    /// Per-key single-flight index (T017): coalesces concurrent fetches of the
    /// same key onto one RDMA.
    in_flight: HashMap<CacheKey, InFlight>,
    /// Landing slots outliving their operation, awaiting late status or peer exit
    /// before physical reclaim (SC-005), keyed by `CacheKey`.
    orphans: HashMap<CacheKey, Orphan>,
    /// Sender to the off-loop initiator worker (warm + serve dispatch).
    initiator_tx: StdSender<InitiatorCmd>,
}

/// The actor poll loop (research Decision 1). Owns the started zyre node (which
/// is `Send + !Sync`, so it must live on this one thread).
pub(crate) fn run(init: ActorInit, rx: MpscReceiver<ActorMsg>) {
    let own_uuid = init.node.uuid();
    let mut state = ActorState {
        node: init.node,
        deps: init.deps,
        control: init.control,
        own_uuid,
        local_endpoint: init.local_endpoint,
        local_rkey: init.local_rkey,
        group: init.group,
        config: init.config,
        peers_seen: init.peers_seen,
        ops: HashMap::new(),
        in_flight: HashMap::new(),
        orphans: HashMap::new(),
        initiator_tx: init.initiator_tx,
    };

    if let Some(logger) = &state.deps.logger {
        logger.debug("remote-lookup: actor poll loop started");
    }

    // Let submission senders unpark us so a Submit is picked up promptly.
    rx.register_for_unpark();

    loop {
        let mut idle = true;

        // 1. Drain caller submissions.
        loop {
            match rx.try_recv() {
                Ok(msg) => {
                    idle = false;
                    match msg {
                        ActorMsg::Shutdown => {
                            state.shutdown();
                            return;
                        }
                        ActorMsg::Join(g) => {
                            let _ = state.node.join(&g);
                        }
                        ActorMsg::Leave => {
                            let _ = state.node.leave(&state.group);
                        }
                        ActorMsg::Submit(req) => state.on_submit(req),
                        ActorMsg::PushComplete {
                            from,
                            op_id,
                            statuses,
                        } => state.on_push_complete(from, op_id, statuses),
                    }
                }
                Err(ChannelError::Empty) => break,
                Err(ChannelError::Closed) => {
                    state.shutdown();
                    return;
                }
                Err(_) => break,
            }
        }

        // 2. Drain inbound zyre events.
        while let Ok(Some(ev)) = state.node.try_recv() {
            idle = false;
            match ev {
                ZyreEvent::Enter { headers, .. } => {
                    state.peers_seen.fetch_add(1, Ordering::Relaxed);
                    state.on_enter(&headers);
                }
                ZyreEvent::Exit { peer, .. } => {
                    let _ =
                        state
                            .peers_seen
                            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                                Some(v.saturating_sub(1))
                            });
                    state.on_exit(peer);
                }
                ZyreEvent::Shout { peer, message, .. } => state.on_wire(peer, &message),
                ZyreEvent::Whisper { peer, message, .. } => state.on_wire(peer, &message),
                ZyreEvent::Stop => return,
                _ => {}
            }
        }

        // 3. Finalize operations whose deadline has elapsed.
        if state.tick_deadlines() {
            idle = false;
        }

        // 3b. Force-reclaim orphaned slots whose teardown grace has elapsed
        // (backstop for a peer that neither reports a late status nor exits).
        if state.tick_orphans() {
            idle = false;
        }

        // 4. Bounded sleep only when fully idle.
        if idle {
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
}

impl ActorState {
    /// Start a new client operation: snapshot the group size, set the deadline,
    /// and SHOUT the KEY_QUERY (split under `max_keys_per_query`) — FR-005.
    fn on_submit(&mut self, req: OperationRequest) {
        let peers_expected = self.node.peers_by_group(&self.group).len();
        let now = Instant::now();
        let deadline = now + self.config.op_deadline;
        let phase1_deadline = now + self.config.phase1_timeout;
        let op = Operation::new(
            req.entries.clone(),
            deadline,
            phase1_deadline,
            peers_expected,
            req.done,
        );
        self.ops.insert(req.op_id, op);

        let chunk = self.config.max_keys_per_query.max(1);
        for entries in req.entries.chunks(chunk) {
            let msg = WireMessage::KeyQuery {
                op_id: req.op_id,
                entries: entries.to_vec(),
            };
            let _ = self.node.shout(&self.group, &msg.encode());
        }

        // A node with no peers can never be satisfied remotely — finalize now
        // rather than block the caller for the full deadline.
        if peers_expected == 0 {
            self.finalize(req.op_id);
        }
    }

    /// Decode and route one inbound frame (FR-018: unknown frames are ignored).
    fn on_wire(&mut self, from: PeerId, bytes: &[u8]) {
        let msg = match WireMessage::decode(bytes) {
            Ok(m) => m,
            Err(e) => {
                // FR-018(b): malformed/truncated frame — log then drop; the poll
                // loop continues. Logged so a framing mismatch is diagnosable.
                if let Some(logger) = &self.deps.logger {
                    logger.debug(&format!(
                        "remote-lookup: dropping malformed frame from {from} ({} bytes): {e:?}",
                        bytes.len()
                    ));
                }
                return;
            }
        };
        match msg {
            WireMessage::KeyQuery { op_id, entries } => self.handle_key_query(from, op_id, entries),
            WireMessage::KeyResponse {
                op_id,
                endpoint,
                entries,
            } => self.on_key_response(from, op_id, endpoint, entries),
            WireMessage::RdmaRequest {
                op_id,
                endpoint,
                rkey,
                slots,
            } => self.handle_rdma_request(from, op_id, endpoint, rkey, slots),
            WireMessage::RdmaStatus { op_id, entries } => self.on_rdma_status(from, op_id, entries),
            // FR-018(a): unrecognized `msg_type` — log then ignore.
            WireMessage::Unknown {
                version,
                msg_type,
                op_id,
            } => {
                if let Some(logger) = &self.deps.logger {
                    logger.debug(&format!(
                        "remote-lookup: ignoring unknown frame from {from} \
                         (version={version}, msg_type={msg_type}, op_id={op_id})"
                    ));
                }
            }
        }
    }

    // --- server role -------------------------------------------------------

    /// Answer a peer's KEY_QUERY (US2). Ignores our own SHOUT (FR-021).
    fn handle_key_query(&mut self, from: PeerId, op_id: u64, entries: Vec<(CacheKey, u32)>) {
        if from == self.own_uuid {
            return;
        }
        let classified = server::classify_query(&self.deps.dispatch_map, &entries);
        let resp = WireMessage::KeyResponse {
            op_id,
            endpoint: self.local_endpoint.clone(),
            entries: classified,
        };
        let _ = self.node.whisper(&from, &resp.encode());
    }

    /// Serve a peer's RDMA_REQUEST (US3). The pin + one-sided write (which may
    /// block on a cold RDMA connect) runs on the off-loop worker; the poll loop
    /// stays responsive. The worker posts the result back as
    /// [`ActorMsg::PushComplete`], handled by [`on_push_complete`], which
    /// whispers the RDMA_STATUS. If the worker channel is closed (shutting down),
    /// the request is dropped — the requester's `op_deadline` backstops it.
    fn handle_rdma_request(
        &mut self,
        from: PeerId,
        op_id: u64,
        requester_endpoint: Endpoint,
        rkey: u32,
        slots: Vec<SlotDesc>,
    ) {
        let _ = self.initiator_tx.send(InitiatorCmd::Serve {
            from,
            op_id,
            requester_endpoint,
            rkey,
            slots,
        });
    }

    /// Whisper the RDMA_STATUS for a serve the worker just finished (FR-016).
    fn on_push_complete(
        &mut self,
        from: PeerId,
        op_id: u64,
        statuses: Vec<(CacheKey, RdmaStatusCode)>,
    ) {
        let resp = WireMessage::RdmaStatus {
            op_id,
            entries: statuses,
        };
        let _ = self.node.whisper(&from, &resp.encode());
    }

    /// On peer discovery (connect-hardening): if the peer advertised its RDMA
    /// responder endpoint, dispatch an off-loop warm-connect so a later serve to
    /// that peer skips the cold `rdma_cm` connect. No header ⇒ nothing to warm
    /// (the serve connects lazily, as before). This node connects *out* to a peer
    /// only when serving that peer's RDMA_REQUEST, so warming every discovered
    /// peer's responder covers whichever peers later request from us.
    fn on_enter(&mut self, headers: &HashMap<String, String>) {
        if let Some(endpoint) = headers.get(RDMA_ENDPOINT_HEADER) {
            let _ = self.initiator_tx.send(InitiatorCmd::Warm {
                endpoint: endpoint.clone(),
            });
        }
    }

    // --- client role -------------------------------------------------------

    /// Process a peer's KEY_RESPONSE (US1): greedily fetch this peer's memory
    /// hits (reserve a landing slot + whisper one RDMA_REQUEST) — FR-006/007.
    fn on_key_response(
        &mut self,
        from: PeerId,
        op_id: u64,
        _responder_endpoint: Endpoint,
        entries: Vec<(CacheKey, u32, crate::wire::Avail)>,
    ) {
        let Some(mut op) = self.ops.remove(&op_id) else {
            return; // stale / unknown op_id (FR-019)
        };

        if !op.replies.contains_key(&from) {
            op.peers_replied += 1;
        }

        let mut slots: Vec<SlotDesc> = Vec::new();
        for &(key, size, avail) in &entries {
            let wants_memory = avail == crate::wire::Avail::Memory
                && op.state_of(key) == KeyState::Unsatisfied
                && op.size_of(key) == Some(size);
            if wants_memory {
                // Single-flight (T017): if another op is already fetching this
                // key, ride along as a follower — no duplicate slot or RDMA.
                if let Some(inf) = self.in_flight.get_mut(&key) {
                    if inf.serving_op != op_id && !inf.followers.contains(&op_id) {
                        inf.followers.push(op_id);
                    }
                    op.set_state(key, KeyState::InProgress);
                    continue;
                }
                // Orphan-reuse guard (memory safety): a prior op's landing slot
                // for this key may still be orphaned and exposed to a peer that
                // could DMA into it. Reserving the key again risks aliasing that
                // buffer, which the orphan's later teardown-reclaim would free out
                // from under this op. Leave the key Unsatisfied this round — the
                // orphan is reclaimed on teardown and a later lookup re-fetches.
                if self.orphans.contains_key(&key) {
                    continue;
                }
                match self.deps.memory_tier.insert(key, size) {
                    Ok(ptr) => {
                        let addr = ptr as u64;
                        op.slots.insert(
                            key,
                            LandingSlot {
                                addr,
                                len: size,
                                peer: from.clone(),
                            },
                        );
                        op.set_state(key, KeyState::InProgress);
                        slots.push(SlotDesc {
                            key,
                            addr,
                            length: size,
                        });
                        self.in_flight.insert(
                            key,
                            InFlight {
                                serving_op: op_id,
                                followers: Vec::new(),
                            },
                        );
                    }
                    Err(_) => { /* slot reservation failed: leave Unsatisfied */ }
                }
            }
        }

        // Cache the reply for Phase-2/retry (US4/US5).
        op.replies.insert(from.clone(), PeerReply { entries });

        if !slots.is_empty() {
            let msg = WireMessage::RdmaRequest {
                op_id,
                endpoint: self.local_endpoint.clone(),
                rkey: self.local_rkey,
                slots,
            };
            let _ = self.node.whisper(&from, &msg.encode());
        }

        self.reinsert_or_finalize(op_id, op);
    }

    /// Process a peer's RDMA_STATUS (US1): publish successes (publish-on-success,
    /// FR-008), reclaim failed slots (FR-009).
    fn on_rdma_status(
        &mut self,
        from: PeerId,
        op_id: u64,
        entries: Vec<(CacheKey, RdmaStatusCode)>,
    ) {
        let Some(mut op) = self.ops.remove(&op_id) else {
            // Late status for a finalized op (FR-019). If we orphaned a slot for
            // one of these keys exposed to this peer, the peer has now finished
            // writing — reclaim it (SC-005).
            for (key, _code) in entries {
                if self.orphans.get(&key).map(|o| o.peer.clone()) == Some(from.clone()) {
                    self.orphans.remove(&key);
                    let _ = self.deps.memory_tier.remove(key);
                }
            }
            return;
        };

        // (key, satisfied) outcomes to propagate to any single-flight followers.
        let mut resolved: Vec<(CacheKey, bool)> = Vec::new();
        for (key, code) in entries {
            match code {
                RdmaStatusCode::Success => {
                    self.publish_success(&mut op, key);
                    resolved.push((key, op.state_of(key) == KeyState::Satisfied));
                }
                RdmaStatusCode::UnableToConnect | RdmaStatusCode::KeyNoLongerAvailable => {
                    // Failed fetch: reclaim the private slot, return to
                    // Unsatisfied, and remember not to retry this peer (US5).
                    if op.slots.remove(&key).is_some() {
                        let _ = self.deps.memory_tier.remove(key);
                    }
                    op.set_state(key, KeyState::Unsatisfied);
                    op.note_tried(key, &from);
                    // Re-target to an alternate cached peer (US5, FR-011). If we
                    // retried, the key is back in flight (keep its in-flight
                    // entry + followers); otherwise it is terminally failed.
                    if !self.try_retry(op_id, &mut op, key) {
                        resolved.push((key, false));
                    }
                }
            }
        }

        self.ops.insert(op_id, op);

        // Propagate each outcome to the key's single-flight followers, then clear
        // the in-flight entry (T017).
        for (key, satisfied) in resolved {
            if let Some(inf) = self.in_flight.remove(&key) {
                let new_state = if satisfied {
                    KeyState::Satisfied
                } else {
                    KeyState::Unsatisfied
                };
                for follower in inf.followers {
                    if let Some(fop) = self.ops.get_mut(&follower) {
                        fop.set_state(key, new_state);
                    }
                }
            }
        }

        self.check_all_completions();
    }

    /// Publish a successfully-fetched key to the dispatch map (publish-on-success
    /// with the `AlreadyExists` size-check, T016 / `size-mismatch-handling.md`).
    fn publish_success(&mut self, op: &mut Operation, key: CacheKey) {
        let Some(slot) = op.slots.get(&key) else {
            return;
        };
        let ptr = slot.addr as *mut u8;
        let len = slot.len;
        match self
            .deps
            .dispatch_map
            .create_memory_tier_entry(key, ptr, len)
        {
            Ok(()) => {
                let _ = self.deps.dispatch_map.release_write(key);
                op.set_state(key, KeyState::Satisfied);
            }
            Err(DispatchMapError::AlreadyExists(_)) => {
                // Someone already published this key. Genuine success only if the
                // existing entry matches our size; a different size is a key
                // collision — treat as unsatisfied and reclaim, never evict.
                if matches!(self.deps.dispatch_map.entry_size(key), Ok(sz) if sz == len) {
                    op.set_state(key, KeyState::Satisfied);
                } else {
                    if op.slots.remove(&key).is_some() {
                        let _ = self.deps.memory_tier.remove(key);
                    }
                    op.set_state(key, KeyState::Unsatisfied);
                }
            }
            Err(_) => op.set_state(key, KeyState::Unsatisfied),
        }
    }

    /// Re-target a failed key to an alternate cached peer that reported it in
    /// memory and has not been tried in this operation (US5, FR-011). Bounded by
    /// `max_retry_rounds` attempts per key. On success reserves a fresh landing
    /// slot, whispers a new RDMA_REQUEST, and returns `true` (the key is back
    /// in flight); returns `false` when the budget is spent or no alternate
    /// remains. (Disk holders are skipped — serving from disk is US4.)
    fn try_retry(&mut self, op_id: u64, op: &mut Operation, key: CacheKey) -> bool {
        let Some(size) = op.size_of(key) else {
            return false;
        };
        // Bound the number of peers tried per key.
        let attempts = op.tried.get(&key).map_or(0, |v| v.len());
        if attempts > self.config.max_retry_rounds as usize {
            return false;
        }
        // Find an untried peer that reported this key — memory preferred, then
        // disk (served via promotion, US4).
        let find = |want: crate::wire::Avail| {
            op.replies.iter().find_map(|(peer, reply)| {
                if op.already_tried(key, peer) {
                    return None;
                }
                let has = reply
                    .entries
                    .iter()
                    .any(|(k, s, a)| *k == key && *s == size && *a == want);
                has.then(|| peer.clone())
            })
        };
        let Some(alternate) =
            find(crate::wire::Avail::Memory).or_else(|| find(crate::wire::Avail::Disk))
        else {
            return false;
        };

        // Orphan-reuse guard (see `on_key_response`): don't reserve a key whose
        // prior landing slot is still orphaned and exposed to a peer.
        if self.orphans.contains_key(&key) {
            return false;
        }
        let Ok(ptr) = self.deps.memory_tier.insert(key, size) else {
            return false;
        };
        let addr = ptr as u64;
        op.slots.insert(
            key,
            LandingSlot {
                addr,
                len: size,
                peer: alternate.clone(),
            },
        );
        op.set_state(key, KeyState::InProgress);
        // The key stays in-flight (serving_op is still this op); its followers
        // keep waiting on the retry.
        let msg = WireMessage::RdmaRequest {
            op_id,
            endpoint: self.local_endpoint.clone(),
            rkey: self.local_rkey,
            slots: vec![SlotDesc {
                key,
                addr,
                length: size,
            }],
        };
        let _ = self.node.whisper(&alternate, &msg.encode());
        true
    }

    // --- completion --------------------------------------------------------

    /// Re-insert the operation, then advance it toward completion.
    fn reinsert_or_finalize(&mut self, op_id: u64, op: Operation) {
        self.ops.insert(op_id, op);
        self.advance(op_id);
    }

    /// Advance every operation (used after cross-op state changes such as
    /// single-flight follower resolution, where more than one op may progress).
    fn check_all_completions(&mut self) {
        for id in self.ops.keys().copied().collect::<Vec<_>>() {
            self.advance(id);
        }
    }

    /// Drive one operation toward completion (FR-010/FR-012): finalize when all
    /// keys are satisfied; transition Phase-1 → Phase-2 once a quorum of peers has
    /// replied or the Phase-1 timeout elapses; in Phase 2 re-scan cached replies
    /// for disk holders and finalize when nothing remains in flight.
    fn advance(&mut self, op_id: u64) {
        let (all_satisfied, phase, quorum, past_phase1) = match self.ops.get(&op_id) {
            Some(op) => (
                op.all_satisfied(),
                op.phase,
                op.quorum_reached(self.config.quorum_pct),
                Instant::now() >= op.phase1_deadline,
            ),
            None => return,
        };
        if all_satisfied {
            self.finalize(op_id);
            return;
        }

        // Phase-1 → Phase-2 transition (once), on quorum or Phase-1 timeout (FR-010).
        let phase = if phase == Phase::Phase1 && (quorum || past_phase1) {
            if let Some(op) = self.ops.get_mut(&op_id) {
                op.phase = Phase::Phase2;
            }
            Phase::Phase2
        } else {
            phase
        };
        if phase != Phase::Phase2 {
            return; // still Phase 1: keep collecting replies / memory hits
        }

        // Phase 2: (re)scan cached replies for disk holders of still-unsatisfied
        // keys (idempotent — only untried, not-in-flight holders are fetched),
        // then finalize once every expected peer has replied and nothing is left
        // in flight (FR-012 — a not-yet-replied peer may still hold a key; the
        // `op_deadline` backstops a peer that never replies).
        self.try_phase2(op_id);
        let done = self
            .ops
            .get(&op_id)
            .map(|op| {
                let in_flight = op.state.values().any(|s| *s == KeyState::InProgress);
                op.peers_replied >= op.peers_expected && !in_flight
            })
            .unwrap_or(false);
        if done {
            self.finalize(op_id);
        }
    }

    /// Phase-2 disk re-scan (US4, FR-010): for still-unsatisfied keys, fetch from
    /// a cached peer that reported the key on **disk** (the serving peer promotes
    /// it). No new SHOUT — only cached replies are consulted. Returns whether any
    /// fetch was launched.
    fn try_phase2(&mut self, op_id: u64) -> bool {
        let Some(mut op) = self.ops.remove(&op_id) else {
            return false;
        };
        let mut launched = false;
        let unsatisfied: Vec<CacheKey> = op
            .entries
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| op.state_of(*k) == KeyState::Unsatisfied)
            .collect();
        for key in unsatisfied {
            let Some(size) = op.size_of(key) else {
                continue;
            };
            // Single-flight: ride along if another op is already fetching it.
            if let Some(inf) = self.in_flight.get_mut(&key) {
                if inf.serving_op != op_id && !inf.followers.contains(&op_id) {
                    inf.followers.push(op_id);
                }
                op.set_state(key, KeyState::InProgress);
                launched = true;
                continue;
            }
            // An untried cached disk holder.
            let disk_peer = op.replies.iter().find_map(|(peer, reply)| {
                if op.already_tried(key, peer) {
                    return None;
                }
                let has_disk = reply
                    .entries
                    .iter()
                    .any(|(k, s, a)| *k == key && *s == size && *a == crate::wire::Avail::Disk);
                has_disk.then(|| peer.clone())
            });
            let Some(peer) = disk_peer else {
                continue;
            };
            // Orphan-reuse guard (see `on_key_response`).
            if self.orphans.contains_key(&key) {
                continue;
            }
            let Ok(ptr) = self.deps.memory_tier.insert(key, size) else {
                continue;
            };
            let addr = ptr as u64;
            op.slots.insert(
                key,
                LandingSlot {
                    addr,
                    len: size,
                    peer: peer.clone(),
                },
            );
            op.set_state(key, KeyState::InProgress);
            self.in_flight.insert(
                key,
                InFlight {
                    serving_op: op_id,
                    followers: Vec::new(),
                },
            );
            let msg = WireMessage::RdmaRequest {
                op_id,
                endpoint: self.local_endpoint.clone(),
                rkey: self.local_rkey,
                slots: vec![SlotDesc {
                    key,
                    addr,
                    length: size,
                }],
            };
            let _ = self.node.whisper(&peer, &msg.encode());
            launched = true;
        }
        self.ops.insert(op_id, op);
        launched
    }

    /// Time-driven progress: finalize operations past their `op_deadline`, and
    /// fire the Phase-1 → Phase-2 transition for operations whose `phase1_timeout`
    /// has elapsed with no quorum yet (FR-010). Returns whether anything fired.
    fn tick_deadlines(&mut self) -> bool {
        let now = Instant::now();
        let expired: Vec<u64> = self
            .ops
            .iter()
            .filter(|(_, op)| now >= op.deadline)
            .map(|(id, _)| *id)
            .collect();
        for op_id in &expired {
            self.finalize(*op_id);
        }
        // Ops still in Phase 1 whose Phase-1 timeout has elapsed — advance them so
        // the disk re-scan fires even without further inbound events.
        let transition: Vec<u64> = self
            .ops
            .iter()
            .filter(|(_, op)| op.phase == Phase::Phase1 && now >= op.phase1_deadline)
            .map(|(id, _)| *id)
            .collect();
        for op_id in &transition {
            self.advance(*op_id);
        }
        !expired.is_empty() || !transition.is_empty()
    }

    /// Backstop reclaim for orphaned landing slots whose teardown grace has
    /// elapsed (SC-005). Orphans are normally reclaimed by a late RDMA_STATUS or
    /// the peer's exit; this covers a peer that neither reports nor leaves, which
    /// would otherwise leak the buffer forever. Expired orphans are grouped by
    /// peer so each peer is torn down at most once (bounding the per-tick
    /// DisconnectAck wait), and reclaim strictly follows teardown — the peer's QP
    /// is severed (Disconnect → DisconnectAck) before any buffer it could DMA into
    /// is freed, exactly as [`Self::on_exit`] does. A peer a live operation still
    /// depends on is not severed; its orphans wait for a later tick.
    fn tick_orphans(&mut self) -> bool {
        let now = Instant::now();
        let mut peers: Vec<PeerId> = Vec::new();
        for orphan in self.orphans.values() {
            if now >= orphan.deadline && !peers.contains(&orphan.peer) {
                peers.push(orphan.peer.clone());
            }
        }

        let mut reclaimed = false;
        for peer in peers {
            // Never sever a peer a live op is still fetching from — that would
            // abort its in-flight write. Defer; a later tick retries.
            let peer_in_use = self
                .ops
                .values()
                .any(|op| op.slots.values().any(|s| s.peer == peer));
            if peer_in_use {
                continue;
            }
            // Teardown-before-reclaim: the peer's QP is in ERROR (no more DMA)
            // before we free any buffer it was exposed to.
            self.teardown_peer(&peer);
            let keys: Vec<CacheKey> = self
                .orphans
                .iter()
                .filter(|(_, o)| o.peer == peer && now >= o.deadline)
                .map(|(k, _)| *k)
                .collect();
            for key in keys {
                self.orphans.remove(&key);
                let _ = self.deps.memory_tier.remove(key);
                reclaimed = true;
            }
        }
        reclaimed
    }

    /// Finalize an operation: reclaim unpublished landing slots, deliver the
    /// positional result vector to the blocked caller, and drop the state.
    fn finalize(&mut self, op_id: u64) {
        let Some(mut op) = self.ops.remove(&op_id) else {
            return;
        };
        // Clear single-flight state owned by this op (T017): keys it was serving
        // lose their server (in-progress followers fall back to Unsatisfied), and
        // it drops out of any follower lists.
        let served: Vec<CacheKey> = self
            .in_flight
            .iter()
            .filter(|(_, inf)| inf.serving_op == op_id)
            .map(|(k, _)| *k)
            .collect();
        for key in served {
            if let Some(inf) = self.in_flight.remove(&key) {
                for follower in inf.followers {
                    if let Some(fop) = self.ops.get_mut(&follower) {
                        if fop.state_of(key) == KeyState::InProgress {
                            fop.set_state(key, KeyState::Unsatisfied);
                        }
                    }
                }
            }
        }
        for inf in self.in_flight.values_mut() {
            inf.followers.retain(|f| *f != op_id);
        }
        // SC-005: a slot exposed to a still-live peer must not be reclaimed on a
        // timeout — a late one-sided write could still land. Orphan any
        // unpublished slot (reclaimed later on a late RDMA_STATUS or peer exit)
        // rather than freeing it here.
        let exposed: Vec<CacheKey> = op
            .slots
            .keys()
            .copied()
            .filter(|k| op.state_of(*k) != KeyState::Satisfied)
            .collect();
        let orphan_deadline = Instant::now() + self.config.connection_teardown_timeout;
        for key in exposed {
            if let Some(slot) = op.slots.remove(&key) {
                self.orphans.insert(
                    key,
                    Orphan {
                        peer: slot.peer,
                        deadline: orphan_deadline,
                    },
                );
            }
        }
        let results = op.results();
        if let Some(done) = op.done.take() {
            let _ = done.send(results);
        }
    }

    // --- peer departure (US7) ---------------------------------------------

    /// Handle a peer's zyre `Exit` (FR-013/FR-014): drop its cached replies,
    /// return keys it was fetching to Unsatisfied, then — for any landing slot
    /// exposed to it (active or orphaned) — tear the QP down and await the
    /// DisconnectAck before reclaiming (teardown-before-reclaim, SC-005/SC-006).
    fn on_exit(&mut self, peer: PeerId) {
        // Drop the departed peer's cached replies everywhere (FR-013).
        for op in self.ops.values_mut() {
            op.replies.remove(&peer);
        }

        // Slots exposed to the departed peer, in active ops and among orphans.
        let exposed_in_ops: Vec<(u64, CacheKey)> = self
            .ops
            .iter()
            .flat_map(|(op_id, op)| {
                op.slots
                    .iter()
                    .filter(|(_, slot)| slot.peer == peer)
                    .map(move |(key, _)| (*op_id, *key))
            })
            .collect();
        let exposed_orphans: Vec<CacheKey> = self
            .orphans
            .iter()
            .filter(|(_, o)| o.peer == peer)
            .map(|(k, _)| *k)
            .collect();

        // Return in-op keys to Unsatisfied now (waiters may finalize not-found
        // immediately); only the physical reclaim waits on the ack.
        for (op_id, key) in &exposed_in_ops {
            if let Some(op) = self.ops.get_mut(op_id) {
                op.set_state(*key, KeyState::Unsatisfied);
            }
            // Single-flight: if this op was serving the key, its followers lose
            // their server too.
            if self.in_flight.get(key).map(|i| i.serving_op) == Some(*op_id) {
                if let Some(inf) = self.in_flight.remove(key) {
                    for follower in inf.followers {
                        if let Some(fop) = self.ops.get_mut(&follower) {
                            fop.set_state(*key, KeyState::Unsatisfied);
                        }
                    }
                }
            }
        }

        // Teardown-before-reclaim (FR-014): sever the QP and wait for the ack,
        // then return the exposed slots to the allocator.
        if !exposed_in_ops.is_empty() || !exposed_orphans.is_empty() {
            self.teardown_peer(&peer);
            for (op_id, key) in &exposed_in_ops {
                if let Some(op) = self.ops.get_mut(op_id) {
                    op.slots.remove(key);
                }
                let _ = self.deps.memory_tier.remove(*key);
            }
            for key in &exposed_orphans {
                self.orphans.remove(key);
                let _ = self.deps.memory_tier.remove(*key);
            }
        }

        self.check_all_completions();
    }

    /// Sever the RDMA connection to `peer` and block (bounded) for the
    /// responder's `DisconnectAck`, so the peer's queue pair is in ERROR before
    /// any slot it could write to is reclaimed (SC-005).
    fn teardown_peer(&mut self, peer: &PeerId) {
        let _ = self
            .control
            .command_tx
            .send(ResponderCommand::Disconnect { node: peer.clone() });
        let deadline = Instant::now() + DISCONNECT_ACK_TIMEOUT;
        loop {
            match self.control.event_rx.try_recv() {
                Ok(ResponderEvent::DisconnectAck { node }) if &node == peer => break,
                Ok(_) => {} // unrelated event: keep draining
                Err(ChannelError::Empty) => {
                    if Instant::now() >= deadline {
                        break; // give up rather than hang the actor
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break, // channel closed
            }
        }
    }

    /// Leave the group, reclaim all in-flight slots, and stop the zyre node.
    fn shutdown(&mut self) {
        let ids: Vec<u64> = self.ops.keys().copied().collect();
        for id in ids {
            self.finalize(id);
        }
        let _ = self.node.leave(&self.group);
        self.node.stop();
    }
}
