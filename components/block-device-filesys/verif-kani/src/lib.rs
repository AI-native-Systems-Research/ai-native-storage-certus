//! Faithful standalone Kani mirror of the block-device-filesys pure logic.
//!
//! The real `block-device-filesys` crate depends on `io-uring`, `libc`,
//! `crossbeam-channel`, and `interfaces` (feature `spdk` -> spdk-sys, needs a
//! built SPDK). None of those build under Kani, so this crate copies — VERBATIM
//! in behaviour — the pure arithmetic/logic cores whose properties the
//! component-verify inventory marks Kani-appropriate:
//!   * device-config geometry validation + accessors  (config.rs)
//!   * LBA validation + byte-offset arithmetic         (actor.rs validate_lba / offset_for_lba)
//!   * the IBlockDevice constant/geometry accessors     (lib.rs)
//!   * telemetry snapshot arithmetic (mean/min/max/record) (telemetry.rs)
//!   * small pure command-path guards: op-handle counter, harvest fsync-skip
//!     bit test, unsupported-admin arm, flush-sync namespace guard  (actor.rs)
//! and proves them with `#[kani::proof]` harnesses (see the `proofs` module).
//!
//! Mirror-fidelity notes (behaviour identical to the product; only Kani-usage
//! artifacts differ):
//!   * Error payloads (the `String` messages) are dropped: the pure logic under
//!     test branches on numeric/enum conditions only, never on message content,
//!     so a unit-variant error enum is behaviourally identical for these proofs.
//!   * `DeviceConfig` keeps only the numeric fields the arithmetic reads
//!     (`block_size`, `num_blocks`, `total_bytes`); the `PathBuf` file_path is a
//!     pass-through the arithmetic never inspects.
//!   * The `IBlockDevice` accessors read `AtomicU32/AtomicU64` (Relaxed) in the
//!     product; a single-threaded read of an atomic returns the stored value, so
//!     the mirror stores plain fields. The concurrency of those atomics is a
//!     DELEGATED (Loom) concern, not part of these value postconditions.
//!   * `TelemetryStats` is modelled single-threaded: the product's `fetch_add`
//!     and CAS-loops collapse to `wrapping_add` / `if new < min` in one thread.
//!     The MULTI-thread linearizability of those atomics is delegated to Loom;
//!     the arithmetic postconditions (mean = sum/count, min/max tracking, record
//!     accounting) proved here are the sequential value obligations.

#![allow(dead_code)]

// ===========================================================================
// config.rs mirror — DeviceConfig geometry validation + accessors
// ===========================================================================

/// Mirror of the config-construction error (message payload dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    BlockSizeTooSmall,
    BlockSizeNotPow2,
    ZeroBlocks,
    TotalBytesOverflow,
}

/// Mirror of `config::DeviceConfig` (numeric fields only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceConfig {
    block_size: u32,
    num_blocks: u64,
    total_bytes: u64,
}

impl DeviceConfig {
    /// Verbatim mirror of `DeviceConfig::new` (config.rs:58-79), message bodies
    /// replaced by unit error variants.
    pub fn new(block_size: u32, num_blocks: u64) -> Result<Self, ConfigError> {
        if block_size < 512 {
            return Err(ConfigError::BlockSizeTooSmall);
        }
        if !block_size.is_power_of_two() {
            return Err(ConfigError::BlockSizeNotPow2);
        }
        if num_blocks == 0 {
            return Err(ConfigError::ZeroBlocks);
        }

        let total_bytes = (block_size as u64)
            .checked_mul(num_blocks)
            .ok_or(ConfigError::TotalBytesOverflow)?;

        Ok(Self {
            block_size,
            num_blocks,
            total_bytes,
        })
    }

    #[inline]
    pub fn block_size(&self) -> u32 {
        self.block_size
    }
    #[inline]
    pub fn num_blocks(&self) -> u64 {
        self.num_blocks
    }
    #[inline]
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }
}

// ===========================================================================
// actor.rs mirror — validate_lba / offset_for_lba
// ===========================================================================

/// Mirror of the LBA-validation error (message payload dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LbaError {
    InvalidNamespace,
    Overflow,
    OutOfRange,
}

