//! Cache-key derivation — the normative implementation of
//! `contracts/key-derivation.md`.
//!
//! A block's key is a `u64` derived from its own identity **and its parent's
//! key**, so reuse between two sessions requires a matching *leading run* of
//! blocks. An object the two hold in common at differing positions contributes
//! no reuse at all. That is the whole sharing mechanism, and it is why the key
//! function is specified in a contract rather than left to this file.
//!
//! # Why derive a key at all, rather than mint one
//!
//! Not for cross-node agreement — that is worth stating, because it is the
//! intuitive answer and it is wrong. A run has exactly one generator process;
//! the per-node daemons relay keys and never derive them, and Certus treats a
//! key as opaque. During a live run no second party ever computes a key, so two
//! nodes cannot disagree about one whatever function is used.
//!
//! The reason is that deriving makes a prefix **stateless**. The alternative —
//! mint keys from a counter and remember which prefix path got which key — needs
//! a global prefix trie with a lookup on the per-key hot path, where the budget
//! is a few hundred megabytes of live-key state and a per-key cost far below
//! Certus's own 5.6–14 µs.
//!
//! # Why not a hashing crate
//!
//! Because of the one genuine *second* implementation: an offline consumer
//! analysing a trace it did not produce must recompute its keys to check them.
//! [`std::collections::hash_map::DefaultHasher`] (SipHash) is not stable across
//! Rust releases and `ahash` is not stable across its own, so either would make
//! a trace verifiable only by the exact binary that wrote it — and silently, since
//! the trace still loads and replays. So the mix function is written out here,
//! zero-dependency, and pinned by test vectors any other tool can reimplement
//! against.
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

/// Version of the key derivation, recorded in a trace manifest.
///
/// Bumped only when the derivation changes, which per this module's Versioning
/// rules is a new version and never an edit — a consumer that cannot compute this
/// version must refuse the trace rather than derive different keys from it.
pub const KEY_DERIVATION_VERSION: u32 = 1;

// Salt field layout, per `contracts/key-derivation.md`. Both arrangements use
// all 64 bits exactly: 2 + 12 + 26 + 24 for a shared block, 2 + 38 + 24 for a
// session block. Each field's *width is the gap to the next*, which is why the
// widths are part of the contract rather than an implementation detail — an
// overflowing field would alias two different blocks onto one key.
//
// The tag deliberately sits in the top 2 bits. An earlier revision put it at
// bit 48 with every field 16 bits wide, which spent 16 bits on a 3-value tag
// while capping `block_ordinal` and `instance_index` at 65 536 — both reachable,
// and both aliasing silently when exceeded.

/// Bit position of the block-kind tag.
const TAG_SHIFT: u32 = 62;
/// Tag for a shared-object block. Tag `0` is unused and reserved.
const SHARED_TAG: u64 = 1;
/// Tag for a session input block.
const INPUT_TAG: u64 = 2;
/// Tag for a session output block.
const OUTPUT_TAG: u64 = 3;

/// `class_id` occupies bits 50..61 — 4 096 shared classes.
const CLASS_SHIFT: u32 = 50;
const CLASS_LIMIT: u64 = 1 << 12;
/// `instance_index` occupies bits 24..49 — 67 108 864 mints per class per run.
const INSTANCE_SHIFT: u32 = 24;
const INSTANCE_LIMIT: u64 = 1 << 26;
/// `session_id` occupies bits 24..61 — 274 877 906 944 sessions per run.
const SESSION_SHIFT: u32 = 24;
const SESSION_LIMIT: u64 = 1 << 38;
/// `block_ordinal` occupies bits 0..23 — 16 777 216 blocks per instance or
/// per session stream.
const ORDINAL_LIMIT: u64 = 1 << 24;

/// Most shared classes a description may declare.
///
/// These four limits are what the salt layout can encode. They exist as public
/// constants so the description validator can refuse an over-large description
/// at load time, which turns what would be a panic part-way through a run —
/// destroying its output — into an error before anything is issued. All four are
/// orders of magnitude beyond the scale target of 10 000 concurrent sessions and
/// 10 000 000 live keys, so a description that trips one is a mistake rather
/// than an ambition.
///
/// # Examples
///
/// ```
/// use workload_model::keys;
///
/// assert_eq!(keys::MAX_CLASSES, 4_096);
/// assert_eq!(keys::MAX_BLOCKS_PER_STREAM, 16_777_216);
/// ```
pub const MAX_CLASSES: u64 = CLASS_LIMIT;

/// Most instances a single shared class may ever mint in one run.
///
/// This bounds *total mints*, not the live count: the salt's `instance_index` is
/// an instance's monotonic mint counter, because a replacement must not inherit
/// its dead predecessor's keys. The live count is bounded by this too, being no
/// larger, and that much is checkable at load; the mint total depends on the
/// run's span and so belongs to the pre-flight projection.
pub const MAX_INSTANCES_PER_POOL: u64 = INSTANCE_LIMIT;

/// Most blocks in one shared instance, or in one session's input or output
/// stream.
pub const MAX_BLOCKS_PER_STREAM: u64 = ORDINAL_LIMIT;

