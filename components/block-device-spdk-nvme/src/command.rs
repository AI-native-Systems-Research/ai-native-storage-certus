//! Internal message types for actor lifecycle management.
//!
//! Public command/completion types live in the `interfaces` crate.
//! This module contains only the actor-internal control messages
//! and per-client session state.

use std::collections::VecDeque;

use component_core::channel::{Receiver, Sender};
use interfaces::{Command, Completion};

/// Internal: per-client state maintained by the actor handler.
pub(crate) struct ClientSession {
    /// Unique client session identifier.
    pub id: u64,
    /// Receiver end of client's ingress SPSC channel.
    pub ingress_rx: Receiver<Command>,
    /// Sender end of client's callback SPSC channel.
    pub callback_tx: Sender<Completion>,
    /// Completions that couldn't be delivered because the client's callback
    /// ring was full. Retried by [`Self::flush_pending`] each poll cycle.
    ///
    /// This makes completion delivery non-blocking: the single-threaded actor
    /// must never block sending to one client, or a slow/stalled client would
    /// head-of-line-block completion delivery to every other client on the
    /// drive (a whole-drive deadlock). Bounded in practice by the client's
    /// outstanding operations.
    pub pending: VecDeque<Completion>,
}

impl ClientSession {
    /// Deliver a completion without ever blocking the actor. Fast path is a
    /// single `try_send`; on a full ring (or an existing backlog) the completion
    /// is buffered in FIFO order and retried by [`Self::flush_pending`].
    pub fn deliver(&mut self, completion: Completion) {
        if self.pending.is_empty() && self.callback_tx.try_send(completion.clone()).is_ok() {
            return;
        }
        self.pending.push_back(completion);
    }

    /// Retry delivering buffered completions, oldest first, stopping at the
    /// first that still can't be sent (ring full) to preserve ordering.
    /// Returns true if any were delivered.
    pub fn flush_pending(&mut self) -> bool {
        let mut delivered = false;
        while let Some(front) = self.pending.front() {
            if self.callback_tx.try_send(front.clone()).is_ok() {
                self.pending.pop_front();
                delivered = true;
            } else {
                break;
            }
        }
        delivered
    }
}

/// Control messages on the actor's main MPSC channel.
#[allow(dead_code)]
pub(crate) enum ControlMessage {
    /// Register a new client.
    ConnectClient { session: ClientSession },
    /// Remove a client by ID.
    DisconnectClient { client_id: u64 },
}