/// Verbatim mirror of `FilesysHandler::validate_lba` (actor.rs:179-195).
/// `cfg_num_blocks` is `self.config.num_blocks()`.
pub fn validate_lba(
    cfg_num_blocks: u64,
    ns_id: u32,
    lba: u64,
    num_blocks: u64,
) -> Result<(), LbaError> {
    if ns_id != 1 {
        return Err(LbaError::InvalidNamespace);
    }
    let end = lba.checked_add(num_blocks).ok_or(LbaError::Overflow)?;
    if end > cfg_num_blocks {
        return Err(LbaError::OutOfRange);
    }
    Ok(())
}

/// Verbatim mirror of `FilesysHandler::offset_for_lba` (actor.rs:197-199).
/// `block_size` is `self.config.block_size()`.
pub fn offset_for_lba(lba: u64, block_size: u32) -> u64 {
    lba * block_size as u64
}

// ===========================================================================
// lib.rs mirror — IBlockDevice accessors + constants
// ===========================================================================

pub const CLIENT_CHANNEL_CAPACITY: usize = 64;
pub const DEFAULT_RING_DEPTH: u32 = 128;

/// Namespace-query error (only the discriminant matters here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NsError {
    InvalidNamespace,
}

/// Mirror of the geometry-bearing / constant `IBlockDevice` accessors. Stored
/// fields stand in for the product's `AtomicU32/AtomicU64` (single-thread read).
pub struct Accessors {
    pub block_size: u32,
    pub num_blocks: u64,
}

impl Accessors {
    /// Mirror of `IBlockDevice::sector_size` (lib.rs:266-273).
    pub fn sector_size(&self, ns_id: u32) -> Result<u32, NsError> {
        if ns_id != 1 {
            return Err(NsError::InvalidNamespace);
        }
        Ok(self.block_size)
    }

    /// Mirror of `IBlockDevice::num_sectors` (lib.rs:275-282).
    pub fn num_sectors(&self, ns_id: u32) -> Result<u64, NsError> {
        if ns_id != 1 {
            return Err(NsError::InvalidNamespace);
        }
        Ok(self.num_blocks)
    }

    /// Mirror of `IBlockDevice::max_queue_depth` (lib.rs:284-286).
    pub fn max_queue_depth(&self) -> u32 {
        DEFAULT_RING_DEPTH
    }

    /// Mirror of `IBlockDevice::num_io_queues` (lib.rs:288-290).
    pub fn num_io_queues(&self) -> u32 {
        1
    }

    /// Mirror of `IBlockDevice::max_transfer_size` (lib.rs:292-294).
    pub fn max_transfer_size(&self) -> u32 {
        self.block_size.saturating_mul(256)
    }

    /// Mirror of `IBlockDevice::block_size` (lib.rs:296-298).
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Mirror of `IBlockDevice::numa_node` (lib.rs:300-302).
    pub fn numa_node(&self) -> i32 {
        -1
    }

    /// Mirror of `IBlockDevice::nvme_version` (lib.rs:304-306).
    pub fn nvme_version(&self) -> &'static str {
        "N/A (file-backed)"
    }
}

// ===========================================================================
// telemetry.rs mirror — single-threaded sequential model of TelemetryStats
// ===========================================================================

/// Sequential mirror of `telemetry::TelemetryStats`. The product uses atomics
/// (fetch_add + compare_exchange loops); in a single thread those collapse to
/// the plain updates below, preserving the snapshot arithmetic exactly.
pub struct TelemetryStats {
    total_ops: u64,
    min_latency_ns: u64,
    max_latency_ns: u64,
    total_latency_ns: u64,
    total_bytes: u64,
}

/// Mirror of `interfaces::TelemetrySnapshot` (fields under test).
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetrySnapshot {
    pub total_ops: u64,
    pub min_latency_ns: u64,
    pub max_latency_ns: u64,
    pub mean_latency_ns: u64,
    pub mean_throughput_mbps: f64,
}

impl TelemetryStats {
    /// Mirror of `TelemetryStats::new` (telemetry.rs:24-33).
    pub fn new() -> Self {
        Self {
            total_ops: 0,
            min_latency_ns: u64::MAX,
            max_latency_ns: 0,
            total_latency_ns: 0,
            total_bytes: 0,
        }
    }

