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
#![warn(missing_docs)]
