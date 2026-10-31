//! Counters, in two tiers.
//!
//! **Always counted** — in every build — is everything [`TransferStats`] names,
//! because `ITransferEndpoint::stats` returns it in every build. Gating those
//! on a feature would make `stats()` report zeros by default: a plausible
//! number from an unexercised path, which Principle XI forbids. They are plain
//! integers owned by the endpoint thread, a few adds per request against
//! writes costing tens of microseconds.
//!
//! **Feature-gated** (`telemetry`) are the extras no caller sees through the
//! interface ([`Extras`]): a zero-sized no-op without the feature, with an
//! identical method surface so call sites never change, and a snapshot that is
//! `None` rather than zeros when off.
//!
//! Every `TransferStats` field is filled. The live gauges are read at each
//! snapshot rather than counted: the send-queue ones from the links' credits,
//! the request ones from the sessions' message credits.

use interfaces::TransferStats;

/// The always-counted tier: every [`TransferStats`] field that is a count or a
/// gauge kept here. The live gauges are not: the send-queue ones are read from
/// the links' credits when a snapshot is taken (see `ConnectionTable::stats`),
/// and the request gauges from the sessions' message credits, so neither can
/// drift from what the credits enforce.
/// Each field means what the [`TransferStats`] field of the same name says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    pub entries_requested: u64,
    pub hit_memory_tier: u64,
    pub hit_block_tier: u64,
    pub miss: u64,
    pub failed: u64,
    pub requests_attempted: u64,
    pub requests_completed: u64,
    pub chunks_sent: u64,
    /// A gauge, kept here because it changes only on the endpoint thread.
    pub bounce_buffers_in_use: u32,
    pub bounce_buffers_peak: u32,
    pub block_reads_attempted: u64,
    pub block_reads_completed: u64,
    pub block_reads_failed: u64,
    pub block_deferrals: u64,
    /// Reply deadlines that expired and tore a connection down (T050, T070).
    pub deadline_expiries: u64,
    pub protocol_faults: u64,
    pub version_downgrades: u64,
    /// Counted by the connection table: once per simultaneous-connect race, at
    /// the end whose own attempt was discarded.
    pub duplicate_connections_resolved: u64,
    /// The highest send-queue in-flight any link has reached, including links
    /// since retired: a link's own peak dies with it, so it is folded in here
    /// when the link leaves service. Per link, so it is comparable with the
    /// per-link credit limit.
    pub send_queue_peak_in_flight: u32,
}

impl Counters {
    /// Raise the retained send-queue peak to `peak` if it is higher.
    pub fn fold_peak(&mut self, peak: u32) {
        self.send_queue_peak_in_flight = self.send_queue_peak_in_flight.max(peak);
    }

    /// A snapshot, given the send-queue gauges read from the live links.
    pub fn snapshot(&self, in_flight: u32, live_peak: u32) -> TransferStats {
        TransferStats {
            entries_requested: self.entries_requested,
            hit_memory_tier: self.hit_memory_tier,
            hit_block_tier: self.hit_block_tier,
            miss: self.miss,
            failed: self.failed,
            requests_attempted: self.requests_attempted,
            requests_completed: self.requests_completed,
            chunks_sent: self.chunks_sent,
            send_queue_in_flight: in_flight,
            send_queue_peak_in_flight: self.send_queue_peak_in_flight.max(live_peak),
            // The message-credit gauges are the endpoint's, read from its
            // sessions (`Command::Stats`).
            requests_outstanding_outbound: 0,
            requests_in_service_inbound: 0,
            bounce_buffers_in_use: self.bounce_buffers_in_use,
            bounce_buffers_peak: self.bounce_buffers_peak,
            block_reads_attempted: self.block_reads_attempted,
            block_reads_completed: self.block_reads_completed,
            block_reads_failed: self.block_reads_failed,
            block_deferrals: self.block_deferrals,
            deadline_expiries: self.deadline_expiries,
            protocol_faults: self.protocol_faults,
            version_downgrades: self.version_downgrades,
            duplicate_connections_resolved: self.duplicate_connections_resolved,
        }
    }
}