    /// Sequential mirror of `TelemetryStats::record_op` (telemetry.rs:35-66).
    /// `fetch_add` wraps on overflow (Relaxed atomic add); modelled with
    /// `wrapping_add`. The min/max CAS loops resolve, in one thread, to a plain
    /// compare-and-store.
    pub fn record_op(&mut self, latency_ns: u64, bytes: u64) {
        self.total_ops = self.total_ops.wrapping_add(1);
        self.total_latency_ns = self.total_latency_ns.wrapping_add(latency_ns);
        self.total_bytes = self.total_bytes.wrapping_add(bytes);

        if latency_ns < self.min_latency_ns {
            self.min_latency_ns = latency_ns;
        }
        if latency_ns > self.max_latency_ns {
            self.max_latency_ns = latency_ns;
        }
    }

    /// Mirror of `TelemetryStats::snapshot` (telemetry.rs:68-102), with
    /// `elapsed_secs` supplied explicitly (the product reads a wall clock; the
    /// arithmetic under test is a pure function of it).
    pub fn snapshot(&self, elapsed_secs: f64) -> TelemetrySnapshot {
        let total_ops = self.total_ops;

        let min_latency_ns = if total_ops == 0 {
            0
        } else {
            self.min_latency_ns
        };
        let max_latency_ns = self.max_latency_ns;
        let total_latency_ns = self.total_latency_ns;
        let total_bytes = self.total_bytes;

        let mean_latency_ns = if total_ops > 0 {
            total_latency_ns / total_ops
        } else {
            0
        };

        let mean_throughput_mbps = if elapsed_secs > 0.0 {
            (total_bytes as f64) / (1024.0 * 1024.0) / elapsed_secs
        } else {
            0.0
        };

        TelemetrySnapshot {
            total_ops,
            min_latency_ns,
            max_latency_ns,
            mean_latency_ns,
            mean_throughput_mbps,
        }
    }
}

// ===========================================================================
// actor.rs mirror — small pure command-path guards
// ===========================================================================

/// Mirror of the op-handle counter (`FilesysHandler::next_op_handle`,
/// actor.rs:173-177; initial `next_handle = 1` per `FilesysHandler::new`).
pub struct HandleGen {
    next_handle: u64,
}

impl HandleGen {
    pub fn new() -> Self {
        Self { next_handle: 1 }
    }
    /// With arbitrary starting value (for the general successive-handle proof).
    pub fn with_start(start: u64) -> Self {
        Self { next_handle: start }
    }
    pub fn next_op_handle(&mut self) -> u64 {
        let h = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1);
        h
    }
}

/// High bit that tags the chained data-sync submission's user_data
/// (`handle.0 | (1 << 63)`, actor.rs:600).
pub const FSYNC_TAG_BIT: u64 = 1 << 63;

/// Mirror of the harvest fsync-skip test (actor.rs:823-826): a completion whose
/// user_data has the high bit set is the chained fsync and is skipped.
pub fn harvest_is_skipped(user_data: u64) -> bool {
    user_data & FSYNC_TAG_BIT != 0
}

/// The four admin commands the file-backed device does not support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminCmd {
    NsCreate,
    NsDelete,
    NsFormat,
    ControllerReset,
}

/// Mirror of the unsupported-admin arm (actor.rs:273-286): every admin command
/// maps to a NotSupported error (modelled as `true`).
pub fn admin_is_not_supported(_cmd: AdminCmd) -> bool {
    // The product's match arm returns `Completion::Error { NotSupported }` for
    // exactly these four variants; there is no other branch.
    true
}

/// Flush-sync namespace guard result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlushGuard {
    InvalidNamespace,
    Proceed,
}

/// Mirror of the FlushSync namespace guard (actor.rs:252-256): ns_id != 1 is
/// rejected with InvalidNamespace before the backing file is touched.
pub fn flush_ns_guard(ns_id: u32) -> FlushGuard {
    if ns_id != 1 {
        FlushGuard::InvalidNamespace
    } else {
        FlushGuard::Proceed
    }
}

#[cfg(kani)]
pub mod proofs;
