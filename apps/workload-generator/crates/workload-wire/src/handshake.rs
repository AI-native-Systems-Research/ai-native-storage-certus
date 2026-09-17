//! The `Hello` handshake: fail-closed on provenance, and it names the node.
//!
//! # What is being prevented
//!
//! FR-051 requires refusing a daemon that was not built from the same sources as the
//! generator, and the reason is recorded in this repository's own history: **a stale remote
//! binary has already invalidated measurements, and it failed silently** — the run completed
//! and produced numbers. That is the worst shape a failure can take for an instrument, so
//! the handshake is mandatory, is the first frame on every connection, and refuses rather
//! than warns.
//!
//! # The identity is the source tree, not the executable
//!
//! The generator and the agent are different binaries; a digest of either would differ from
//! the other by construction. FR-051 asks about *sources*, so [`current`] is a digest of a
//! build-time source identity — commit, tracked diff, and porcelain status — computed once in
//! this crate's build script and shared by both binaries through this crate. See `build.rs`
//! for what it covers and, importantly, for the one thing it does not: the contents of
//! untracked files.
//!
//! An identity that could not be established is [`UNKNOWN`], and a peer reporting it is
//! **refused**. Provenance that cannot be established must not be assumed, and the only cost
//! of refusing is that a multi-node run needs a tree git can describe.
//!
//! # Both ends check
//!
//! The client compares what the agent reports against its own and refuses; the agent compares
//! what the client sent against its own and answers with a non-zero status. Either alone would
//! be enough to stop a bad run, but only checking on both ends makes the refusal *legible* —
//! the operator learns which side disagreed and what each thought it was.
//!
//! # The reply carries capacity, and it is checked
//!
//! `channels` and `block_bytes` come back so the generator can compare its lane count against
//! the node's real mailbox capacity. The mailbox is depth-1 per channel, so asking for more
//! lanes than channels does not fail — it **serialises silently**, which would look like a
//! slow server rather than a misconfigured run. See [`check_capacity`].
//!
//! # Examples
//!
//! Both ends of an accepted handshake, with no socket involved — [`answer`] is the
//! agent's side and [`verify`] the generator's:
//!
//! ```
//! use workload_wire::handshake::{self, status};
//!
//! let hello = handshake::hello("/dev/shm/certus-shmq");
//! let ack = handshake::answer(&hello, 8, 32768);
//!
//! assert_eq!(ack.status, status::OK);
//! handshake::verify("node2", &ack).unwrap();
//!
//! // Capacity is a separate check, because it is about the run's shape rather than
//! // about who the peer is.
//! handshake::check_capacity("node2", &ack, 4, 32768).unwrap();
//! ```
//!
//! A refusal names the node and says what each side thought, which is what makes it
//! actionable on a cluster:
//!
//! ```
//! use workload_wire::handshake::{self, Refused};
//!
//! let hello = handshake::hello("/dev/shm/certus-shmq");
//! let mut ack = handshake::answer(&hello, 8, 32768);
//!
//! // An agent built from a different tree.
//! ack.build_id = handshake::digest("some other source tree");
//! let refused = handshake::verify("node7", &ack).unwrap_err();
//! assert!(matches!(refused, Refused::SourceIdentity { .. }));
//! assert!(refused.to_string().contains("node7"));
//! assert!(refused.to_string().contains("FR-051"));
//! ```
//!
//! And the capacity check is what stops the failure that would otherwise look like a
//! slow server:
//!
//! ```
//! use workload_wire::handshake::{self, Refused};
//!
//! let hello = handshake::hello("/dev/shm/certus-shmq");
//! let ack = handshake::answer(&hello, 4, 32768);
//!
//! // Eight lanes over four depth-1 channels would serialise, not fail.
//! let refused = handshake::check_capacity("node5", &ack, 8, 32768).unwrap_err();
//! assert!(matches!(refused, Refused::TooManyLanes { channels: 4, .. }));
//! assert!(refused.to_string().contains("serialise silently"));
//! ```
//!
//! Both of the accepting examples above assume this build's provenance is known. If it
//! is not — [`is_known`] is false, because the source files could not be read at build
//! time — `verify` refuses instead, which is the fail-closed direction.

use std::fmt;

use workload_model::keys::splitmix64;

use crate::frame::{Hello, HelloAck, BUILD_ID_BYTES, PROTO_VERSION};

