//! The per-connection service: apply the generator's turn to the local mailbox.
//!
//! # What the agent decides, and what it does not
//!
//! It decides **nothing about the workload**. Which keys, in which order, for which session,
//! at which virtual time — all of that arrives in a `SubmitTurn` and comes from the
//! generator's single simulation core (FR-072). What the agent applies is the *client rule*:
//! check the path, load what is resident, store what is absent. That is a mechanical
//! consequence of what the cache reports, not a choice about the workload.
//!
//! And it applies that rule by calling [`workload_gen::exec::TurnExecutor`], the same code
//! the generator's local path runs. There is deliberately no second implementation here: two
//! would be free to drift, and a fix applied on one side and forgotten on the other would
//! surface as a local/remote difference that looked like a property of the network.
//!
//! # One connection, one channel, one executor
//!
//! A connection is a lane. The mailbox is depth-1 per channel, so a lane claims its own
//! channel — sharing one would serialise lanes while still reporting the concurrency that was
//! asked for. Each connection therefore gets its own channel, its own payload slot and its own
//! executor, and [`AgentFactory`] refuses a connection it cannot equip rather than serving it
//! badly.
//!
//! Because `Service` takes `&mut self`, one connection's frames are handled one at a time in
//! arrival order, which is what keeps a session's causally dependent turns from overlapping.
//!
//! **A closed connection returns its channel and slot to the pool.** The first version did
//! not, and a live run found it immediately: an agent could serve only `--lanes` connections
//! *in its whole lifetime*, so the third client of a two-lane agent was refused and saw a
//! connection reset. An agent is long-lived and a generator may reconnect — between runs, or
//! after a lane is restarted — so holding a channel past the connection that used it is a
//! leak of exactly the kind FR-052 and FR-053 are about.
//!
//! # Measurement happens here, and travels back for reporting only
//!
//! The agent is the only thing near this mailbox, so it is the only thing that can time a
//! `LOOKUP` or count a block that moved. `Stats` returns those counters and the per-operation
//! **histograms** — never percentiles, because the median of two nodes' medians is not a
//! median. Nothing the agent does depends on them.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use hdrhistogram::serialization::{Serializer, V2Serializer};
use shm_queue::Client;
use workload_gen::exec::TurnExecutor;
use workload_gen::payload::PayloadBuffer;
use workload_wire::frame::{
    DrainAck, Hello, HelloAck, OpHistogram, ShutdownAck, Stats, SubmitTurn, TurnOutcome,
};
use workload_wire::handshake;
use workload_wire::server::{Service, ServiceFactory};

/// Channels and payload slots a connection borrows and must give back.
///
/// Shared with every [`Agent`] so a closed connection can return what it held; see the module
/// docs on the leak this replaced.
#[derive(Debug, Default)]
struct Pool {
    /// Channels not currently held by a connection.
    channels: Vec<usize>,
    /// Payload slots, paired with channels.
    slots: Vec<usize>,
}

/// Everything a connection needs, and the pool it draws from.
pub struct AgentFactory {
    client: Arc<Client>,
    payload: Option<Arc<PayloadBuffer>>,
    pool: Arc<Mutex<Pool>>,
    block_bytes: u32,
    batch_keys: usize,
    channels: u32,
    /// Requests issued to the mailbox across every connection, for `Shutdown`'s tally.
    submitted: Arc<AtomicU64>,
    /// Requests that failed.
    failed: Arc<AtomicU64>,
}

impl std::fmt::Debug for AgentFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentFactory")
            .field("channels", &self.channels)
            .field("block_bytes", &self.block_bytes)
            .field("batch_keys", &self.batch_keys)
            .field("moves_data", &self.payload.is_some())
            .finish()
    }
}

