//! Kani harnesses for the Kani-appropriate block-device-filesys properties.
//!
//! Property id -> harness name mapping is scorer_kani.py's rule
//! (`"verify_" + id.lower().replace("-","_")`). Every `verify_<id>` proves the
//! property; every `verify_<id>__mutant` is a deliberately-false twin that MUST
//! fail (anti-vacuity: it shows the harness setup reaches and exercises the
//! assertion, and that Kani would catch a broken property).
//!
//! Properties requiring real io_uring / libc / filesystem / thread / channel
//! behaviour (file create/size/fallocate/O_DIRECT, init/connect/shutdown
//! lifecycle, sync/async read/write/write-zeros IO, batch dispatch, abort,
//! ns-probe delivery, flush data-sync, completion delivery/FIFO/isolation,
//! harvest result mapping, timeouts, admin no-ops) are `delegate_to` other
//! referents in the inventory and are intentionally not harnessed here.
#![allow(non_snake_case)]

use crate::{
    admin_is_not_supported, flush_ns_guard, harvest_is_skipped, offset_for_lba, validate_lba,
    Accessors, AdminCmd, ConfigError, DeviceConfig, FlushGuard, HandleGen, LbaError, NsError,
    TelemetryStats, FSYNC_TAG_BIT,
};

// ===========================================================================
// BDF-CONFIG-BLKSZ-MIN — block_size < 512 is rejected.
// ===========================================================================
#[kani::proof]
fn verify_bdf_config_blksz_min() {
    let bs: u32 = kani::any();
    kani::assume(bs < 512);
    let nb: u64 = kani::any();
    assert_eq!(DeviceConfig::new(bs, nb), Err(ConfigError::BlockSizeTooSmall));
}
#[kani::proof]
fn verify_bdf_config_blksz_min__mutant() {
    let bs: u32 = kani::any();
    kani::assume(bs < 512);
    let nb: u64 = kani::any();
    // FALSE: claims a sub-512 block size is accepted.
    assert!(DeviceConfig::new(bs, nb).is_ok());
}

// ===========================================================================
// BDF-CONFIG-BLKSZ-POW2 — block_size >= 512 but not a power of two is rejected.
// ===========================================================================
#[kani::proof]
fn verify_bdf_config_blksz_pow2() {
    let bs: u32 = kani::any();
    kani::assume(bs >= 512);
    kani::assume(!bs.is_power_of_two());
    let nb: u64 = kani::any();
    assert_eq!(DeviceConfig::new(bs, nb), Err(ConfigError::BlockSizeNotPow2));
}
#[kani::proof]
fn verify_bdf_config_blksz_pow2__mutant() {
    let bs: u32 = kani::any();
    kani::assume(bs >= 512);
    kani::assume(!bs.is_power_of_two());
    // FALSE: claims a non-power-of-two block size is accepted.
    assert!(DeviceConfig::new(bs, 1).is_ok());
}

// ===========================================================================
// BDF-CONFIG-NBLK-NONZERO — zero blocks is rejected.
// ===========================================================================
#[kani::proof]
fn verify_bdf_config_nblk_nonzero() {
    let bs: u32 = kani::any();
    kani::assume(bs >= 512);
    kani::assume(bs.is_power_of_two());
    assert_eq!(DeviceConfig::new(bs, 0), Err(ConfigError::ZeroBlocks));
}
#[kani::proof]
fn verify_bdf_config_nblk_nonzero__mutant() {
    let bs: u32 = kani::any();
    kani::assume(bs >= 512);
    kani::assume(bs.is_power_of_two());
    // FALSE: claims zero blocks is accepted.
    assert!(DeviceConfig::new(bs, 0).is_ok());
}

