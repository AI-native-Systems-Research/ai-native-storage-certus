//! The one place the random number generator is chosen.
//!
//! # Why not `StdRng`
//!
//! `rand`'s `StdRng` and `SmallRng` are explicitly **not** reproducible across
//! `rand` releases — the crate reserves the right to change which algorithm they
//! name. FR-012 and FR-034 require a recorded seed to reproduce a plan, so using
//! either would produce a tool that appears to work and then silently stops
//! reproducing its own recorded seeds on a routine dependency bump. That failure
//! mode is invisible: the plan is still self-consistent, it just no longer
//! matches the one in the report.
//!
//! `ChaCha20Rng` is specified by its round count and is stable by contract, so
//! it is named here once and nowhere else.
//!
//! # Examples
//!
//! ```
//! use workload_model::rng;
//! use rand::Rng;
//!
//! let mut a = rng::seeded(42);
//! let mut b = rng::seeded(42);
//! assert_eq!(a.gen::<u64>(), b.gen::<u64>());
//!
//! let mut c = rng::seeded(43);
//! assert_ne!(rng::seeded(42).gen::<u64>(), c.gen::<u64>());
//! ```

use rand_chacha::rand_core::SeedableRng;

/// The generator every draw in this crate comes from.
pub type Rng = rand_chacha::ChaCha20Rng;

/// Build the generator for a run from its seed.
///
/// The seed is widened to ChaCha's 32-byte key by writing it little-endian into
/// the first eight bytes and leaving the rest zero, which is
/// `ChaCha20Rng::seed_from_u64`'s own documented behaviour — named here so the
/// mapping from a recorded `--seed` to a byte stream is stated somewhere.
///
/// # Examples
///
/// ```
/// use workload_model::rng;
/// use rand::Rng;
///
/// let mut r = rng::seeded(0);
/// let first: f64 = r.gen();
/// assert!((0.0..1.0).contains(&first));
/// ```
pub fn seeded(seed: u64) -> Rng {
    Rng::seed_from_u64(seed)
}

/// Build an independent generator for a named sub-stream of the same run.
///
/// Distinct concerns draw from distinct streams, so that adding a draw in one
/// place cannot shift every later draw everywhere else. Without this, inserting
/// a single new sample during development silently changes an entire recorded
/// plan, and every previously recorded seed stops reproducing.
///
/// # Examples
///
/// ```
/// use workload_model::rng;
/// use rand::Rng;
///
/// let a: u64 = rng::substream(1, "sessions").gen();
/// let b: u64 = rng::substream(1, "pools").gen();
/// assert_ne!(a, b);
///
/// // Same name, same seed, same stream.
/// let c: u64 = rng::substream(1, "sessions").gen();
/// assert_eq!(a, c);
/// ```
pub fn substream(seed: u64, name: &str) -> Rng {
    // The name is folded in with the crate's own splitmix64 rather than a
    // standard-library hasher, for the same reason keys use it: `DefaultHasher`
    // is not stable across Rust releases, and a shifting stream index would
    // break reproducibility exactly as a shifting key function would.
    let mut mixed = crate::keys::splitmix64(seed);
    for byte in name.as_bytes() {
        mixed = crate::keys::splitmix64(mixed ^ u64::from(*byte));
    }
    Rng::seed_from_u64(mixed)
}
