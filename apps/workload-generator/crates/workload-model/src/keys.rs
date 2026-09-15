//! Cache-key derivation — the normative implementation of
//! `contracts/key-derivation.md`.
//!
//! A block's key is a `u64` derived from its own identity **and its parent's
//! key**, so reuse between two sessions requires a matching *leading run* of
//! blocks. An object the two hold in common at differing positions contributes
//! no reuse at all. That is the whole sharing mechanism, and it is why the key
//! function is specified in a contract rather than left to this file.
//!
//! # Why not a hashing crate
//!
//! Certus treats a key as an opaque `u64`, so any function would work
//! *locally*. It would not work across machines:
//! [`std::collections::hash_map::DefaultHasher`] (SipHash) is not stable across
//! Rust releases and `ahash` is not stable across its own. Either would give
//! two nodes different keys for the same object while appearing to work, which
//! costs every cross-node hit and reports no error (FR-029). So the mix
//! function is written out here, zero-dependency, and pinned by test vectors
//! that any other tool can reimplement against.
//!
//! # Examples
//!
//! A chain root uses [`ROOT_PARENT`]; each subsequent block chains onto the tip.
//!
//! ```
//! use workload_model::keys::{self, ROOT_PARENT};
//!
//! let first = keys::key(ROOT_PARENT, keys::input_salt(7, 0));
//! let second = keys::key(first, keys::input_salt(7, 1));
//! assert_ne!(first, second);
//!
//! // The same coordinates always give the same key, on any machine.
//! assert_eq!(first, keys::key(ROOT_PARENT, keys::input_salt(7, 0)));
//! ```
//!
//! Salts are partitioned by block kind, so a shared block and a session block
//! cannot collide however their coordinates line up:
//!
//! ```
//! use workload_model::keys;
//!
//! assert_ne!(keys::shared_salt(0, 7, 3), keys::input_salt(7, 3));
//! ```

/// A Certus cache key. Opaque to the server, which is why this crate owns the
/// key space (`components/interfaces/src/idispatch_map.rs:6`).
pub type CacheKey = u64;

/// The parent of a chain root.
pub const ROOT_PARENT: CacheKey = 0;

/// Tag for a shared-object block, occupying bits 48 and above of the salt.
const SHARED_TAG: u64 = 1;
/// Tag for a session input block.
const INPUT_TAG: u64 = 2;
/// Tag for a session output block.
const OUTPUT_TAG: u64 = 3;

/// Width of the `block_ordinal` field, and of `instance_index` and `class_id`.
const FIELD16: u64 = 1 << 16;
/// Width of the `session_id` field, which spans bits 16..48.
const FIELD32: u64 = 1 << 32;