// ===========================================================================
// BDF-CONFIG-TOTALBYTES-OVERFLOW — block_size*num_blocks overflow is rejected.
// ===========================================================================
#[kani::proof]
fn verify_bdf_config_totalbytes_overflow() {
    let bs: u32 = kani::any();
    kani::assume(bs >= 512);
    kani::assume(bs.is_power_of_two());
    let nb: u64 = kani::any();
    kani::assume(nb > 0);
    kani::assume((bs as u64).checked_mul(nb).is_none());
    assert_eq!(
        DeviceConfig::new(bs, nb),
        Err(ConfigError::TotalBytesOverflow)
    );
}
#[kani::proof]
fn verify_bdf_config_totalbytes_overflow__mutant() {
    let bs: u32 = kani::any();
    kani::assume(bs >= 512);
    kani::assume(bs.is_power_of_two());
    let nb: u64 = kani::any();
    kani::assume(nb > 0);
    kani::assume((bs as u64).checked_mul(nb).is_none());
    // FALSE: claims an overflowing geometry is accepted.
    assert!(DeviceConfig::new(bs, nb).is_ok());
}

// ===========================================================================
// BDF-CONFIG-TOTALBYTES-EQ — total_bytes == block_size * num_blocks.
// ===========================================================================
#[kani::proof]
fn verify_bdf_config_totalbytes_eq() {
    let bs: u32 = kani::any();
    let nb: u64 = kani::any();
    if let Ok(cfg) = DeviceConfig::new(bs, nb) {
        // new() only succeeds when checked_mul succeeded, so this product is exact.
        assert_eq!(cfg.total_bytes(), (bs as u64) * nb);
    }
}
#[kani::proof]
fn verify_bdf_config_totalbytes_eq__mutant() {
    let bs: u32 = kani::any();
    let nb: u64 = kani::any();
    if let Ok(cfg) = DeviceConfig::new(bs, nb) {
        // FALSE: off-by-one total capacity.
        assert_eq!(cfg.total_bytes(), (bs as u64) * nb + 1);
    } else {
        // Keep the mutant reachable/falsifiable even when new() fails: force a
        // valid config so the true branch is always exercised for some input.
        let cfg = DeviceConfig::new(512, 1).unwrap();
        assert_eq!(cfg.total_bytes(), 512 * 1 + 1);
    }
}

// ===========================================================================
// BDF-CONFIG-ACCESSORS — block_size()/num_blocks() return construction values.
// ===========================================================================
#[kani::proof]
fn verify_bdf_config_accessors() {
    let bs: u32 = kani::any();
    let nb: u64 = kani::any();
    if let Ok(cfg) = DeviceConfig::new(bs, nb) {
        assert_eq!(cfg.block_size(), bs);
        assert_eq!(cfg.num_blocks(), nb);
    }
}
#[kani::proof]
fn verify_bdf_config_accessors__mutant() {
    let cfg = DeviceConfig::new(4096, 1024).unwrap();
    // FALSE: accessor claims a different block size than constructed.
    assert_eq!(cfg.block_size(), 512);
}

// ===========================================================================
// BDF-LBA-NS — ns_id != 1 rejected with InvalidNamespace (no IO).
// ===========================================================================
#[kani::proof]
fn verify_bdf_lba_ns() {
    let cfgnb: u64 = kani::any();
    let ns: u32 = kani::any();
    kani::assume(ns != 1);
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    assert_eq!(
        validate_lba(cfgnb, ns, lba, n),
        Err(LbaError::InvalidNamespace)
    );
}
#[kani::proof]
fn verify_bdf_lba_ns__mutant() {
    let cfgnb: u64 = kani::any();
    let ns: u32 = kani::any();
    kani::assume(ns != 1);
    // FALSE: claims a foreign namespace is accepted.
    assert_eq!(validate_lba(cfgnb, ns, 0, 0), Ok(()));
}

