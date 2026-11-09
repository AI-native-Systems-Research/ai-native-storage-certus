//! `IRemoteLookupRdmaResponder` — the accept side of a remote RDMA lookup.
//!
//! This interface belongs to the **requesting** instance: the node that wants a
//! value and offers memory for it to be written into. It is the passive
//! (accept) counterpart of the outbound initiator (`remote-lookup-rdma-initiator`
//! / `IRemoteLookupRdmaInitiator`).
//!
//! The responder is an **actor**: it owns a dedicated thread running an
//! `rdma_cm` accept loop that binds an ephemeral port and accepts inbound RDMA
//! connections from serving peers. Serving peers RDMA-*write* values directly
//! into pre-registered local memory (a one-sided operation), so the responder's
//! CPU never touches the data — it manages only *connections*.
//!
//! # Control, not data
//!
//! Because the writes are one-sided and the completion signal travels back over
//! the zyre control plane (the whisper status vector, owned by `remote-lookup`),
//! this interface carries **control** traffic only, not a data path:
//!
//! - [`IRemoteLookupRdmaResponderAdmin`] — lifecycle: bind + start the accept
//!   loop, pin its thread to the instance's NUMA node, and stop it.
//! - [`IRemoteLookupRdmaResponder`] — the runtime control channel and endpoint
//!   discovery, used by `remote-lookup`:
//!   - [`local_endpoint`](IRemoteLookupRdmaResponder::local_endpoint) returns the
//!     bound `{ip, port}` so `remote-lookup` can advertise it in whispers (the
//!     port is ephemeral, assigned at bind).
//!   - [`open_control_channel`](IRemoteLookupRdmaResponder::open_control_channel)
//!     returns a [`ControlChannel`] over which `remote-lookup` issues
//!     [`ResponderCommand::Disconnect`] and awaits [`ResponderEvent::DisconnectAck`]. This is the
//!     **teardown-before-reclaim** handshake: on a peer's departure the QP to
//!     that peer must be destroyed (so its late one-sided writes can no longer
//!     land) *before* the requester reclaims the peer's locked landing slots.

use std::fmt;

use component_core::channel::{Receiver, Sender};

use crate::izyre::PeerId;

/// Errors from the RDMA responder.
///
/// # Examples
///
/// ```
/// use interfaces::RemoteLookupRdmaResponderError;
///
/// let e = RemoteLookupRdmaResponderError::NotInitialized("start me first".into());
/// assert!(e.to_string().contains("not initialized"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteLookupRdmaResponderError {
    /// A control/channel method was called before [`IRemoteLookupRdmaResponderAdmin::initialize`].
    NotInitialized(String),
    /// [`IRemoteLookupRdmaResponderAdmin::initialize`] was called more than once.
    AlreadyInitialized(String),
    /// Binding the listener / starting the accept loop failed.
    Bind(String),
    /// Registering the memory-tier pool as an RDMA memory region failed (the
    /// `memory_tier` receptacle was unbound, its pool was not initialized, or
    /// `ibv_reg_mr` failed).
    Registration(String),
    /// The control channel could not be created or has been closed.
    ChannelClosed(String),
    /// An internal error in the responder actor.
    Internal(String),
}

impl fmt::Display for RemoteLookupRdmaResponderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInitialized(msg) => write!(f, "responder not initialized: {msg}"),
            Self::AlreadyInitialized(msg) => write!(f, "responder already initialized: {msg}"),
            Self::Bind(msg) => write!(f, "responder bind/listen failed: {msg}"),
            Self::Registration(msg) => write!(f, "responder pool registration failed: {msg}"),
            Self::ChannelClosed(msg) => write!(f, "responder control channel closed: {msg}"),
            Self::Internal(msg) => write!(f, "responder internal error: {msg}"),
        }
    }
}

impl std::error::Error for RemoteLookupRdmaResponderError {}

