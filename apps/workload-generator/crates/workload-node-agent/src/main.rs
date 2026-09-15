//! The per-node relay daemon.
//!
//! Attaches to the local Certus mailbox and serves `Submit` frames from the
//! generator, reconstructing block payloads from the keys they carry so only
//! keys cross the network. It is a relay, not a second generator: it never
//! decides *what* to issue.
//!
//! Exits non-zero if the local mailbox is absent, so a missing server is a
//! startup failure rather than a run that quietly measures nothing.

fn main() {
    // Mailbox attach and the Submit relay land with T068.
}
