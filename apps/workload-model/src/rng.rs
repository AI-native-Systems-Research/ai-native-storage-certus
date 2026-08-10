//! Deterministic, seekable randomness.
//!
//! Every draw derives from the root seed plus an explicit *label* identifying
//! what is being drawn — a node's identity, a session's id, a sweep point. No
//! draw depends on how many draws came before it in wall-clock or arrival order.
//!
//! This is not a style preference. It is what makes an arbitrarily long run
//! reproducible (constitution principle II), what lets a node generate its own
//! slice of a plan without replaying the others, and what keeps `corpus`
//! orthogonal to `workload`: a trunk node's child count is a function of the
//! node, so changing the request rate cannot change which keys exist.
//!
//! ```
//! use workload_model::rng::Stream;
//! // The same label always yields the same sequence.
//! let a: Vec<u64> = Stream::new(7, 42).take_u64(3);
//! let b: Vec<u64> = Stream::new(7, 42).take_u64(3);
//! assert_eq!(a, b);
//! // A different label yields a different one.
//! assert_ne!(a, Stream::new(7, 43).take_u64(3));
//! ```

/// A reproducible stream of draws, keyed on a seed and a label.
#[derive(Debug, Clone)]
pub struct Stream {
    state: u64,
}

impl Stream {
    /// A stream for `label` under `seed`.
    ///
    /// The label is mixed rather than concatenated so that adjacent labels —
    /// consecutive session ids, sibling child indices — give unrelated streams.
    pub fn new(seed: u64, label: u64) -> Self {
        let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
        state = mix(state ^ label.wrapping_mul(0xBF58_476D_1CE4_E5B9));
        Stream { state: mix(state) }
    }

    /// A sub-stream for a distinct purpose under the same label.
    ///
    /// Keeps two independent draws about one entity from consuming each other's
    /// values, which would couple them for no reason.
    pub fn split(&self, purpose: u64) -> Self {
        Stream::new(self.state, purpose)
    }

    /// Next raw value.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.state)
    }

    /// Next value in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        // 53 significant bits, the most an f64 mantissa holds exactly.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Next value in `[0, n)`, unbiased by rejection.
    pub fn next_below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }

    /// The first `n` raw values; for tests and documentation.
    pub fn take_u64(mut self, n: usize) -> Vec<u64> {
        (0..n).map(|_| self.next_u64()).collect()
    }
}

/// SplitMix64 finalizer: strong avalanche, so adjacent labels decorrelate.
#[inline]
fn mix(mut z: u64) -> u64 {
    z ^= z >> 30;
    z = z.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z ^= z >> 27;
    z = z.wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_label_same_sequence() {
        assert_eq!(Stream::new(1, 2).take_u64(8), Stream::new(1, 2).take_u64(8));
    }

    #[test]
    fn adjacent_labels_decorrelate() {
        // Sibling child indices and consecutive session ids are adjacent
        // labels; if they correlated, trunk shape would show artefacts.
        let a = Stream::new(1, 1000).take_u64(4);
        let b = Stream::new(1, 1001).take_u64(4);
        assert!(a.iter().zip(&b).all(|(x, y)| x != y));
    }

    #[test]
    fn different_seeds_differ() {
        assert_ne!(Stream::new(1, 5).take_u64(4), Stream::new(2, 5).take_u64(4));
    }

    #[test]
    fn split_streams_are_independent() {
        let s = Stream::new(9, 9);
        assert_ne!(s.split(0).take_u64(4), s.split(1).take_u64(4));
    }

    #[test]
    fn uniform_is_in_range_and_unbiased_enough() {
        let mut s = Stream::new(3, 3);
        let mut counts = [0u32; 8];
        for _ in 0..80_000 {
            let v = s.next_below(8);
            assert!(v < 8);
            counts[v as usize] += 1;
        }
        // 10k expected per bucket; a broken generator fails this by miles.
        for c in counts {
            assert!((8_000..12_000).contains(&c), "skewed: {counts:?}");
        }
    }

    #[test]
    fn f64_is_in_unit_interval() {
        let mut s = Stream::new(4, 4);
        for _ in 0..10_000 {
            let v = s.next_f64();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