/// The bound listening endpoint of a responder.
///
/// `port` is ephemeral — assigned by the OS at bind time and read back with
/// `rdma_get_src_port` — so co-resident Certus instances (one per NUMA domain,
/// possibly sharing one NIC) never collide. `remote-lookup` advertises this
/// endpoint to serving peers in its whispers.
///
/// # Examples
///
/// ```
/// use interfaces::Endpoint;
///
/// let ep = Endpoint { ip: "192.0.2.10".into(), port: 49152 };
/// assert_eq!(ep.to_string(), "192.0.2.10:49152");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The local IPv4 the listener is bound to (pins the NIC/NUMA path).
    pub ip: String,
    /// The ephemeral port assigned at bind time.
    pub port: u16,
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.ip, self.port)
    }
}

/// The responder's pre-registered memory-tier pool, exposed to `remote-lookup`.
///
/// The responder registers the **whole** DRAM memory-tier pool once, in its own
/// protection domain, with `REMOTE_WRITE` access. `remote-lookup` reads this
/// region at startup, caches the single pool-wide `rkey`, and pairs it with each
/// individual landing-slot address to build the per-key `RemoteRegion` it hands
/// the initiator — so there is no per-request `ibv_reg_mr` on the I/O path.
///
/// `length` is `usize` (not `u32`) because the pool can exceed 4 GiB; the
/// per-slot `RemoteRegion.length` stays `u32` since individual entries are
/// bounded well below that.
///
/// # Examples
///
/// ```
/// use interfaces::LocalRegion;
///
/// let r = LocalRegion { addr: 0x7f00_0000_0000, rkey: 0x4242, length: 8 << 30 };
/// assert_eq!(r.length, 8 << 30);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalRegion {
    /// Base virtual address of the registered pool.
    pub addr: u64,
    /// The pool-wide remote key authorizing inbound one-sided writes.
    pub rkey: u32,
    /// Length of the registered pool in bytes.
    pub length: usize,
}

/// Control commands sent *to* the responder actor by `remote-lookup`.
///
/// This is control traffic only — there is no per-value command, because the
/// RDMA writes are one-sided and land without the responder's involvement.
///
/// # Examples
///
/// ```
/// use interfaces::{ResponderCommand, PeerId};
///
/// let cmd = ResponderCommand::Disconnect { node: PeerId::new("uuid-1") };
/// assert!(matches!(cmd, ResponderCommand::Disconnect { .. }));
/// ```
#[derive(Debug, Clone)]
pub enum ResponderCommand {
    /// Tear down the QP to `node` **before** the requester reclaims that node's
    /// locked landing slots. The actor replies with [`ResponderEvent::DisconnectAck`]
    /// once teardown is complete; only then is reclaiming safe.
    Disconnect {
        /// The departing peer (a zyre node) whose connection must be severed.
        node: PeerId,
    },
}

/// Events emitted *by* the responder actor on the [`ControlChannel`].
///
/// # Examples
///
/// ```
/// use interfaces::{ResponderEvent, PeerId};
///
/// let ev = ResponderEvent::DisconnectAck { node: PeerId::new("uuid-1") };
/// assert!(matches!(ev, ResponderEvent::DisconnectAck { .. }));
/// ```
#[derive(Debug, Clone)]
pub enum ResponderEvent {
    /// A serving peer established an inbound RDMA connection.
    ConnectionEstablished {
        /// The connecting peer, if it could be identified.
        node: Option<PeerId>,
    },
    /// Teardown of the QP to `node` has completed; reclaiming its slots is now
    /// safe (the peer's late one-sided writes can no longer land).
    DisconnectAck {
        /// The peer whose connection was torn down.
        node: PeerId,
    },
    /// A non-fatal error occurred in the accept loop.
    Error {
        /// Human-readable description.
        message: String,
    },
}