impl AgentFactory {
    /// Claim `lanes` channels on the local mailbox and prepare to serve that many connections.
    ///
    /// # Errors
    ///
    /// If the mailbox is absent or the channels cannot be claimed. A missing mailbox is a
    /// startup failure rather than a run that quietly measures nothing.
    pub fn new(
        client: Arc<Client>,
        channels: Vec<usize>,
        payload: Option<Arc<PayloadBuffer>>,
        block_bytes: u32,
        batch_keys: usize,
    ) -> Self {
        let total = client.channel_count() as u32;
        let slots = (0..channels.len()).collect();
        Self {
            client,
            payload,
            pool: Arc::new(Mutex::new(Pool { channels, slots })),
            block_bytes,
            batch_keys,
            channels: total,
            submitted: Arc::new(AtomicU64::new(0)),
            failed: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl ServiceFactory for AgentFactory {
    type Service = Agent;

    fn accept(&self) -> io::Result<Agent> {
        let (channel, slot) = {
            let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
            let channel = pool.channels.pop().ok_or_else(|| {
                io::Error::other(
                    "no mailbox channel free for this connection; the mailbox is depth-1 per \
                     channel, so serving it on a shared channel would serialise the lane \
                     while still reporting the concurrency that was asked for",
                )
            })?;
            let slot = pool.slots.pop().unwrap_or(0);
            (channel, slot)
        };
        let mut exec = match TurnExecutor::new(self.block_bytes, self.batch_keys) {
            Ok(e) => e,
            Err(e) => {
                // Give the borrowed resources back before failing, or a refused connection
                // would cost the agent a channel permanently.
                let mut pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
                pool.channels.push(channel);
                pool.slots.push(slot);
                return Err(io::Error::other(e));
            }
        };
        if let Some(buffer) = self.payload.clone() {
            exec = exec.with_payload(buffer, slot);
        }
        Ok(Agent {
            client: Arc::clone(&self.client),
            channel,
            slot,
            pool: Arc::clone(&self.pool),
            exec,
            channels: self.channels,
            block_bytes: self.block_bytes,
            submitted: Arc::clone(&self.submitted),
            failed: Arc::clone(&self.failed),
        })
    }
}

/// One connection's handler.
pub struct Agent {
    client: Arc<Client>,
    channel: usize,
    slot: usize,
    pool: Arc<Mutex<Pool>>,
    exec: TurnExecutor,
    channels: u32,
    block_bytes: u32,
    submitted: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("channel", &self.channel)
            .finish()
    }
}

impl Service for Agent {
    fn hello(&mut self, hello: &Hello) -> HelloAck {
        // The agent checks provenance too, so a mismatch is reported from both ends and the
        // operator learns which side disagreed rather than only that someone did.
        handshake::answer(hello, self.channels, self.block_bytes)
    }

    fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome {
        let started = std::time::Instant::now();
        let before = self.exec.counters().requests;
        let result = self.exec.run_turn(
            &self.client,
            self.channel,
            turn.session,
            &turn.path,
            turn.polls_events(),
        );
        let issued = self.exec.counters().requests - before;
        self.submitted.fetch_add(issued, Ordering::Relaxed);
        match result {
            Ok(r) => TurnOutcome {
                resident: r.resident,
                pending: r.pending,
                missing: r.missing,
                granted: r.granted,
                blocks_read: r.blocks_read,
                blocks_written: r.blocks_written,
                elapsed_ns: started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
            },
            Err(e) => {
                // Reported and counted, not hidden: the generator sees a turn that moved
                // nothing, and `Shutdown`'s tally carries the failure count.
                eprintln!("channel {}: {e}", self.channel);
                self.failed.fetch_add(1, Ordering::Relaxed);
                TurnOutcome {
                    elapsed_ns: started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
                    ..Default::default()
                }
            }
        }
    }

    fn drain(&mut self) -> DrainAck {
        // Requests are synchronous against the mailbox, so nothing outstanding can remain by
        // the time a frame is being answered.
        DrainAck { pending: 0 }
    }

    fn stats(&mut self) -> Stats {
        Stats {
            counters: *self.exec.counters(),
            ops: serialize_histograms(&mut self.exec),
        }
    }

    fn shutdown(&mut self) -> ShutdownAck {
        ShutdownAck {
            ops_submitted: self.submitted.load(Ordering::Relaxed),
            ops_failed: self.failed.load(Ordering::Relaxed),
        }
    }
}

/// Serialize each opcode's histogram for the wire.
///
/// Histograms rather than percentiles: quantiles do not merge, so the generator must receive
/// the distribution and take its quantiles only after merging every node's. A histogram that
/// cannot be serialized is dropped with a note rather than failing the run — the counters
/// beside it are still true, and losing a latency distribution is not worth aborting a
/// measurement for.
fn serialize_histograms(exec: &mut TurnExecutor) -> Vec<OpHistogram> {
    let mut out = Vec::new();
    let mut ser = V2Serializer::new();
    for (opcode, hist) in exec.latency_by_op() {
        let mut bytes = Vec::new();
        match ser.serialize(hist, &mut bytes) {
            Ok(_) => out.push(OpHistogram {
                op_kind: u8::try_from(*opcode).unwrap_or(u8::MAX),
                requests: hist.len(),
                histogram: bytes,
            }),
            Err(e) => eprintln!("dropping opcode {opcode}'s histogram: {e}"),
        }
    }
    out
}

impl Drop for Agent {
    fn drop(&mut self) {
        // Hand the channel and the payload slot back, so a long-lived agent can serve more
        // connections than it has lanes. Without this an agent could serve `--lanes`
        // connections in its whole lifetime and then refuse every client with a connection
        // reset — which is how a live run found the leak.
        let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
        pool.channels.push(self.channel);
        pool.slots.push(self.slot);
    }
}