/// The `splitmix64` mix function, verbatim from the contract.
///
/// A pure function of its argument — *not* a generator advancing a seed, which
/// is how splitmix64 is normally used. Keys must depend on identity, never on
/// call order.
///
/// # Examples
///
/// ```
/// use workload_model::keys::splitmix64;
///
/// assert_eq!(splitmix64(0), 0xe220_a839_7b1d_cdaf);
/// ```
#[inline]
pub fn splitmix64(x: u64) -> u64 {
    let x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Derive a block's key from its parent's key and its own salt.
///
/// `key(parent, salt) = splitmix64(splitmix64(parent) ^ salt)`.
///
/// The inner mix on `parent` is not redundant. Without it, a low-entropy salt
/// would leave the parent's structure visible in the child's low bits, letting
/// two different chains collide there.
///
/// # Examples
///
/// ```
/// use workload_model::keys::{key, splitmix64, ROOT_PARENT};
///
/// assert_eq!(key(ROOT_PARENT, 1), splitmix64(splitmix64(0) ^ 1));
/// ```
#[inline]
pub fn key(parent: CacheKey, salt: u64) -> CacheKey {
    splitmix64(splitmix64(parent) ^ salt)
}

/// Salt for a shared-object block.
///
/// `class_id` is the class's **declaration index** in the description, not a
/// hash of its name: renaming a shared class does not change any key, but
/// reordering declarations does. Declaration order is already semantic — it
/// fixes the order shared prefixes appear in a chain — so this is the intended
/// trade rather than a limitation.
///
/// # Panics
///
/// If any coordinate exceeds its 16-bit field. The salt is assembled with XOR,
/// so an overflowing field does not saturate: it corrupts its neighbour and
/// aliases two different blocks onto one key, which would show up only as an
/// inexplicable cache hit. Failing loudly is the point.
///
/// # Examples
///
/// ```
/// use workload_model::keys::shared_salt;
///
/// // Tag 1 in the high bits, then class, instance, ordinal.
/// assert_eq!(shared_salt(0, 0, 0) >> 48, 1);
/// ```
#[inline]
pub fn shared_salt(class_id: u64, instance_index: u64, block_ordinal: u64) -> u64 {
    assert!(
        class_id < FIELD16,
        "class_id {class_id} exceeds its 16-bit salt field; keys would alias"
    );
    assert!(
        instance_index < FIELD16,
        "instance_index {instance_index} exceeds its 16-bit salt field; keys would alias"
    );
    assert!(
        block_ordinal < FIELD16,
        "block_ordinal {block_ordinal} exceeds its 16-bit salt field; keys would alias"
    );
    (SHARED_TAG << 48) ^ (class_id << 32) ^ (instance_index << 16) ^ block_ordinal
}

/// Salt for a session **input** block.
///
/// # Panics
///
/// If `session_id` exceeds its 32-bit field or `block_ordinal` its 16-bit one —
/// see [`shared_salt`] for why this is an assertion rather than a truncation.
///
/// # Examples
///
/// ```
/// use workload_model::keys::input_salt;
///
/// assert_eq!(input_salt(7, 3) >> 48, 2);
/// ```
#[inline]
pub fn input_salt(session_id: u64, block_ordinal: u64) -> u64 {
    session_salt(INPUT_TAG, session_id, block_ordinal)
}

/// Salt for a session **output** block.
///
/// Output blocks are stored and chained exactly like input blocks (FR-026);
/// only the tag differs, which keeps the two streams from colliding.
///
/// # Panics
///
/// As [`input_salt`].
///
/// # Examples
///
/// ```
/// use workload_model::keys::output_salt;
///
/// assert_eq!(output_salt(7, 3) >> 48, 3);
/// ```
#[inline]
pub fn output_salt(session_id: u64, block_ordinal: u64) -> u64 {
    session_salt(OUTPUT_TAG, session_id, block_ordinal)
}

#[inline]
fn session_salt(tag: u64, session_id: u64, block_ordinal: u64) -> u64 {
    assert!(
        session_id < FIELD32,
        "session_id {session_id} exceeds its 32-bit salt field; keys would alias"
    );
    assert!(
        block_ordinal < FIELD16,
        "block_ordinal {block_ordinal} exceeds its 16-bit salt field; keys would alias"
    );
    (tag << 48) ^ (session_id << 16) ^ block_ordinal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_chains_forward() {
        let a = key(ROOT_PARENT, input_salt(0, 0));
        let b = key(a, input_salt(0, 1));
        assert_ne!(a, b);
    }

    #[test]
    fn tags_do_not_overlap_coordinates() {
        // Every tag must survive in the high bits for every legal coordinate.
        assert_eq!(
            shared_salt(u16::MAX as u64, u16::MAX as u64, u16::MAX as u64) >> 48,
            SHARED_TAG
        );
        assert_eq!(
            input_salt(u32::MAX as u64, u16::MAX as u64) >> 48,
            INPUT_TAG
        );
        assert_eq!(
            output_salt(u32::MAX as u64, u16::MAX as u64) >> 48,
            OUTPUT_TAG
        );
    }
}
