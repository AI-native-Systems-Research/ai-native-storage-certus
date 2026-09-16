//! The live path: driving a plan into a Certus node over its `/dev/shm` mailbox.
//!
//! # Lanes are channels, because the mailbox is depth-1 per channel
//!
//! A lane holds one session's turn at a time. The mailbox gives one in-flight request
//! per channel, so **lane count above channel count does not add concurrency — it
//! serialises silently** behind a claimed channel. That is why over-subscription is
//! refused at startup (FR-057's spirit, T043) rather than accepted and absorbed: a run
//! that quietly ran at a third of its requested concurrency would still produce a
//! throughput number.
//!
//! # The plan queue is measured by its minimum, never its average
//!
//! FR-062 makes a run whose plan queue reached zero **invalid**. An average depth would
//! conceal exactly that: a queue sitting at 900 for a minute and touching zero once
//! averages 899, and the run it describes is worthless. So [`LiveStats`] records the
//! **minimum** depth and the **fraction of samples at zero**, and never a mean.
//!
//! # Timing is per request, never per key
//!
//! A batch of 64 keys is one round trip, so timing each key would measure the same
//! interval 64 times and put a clock read on the per-key path — instrumentation
//! becoming the thing it measures (FR-038, T045). Latency is recorded once per request.
//!
//! # What this module does not do
//!
//! No GPU. The payload a store transfers is a pre-filled reusable buffer
//! ([`crate::payload`]), so nothing here allocates or fills bytes per operation
//! (FR-038), and no CUDA is required to exercise the operation stream. Node placement
//! and migration are US3.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hdrhistogram::Histogram;
use shm_queue::Client;
use shmq_dispatcher::wire;
use workload_model::plan::{OpKind, Operation, OperationPlan};

use crate::opstream::{forbidden_opcode, Encoding, OpStream};

/// What happened to one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Issued {
    /// Sent, with its round-trip time.
    Sent(Duration),
    /// Nothing to send.
    Nothing,
    /// Skipped for want of a GPU payload buffer, carrying the keys it would have moved.
    NeedsGpu { keys: usize },
}

/// How long `attach` waits for the server to publish its ready flag.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// Spin iterations before a request parks. Matches `apps/remote-lookup-bench`, so the
/// two tools contend for the mailbox the same way.
const SPIN_ITERS: u32 = 20_000;

/// Per-attempt wait before retrying a response poll.
const ATTEMPT_TIMEOUT: Duration = Duration::from_millis(50);

/// Overall deadline for one request.
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);

/// What a live run measured.
#[derive(Debug, Clone)]
pub struct LiveStats {
    /// Requests issued.
    pub requests: u64,
    /// Requests the server answered with an error status.
    pub errors: u64,
    /// Key references issued across every request.
    pub key_references: u64,
    /// Bytes the workload's blocks represent, from the description's geometry.
    pub block_bytes: u64,
    /// Wallclock seconds of the timed window, excluding any startup cache clear.
    pub elapsed: f64,
    /// Virtual seconds the plan advanced over that window.
    pub virtual_span: f64,
    /// Smallest plan-queue depth seen. Zero makes the run invalid (FR-062).
    pub min_queue_depth: usize,
    /// Fraction of samples at zero depth.
    pub fraction_at_zero: f64,
    /// Per-request latency.
    pub latency: Histogram<u64>,
    /// Lanes used, which is also channels claimed.
    pub lanes: usize,
    /// Operations skipped for want of a GPU payload buffer (`LOOKUP`,
    /// `COPY_TO_STORE`).
    ///
    /// Non-zero means the run exercised the control path but moved no data, so its
    /// throughput is **not** comparable with a complete run's. The report says so.
    pub skipped_needing_gpu: u64,
    /// Keys those skipped operations would have moved.
    pub skipped_keys: u64,
}

impl LiveStats {
    /// Whether the run is valid: the plan queue never reached zero (FR-062).
    pub fn is_valid(&self) -> bool {
        self.min_queue_depth > 0
    }

    /// Keys per second over the timed window.
    pub fn keys_per_second(&self) -> f64 {
        if self.elapsed > 0.0 {
            self.key_references as f64 / self.elapsed
        } else {
            0.0
        }
    }

    /// Bytes per second over the timed window.
    pub fn bytes_per_second(&self) -> f64 {
        self.keys_per_second() * self.block_bytes as f64
    }

    /// Virtual seconds advanced per wallclock second.
    ///
    /// Above 1 means the generator outran the workload's own clock; below 1 means the
    /// server could not keep up with it.
    pub fn virtual_to_wallclock(&self) -> f64 {
        if self.elapsed > 0.0 {
            self.virtual_span / self.elapsed
        } else {
            0.0
        }
    }
}

