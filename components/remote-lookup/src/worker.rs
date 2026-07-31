//! Off-loop initiator worker (T-connect-hardening).
//!
//! A blocking RDMA connect can take on the order of seconds (cold `rdma_cm`
//! establishment + full-pool MR registration). Running it inline on the actor's
//! single poll-loop thread would freeze the whole node — no zyre events, no
//! status, no teardown processed. This worker owns clones of the server-role
//! receptacles and performs the two blocking RDMA operations off that thread:
//!
//! - **Warm** — proactively establish a connection to a discovered peer's
//!   responder ([`IRemoteLookupRdmaInitiator::connect`]), so a later serve hits
//!   the established-connection fast path (latency hidden at discovery time).
//! - **Serve** — answer a peer's RDMA_REQUEST ([`server::serve_rdma_request`])
//!   and hand the per-key statuses back to the poll loop as
//!   [`ActorMsg::PushComplete`]; the poll loop owns the zyre node and whispers
//!   the RDMA_STATUS.
//!
//! Mirrors the responder's command-channel thread pattern.
//!
//! A single worker is enough for the memory-tier path: since
//! [`IRemoteLookupRdmaInitiator::push_async`] only *enqueues* the writes, a serve no
//! longer occupies this thread for the duration of its RDMA transfer, and many
//! transfers to many peers proceed concurrently on the initiator's per-connection
//! threads. What remains on this thread is the local bookkeeping — the dispatch-map
//! lookups and, for block-tier keys, the promotion.
//!
//! That promotion is still blocking, so a disk-tier serve does head-of-line-block
//! other serves; a worker pool would be the fix there. Warming keeps cold connects
//! off this thread already.
//!
//! The worker exits when its command channel closes — i.e. when the actor thread
//! ends and drops the sole [`InitiatorCmd`] sender.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use component_core::channel::mpsc::MpscSender;
use interfaces::{
    Endpoint, IDispatchMap, IDispatcher, ILogger, IRemoteLookupRdmaInitiator, PeerId,
};

use crate::actor::ActorMsg;
use crate::server;
use crate::wire::SlotDesc;

/// Work handed off the poll loop to the initiator worker.
pub(crate) enum InitiatorCmd {
    /// Proactively establish a connection to a discovered peer's responder
    /// endpoint (`"ip:port"`), so a later serve does not pay the cold connect.
    Warm { endpoint: String },
    /// Serve a peer's RDMA_REQUEST off-loop. The result returns to the poll loop
    /// as [`ActorMsg::PushComplete`] for it to whisper the RDMA_STATUS.
    Serve {
        from: PeerId,
        op_id: u64,
        requester_endpoint: Endpoint,
        rkey: u32,
        slots: Vec<SlotDesc>,
    },
}

/// Server-role receptacle handles the worker needs (clones of the actor's).
pub(crate) struct ServerDeps {
    pub dispatch_map: Arc<dyn IDispatchMap + Send + Sync>,
    /// Optional — enables serving disk-only keys by promoting them (US4).
    pub dispatcher: Option<Arc<dyn IDispatcher + Send + Sync>>,
    pub initiator: Arc<dyn IRemoteLookupRdmaInitiator + Send + Sync>,
    pub logger: Option<Arc<dyn ILogger + Send + Sync>>,
}

/// The worker loop: drain [`InitiatorCmd`]s until the channel closes.
pub(crate) fn run(deps: ServerDeps, rx: Receiver<InitiatorCmd>, back: MpscSender<ActorMsg>) {
    if let Some(logger) = &deps.logger {
        logger.debug("remote-lookup: initiator worker started");
    }
    while let Ok(cmd) = rx.recv() {
        match cmd {
            InitiatorCmd::Warm { endpoint } => {
                // Best-effort: a failed warm caches nothing; the serve retries.
                let _ = deps.initiator.connect(&endpoint);
            }
            InitiatorCmd::Serve {
                from,
                op_id,
                requester_endpoint,
                rkey,
                slots,
            } => {
                let back = back.clone();
                server::serve_rdma_request(
                    &deps.dispatch_map,
                    deps.dispatcher.as_ref(),
                    &deps.initiator,
                    &requester_endpoint,
                    rkey,
                    &slots,
                    // Runs on the initiator's connection thread once the writes have
                    // landed. The poll loop owns the zyre node, so it whispers the
                    // status; a closed channel means the actor is shutting down and
                    // the requester's deadline backstops it.
                    Box::new(move |statuses| {
                        let _ = back.send(ActorMsg::PushComplete {
                            from,
                            op_id,
                            statuses,
                        });
                    }),
                );
            }
        }
    }
    if let Some(logger) = &deps.logger {
        logger.debug("remote-lookup: initiator worker stopped");
    }
}