/// What [`Extras::snapshot`] reports with the `telemetry` feature on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtraStats {
    /// Outbound connects started.
    pub connects_attempted: u64,
    /// Outbound connects that came up.
    pub connects_established: u64,
    /// Outbound connects that failed; a lost tie-break is not a failure.
    pub connects_failed: u64,
}

/// The feature-gated tier.
#[cfg(feature = "telemetry")]
#[derive(Debug, Default)]
pub struct Extras {
    stats: ExtraStats,
}

#[cfg(feature = "telemetry")]
impl Extras {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn connect_attempted(&mut self) {
        self.stats.connects_attempted += 1;
    }

    pub fn connect_established(&mut self) {
        self.stats.connects_established += 1;
    }

    pub fn connect_failed(&mut self) {
        self.stats.connects_failed += 1;
    }

    pub fn snapshot(&self) -> Option<ExtraStats> {
        Some(self.stats)
    }
}

/// The feature-gated tier, compiled out.
#[cfg(not(feature = "telemetry"))]
#[derive(Debug, Default)]
pub struct Extras;

#[cfg(not(feature = "telemetry"))]
#[allow(clippy::unused_self)]
impl Extras {
    pub fn new() -> Self {
        Extras
    }

    #[inline]
    pub fn connect_attempted(&mut self) {}

    #[inline]
    pub fn connect_established(&mut self) {}

    #[inline]
    pub fn connect_failed(&mut self) {}

    /// `None`: nothing was counted, which is not the same as zero.
    pub fn snapshot(&self) -> Option<ExtraStats> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_maps_every_field_and_takes_the_higher_peak() {
        let c = Counters {
            entries_requested: 1,
            hit_memory_tier: 2,
            hit_block_tier: 3,
            miss: 4,
            failed: 40,
            requests_attempted: 5,
            requests_completed: 6,
            chunks_sent: 7,
            bounce_buffers_in_use: 10,
            bounce_buffers_peak: 11,
            block_reads_attempted: 18,
            block_reads_completed: 19,
            block_reads_failed: 20,
            block_deferrals: 21,
            deadline_expiries: 12,
            protocol_faults: 13,
            version_downgrades: 14,
            duplicate_connections_resolved: 15,
            send_queue_peak_in_flight: 16,
        };
        let s = c.snapshot(17, 3);
        assert_eq!(
            s,
            TransferStats {
                entries_requested: 1,
                hit_memory_tier: 2,
                hit_block_tier: 3,
                miss: 4,
                failed: 40,
                requests_attempted: 5,
                requests_completed: 6,
                chunks_sent: 7,
                send_queue_in_flight: 17,
                send_queue_peak_in_flight: 16,
                requests_outstanding_outbound: 0,
                requests_in_service_inbound: 0,
                bounce_buffers_in_use: 10,
                bounce_buffers_peak: 11,
                block_reads_attempted: 18,
                block_reads_completed: 19,
                block_reads_failed: 20,
                block_deferrals: 21,
                deadline_expiries: 12,
                protocol_faults: 13,
                version_downgrades: 14,
                duplicate_connections_resolved: 15,
            }
        );
        assert_eq!(c.snapshot(0, 40).send_queue_peak_in_flight, 40);
    }

    #[test]
    fn fold_peak_never_lowers() {
        let mut c = Counters::default();
        c.fold_peak(5);
        c.fold_peak(2);
        assert_eq!(c.send_queue_peak_in_flight, 5);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn extras_count_with_the_feature() {
        let mut e = Extras::new();
        e.connect_attempted();
        e.connect_attempted();
        e.connect_established();
        e.connect_failed();
        assert_eq!(
            e.snapshot(),
            Some(ExtraStats {
                connects_attempted: 2,
                connects_established: 1,
                connects_failed: 1,
            })
        );
    }

    #[cfg(not(feature = "telemetry"))]
    #[test]
    fn extras_report_none_without_the_feature() {
        let mut e = Extras::new();
        e.connect_attempted();
        assert_eq!(e.snapshot(), None, "not counted is not zero");
        assert_eq!(std::mem::size_of::<Extras>(), 0);
    }
}
