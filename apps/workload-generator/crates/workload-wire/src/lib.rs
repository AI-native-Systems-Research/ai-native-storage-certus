//! The generator-to-node-agent wire protocol.
//!
//! Plain TCP, length-prefixed, `TCP_NODELAY`, pipelined — per
//! `contracts/node-agent-wire.md`. Only *keys* cross this wire; payload bytes
//! are reconstructed from the key by the agent, which is why a commodity TCP
//! transport has an order of magnitude of headroom (research.md D2).
//!
//! Every frame is a 12-byte little-endian header plus a body. `Hello` must be
//! the first frame on every connection and is **fail-closed** on both
//! `proto_version` and `build_id`: a peer built from different source is refused
//! rather than measured.
//!
//! # Four modules, and the line between them
//!
//! [`frame`] is the codec and nothing else: pure functions over bytes, so every
//! encoding property is testable without a socket. [`handshake`] is the fail-closed
//! provenance check, likewise pure. [`client`] is the generator's end — framing I/O,
//! the pipelining window, correlation-id matching — and [`server`] is the agent's
//! accept and frame loops. Neither of the last two knows anything about mailboxes,
//! keys or caches: that work belongs to [`server::Service`], which the node agent
//! implements, and keeping it out of here is what lets the whole protocol run
//! in-process with no agent and no accelerator.
//!
//! # Examples
//!
//! ```
//! use std::sync::atomic::AtomicBool;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! use workload_wire::client::Client;
//! use workload_wire::frame::{Hello, HelloAck, SubmitTurn, TurnOutcome};
//! use workload_wire::handshake;
//! use workload_wire::server::{FnFactory, Server, Service};
//!
//! /// The agent's side of one connection. A real one drives a mailbox; this one
//! /// answers from a set of keys it pretends to hold.
//! struct Agent {
//!     held: Vec<u64>,
//! }
//!
//! impl Service for Agent {
//!     fn hello(&mut self, hello: &Hello) -> HelloAck {
//!         handshake::answer(hello, 8, 32768)
//!     }
//!
//!     fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome {
//!         let resident = turn.path.iter().filter(|k| self.held.contains(k)).count() as u32;
//!         TurnOutcome {
//!             resident,
//!             missing: turn.path.len() as u32 - resident,
//!             blocks_read: resident,
//!             ..Default::default()
//!         }
//!     }
//! }
//!
//! let server = Server::bind("127.0.0.1:0", FnFactory(|| Ok(Agent { held: vec![10, 11] })))
//!     .unwrap()
//!     .with_linger(Duration::ZERO);
//! let addr = server.local_addr().unwrap();
//! let serving = std::thread::spawn(move || {
//!     server.serve(Arc::new(AtomicBool::new(false))).unwrap();
//! });
//!
//! let mut client = Client::connect(addr, 8, Some(Duration::from_secs(10))).unwrap();
//! // Mandatory, and fail-closed: this refuses an agent built from other sources.
//! client.handshake("localhost", "/dev/shm/certus-shmq", 8, 32768).unwrap();
//!
//! // Only keys cross the wire. The agent reconstructs payload bytes from them.
//! client.submit(&SubmitTurn { session: 1, flags: 0, path: vec![10, 11, 12] }).unwrap();
//! let outcome = client.finish().unwrap().remove(0);
//! assert_eq!((outcome.resident, outcome.missing), (2, 1));
//!
//! client.shutdown().unwrap();
//! serving.join().unwrap();
//! ```
#![warn(missing_docs)]

pub mod client;
pub mod frame;
pub mod handshake;
pub mod server;
