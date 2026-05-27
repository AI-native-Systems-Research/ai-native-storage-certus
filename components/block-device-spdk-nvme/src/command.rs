//! Internal message types for actor lifecycle management.
//!
//! Public command/completion types live in the `interfaces` crate.
//! This module contains only the actor-internal control messages
//! and per-client session state.

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
}

/// Control messages on the actor's main MPSC channel.
#[allow(dead_code)]
pub(crate) enum ControlMessage {
    /// Register a new client.
    ConnectClient { session: ClientSession },
    /// Remove a client by ID.
    DisconnectClient { client_id: u64 },
}