/// The control channel handed to `remote-lookup` by
/// [`IRemoteLookupRdmaResponder::open_control_channel`].
///
/// Send [`ResponderCommand`]s on `command_tx`; receive [`ResponderEvent`]s on `event_rx`.
///
/// # Examples
///
/// ```ignore
/// let ch = responder.open_control_channel().unwrap();
/// ch.command_tx.send(ResponderCommand::Disconnect { node }).unwrap();
/// // ... later, block until teardown completes before reclaiming slots:
/// let ack = ch.event_rx.recv().unwrap();
/// ```
pub struct ControlChannel {
    /// Sender for control [`ResponderCommand`]s to the actor.
    pub command_tx: Sender<ResponderCommand>,
    /// Receiver for [`ResponderEvent`]s from the actor.
    pub event_rx: Receiver<ResponderEvent>,
}

impl fmt::Debug for ControlChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlChannel")
            .field("command_tx", &"Sender<ResponderCommand>")
            .field("event_rx", &"Receiver<ResponderEvent>")
            .finish()
    }
}

// Runtime control surface of the RDMA responder, used by `remote-lookup`.
component_macros::define_interface! {
    pub IRemoteLookupRdmaResponder {
        /// Open the control channel for issuing [`ResponderCommand`]s and receiving
        /// [`ResponderEvent`]s. Fails if the responder has not been initialized.
        fn open_control_channel(&self) -> Result<ControlChannel, RemoteLookupRdmaResponderError>;

        /// Return the bound `{ip, port}` so `remote-lookup` can advertise it in
        /// whispers. Fails if the responder has not been initialized.
        fn local_endpoint(&self) -> Result<Endpoint, RemoteLookupRdmaResponderError>;

        /// Return the pre-registered memory-tier pool region — its base address,
        /// length, and the pool-wide `rkey` — so `remote-lookup` can advertise the
        /// rkey (paired with each landing-slot address) in its RDMA requests.
        /// Fails if the responder has not been initialized.
        fn local_region(&self) -> Result<LocalRegion, RemoteLookupRdmaResponderError>;
    }
}

// Lifecycle / configuration surface of the RDMA responder, driven by the
// application (mainline), not by `remote-lookup`.
component_macros::define_interface! {
    pub IRemoteLookupRdmaResponderAdmin {
        /// Pin the accept-loop thread to `cpu`. Must be called before
        /// [`initialize`](Self::initialize); one Certus instance runs per NUMA
        /// domain, so the listener should run on that instance's node.
        fn set_actor_cpu(&self, cpu: usize);

        /// Supply the local RoCE IPv4 the listener binds to. Must be called
        /// before [`initialize`](Self::initialize).
        ///
        /// The responder never auto-detects the address: the mainline (which
        /// already owns NUMA placement via [`set_actor_cpu`](Self::set_actor_cpu))
        /// is the single source of the bind IP, so the choice is deterministic on
        /// hosts with multiple RoCE NICs / NUMA domains. Binding by IP implies the
        /// NIC/NUMA path — the device is never selected by name. If no IP was
        /// supplied (or it is unusable on this host), [`initialize`](Self::initialize)
        /// fails with [`RemoteLookupRdmaResponderError::Bind`].
        fn set_bind_ip(&self, ip: String);

        /// Bind an ephemeral port on the IP supplied via
        /// [`set_bind_ip`](Self::set_bind_ip), register the whole memory-tier pool
        /// (read from the `memory_tier` receptacle via `IMemoryTier::pool_info`) as
        /// a single `REMOTE_WRITE` memory region in the responder's protection
        /// domain, and start the `rdma_cm` accept loop on the actor thread. The
        /// resulting pool-wide region is exposed via
        /// [`local_region`](IRemoteLookupRdmaResponder::local_region). Fails with
        /// [`Registration`](RemoteLookupRdmaResponderError::Registration) if the
        /// pool is unavailable or `ibv_reg_mr` fails.
        fn initialize(&self) -> Result<(), RemoteLookupRdmaResponderError>;

        /// Signal the accept loop to stop without joining its thread (closes the
        /// actor's command channel so the loop exits).
        fn signal_stop(&self);

        /// Stop the accept loop and join its thread, tearing down all
        /// connections and the listener.
        fn shutdown(&self) -> Result<(), RemoteLookupRdmaResponderError>;
    }
}
