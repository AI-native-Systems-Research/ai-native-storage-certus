//! The self-describing manifest (FR-056, FR-057).
//!
//! A reader must learn what a trace supports by reading this, never by recognising
//! which trace it is. That is why the fields that could be guessed from context are
//! written out anyway — `encoding`, `timestamp_is_synthetic`, `block_id_space` — and
//! why the one field a consumer is most likely to assume wrongly gets the loudest
//! treatment.
//!
//! # `block_id_space` is the field that stops a consumer guessing
//!
//! The corpus's own traces number blocks **densely, in creation order**, so a
//! consumer can and does treat an identifier as an index. This generator does not:
//! an identifier is the chained 64-bit key `contracts/key-derivation.md` specifies,
//! because stateless prefix derivation is what keeps a global trie off the per-key
//! path (FR-029).
//!
//! Anything that indexes an array by identifier will therefore break on our traces.
//! It should break **at the manifest**, loudly, rather than at the first key — which
//! is why the value is a named string rather than an omitted field.
//!
//! # It is written last, so completeness needs no flag
//!
//! FR-073: a trace directory without a manifest is incomplete **by construction**.
//! A run that dies of a full filesystem leaves a directory no reader will accept,
//! and there is no "complete" boolean to get wrong. So [`Manifest::to_json`] is
//! serialised and written after the last record, never before the first.

use serde::Serialize;
use workload_model::description::WorkloadDescription;
use workload_model::keys::KEY_DERIVATION_VERSION;

/// Statistics a reader needs before it starts, keyed by block size.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BlockStats {
    /// Sessions that contributed at least one row.
    pub sessions: u64,
    /// Rows written.
    pub invocations: u64,
    /// Distinct keys appearing anywhere in the trace.
    pub unique_blocks: u64,
}

/// A trace's self-description.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manifest {
    /// This trace's identifier, which every row repeats.
    pub trace_id: String,
    /// Always `"pre_hashed"`: this generator mints keys and never text.
    pub source_class: &'static str,
    /// Always `"synthetic"`.
    pub provenance: &'static str,
    /// Always `"rolling_prefix"` — a key depends on the whole run ahead of it.
    pub id_semantics: &'static str,
    /// Always `"chained_u64"`. **Not** dense mint order; see the module docs.
    pub block_id_space: &'static str,
    /// Which key derivation produced these identifiers.
    pub key_derivation_version: u32,
    /// Tokens per block.
    pub block_size: u64,
    /// Every block size present in the directory.
    pub block_sizes_available: Vec<u64>,
    /// Bytes per block on the wire, from the description.
    pub block_bytes: u64,
    /// Always `"seconds"`.
    pub time_unit: &'static str,
    /// Always `"run_start"`.
    pub time_origin: &'static str,
    /// Always `"start"`.
    pub timestamp_kind: &'static str,
    /// Always `true`: the clock is virtual, not measured.
    pub timestamp_is_synthetic: bool,
    /// Always `"full"` — both `full_*` and `new_*` fields are populated.
    pub encoding: &'static str,
    /// Always `{left: false, right: true}`: the span ended the run, so the last
    /// sessions are cut off mid-conversation.
    pub censoring: Censoring,
    /// Always `None`: there is no text, so no tokenizer was involved.
    pub tokenizer_name: Option<String>,
    /// Always `None`, for the same reason.
    pub chat_template_id: Option<String>,
    /// Non-cryptographic digest of the description that produced this trace.
    ///
    /// Enough to notice that a trace and a description have drifted apart. Not a
    /// signature and not collision-resistant against anyone trying.
    pub description_digest: String,
    /// The seed, so the run can be reproduced (FR-072).
    pub seed: u64,
    /// The span requested, in virtual seconds.
    pub span: f64,
    /// Counts, keyed by block size as a string so the JSON matches the corpus's
    /// shape.
    pub block_stats: std::collections::BTreeMap<String, BlockStats>,
}

/// Which ends of the observation window cut sessions short.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Censoring {
    /// Whether sessions were already running when observation began.
    ///
    /// **False**, and that deserves a note: sessions *are* seeded mid-conversation
    /// at `t = 0` (FR-015), but they are seeded with a residual number of turns and
    /// a **fresh chain**, so no row refers to a block minted before the trace
    /// begins. Nothing is missing from the left, which is what this field asks.
    pub left: bool,
    /// Whether the window ended while sessions were still running. Always true.
    pub right: bool,
}

