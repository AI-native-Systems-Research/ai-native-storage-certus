//! The plan artifact: fixed-width event records, manifest, and digests.

pub mod digest;
pub mod generate;
pub mod manifest;
pub mod record;
pub mod writer;

pub use digest::{parameter_hash, PlanDigest, StreamDigest};
pub use generate::{Budget, GenError, Generator, Horizon, DEFAULT_HORIZON_EVENTS};
pub use manifest::{CorpusSummary, Identity, Manifest, ManifestError, PLAN_FORMAT};
pub use record::{flags, DecodeError, PlanEvent, RECORD_BYTES};
pub use writer::{read_plan, unbounded_manifest, write_plan, PlanStats, PlanWriter, WriteError};
