//! The plan artifact: fixed-width event records, manifest, and digests.

pub mod digest;
pub mod record;

pub use digest::{parameter_hash, PlanDigest, StreamDigest};
pub use record::{flags, DecodeError, PlanEvent, RECORD_BYTES};