/// Most sessions one run may create.
pub const MAX_SESSIONS_PER_RUN: u64 = SESSION_LIMIT;

// Compile-time proof that the fields tile the word exactly, with each field's
// width equal to the gap to the next. If a later edit moves one shift without
// moving its neighbour, this fails to *compile* rather than silently overlapping
// two fields and aliasing keys — which is the one failure mode here that no test
// would notice, because both fields would still round-trip on their own.
const _: () = {
    assert!(CLASS_SHIFT + CLASS_LIMIT.trailing_zeros() == TAG_SHIFT);
    assert!(INSTANCE_SHIFT + INSTANCE_LIMIT.trailing_zeros() == CLASS_SHIFT);
    assert!(SESSION_SHIFT + SESSION_LIMIT.trailing_zeros() == TAG_SHIFT);
    assert!(ORDINAL_LIMIT.trailing_zeros() == INSTANCE_SHIFT);
    assert!(ORDINAL_LIMIT.trailing_zeros() == SESSION_SHIFT);
    // The tag has room for its three values and no more.
    assert!(OUTPUT_TAG < 1 << (64 - TAG_SHIFT));
};

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
/// If any coordinate exceeds its field: `class_id` 12 bits, `instance_index`
/// 26 bits, `block_ordinal` 24 bits. The salt is assembled with XOR, so an
/// overflowing field does not saturate — it corrupts its neighbour and aliases
/// two different blocks onto one key, which would show up only as an
/// inexplicable cache hit. Failing loudly is the point.
///
/// This is not the same posture as the deliberately-unhandled birthday
/// collision. That is random, happens at a rate of 10^-6 per run, and cannot
/// bias a measurement; an overflowing coordinate aliases two *specific* blocks
/// on every run for as long as the description is used.
///
/// # Examples
///
/// ```
/// use workload_model::keys::shared_salt;
///
/// // Tag 1 in the top two bits, then class, instance, ordinal.
/// assert_eq!(shared_salt(0, 0, 0) >> 62, 1);
/// ```
#[inline]
pub fn shared_salt(class_id: u64, instance_index: u64, block_ordinal: u64) -> u64 {
    assert!(
        class_id < CLASS_LIMIT,
        "class_id {class_id} exceeds its 12-bit salt field; keys would alias"
    );
    assert!(
        instance_index < INSTANCE_LIMIT,
        "instance_index {instance_index} exceeds its 26-bit salt field; keys would alias"
    );
    assert!(
        block_ordinal < ORDINAL_LIMIT,
        "block_ordinal {block_ordinal} exceeds its 24-bit salt field; keys would alias"
    );
    (SHARED_TAG << TAG_SHIFT)
        ^ (class_id << CLASS_SHIFT)
        ^ (instance_index << INSTANCE_SHIFT)
        ^ block_ordinal
}

/// Salt for a session **input** block.
///
/// # Panics
///
/// If `session_id` exceeds its 38-bit field or `block_ordinal` its 24-bit one —
/// see [`shared_salt`] for why this is an assertion rather than a truncation.
///
/// # Examples
///
/// ```
/// use workload_model::keys::input_salt;
///
/// assert_eq!(input_salt(7, 3) >> 62, 2);
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
/// assert_eq!(output_salt(7, 3) >> 62, 3);
/// ```
#[inline]
pub fn output_salt(session_id: u64, block_ordinal: u64) -> u64 {
    session_salt(OUTPUT_TAG, session_id, block_ordinal)
}

#[inline]
fn session_salt(tag: u64, session_id: u64, block_ordinal: u64) -> u64 {
    assert!(
        session_id < SESSION_LIMIT,
        "session_id {session_id} exceeds its 38-bit salt field; keys would alias"
    );
    assert!(
        block_ordinal < ORDINAL_LIMIT,
        "block_ordinal {block_ordinal} exceeds its 24-bit salt field; keys would alias"
    );
    (tag << TAG_SHIFT) ^ (session_id << SESSION_SHIFT) ^ block_ordinal
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
        // Every tag must survive in the top bits for every legal coordinate.
        // With all fields at their maximum the three salts are 7fff..., bfff...
        // and ffff..., which is what "the layout uses all 64 bits exactly and
        // the fields are disjoint" looks like written down.
        let shared = shared_salt(CLASS_LIMIT - 1, INSTANCE_LIMIT - 1, ORDINAL_LIMIT - 1);
        let input = input_salt(SESSION_LIMIT - 1, ORDINAL_LIMIT - 1);
        let output = output_salt(SESSION_LIMIT - 1, ORDINAL_LIMIT - 1);
        assert_eq!(shared, 0x7fff_ffff_ffff_ffff);
        assert_eq!(input, 0xbfff_ffff_ffff_ffff);
        assert_eq!(output, 0xffff_ffff_ffff_ffff);
        assert_eq!(shared >> TAG_SHIFT, SHARED_TAG);
        assert_eq!(input >> TAG_SHIFT, INPUT_TAG);
        assert_eq!(output >> TAG_SHIFT, OUTPUT_TAG);
    }

    // The field *arithmetic* is checked by the `const _` block near the top of
    // this file rather than here. That is strictly stronger than a test: a
    // layout whose fields overlap does not compile at all.
}
