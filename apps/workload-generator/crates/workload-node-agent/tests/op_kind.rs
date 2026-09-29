//! The wire's `op_kind` numbers are the mailbox's opcodes, pinned to the dispatcher.
//!
//! # Why this test is in this crate
//!
//! A `Stats` reply keys its histograms by the opcode the agent sent to the mailbox, and the
//! generator renders a name for it. Those are two crates: the agent fills the field and
//! `workload-gen` prints it. Neither can check the numbers — `workload-gen` deliberately does not
//! depend on `shmq-dispatcher` at all since FR-079, and `workload-wire` must not, because it is a
//! CUDA-free workspace default member and the dependency would unify `interfaces/spdk` into every
//! plain `cargo build`.
//!
//! This crate is the only one that sees both, so this is where the duplication is made safe. A
//! table copied and then left to drift is a second source of truth; a table copied and pinned is
//! not, and the failure it prevents is a silent one — the run would complete and report a
//! `LOOKUP` percentile under the name `RESERVE`.

use shmq_dispatcher::wire::op;
use workload_wire::frame::op_kind;

#[test]
fn every_wire_op_kind_is_the_dispatchers_own_opcode() {
    // Written out one by one rather than in a loop over pairs, so a swapped pair is visible in
    // the failure rather than being a count that still matches.
    assert_eq!(u32::from(op_kind::CHECK), op::CHECK);
    assert_eq!(u32::from(op_kind::TOUCH), op::TOUCH);
    assert_eq!(u32::from(op_kind::RESERVE), op::RESERVE);
    assert_eq!(u32::from(op_kind::COPY_TO_STORE), op::COPY_TO_STORE);
    assert_eq!(u32::from(op_kind::COMMIT_STORE), op::COMMIT_STORE);
    assert_eq!(u32::from(op_kind::ABORT_STORE), op::ABORT_STORE);
    assert_eq!(u32::from(op_kind::LOOKUP), op::LOOKUP);
    assert_eq!(u32::from(op_kind::TAKE_EVENTS), op::TAKE_EVENTS);
}

#[test]
fn every_opcode_the_executor_can_issue_has_a_name() {
    // The set the agent actually times, so a new operation added to the executor without a name
    // shows up here rather than as the word "other" in a report a year later.
    for opcode in [
        op::CHECK,
        op::TOUCH,
        op::RESERVE,
        op::COPY_TO_STORE,
        op::COMMIT_STORE,
        op::ABORT_STORE,
        op::LOOKUP,
        op::TAKE_EVENTS,
    ] {
        assert_ne!(
            op_kind::name(opcode),
            "other",
            "opcode {opcode} would be reported as \"other\""
        );
    }
    // And a forbidden one is *not* named, because naming it would suggest it can appear.
    // `CLEAR_MEMORY_TIER` is issued exactly once, outside the timed window, and is never timed as
    // part of the stream (FR-043, FR-046).
    assert_eq!(op_kind::name(op::CLEAR_MEMORY_TIER), "other");
    assert_eq!(op_kind::name(op::PIN), "other");
}

#[test]
fn a_kind_that_does_not_fit_a_byte_is_named_rather_than_panicking() {
    // `op_kind` is a `u8` on the wire and a `u32` in the histogram map, so the conversion has to
    // be fallible. Losing one latency figure to an unknown opcode is a reporting curiosity;
    // panicking in the middle of rendering a completed run's report is not.
    assert_eq!(op_kind::name(9_999), "other");
    assert_eq!(op_kind::name(u32::MAX), "other");
}