// ===========================================================================
// BDF-LBA-OVERFLOW — lba + num_blocks overflow rejected with Overflow.
// ===========================================================================
#[kani::proof]
fn verify_bdf_lba_overflow() {
    let cfgnb: u64 = kani::any();
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    kani::assume(lba.checked_add(n).is_none());
    assert_eq!(validate_lba(cfgnb, 1, lba, n), Err(LbaError::Overflow));
}
#[kani::proof]
fn verify_bdf_lba_overflow__mutant() {
    let cfgnb: u64 = kani::any();
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    kani::assume(lba.checked_add(n).is_none());
    // FALSE: claims a wrapping lba range is in-range.
    assert_eq!(validate_lba(cfgnb, 1, lba, n), Ok(()));
}

// ===========================================================================
// BDF-LBA-INRANGE — accepted iff lba+n <= cfg.num_blocks, else OutOfRange.
// ===========================================================================
#[kani::proof]
fn verify_bdf_lba_inrange() {
    let cfgnb: u64 = kani::any();
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    kani::assume(lba.checked_add(n).is_some());
    let end = lba + n;
    let r = validate_lba(cfgnb, 1, lba, n);
    if end <= cfgnb {
        assert_eq!(r, Ok(()));
    } else {
        assert_eq!(r, Err(LbaError::OutOfRange));
    }
}
#[kani::proof]
fn verify_bdf_lba_inrange__mutant() {
    let cfgnb: u64 = kani::any();
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    kani::assume(lba.checked_add(n).is_some());
    kani::assume(lba + n > cfgnb); // strictly past the end
    // FALSE: claims an out-of-range request is accepted.
    assert_eq!(validate_lba(cfgnb, 1, lba, n), Ok(()));
}

// ===========================================================================
// BDF-OFFSET-EQ — offset is block-aligned to lba (offset/bs == lba, offset%bs==0).
// Proved in the gated context (valid config + validate_lba OK) where the
// product is well-defined.
// ===========================================================================
#[kani::proof]
fn verify_bdf_offset_eq() {
    let bs: u32 = kani::any();
    let nb: u64 = kani::any();
    let cfg = match DeviceConfig::new(bs, nb) {
        Ok(c) => c,
        Err(_) => return,
    };
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    if validate_lba(cfg.num_blocks(), 1, lba, n).is_ok() {
        let off = offset_for_lba(lba, cfg.block_size());
        let bsz = cfg.block_size() as u64; // >= 512, never 0
        assert_eq!(off % bsz, 0);
        assert_eq!(off / bsz, lba);
    }
}
#[kani::proof]
fn verify_bdf_offset_eq__mutant() {
    let cfg = DeviceConfig::new(4096, 1024).unwrap();
    let lba: u64 = kani::any();
    kani::assume(lba < 1024);
    let off = offset_for_lba(lba, cfg.block_size());
    // FALSE: claims the offset recovers a shifted block index.
    assert_eq!(off / (cfg.block_size() as u64), lba + 1);
}

// ===========================================================================
// BDF-OFFSET-NOOVERFLOW — validated offset never overflows and stays within
// total_bytes.
// ===========================================================================
#[kani::proof]
fn verify_bdf_offset_nooverflow() {
    let bs: u32 = kani::any();
    let nb: u64 = kani::any();
    let cfg = match DeviceConfig::new(bs, nb) {
        Ok(c) => c,
        Err(_) => return,
    };
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    if validate_lba(cfg.num_blocks(), 1, lba, n).is_ok() {
        // offset_for_lba multiplies; Kani's overflow check must pass here.
        let off = offset_for_lba(lba, cfg.block_size());
        assert!(off <= cfg.total_bytes());
    }
}
#[kani::proof]
fn verify_bdf_offset_nooverflow__mutant() {
    let bs: u32 = kani::any();
    let nb: u64 = kani::any();
    let cfg = match DeviceConfig::new(bs, nb) {
        Ok(c) => c,
        Err(_) => return,
    };
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    if validate_lba(cfg.num_blocks(), 1, lba, n).is_ok() {
        let off = offset_for_lba(lba, cfg.block_size());
        // FALSE: claims the offset always lands strictly below total_bytes,
        // which fails at lba == num_blocks (n == 0), where off == total_bytes.
        assert!(off < cfg.total_bytes());
    }
}

