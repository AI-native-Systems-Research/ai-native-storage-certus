//! The plan manifest.
//!
//! Carries the normalised input, the identity of what was generated, and the
//! realised corpus summary — so a report can always be traced to its exact
//! input, including defaults that were applied rather than written.
//!
//! Two things it must not do. It must not let a decoder guess the record layout:
//! `plan_format` is the only signal of the record width, because the record has
//! no length prefix. And it must not let one kind of identity pass for another: a
//! bounded plan carries a hash over realised events, an unbounded run carries a
//! hash over its parameters, and a report states which.

use serde::{Deserialize, Serialize};

use crate::plan::digest::PlanDigest;

/// The record layout version. Bump on any change to a field's presence, order or
/// width, and refuse a version this build does not implement.
pub const PLAN_FORMAT: u32 = 1;

/// Which kind of identity the artifact carries, as written to JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Identity {
    /// Hash over the realised events; bounded plans only.
    ContentHash(String),
    /// Hash over normalised YAML + seed + `plan_format`; unbounded runs.
    ParameterHash(String),
}

impl From<PlanDigest> for Identity {
    fn from(d: PlanDigest) -> Self {
        match d {
            PlanDigest::Content(s) => Identity::ContentHash(s),
            PlanDigest::Parameters(s) => Identity::ParameterHash(s),
        }
    }
}

impl Identity {
    /// The digest string.
    pub fn digest(&self) -> &str {
        match self {
            Identity::ContentHash(s) | Identity::ParameterHash(s) => s,
        }
    }

    /// A label for reports, so the two kinds are never conflated.
    pub fn label(&self) -> &'static str {
        match self {
            Identity::ContentHash(_) => "content-hash (bounded plan)",
            Identity::ParameterHash(_) => "parameter-hash (unbounded run)",
        }
    }
}

/// Realised corpus properties.
///
/// Every field is the **realised** value, not the configured one (spec FR-012):
/// a document states a fanout, and the width and occupancy it produces are only
/// knowable after generating.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CorpusSummary {
    /// Distinct keys minted.
    pub distinct_keys: u64,
    /// Total payload bytes referenced.
    pub total_bytes: u64,
    /// Working-set size over `wss_window_requests`.
    pub working_set_bytes: u64,
    /// The window, as a request count.
    pub wss_window_requests: u64,
    /// Realised trunk width by depth.
    pub trunk_width_per_depth: Vec<(u32, u64)>,
    /// Realised trunk occupancy by depth.
    pub trunk_occupancy_per_depth: Vec<(u32, f64)>,
    /// The resolved branching profile, including what `auto` chose.
    pub branching_resolved: Vec<(u32, f64)>,
    /// Where the root boundary was placed.
    pub root_boundary_depth: u32,
    /// Churn half-life in nanoseconds; 0 means an immortal trunk.
    pub churn_half_life_ns: u64,
    /// How many trunk rotations occurred.
    pub churn_rotations: u64,
    /// Adjustments applied to drawn values, surfaced rather than hidden.
    pub clamps_applied: u64,
}

/// The manifest beside `events.bin`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Record layout version; a decoder's only signal of the width.
    pub plan_format: u32,
    /// Which build produced this.
    pub generator_version: String,
    /// The identity of what was generated.
    pub identity: Identity,
    /// The seed every draw derived from.
    pub seed: u64,
    /// The full input after `extends` merge and defaulting.
    pub normalised_yaml: String,
    /// How many events, or `None` for an unbounded run.
    pub event_count: Option<u64>,
    /// Plan time origin.
    pub time_origin_ns: u64,
    /// Plan duration, or `None` for an unbounded run.
    pub duration_ns: Option<u64>,
    /// Realised corpus properties.
    pub corpus_summary: CorpusSummary,
    /// Digest over the key sequence, so two arms can be proven equal.
    pub stream_digest: String,
}

