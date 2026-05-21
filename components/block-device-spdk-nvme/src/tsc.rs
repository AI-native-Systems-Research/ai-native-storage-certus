//! Userspace TSC (Time Stamp Counter) clock for low-overhead timing.
//!
//! On x86_64 Linux with an invariant TSC, `rdtsc` is ~20 cycles versus
//! ~50-80 cycles for `clock_gettime` via the vDSO. This module provides
//! a [`TscClock`] that calibrates the TSC frequency once at construction
//! and then offers fast tick reads and conversions.

use std::time::Instant;

/// Read the TSC via `rdtscp` (serializing variant).
///
/// Returns the 64-bit timestamp counter value. On x86_64 with invariant
/// TSC this is monotonic and synchronized across cores.
#[inline(always)]
pub(crate) fn rdtsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::x86_64::_rdtsc()
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        // Fallback: use Instant for non-x86_64 (keeps the code compilable).
        Instant::now().duration_since(Instant::now()).as_nanos() as u64
    }
}

/// Low-overhead clock based on the hardware TSC.
///
/// Calibrated once at construction by measuring TSC ticks against
/// `clock_gettime` over a short spin interval.
#[derive(Clone)]
pub(crate) struct TscClock {
    /// Nanoseconds per TSC tick, scaled by 2^32 for fixed-point arithmetic.
    #[allow(dead_code)]
    ns_per_tick_fp32: u64,
    /// TSC ticks per millisecond (pre-computed for deadline conversion).
    ticks_per_ms: u64,
}

impl TscClock {
    /// Create a new clock, calibrating TSC frequency against `clock_gettime`.
    pub fn new() -> Self {
        let (ns_per_tick_fp32, ticks_per_ms) = Self::calibrate();
        Self {
            ns_per_tick_fp32,
            ticks_per_ms,
        }
    }

    /// Read the current TSC value.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        rdtsc()
    }

    /// Convert a TSC tick delta to nanoseconds.
    #[inline(always)]
    #[allow(dead_code)]
    pub fn ticks_to_ns(&self, ticks: u64) -> u64 {
        // Fixed-point multiply: (ticks * ns_per_tick_fp32) >> 32
        ((ticks as u128 * self.ns_per_tick_fp32 as u128) >> 32) as u64
    }

    /// Compute a deadline TSC value `ms` milliseconds from `start`.
    #[inline(always)]
    pub fn deadline_from_ms(&self, start: u64, ms: u64) -> u64 {
        start + ms * self.ticks_per_ms
    }

    /// Check if the current TSC has passed `deadline`.
    #[inline(always)]
    #[allow(dead_code)]
    pub fn has_elapsed(&self, deadline: u64) -> bool {
        rdtsc() >= deadline
    }

    /// Calibrate TSC frequency by spinning for ~2ms against `clock_gettime`.
    fn calibrate() -> (u64, u64) {
        let spin_ns: u64 = 2_000_000; // 2ms calibration window

        let t0 = Instant::now();
        let tsc0 = rdtsc();

        // Spin until 2ms of wall-clock time has elapsed.
        loop {
            if t0.elapsed().as_nanos() as u64 >= spin_ns {
                break;
            }
            std::hint::spin_loop();
        }

        let tsc1 = rdtsc();
        let elapsed_ns = t0.elapsed().as_nanos() as u64;
        let elapsed_ticks = tsc1 - tsc0;

        // ns_per_tick as fixed-point Q32: (elapsed_ns << 32) / elapsed_ticks
        let ns_per_tick_fp32 = ((elapsed_ns as u128) << 32) / elapsed_ticks as u128;
        let ticks_per_ms = elapsed_ticks * 1_000_000 / elapsed_ns;

        (ns_per_tick_fp32 as u64, ticks_per_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdtsc_monotonic() {
        let a = rdtsc();
        let b = rdtsc();
        assert!(b >= a);
    }

    #[test]
    fn calibration_reasonable() {
        let clock = TscClock::new();
        // TSC frequency should be between 500 MHz and 10 GHz.
        let ticks_per_sec = clock.ticks_per_ms * 1000;
        assert!(
            ticks_per_sec >= 500_000_000,
            "TSC too slow: {ticks_per_sec} ticks/sec"
        );
        assert!(
            ticks_per_sec <= 10_000_000_000,
            "TSC too fast: {ticks_per_sec} ticks/sec"
        );
    }

    #[test]
    fn ticks_to_ns_round_trip() {
        let clock = TscClock::new();
        let t0 = clock.now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let t1 = clock.now();
        let ns = clock.ticks_to_ns(t1 - t0);
        // Should be roughly 10ms (±5ms tolerance for scheduling jitter).
        assert!(ns >= 5_000_000, "too short: {ns} ns");
        assert!(ns <= 50_000_000, "too long: {ns} ns");
    }

    #[test]
    fn deadline_from_ms_works() {
        let clock = TscClock::new();
        let start = clock.now();
        let deadline = clock.deadline_from_ms(start, 1);
        // Deadline should be in the future.
        assert!(deadline > start);
        // And roughly 1ms of ticks away.
        let delta_ns = clock.ticks_to_ns(deadline - start);
        assert!(delta_ns >= 800_000, "deadline too close: {delta_ns} ns");
        assert!(delta_ns <= 1_500_000, "deadline too far: {delta_ns} ns");
    }
}