// ===========================================================================
// BDF-SAFETY-NO-OOB — the full validated IO byte-range stays within capacity.
// (Arithmetic composition; the "every handler is gated" placement fact is a
// code-structure obligation delegated to Creusot / code review.)
// ===========================================================================
#[kani::proof]
fn verify_bdf_safety_no_oob() {
    let bs: u32 = kani::any();
    let nb: u64 = kani::any();
    let cfg = match DeviceConfig::new(bs, nb) {
        Ok(c) => c,
        Err(_) => return,
    };
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    if validate_lba(cfg.num_blocks(), 1, lba, n).is_ok() {
        let bsz = cfg.block_size() as u64;
        let off = offset_for_lba(lba, cfg.block_size());
        let span = n * bsz; // bytes touched; must not overflow
        let io_end = off + span; // must not overflow
        assert!(io_end <= cfg.total_bytes());
        assert_eq!(io_end, (lba + n) * bsz);
    }
}
#[kani::proof]
fn verify_bdf_safety_no_oob__mutant() {
    let bs: u32 = kani::any();
    let nb: u64 = kani::any();
    let cfg = match DeviceConfig::new(bs, nb) {
        Ok(c) => c,
        Err(_) => return,
    };
    let lba: u64 = kani::any();
    let n: u64 = kani::any();
    kani::assume(n > 0);
    if validate_lba(cfg.num_blocks(), 1, lba, n).is_ok() {
        let bsz = cfg.block_size() as u64;
        let io_end = offset_for_lba(lba, cfg.block_size()) + n * bsz;
        // FALSE: claims the IO end never reaches capacity, but a full-span
        // request (lba+n == num_blocks) ends exactly at total_bytes.
        assert!(io_end < cfg.total_bytes());
    }
}

// ===========================================================================
// BDF-SECTORSIZE — sector_size(1) == block_size; other ns -> InvalidNamespace.
// ===========================================================================
#[kani::proof]
fn verify_bdf_sectorsize() {
    let bs: u32 = kani::any();
    let a = Accessors {
        block_size: bs,
        num_blocks: 0,
    };
    let ns: u32 = kani::any();
    if ns == 1 {
        assert_eq!(a.sector_size(ns), Ok(bs));
    } else {
        assert_eq!(a.sector_size(ns), Err(NsError::InvalidNamespace));
    }
}
#[kani::proof]
fn verify_bdf_sectorsize__mutant() {
    let bs: u32 = kani::any();
    let a = Accessors {
        block_size: bs,
        num_blocks: 0,
    };
    // FALSE: claims a foreign namespace returns the block size.
    assert_eq!(a.sector_size(2), Ok(bs));
}

// ===========================================================================
// BDF-NUMSECTORS — num_sectors(1) == num_blocks; other ns -> InvalidNamespace.
// ===========================================================================
#[kani::proof]
fn verify_bdf_numsectors() {
    let nb: u64 = kani::any();
    let a = Accessors {
        block_size: 512,
        num_blocks: nb,
    };
    let ns: u32 = kani::any();
    if ns == 1 {
        assert_eq!(a.num_sectors(ns), Ok(nb));
    } else {
        assert_eq!(a.num_sectors(ns), Err(NsError::InvalidNamespace));
    }
}
#[kani::proof]
fn verify_bdf_numsectors__mutant() {
    let nb: u64 = kani::any();
    let a = Accessors {
        block_size: 512,
        num_blocks: nb,
    };
    // FALSE: claims namespace 1 returns a shifted sector count.
    assert_eq!(a.num_sectors(1), Ok(nb.wrapping_add(1)));
}