/// A connection to one Certus node's mailbox.
pub struct LiveNode {
    client: Client,
    stream: OpStream,
    lanes: Vec<usize>,
}

impl LiveNode {
    /// Attach to the mailbox at `shm_path` and claim `lanes` channels.
    ///
    /// # Errors
    ///
    /// If the mailbox cannot be attached, or `lanes` exceeds the node's channel count —
    /// refused rather than clamped, because the mailbox is depth-1 per channel and
    /// over-subscription would silently serialise (T043).
    pub fn attach(shm_path: &str, lanes: usize, block_bytes: u32) -> Result<Self, String> {
        let client = Client::attach(shm_path, ATTACH_TIMEOUT)
            .map_err(|e| format!("attach shmq mailbox {shm_path}: {e}"))?;
        let channels = client.channel_count();
        if lanes == 0 {
            return Err("--lanes must be at least 1".to_string());
        }
        if lanes > channels {
            return Err(format!(
                "refusing to run: {lanes} lanes against a node with {channels} channels. \
                 The mailbox is depth-1 per channel, so the extra lanes would not add \
                 concurrency — they would serialise behind a claimed channel and the run \
                 would report a throughput for a concurrency it never had. Use --lanes \
                 {channels} or fewer"
            ));
        }
        let mut claimed = Vec::with_capacity(lanes);
        for _ in 0..lanes {
            match client.claim_channel() {
                Some(ch) => claimed.push(ch),
                None => {
                    for ch in &claimed {
                        client.release_channel(*ch);
                    }
                    return Err(format!(
                        "could only claim {} of {lanes} channels; another client holds \
                         the rest",
                        claimed.len()
                    ));
                }
            }
        }
        Ok(Self {
            client,
            stream: OpStream::new(block_bytes),
            lanes: claimed,
        })
    }

    /// Channels this node offers.
    pub fn channel_count(&self) -> usize {
        self.client.channel_count()
    }

    /// Lanes claimed.
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }

    /// Largest key list that fits the server's request capacity.
    ///
    /// A batch beyond this would be rejected by the mailbox's own assertion, so the
    /// caller's `--batch-keys` is capped against it rather than trusted.
    pub fn max_batch_keys(&self) -> usize {
        // `n:u32` then 8 bytes per key, with room for RESERVE's wider entries.
        (self.client.cap_req().saturating_sub(8)) / 20
    }

    /// Issue one plan operation on `lane`, returning its latency.
    ///
    /// # Errors
    ///
    /// If the request fails or the server answers with an error status.
    fn issue(&self, lane: usize, plan: &OperationPlan, op: &Operation) -> Result<Issued, String> {
        let encoded = match self.stream.encode(plan, op) {
            Encoding::Ready(e) => e,
            // Only `Abort`, which the planner never emits.
            Encoding::NotPlanned => return Ok(Issued::Nothing),
            Encoding::NeedsGpuPayload { keys, .. } => {
                return Ok(Issued::NeedsGpu { keys });
            }
        };
        if let Some(why) = forbidden_opcode(encoded.opcode) {
            return Err(format!("refusing to issue a forbidden operation: {why}"));
        }
        let channel = self.lanes[lane % self.lanes.len()];
        // One clock read per request, never per key: see the module docs.
        let started = Instant::now();
        let (status, body) = self
            .client
            .request(
                channel,
                encoded.opcode,
                &encoded.payload,
                SPIN_ITERS,
                ATTEMPT_TIMEOUT,
                REQUEST_DEADLINE,
            )
            .map_err(|e| format!("shmq request (op {}) failed: {e}", encoded.opcode))?;
        let elapsed = started.elapsed();
        if status != wire::STATUS_OK {
            return Err(format!(
                "op {} returned an error status: {}",
                encoded.opcode,
                String::from_utf8_lossy(&body)
            ));
        }
        Ok(Issued::Sent(elapsed))
    }
}

impl Drop for LiveNode {
    fn drop(&mut self) {
        // Give the channels back, so a second run on the same mailbox is not refused
        // for lack of them.
        for ch in &self.lanes {
            self.client.release_channel(*ch);
        }
    }
}