/// Why a manifest was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// A `plan_format` this build does not implement.
    UnsupportedFormat { found: u32, supported: u32 },
    /// Not valid JSON, or missing a required field.
    Malformed(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::UnsupportedFormat { found, supported } => write!(
                f,
                "plan_format {found} is not implemented by this build (supports {supported}); \
                 refusing rather than guessing the record width, which has no length prefix"
            ),
            ManifestError::Malformed(e) => write!(f, "malformed manifest: {e}"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl Manifest {
    /// Serialise to pretty JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse, refusing a `plan_format` this build does not implement.
    pub fn from_json(s: &str) -> Result<Manifest, ManifestError> {
        let m: Manifest =
            serde_json::from_str(s).map_err(|e| ManifestError::Malformed(e.to_string()))?;
        if m.plan_format != PLAN_FORMAT {
            return Err(ManifestError::UnsupportedFormat {
                found: m.plan_format,
                supported: PLAN_FORMAT,
            });
        }
        Ok(m)
    }

    /// Whether this describes an unbounded run.
    pub fn is_unbounded(&self) -> bool {
        self.event_count.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::digest::parameter_hash;

    fn bounded() -> Manifest {
        Manifest {
            plan_format: PLAN_FORMAT,
            generator_version: "certus-workload 0.1.0".into(),
            identity: Identity::ContentHash("blake3:abc".into()),
            seed: 0xC0FFEE,
            normalised_yaml: "version: 1\n".into(),
            event_count: Some(10_000),
            time_origin_ns: 0,
            duration_ns: Some(60_000_000_000),
            corpus_summary: CorpusSummary::default(),
            stream_digest: "blake3:def".into(),
        }
    }

    #[test]
    fn round_trips_through_json() {
        let m = bounded();
        let back = Manifest::from_json(&m.to_json().unwrap()).unwrap();
        assert_eq!(back.seed, m.seed);
        assert_eq!(back.identity, m.identity);
    }

    #[test]
    fn an_unimplemented_plan_format_is_refused() {
        // The record has no length prefix, so guessing the width would
        // mis-align every field silently -- the dispatcher's own wire codec
        // being the cautionary precedent.
        let mut m = bounded();
        m.plan_format = 99;
        let e = Manifest::from_json(&m.to_json().unwrap()).unwrap_err();
        assert_eq!(
            e,
            ManifestError::UnsupportedFormat {
                found: 99,
                supported: PLAN_FORMAT
            }
        );
    }

    #[test]
    fn the_two_identity_kinds_stay_distinguishable() {
        let bounded_id = Identity::ContentHash("blake3:1".into());
        let unbounded_id: Identity = parameter_hash("version: 1", 1, PLAN_FORMAT).into();
        assert_ne!(bounded_id.label(), unbounded_id.label());
        // And they serialise as distinct JSON shapes, so a reader cannot mistake
        // one for the other even without the label.
        let a = serde_json::to_string(&bounded_id).unwrap();
        let b = serde_json::to_string(&unbounded_id).unwrap();
        assert!(a.contains("content_hash"), "{a}");
        assert!(b.contains("parameter_hash"), "{b}");
    }

    #[test]
    fn an_unbounded_run_has_no_event_count() {
        let mut m = bounded();
        m.event_count = None;
        m.duration_ns = None;
        m.identity = parameter_hash("version: 1", 1, PLAN_FORMAT).into();
        assert!(m.is_unbounded());
        let back = Manifest::from_json(&m.to_json().unwrap()).unwrap();
        assert!(back.is_unbounded());
    }

    #[test]
    fn malformed_json_is_reported_rather_than_panicking() {
        let e = Manifest::from_json("{not json").unwrap_err();
        assert!(matches!(e, ManifestError::Malformed(_)));
    }

    #[test]
    fn the_normalised_input_is_embedded_in_full() {
        // So a report is always traceable to its exact input, including
        // defaults that were applied rather than written.
        let mut m = bounded();
        m.normalised_yaml = "version: 1\nseed: 1\nrun:\n  mode: hardware\n".into();
        let back = Manifest::from_json(&m.to_json().unwrap()).unwrap();
        assert_eq!(back.normalised_yaml, m.normalised_yaml);
    }
}
