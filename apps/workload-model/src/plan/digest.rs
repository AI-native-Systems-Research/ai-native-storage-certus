//! Content hashing and stream digests.
//!
//! Two kinds of identity, and a report must say which it carries, because they
//! answer different questions and confusing them would let an unbounded run
//! masquerade as a bounded one.
//!
//! - [`PlanDigest::Content`] — a hash over the realised events. Available only
//!   for a bounded plan, and the strongest statement: *these exact events*.
//! - [`PlanDigest::Parameters`] — a hash over the normalised YAML, the seed and
//!   `plan_format`. What an unbounded run carries, since it has no final event
//!   set to hash. Sufficient because generation is fully determined by exactly
//!   those inputs (spec FR-024).

use crate::keys::CacheKey;

/// Which kind of identity an artifact carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanDigest {
    /// Hash over the realised events; bounded plans only.
    Content(String),
    /// Hash over normalised YAML + seed + `plan_format`; unbounded runs.
    Parameters(String),
}

impl PlanDigest {
    /// The `blake3:`-prefixed hex form written into a manifest.
    pub fn as_str(&self) -> &str {
        match self {
            PlanDigest::Content(s) | PlanDigest::Parameters(s) => s,
        }
    }

    /// Human label, so a report can never present one kind as the other.
    pub fn kind(&self) -> &'static str {
        match self {
            PlanDigest::Content(_) => "content-hash (bounded plan)",
            PlanDigest::Parameters(_) => "parameter-hash (unbounded run)",
        }
    }
}

fn fmt(h: blake3::Hash) -> String {
    format!("blake3:{}", h.to_hex())
}

/// Hash the generator's identity: normalised input, seed, and format version.
pub fn parameter_hash(normalised_yaml: &str, seed: u64, plan_format: u32) -> PlanDigest {
    let mut h = blake3::Hasher::new();
    h.update(normalised_yaml.as_bytes());
    h.update(&seed.to_le_bytes());
    h.update(&plan_format.to_le_bytes());
    PlanDigest::Parameters(fmt(h.finalize()))
}

/// Accumulates a digest over a consumed key sequence.
///
/// Every consumer of a plan emits one (spec FR-036). Two arms with equal digests
/// provably saw the identical stream, which is this tool's whole contribution to
/// a comparison's validity — and a comparison between differing digests is
/// refused rather than reported.
#[derive(Debug, Default, Clone)]
pub struct StreamDigest {
    hasher: blake3::Hasher,
    count: u64,
}

impl StreamDigest {
    /// A fresh digest.
    pub fn new() -> Self {
        Self::default()
    }

    /// Absorb one key, in consumption order.
    pub fn push(&mut self, key: CacheKey) {
        self.hasher.update(&key.0.to_le_bytes());
        self.count += 1;
    }

    /// How many keys were absorbed.
    pub fn len(&self) -> u64 {
        self.count
    }

    /// Whether nothing was absorbed.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Finish, returning the `blake3:`-prefixed digest.
    pub fn finish(&self) -> String {
        let mut h = self.hasher.clone();
        h.update(&self.count.to_le_bytes());
        fmt(h.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_streams_agree() {
        let keys = [CacheKey(1), CacheKey(2), CacheKey(3)];
        let mut a = StreamDigest::new();
        let mut b = StreamDigest::new();
        for k in keys {
            a.push(k);
            b.push(k);
        }
        assert_eq!(a.finish(), b.finish());
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn order_matters() {
        // A reordered stream is a different stream: two arms that saw the same
        // keys in a different order did not see the same workload.
        let mut a = StreamDigest::new();
        let mut b = StreamDigest::new();
        for k in [CacheKey(1), CacheKey(2)] {
            a.push(k);
        }
        for k in [CacheKey(2), CacheKey(1)] {
            b.push(k);
        }
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn a_prefix_is_not_the_whole() {
        // Length is folded in, so a truncated run cannot collide with a
        // complete one that happens to start the same way.
        let mut a = StreamDigest::new();
        let mut b = StreamDigest::new();
        a.push(CacheKey(1));
        b.push(CacheKey(1));
        b.push(CacheKey(2));
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn parameter_hash_depends_on_every_input() {
        let base = parameter_hash("corpus: {}", 1, 1);
        assert_ne!(base, parameter_hash("corpus: {}", 2, 1));
        assert_ne!(base, parameter_hash("corpus: {}", 1, 2));
        assert_ne!(base, parameter_hash("corpus: {a: 1}", 1, 1));
        assert_eq!(base, parameter_hash("corpus: {}", 1, 1));
    }

    #[test]
    fn the_two_digest_kinds_are_labelled_distinctly() {
        // So a report can never present a parameter hash as a plan hash.
        let p = parameter_hash("x", 1, 1);
        let c = PlanDigest::Content("blake3:00".into());
        assert_ne!(p.kind(), c.kind());
    }
}