/// The source identity captured at build time.
///
/// A digest of this application's own source files, so it is available whether or not the
/// build happened inside a repository. `unknown` only when those files could not be read;
/// see [`is_known`].
pub const SOURCE_ID: &str = env!("WORKLOAD_SOURCE_ID");

/// The identity used when provenance could not be established.
pub const UNKNOWN: &str = "unknown";

/// `HelloAck::status` values.
pub mod status {
    /// Accepted.
    pub const OK: u16 = 0;
    /// The client's protocol version differs from the agent's.
    pub const PROTO_MISMATCH: u16 = 1;
    /// The client's source identity differs from the agent's.
    pub const BUILD_MISMATCH: u16 = 2;
    /// One side could not establish its provenance at all.
    pub const UNKNOWN_PROVENANCE: u16 = 3;
}

/// Whether this build's provenance is known.
///
/// A run across nodes requires it: an unknown identity cannot demonstrate that two binaries
/// came from one tree, and FR-051 asks for a demonstration rather than an assumption.
pub fn is_known() -> bool {
    SOURCE_ID != UNKNOWN && !SOURCE_ID.is_empty()
}

/// This build's 32-byte source digest.
///
/// Derived from [`SOURCE_ID`] by mixing with `splitmix64` — the repository's own primitive,
/// reused here rather than adding a hashing dependency to a workspace default member. It is
/// not cryptographic and does not need to be: the failure being prevented is an accident, and
/// anyone able to replace a binary can replace the identity it reports.
pub fn current() -> [u8; BUILD_ID_BYTES] {
    digest(SOURCE_ID)
}

/// A 32-byte digest of an arbitrary identity string.
pub fn digest(id: &str) -> [u8; BUILD_ID_BYTES] {
    let mut out = [0u8; BUILD_ID_BYTES];
    // Four independent 64-bit chains, each seeded differently, so a one-byte change in the
    // identity moves every word of the digest rather than one.
    for (word, seed) in out.chunks_mut(8).zip(0u64..) {
        let mut h = splitmix64(seed ^ 0x5DEE_CE66_D000_0005);
        for b in id.as_bytes() {
            h = splitmix64(h ^ u64::from(*b));
        }
        word.copy_from_slice(&h.to_le_bytes());
    }
    out
}

/// Why a handshake was refused.
///
/// Every variant names the node, because a refusal an operator cannot attribute to a machine
/// is a refusal they cannot act on — and on a cluster the whole point is to learn *which* node
/// is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The protocol versions differ.
    ProtocolVersion {
        /// The node that answered.
        node: String,
        /// What this build speaks.
        ours: u32,
        /// What the agent speaks.
        theirs: u32,
    },
    /// The source identities differ, so the agent is not this build.
    SourceIdentity {
        /// The node that answered.
        node: String,
        /// This build's digest, abbreviated.
        ours: String,
        /// The agent's digest, abbreviated.
        theirs: String,
    },
    /// One side could not establish provenance.
    UnknownProvenance {
        /// The node that answered.
        node: String,
        /// Whether it was this side that could not.
        ours: bool,
    },
    /// The agent refused us, with its own status code.
    ByPeer {
        /// The node that refused.
        node: String,
        /// The status it sent.
        status: u16,
    },
    /// More lanes were asked for than the node has channels.
    TooManyLanes {
        /// The node.
        node: String,
        /// Lanes requested.
        lanes: usize,
        /// Channels available.
        channels: u32,
    },
    /// The node's block size differs from the description's.
    BlockBytes {
        /// The node.
        node: String,
        /// What the description says.
        ours: u32,
        /// What the node reports.
        theirs: u32,
    },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProtocolVersion { node, ours, theirs } => write!(
                f,
                "node {node} speaks wire protocol {theirs}, this build speaks {ours}; \
                 refusing rather than guessing what its frames mean"
            ),
            Self::SourceIdentity { node, ours, theirs } => write!(
                f,
                "node {node}'s agent was built from {theirs}, this build is {ours}; a stale \
                 remote binary has invalidated measurements here before, and it did so \
                 silently — refusing (FR-051)"
            ),
            Self::UnknownProvenance { node, ours } => {
                let side = if *ours { "this build" } else { "its agent" };
                write!(
                    f,
                    "node {node}: {side} cannot describe the sources it was built from, so \
                     the two cannot be shown to match — its source files could not be read \
                     at build time. Set WORKLOAD_SOURCE_ID deliberately if a packager knows \
                     better (FR-051)"
                )
            }
            Self::ByPeer { node, status } => write!(
                f,
                "node {node}'s agent refused this generator with status {status}; it checks \
                 provenance too, so the disagreement is mutual"
            ),
            Self::TooManyLanes {
                node,
                lanes,
                channels,
            } => write!(
                f,
                "asked node {node} for {lanes} lanes but its mailbox has {channels} \
                 channels; the mailbox is depth-1 per channel, so the extra lanes would \
                 serialise silently and look like a slow server"
            ),
            Self::BlockBytes { node, ours, theirs } => write!(
                f,
                "node {node} holds {theirs}-byte blocks, the description says {ours}; the \
                 cache would be sized in the wrong unit and nothing would report it"
            ),
        }
    }
}

