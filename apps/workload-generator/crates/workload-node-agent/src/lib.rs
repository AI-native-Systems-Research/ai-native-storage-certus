//! The node agent's library half: everything that touches the Certus mailbox.
//!
//! # Why this is a library at all, and why it is *here*
//!
//! The generator no longer talks to a mailbox. FR-079 routes every node through an agent —
//! including the local one — so all the mailbox-facing code lives on one side of the wire:
//! [`mailbox::attach`] claims the channels, [`opstream`] maps the plan's operations onto the
//! dispatcher's opcodes, [`payload`] holds the pre-filled device buffer, and [`exec`] applies
//! FR-072a's reactive rule.
//!
//! These four used to live in `workload-gen`, which the agent depended on so the rule would have
//! exactly **one** implementation. That dependency ran the wrong way for a daemon and was
//! documented as accepted for the stronger property. Collapsing the local path onto the agent
//! removed the reason for it: there is still exactly one implementation, it is now in the crate
//! that uses it, and the arrow points the way a daemon's normally does. `workload-gen` in turn
//! stops depending on `shm-queue` and on CUDA, and so stops enabling `interfaces/spdk`
//! transitively — which is what lets it be a workspace default member.
//!
//! A library rather than four modules of a binary because the mapping in [`opstream`] is checked
//! against the dispatcher's own rules by an integration test, and a `main.rs` module cannot be.
//!
//! # What the agent decides, which is nothing about the workload
//!
//! Which keys, in which order, for which session, at which virtual time — all of that arrives in
//! a `SubmitTurn` from the generator's single simulation core (FR-072). What the agent applies is
//! the *client rule*: check the path, load what is resident, store what is absent. That is a
//! mechanical consequence of what the cache reports, not a choice about the workload.
#![warn(missing_docs)]

pub mod agent;
pub mod cuda;
pub mod exec;
pub mod mailbox;
pub mod opstream;
pub mod payload;
