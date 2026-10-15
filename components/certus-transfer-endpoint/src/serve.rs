//! Source side: per-entry lookup, pin, push and outcome accumulation.
//!
//! One `IDispatchMap::lookup` yields residency, address and pin together
//! (research D6), and pins are released per entry by the completion reaper that
//! owns their work-request id (D7, constitution Principle IX).
//!
//! This path must not phase-barrier: memory-tier writes post as soon as they are
//! resolved, each block-tier write posts when its own device read completes, and
//! the send queue is never drained between entries while work remains (FR-022,
//! research D19).
//!
//! Scaffolding: the push path lands in T017, pin discipline in T022–T024.