impl std::error::Error for Refused {}

/// Build the `Hello` this generator sends.
pub fn hello(mailbox: &str) -> Hello {
    Hello {
        proto_version: PROTO_VERSION,
        build_id: current(),
        mailbox: mailbox.to_string(),
    }
}

/// Check an agent's reply, fail-closed, naming `node`.
///
/// Checked in the order a human would want to hear about: the protocol first, since a version
/// mismatch means the rest of the reply may not mean what it appears to; then provenance; then
/// what the agent thought of us; then capacity.
///
/// # Errors
///
/// [`Refused`] on any mismatch. There is no lenient mode: FR-051 says refuse.
pub fn verify(node: &str, ack: &HelloAck) -> Result<(), Refused> {
    if ack.proto_version != PROTO_VERSION {
        return Err(Refused::ProtocolVersion {
            node: node.to_string(),
            ours: PROTO_VERSION,
            theirs: ack.proto_version,
        });
    }
    if !is_known() {
        return Err(Refused::UnknownProvenance {
            node: node.to_string(),
            ours: true,
        });
    }
    if ack.build_id == digest(UNKNOWN) {
        return Err(Refused::UnknownProvenance {
            node: node.to_string(),
            ours: false,
        });
    }
    if ack.build_id != current() {
        return Err(Refused::SourceIdentity {
            node: node.to_string(),
            ours: abbreviate(&current()),
            theirs: abbreviate(&ack.build_id),
        });
    }
    // Checked after provenance: an agent that refused us for a reason we have not detected is
    // worth reporting on its own terms rather than as a mystery.
    if ack.status != status::OK {
        return Err(Refused::ByPeer {
            node: node.to_string(),
            status: ack.status,
        });
    }
    Ok(())
}

/// Check the run's shape against what the node reports.
///
/// # Errors
///
/// [`Refused::TooManyLanes`] or [`Refused::BlockBytes`].
pub fn check_capacity(
    node: &str,
    ack: &HelloAck,
    lanes: usize,
    block_bytes: u32,
) -> Result<(), Refused> {
    if lanes as u64 > u64::from(ack.channels) {
        return Err(Refused::TooManyLanes {
            node: node.to_string(),
            lanes,
            channels: ack.channels,
        });
    }
    if ack.block_bytes != block_bytes {
        return Err(Refused::BlockBytes {
            node: node.to_string(),
            ours: block_bytes,
            theirs: ack.block_bytes,
        });
    }
    Ok(())
}

/// The agent's side: judge a `Hello` and build the reply.
///
/// The agent refuses too, so a mismatch is reported from both ends and the operator learns
/// which side disagreed rather than only that someone did.
pub fn answer(hello: &Hello, channels: u32, block_bytes: u32) -> HelloAck {
    let status = if hello.proto_version != PROTO_VERSION {
        status::PROTO_MISMATCH
    } else if !is_known() || hello.build_id == digest(UNKNOWN) {
        status::UNKNOWN_PROVENANCE
    } else if hello.build_id != current() {
        status::BUILD_MISMATCH
    } else {
        status::OK
    };
    HelloAck {
        proto_version: PROTO_VERSION,
        // Always this build's own, even when refusing: the generator's message is far more
        // useful when it can print both identities.
        build_id: current(),
        channels,
        block_bytes,
        status,
    }
}

/// The first eight bytes of a digest, hex, for a message a human reads.
pub fn abbreviate(id: &[u8; BUILD_ID_BYTES]) -> String {
    id[..8].iter().map(|b| format!("{b:02x}")).collect()
}