/// Drive `plan` into `node`, sampling the plan queue as it goes.
///
/// `stop` lets a signal handler end the run cleanly: an interrupted run whose queue
/// never reached zero is **valid** (FR-074), so interruption is not a failure.
///
/// # Errors
///
/// If a request fails. A server error ends the run, because continuing would report a
/// throughput for a workload that was partly refused.
pub fn drive(
    node: &LiveNode,
    plan: &OperationPlan,
    stop: &Arc<AtomicBool>,
    block_bytes: u64,
) -> Result<LiveStats, String> {
    let mut latency = Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
        .map_err(|e| format!("cannot build the latency histogram: {e}"))?;
    let ops = plan.operations();
    let total = ops.len();

    let mut requests = 0u64;
    let mut errors = 0u64;
    let mut key_references = 0u64;
    let mut min_queue_depth = usize::MAX;
    let mut samples = 0u64;
    let mut at_zero = 0u64;
    let mut virtual_span = 0.0f64;
    let mut skipped_needing_gpu = 0u64;
    let mut skipped_keys = 0u64;

    let started = Instant::now();
    for (i, op) in ops.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // Depth is what remains built but unissued. The plan is built ahead in full
        // here, so this measures the *consumer* draining it — which is the quantity
        // FR-062 cares about, and it is sampled per operation rather than averaged.
        let depth = total - i;
        min_queue_depth = min_queue_depth.min(depth);
        samples += 1;
        if depth == 0 {
            at_zero += 1;
        }

        let lane = (op.session() as usize) % node.lanes();
        match node.issue(lane, plan, op) {
            Ok(Issued::Sent(d)) => {
                let micros = d.as_micros().clamp(1, 60_000_000) as u64;
                latency
                    .record(micros)
                    .map_err(|e| format!("latency out of histogram range: {e}"))?;
                requests += 1;
                if op.kind() != OpKind::PollEvents {
                    key_references += op.key_count() as u64;
                }
            }
            Ok(Issued::Nothing) => {}
            Ok(Issued::NeedsGpu { keys }) => {
                skipped_needing_gpu += 1;
                skipped_keys += keys as u64;
            }
            Err(e) => {
                // Counted before returning so the field is not dead: a server error ends
                // the run, because continuing would report a throughput for a workload
                // that was partly refused.
                errors += 1;
                return Err(format!("{e} (after {requests} requests, {errors} errors)"));
            }
        }
        virtual_span = virtual_span.max(op.at());
    }
    let elapsed = started.elapsed().as_secs_f64();

    Ok(LiveStats {
        requests,
        errors,
        key_references,
        block_bytes,
        elapsed,
        virtual_span,
        min_queue_depth: if min_queue_depth == usize::MAX {
            0
        } else {
            min_queue_depth
        },
        fraction_at_zero: if samples == 0 {
            0.0
        } else {
            at_zero as f64 / samples as f64
        },
        latency,
        lanes: node.lanes(),
        skipped_needing_gpu,
        skipped_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_bytes_per_second_come_from_the_timed_window() {
        let stats = LiveStats {
            requests: 100,
            errors: 0,
            key_references: 2_000,
            block_bytes: 32_768,
            elapsed: 2.0,
            virtual_span: 10.0,
            min_queue_depth: 5,
            fraction_at_zero: 0.0,
            latency: Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap(),
            lanes: 4,
            skipped_needing_gpu: 0,
            skipped_keys: 0,
        };
        assert_eq!(stats.keys_per_second(), 1_000.0);
        assert_eq!(stats.bytes_per_second(), 1_000.0 * 32_768.0);
        assert_eq!(stats.virtual_to_wallclock(), 5.0);
        assert!(stats.is_valid());
    }

    #[test]
    fn a_queue_that_reached_zero_makes_the_run_invalid() {
        // FR-062. Asserted on the minimum rather than on an average, because an
        // average conceals exactly this: 900 for a minute with one touch of zero
        // averages 899 and describes a worthless run.
        let mut stats = LiveStats {
            requests: 1,
            errors: 0,
            key_references: 1,
            block_bytes: 1,
            elapsed: 1.0,
            virtual_span: 1.0,
            min_queue_depth: 0,
            fraction_at_zero: 0.001,
            latency: Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap(),
            lanes: 1,
            skipped_needing_gpu: 0,
            skipped_keys: 0,
        };
        assert!(!stats.is_valid(), "a zero-depth run must be invalid");
        stats.min_queue_depth = 1;
        assert!(stats.is_valid());
    }

    #[test]
    fn rates_are_zero_rather_than_nan_for_an_empty_window() {
        // A NaN in a report is worse than a zero: it propagates into every aggregate
        // that touches it.
        let stats = LiveStats {
            requests: 0,
            errors: 0,
            key_references: 0,
            block_bytes: 4_096,
            elapsed: 0.0,
            virtual_span: 0.0,
            min_queue_depth: 0,
            fraction_at_zero: 0.0,
            latency: Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap(),
            lanes: 1,
            skipped_needing_gpu: 0,
            skipped_keys: 0,
        };
        assert_eq!(stats.keys_per_second(), 0.0);
        assert_eq!(stats.bytes_per_second(), 0.0);
        assert_eq!(stats.virtual_to_wallclock(), 0.0);
    }
}