// ===========================================================================
// BDF-MAXQUEUEDEPTH — fixed submission-queue depth 128.
// ===========================================================================
#[kani::proof]
fn verify_bdf_maxqueuedepth() {
    let a = Accessors {
        block_size: 512,
        num_blocks: 1,
    };
    assert_eq!(a.max_queue_depth(), 128);
}
#[kani::proof]
fn verify_bdf_maxqueuedepth__mutant() {
    let a = Accessors {
        block_size: 512,
        num_blocks: 1,
    };
    // FALSE.
    assert_eq!(a.max_queue_depth(), 64);
}

// ===========================================================================
// BDF-NUMIOQUEUES — exactly 1 IO queue.
// ===========================================================================
#[kani::proof]
fn verify_bdf_numioqueues() {
    let a = Accessors {
        block_size: 512,
        num_blocks: 1,
    };
    assert_eq!(a.num_io_queues(), 1);
}
#[kani::proof]
fn verify_bdf_numioqueues__mutant() {
    let a = Accessors {
        block_size: 512,
        num_blocks: 1,
    };
    // FALSE.
    assert_eq!(a.num_io_queues(), 2);
}

// ===========================================================================
// BDF-MAXTRANSFER — block_size.saturating_mul(256): == bs*256 in range, else
// saturates to u32::MAX (the documented divergence from spec's plain bs*256).
// ===========================================================================
#[kani::proof]
fn verify_bdf_maxtransfer() {
    let bs: u32 = kani::any();
    let a = Accessors {
        block_size: bs,
        num_blocks: 1,
    };
    let mt = a.max_transfer_size();
    if bs <= u32::MAX / 256 {
        assert_eq!(mt, bs * 256);
    } else {
        assert_eq!(mt, u32::MAX);
    }
}
#[kani::proof]
fn verify_bdf_maxtransfer__mutant() {
    let bs: u32 = kani::any();
    let a = Accessors {
        block_size: bs,
        num_blocks: 1,
    };
    // FALSE: claims plain wrapping multiply (would differ from saturating for
    // large bs, and is anyway asserted for all bs here).
    assert_eq!(a.max_transfer_size(), bs.wrapping_mul(256));
}

// ===========================================================================
// BDF-BLOCKSIZE — block_size() returns the configured block size.
// ===========================================================================
#[kani::proof]
fn verify_bdf_blocksize() {
    let bs: u32 = kani::any();
    let a = Accessors {
        block_size: bs,
        num_blocks: 1,
    };
    assert_eq!(a.block_size(), bs);
}
#[kani::proof]
fn verify_bdf_blocksize__mutant() {
    let bs: u32 = kani::any();
    kani::assume(bs != 999);
    let a = Accessors {
        block_size: bs,
        num_blocks: 1,
    };
    // FALSE.
    assert_eq!(a.block_size(), 999);
}

// ===========================================================================
// BDF-NUMANODE — fixed placeholder -1.
// ===========================================================================
#[kani::proof]
fn verify_bdf_numanode() {
    let a = Accessors {
        block_size: 512,
        num_blocks: 1,
    };
    assert_eq!(a.numa_node(), -1);
}
#[kani::proof]
fn verify_bdf_numanode__mutant() {
    let a = Accessors {
        block_size: 512,
        num_blocks: 1,
    };
    // FALSE.
    assert_eq!(a.numa_node(), 0);
}

// ===========================================================================
// BDF-NVMEVERSION — fixed placeholder string.
// ===========================================================================
#[kani::proof]
fn verify_bdf_nvmeversion() {
    let a = Accessors {
        block_size: 512,
        num_blocks: 1,
    };
    assert_eq!(a.nvme_version(), "N/A (file-backed)");
}
#[kani::proof]
fn verify_bdf_nvmeversion__mutant() {
    let a = Accessors {
        block_size: 512,
        num_blocks: 1,
    };
    // FALSE.
    assert_eq!(a.nvme_version(), "NVMe 1.4");
}

