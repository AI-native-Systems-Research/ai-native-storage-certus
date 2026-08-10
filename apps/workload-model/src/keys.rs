//! Key derivation: the trie whose node identity *is* the hash of the path to it.
//!
//! Three derivations, deliberately kept as three functions rather than one with
//! flags, because the namespaces they inhabit are the invariant:
//!
//! - [`root`] — a depth-0 key.
//! - [`trunk_child`] — a shared node, identified by its path alone.
//! - [`private_child`] — a node private to the session that **minted** it.
//!
//! Two properties everything else depends on:
//!
//! **Divergence is irreversible.** A node's identity is a hash over the path to
//! it, so once two sessions take different children every key below that point
//! differs, forever. Sharing is therefore necessarily a monotone prefix property
//! — which is why the schema offers no per-depth *sharing* table.
//!
//! **Identity is computable from the path alone.** No arrival order, no global
//! mutable state. That is what buys O(active paths) memory, independent per-node
//! generation, and `corpus` staying orthogonal to `workload`.
//!
//! ```
//! use workload_model::keys::{root, trunk_child, Generation};
//! let r = root(0, Generation::STABLE);
//! let a = trunk_child(r, 0, Generation::STABLE);
//! let b = trunk_child(r, 1, Generation::STABLE);
//! assert_ne!(a, b);                       // siblings differ
//! assert_eq!(a, trunk_child(r, 0, Generation::STABLE)); // and are reproducible
//! ```

use serde::{Deserialize, Serialize};

/// An opaque KV block identity.
///
/// Not an index and not meaningfully ordered: in the vLLM path this is a rolling
/// hash over the block chain, and the only operations that mean anything are
/// equality and hashing.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CacheKey(pub u64);

/// Which session **minted** a key — not which reads it.
///
/// The distinction is load-bearing. An agent-fan-out child reads keys its parent
/// minted, so it passes the *parent's* id for the inherited prefix and its own
/// below the spawn point.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u32);

/// Which generation of shared content a trunk node belongs to.
///
/// Advances only when `corpus.trees.churn` is configured. [`Generation::STABLE`]
/// means no churn, and reduces the derivation to exactly the pre-churn form.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Generation(pub u32);

impl Generation {
    /// The no-churn generation. The default everywhere.
    pub const STABLE: Generation = Generation(0);

    /// Whether this is the no-churn generation.
    pub fn is_stable(&self) -> bool {
        self.0 == 0
    }
}

// Domain separators. Distinct constants rather than distinct call sites, so the
// namespaces cannot collide even if a caller passes the wrong function's
// arguments.
const TAG_ROOT: u8 = 1;
const TAG_TRUNK: u8 = 2;
const TAG_PRIVATE: u8 = 3;

/// Hash the domain tag and the given words into a key.
fn derive(tag: u8, words: &[u64]) -> CacheKey {
    let mut h = blake3::Hasher::new();
    h.update(&[tag]);
    for w in words {
        h.update(&w.to_le_bytes());
    }
    let mut out = [0u8; 8];
    out.copy_from_slice(&h.finalize().as_bytes()[..8]);
    CacheKey(u64::from_le_bytes(out))
}

/// A depth-0 key: the start of one tree in the forest.
pub fn root(root_index: u32, gen: Generation) -> CacheKey {
    if gen.is_stable() {
        derive(TAG_ROOT, &[u64::from(root_index)])
    } else {
        derive(TAG_ROOT, &[u64::from(root_index), u64::from(gen.0)])
    }
}

/// A shared trunk node: `H(parent, child_index, generation)`.
///
/// With [`Generation::STABLE`] the generation term is omitted entirely, so a
/// build that never configures churn produces byte-identical keys to one with no
/// churn concept at all (spec FR-008). That equivalence is asserted in the tests
/// and is what makes churn a genuinely opt-in feature rather than a change to
/// every existing plan.
pub fn trunk_child(parent: CacheKey, child_index: u32, gen: Generation) -> CacheKey {
    if gen.is_stable() {
        derive(TAG_TRUNK, &[parent.0, u64::from(child_index)])
    } else {
        derive(
            TAG_TRUNK,
            &[parent.0, u64::from(child_index), u64::from(gen.0)],
        )
    }
}

/// A node private to the session that minted it.
///
/// The namespace is keyed on `minting_session` — **not** on whoever is reading.
/// For an ordinary session the two coincide. For an agent-fan-out child they do
/// not: the child passes its parent's id for the inherited prefix, so it derives
/// the parent's keys rather than different ones.
///
/// Keying this on the reader instead would give the child a fresh key for
/// content the parent already holds, turning every fan-out into a miss storm
/// that looks like a cache result but is a generator artefact. The test
/// `reader_id_would_produce_different_keys` pins that down.
pub fn private_child(parent: CacheKey, minting_session: SessionId, index: u32) -> CacheKey {
    derive(
        TAG_PRIVATE,
        &[parent.0, u64::from(minting_session.0), u64::from(index)],
    )
}