impl Manifest {
    /// Build a manifest for a completed emit run.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::description::WorkloadDescription;
    /// use workload_trace::manifest::{BlockStats, Manifest};
    ///
    /// let yaml = r#"
    /// version: 1
    /// blocks: {tokens: 16, bytes: 32768}
    /// shared_classes:
    ///   m: {length: {constant: 2}, lifetime: {constant: .inf}}
    /// session_classes:
    ///   c:
    ///     pool: {size: {exact: 1}}
    ///     uses: [{class: m, count: {constant: 1}}]
    ///     turns: {constant: 2}
    ///     input_growth: {constant: 1}
    ///     output_growth: {constant: 1}
    ///     think_time: {constant: 1}
    /// "#;
    /// let d: WorkloadDescription = yaml.parse().unwrap();
    /// let m = Manifest::new("demo", &d, yaml, 42, 100.0, BlockStats::default());
    ///
    /// assert_eq!(m.block_id_space, "chained_u64");
    /// assert_eq!(m.block_size, 16);
    /// assert!(m.tokenizer_name.is_none());
    /// ```
    pub fn new(
        trace_id: &str,
        description: &WorkloadDescription,
        description_text: &str,
        seed: u64,
        span: f64,
        stats: BlockStats,
    ) -> Self {
        let block_size = description.blocks.tokens;
        let mut block_stats = std::collections::BTreeMap::new();
        block_stats.insert(block_size.to_string(), stats);
        Self {
            trace_id: trace_id.to_string(),
            source_class: "pre_hashed",
            provenance: "synthetic",
            id_semantics: "rolling_prefix",
            block_id_space: "chained_u64",
            key_derivation_version: KEY_DERIVATION_VERSION,
            block_size,
            block_sizes_available: vec![block_size],
            block_bytes: description.blocks.bytes,
            time_unit: "seconds",
            time_origin: "run_start",
            timestamp_kind: "start",
            timestamp_is_synthetic: true,
            encoding: "full",
            censoring: Censoring {
                left: false,
                right: true,
            },
            tokenizer_name: None,
            chat_template_id: None,
            description_digest: digest(description_text),
            seed,
            span,
            block_stats,
        }
    }

    /// Serialise as pretty JSON with a trailing newline.
    ///
    /// Pretty rather than compact: a manifest is read by people at least as often as
    /// by programs, and it is one small file per trace rather than per row.
    ///
    /// # Errors
    ///
    /// If serialisation fails, which for this type means a bug rather than bad input.
    pub fn to_json(&self) -> serde_json::Result<String> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }
}

/// A non-cryptographic digest of the description text, rendered as hex.
///
/// splitmix64 over the bytes, for the same reason the keys use it: a general-purpose
/// hasher would not be stable across toolchains, so two builds would disagree about
/// whether a trace matched its description.
fn digest(text: &str) -> String {
    let mut acc = 0x9e37_79b9_7f4a_7c15u64;
    for b in text.as_bytes() {
        acc = workload_model::keys::splitmix64(acc ^ *b as u64);
    }
    format!("{acc:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn description() -> (WorkloadDescription, &'static str) {
        let yaml = r#"
version: 1
blocks: {tokens: 32, bytes: 65536}
shared_classes:
  m: {length: {constant: 2}, lifetime: {constant: .inf}}
session_classes:
  c:
    pool: {size: {exact: 1}}
    uses: [{class: m, count: {constant: 1}}]
    turns: {constant: 2}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 1}
"#;
        (yaml.parse().unwrap(), yaml)
    }

    #[test]
    fn the_manifest_declares_what_a_consumer_would_otherwise_assume() {
        let (d, text) = description();
        let m = Manifest::new("t", &d, text, 1, 10.0, BlockStats::default());
        let json = m.to_json().unwrap();
        // The four a consumer is most likely to get wrong by assumption.
        for field in [
            "\"block_id_space\": \"chained_u64\"",
            "\"encoding\": \"full\"",
            "\"timestamp_is_synthetic\": true",
            "\"source_class\": \"pre_hashed\"",
        ] {
            assert!(json.contains(field), "manifest is missing {field}:\n{json}");
        }
        assert!(json.ends_with("}\n"), "no trailing newline");
    }

    #[test]
    fn block_geometry_comes_from_the_description() {
        let (d, text) = description();
        let m = Manifest::new("t", &d, text, 1, 10.0, BlockStats::default());
        assert_eq!(m.block_size, 32);
        assert_eq!(m.block_bytes, 65536);
        assert_eq!(m.block_sizes_available, vec![32]);
        assert!(m.block_stats.contains_key("32"));
    }

    #[test]
    fn the_digest_tracks_the_description_text() {
        let (d, text) = description();
        let a = Manifest::new("t", &d, text, 1, 10.0, BlockStats::default());
        let edited = text.replace("turns: {constant: 2}", "turns: {constant: 3}");
        let d2: WorkloadDescription = edited.parse().unwrap();
        let b = Manifest::new("t", &d2, &edited, 1, 10.0, BlockStats::default());
        assert_ne!(a.description_digest, b.description_digest);
        // And it is stable, or it could not be compared across runs.
        let c = Manifest::new("t", &d, text, 1, 10.0, BlockStats::default());
        assert_eq!(a.description_digest, c.description_digest);
        assert_eq!(a.description_digest.len(), 16, "expected 16 hex digits");
    }

    #[test]
    fn the_seed_and_span_are_recorded_so_a_run_can_be_reproduced() {
        let (d, text) = description();
        let m = Manifest::new("t", &d, text, 4242, 987.0, BlockStats::default());
        assert_eq!(m.seed, 4242);
        assert_eq!(m.span, 987.0);
    }

    #[test]
    fn left_censoring_is_false_and_the_reason_is_recorded() {
        // Sessions ARE seeded mid-conversation, but with a fresh chain — so no row
        // refers to a block minted before the trace begins, which is what `left`
        // asks. Asserted so the reasoning in the field's doc cannot silently rot.
        let (d, text) = description();
        let m = Manifest::new("t", &d, text, 1, 10.0, BlockStats::default());
        assert!(!m.censoring.left);
        assert!(m.censoring.right);
    }
}