// ===========================================================================
// BDF-TELEMETRY-MEAN-LATENCY — mean == total_latency / total_ops; 0 with no ops.
// ===========================================================================
#[kani::proof]
fn verify_bdf_telemetry_mean_latency() {
    let mut s = TelemetryStats::new();
    // No ops recorded -> mean 0.
    assert_eq!(s.snapshot(0.0).mean_latency_ns, 0);

    let l0: u64 = kani::any();
    let l1: u64 = kani::any();
    let l2: u64 = kani::any();
    s.record_op(l0, 0);
    s.record_op(l1, 0);
    s.record_op(l2, 0);
    let snap = s.snapshot(1.0);
    assert_eq!(snap.total_ops, 3);
    let sum = l0.wrapping_add(l1).wrapping_add(l2);
    assert_eq!(snap.mean_latency_ns, sum / 3);
}
#[kani::proof]
fn verify_bdf_telemetry_mean_latency__mutant() {
    let s = TelemetryStats::new();
    // FALSE: claims a fresh collector reports a non-zero mean latency.
    assert_eq!(s.snapshot(0.0).mean_latency_ns, 1);
}

// ===========================================================================
// BDF-TELEMETRY-MEAN-THROUGHPUT — 0.0 when no time elapsed or no bytes.
// ===========================================================================
#[kani::proof]
fn verify_bdf_telemetry_mean_throughput() {
    // elapsed == 0 -> 0.0 regardless of bytes transferred.
    let b: u64 = kani::any();
    let mut s = TelemetryStats::new();
    s.record_op(0, b);
    assert_eq!(s.snapshot(0.0).mean_throughput_mbps, 0.0);

    // positive elapsed but zero bytes -> 0.0.
    let s2 = TelemetryStats::new();
    let e: f64 = kani::any();
    kani::assume(e > 0.0);
    kani::assume(e.is_finite());
    assert_eq!(s2.snapshot(e).mean_throughput_mbps, 0.0);
}
#[kani::proof]
fn verify_bdf_telemetry_mean_throughput__mutant() {
    let mut s = TelemetryStats::new();
    s.record_op(0, 5); // 5 bytes
    // FALSE: claims zero elapsed still yields a positive throughput.
    assert!(s.snapshot(0.0).mean_throughput_mbps > 0.0);
}

// ===========================================================================
// BDF-TELEMETRY-MINMAX — snapshot reports true min/max; min 0 with no ops.
// ===========================================================================
#[kani::proof]
fn verify_bdf_telemetry_minmax() {
    let mut s = TelemetryStats::new();
    assert_eq!(s.snapshot(0.0).min_latency_ns, 0); // no ops -> 0

    let l0: u64 = kani::any();
    let l1: u64 = kani::any();
    let l2: u64 = kani::any();
    s.record_op(l0, 0);
    s.record_op(l1, 0);
    s.record_op(l2, 0);
    let snap = s.snapshot(1.0);
    let true_min = l0.min(l1).min(l2);
    let true_max = l0.max(l1).max(l2);
    assert_eq!(snap.min_latency_ns, true_min);
    assert_eq!(snap.max_latency_ns, true_max);
}
#[kani::proof]
fn verify_bdf_telemetry_minmax__mutant() {
    let mut s = TelemetryStats::new();
    let l0: u64 = kani::any();
    let l1: u64 = kani::any();
    s.record_op(l0, 0);
    s.record_op(l1, 0);
    // FALSE: claims min equals max even when the two latencies differ.
    assert_eq!(s.snapshot(1.0).min_latency_ns, s.snapshot(1.0).max_latency_ns);
}