/// Entry size in bytes for `key`, drawn from `block_bytes`.
///
/// A **pure function of key identity** (spec FR-011): the draw is keyed on the
/// key's own value, never on position in the stream. Two consequences:
///
/// - The same key has the same size in every run and at every position, so a
///   consumer reporting a size mismatch has found a real disagreement rather
///   than an artefact of when the key was asked for.
/// - It is what licenses FR-039b's inference — that a non-zero `SIZE_MISMATCH`
///   indicates a generator or plan defect. That reading is only valid while this
///   holds, which is why it is tested rather than merely documented.
pub fn entry_size(key: CacheKey, block_bytes: &crate::dist::Dist) -> u32 {
    let mut st = crate::rng::Stream::new(key.0, u64::from(TAG_PRIVATE));
    let v = block_bytes.sample_u64(&mut st);
    v.min(u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::{Dist, Shape};

    const S: SessionId = SessionId(7);
    const T: SessionId = SessionId(8);

    #[test]
    fn stable_generation_reduces_to_the_pre_churn_form() {
        // FR-008: the default must be bit-identical to a build with no churn
        // concept. Asserted by construction: the generation word is absent.
        let parent = root(3, Generation::STABLE);
        let with_stable = trunk_child(parent, 5, Generation::STABLE);
        let by_hand = derive(TAG_TRUNK, &[parent.0, 5]);
        assert_eq!(with_stable, by_hand);
    }

    #[test]
    fn a_rotated_generation_changes_the_key() {
        let parent = root(3, Generation::STABLE);
        assert_ne!(
            trunk_child(parent, 5, Generation::STABLE),
            trunk_child(parent, 5, Generation(1))
        );
    }

    #[test]
    fn rotating_a_node_invalidates_its_whole_subtree() {
        // The rolling hash does this implicitly: a changed parent rehashes
        // every descendant. One parameter therefore covers whole-tree and
        // per-branch replacement.
        let old = trunk_child(root(0, Generation::STABLE), 0, Generation::STABLE);
        let new = trunk_child(root(0, Generation::STABLE), 0, Generation(1));
        for d in 0..8 {
            assert_ne!(
                trunk_child(old, d, Generation::STABLE),
                trunk_child(new, d, Generation::STABLE)
            );
        }
    }

    #[test]
    fn private_keys_follow_the_minter_not_the_reader() {
        // FR-009c. A fan-out child passes its PARENT's id and must therefore
        // derive the parent's keys.
        let p = root(0, Generation::STABLE);
        let parent_minted = private_child(p, S, 0);
        let child_reading_parents_prefix = private_child(p, S, 0);
        assert_eq!(parent_minted, child_reading_parents_prefix);
    }

    #[test]
    fn reader_id_would_produce_different_keys() {
        // The failure this guards against: key on the reader and the child
        // derives a fresh key for content the parent already holds, so every
        // fan-out becomes a miss storm that reads as a cache result.
        let p = root(0, Generation::STABLE);
        assert_ne!(private_child(p, S, 0), private_child(p, T, 0));
    }

    #[test]
    fn trunk_and_private_namespaces_are_disjoint_by_construction() {
        // FR-007: disjoint by domain tag, not probabilistically. Same words
        // into both derivations must not collide.
        let p = root(1, Generation::STABLE);
        for i in 0..256 {
            assert_ne!(
                trunk_child(p, i, Generation::STABLE),
                private_child(p, SessionId(i), i)
            );
        }
        // And a root can never equal a trunk or private node.
        for i in 0..64u32 {
            let r = root(i, Generation::STABLE);
            assert_ne!(r, trunk_child(r, i, Generation::STABLE));
            assert_ne!(r, private_child(r, SessionId(i), i));
        }
    }

    #[test]
    fn siblings_differ_and_are_reproducible() {
        let p = root(0, Generation::STABLE);
        let kids: Vec<_> = (0..64)
            .map(|i| trunk_child(p, i, Generation::STABLE))
            .collect();
        let again: Vec<_> = (0..64)
            .map(|i| trunk_child(p, i, Generation::STABLE))
            .collect();
        assert_eq!(kids, again);
        let uniq: std::collections::HashSet<_> = kids.iter().collect();
        assert_eq!(uniq.len(), kids.len(), "sibling collision");
    }

    #[test]
    fn divergence_is_irreversible() {
        // Once two paths differ, every key below differs -- so sharing can only
        // ever be a monotone prefix property.
        let r = root(0, Generation::STABLE);
        let mut a = trunk_child(r, 0, Generation::STABLE);
        let mut b = trunk_child(r, 1, Generation::STABLE);
        for _ in 0..32 {
            // Same child index on both branches: content "identical", keys not.
            a = trunk_child(a, 0, Generation::STABLE);
            b = trunk_child(b, 0, Generation::STABLE);
            assert_ne!(a, b);
        }
    }

    #[test]
    fn entry_size_is_a_pure_function_of_key_identity() {
        // FR-011. The same key must have the same size regardless of when or
        // where it is asked for -- which is what licenses FR-039b's reading of
        // a SIZE_MISMATCH as a generator defect.
        let bb = Dist::Shaped(Shape::Lognormal {
            median: 131_072.0,
            sigma: 0.4,
        });
        let keys: Vec<_> = (0..500)
            .map(|i| trunk_child(root(0, Generation::STABLE), i, Generation::STABLE))
            .collect();
        let first: Vec<_> = keys.iter().map(|k| entry_size(*k, &bb)).collect();
        // Re-derive in a different order; sizes must not move.
        let mut shuffled: Vec<_> = keys.iter().copied().enumerate().collect();
        shuffled.rotate_left(137);
        for (orig_idx, k) in shuffled {
            assert_eq!(entry_size(k, &bb), first[orig_idx]);
        }
    }

    #[test]
    fn entry_size_is_constant_for_a_constant_distribution() {
        let bb = Dist::Scalar(131_072.0);
        for i in 0..64 {
            let k = trunk_child(root(0, Generation::STABLE), i, Generation::STABLE);
            assert_eq!(entry_size(k, &bb), 131_072);
        }
    }
}
