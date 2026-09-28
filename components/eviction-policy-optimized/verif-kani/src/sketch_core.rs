// The count-min-sketch arithmetic core.
//
// ---------------------------------------------------------------------------------
//  READ THIS BEFORE TRUSTING A `verify_epo_inv_sketch_*` RESULT — this is the ONE
//  place in this crate that is a TRANSCRIPTION rather than the production source, so
//  every property proved against it is recorded with `fidelity: arithmetic-core`, not
//  `real-type-bounded`.
// ---------------------------------------------------------------------------------
//
// Why it cannot be the real thing. `CountMinSketch` is declared private inside
// `components/eviction-policy-optimized/src/lib.rs`, and `src/lib.rs` cannot be pulled
// in with `include!` the way `src/lru_list.rs` is: it opens with
// `use component_framework::define_component;` and its `define_component!` expansion
// names `::component_core::...` throughout, and this crate cannot depend on
// `component-framework` or `component-core` directly — a path dependency on any
// `lib/component-framework/crates/*` crate fails to load, because cargo's walk-up for
// workspace inheritance hits `lib/component-framework/Cargo.toml`, a comment-only stub
// with neither `[package]` nor `[workspace]` (see this crate's Cargo.toml). So the
// sketch is reachable only through `IEvictionPolicy`, which never exposes a counter.
//
// What is transcribed, and from where. The four items below are copied verbatim from
// `components/eviction-policy-optimized/src/lib.rs` lines 18-61:
//   * `CMS_ROWS = 4`, `CMS_COLS = 1024`, the four `CMS_PRIMES`   (lib.rs:18-25)
//   * the column expression `(key.wrapping_mul(prime) >> 54) as usize`  (lib.rs:40, 48)
//   * `increment` using `saturating_add(1)`                       (lib.rs:38-43)
//   * `estimate` as the minimum over the four rows                (lib.rs:45-52)
//   * `halve` as `*counter >>= 1` over the whole table            (lib.rs:54-60)
// `proofs_sketch.rs` additionally pins the transcription DOWNWARD against the real
// component wherever the real sketch is observable through `IEvictionPolicy` — the
// admission decision in `track` is a function of `estimate`, so
// `verify_epo_track_post_admit_*` and `verify_epo_evict_post_order_first_time_keys`
// exercise the production sketch end to end and would diverge from these proofs if
// the transcription were wrong. A transcription plus an end-to-end cross-check is
// weaker than the real type and is labelled as such; it is not a claim about a model
// of my own invention.

/// lib.rs:18 — `const CMS_ROWS: usize = 4;`
pub const CMS_ROWS: usize = 4;

/// lib.rs:19 — `const CMS_COLS: usize = 1024;`
pub const CMS_COLS: usize = 1024;

/// lib.rs:20-25 — `const CMS_PRIMES: [u64; CMS_ROWS]`.
pub const CMS_PRIMES: [u64; CMS_ROWS] = [
    0x9E3779B97F4A7C15,
    0x517CC1B727220A95,
    0x6C62272E07BB0143,
    0xD45D0D6AB7E0F981,
];

/// lib.rs:27-29 — `struct CountMinSketch { counters: [[u8; CMS_COLS]; CMS_ROWS] }`.
pub struct CountMinSketch {
    pub counters: [[u8; CMS_COLS]; CMS_ROWS],
}

impl CountMinSketch {
    /// lib.rs:32-36.
    pub fn new() -> Self {
        Self {
            counters: [[0u8; CMS_COLS]; CMS_ROWS],
        }
    }

    /// lib.rs:38-43.
    pub fn increment(&mut self, key: u64) {
        for (row, &prime) in CMS_PRIMES.iter().enumerate() {
            let col = (key.wrapping_mul(prime) >> 54) as usize;
            self.counters[row][col] = self.counters[row][col].saturating_add(1);
        }
    }

    /// lib.rs:45-52.
    pub fn estimate(&self, key: u64) -> u8 {
        let mut min = u8::MAX;
        for (row, &prime) in CMS_PRIMES.iter().enumerate() {
            let col = (key.wrapping_mul(prime) >> 54) as usize;
            min = min.min(self.counters[row][col]);
        }
        min
    }

    /// lib.rs:54-60.
    pub fn halve(&mut self) {
        for row in &mut self.counters {
            for counter in row.iter_mut() {
                *counter >>= 1;
            }
        }
    }
}

/// The column a key maps to in a given row — lib.rs:40 / lib.rs:48, extracted so it can
/// be reasoned about on its own (`EPO-INV-SKETCH-BUCKET-IN-RANGE`).
pub fn col_of(key: u64, prime: u64) -> usize {
    (key.wrapping_mul(prime) >> 54) as usize
}