// ===========================================================================
// BDF-TELEMETRY-RECORD — each record increments op count by one; latency
// accumulates into the running total (observed via mean).
// ===========================================================================
#[kani::proof]
fn verify_bdf_telemetry_record() {
    let mut s = TelemetryStats::new();
    let l0: u64 = kani::any();
    s.record_op(l0, kani::any());
    let s1 = s.snapshot(1.0);
    assert_eq!(s1.total_ops, 1);
    assert_eq!(s1.mean_latency_ns, l0); // sum(=l0) / 1

    let l1: u64 = kani::any();
    s.record_op(l1, kani::any());
    let s2 = s.snapshot(1.0);
    assert_eq!(s2.total_ops, 2);
    assert_eq!(s2.mean_latency_ns, l0.wrapping_add(l1) / 2);
}
#[kani::proof]
fn verify_bdf_telemetry_record__mutant() {
    let mut s = TelemetryStats::new();
    s.record_op(kani::any(), kani::any());
    // FALSE: claims one recorded op leaves the count at 0.
    assert_eq!(s.snapshot(1.0).total_ops, 0);
}

// ===========================================================================
// BDF-OPHANDLE-UNIQUE — counter starts at 1, advances by wrapping +1.
// ===========================================================================
#[kani::proof]
fn verify_bdf_ophandle_unique() {
    let mut g = HandleGen::new();
    assert_eq!(g.next_op_handle(), 1);
    assert_eq!(g.next_op_handle(), 2);
    assert_eq!(g.next_op_handle(), 3);

    // General: successive handles differ by a wrapping +1 from any start.
    let start: u64 = kani::any();
    let mut g2 = HandleGen::with_start(start);
    let a = g2.next_op_handle();
    let b = g2.next_op_handle();
    assert_eq!(a, start);
    assert_eq!(b, a.wrapping_add(1));
}
#[kani::proof]
fn verify_bdf_ophandle_unique__mutant() {
    let mut g = HandleGen::new();
    // FALSE: claims the first handle is 0 (it starts at 1).
    assert_eq!(g.next_op_handle(), 0);
}

// ===========================================================================
// BDF-HARVEST-FSYNC-SKIP — the chained-fsync completion (high user_data bit) is
// skipped; the write completion (bit clear) is not.
// ===========================================================================
#[kani::proof]
fn verify_bdf_harvest_fsync_skip() {
    let h: u64 = kani::any();
    kani::assume(h < FSYNC_TAG_BIT); // real handles come from a small counter
    assert!(!harvest_is_skipped(h)); // write submission's user_data == handle
    assert!(harvest_is_skipped(h | FSYNC_TAG_BIT)); // chained fsync's user_data
}
#[kani::proof]
fn verify_bdf_harvest_fsync_skip__mutant() {
    let h: u64 = kani::any();
    kani::assume(h < FSYNC_TAG_BIT);
    // FALSE: claims the tagged fsync completion is NOT skipped.
    assert!(!harvest_is_skipped(h | FSYNC_TAG_BIT));
}

// ===========================================================================
// BDF-UNSUPPORTED — the four NVMe admin commands map to NotSupported.
// ===========================================================================
#[kani::proof]
#[kani::unwind(6)]
fn verify_bdf_unsupported() {
    for c in [
        AdminCmd::NsCreate,
        AdminCmd::NsDelete,
        AdminCmd::NsFormat,
        AdminCmd::ControllerReset,
    ] {
        assert!(admin_is_not_supported(c));
    }
}
#[kani::proof]
fn verify_bdf_unsupported__mutant() {
    // FALSE: claims an admin command is supported.
    assert!(!admin_is_not_supported(AdminCmd::NsCreate));
}

// ===========================================================================
// BDF-FLUSHSYNC-NS — flush on ns != 1 is rejected before touching the file.
// ===========================================================================
#[kani::proof]
fn verify_bdf_flushsync_ns() {
    let ns: u32 = kani::any();
    let g = flush_ns_guard(ns);
    if ns == 1 {
        assert_eq!(g, FlushGuard::Proceed);
    } else {
        assert_eq!(g, FlushGuard::InvalidNamespace);
    }
}
#[kani::proof]
fn verify_bdf_flushsync_ns__mutant() {
    let ns: u32 = kani::any();
    kani::assume(ns != 1);
    // FALSE: claims a foreign namespace flush proceeds.
    assert_eq!(flush_ns_guard(ns), FlushGuard::Proceed);
}
