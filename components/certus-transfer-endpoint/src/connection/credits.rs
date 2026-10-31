//! Send-queue credits: the first of D10's three credit domains (FR-014).
//!
//! A credit is one send-queue work request in flight on one link. The slab
//! (T013) already refuses a batch once the queue pair is full, so a credit is
//! not about overflow: it is a **configured limit below the hardware depth**,
//! which is what bounds held read pins (SC-004), and it lets a batch post
//! **partly** — as far as the free credits go — instead of not at all
//! (FR-015's "chunk, don't fail").
//!
//! # What is counted
//!
//! Every send-queue request, not only RDMA writes. FR-014 names "in-flight RDMA
//! writes", but requests and replies are sends on the same queue and fill it
//! the same way; counting only writes would let a burst of messages fill the
//! queue under writes that believe they have room. The cost — a reply holds a
//! credit a write could have used — is accepted. Receives are **not** counted:
//! they are partitioned under their own rule (FR-026, T080), and charging them
//! here would merge two of D10's domains.
//!
//! # The pin bound
//!
//! Each tracked send carries its pin in its slab state, and a credit is taken
//! before that state enters the slab, so held pins ≤ `in_flight` ≤ `limit`. The
//! bound holds by construction; T025 proves it end to end.

// Read by the endpoint and telemetry from T015/T016 onward.
#![allow(dead_code)]

use std::fmt;

/// One link's send-queue credits.
#[derive(Clone, PartialEq, Eq)]
pub struct SendCredits {
    limit: u32,
    in_flight: u32,
    peak: u32,
}

impl SendCredits {
    /// Credits for a link whose send queue the device granted `granted` deep,
    /// under a configured limit of `configured`. The limit is clamped to the
    /// grant, never trusted above it.
    pub fn new(configured: u32, granted: u32) -> Self {
        SendCredits {
            limit: configured.min(granted),
            in_flight: 0,
            peak: 0,
        }
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }

    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    pub fn available(&self) -> u32 {
        self.limit - self.in_flight
    }

    /// The highest `in_flight` has been on this link — T015's "peak depth".
    pub fn peak(&self) -> u32 {
        self.peak
    }

    /// Take up to `want` credits, returning how many were granted.
    pub fn take(&mut self, want: u32) -> u32 {
        let n = want.min(self.available());
        self.in_flight += n;
        self.peak = self.peak.max(self.in_flight);
        n
    }

    /// Return `n` credits: completed requests, or taken ones never posted.
    pub fn give_back(&mut self, n: u32) {
        assert!(
            n <= self.in_flight,
            "returned {n} send credits with only {} in flight: a completion was counted twice",
            self.in_flight
        );
        self.in_flight -= n;
    }
}

impl fmt::Debug for SendCredits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SendCredits({}/{} in flight, peak {})",
            self.in_flight, self.limit, self.peak
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_is_clamped_to_the_grant() {
        assert_eq!(SendCredits::new(64, 8).limit(), 8);
        assert_eq!(SendCredits::new(4, 8).limit(), 4);
    }

    #[test]
    fn take_is_partial_and_peak_is_kept() {
        let mut c = SendCredits::new(3, 8);
        assert_eq!(c.take(2), 2);
        assert_eq!(c.take(5), 1, "only one left");
        assert_eq!(c.take(1), 0);
        c.give_back(3);
        assert_eq!((c.in_flight(), c.available(), c.peak()), (0, 3, 3));
    }

    #[test]
    #[should_panic(expected = "counted twice")]
    fn over_return_is_loud() {
        let mut c = SendCredits::new(2, 2);
        c.take(1);
        c.give_back(2);
    }
}
