// The REAL count-min sketch, compiled in situ — the sketch-side twin of `lru_real.rs`.
//
// `real` below is the text of `components/eviction-policy-optimized/src/lib.rs` from
// `const CMS_ROWS` through the end of `impl CountMinSketch`, cut out of the production file
// by build.rs on every build and pulled in with `include!`. It is NOT a transcription: the
// constants (`CMS_ROWS`, `CMS_COLS`, the four `CMS_PRIMES`), the struct and the bodies of
// `new` / `increment` / `estimate` / `halve` are the shipped tokens, and build.rs fails the
// build if the region it expects is not there. Editing the production sketch therefore
// changes what the harnesses that use this module check.
//
// Why a cut and not the whole file: the rest of `src/lib.rs` needs `component-framework`
// (`define_component!`), which this crate cannot depend on directly (see Cargo.toml). The
// sketch region depends on nothing but `interfaces::CacheKey`, imported below.
//
// `sketch_core.rs` (the older transcription) is still used by the other sketch harnesses;
// the harnesses re-proved on 2026-10-07 use this module instead.

pub mod real {
    use interfaces::CacheKey;

    include!(concat!(env!("OUT_DIR"), "/sketch_real.rs"));

    /// Read-only access to the private sketch representation, for stating properties.
    /// A CHILD of the module holding `CountMinSketch`, so it may read `counters`.
    #[cfg(kani)]
    pub(crate) mod inspect {
        use super::{CountMinSketch, CMS_COLS, CMS_PRIMES, CMS_ROWS};

        pub const ROWS: usize = CMS_ROWS;
        pub const COLS: usize = CMS_COLS;

        /// The production multiplier table itself (lib.rs `CMS_PRIMES`).
        pub fn primes() -> [u64; CMS_ROWS] {
            CMS_PRIMES
        }
        /// The production `CountMinSketch` is a private type, so it is carried in an opaque
        /// wrapper; every method below forwards to the real one unchanged.
        pub struct Sketch(CountMinSketch);

        pub fn fresh() -> Sketch {
            Sketch(CountMinSketch::new())
        }
        pub fn increment(s: &mut Sketch, key: u64) {
            s.0.increment(key)
        }
        pub fn estimate(s: &Sketch, key: u64) -> u8 {
            s.0.estimate(key)
        }
        pub fn counter(s: &Sketch, row: usize, col: usize) -> u8 {
            s.0.counters[row][col]
        }
    }
}
