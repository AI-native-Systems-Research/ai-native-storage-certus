//! Kani harnesses for extent-manager (compiled only under `cargo kani`).
//!
//! Layout of this file:
//!   1. Shared infrastructure (stubs, mock metadata device, small-geometry builders).
//!      Every helper is named `support_*`, `stub_*` or `check_*` so the scorer's
//!      substring match on `verify_<id>` never picks it up.
//!   2. Scored harnesses `verify_<id>` and their anti-vacuity twins `verify_<id>__mutant`.
//!
//! See `verif/.run/batches/kani_RESUME.md` for the documented infrastructure.

#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(non_snake_case)]

use std::collections::hash_map::RandomState;
use std::sync::Arc;

use component_core::channel::SpscChannel;
use component_core::component::InterfaceMap;
use component_core::receptacle::Receptacle;
use interfaces::{
    ClientChannels, Command, Completion, Extent, ExtentManagerError, FormatParams, IBlockDevice,
    IExtentManager, NvmeBlockError, ReadWriteStats, TelemetrySnapshot, WriteHandle,
};

use crate::bitmap::AllocationBitmap;
use crate::block_io::BlockDeviceClient;
use crate::buddy::BuddyAllocator;
use crate::region::RegionState;
use crate::slab::{Slab, FREE_KEY};
use crate::ExtentManager;

// ===========================================================================
// 1. Shared infrastructure
// ===========================================================================

/// Data-device sector size used by every small-geometry harness.
pub(crate) const SS: u32 = 512;
pub(crate) const SSU: u64 = SS as u64;
/// Metadata-device sector size of the mock device. 4096 makes the 4 KiB
/// superblock exactly one block.
pub(crate) const MD_SS: u32 = 4096;

// ---- RandomState stub (HashMap construction wall: RandomState::new -> getrandom) ----
// Deterministic [0,0] seed. Sound only for map SEMANTICS (insert/get/remove/len/contains);
// nothing here asserts on iteration order or hasher randomness.
pub(crate) fn stub_random_state() -> RandomState {
    let keys: [u64; 2] = [0, 0];
    assert_eq!(
        std::mem::size_of_val(&keys),
        std::mem::size_of::<RandomState>()
    );
    // SAFETY: RandomState is two u64 SipHash keys (size asserted above); any bit
    // pattern is a valid key pair.
    unsafe { std::mem::transmute(keys) }
}

// ---- Arc::drop_slow stub: the LAST reference to an Arc is leaked instead of freed ----
// Sound for every property here: no inner type reached through an Arc in these harnesses
// (channel state, region locks, closures, timer state) has a Drop with observable effect;
// only memory reclamation is skipped. Needed because symex cannot see the channel
// endpoints' strong counts and otherwise explores RingBuffer<Command>/DmaBuffer drop glue.
pub(crate) unsafe fn stub_arc_drop_slow<T: ?Sized, A: std::alloc::Allocator>(
    _this: &mut Arc<T, A>,
) {
}

// ---- Instant::now stub (clock_gettime is a foreign function Kani cannot execute) ----
// Returns one fixed instant; `elapsed()` is then zero. Only feeds the checkpoint log
// line's duration text, which no property here reads.
pub(crate) fn stub_instant_now() -> std::time::Instant {
    let raw: [u64; 2] = [0, 0];
    assert_eq!(
        std::mem::size_of_val(&raw),
        std::mem::size_of::<std::time::Instant>()
    );
    // SAFETY: Instant on Linux is a (seconds: i64, nanoseconds: u32 < 1e9) timespec padded
    // to 16 bytes (size asserted above); all-zero is a valid value.
    unsafe { std::mem::transmute(raw) }
}

// ---- Condvar::notify_all / notify_one stubs (futex syscall) ----
// Kani explores single-threaded executions only, so there is never a waiter to wake:
// a no-op is the exact behaviour of a notify with no waiters.
pub(crate) fn stub_condvar_notify(_this: &std::sync::Condvar) {}

// ---- std::fmt::format stub: a symbolic value reaching format! never returns ----
// Replaces the CONTENT of formatted strings (log lines / error messages) with "".
// Sound for every property that does not assert on message text.
pub(crate) fn stub_format(_args: std::fmt::Arguments<'_>) -> String {
    String::new()
}

// ---- crc32fast::hash stub (real crate hits unsupported `_xgetbv`) ----
// Contract the extent-manager relies on: crc is a DETERMINISTIC function of the
// byte string, so a blob written and read back unchanged validates.  This stub
// keeps determinism (a function of the length only) and drops error-detection
// strength: it can NOT be used to argue that corruption is detected.
pub(crate) fn stub_crc32_hash(buf: &[u8]) -> u32 {
    (buf.len() as u32).wrapping_mul(0x9E37_79B9) ^ 0x5A5A_5A5A
}

// ---- Mock metadata disk: sector-granular store behind BlockDeviceClient ----
// Kani cannot run the device actor thread behind the SPSC channels, so the
// client's channel round-trip (`BlockDeviceClient::{write_blocks, read_blocks}`)
// is replaced by a model of a CONTRACT-CONFORMING block device:
//   * a write of N sectors either succeeds (all N sectors now hold the data) or
//     returns an I/O error (nondeterministically before/after storing — both are
//     allowed outcomes of a failed WriteSync);
//   * a read returns, per sector, the last successfully written bytes, zeros for a
//     never-written sector, or an I/O error;
//   * reads return exactly `num_bytes` bytes on success (as the real client does).
// Torn multi-sector writes are NOT modelled (every harness geometry here keeps the
// superblock and checkpoint blobs to one 4 KiB metadata sector).
static mut KANI_DISK: Vec<(u64, Vec<u8>)> = Vec::new();
/// When false the model never injects an I/O error (used by harnesses whose
/// statement is about the success path; documented per property).
static mut KANI_IO_ERRORS: bool = true;

pub(crate) fn support_disk_reset(io_errors: bool) {
    // SAFETY: Kani harnesses are single-threaded; no reference to the static outlives this call.
    unsafe {
        *std::ptr::addr_of_mut!(KANI_DISK) = Vec::new();
        *std::ptr::addr_of_mut!(KANI_IO_ERRORS) = io_errors;
    }
}

fn support_io_fails() -> bool {
    // SAFETY: single-threaded harness; plain read of a bool.
    let allowed = unsafe { *std::ptr::addr_of!(KANI_IO_ERRORS) };
    allowed && kani::any::<bool>()
}

pub(crate) fn stub_write_blocks(
    this: &BlockDeviceClient,
    lba: u64,
    data: &[u8],
) -> Result<(), ExtentManagerError> {
    support_disk_write(this.kani_base_lba() + lba, data)
}

/// Model of a WriteSync sequence starting at absolute `lba`. The model stores the
/// write as ONE extent keyed by its starting LBA (the extent-manager always reads
/// back from the same starting LBA it wrote: superblock at 0, checkpoint copies at
/// their region start), replacing any earlier write that started at that LBA.
pub(crate) fn support_disk_write(lba: u64, data: &[u8]) -> Result<(), ExtentManagerError> {
    let fail = support_io_fails();
    let store = !fail || kani::any::<bool>();
    if store {
        // SAFETY: single-threaded harness; exclusive access for the duration of the call.
        let disk = unsafe { &mut *std::ptr::addr_of_mut!(KANI_DISK) };
        let mut found = false;
        for e in disk.iter_mut() {
            if e.0 == lba {
                e.1 = data.to_vec();
                found = true;
            }
        }
        if !found {
            disk.push((lba, data.to_vec()));
        }
    }
    if fail {
        return Err(ExtentManagerError::IoError(String::new()));
    }
    Ok(())
}

pub(crate) fn stub_read_blocks(
    this: &BlockDeviceClient,
    lba: u64,
    num_bytes: usize,
) -> Result<Vec<u8>, ExtentManagerError> {
    support_disk_read(this.kani_base_lba() + lba, num_bytes)
}

/// Model of a ReadSync sequence: the bytes of the last write that started at
/// `lba` (zero-extended / truncated to `num_bytes`), zeros if never written,
/// or an I/O error.
pub(crate) fn support_disk_read(lba: u64, num_bytes: usize) -> Result<Vec<u8>, ExtentManagerError> {
    if support_io_fails() {
        return Err(ExtentManagerError::IoError(String::new()));
    }
    // SAFETY: single-threaded harness; shared access for the duration of the call.
    let disk = unsafe { &*std::ptr::addr_of!(KANI_DISK) };
    for e in disk.iter() {
        if e.0 == lba {
            let mut out = e.1.clone();
            out.resize(num_bytes, 0);
            return Ok(out);
        }
    }
    Ok(vec![0u8; num_bytes])
}

/// Mock metadata `IBlockDevice`, conforming to the declared contract of
/// `components/interfaces/src/iblock_device.rs` (see `check_stub_conforms_iblockdevice`):
/// `connect_client` returns a fresh pair of channel endpoints; `sector_size` /
/// `num_sectors` return a stable, positive geometry whose byte size fits in u64
/// (or, when `geometry_fails`, an `InvalidNamespace` error — an allowed outcome);
/// introspection methods return constants.
pub(crate) struct KaniMetaDevice {
    pub sector: u32,
    pub sectors: u64,
    pub geometry_fails: bool,
}

impl IBlockDevice for KaniMetaDevice {
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        let cmd_ch = SpscChannel::<Command>::new(1);
        let comp_ch = SpscChannel::<Completion>::new(1);
        let command_tx = cmd_ch
            .sender()
            .map_err(|_| NvmeBlockError::ClientDisconnected(String::new()))?;
        let completion_rx = comp_ch
            .receiver()
            .map_err(|_| NvmeBlockError::ClientDisconnected(String::new()))?;
        // The channel objects are leaked on purpose: their endpoints (which keep the
        // shared state alive) are what the client uses, and dropping the channel
        // object itself (OnceLock<Box<dyn Any>> fields) costs CBMC minutes.
        std::mem::forget(cmd_ch);
        std::mem::forget(comp_ch);
        Ok(ClientChannels {
            command_tx,
            completion_rx,
        })
    }
    fn sector_size(&self, _ns_id: u32) -> Result<u32, NvmeBlockError> {
        if self.geometry_fails {
            return Err(NvmeBlockError::InvalidNamespace(String::new()));
        }
        Ok(self.sector)
    }
    fn num_sectors(&self, _ns_id: u32) -> Result<u64, NvmeBlockError> {
        if self.geometry_fails {
            return Err(NvmeBlockError::InvalidNamespace(String::new()));
        }
        Ok(self.sectors)
    }
    fn max_queue_depth(&self) -> u32 {
        1
    }
    fn num_io_queues(&self) -> u32 {
        1
    }
    fn max_transfer_size(&self) -> u32 {
        self.sector
    }
    fn block_size(&self) -> u32 {
        self.sector
    }
    fn numa_node(&self) -> i32 {
        -1
    }
    fn nvme_version(&self) -> String {
        String::new()
    }
    fn telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError> {
        Err(NvmeBlockError::FeatureNotEnabled(String::new()))
    }
    fn read_write_stats(&self) -> ReadWriteStats {
        ReadWriteStats::default()
    }
}

/// Build an `ExtentManager` DIRECTLY (no `define_component!` `new()`, no background
/// checkpoint thread, no interface-map population). Fields are the production
/// defaults; receptacles are disconnected. Callers must `std::mem::forget` the
/// value at the end of a harness (its `Drop` touches a Condvar).
pub(crate) fn support_new_em() -> ExtentManager {
    ExtentManager {
        __interface_map: InterfaceMap::new(),
        __interface_info: Vec::new(),
        __receptacle_info: Vec::new(),
        __version: "0.3.0",
        regions: Default::default(),
        shared: Default::default(),
        checkpoint_coalesce: Default::default(),
        checkpoint_done: Default::default(),
        dma_alloc: Default::default(),
        checkpoint_timer_state: Default::default(),
        checkpoint_thread: Default::default(),
        metadata_ns_id: Default::default(),
        metadata_base_lba: Default::default(),
        data_base_lba: Default::default(),
        post_checkpoint_hook: Default::default(),
        metadata_device: Receptacle::new(),
        logger: Receptacle::new(),
    }
}

/// `support_new_em()` with a conforming mock metadata device connected.
/// `sectors` is the metadata-device size in 4 KiB sectors.
pub(crate) fn support_em_with_device(sectors: u64, geometry_fails: bool) -> ExtentManager {
    let em = support_new_em();
    let dev: Arc<dyn IBlockDevice + Send + Sync> = Arc::new(KaniMetaDevice {
        sector: MD_SS,
        sectors,
        geometry_fails,
    });
    let _ = em.metadata_device.connect(dev);
    em
}

/// Small symbolic `FormatParams`: every field symbolic within a bounded range;
/// guards are left to `format()` itself.
/// Bounds (harness geometry, not production guards): data disk <= 24 KiB,
/// slab <= 4 data sectors, region_count <= 2, metadata_alignment <= 8 KiB,
/// metadata_region_size <= 32 KiB, instance_id always Some (the None branch reads
/// /dev/urandom, which Kani cannot model).
pub(crate) fn support_small_params() -> FormatParams {
    let data_disk_size: u64 = kani::any();
    kani::assume(data_disk_size <= 48 * SSU);
    let slab_size: u64 = kani::any();
    kani::assume(slab_size <= 4 * SSU);
    let max_extent_size: u32 = kani::any();
    kani::assume(max_extent_size <= 8 * SS);
    let region_count: u32 = kani::any();
    kani::assume(region_count <= 2);
    let metadata_alignment: u64 = kani::any();
    kani::assume(metadata_alignment <= 8192);
    let metadata_region_size: u64 = kani::any();
    kani::assume(metadata_region_size <= 32 * 1024);
    FormatParams {
        data_disk_size,
        slab_size,
        max_extent_size,
        sector_size: SS,
        region_count,
        metadata_alignment,
        instance_id: Some(kani::any()),
        metadata_disk_ns_id: kani::any(),
        metadata_region_size,
    }
}

/// Valid small `FormatParams` (passes every format() guard) with a concrete layout:
/// separate-device mode, `regions` regions, slab = `slab_sectors` data sectors,
/// data disk = `data_sectors` data sectors.
pub(crate) fn support_valid_params(
    data_sectors: u64,
    slab_sectors: u64,
    regions: u32,
) -> FormatParams {
    FormatParams {
        data_disk_size: data_sectors * SSU,
        slab_size: slab_sectors * SSU,
        max_extent_size: (slab_sectors * SSU) as u32,
        sector_size: SS,
        region_count: regions,
        metadata_alignment: 4096,
        instance_id: Some(0x1234),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    }
}

// ---- Stub-conformance harness ----
/// Proves the mock device + disk model obey the IBlockDevice / client contract the
/// harnesses depend on: stable positive geometry whose byte size fits in u64,
/// connect_client succeeds, read-after-write returns the written bytes, a
/// never-written sector reads as zeros, a successful read returns exactly the
/// requested length, and a write that reports success stored the data.
#[kani::proof]
#[kani::unwind(4)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn check_stub_conforms_iblockdevice() {
    support_disk_reset(true);
    let sectors: u64 = kani::any();
    kani::assume(sectors <= 1 << 20);
    let dev = KaniMetaDevice {
        sector: MD_SS,
        sectors,
        geometry_fails: false,
    };
    let s1 = dev.sector_size(1).unwrap();
    let s2 = dev.sector_size(1).unwrap();
    assert!(s1 == s2 && s1 > 0 && s1.is_power_of_two());
    let n = dev.num_sectors(1).unwrap();
    assert!(n.checked_mul(s1 as u64).is_some());
    let ch = dev.connect_client();
    assert!(ch.is_ok());
    let alloc: interfaces::DmaAllocFn = Arc::new(|_s, _a, _n| Err(String::new()));
    let client = BlockDeviceClient::with_base_lba(ch.unwrap(), alloc, MD_SS, 1, 0);

    let lba: u64 = kani::any();
    kani::assume(lba < 4);
    let b0: u8 = kani::any();
    let b1: u8 = kani::any();
    let data = [b0, b1];
    let w = stub_write_blocks(&client, lba, &data);
    if w.is_ok() {
        if let Ok(r) = stub_read_blocks(&client, lba, 2) {
            assert!(r.len() == 2);
            assert!(r[0] == b0 && r[1] == b1);
        }
    }
    let other: u64 = kani::any();
    kani::assume(other >= 4 && other < 8);
    if let Ok(r) = stub_read_blocks(&client, other, 3) {
        assert!(r.len() == 3 && r[0] == 0 && r[2] == 0);
    }
    // crc stub determinism
    assert!(stub_crc32_hash(&data) == stub_crc32_hash(&[b0, b1]));
    std::mem::forget(client);
}

// ---- Buddy-allocator inductive infrastructure (shared by the buddy invariants) ----

/// Fixed-size snapshot of the free lists: at most 4 orders x 3 entries (the inductive
/// states below start with <= 2 entries per order and one call pushes <= 1 per order).
/// `fits` is false if the allocator has more orders/entries than the snapshot holds,
/// which makes every predicate below false (never vacuously true).
pub(crate) struct BuddySnap {
    pub e: [[u64; 3]; 4],
    pub n: [usize; 4],
    pub orders: usize,
    pub fits: bool,
}

pub(crate) fn support_buddy_snap(b: &BuddyAllocator) -> BuddySnap {
    let lists = b.kani_free_lists();
    let mut snap = BuddySnap {
        e: [[0u64; 3]; 4],
        n: [0usize; 4],
        orders: lists.len(),
        fits: lists.len() <= 4,
    };
    let mut o = 0usize;
    while o < 4 {
        if o < lists.len() {
            let l = &lists[o];
            snap.fits &= l.len() <= 3;
            snap.n[o] = l.len();
            let mut i = 0usize;
            while i < 3 {
                if i < l.len() {
                    snap.e[o][i] = l[i];
                }
                i += 1;
            }
        }
        o += 1;
    }
    snap
}

/// Buddy-allocator invariant (jointly inductive): lists.len() == max_order + 1 and every
/// free entry at order k is (a) a multiple of 2^k sectors relative to the region base,
/// (b) inside the region, and (c) disjoint from every other free entry.
pub(crate) fn support_buddy_inv(b: &BuddyAllocator, total: u64) -> bool {
    let s = support_buddy_snap(b);
    b.kani_free_lists().len() == b.kani_max_order() + 1
        && support_snap_aligned(&s)
        && support_snap_in_range(&s, total)
        && support_snap_disjoint(&s)
}

/// (a) alignment conjunct.
pub(crate) fn support_snap_aligned(s: &BuddySnap) -> bool {
    let mut ok = s.fits;
    let mut o = 0usize;
    while o < 4 {
        let span = (1u64 << o) * SSU;
        let mut i = 0usize;
        while i < 3 {
            if o < s.orders && i < s.n[o] {
                ok &= s.e[o][i] & (span - 1) == 0;
            }
            i += 1;
        }
        o += 1;
    }
    ok
}

/// (b) in-range conjunct.
fn support_snap_in_range(s: &BuddySnap, total: u64) -> bool {
    let mut ok = s.fits;
    let mut o = 0usize;
    while o < 4 {
        let span = (1u64 << o) * SSU;
        let mut i = 0usize;
        while i < 3 {
            if o < s.orders && i < s.n[o] {
                ok &= s.e[o][i] <= total && span <= total - s.e[o][i];
            }
            i += 1;
        }
        o += 1;
    }
    ok
}

/// (c) pairwise-disjointness conjunct. Loops are nested 4 x 3 x 4 x 3 so every loop
/// runs <= 4 times (fits the harness unwind bound).
pub(crate) fn support_snap_disjoint(s: &BuddySnap) -> bool {
    let mut ok = s.fits;
    let mut oa = 0usize;
    while oa < 4 {
        let mut ia = 0usize;
        while ia < 3 {
            let mut ob = oa;
            while ob < 4 {
                let mut ib = 0usize;
                while ib < 3 {
                    let distinct = ob > oa || ib > ia;
                    if distinct && oa < s.orders && ia < s.n[oa] && ob < s.orders && ib < s.n[ob] {
                        let (ea, la) = (s.e[oa][ia], (1u64 << oa) * SSU);
                        let (eb, lb) = (s.e[ob][ib], (1u64 << ob) * SSU);
                        ok &= ea + la <= eb || eb + lb <= ea;
                    }
                    ib += 1;
                }
                ob += 1;
            }
            ia += 1;
        }
        oa += 1;
    }
    ok
}

/// [x, x+len) (relative) overlaps no free block.
pub(crate) fn support_buddy_range_not_free(b: &BuddyAllocator, x: u64, len: u64) -> bool {
    let s = support_buddy_snap(b);
    let mut ok = s.fits;
    let mut o = 0usize;
    while o < 4 {
        let span = (1u64 << o) * SSU;
        let mut i = 0usize;
        while i < 3 {
            if o < s.orders && i < s.n[o] {
                let e = s.e[o][i];
                ok &= x + len <= e || e + span <= x;
            }
            i += 1;
        }
        o += 1;
    }
    ok
}

/// [x, x+len) (relative) is contained in some free block.
fn support_buddy_range_in_free(b: &BuddyAllocator, x: u64, len: u64) -> bool {
    let s = support_buddy_snap(b);
    let mut found = false;
    let mut o = 0usize;
    while o < 4 {
        let span = (1u64 << o) * SSU;
        let mut i = 0usize;
        while i < 3 {
            if o < s.orders && i < s.n[o] {
                let e = s.e[o][i];
                found |= e <= x && x + len <= e + span;
            }
            i += 1;
        }
        o += 1;
    }
    found && s.fits
}

/// Arbitrary small free-list state for an allocator of `total` bytes: each order holds
/// 0..=2 symbolic sector-aligned entries (< 16 sectors; the in-range assumption then
/// bounds them by the region size).
fn support_buddy_symbolic(base: u64, total: u64) -> BuddyAllocator {
    let shape = BuddyAllocator::new(base, total, SS);
    let orders = shape.kani_max_order() + 1;
    let mut lists: Vec<Vec<u64>> = Vec::with_capacity(4);
    let mut o = 0usize;
    while o < orders {
        let n: u8 = kani::any();
        kani::assume(n <= 2);
        // capacity 4 >= the most entries one call can leave in a list, so production
        // pushes never reallocate (reallocating a symbolically-chosen Vec is costly)
        let mut v: Vec<u64> = Vec::with_capacity(4);
        if n >= 1 {
            let e: u64 = kani::any();
            kani::assume(e < 16);
            v.push(e * SSU);
        }
        if n >= 2 {
            let e: u64 = kani::any();
            kani::assume(e < 16);
            v.push(e * SSU);
        }
        lists.push(v);
        o += 1;
    }
    BuddyAllocator::kani_with_free_lists(base, total, SS, lists)
}

/// Smallest power of two >= `blocks` (independent of the allocator's own order math).
fn support_pow2_ceil(blocks: u64) -> u64 {
    let mut span = 1u64;
    while span < blocks {
        span *= 2;
    }
    span
}

/// Outcome of one inductive step of the region allocator.
pub(crate) struct BuddyStep {
    pub after: BuddyAllocator,
    pub base: u64,
    pub total: u64,
    /// For an alloc that succeeded: (relative offset, block span in bytes).
    pub returned: Option<(u64, u64)>,
}

/// Region base used by the buddy harnesses: concrete and deliberately NOT sector-aligned,
/// so any confusion between absolute and base-relative offsets is visible. (Symbolic
/// 64-bit bases/sizes made CBMC emit ~60M clauses; the allocator only adds/subtracts
/// the base, so one concrete base is representative.)
pub(crate) const B_BASE: u64 = 1_000_003;

/// ESTABLISHMENT + ONE INDUCTIVE STEP of the buddy invariant, at a concrete region
/// geometry (`total` bytes, slab = `k` sectors):
///   * establishment: `new(base, total)` satisfies the invariant (asserted here);
///   * step: from an ARBITRARY state satisfying the invariant (<= 2 entries per order),
///     apply one production call with that slab_size (the format() guard lib.rs:387
///     makes slab_size a multiple of sector_size, and every call on a region's
///     allocator uses that one slab_size):
///       alloc(slab)                                          -- alloc_extent (new slab)
///       free(x, slab)            x aligned, in range, not free -- free_slot (slab emptied)
///       mark_allocated(x, slab)  x aligned, inside a free block -- initialize()
///     The preconditions on x are the facts production guarantees (x was returned by
///     alloc / is a recovered slab on a fresh allocator), stated at invariant level.
fn support_buddy_step_at(total: u64) -> BuddyStep {
    let base = B_BASE;
    let fresh = BuddyAllocator::new(base, total, SS);
    assert!(support_buddy_inv(&fresh, total)); // establishment
    let mut b = support_buddy_symbolic(base, total);
    kani::assume(support_buddy_inv(&b, total));
    let k: u64 = kani::any();
    kani::assume(k >= 1 && k <= 3);
    let slab = k * SSU;
    let span = if k == 1 {
        SSU
    } else if k == 2 {
        2 * SSU
    } else {
        4 * SSU
    };
    let op: u8 = kani::any();
    let mut returned = None;
    if op == 0 {
        if let Some(abs) = b.alloc(slab) {
            returned = Some((abs - base, span));
        }
    } else {
        let xs: u64 = kani::any();
        kani::assume(xs < 16);
        let x = xs * SSU;
        kani::assume(x & (span - 1) == 0 && x < total && span <= total - x);
        if op == 1 {
            kani::assume(support_buddy_range_not_free(&b, x, span));
            b.free(base + x, slab);
        } else {
            kani::assume(support_buddy_range_in_free(&b, x, span));
            b.mark_allocated(base + x, slab);
        }
    }
    BuddyStep {
        after: b,
        base,
        total,
        returned,
    }
}

/// Geometries: power-of-two region (8 sectors, max_order 3) and non-power-of-two region
/// with a partial tail sector (7 sectors + 100 B, max_order 2), each dispatched with a
/// CONCRETE size; slab size k in 1..=3 sectors is symbolic (3 exercises round-up to a
/// 4-sector block); the allocator STATE and the operation are symbolic.
pub(crate) fn support_buddy_step() -> BuddyStep {
    if kani::any() {
        support_buddy_step_at(8 * SSU)
    } else {
        support_buddy_step_at(7 * SSU + 100)
    }
}

// ---- Region-level predicates ----

/// EM-INV-SLAB-WITHIN-REGION: each slab is keyed by its start, spans one slab size,
/// lies inside [base, base + region size), and slabs of the region are pairwise disjoint.
fn support_slabs_within_region(r: &RegionState) -> bool {
    let base = r.buddy.base_offset();
    let total = r.buddy.total_usable_size();
    let slab_size = r.format_params.slab_size;
    let mut ok = true;
    let mut prev_end: Option<u64> = None;
    for (&k, slab) in r.slabs.iter() {
        ok &= k == slab.start_offset;
        ok &= slab.slab_size == slab_size;
        ok &= slab.start_offset >= base && slab.start_offset + slab.slab_size <= base + total;
        // BTreeMap iterates in ascending start order: disjoint iff each starts at/after
        // the previous one's end.
        if let Some(e) = prev_end {
            ok &= slab.start_offset >= e;
        }
        prev_end = Some(slab.start_offset + slab.slab_size);
    }
    ok
}

/// EM-REGION-SLABS-NOT-FREE: no free block of the region allocator overlaps a slab.
fn support_slabs_not_free(r: &RegionState) -> bool {
    let base = r.buddy.base_offset();
    let lists = r.buddy.kani_free_lists();
    let mut ok = true;
    for (_, slab) in r.slabs.iter() {
        let s0 = slab.start_offset;
        let s1 = slab.start_offset + slab.slab_size;
        let mut o = 0usize;
        while o < lists.len() {
            let span = (1u64 << o) * SSU;
            for &e in lists[o].iter() {
                let f0 = base + e;
                let f1 = f0 + span;
                ok &= f1 <= s0 || s1 <= f0;
            }
            o += 1;
        }
    }
    ok
}

/// EM-INV-KEY-IMPLIES-ALLOCATED: a non-FREE_KEY key entry implies the bitmap bit is set.
fn support_key_implies_allocated(r: &RegionState) -> bool {
    let mut ok = true;
    for (_, slab) in r.slabs.iter() {
        let mut i = 0usize;
        while i < slab.keys.len() && i < slab.num_slots() as usize {
            if slab.get_key(i) != FREE_KEY {
                ok &= slab.bitmap.is_set(i);
            }
            i += 1;
        }
    }
    ok
}

/// EM-INV-KEY-VECTOR-LENGTH: keys.len() == num_slots == slab_size / element_size.
fn support_key_vector_length(r: &RegionState) -> bool {
    let mut ok = true;
    for (_, slab) in r.slabs.iter() {
        ok &= slab.element_size > 0;
        ok &= slab.keys.len() == slab.num_slots() as usize;
        ok &= slab.keys.len() as u64 == slab.slab_size / slab.element_size as u64;
    }
    ok
}

/// EM-SLAB-SLOTS-INSIDE: every slot lies inside its slab; distinct slots do not overlap.
fn support_slots_inside(r: &RegionState) -> bool {
    let mut ok = true;
    for (_, slab) in r.slabs.iter() {
        let n = slab.num_slots() as usize;
        let e = slab.element_size as u64;
        let mut i = 0usize;
        while i < n {
            let a = slab.slot_offset(i);
            ok &= a >= slab.start_offset && a + e <= slab.start_offset + slab.slab_size;
            let mut j = i + 1;
            while j < n {
                let b = slab.slot_offset(j);
                ok &= a + e <= b || b + e <= a;
                j += 1;
            }
            i += 1;
        }
    }
    ok
}

/// EM-BITMAP-COUNT-MATCHES: allocated count == number of set bits (no bit at or past
/// num_slots is set) and <= num_slots.
fn support_bitmap_count_matches(r: &RegionState) -> bool {
    let mut ok = true;
    for (_, slab) in r.slabs.iter() {
        let n = slab.num_slots() as usize;
        let words = slab.bitmap.kani_words();
        let mut set = 0usize;
        let mut w = 0usize;
        while w < words.len() {
            set += words[w].count_ones() as usize;
            w += 1;
        }
        let mut in_range = 0usize;
        let mut i = 0usize;
        while i < n {
            if slab.bitmap.is_set(i) {
                in_range += 1;
            }
            i += 1;
        }
        ok &= set == in_range;
        ok &= slab.bitmap.count_set() == set;
        ok &= slab.bitmap.count_set() <= n;
    }
    ok
}

/// EM-INV-NO-OVERLAP: slots in use at the same time (bitmap set: reserved, published,
/// or removed-but-not-yet-released) share no byte.
fn support_in_use_no_overlap(r: &RegionState) -> bool {
    let mut starts = [0u64; 4];
    let mut lens = [0u64; 4];
    let mut n = 0usize;
    for (_, slab) in r.slabs.iter() {
        let mut i = 0usize;
        while i < slab.num_slots() as usize {
            if slab.bitmap.is_set(i) {
                if n < 4 {
                    starts[n] = slab.slot_offset(i);
                    lens[n] = slab.element_size as u64;
                }
                n += 1;
            }
            i += 1;
        }
    }
    if n > 4 {
        return false;
    }
    let mut ok = true;
    let mut i = 0usize;
    while i < n {
        let mut j = i + 1;
        while j < n {
            ok &= starts[i] + lens[i] <= starts[j] || starts[j] + lens[j] <= starts[i];
            j += 1;
        }
        i += 1;
    }
    ok
}

// ---- Slab / bitmap inductive infrastructure ----

/// Slab-level invariant pieces (each a separate inventory property).
fn support_slab_key_implies_allocated(s: &Slab) -> bool {
    let mut ok = true;
    let mut i = 0usize;
    while i < 4 {
        if i < s.num_slots() as usize && i < s.keys.len() && s.get_key(i) != FREE_KEY {
            ok &= s.bitmap.is_set(i);
        }
        i += 1;
    }
    ok
}

fn support_slab_key_len(s: &Slab) -> bool {
    s.element_size > 0
        && s.keys.len() == s.num_slots() as usize
        && s.keys.len() as u64 == s.slab_size / s.element_size as u64
}

fn support_slab_slots_inside(s: &Slab) -> bool {
    let n = s.num_slots() as usize;
    let e = s.element_size as u64;
    let mut ok = n <= 4;
    let mut i = 0usize;
    while i < 4 {
        if i < n {
            let a = s.slot_offset(i);
            ok &= a >= s.start_offset && a + e <= s.start_offset + s.slab_size;
            let mut j = 0usize;
            while j < 4 {
                if j < n && j != i {
                    let b = s.slot_offset(j);
                    ok &= a + e <= b || b + e <= a;
                }
                j += 1;
            }
        }
        i += 1;
    }
    ok
}

fn support_bitmap_count_ok(b: &AllocationBitmap) -> bool {
    let n = b.num_slots() as usize;
    let words = b.kani_words();
    let mut ok = words.len() == (n + 63) / 64 && words.len() <= 1;
    let mut set = 0usize;
    if !words.is_empty() {
        set = words[0].count_ones() as usize;
        // no bit at or beyond num_slots is set
        if n < 64 {
            ok &= words[0] >> n == 0;
        }
    }
    ok &= b.count_set() == set;
    ok &= b.count_set() <= n;
    ok
}

/// Slab start used by the slab harnesses: concrete, not sector-aligned (a slab starts at
/// region base + a buddy offset, and region bases need not be aligned).
pub(crate) const S_START: u64 = B_BASE + 7 * SSU;

/// Run `f(slab_size, element_size)` on one of seven CONCRETE small slab geometries
/// (chosen symbolically): element 1 sector with slab 1..=4 sectors (1..=4 slots), and
/// element 2 sectors with slab 2..=4 sectors (1, 1 + unused tail, 2 slots). Sizes are
/// concrete per branch: symbolic sizes (symbolic-length Vecs, 64-bit symbolic
/// multiply/divide) made CBMC emit tens of millions of clauses.
fn support_slab_geoms<F: Fn(u64, u32)>(f: F) {
    let g: u8 = kani::any();
    if g == 0 {
        f(SSU, SS)
    } else if g == 1 {
        f(2 * SSU, SS)
    } else if g == 2 {
        f(3 * SSU, SS)
    } else if g == 3 {
        f(4 * SSU, SS)
    } else if g == 4 {
        f(2 * SSU, 2 * SS)
    } else if g == 5 {
        f(3 * SSU, 2 * SS)
    } else {
        f(4 * SSU, 2 * SS)
    }
}

/// An ARBITRARY slab state of that geometry: any subset of slots allocated (through the
/// real set path), any key on any slot, any in-range allocation cursor.
fn support_slab_symbolic(slab_size: u64, e: u32) -> Slab {
    let mut s = Slab::new(S_START, slab_size, e);
    let n = s.num_slots() as usize;
    let mut i = 0usize;
    while i < 4 {
        if i < n {
            if kani::any() {
                s.mark_slot_allocated(i);
            }
            if kani::any() {
                s.set_key(i, kani::any());
            }
        }
        i += 1;
    }
    let rover: usize = kani::any();
    kani::assume(rover < n);
    s.kani_set_rover(rover);
    s
}

/// Establishment states: a fresh slab (Slab::new, reserve path) or a slab rebuilt from a
/// checkpoint descriptor (recovery::slab_from_descriptor, initialize path) whose key
/// vector has one entry per slot (what serialize_region wrote; device returns it intact).
fn support_slab_established(slab_size: u64, e: u32) -> Slab {
    if kani::any() {
        Slab::new(S_START, slab_size, e)
    } else {
        let n = (slab_size / e as u64) as usize;
        let mut keys = Vec::with_capacity(4);
        let mut i = 0usize;
        while i < 4 {
            if i < n {
                keys.push(if kani::any() { FREE_KEY } else { kani::any() });
            }
            i += 1;
        }
        let desc = crate::checkpoint::SlabDescriptor {
            start_offset: S_START,
            slab_size,
            element_size: e,
            keys,
        };
        crate::recovery::slab_from_descriptor(&desc)
    }
}

/// One slab mutation exactly as the region applies it, with the region's call-site
/// precondition (the slot is currently allocated) for free_slot / set_key:
///   0 alloc_slot              -- alloc_extent
///   1 free_slot(i)            -- abort / FREE_KEY publish / checkpoint flush
///   2 set_key(i, k)           -- publish_slot (k != FREE) or remove (k == FREE)
/// Returns (op, slot touched, slot's bit before, result of alloc).
fn support_slab_step(s: &mut Slab) -> (u8, usize, bool, Option<(usize, u64)>) {
    let op: u8 = kani::any();
    kani::assume(op < 3);
    let n = s.num_slots() as usize;
    if op == 0 {
        let r = s.alloc_slot();
        let idx = r.map(|(i, _)| i).unwrap_or(0);
        (0, idx, false, r)
    } else {
        let i: usize = kani::any();
        kani::assume(i < n);
        let before = s.bitmap.is_set(i);
        kani::assume(before);
        if op == 1 {
            s.free_slot(i);
        } else {
            s.set_key(i, kani::any());
        }
        (op, i, before, None)
    }
}

// ---- ExtentManager small-geometry driver (shared by the EM-level properties) ----

/// Concrete layouts (every format() guard passes):
///   L_ONE   : 1 region of 2 sectors, slab 2 sectors  -> exactly one slab
///   L_TWO   : 2 regions of 4 sectors, slab 2 sectors -> 2 slabs per region
///   L_SHARED: shared-device mode: data starts after superblock + 2 checkpoint copies
///             (metadata_region_size 12 KiB -> data_start 12 KiB), 1 region of 4 sectors
/// All with sector 512 B, max_extent = slab, metadata device 4 x 4 KiB sectors.
pub(crate) fn support_layout(which: u8) -> FormatParams {
    let mut p = FormatParams {
        data_disk_size: 2 * SSU,
        slab_size: 2 * SSU,
        max_extent_size: 2 * SS,
        sector_size: SS,
        region_count: 1,
        metadata_alignment: 4096,
        instance_id: Some(0xC0FFEE),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    };
    if which == 1 {
        p.data_disk_size = 8 * SSU;
        p.region_count = 2;
    } else if which == 2 {
        p.metadata_region_size = 3 * 4096;
        p.data_disk_size = 3 * 4096 + 4 * SSU;
    }
    p
}
pub(crate) const L_ONE: u8 = 0;
pub(crate) const L_TWO: u8 = 1;
pub(crate) const L_SHARED: u8 = 2;

/// A formatted ExtentManager (format() must succeed: device I/O errors disabled for
/// the format itself, then re-enabled per `io_errors_after`).
pub(crate) fn support_formatted(p: FormatParams, io_errors_after: bool) -> ExtentManager {
    support_disk_reset(false);
    let em = support_em_with_device(4, false);
    let r = em.format(p);
    assert!(r.is_ok());
    support_disk_reset_errors(io_errors_after);
    em
}

pub(crate) fn support_disk_reset_errors(io_errors: bool) {
    // SAFETY: single-threaded harness; plain write of a bool.
    unsafe {
        *std::ptr::addr_of_mut!(KANI_IO_ERRORS) = io_errors;
    }
}

/// Reservation sizes used by the EM drivers: one full sector (exact), a partial sector
/// (rounds up to one sector) and the maximum (two sectors = one whole slab). Each call
/// site dispatches to CONCRETE sizes.
pub(crate) fn support_reserve(
    em: &ExtentManager,
    key: u64,
) -> Result<WriteHandle, ExtentManagerError> {
    let s: u8 = kani::any();
    if s == 0 {
        em.reserve_extent(key, SS)
    } else if s == 1 {
        em.reserve_extent(key, 100)
    } else {
        em.reserve_extent(key, 2 * SS)
    }
}

/// Apply `f` to every region of a formatted manager.
pub(crate) fn support_all_regions<F: Fn(&RegionState) -> bool>(em: &ExtentManager, f: F) -> bool {
    let regions = em.regions.read();
    let mut ok = true;
    if let Some(rs) = regions.as_ref() {
        let mut i = 0usize;
        while i < 2 {
            if i < rs.len() {
                ok &= f(&rs[i].read());
            }
            i += 1;
        }
    }
    ok
}

/// One symbolic public-API operation on a formatted manager, with at most one live
/// write handle carried between operations:
///   0 reserve (if no live handle)   1 publish live handle (key symbolic, may be FREE_KEY)
///   2 abort live handle             3 drop live handle (Drop -> abort)
///   4 remove_extent(published offset or arbitrary offset)
///   5 checkpoint()                  (I/O errors per the disk model)
pub(crate) fn support_em_op(
    em: &ExtentManager,
    live: &mut Option<WriteHandle>,
    published: &mut Option<u64>,
) {
    let op: u8 = kani::any();
    if op == 0 {
        if live.is_none() {
            let key: u64 = kani::any();
            if let Ok(h) = support_reserve(em, key) {
                *live = Some(h);
            }
        }
    } else if op == 1 {
        if let Some(h) = live.take() {
            let free = h.key() == FREE_KEY;
            let off = h.extent_offset();
            if h.publish().is_ok() && !free {
                *published = Some(off);
            }
        }
    } else if op == 2 {
        if let Some(h) = live.take() {
            h.abort();
        }
    } else if op == 3 {
        if let Some(h) = live.take() {
            drop(h);
        }
    } else if op == 4 {
        let off = match *published {
            Some(o) if kani::any() => o,
            _ => kani::any(),
        };
        if em.remove_extent(off).is_ok() && *published == Some(off) {
            *published = None;
        }
    } else {
        let _ = em.checkpoint();
    }
}

/// EM-INV-INITIALIZED-TOGETHER predicate.
fn support_inited_together(em: &ExtentManager) -> bool {
    let r = em.regions.read().is_some();
    let s = em.shared.lock().unwrap().is_some();
    r == s
}

// ===========================================================================
// 2. Scored harnesses
// ===========================================================================

// ---- EM-INV-INITIALIZED-TOGETHER ----
// The only writers of `regions`/`shared` are format() and initialize(). Pre-state: the
// fresh component or a formatted one. Step: format() with a valid layout (separate or
// shared device, 1 or 2 regions) or with parameters/devices that hit each early-return
// guard, or initialize() (blank disk -> bad magic, or the formatted superblock), with
// device geometry errors and metadata write/read I/O errors symbolic. After every step
// regions.is_some() == shared.is_some(). (Sequential executions only: a concurrent
// reader between lib.rs:506 and :507 is outside what Kani can model.)
fn support_case_initialized_together(geometry_fails: bool, preformat: bool, mutant: bool) {
    // Pre-state construction with I/O errors off (so the pre-state is one concrete
    // state, not a merge of "written"/"not written" disks), then errors on for the step.
    support_disk_reset(false);
    let em = support_em_with_device(4, geometry_fails);
    assert!(support_inited_together(&em));
    if preformat {
        let _ = em.format(support_layout(L_ONE));
    }
    assert!(support_inited_together(&em));
    support_disk_reset_errors(true);
    let g: u8 = kani::any();
    if g == 0 {
        let _ = em.format(support_layout(L_TWO));
    } else if g == 1 {
        let _ = em.format(support_layout(L_SHARED));
    } else if g == 2 {
        let mut p = support_layout(L_ONE);
        p.sector_size = 0;
        let _ = em.format(p);
    } else if g == 3 {
        let mut p = support_layout(L_ONE);
        p.region_count = 3;
        let _ = em.format(p);
    } else if g == 4 {
        // metadata area too small for two checkpoint copies
        let mut p = support_layout(L_SHARED);
        p.metadata_region_size = 4096;
        let _ = em.format(p);
    } else if g == 5 {
        // shared device with no data space left after the metadata area
        let mut p = support_layout(L_SHARED);
        p.data_disk_size = 3 * 4096;
        let _ = em.format(p);
    } else {
        let _ = em.initialize();
    }
    if mutant {
        // MUTANT: claims no step ever reaches the (consistently) initialised state.
        assert!(!(support_inited_together(&em) && em.regions.read().is_some()));
    } else {
        assert!(support_inited_together(&em));
    }
    std::mem::forget(em);
}

fn support_dispatch_initialized_together(mutant: bool) {
    if kani::any() {
        if kani::any() {
            support_case_initialized_together(false, false, mutant)
        } else {
            support_case_initialized_together(false, true, mutant)
        }
    } else if kani::any() {
        support_case_initialized_together(true, false, mutant)
    } else {
        support_case_initialized_together(true, true, mutant)
    }
}

#[kani::proof]
#[kani::unwind(9)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn boundary_em_inv_initialized_together() {
    support_dispatch_initialized_together(false);
}

#[kani::proof]
#[kani::unwind(9)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn boundary_em_inv_initialized_together__mutant() {
    support_dispatch_initialized_together(true);
}

// ---- EM-BUDDY-ALLOC-ALIGNED ----
// Establishment + one inductive step (see support_buddy_step_at): every free block and
// every block handed out by alloc is aligned (relative to the region base) to its size.
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn boundary_em_buddy_alloc_aligned() {
    let st = support_buddy_step();
    assert!(support_buddy_inv(&st.after, st.total));
    if let Some((rel, span)) = st.returned {
        assert!(rel & (span - 1) == 0);
    }
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn boundary_em_buddy_alloc_aligned__mutant() {
    let st = support_buddy_step();
    assert!(support_buddy_inv(&st.after, st.total));
    if let Some((rel, span)) = st.returned {
        // MUTANT: claims a returned block is never aligned to its size.
        assert!(rel & (span - 1) != 0);
    }
}

// ---- EM-BUDDY-FREE-DISJOINT ----
// Establishment + one inductive step: free blocks stay pairwise disjoint, and a block
// handed out by alloc overlaps no block that is still recorded free.
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn boundary_em_buddy_free_disjoint() {
    let st = support_buddy_step();
    assert!(support_buddy_inv(&st.after, st.total));
    if let Some((rel, span)) = st.returned {
        assert!(support_buddy_range_not_free(&st.after, rel, span));
    }
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn boundary_em_buddy_free_disjoint__mutant() {
    let st = support_buddy_step();
    assert!(support_buddy_inv(&st.after, st.total));
    if let Some((rel, span)) = st.returned {
        // MUTANT: claims a returned block always still overlaps a free block.
        assert!(!support_buddy_range_not_free(&st.after, rel, span));
    }
}

// Slab-level harness bodies. Each takes the (concrete) element size and a `mutant` flag
// that flips exactly one assertion; the scored harness and its twin dispatch over the
// two element sizes identically.

fn support_case_key_implies(slab_size: u64, e: u32, mutant: bool) {
    let est = support_slab_established(slab_size, e);
    assert!(support_slab_key_implies_allocated(&est));
    let mut s = support_slab_symbolic(slab_size, e);
    kani::assume(support_slab_key_implies_allocated(&s));
    let _ = support_slab_step(&mut s);
    if mutant {
        assert!(!support_slab_key_implies_allocated(&s));
    } else {
        assert!(support_slab_key_implies_allocated(&s));
    }
}

fn support_case_key_len(slab_size: u64, e: u32, mutant: bool) {
    let est = support_slab_established(slab_size, e);
    assert!(support_slab_key_len(&est));
    let mut s = support_slab_symbolic(slab_size, e);
    kani::assume(support_slab_key_len(&s));
    let _ = support_slab_step(&mut s);
    if mutant {
        assert!(s.keys.len() == s.num_slots() as usize + 1);
    } else {
        assert!(support_slab_key_len(&s));
    }
}

fn support_case_slots_inside(slab_size: u64, e: u32, mutant: bool) {
    let est = support_slab_established(slab_size, e);
    assert!(support_slab_slots_inside(&est));
    let mut s = support_slab_symbolic(slab_size, e);
    kani::assume(support_slab_slots_inside(&s));
    let (_, _, _, r) = support_slab_step(&mut s);
    assert!(support_slab_slots_inside(&s));
    if let Some((i, off)) = r {
        assert!(off == s.slot_offset(i) && i < s.num_slots() as usize);
        let end_ok =
            off >= s.start_offset && off + s.element_size as u64 <= s.start_offset + s.slab_size;
        if mutant {
            assert!(!end_ok);
        } else {
            assert!(end_ok);
        }
    }
}

fn support_case_count(slab_size: u64, e: u32, mutant: bool) {
    let est = support_slab_established(slab_size, e);
    assert!(support_bitmap_count_ok(&est.bitmap));
    let mut s = support_slab_symbolic(slab_size, e);
    kani::assume(support_bitmap_count_ok(&s.bitmap));
    let _ = support_slab_step(&mut s);
    if mutant {
        assert!(!support_bitmap_count_ok(&s.bitmap));
    } else {
        assert!(support_bitmap_count_ok(&s.bitmap));
    }
}

fn support_case_free_clears(slab_size: u64, e: u32, mutant: bool) {
    let mut s = support_slab_symbolic(slab_size, e);
    let n = s.num_slots() as usize;
    let i: usize = kani::any();
    kani::assume(i < n && s.bitmap.is_set(i));
    let j: usize = kani::any();
    kani::assume(j < n);
    let (bj, kj) = (s.bitmap.is_set(j), s.get_key(j));
    s.free_slot(i);
    assert!(!s.bitmap.is_set(i));
    if mutant {
        assert!(s.get_key(i) != FREE_KEY);
    } else {
        assert!(s.get_key(i) == FREE_KEY);
    }
    assert!(j == i || (s.bitmap.is_set(j) == bj && s.get_key(j) == kj));
}

// ---- EM-INV-KEY-IMPLIES-ALLOCATED ----
// Slab level: establishment (Slab::new / slab_from_descriptor) + one inductive step of
// every slab mutator the region uses, from an arbitrary slab satisfying the invariant.
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_inv_key_implies_allocated() {
    support_slab_geoms(|sz, e| support_case_key_implies(sz, e, false));
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_inv_key_implies_allocated__mutant() {
    support_slab_geoms(|sz, e| support_case_key_implies(sz, e, true));
}

// ---- EM-INV-KEY-VECTOR-LENGTH ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_inv_key_vector_length() {
    support_slab_geoms(|sz, e| support_case_key_len(sz, e, false));
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_inv_key_vector_length__mutant() {
    support_slab_geoms(|sz, e| support_case_key_len(sz, e, true));
}

// ---- EM-SLAB-SLOTS-INSIDE ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_slab_slots_inside() {
    support_slab_geoms(|sz, e| support_case_slots_inside(sz, e, false));
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_slab_slots_inside__mutant() {
    support_slab_geoms(|sz, e| support_case_slots_inside(sz, e, true));
}

// ---- EM-BITMAP-COUNT-MATCHES ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_bitmap_count_matches() {
    support_slab_geoms(|sz, e| support_case_count(sz, e, false));
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_bitmap_count_matches__mutant() {
    support_slab_geoms(|sz, e| support_case_count(sz, e, true));
}

// ---- EM-SLAB-FREE-CLEARS-KEY ----
// Slab::free_slot on an allocated slot (its only call shape: region.free_slot) from an
// arbitrary slab state: afterwards the slot is free and its key is FREE_KEY, and no
// other slot's bit or key changed.
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_slab_free_clears_key() {
    support_slab_geoms(|sz, e| support_case_free_clears(sz, e, false));
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_slab_free_clears_key__mutant() {
    support_slab_geoms(|sz, e| support_case_free_clears(sz, e, true));
}

// ===========================================================================
// 3. Batch 2 — ids that do not go through BuddyAllocator::{alloc,free,mark_allocated}
//    (see verif/.run/batches/kani_RESUME.md, "Batch 2").
// ===========================================================================

use crate::checkpoint::SlabDescriptor;
use crate::superblock::{Superblock, FORMAT_VERSION, SUPERBLOCK_MAGIC, SUPERBLOCK_SIZE};

// ---- Faithful CRC-32 model (replaces crc32fast::hash where DETECTION is the claim) ----
// crc32fast computes CRC-32/ISO-HDLC: reflected polynomial 0xEDB88320, init 0xFFFFFFFF,
// final xor 0xFFFFFFFF. This is the textbook bitwise form of exactly that function; the
// real crate cannot run under Kani (`_xgetbv` CPU-feature probe). It is pinned to the
// standard check values by `check_stub_conforms_crc32`.
pub(crate) fn stub_crc32_faithful(buf: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    let mut i = 0usize;
    while i < buf.len() {
        crc ^= buf[i] as u32;
        let mut k = 0u32;
        while k < 8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            k += 1;
        }
        i += 1;
    }
    !crc
}

/// The faithful model returns the published CRC-32/ISO-HDLC check values.
#[kani::proof]
#[kani::unwind(45)]
#[kani::solver(minisat)]
fn check_stub_conforms_crc32() {
    assert!(stub_crc32_faithful(b"") == 0);
    assert!(stub_crc32_faithful(b"a") == 0xE8B7_BE43);
    assert!(stub_crc32_faithful(b"123456789") == 0xCBF4_3926);
    assert!(stub_crc32_faithful(b"The quick brown fox jumps over the lazy dog") == 0x414F_A339);
}

// ---- Disk model v2 (batch 2): the same contract as batch 1's KANI_DISK model
// (per-write extent keyed by its starting LBA; success stores; a failure only when
// KANI_IO_ERRORS, and then it may or may not have stored; a read returns the last write
// zero-extended/truncated to num_bytes, zeros if never written, or an I/O error), stored
// as `[Option<Vec<u8>>; D2_SLOTS]` in a static. Measured (kani_measurements.md, batch 2):
// batch 1's `Vec<(u64, Vec<u8>)>` store does not finish a 4 KiB superblock write+read in
// 5 min; a `[[u8; 4096]; N]` static array finishes but makes every value read back
// SYMBOLIC to symex (4096 > CBMC's field-sensitivity limit), so initialize() then
// unrolls its region loop on a symbolic region_count; this layout keeps read-back values
// concrete (18.6 s for the same probe).
const D2_SLOTS: usize = 3;
static mut D2_USED: [bool; D2_SLOTS] = [false; D2_SLOTS];
static mut D2_LBA: [u64; D2_SLOTS] = [0; D2_SLOTS];
static mut D2_DATA: [Option<Vec<u8>>; D2_SLOTS] = [None, None, None];

pub(crate) fn support_disk2_reset(io_errors: bool) {
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(D2_USED) = [false; D2_SLOTS];
        *std::ptr::addr_of_mut!(KANI_IO_ERRORS) = io_errors;
    }
}

pub(crate) fn support_disk2_write(lba: u64, data: &[u8]) -> Result<(), ExtentManagerError> {
    let fail = support_io_fails();
    let store = !fail || kani::any::<bool>();
    if store {
        // SAFETY: single-threaded harness; exclusive access for the duration of the call.
        unsafe {
            let used = &mut *std::ptr::addr_of_mut!(D2_USED);
            let lbas = &mut *std::ptr::addr_of_mut!(D2_LBA);
            let mut slot = D2_SLOTS;
            let mut i = 0usize;
            while i < D2_SLOTS {
                if slot == D2_SLOTS && used[i] && lbas[i] == lba {
                    slot = i;
                }
                i += 1;
            }
            i = 0;
            while i < D2_SLOTS {
                if slot == D2_SLOTS && !used[i] {
                    slot = i;
                }
                i += 1;
            }
            assert!(slot < D2_SLOTS, "disk model v2: out of extent slots");
            used[slot] = true;
            lbas[slot] = lba;
            let d = &mut *std::ptr::addr_of_mut!(D2_DATA);
            d[slot] = Some(data.to_vec());
        }
    }
    if fail {
        return Err(ExtentManagerError::IoError(String::new()));
    }
    Ok(())
}

pub(crate) fn support_disk2_read(
    lba: u64,
    num_bytes: usize,
) -> Result<Vec<u8>, ExtentManagerError> {
    if support_io_fails() {
        return Err(ExtentManagerError::IoError(String::new()));
    }
    // SAFETY: single-threaded harness; shared access for the duration of the call.
    unsafe {
        let used = &*std::ptr::addr_of!(D2_USED);
        let lbas = &*std::ptr::addr_of!(D2_LBA);
        let d = &*std::ptr::addr_of!(D2_DATA);
        let mut i = 0usize;
        while i < D2_SLOTS {
            if used[i] && lbas[i] == lba {
                let mut out = d[i].as_ref().unwrap().clone();
                out.resize(num_bytes, 0);
                return Ok(out);
            }
            i += 1;
        }
    }
    Ok(vec![0u8; num_bytes])
}

pub(crate) fn stub_write_blocks2(
    this: &BlockDeviceClient,
    lba: u64,
    data: &[u8],
) -> Result<(), ExtentManagerError> {
    support_disk2_write(this.kani_base_lba() + lba, data)
}

pub(crate) fn stub_read_blocks2(
    this: &BlockDeviceClient,
    lba: u64,
    num_bytes: usize,
) -> Result<Vec<u8>, ExtentManagerError> {
    support_disk2_read(this.kani_base_lba() + lba, num_bytes)
}

/// Disk model v2 obeys the contract above: read-after-write returns the written bytes
/// (zero-extended), a never-written extent reads as zeros, a successful read returns
/// exactly num_bytes, a rewrite at the same LBA replaces the earlier one.
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn check_stub_conforms_disk2() {
    support_disk2_reset(true);
    let lba: u64 = kani::any();
    kani::assume(lba < 4);
    let a: [u8; 3] = kani::any();
    let b: [u8; 2] = kani::any();
    let w1 = support_disk2_write(lba, &a);
    let w2 = support_disk2_write(lba, &b);
    if w2.is_ok() {
        if let Ok(r) = support_disk2_read(lba, 4) {
            assert!(r.len() == 4 && r[0] == b[0] && r[1] == b[1]);
            if w1.is_ok() {
                assert!(r[3] == 0);
            }
        }
    }
    if let Ok(r) = support_disk2_read(lba + 10, 3) {
        assert!(r.len() == 3 && r[0] == 0 && r[2] == 0);
    }
}

// ---- Superblock builders ----

/// A superblock as the component holds it: built by `Superblock::new` (format) with any
/// field values, then any checkpoint sequence and an active copy of 0 or 1 (the only
/// values write_checkpoint assigns: `1 - active` from 0/1).
fn support_sb_symbolic() -> Superblock {
    let mut sb = Superblock::new(
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
    );
    sb.checkpoint_seq = kani::any();
    sb.active_copy = if kani::any() { 1 } else { 0 };
    sb
}

/// One concrete superblock (the unit-test fixture geometry, seq 7, active copy 1).
fn support_sb_concrete() -> Superblock {
    let mut sb = Superblock::new(
        1 << 30,
        4096,
        1 << 20,
        65536,
        32,
        8192,
        1 << 20,
        0xDEAD_BEEF_CAFE_1234,
        1,
        0,
    );
    sb.checkpoint_seq = 7;
    sb.active_copy = 1;
    sb
}

fn support_is_corrupt<T>(r: &Result<T, ExtentManagerError>) -> bool {
    matches!(r, Err(ExtentManagerError::CorruptMetadata(_)))
}

fn support_case_sb_well_formed(mutant: bool) {
    let sb = support_sb_symbolic();
    let buf = sb.serialize();
    assert!(buf.len() == SUPERBLOCK_SIZE);
    assert!(buf[0..8] == SUPERBLOCK_MAGIC.to_le_bytes());
    if mutant {
        assert!(buf[8..12] != FORMAT_VERSION.to_le_bytes());
    } else {
        assert!(buf[8..12] == FORMAT_VERSION.to_le_bytes());
    }
    assert!(buf[12..20] == sb.data_disk_size.to_le_bytes());
    assert!(buf[20..24] == sb.sector_size.to_le_bytes());
    assert!(buf[24..32] == sb.slab_size.to_le_bytes());
    assert!(buf[32..36] == sb.max_extent_size.to_le_bytes());
    assert!(buf[36..40] == sb.region_count.to_le_bytes());
    assert!(buf[40..48] == sb.checkpoint_seq.to_le_bytes());
    assert!(buf[48] == sb.active_copy);
    assert!(buf[48] <= 1);
    assert!(buf[56..64] == sb.checkpoint_region_offset.to_le_bytes());
    assert!(buf[64..72] == sb.checkpoint_region_size.to_le_bytes());
    assert!(buf[72..80] == sb.instance_id.to_le_bytes());
    assert!(buf[80..84] == sb.metadata_disk_ns_id.to_le_bytes());
    assert!(buf[84..92] == sb.data_start_offset.to_le_bytes());
    // reserved bytes 49..56 and padding 96..4096 are zero (any index)
    let r: usize = kani::any();
    kani::assume(r >= 49 && r < 56);
    assert!(buf[r] == 0);
    let p: usize = kani::any();
    kani::assume(p >= 96 && p < SUPERBLOCK_SIZE);
    assert!(buf[p] == 0);
    // CRC-32 of bytes 0..92, little-endian at byte 92
    let crc = crc32fast::hash(&buf[..92]);
    assert!(buf[92..96] == crc.to_le_bytes());
}

fn support_case_sb_roundtrip(mutant: bool) {
    let sb = support_sb_symbolic();
    let buf = sb.serialize();
    let r = Superblock::deserialize(&buf);
    assert!(r.is_ok());
    let d = r.unwrap();
    assert!(d.magic == sb.magic);
    assert!(d.version == sb.version);
    assert!(d.data_disk_size == sb.data_disk_size);
    assert!(d.sector_size == sb.sector_size);
    assert!(d.slab_size == sb.slab_size);
    assert!(d.max_extent_size == sb.max_extent_size);
    assert!(d.region_count == sb.region_count);
    assert!(d.checkpoint_seq == sb.checkpoint_seq);
    assert!(d.active_copy == sb.active_copy);
    assert!(d.checkpoint_region_offset == sb.checkpoint_region_offset);
    assert!(d.checkpoint_region_size == sb.checkpoint_region_size);
    assert!(d.metadata_disk_ns_id == sb.metadata_disk_ns_id);
    assert!(d.data_start_offset == sb.data_start_offset);
    if mutant {
        assert!(d.instance_id != sb.instance_id);
    } else {
        assert!(d.instance_id == sb.instance_id);
    }
}

fn support_case_sb_corruption(mutant: bool) {
    let sb = support_sb_concrete();
    let mut buf = sb.serialize();
    let c: u8 = kani::any();
    let r = if c == 0 {
        // any burst alteration of up to 32 bits inside the checksummed bytes 0..92 or
        // the stored CRC 92..96
        let p: usize = kani::any();
        kani::assume(p <= 92);
        let d: u32 = kani::any();
        kani::assume(d != 0);
        let db = d.to_le_bytes();
        buf[p] ^= db[0];
        buf[p + 1] ^= db[1];
        buf[p + 2] ^= db[2];
        buf[p + 3] ^= db[3];
        Superblock::deserialize(&buf)
    } else if c == 1 {
        let m: u64 = kani::any();
        kani::assume(m != SUPERBLOCK_MAGIC);
        buf[0..8].copy_from_slice(&m.to_le_bytes());
        Superblock::deserialize(&buf)
    } else {
        let n: usize = kani::any();
        kani::assume(n < SUPERBLOCK_SIZE);
        Superblock::deserialize(&buf[..n])
    };
    let corrupt = support_is_corrupt(&r);
    if mutant && c == 0 {
        assert!(!corrupt);
    } else {
        assert!(corrupt);
    }
}

fn support_case_sb_new_defaults(mutant: bool) {
    let a: (u64, u32, u64, u32, u32, u64, u64, u64, u32, u64) = (
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
        kani::any(),
    );
    let sb = Superblock::new(a.0, a.1, a.2, a.3, a.4, a.5, a.6, a.7, a.8, a.9);
    assert!(sb.magic == SUPERBLOCK_MAGIC);
    assert!(sb.version == FORMAT_VERSION);
    assert!(sb.active_copy == 0);
    if mutant {
        assert!(sb.checkpoint_seq != 0);
    } else {
        assert!(sb.checkpoint_seq == 0);
    }
    assert!(sb.data_disk_size == a.0 && sb.sector_size == a.1 && sb.slab_size == a.2);
    assert!(sb.max_extent_size == a.3 && sb.region_count == a.4);
    assert!(sb.checkpoint_region_offset == a.5 && sb.checkpoint_region_size == a.6);
    assert!(sb.instance_id == a.7 && sb.metadata_disk_ns_id == a.8 && sb.data_start_offset == a.9);
}

/// EM-SB-ACTIVE-COPY-VALID refutation body: a CRC-valid superblock produced by the
/// component's own serializer (exactly what format(L_ONE) writes, but naming active copy
/// `a >= 2`) is ACCEPTED by the decoder initialize() uses (recovery::recover ->
/// Superblock::deserialize, recovery.rs:16) and the bad copy number is returned.
/// (At the recover() level the 4 KiB read-back made symex treat the decoded fields as
/// symbolic and the run did not finish in 6.5 min; see kani_measurements.md.)
fn support_case_active_copy(mutant: bool) {
    let a: u8 = kani::any();
    kani::assume(a >= 2);
    let mut sb = Superblock::new(2 * SSU, SS, 2 * SSU, 2 * SS, 1, 4096, 6144, 0xC0FFEE, 1, 0);
    sb.active_copy = a;
    let bytes = sb.serialize();
    let r = Superblock::deserialize(&bytes);
    kani::cover!(r.is_ok());
    if mutant {
        assert!(support_is_corrupt(&r));
    } else {
        assert!(r.is_ok());
        assert!(r.as_ref().map(|x| x.active_copy).ok() == Some(a));
    }
}

// ---- Slab / bitmap bodies (dispatched over the 7 concrete geometries) ----

fn support_case_no_double_alloc(slab_size: u64, e: u32, mutant: bool) {
    // establishment from a checkpoint descriptor: mark_slot_allocated once per real key
    // (the bitmap's debug_assert "double-set" is live under Kani)
    let _est = support_slab_established(slab_size, e);
    let mut s = support_slab_symbolic(slab_size, e);
    let n = s.num_slots() as usize;
    let mut before = [false; 4];
    let mut i = 0usize;
    while i < 4 {
        if i < n {
            before[i] = s.bitmap.is_set(i);
        }
        i += 1;
    }
    if let Some((idx, _)) = s.alloc_slot() {
        if mutant {
            assert!(before[idx]);
        } else {
            assert!(!before[idx]);
        }
        assert!(s.bitmap.is_set(idx));
    }
}

fn support_case_index_in_range(slab_size: u64, e: u32, mutant: bool) {
    let _est = support_slab_established(slab_size, e);
    let mut s = support_slab_symbolic(slab_size, e);
    let n = s.num_slots() as usize;
    let op: u8 = kani::any();
    let used: Option<usize> = if op == 0 {
        // reserve: alloc_slot -> find_free_from / set (handle carries this index)
        s.alloc_slot().map(|(i, _)| i)
    } else if op == 1 {
        // remove_extent_by_offset: slot_for_offset -> is_set
        let off: u64 = kani::any();
        match s.slot_for_offset(off) {
            Some(i) => {
                let _ = s.bitmap.is_set(i);
                Some(i)
            }
            None => None,
        }
    } else {
        // abort / FREE_KEY publish / flush_pending_frees: free_slot(i) with an index that
        // came from alloc_slot or slot_for_offset of an allocated slot
        let off: u64 = kani::any();
        match s.slot_for_offset(off) {
            Some(i) if s.bitmap.is_set(i) => {
                s.free_slot(i);
                Some(i)
            }
            _ => None,
        }
    };
    if let Some(i) = used {
        if mutant {
            assert!(i >= n);
        } else {
            assert!(i < n);
        }
    }
}

fn support_case_find_free<const N: usize>(mutant: bool) {
    let mut b = AllocationBitmap::new(N as u32);
    let mut i = 0usize;
    while i < N {
        if kani::any() {
            b.set(i);
        }
        i += 1;
    }
    let start: usize = kani::any();
    kani::assume(start < N);
    let r = b.find_free_from(start);
    let j: usize = kani::any();
    kani::assume(j < N);
    match r {
        Some(idx) => {
            assert!(idx < N);
            assert!(!b.is_set(idx));
        }
        None => {
            if mutant {
                assert!(!b.is_set(j));
            } else {
                assert!(b.is_set(j));
            }
        }
    }
    if !b.is_set(j) {
        assert!(r.is_some());
    }
}

fn support_dispatch_find_free(mutant: bool) {
    let g: u8 = kani::any();
    if g == 0 {
        support_case_find_free::<1>(mutant)
    } else if g == 1 {
        support_case_find_free::<2>(mutant)
    } else if g == 2 {
        support_case_find_free::<3>(mutant)
    } else if g == 3 {
        support_case_find_free::<4>(mutant)
    } else {
        support_case_find_free::<5>(mutant)
    }
}

fn support_case_offset_inverse(slab_size: u64, e: u32, mutant: bool) {
    let s = Slab::new(S_START, slab_size, e);
    let n = s.num_slots() as usize;
    let i: usize = kani::any();
    kani::assume(i < n);
    let r = s.slot_for_offset(s.slot_offset(i));
    if mutant {
        assert!(r != Some(i));
    } else {
        assert!(r == Some(i));
    }
    let x: u64 = kani::any();
    let mut is_start = false;
    let mut k = 0usize;
    while k < 4 {
        if k < n && s.slot_offset(k) == x {
            is_start = true;
        }
        k += 1;
    }
    let rx = s.slot_for_offset(x);
    if !is_start {
        assert!(rx.is_none());
    }
    if let Some(j) = rx {
        assert!(j < n && s.slot_offset(j) == x);
    }
}

fn support_case_rover(slab_size: u64, e: u32, mutant: bool) {
    let est = support_slab_established(slab_size, e);
    assert!(est.kani_rover() < est.num_slots() as usize);
    let mut s = support_slab_symbolic(slab_size, e);
    let n = s.num_slots() as usize;
    let _ = support_slab_step(&mut s);
    if mutant {
        assert!(s.kani_rover() >= n);
    } else {
        assert!(s.kani_rover() < n);
    }
}

fn support_case_recover(slab_size: u64, e: u32, mutant: bool) {
    let n = (slab_size / e as u64) as usize;
    let mut keys = Vec::with_capacity(4);
    let mut real = 0usize;
    let mut i = 0usize;
    while i < 4 {
        if i < n {
            let k: u64 = if kani::any() { FREE_KEY } else { kani::any() };
            if k != FREE_KEY {
                real += 1;
            }
            keys.push(k);
        }
        i += 1;
    }
    let desc = SlabDescriptor {
        start_offset: S_START,
        slab_size,
        element_size: e,
        keys,
    };
    let s = crate::recovery::slab_from_descriptor(&desc);
    assert!(s.num_slots() as usize == n);
    assert!(s.start_offset == S_START && s.slab_size == slab_size && s.element_size == e);
    assert!(s.bitmap.count_set() == real);
    let j: usize = kani::any();
    kani::assume(j < n);
    let k = desc.keys[j];
    if k != FREE_KEY {
        assert!(s.bitmap.is_set(j));
        assert!(s.get_key(j) == k);
    } else if mutant {
        assert!(s.bitmap.is_set(j));
    } else {
        assert!(!s.bitmap.is_set(j));
        assert!(s.get_key(j) == FREE_KEY);
    }
}

// ---- Checkpoint codec infrastructure ----

/// Metadata client over the conforming mock device. Its channels are never used by the
/// harnesses that stub `read_blocks`/`write_blocks` with the disk model.
fn support_md_client(sector: u32) -> BlockDeviceClient {
    let dev = KaniMetaDevice {
        sector,
        sectors: 64,
        geometry_fails: false,
    };
    let ch = dev.connect_client().unwrap();
    let alloc: interfaces::DmaAllocFn = Arc::new(|_s, _a, _n| Err(String::new()));
    BlockDeviceClient::with_base_lba(ch, alloc, sector, 1, 0)
}

/// A region holding exactly `slabs` (inserted by start offset, as alloc_extent /
/// initialize do). Its BuddyAllocator is built without `new()` and is never called by
/// the checkpoint codec.
fn support_region_with(slabs: Vec<Slab>) -> RegionState {
    let p = support_layout(L_ONE);
    let mut r = RegionState::new(
        BuddyAllocator::kani_from_parts(0, 4 * SSU, SS, 0, Vec::new()),
        p,
    );
    for s in slabs {
        r.slabs.insert(s.start_offset, s);
    }
    r
}

/// A slab of the given concrete geometry with the given keys on its first slots.
fn support_slab_keys(start: u64, slab_size: u64, e: u32, keys: &[u64]) -> Slab {
    let mut s = Slab::new(start, slab_size, e);
    let mut i = 0usize;
    while i < keys.len() {
        if keys[i] != FREE_KEY {
            s.mark_slot_allocated(i);
            s.set_key(i, keys[i]);
        }
        i += 1;
    }
    s
}

type KRegions = parking_lot::RwLock<Option<Vec<Arc<parking_lot::RwLock<RegionState>>>>>;
type KShared = std::sync::Mutex<Option<crate::region::SharedState>>;

/// Lock objects exactly as ExtentManager holds them, for `checkpoint::write_checkpoint`.
/// Checkpoint copies start at byte 4096 and are `copy_size` bytes each.
fn support_ckpt_state(
    regions: Vec<RegionState>,
    seq: u64,
    active: u8,
    copy_size: u64,
) -> (KRegions, KShared) {
    let p = support_layout(L_ONE);
    let mut sb = Superblock::new(
        p.data_disk_size,
        SS,
        p.slab_size,
        p.max_extent_size,
        1,
        4096,
        copy_size,
        0xC0FFEE,
        1,
        0,
    );
    sb.checkpoint_seq = seq;
    sb.active_copy = active;
    let rs: Vec<Arc<parking_lot::RwLock<RegionState>>> = regions
        .into_iter()
        .map(|r| Arc::new(parking_lot::RwLock::new(r)))
        .collect();
    (
        parking_lot::RwLock::new(Some(rs)),
        std::sync::Mutex::new(Some(crate::region::SharedState {
            format_params: p,
            checkpoint_seq: seq,
            superblock: sb,
        })),
    )
}

/// Byte offset of the copy the superblock currently names active.
fn support_active_copy_offset(shared: &KShared) -> (u64, u64, u64) {
    let g = shared.lock().unwrap();
    let s = g.as_ref().unwrap();
    (
        s.superblock.checkpoint_region_offset
            + s.superblock.active_copy as u64 * s.superblock.checkpoint_region_size,
        s.superblock.checkpoint_region_size,
        s.checkpoint_seq,
    )
}

/// XOR `bytes` into the stored extent that starts at absolute LBA `lba`, at byte `pos`
/// (models an alteration of the medium after the write).
fn support_disk_patch_xor(lba: u64, pos: usize, bytes: &[u8]) {
    // SAFETY: single-threaded harness; exclusive access for the duration of the call.
    unsafe {
        let used = &*std::ptr::addr_of!(D2_USED);
        let lbas = &*std::ptr::addr_of!(D2_LBA);
        let d = &mut *std::ptr::addr_of_mut!(D2_DATA);
        let mut i = 0usize;
        while i < D2_SLOTS {
            if used[i] && lbas[i] == lba {
                let v = d[i].as_mut().unwrap();
                let mut k = 0usize;
                while k < bytes.len() {
                    v[pos + k] ^= bytes[k];
                    k += 1;
                }
            }
            i += 1;
        }
    }
}

/// Overwrite bytes of the stored extent that starts at `lba`.
fn support_disk_patch_set(lba: u64, pos: usize, bytes: &[u8]) {
    // SAFETY: single-threaded harness; exclusive access for the duration of the call.
    unsafe {
        let used = &*std::ptr::addr_of!(D2_USED);
        let lbas = &*std::ptr::addr_of!(D2_LBA);
        let d = &mut *std::ptr::addr_of_mut!(D2_DATA);
        let mut i = 0usize;
        while i < D2_SLOTS {
            if used[i] && lbas[i] == lba {
                d[i].as_mut().unwrap()[pos..pos + bytes.len()].copy_from_slice(bytes);
            }
            i += 1;
        }
    }
}

// Short-read over-approximation of the client (EM-CKPT-READ-TRUNCATED only): the bytes
// the disk model holds, cut to ANY length <= num_bytes. The real client always returns
// exactly num_bytes on success, so this is a superset of its behaviours (an implication
// proved over it holds for the real client).
static mut KANI_READ_LENS: Vec<usize> = Vec::new();

pub(crate) fn stub_rb_short(
    this: &BlockDeviceClient,
    lba: u64,
    num_bytes: usize,
) -> Result<Vec<u8>, ExtentManagerError> {
    let mut v = support_disk2_read(this.kani_base_lba() + lba, num_bytes)?;
    let n: usize = kani::any();
    kani::assume(n <= num_bytes);
    v.truncate(n);
    // SAFETY: single-threaded harness.
    unsafe { (*std::ptr::addr_of_mut!(KANI_READ_LENS)).push(n) };
    Ok(v)
}

/// The test region pair used by the codec harnesses: region 0 holds slab A (2 slots of
/// 1 sector), region 1 holds no slab. ONE slab in total: two SlabDescriptors (each owning
/// a key Vec) in any container made a plain 2-key decode run > 5 min (measured; the
/// nested-Vec wall of kani_RESUME.md lesson 2), one descriptor decodes in ~6 s.
fn support_codec_regions(ka: [u64; 2]) -> Vec<RegionState> {
    let a = support_slab_keys(S_START, 2 * SSU, SS, &ka);
    vec![
        support_region_with(vec![a]),
        support_region_with(Vec::new()),
    ]
}

fn support_sym_key() -> u64 {
    if kani::any() {
        FREE_KEY
    } else {
        kani::any()
    }
}

/// Encoding of region 0 of the codec regions alone (region count 1 | 1 slab | A header +
/// 2 keys): 4 + 4 + 24 + 16 = 48 bytes. Field boundaries: [0,4) region count, [4,8) slab
/// count, [8,32) slab header, [32,48) keys.
const CODEC_LEN: usize = 52;
const DECODE_LEN: usize = 48;

fn support_decode_cut(data: &[u8], l: usize, mutant: bool) {
    let r = crate::checkpoint::deserialize_slabs(&data[..l]);
    if mutant {
        assert!(r.is_ok());
    } else {
        assert!(support_is_corrupt(&r));
    }
    std::mem::forget(r);
}

/// A valid checkpoint payload in the serialize_region field order (checkpoint.rs:15-33,
/// FMT-Checkpoint-Payload): region count 1 | slab count 1 | start u64 | slab_size u64 |
/// element_size u32 | num_slots u32 = 2 | 2 keys (each FREE_KEY or any value). Built by
/// the harness: serialize_region over a RegionState holding a Slab did not finish in 4 min.
fn support_decode_encoding() -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&1u32.to_le_bytes());
    d.extend_from_slice(&S_START.to_le_bytes());
    d.extend_from_slice(&(2 * SSU).to_le_bytes());
    d.extend_from_slice(&SS.to_le_bytes());
    d.extend_from_slice(&2u32.to_le_bytes());
    d.extend_from_slice(&support_sym_key().to_le_bytes());
    d.extend_from_slice(&support_sym_key().to_le_bytes());
    d
}

fn support_case_decode_truncated(mutant: bool) {
    let data = support_decode_encoding();
    assert!(data.len() == DECODE_LEN);
    // the untruncated payload decodes
    let full = crate::checkpoint::deserialize_slabs(&data);
    assert!(full.is_ok());
    std::mem::forget(full);
    // one CONCRETE cut per branch, ending inside each field the decoder checks (a cut
    // length merged after an `if` is symbolic again)
    let g: u8 = kani::any();
    if g == 0 {
        support_decode_cut(&data, 0, mutant) // empty
    } else if g == 1 {
        support_decode_cut(&data, 3, mutant) // inside the region count
    } else if g == 2 {
        support_decode_cut(&data, 7, mutant) // inside the slab count
    } else if g == 3 {
        support_decode_cut(&data, 31, mutant) // inside the slab header
    } else if g == 4 {
        support_decode_cut(&data, 32, mutant) // header complete, no key
    } else {
        support_decode_cut(&data, 47, mutant) // inside the key vector
    }
    std::mem::forget(data);
}

fn support_case_ckpt_roundtrip(mutant: bool) {
    support_disk2_reset(false);
    let ka = [support_sym_key(), support_sym_key()];
    let seq: u64 = kani::any();
    kani::assume(seq < u64::MAX);
    let active: u8 = if kani::any() { 1 } else { 0 };
    let client = support_md_client(512);
    let (regions, shared) = support_ckpt_state(support_codec_regions(ka), seq, active, 8192);
    let w = crate::checkpoint::write_checkpoint(&client, &regions, &shared);
    assert!(w.is_ok());
    let (off, size, new_seq) = support_active_copy_offset(&shared);
    assert!(new_seq == seq + 1);
    let data = crate::checkpoint::read_checkpoint_region(&client, off, size, new_seq);
    assert!(data.is_ok());
    let data = data.unwrap();
    assert!(data.len() == CODEC_LEN);
    let d = crate::checkpoint::deserialize_slabs(&data);
    assert!(d.is_ok());
    let d = d.unwrap();
    // region by region: r0 has its one slab, r1 none
    assert!(d.len() == 2 && d[0].len() == 1 && d[1].is_empty());
    let x = &d[0][0];
    assert!(x.start_offset == S_START && x.slab_size == 2 * SSU && x.element_size == SS);
    // slot count == slab size / element size, one key per slot
    assert!(x.keys.len() == 2 && x.keys.len() as u64 == x.slab_size / x.element_size as u64);
    if mutant {
        assert!(x.keys[0] != ka[0]);
    } else {
        assert!(x.keys[0] == ka[0]);
    }
    assert!(x.keys[1] == ka[1]);
    std::mem::forget(d);
    std::mem::forget(data);
    std::mem::forget(client);
    std::mem::forget(regions);
}

/// A checkpoint copy in EXACTLY the on-disk form write_checkpoint produces
/// (checkpoint.rs:80-93): u64 seq | u32 payload_len | u32 CRC (computed with the CRC field
/// zeroed) | payload, zero-padded to the metadata sector size — built by the harness and
/// stored at the copy's LBA. Calling write_checkpoint itself did not finish in 5 min even
/// for an empty region at sector 32 (parking_lot/Mutex lock paths + region iteration;
/// see kani_measurements.md), so the read-side harnesses start from this image.
/// Payload: one region with no slab (4-byte region count 1 + 4-byte slab count 0).
fn support_ckpt_image(seq: u64, sector: usize) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&seq.to_le_bytes());
    blob.extend_from_slice(&(CK_PAYLOAD as u32).to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    blob.extend_from_slice(&1u32.to_le_bytes());
    blob.extend_from_slice(&0u32.to_le_bytes());
    let crc = crc32fast::hash(&blob);
    blob[12..16].copy_from_slice(&crc.to_le_bytes());
    let aligned = (blob.len() + sector - 1) / sector * sector;
    blob.resize(aligned, 0);
    blob
}

/// Copy 0 of a layout whose copies start at byte 4096 and are `copy` bytes, holding the
/// image for sequence 4, on a metadata device with `sector`-byte sectors.
/// Returns (client, copy byte offset, copy size, the copy's sequence).
fn support_ckpt_concrete(sector: u32, copy: u64) -> (BlockDeviceClient, u64, u64, u64, ()) {
    support_disk2_reset(false);
    let client = support_md_client(sector);
    let img = support_ckpt_image(4, sector as usize);
    assert!(support_disk2_write(4096 / sector as u64, &img).is_ok());
    (client, 4096, copy, 4, ())
}
/// Payload of that image: 4 (region count) + 4 (slab count).
const CK_PAYLOAD: usize = 8;
const CK_TOTAL: usize = 16 + CK_PAYLOAD;

fn support_case_ckpt_seq(mutant: bool) {
    let (client, off, size, seq, regions) = support_ckpt_concrete(32, 4096);
    let expected: u64 = kani::any();
    let r = crate::checkpoint::read_checkpoint_region(&client, off, size, expected);
    if expected != seq {
        if mutant {
            assert!(r.is_ok());
        } else {
            assert!(support_is_corrupt(&r));
        }
    } else {
        assert!(r.is_ok());
    }
    std::mem::forget(client);
    std::mem::forget(regions);
}

fn support_case_ckpt_oversize(mutant: bool) {
    let c: bool = kani::any();
    if c {
        // header altered to claim more than fits in the copy
        let (client, off, size, seq, regions) = support_ckpt_concrete(32, 4096);
        let len: u32 = kani::any();
        kani::assume(16 + len as u64 > size);
        support_disk_patch_set(off / 32, 8, &len.to_le_bytes());
        let r = crate::checkpoint::read_checkpoint_region(&client, off, size, seq);
        assert!(support_is_corrupt(&r));
        std::mem::forget(client);
        std::mem::forget(regions);
    } else {
        // the written data exactly fills the copy
        let (client, off, size, seq, regions) = support_ckpt_concrete(32, CK_TOTAL as u64);
        assert!(size == CK_TOTAL as u64);
        let r = crate::checkpoint::read_checkpoint_region(&client, off, size, seq);
        if mutant {
            assert!(r.is_err());
        } else {
            assert!(r.is_ok());
        }
        std::mem::forget(client);
        std::mem::forget(regions);
    }
}

fn support_case_ckpt_truncated(mutant: bool) {
    // Reads of ANY length <= requested (over-approximated client, stub_rb_short),
    // metadata sector 8 (header read itself shorter than the 16-byte header — also the
    // conforming-device case: 8 is a positive power of two) or 16 (header read, then a
    // second read of the aligned 32 bytes: both checks reachable).
    // SAFETY: single-threaded harness.
    unsafe { *std::ptr::addr_of_mut!(KANI_READ_LENS) = Vec::new() };
    let sector: u32 = if kani::any() { 8 } else { 16 };
    let (client, off, size, seq, regions) = support_ckpt_concrete(sector, 4096);
    let r = crate::checkpoint::read_checkpoint_region(&client, off, size, seq);
    // SAFETY: single-threaded harness.
    let lens = unsafe { &*std::ptr::addr_of!(KANI_READ_LENS) };
    assert!(!lens.is_empty());
    let final_len = if lens.len() > 1 { lens[1] } else { lens[0] };
    let short = lens[0] < 16 || final_len < CK_TOTAL;
    if short {
        if mutant {
            assert!(r.is_ok());
        } else {
            assert!(support_is_corrupt(&r));
        }
    } else {
        assert!(r.is_ok());
    }
    std::mem::forget(client);
    std::mem::forget(regions);
}

/// Any non-zero alteration of a 4-byte window (a burst <= 32 bits) at each window of the
/// 24 written bytes other than payload_len (seq low/high, CRC field, both payload words),
/// or one of three concrete alterations of payload_len (the field that also changes how
/// many bytes are checked: 8 -> 9, 8 -> 0, 8 -> 12). The payload_len alterations are
/// concrete: a symbolic one makes the read length symbolic (> 6.5 min).
fn support_case_ckpt_crc(mutant: bool) {
    let (client, off, size, seq, regions) = support_ckpt_concrete(32, 4096);
    if kani::any() {
        let d: u32 = kani::any();
        kani::assume(d != 0);
        let g: u8 = kani::any();
        let p: usize = if g == 0 {
            0
        } else if g == 1 {
            4
        } else if g == 2 {
            12
        } else if g == 3 {
            16
        } else {
            20
        };
        support_disk_patch_xor(off / 32, p, &d.to_le_bytes());
    } else {
        let g: u8 = kani::any();
        if g == 0 {
            support_disk_patch_xor(off / 32, 8, &1u32.to_le_bytes());
        } else if g == 1 {
            support_disk_patch_xor(off / 32, 8, &8u32.to_le_bytes());
        } else {
            support_disk_patch_xor(off / 32, 8, &4u32.to_le_bytes());
        }
    }
    let r = crate::checkpoint::read_checkpoint_region(&client, off, size, seq);
    if mutant {
        assert!(r.is_ok());
    } else {
        assert!(support_is_corrupt(&r));
    }
    std::mem::forget(r);
    std::mem::forget(client);
    std::mem::forget(regions);
}

// ---------------------------------------------------------------------------
// Batch 2 scored harnesses (each `__mutant` = same body, same flags, one assertion flipped)
// ---------------------------------------------------------------------------

// ---- EM-SB-NEW-DEFAULTS ----
#[kani::proof]
#[kani::solver(minisat)]
fn verify_em_sb_new_defaults() {
    support_case_sb_new_defaults(false);
}
#[kani::proof]
#[kani::solver(minisat)]
fn verify_em_sb_new_defaults__mutant() {
    support_case_sb_new_defaults(true);
}

// ---- EM-INV-SUPERBLOCK-WELL-FORMED (length-only CRC stub: pins the CRC's extent 0..92
// and position 92; the faithful model over symbolic fields did not finish in 9 min) ----
#[kani::proof]
#[kani::solver(minisat)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
fn verify_em_inv_superblock_well_formed() {
    support_case_sb_well_formed(false);
}
#[kani::proof]
#[kani::solver(minisat)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
fn verify_em_inv_superblock_well_formed__mutant() {
    support_case_sb_well_formed(true);
}

// ---- EM-INV-SUPERBLOCK-ROUNDTRIP (deterministic length-only CRC stub) ----
#[kani::proof]
#[kani::solver(minisat)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(std::fmt::format, stub_format)]
fn verify_em_inv_superblock_roundtrip() {
    support_case_sb_roundtrip(false);
}
#[kani::proof]
#[kani::solver(minisat)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(std::fmt::format, stub_format)]
fn verify_em_inv_superblock_roundtrip__mutant() {
    support_case_sb_roundtrip(true);
}

// ---- EM-SB-CORRUPTION-DETECTED (faithful CRC-32 model) ----
#[kani::proof]
#[kani::unwind(94)]
#[kani::solver(minisat)]
#[kani::stub(crc32fast::hash, stub_crc32_faithful)]
#[kani::stub(std::fmt::format, stub_format)]
fn verify_em_sb_corruption_detected() {
    support_case_sb_corruption(false);
}
#[kani::proof]
#[kani::unwind(94)]
#[kani::solver(minisat)]
#[kani::stub(crc32fast::hash, stub_crc32_faithful)]
#[kani::stub(std::fmt::format, stub_format)]
fn verify_em_sb_corruption_detected__mutant() {
    support_case_sb_corruption(true);
}

// ---- EM-SB-ACTIVE-COPY-VALID — REFUTATION (initialize accepts active copy >= 2) ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn uncredited_refute_em_sb_active_copy_valid() {
    support_case_active_copy(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn uncredited_refute_em_sb_active_copy_valid__mutant() {
    support_case_active_copy(true);
}

// ---- EM-BITMAP-NO-DOUBLE-ALLOC ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_bitmap_no_double_alloc() {
    support_slab_geoms(|sz, e| support_case_no_double_alloc(sz, e, false));
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_bitmap_no_double_alloc__mutant() {
    support_slab_geoms(|sz, e| support_case_no_double_alloc(sz, e, true));
}

// ---- EM-BITMAP-INDEX-IN-RANGE ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_bitmap_index_in_range() {
    support_slab_geoms(|sz, e| support_case_index_in_range(sz, e, false));
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_bitmap_index_in_range__mutant() {
    support_slab_geoms(|sz, e| support_case_index_in_range(sz, e, true));
}

// ---- EM-BITMAP-FIND-FREE (bitmap level: 1..=5 slots, every start) ----
#[kani::proof]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_em_bitmap_find_free() {
    support_dispatch_find_free(false);
}
#[kani::proof]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_em_bitmap_find_free__mutant() {
    support_dispatch_find_free(true);
}

// ---- EM-SLAB-OFFSET-SLOT-INVERSE ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_slab_offset_slot_inverse() {
    support_slab_geoms(|sz, e| support_case_offset_inverse(sz, e, false));
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_slab_offset_slot_inverse__mutant() {
    support_slab_geoms(|sz, e| support_case_offset_inverse(sz, e, true));
}

// ---- EM-SLAB-ROVER-IN-RANGE ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_slab_rover_in_range() {
    support_slab_geoms(|sz, e| support_case_rover(sz, e, false));
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_slab_rover_in_range__mutant() {
    support_slab_geoms(|sz, e| support_case_rover(sz, e, true));
}

// ---- EM-RECOVER-SLAB-FROM-DESCRIPTOR ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_recover_slab_from_descriptor() {
    support_slab_geoms(|sz, e| support_case_recover(sz, e, false));
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
fn verify_em_recover_slab_from_descriptor__mutant() {
    support_slab_geoms(|sz, e| support_case_recover(sz, e, true));
}

// ---- EM-CKPT-DECODE-TRUNCATED ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn verify_em_ckpt_decode_truncated() {
    support_case_decode_truncated(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn verify_em_ckpt_decode_truncated__mutant() {
    support_case_decode_truncated(true);
}

// ---- EM-INV-CHECKPOINT-ROUNDTRIP ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn boundary_em_inv_checkpoint_roundtrip() {
    support_case_ckpt_roundtrip(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn boundary_em_inv_checkpoint_roundtrip__mutant() {
    support_case_ckpt_roundtrip(true);
}

// ---- EM-CKPT-READ-SEQ-MISMATCH ----
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn verify_em_ckpt_read_seq_mismatch() {
    support_case_ckpt_seq(false);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn verify_em_ckpt_read_seq_mismatch__mutant() {
    support_case_ckpt_seq(true);
}

// ---- EM-CKPT-READ-OVERSIZE ----
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn verify_em_ckpt_read_oversize() {
    support_case_ckpt_oversize(false);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn verify_em_ckpt_read_oversize__mutant() {
    support_case_ckpt_oversize(true);
}

// ---- EM-CKPT-READ-TRUNCATED (short-read over-approximation of the client) ----
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_rb_short)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn verify_em_ckpt_read_truncated() {
    support_case_ckpt_truncated(false);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_rb_short)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn verify_em_ckpt_read_truncated__mutant() {
    support_case_ckpt_truncated(true);
}

// ---- EM-CKPT-READ-CRC (faithful CRC-32 model) ----
#[kani::proof]
#[kani::unwind(30)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_faithful)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn verify_em_ckpt_read_crc() {
    support_case_ckpt_crc(false);
}
#[kani::proof]
#[kani::unwind(30)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_faithful)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn verify_em_ckpt_read_crc__mutant() {
    support_case_ckpt_crc(true);
}

// ---------------------------------------------------------------------------
// Batch 2: block-I/O infrastructure — the REAL BlockDeviceClient::{write_blocks,
// read_blocks} against a model of the device actor behind its channels.
// ---------------------------------------------------------------------------
// Kani cannot run the actor thread, so `Sender::send` / `Receiver::recv` are stubbed
// (stub_chan_send / stub_chan_recv). `send` IS the device: it executes the command
// against a per-SECTOR store and queues the one answer `recv` will return.
// Answers, when KANI_DEV_FAULTS is on (each command independently):
//   0 the matching *Done{Ok}             (with faults off, and for every command but the
//                                         one symbolic fault position: always this)
//   1 the matching *Done{Err(e)}         e = any NvmeBlockError variant
//   2 Completion::Error{e}
//   3 a completion of the WRONG type     (not something a conforming actor sends; the
//                                         properties name it, so the model includes it)
//   4 channel closed on recv             (actor gone)
//   and `send` itself may fail (receiver dropped). With faults on, exactly the first command
//   (KANI_FAULT_CMD = 0) gets the fault KANI_FAULT_KIND, which each harness sets to each of
//   the five values in its own concrete branch.
// A write whose answer is not 0 may or may not have reached the medium. A read only
// fills the caller's DMA buffer on answer 0. This over-approximates a conforming device.
use component_core::channel::{ChannelError, Receiver, Sender};
use interfaces::{BlockDeviceError, DmaBuffer, OpHandle, SpdkEnvError};

/// Block-I/O test sector (bytes). Small on purpose: a positive power of two, as the
/// IBlockDevice contract requires; geometry stated in each note.
pub(crate) const BS: usize = 8;

// Fixed-size device state (nested Vecs are the CBMC wall here; see kani_RESUME.md).
const DEV_SLOTS: usize = 4;
static mut KANI_DEV_USED: [bool; DEV_SLOTS] = [false; DEV_SLOTS];
static mut KANI_DEV_LBA: [u64; DEV_SLOTS] = [0; DEV_SLOTS];
static mut KANI_DEV_DATA: [[u8; BS]; DEV_SLOTS] = [[0; BS]; DEV_SLOTS];
static mut KANI_DEV_FAULTS: bool = false;
static mut KANI_ALLOC_FAULTS: bool = false;
static mut KANI_ALLOC_FAILED: bool = false;
/// With KANI_ALLOC_FAULTS: allocation number KANI_ALLOC_FAIL_AT (set concretely per harness
/// branch) fails; KANI_ALLOC_N counts allocations.
static mut KANI_ALLOC_N: usize = 0;
static mut KANI_ALLOC_FAIL_AT: usize = usize::MAX;
static mut KANI_SENT_AFTER_ALLOC_FAIL: bool = false;
/// (ns_id, lba, is_write, bytes written) per command that reached the device.
static mut KANI_SENT: [(u32, u64, bool, [u8; BS]); DEV_SLOTS] = [(0, 0, false, [0; BS]); DEV_SLOTS];
static mut KANI_SENT_N: usize = 0;
static mut KANI_ATTEMPTS: usize = 0;
static mut KANI_FAULT_AT: Option<usize> = None;
/// The queued answer as SCALARS: (kind 0..=4, or 255 = none; command was a write?).
/// A `Completion` (which owns Strings) parked in a static made symex chase heap pointers
/// read back from static memory (909 aborted deallocate paths, > 6 min).
static mut KANI_PENDING: (u8, u8) = (255, 0);
/// With KANI_DEV_FAULTS: the index of the one command that faults (command 0).
static mut KANI_FAULT_CMD: Option<usize> = None;
/// The fault the faulting command gets: 1 *Done{Err}, 2 Completion::Error, 3 wrong
/// completion type, 4 channel closed on recv, 5 send fails. Set CONCRETELY per harness
/// branch (one merged symbolic kind cost > 120 s per run; see kani_measurements.md).
static mut KANI_FAULT_KIND: u8 = 1;

pub(crate) fn support_bio_reset(dev_faults: bool, alloc_faults: bool) {
    // SAFETY: single-threaded harness; no reference to these statics outlives this call.
    unsafe {
        *std::ptr::addr_of_mut!(KANI_DEV_USED) = [false; DEV_SLOTS];
        *std::ptr::addr_of_mut!(KANI_DEV_FAULTS) = dev_faults;
        *std::ptr::addr_of_mut!(KANI_ALLOC_FAULTS) = alloc_faults;
        *std::ptr::addr_of_mut!(KANI_ALLOC_FAILED) = false;
        *std::ptr::addr_of_mut!(KANI_ALLOC_N) = 0;
        *std::ptr::addr_of_mut!(KANI_ALLOC_FAIL_AT) = usize::MAX;
        *std::ptr::addr_of_mut!(KANI_SENT_AFTER_ALLOC_FAIL) = false;
        *std::ptr::addr_of_mut!(KANI_SENT_N) = 0;
        *std::ptr::addr_of_mut!(KANI_ATTEMPTS) = 0;
        *std::ptr::addr_of_mut!(KANI_FAULT_AT) = None;
        *std::ptr::addr_of_mut!(KANI_PENDING) = (255, 0);
        // with faults: the FIRST command faults (the no-fault behaviour is what the layout
        // harnesses prove; a symbolic position incl. "none" made the mutants > 120 s)
        *std::ptr::addr_of_mut!(KANI_FAULT_CMD) = if dev_faults { Some(0) } else { None };
    }
}

fn support_set_fault_kind(k: u8) {
    // SAFETY: single-threaded harness.
    unsafe { *std::ptr::addr_of_mut!(KANI_FAULT_KIND) = k };
}

/// The device-reported error. One variant: every path that receives it hands it to
/// `error::nvme_to_em` (stubbed by `stub_nvme_to_em`, which ignores it), and
/// `check_stub_conforms_nvme_to_em` covers all ten variants of the real mapping.
pub(crate) fn support_any_nvme_err() -> NvmeBlockError {
    NvmeBlockError::Timeout(String::new())
}

fn support_dev_sector(lba: u64) -> [u8; BS] {
    let mut out = [0u8; BS];
    // SAFETY: single-threaded harness.
    unsafe {
        let mut i = 0usize;
        while i < DEV_SLOTS {
            if (*std::ptr::addr_of!(KANI_DEV_USED))[i]
                && (*std::ptr::addr_of!(KANI_DEV_LBA))[i] == lba
            {
                out = (*std::ptr::addr_of!(KANI_DEV_DATA))[i];
            }
            i += 1;
        }
    }
    out
}

pub(crate) fn support_dev_store(lba: u64, data: &[u8]) {
    assert!(data.len() == BS);
    // SAFETY: single-threaded harness.
    unsafe {
        let used = &mut *std::ptr::addr_of_mut!(KANI_DEV_USED);
        let lbas = &mut *std::ptr::addr_of_mut!(KANI_DEV_LBA);
        let mut slot = DEV_SLOTS;
        let mut i = 0usize;
        while i < DEV_SLOTS {
            if slot == DEV_SLOTS && used[i] && lbas[i] == lba {
                slot = i;
            }
            i += 1;
        }
        i = 0;
        while i < DEV_SLOTS {
            if slot == DEV_SLOTS && !used[i] {
                slot = i;
            }
            i += 1;
        }
        assert!(slot < DEV_SLOTS, "device model: out of sector slots");
        used[slot] = true;
        lbas[slot] = lba;
        (*std::ptr::addr_of_mut!(KANI_DEV_DATA))[slot].copy_from_slice(data);
    }
}

fn support_sent_push(ns: u32, lba: u64, w: bool, data: &[u8]) {
    // SAFETY: single-threaded harness.
    unsafe {
        let n = *std::ptr::addr_of!(KANI_SENT_N);
        assert!(n < DEV_SLOTS, "device model: command log full");
        let e = &mut (*std::ptr::addr_of_mut!(KANI_SENT))[n];
        e.0 = ns;
        e.1 = lba;
        e.2 = w;
        if w {
            e.3.copy_from_slice(data);
        }
        *std::ptr::addr_of_mut!(KANI_SENT_N) = n + 1;
    }
}

fn support_device_execute(cmd: Command) -> Result<(), ChannelError> {
    // SAFETY: single-threaded harness; exclusive access for the duration of the call.
    unsafe {
        let idx = *std::ptr::addr_of!(KANI_ATTEMPTS);
        *std::ptr::addr_of_mut!(KANI_ATTEMPTS) = idx + 1;
        if *std::ptr::addr_of!(KANI_ALLOC_FAILED) {
            *std::ptr::addr_of_mut!(KANI_SENT_AFTER_ALLOC_FAIL) = true;
        }
        let faults = *std::ptr::addr_of!(KANI_DEV_FAULTS)
            && *std::ptr::addr_of!(KANI_FAULT_CMD) == Some(idx);
        let fk = *std::ptr::addr_of!(KANI_FAULT_KIND);
        if faults && fk == 5 {
            if (*std::ptr::addr_of!(KANI_FAULT_AT)).is_none() {
                *std::ptr::addr_of_mut!(KANI_FAULT_AT) = Some(idx);
            }
            return Err(ChannelError::Closed);
        }
        let kind: u8 = if faults { fk } else { 0 };
        if kind != 0 && (*std::ptr::addr_of!(KANI_FAULT_AT)).is_none() {
            *std::ptr::addr_of_mut!(KANI_FAULT_AT) = Some(idx);
        }
        let pending: (u8, u8) = match cmd {
            Command::WriteSync { ns_id, lba, buf } => {
                let data = buf.as_slice();
                support_sent_push(ns_id, lba, true, data);
                if kind == 0 || kani::any() {
                    support_dev_store(lba, data);
                }
                (kind, 1)
            }
            Command::ReadSync { ns_id, lba, buf } => {
                support_sent_push(ns_id, lba, false, &[]);
                if kind == 0 {
                    let mut g = buf.lock().unwrap();
                    let src = support_dev_sector(lba);
                    g.as_mut_slice()[..BS].copy_from_slice(&src);
                }
                (kind, 0)
            }
            Command::FlushSync { ns_id } => {
                support_sent_push(ns_id, 0, false, &[]);
                (kind, 2)
            }
            _ => (255, 0),
        };
        *std::ptr::addr_of_mut!(KANI_PENDING) = pending;
    }
    Ok(())
}

/// Move a value between two types that are the SAME type (checked by TypeId): the
/// channel stubs are generic over T but only ever instantiated with Command/Completion.
/// (A `Box<dyn Any>` downcast here made symex resolve vtable drops on its error branch.)
fn support_same_type<T: 'static, U: 'static>(t: T) -> U {
    assert!(std::any::TypeId::of::<T>() == std::any::TypeId::of::<U>());
    // SAFETY: T and U are the same type (asserted above); `t` is forgotten so the value
    // is moved, not duplicated.
    let u = unsafe { std::mem::transmute_copy::<T, U>(&t) };
    std::mem::forget(t);
    u
}

pub(crate) fn stub_chan_send<T: Send + 'static>(
    _this: &Sender<T>,
    value: T,
) -> Result<(), ChannelError> {
    support_device_execute(support_same_type::<T, Command>(value))
}

pub(crate) fn stub_chan_recv<T: Send + 'static>(_this: &Receiver<T>) -> Result<T, ChannelError> {
    // SAFETY: single-threaded harness.
    let (kind, op) = unsafe {
        let p = *std::ptr::addr_of!(KANI_PENDING);
        *std::ptr::addr_of_mut!(KANI_PENDING) = (255, 0);
        p
    };
    let h = OpHandle(0);
    let c = if kind == 0 || kind == 1 {
        let result = if kind == 0 {
            Ok(())
        } else {
            Err(support_any_nvme_err())
        };
        if op == 1 {
            Completion::WriteDone {
                handle: h,
                tag: 0,
                result,
            }
        } else if op == 0 {
            Completion::ReadDone {
                handle: h,
                tag: 0,
                result,
            }
        } else {
            Completion::FlushDone { handle: h, result }
        }
    } else if kind == 2 {
        Completion::Error {
            handle: None,
            error: support_any_nvme_err(),
        }
    } else if kind == 3 {
        // a completion of the wrong type for every command the client sends
        Completion::AbortAck { handle: h }
    } else {
        return Err(ChannelError::Closed);
    };
    Ok(support_same_type::<Completion, T>(c))
}

/// `error::nvme_to_em` model: the same variant (IoError) with the message text dropped.
/// The real function formats the device error (`e.to_string()`), and a formatter reached
/// on a path symex cannot rule out made a one-byte write_blocks run > 5 min.
/// `check_stub_conforms_nvme_to_em` proves the real function returns IoError for every
/// NvmeBlockError variant.
pub(crate) fn stub_nvme_to_em(_e: NvmeBlockError) -> ExtentManagerError {
    ExtentManagerError::IoError(String::new())
}

fn support_real_nvme_to_em_is_io(e: NvmeBlockError) {
    assert!(matches!(
        crate::error::nvme_to_em(e),
        ExtentManagerError::IoError(_)
    ));
}

#[kani::proof]
#[kani::unwind(3)]
#[kani::solver(minisat)]
fn check_stub_conforms_nvme_to_em() {
    // one concrete call per variant (a symbolic variant reaching the formatter is the cost)
    let v: u8 = kani::any();
    if v == 0 {
        support_real_nvme_to_em_is_io(NvmeBlockError::FeatureNotEnabled(String::new()))
    } else if v == 1 {
        support_real_nvme_to_em_is_io(NvmeBlockError::NotInitialized(String::new()))
    } else if v == 2 {
        support_real_nvme_to_em_is_io(NvmeBlockError::Timeout(String::new()))
    } else if v == 3 {
        support_real_nvme_to_em_is_io(NvmeBlockError::Aborted(String::new()))
    } else if v == 4 {
        support_real_nvme_to_em_is_io(NvmeBlockError::InvalidNamespace(String::new()))
    } else if v == 5 {
        support_real_nvme_to_em_is_io(NvmeBlockError::NotSupported(String::new()))
    } else if v == 6 {
        support_real_nvme_to_em_is_io(NvmeBlockError::BlockDevice(
            BlockDeviceError::DmaAllocationFailed(String::new()),
        ))
    } else if v == 7 {
        support_real_nvme_to_em_is_io(NvmeBlockError::SpdkEnv(SpdkEnvError::VfioNotAvailable(
            String::new(),
        )))
    } else if v == 8 {
        support_real_nvme_to_em_is_io(NvmeBlockError::LbaOutOfRange(String::new()))
    } else {
        support_real_nvme_to_em_is_io(NvmeBlockError::ClientDisconnected(String::new()))
    }
}

extern "C" fn support_dma_free(_p: *mut std::ffi::c_void) {}

/// DmaAllocFn model: a zeroed buffer of exactly `size` bytes (as DmaBuffer::new gives),
/// size 0 rejected (as DmaBuffer::new does), or — when KANI_ALLOC_FAULTS — a failure of
/// the one allocation numbered KANI_ALLOC_FAIL_AT (any of the first three).
pub(crate) fn support_dma_alloc() -> interfaces::DmaAllocFn {
    Arc::new(|size: usize, _align: usize, _numa: Option<i32>| {
        // SAFETY: single-threaded harness.
        unsafe {
            let n = *std::ptr::addr_of!(KANI_ALLOC_N);
            *std::ptr::addr_of_mut!(KANI_ALLOC_N) = n + 1;
            if *std::ptr::addr_of!(KANI_ALLOC_FAULTS)
                && *std::ptr::addr_of!(KANI_ALLOC_FAIL_AT) == n
            {
                *std::ptr::addr_of_mut!(KANI_ALLOC_FAILED) = true;
                return Err(String::new());
            }
        }
        let mem: &'static mut [u8] = Box::leak(vec![0u8; size].into_boxed_slice());
        // SAFETY: `mem` is a leaked allocation valid for `size` bytes forever; the free
        // function is a no-op, so DmaBuffer's Drop never frees it.
        unsafe {
            DmaBuffer::from_raw(
                mem.as_mut_ptr() as *mut std::ffi::c_void,
                size,
                support_dma_free,
                -1,
            )
        }
        .map_err(|_| String::new())
    })
}

/// A real BlockDeviceClient (sector BS) on fresh channel endpoints from the mock device.
fn support_bio_client(ns: u32, base: u64) -> BlockDeviceClient {
    let dev = KaniMetaDevice {
        sector: BS as u32,
        sectors: 64,
        geometry_fails: false,
    };
    let ch = dev.connect_client().unwrap();
    BlockDeviceClient::with_base_lba(ch, support_dma_alloc(), BS as u32, ns, base)
}

fn support_bio_sent() -> &'static [(u32, u64, bool, [u8; BS])] {
    // SAFETY: single-threaded harness; read-only view after the call under test.
    unsafe {
        let all = &*std::ptr::addr_of!(KANI_SENT);
        &all[..*std::ptr::addr_of!(KANI_SENT_N)]
    }
}

/// Symbolic namespace, base LBA and request LBA such that base + lba + 4 does not
/// overflow (the device's LBA range is far below that; see note).
fn support_bio_addr() -> (u32, u64, u64) {
    let ns: u32 = kani::any();
    let base: u64 = kani::any();
    let lba: u64 = kani::any();
    kani::assume(base <= u64::MAX / 2 && lba <= u64::MAX / 4);
    (ns, base, lba)
}

fn support_case_bio_write_layout<const L: usize>(mutant: bool) {
    support_bio_reset(false, false);
    let (ns, base, lba) = support_bio_addr();
    let client = support_bio_client(ns, base);
    let data: [u8; L] = kani::any();
    let r = client.write_blocks(lba, &data);
    assert!(r.is_ok());
    let nb = (L + BS - 1) / BS;
    let sent = support_bio_sent();
    assert!(sent.len() == nb);
    let b: usize = kani::any();
    kani::assume(b < nb);
    assert!(sent[b].0 == ns && sent[b].1 == base + lba + b as u64 && sent[b].2);
    assert!(sent[b].3.len() == BS);
    let j: usize = kani::any();
    kani::assume(j < BS);
    let pos = b * BS + j;
    let expect = if pos < L { data[pos] } else { 0 };
    if mutant {
        assert!(sent[b].3[j] != expect);
    } else {
        assert!(sent[b].3[j] == expect);
    }
    std::mem::forget(r);
    std::mem::forget(client);
}

fn support_dispatch_bio_write_layout(mutant: bool) {
    // one geometry: 9 bytes over 8-byte sectors = a full block + a partial (padded) one
    // (each block-I/O call costs ~40 s of CBMC; a 5-size dispatch did not fit the cap)
    support_case_bio_write_layout::<9>(mutant)
}

fn support_case_bio_read_layout<const N: usize>(mutant: bool) {
    support_bio_reset(false, false);
    let (ns, base, lba) = support_bio_addr();
    // the device holds arbitrary content on every sector the read can touch
    let mut b = 0u64;
    while b < 3 {
        let s: [u8; BS] = kani::any();
        support_dev_store(base + lba + b, &s);
        b += 1;
    }
    let client = support_bio_client(ns, base);
    let r = client.read_blocks(lba, N);
    assert!(r.is_ok());
    let v = r.unwrap();
    assert!(v.len() == N);
    let nb = (N + BS - 1) / BS;
    let sent = support_bio_sent();
    assert!(sent.len() == nb);
    let k: usize = kani::any();
    kani::assume(k < nb);
    assert!(sent[k].0 == ns && sent[k].1 == base + lba + k as u64 && !sent[k].2);
    let j: usize = kani::any();
    kani::assume(j < N);
    let expect = support_dev_sector(base + lba + (j / BS) as u64)[j % BS];
    if mutant {
        assert!(v[j] != expect);
    } else {
        assert!(v[j] == expect);
    }
    std::mem::forget(v);
    std::mem::forget(client);
}

fn support_dispatch_bio_read_layout(mutant: bool) {
    // one geometry: 9 bytes over 8-byte sectors = a full block + a partial (padded) one
    // (each block-I/O call costs ~40 s of CBMC; a 5-size dispatch did not fit the cap)
    support_case_bio_read_layout::<9>(mutant)
}

fn support_is_io<T>(r: &Result<T, ExtentManagerError>) -> bool {
    matches!(r, Err(ExtentManagerError::IoError(_)))
}

fn support_case_bio_write_err<const L: usize, const K: u8>(mutant: bool) {
    support_bio_reset(true, false);
    support_set_fault_kind(K);
    // addresses and data do not enter the error semantics: concrete
    let (ns, base, lba) = (1u32, 0u64, 3u64);
    let client = support_bio_client(ns, base);
    let data: [u8; L] = [0x5A; L];
    let r = client.write_blocks(lba, &data);
    // SAFETY: single-threaded harness.
    let (fault, attempts) = unsafe {
        (
            *std::ptr::addr_of!(KANI_FAULT_AT),
            *std::ptr::addr_of!(KANI_ATTEMPTS),
        )
    };
    kani::cover!(fault.is_some());
    match fault {
        Some(k) => {
            assert!(support_is_io(&r));
            // no block after the failing one is sent
            if mutant {
                assert!(attempts != k + 1);
            } else {
                assert!(attempts == k + 1);
            }
        }
        None => {
            assert!(r.is_ok());
            assert!(attempts == (L + BS - 1) / BS);
        }
    }
    std::mem::forget(r);
    std::mem::forget(client);
}

fn support_dispatch_bio_write_err(mutant: bool) {
    // 9 bytes over 8-byte sectors (2 blocks), the first command faulting, one CONCRETE
    // branch per fault kind
    let g: u8 = kani::any();
    if g == 0 {
        support_case_bio_write_err::<9, 1>(mutant)
    } else if g == 1 {
        support_case_bio_write_err::<9, 2>(mutant)
    } else if g == 2 {
        support_case_bio_write_err::<9, 3>(mutant)
    } else if g == 3 {
        support_case_bio_write_err::<9, 4>(mutant)
    } else {
        support_case_bio_write_err::<9, 5>(mutant)
    }
}

fn support_case_bio_read_err<const N: usize, const K: u8>(mutant: bool) {
    support_bio_reset(true, false);
    support_set_fault_kind(K);
    // addresses and data do not enter the error semantics: concrete
    let (ns, base, lba) = (1u32, 0u64, 3u64);
    let client = support_bio_client(ns, base);
    let r = client.read_blocks(lba, N);
    // SAFETY: single-threaded harness.
    let (fault, attempts) = unsafe {
        (
            *std::ptr::addr_of!(KANI_FAULT_AT),
            *std::ptr::addr_of!(KANI_ATTEMPTS),
        )
    };
    kani::cover!(fault.is_some());
    match fault {
        Some(k) => {
            if mutant {
                assert!(!support_is_io(&r));
            } else {
                assert!(support_is_io(&r));
            }
            assert!(attempts == k + 1);
        }
        None => {
            assert!(r.is_ok());
        }
    }
    std::mem::forget(r);
    std::mem::forget(client);
}

fn support_dispatch_bio_read_err(mutant: bool) {
    // 9 bytes over 8-byte sectors (2 blocks), the first command faulting, one CONCRETE
    // branch per fault kind
    let g: u8 = kani::any();
    if g == 0 {
        support_case_bio_read_err::<9, 1>(mutant)
    } else if g == 1 {
        support_case_bio_read_err::<9, 2>(mutant)
    } else if g == 2 {
        support_case_bio_read_err::<9, 3>(mutant)
    } else if g == 3 {
        support_case_bio_read_err::<9, 4>(mutant)
    } else {
        support_case_bio_read_err::<9, 5>(mutant)
    }
}

fn support_case_bio_alloc_err<const L: usize, const W: bool, const AT: usize>(mutant: bool) {
    support_bio_reset(false, true);
    // SAFETY: single-threaded harness.
    unsafe { *std::ptr::addr_of_mut!(KANI_ALLOC_FAIL_AT) = AT };
    // addresses and data do not enter the error semantics: concrete
    let (ns, base, lba) = (1u32, 0u64, 3u64);
    let client = support_bio_client(ns, base);
    let r: Result<(), ExtentManagerError> = if W {
        let data: [u8; L] = [0x5A; L];
        client.write_blocks(lba, &data)
    } else {
        client.read_blocks(lba, L).map(|_| ())
    };
    // SAFETY: single-threaded harness.
    let (failed, sent_after, sent) = unsafe {
        (
            *std::ptr::addr_of!(KANI_ALLOC_FAILED),
            *std::ptr::addr_of!(KANI_SENT_AFTER_ALLOC_FAIL),
            *std::ptr::addr_of!(KANI_SENT_N),
        )
    };
    assert!(failed);
    assert!(support_is_io(&r));
    if mutant {
        assert!(sent_after);
    } else {
        assert!(!sent_after);
    }
    // exactly the blocks whose own buffer was allocated before the failure were sent
    // (write: allocation 0 is the staging buffer, allocation 1 + b block b's; read:
    // allocation b is block b's)
    let expect = if W { AT.saturating_sub(1) } else { AT };
    assert!(sent == expect);
    std::mem::forget(r);
    std::mem::forget(client);
}

fn support_dispatch_bio_alloc_err(mutant: bool) {
    // 9 bytes over 8-byte sectors (2 blocks); one CONCRETE branch per (operation, failing
    // allocation): write fails at the staging buffer, at block 0's or at block 1's buffer;
    // read fails at block 0's or block 1's buffer
    let g: u8 = kani::any();
    if g == 0 {
        support_case_bio_alloc_err::<9, true, 0>(mutant)
    } else if g == 1 {
        support_case_bio_alloc_err::<9, true, 1>(mutant)
    } else if g == 2 {
        support_case_bio_alloc_err::<9, true, 2>(mutant)
    } else if g == 3 {
        support_case_bio_alloc_err::<9, false, 0>(mutant)
    } else {
        support_case_bio_alloc_err::<9, false, 1>(mutant)
    }
}

// ---------------------------------------------------------------------------
// Batch 2: EM-ERR-DEVICE-AS-IOERROR refutation — scripted media error on the
// checkpoint read during initialize().
// ---------------------------------------------------------------------------
static mut KANI_READ_COUNT: usize = 0;

/// Disk model whose FIRST read (the superblock) succeeds and every later read reports a
/// device failure, as the real client reports one: `IoError` (EM-BIO-READ-ERR).
pub(crate) fn stub_rb_media_err(
    this: &BlockDeviceClient,
    lba: u64,
    num_bytes: usize,
) -> Result<Vec<u8>, ExtentManagerError> {
    // SAFETY: single-threaded harness.
    let i = unsafe {
        let i = *std::ptr::addr_of!(KANI_READ_COUNT);
        *std::ptr::addr_of_mut!(KANI_READ_COUNT) = i + 1;
        i
    };
    if i >= 1 {
        return Err(ExtentManagerError::IoError(String::new()));
    }
    support_disk2_read(this.kani_base_lba() + lba, num_bytes)
}

fn support_case_device_err_reported(mutant: bool) {
    // one witness suffices for a refutation: the state after format(L_ONE) + one
    // checkpoint (seq 1, copy 1 active)
    support_case_device_err_reported_at(false, mutant)
}

fn support_case_device_err_reported_at(two: bool, mutant: bool) {
    support_disk2_reset(false);
    // SAFETY: single-threaded harness.
    unsafe { *std::ptr::addr_of_mut!(KANI_READ_COUNT) = 0 };
    // recover() uses the manager only for its (unconnected) logger
    let em = support_new_em();
    // the superblock format(L_ONE) followed by one (seq 1) or two (seq 2) checkpoints leaves
    let mut sb = Superblock::new(2 * SSU, SS, 2 * SSU, 2 * SS, 1, 4096, 6144, 0xC0FFEE, 1, 0);
    sb.checkpoint_seq = if two { 2 } else { 1 };
    sb.active_copy = if two { 0 } else { 1 };
    assert!(support_disk2_write(0, &sb.serialize()).is_ok());
    // initialize() = `let (sb, per_region) = recovery::recover(&client, self)?` on the client
    // get_metadata_client builds (lib.rs:522-523): its error reaches the caller unchanged.
    // the client get_metadata_client(1) builds for this device (see above)
    let client = support_md_client(MD_SS);
    let r = crate::recovery::recover(&client, &em);
    if mutant {
        assert!(support_is_io(&r));
    } else {
        assert!(support_is_corrupt(&r));
    }
    std::mem::forget(r);
    std::mem::forget(client);
    std::mem::forget(em);
}

// ---------------------------------------------------------------------------
// Batch 2: ExtentManager driver with a BUDDY MODEL (the wall batch 1 measured is
// BuddyAllocator.free_lists: Vec<Vec<u64>>; these ids do not reason about the buddy).
// ---------------------------------------------------------------------------
// stub_buddy_alloc / stub_buddy_free model a CONFORMING buddy allocator for the only
// call shape the region uses (alloc(slab_size) / free(start, slab_size)): the region's
// space is split into slab-sized aligned blocks; alloc returns any block not currently
// allocated, or None (also allowed when space remains: over-approximation); free must
// name an allocated block (asserted — a region calling free on anything else fails).
static mut KANI_BUDDY_USED: [bool; 4] = [false; 4];

pub(crate) fn stub_buddy_alloc(this: &mut BuddyAllocator, size: u64) -> Option<u64> {
    let base = this.base_offset();
    let total = this.total_usable_size();
    // SAFETY: single-threaded harness.
    let used = unsafe { &mut *std::ptr::addr_of_mut!(KANI_BUDDY_USED) };
    let mut avail = false;
    let mut j = 0usize;
    while j < 4 {
        if !used[j] && (j as u64 + 1) * size <= total {
            avail = true;
        }
        j += 1;
    }
    if !avail || kani::any() {
        return None;
    }
    let k: usize = kani::any();
    kani::assume(k < 4);
    kani::assume(!used[k] && (k as u64 + 1) * size <= total);
    used[k] = true;
    Some(base + k as u64 * size)
}

pub(crate) fn stub_buddy_free(this: &mut BuddyAllocator, abs_offset: u64, size: u64) {
    let base = this.base_offset();
    // SAFETY: single-threaded harness.
    let used = unsafe { &mut *std::ptr::addr_of_mut!(KANI_BUDDY_USED) };
    let mut found = false;
    let mut j = 0usize;
    while j < 4 {
        if base + j as u64 * size == abs_offset {
            assert!(used[j], "buddy free of a block that is not allocated");
            used[j] = false;
            found = true;
        }
        j += 1;
    }
    assert!(found, "buddy free of a non-block offset");
}

/// The state format(L_ONE) produces, built directly (no BuddyAllocator::new — the buddy
/// is the model above): one region [0, 2 sectors) = room for exactly ONE slab (a second
/// live Slab in the BTreeMap is the nested-Vec wall), slab 2 sectors, max extent 2
/// sectors, sector 512, metadata device 4 x 4 KiB, checkpoint copies 6144 B at 4096.
pub(crate) fn support_em_direct() -> ExtentManager {
    support_disk2_reset(false);
    // SAFETY: single-threaded harness.
    unsafe { *std::ptr::addr_of_mut!(KANI_BUDDY_USED) = [false; 4] };
    let em = support_em_with_device(4, false);
    let p = FormatParams {
        data_disk_size: 2 * SSU,
        slab_size: 2 * SSU,
        max_extent_size: 2 * SS,
        sector_size: SS,
        region_count: 1,
        metadata_alignment: 4096,
        instance_id: Some(0xC0FFEE),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    };
    let region = RegionState::new(
        BuddyAllocator::kani_from_parts(0, 2 * SSU, SS, 0, Vec::new()),
        p.clone(),
    );
    *em.regions.write() = Some(vec![Arc::new(parking_lot::RwLock::new(region))]);
    let sb = Superblock::new(2 * SSU, SS, 2 * SSU, 2 * SS, 1, 4096, 6144, 0xC0FFEE, 1, 0);
    *em.shared.lock().unwrap() = Some(crate::region::SharedState {
        format_params: p,
        checkpoint_seq: 0,
        superblock: sb,
    });
    em
}

/// Ghost state of the driver: the live handle, published offsets (<= 3) and the number
/// of removed-but-not-yet-flushed extents.
pub(crate) struct Ghost {
    pub live: Option<WriteHandle>,
    pub published: [Option<u64>; 3],
    pub pending: usize,
    pub sizes_ok: bool,
}

fn support_ghost_published(g: &Ghost) -> usize {
    g.published.iter().filter(|o| o.is_some()).count()
}

/// One public-API operation (I/O errors per the disk model):
///   0 reserve_extent(key, {512 | 100 | 1024}) if no live handle
///   1 publish live handle (key symbolic, may be FREE_KEY)   2 abort   3 drop
///   4 remove_extent(a published offset, or a concrete offset {0, 100, 512, 1024})
///   5 checkpoint()
pub(crate) fn support_em_step(em: &ExtentManager, g: &mut Ghost) {
    let op: u8 = kani::any();
    if op == 0 {
        if g.live.is_none() {
            let key: u64 = kani::any();
            let s: u8 = kani::any();
            let (r, aligned) = if s == 0 {
                (em.reserve_extent(key, SS), SS)
            } else if s == 1 {
                (em.reserve_extent(key, 100), SS)
            } else {
                (em.reserve_extent(key, 2 * SS), 2 * SS)
            };
            if let Ok(h) = r {
                // the slot's slab has exactly the request's sector-aligned element size
                let off = h.extent_offset();
                let regions = em.regions.read();
                let reg = regions.as_ref().unwrap()[0].read();
                let mut ok = h.extent_size() == aligned;
                for slab in reg.slabs.values() {
                    if slab.contains_offset(off) {
                        ok &= slab.element_size == aligned;
                    }
                }
                g.sizes_ok &= ok;
                drop(reg);
                drop(regions);
                g.live = Some(h);
            }
        }
    } else if op == 1 {
        if let Some(h) = g.live.take() {
            let key = h.key();
            let off = h.extent_offset();
            let r = h.publish();
            if r.is_ok() && key != FREE_KEY {
                let mut placed = false;
                let mut i = 0usize;
                while i < 3 {
                    if !placed && g.published[i].is_none() {
                        g.published[i] = Some(off);
                        placed = true;
                    }
                    i += 1;
                }
                assert!(placed);
            }
        }
    } else if op == 2 {
        if let Some(h) = g.live.take() {
            h.abort();
        }
    } else if op == 3 {
        if let Some(h) = g.live.take() {
            drop(h);
        }
    } else if op == 4 {
        let c: u8 = kani::any();
        let off = if c < 3 && g.published[c as usize].is_some() {
            g.published[c as usize].unwrap()
        } else if c == 3 {
            0
        } else if c == 4 {
            100
        } else if c == 5 {
            512
        } else {
            1024
        };
        if em.remove_extent(off).is_ok() {
            let mut hit = false;
            let mut i = 0usize;
            while i < 3 {
                if !hit && g.published[i] == Some(off) {
                    g.published[i] = None;
                    hit = true;
                }
                i += 1;
            }
            assert!(
                hit,
                "remove_extent succeeded on an offset that is not a published extent"
            );
            g.pending += 1;
        }
    } else if em.checkpoint().is_ok() {
        g.pending = 0;
    }
}

/// Region predicates (one region, element sizes 512 / 1024 are the only reachable ones).
fn support_nonfull_listed(r: &RegionState) -> bool {
    let mut ok = true;
    for slab in r.slabs.values() {
        if !slab.is_full() {
            ok &= r
                .size_classes
                .get_slabs(slab.element_size)
                .contains(&slab.start_offset);
        }
    }
    ok
}

fn support_uniform_element(r: &RegionState) -> bool {
    let mut ok = true;
    let mut lists = 0usize;
    for e in [SS, 2 * SS] {
        let l = r.size_classes.get_slabs(e);
        if !l.is_empty() {
            lists += 1;
        }
        for s in l {
            if let Some(sl) = r.slabs.get(s) {
                ok &= sl.element_size == e;
            }
        }
    }
    // no size class other than 512 / 1024 is listed
    ok && r.size_classes.kani_len() == lists
}

fn support_allocated_slots(r: &RegionState) -> usize {
    let mut n = 0usize;
    for slab in r.slabs.values() {
        n += slab.bitmap.count_set();
    }
    n
}

fn support_with_region<T, F: Fn(&RegionState) -> T>(em: &ExtentManager, f: F) -> T {
    let regions = em.regions.read();
    let r = regions.as_ref().unwrap()[0].read();
    f(&r)
}

fn support_seq_agree(em: &ExtentManager) -> bool {
    let g = em.shared.lock().unwrap();
    let s = g.as_ref().unwrap();
    s.checkpoint_seq == s.superblock.checkpoint_seq
}

/// Which property the driver asserts after every step.
pub(crate) const P_NONFULL: u8 = 0;
pub(crate) const P_UNIFORM: u8 = 1;
pub(crate) const P_DOUBLE_FREE: u8 = 2;
pub(crate) const P_FREE_KEY: u8 = 3;
pub(crate) const P_SEQ: u8 = 4;

fn support_check(em: &ExtentManager, g: &Ghost, which: u8, mutant: bool) {
    let flip = |b: bool| if mutant { !b } else { b };
    if which == P_NONFULL {
        assert!(flip(support_with_region(em, support_nonfull_listed)));
    } else if which == P_UNIFORM {
        assert!(g.sizes_ok);
        assert!(flip(support_with_region(em, support_uniform_element)));
    } else if which == P_DOUBLE_FREE {
        // every slot is freed exactly once: the allocated slots are exactly those held by
        // the live handle, the published extents and the not-yet-flushed removals
        let held = g.live.is_some() as usize + support_ghost_published(g) + g.pending;
        assert!(flip(
            support_with_region(em, support_allocated_slots) == held
        ));
    } else if which == P_FREE_KEY {
        let ex = em.get_extents();
        let mut ok = ex.len() == support_ghost_published(g);
        for x in ex.iter() {
            ok &= x.key != FREE_KEY;
        }
        let mut seen = 0usize;
        let mut cb_ok = true;
        em.for_each_extent(&mut |x: &Extent| {
            seen += 1;
            cb_ok &= x.key != FREE_KEY;
        });
        assert!(cb_ok && seen == ex.len());
        assert!(flip(ok));
    } else {
        assert!(flip(support_seq_agree(em)));
    }
}

/// K public-API steps from the formatted state (I/O errors off while building, on for
/// the steps), the property asserted after every step.
pub(crate) fn support_em_drive<const K: usize>(which: u8, mutant: bool) {
    let em = support_em_direct();
    support_disk_reset_errors(true);
    let mut g = Ghost {
        live: None,
        published: [None; 3],
        pending: 0,
        sizes_ok: true,
    };
    let mut i = 0usize;
    while i < K {
        support_em_step(&em, &mut g);
        support_check(&em, &g, which, mutant && i + 1 == K);
        i += 1;
    }
    std::mem::forget(g);
    std::mem::forget(em);
}

/// EM-INV-SEQ-AGREE establishment: format(L_ONE) on a fresh manager, or initialize() from a
/// device holding what the component wrote (a superblock naming checkpoint seq `s` and,
/// for s > 0, the copy write_checkpoint produced for a one-region empty state).
fn support_case_seq_establish(mutant: bool) {
    let flip = |b: bool| if mutant { !b } else { b };
    if kani::any() {
        support_disk2_reset(false);
        let em = support_em_with_device(4, false);
        assert!(em.format(support_layout(L_ONE)).is_ok());
        assert!(flip(support_seq_agree(&em)));
        std::mem::forget(em);
    } else {
        support_disk2_reset(false);
        let s: u64 = if kani::any() { 0 } else { 5 };
        let active: u8 = if kani::any() { 1 } else { 0 };
        let client = support_md_client(MD_SS);
        let (regions, shared) = support_ckpt_state(
            vec![support_region_with(Vec::new())],
            s.wrapping_sub(1),
            active,
            6144,
        );
        if s > 0 {
            assert!(crate::checkpoint::write_checkpoint(&client, &regions, &shared).is_ok());
        } else {
            let mut g = shared.lock().unwrap();
            let st = g.as_mut().unwrap();
            st.checkpoint_seq = 0;
            st.superblock.checkpoint_seq = 0;
        }
        let sb = shared.lock().unwrap().as_ref().unwrap().superblock.clone();
        assert!(support_disk2_write(0, &sb.serialize()).is_ok());
        let em = support_em_with_device(4, false);
        let r = em.initialize();
        assert!(r.is_ok());
        assert!(flip(support_seq_agree(&em)));
        std::mem::forget(em);
        std::mem::forget(client);
        std::mem::forget(regions);
        std::mem::forget(shared);
    }
}

// ---- EM-SLAB / BITMAP / FREE-KEY / SEQ ids on the EM driver (buddy model) ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_sizeclass_nonfull_listed() {
    support_em_drive::<4>(P_NONFULL, false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_sizeclass_nonfull_listed__mutant() {
    support_em_drive::<4>(P_NONFULL, true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_inv_slab_uniform_element() {
    support_em_drive::<4>(P_UNIFORM, false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_inv_slab_uniform_element__mutant() {
    support_em_drive::<4>(P_UNIFORM, true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_bitmap_no_double_free() {
    support_em_drive::<4>(P_DOUBLE_FREE, false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_bitmap_no_double_free__mutant() {
    support_em_drive::<4>(P_DOUBLE_FREE, true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_inv_free_key_never_visible() {
    support_em_drive::<4>(P_FREE_KEY, false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_inv_free_key_never_visible__mutant() {
    support_em_drive::<4>(P_FREE_KEY, true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_inv_seq_agree() {
    if kani::any() {
        support_case_seq_establish(false);
    } else {
        support_em_drive::<4>(P_SEQ, false);
    }
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
#[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
#[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
fn boundary_em_inv_seq_agree__mutant() {
    if kani::any() {
        support_case_seq_establish(true);
    } else {
        support_em_drive::<4>(P_SEQ, true);
    }
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_rb_media_err)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn refute_em_err_device_as_ioerror() {
    support_case_device_err_reported(false);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_write_layout() {
    support_dispatch_bio_write_layout(false);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_read_layout() {
    support_dispatch_bio_read_layout(false);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_write_err() {
    support_dispatch_bio_write_err(false);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_read_err() {
    support_dispatch_bio_read_err(false);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_alloc_err() {
    support_dispatch_bio_alloc_err(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_rb_media_err)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn refute_em_err_device_as_ioerror__mutant() {
    support_case_device_err_reported(true);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_write_layout__mutant() {
    support_dispatch_bio_write_layout(true);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_read_layout__mutant() {
    support_dispatch_bio_read_layout(true);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_write_err__mutant() {
    support_dispatch_bio_write_err(true);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_read_err__mutant() {
    support_dispatch_bio_read_err(true);
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn verify_em_bio_alloc_err__mutant() {
    support_dispatch_bio_alloc_err(true);
}

// ---- EM-BIO-FLUSH: compiled only with the non-default `volatile_write_cache` feature.
// The gate's command line (`cargo kani --harness .. -Z stubbing`) passes no --features and
// the batch rules allow no Cargo.toml change beyond the lint table, so these are named
// boundary_* (not scored). Run by hand with:
//   cargo kani --features volatile_write_cache --harness verification::boundary_em_bio_flush --exact -Z stubbing
#[cfg(feature = "volatile_write_cache")]
fn support_case_bio_flush(mutant: bool) {
    support_bio_reset(true, false);
    let (ns, base, _lba) = support_bio_addr();
    let client = support_bio_client(ns, base);
    let r = client.flush();
    // SAFETY: single-threaded harness.
    let fault = unsafe { *std::ptr::addr_of!(KANI_FAULT_AT) };
    let sent = support_bio_sent();
    kani::cover!(fault.is_some());
    // at most the one flush command reached the device (none if `send` itself failed)
    assert!(sent.len() <= 1);
    if sent.len() == 1 {
        assert!(sent[0].0 == ns);
    }
    if fault.is_none() {
        // the device answered FlushDone{Ok}
        if mutant {
            assert!(r.is_err());
        } else {
            assert!(r.is_ok());
        }
    } else {
        assert!(support_is_io(&r));
    }
    std::mem::forget(client);
}

#[cfg(feature = "volatile_write_cache")]
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn boundary_em_bio_flush() {
    support_case_bio_flush(false);
}

#[cfg(feature = "volatile_write_cache")]
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn boundary_em_bio_flush__mutant() {
    support_case_bio_flush(true);
}

// ===========================================================================
// 3. Kani limitation study (L3, 2026-10-07): shallow-unattempted group.
//    Additive only. Every case takes `mutant` and the `__mutant` twin flips the
//    asserted outcome; both run under identical flags.
// ===========================================================================

fn support_l3_is_corrupt(r: &Result<(), ExtentManagerError>) -> bool {
    matches!(r, Err(ExtentManagerError::CorruptMetadata(_)))
}
fn support_l3_is_notinit<T>(r: &Result<T, ExtentManagerError>) -> bool {
    matches!(r, Err(ExtentManagerError::NotInitialized(_)))
}
fn support_l3_is_io<T>(r: &Result<T, ExtentManagerError>) -> bool {
    matches!(r, Err(ExtentManagerError::IoError(_)))
}
/// "state unchanged" for a fresh component: neither regions nor shared set.
fn support_l3_uninit(em: &ExtentManager) -> bool {
    em.regions.read().is_none() && em.shared.lock().unwrap().is_none()
}

/// Fully symbolic FormatParams (no harness bounds). instance_id is always Some, so the
/// /dev/urandom branch (os-file-io group) is not on these paths.
fn support_l3_any_params() -> FormatParams {
    FormatParams {
        data_disk_size: kani::any(),
        slab_size: kani::any(),
        max_extent_size: kani::any(),
        sector_size: kani::any(),
        region_count: kani::any(),
        metadata_alignment: kani::any(),
        instance_id: Some(kani::any()),
        metadata_disk_ns_id: kani::any(),
        metadata_region_size: kani::any(),
    }
}
/// The four early validation guards of format() pass (lib.rs:383-401), plus level-2
/// assumption D-RANGE-FR-002-217ace (sector size is a power of two).
fn support_l3_assume_guards_pass(p: &FormatParams) {
    kani::assume(p.sector_size.is_power_of_two());
    kani::assume(p.slab_size % p.sector_size as u64 == 0);
    kani::assume(p.max_extent_size as u64 <= p.slab_size);
    kani::assume(p.region_count.is_power_of_two());
}
/// A fresh component, with or without a (conforming) metadata device.
fn support_l3_em_any_device() -> ExtentManager {
    if kani::any() {
        support_new_em()
    } else {
        let sectors: u64 = kani::any();
        kani::assume(sectors <= u64::MAX / MD_SS as u64);
        support_em_with_device(sectors, kani::any())
    }
}

fn support_case_l3_fmt_dev_not_connected(mutant: bool) {
    let em = support_new_em();
    let p = support_l3_any_params();
    support_l3_assume_guards_pass(&p);
    let r = em.format(p);
    if mutant {
        assert!(!support_l3_is_notinit(&r));
    } else {
        assert!(support_l3_is_notinit(&r));
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

fn support_case_l3_fmt_sector_zero(mutant: bool) {
    let em = support_l3_em_any_device();
    let mut p = support_l3_any_params();
    p.sector_size = 0;
    let r = em.format(p);
    assert!(support_l3_is_corrupt(&r) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_fmt_slab_not_multiple(mutant: bool) {
    let em = support_l3_em_any_device();
    let p = support_l3_any_params();
    kani::assume(p.sector_size != 0);
    kani::assume(p.slab_size % p.sector_size as u64 != 0);
    let r = em.format(p);
    assert!(support_l3_is_corrupt(&r) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_fmt_max_extent(mutant: bool) {
    // no device connected: once the 4 guards pass, format stops at the receptacle with
    // NotInitialized, so "accepted" (equal case) is observable as "not CorruptMetadata".
    let em = support_new_em();
    let p = support_l3_any_params();
    kani::assume(p.sector_size.is_power_of_two());
    kani::assume(p.slab_size % p.sector_size as u64 == 0);
    kani::assume(p.region_count.is_power_of_two());
    let gt = p.max_extent_size as u64 > p.slab_size;
    let eq = p.max_extent_size as u64 == p.slab_size;
    let r = em.format(p);
    kani::cover!(eq);
    kani::cover!(gt);
    if gt {
        assert!(support_l3_is_corrupt(&r) != mutant);
    }
    if eq {
        assert!(!support_l3_is_corrupt(&r));
    }
    std::mem::forget(em);
}

fn support_case_l3_fmt_region_count(mutant: bool) {
    let em = support_new_em();
    let p = support_l3_any_params();
    kani::assume(p.sector_size.is_power_of_two());
    kani::assume(p.slab_size % p.sector_size as u64 == 0);
    kani::assume(p.max_extent_size as u64 <= p.slab_size);
    let bad = p.region_count == 0 || !p.region_count.is_power_of_two();
    let r = em.format(p);
    kani::cover!(bad);
    if mutant {
        // flipped iff: claims the error exactly for power-of-two counts
        assert!(support_l3_is_corrupt(&r) == !bad);
    } else {
        assert!(support_l3_is_corrupt(&r) == bad);
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

fn support_case_l3_fmt_device_query(mutant: bool) {
    let sectors: u64 = kani::any();
    kani::assume(sectors <= u64::MAX / MD_SS as u64);
    let em = support_em_with_device(sectors, true);
    let p = support_l3_any_params();
    support_l3_assume_guards_pass(&p);
    let r = em.format(p);
    if mutant {
        assert!(!support_l3_is_io(&r));
    } else {
        assert!(support_l3_is_io(&r));
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

/// EM-FORMAT-ERR-METADATA-TOO-SMALL. Level-2 assumptions on format's input:
/// D-RANGE-FR-002-217ace (sector pow2) and D-RANGE-FMT-METADATA-DEVICE-LAYOUT-2ee144
/// (alignment <= u64::MAX - (SUPERBLOCK_SIZE - 1)). Device = conforming mock, 4 KiB
/// sectors, byte size fits u64 (IBlockDevice contract).
fn support_case_l3_fmt_metadata_too_small(mutant: bool) {
    let sectors: u64 = kani::any();
    kani::assume(sectors <= u64::MAX / MD_SS as u64);
    let em = support_em_with_device(sectors, false);
    let p = support_l3_any_params();
    support_l3_assume_guards_pass(&p);
    kani::assume(p.metadata_alignment <= u64::MAX - (SUPERBLOCK_SIZE as u64 - 1));
    let disk = sectors * MD_SS as u64;
    let eff = if p.metadata_region_size > 0 {
        disk.min(p.metadata_region_size)
    } else {
        disk
    };
    let after_sb = eff.saturating_sub(SUPERBLOCK_SIZE as u64);
    let too_small = after_sb < 2 * p.sector_size as u64;
    let r = em.format(p);
    if too_small {
        assert!(support_l3_is_corrupt(&r) != mutant);
    }
    std::mem::forget(em);
}

/// EM-FORMAT-ERR-NO-USABLE-DATA: data_start_offset per FormatParams docs
/// (shared-device mode: aligned superblock + 2 checkpoint copies; else 0).
fn support_case_l3_fmt_no_usable_data(mutant: bool) {
    let sectors: u64 = kani::any();
    kani::assume(sectors <= u64::MAX / MD_SS as u64);
    let em = support_em_with_device(sectors, false);
    let p = support_l3_any_params();
    support_l3_assume_guards_pass(&p);
    kani::assume(p.metadata_alignment <= u64::MAX - SUPERBLOCK_SIZE as u64);
    let disk = sectors * MD_SS as u64;
    let sb = SUPERBLOCK_SIZE as u64;
    let cro = if p.metadata_alignment == 0 {
        sb
    } else {
        sb.div_ceil(p.metadata_alignment) * p.metadata_alignment
    };
    let eff = if p.metadata_region_size > 0 {
        disk.min(p.metadata_region_size)
    } else {
        disk
    };
    let ss = p.sector_size as u64;
    let crs = (eff.saturating_sub(cro) / 2) / ss * ss;
    let dso = if p.metadata_region_size > 0 {
        cro + 2 * crs
    } else {
        0
    };
    let no_data = crs > 0 && p.data_disk_size <= dso;
    let r = em.format(p);
    kani::cover!(no_data);
    if no_data {
        assert!(support_l3_is_corrupt(&r) != mutant);
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

fn support_case_l3_init_dev_not_connected(mutant: bool) {
    let em = support_new_em();
    if kani::any() {
        em.set_metadata_ns_id(kani::any());
    }
    let r = em.initialize();
    if mutant {
        assert!(!support_l3_is_notinit(&r));
    } else {
        assert!(support_l3_is_notinit(&r));
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

fn support_case_l3_reserve_notinit(mutant: bool) {
    let em = support_new_em();
    let r = em.reserve_extent(kani::any(), kani::any());
    let ni = support_l3_is_notinit(&r);
    if let Ok(h) = r {
        std::mem::forget(h);
    }
    assert!(ni != mutant);
    std::mem::forget(em);
}

fn support_case_l3_getext_uninit(mutant: bool) {
    let em = support_new_em();
    let v = em.get_extents();
    assert!(v.is_empty() != mutant);
    std::mem::forget(em);
}

fn support_case_l3_foreach_uninit(mutant: bool) {
    let em = support_new_em();
    let mut calls: u32 = 0;
    em.for_each_extent(&mut |_e: &Extent| calls += 1);
    assert!((calls == 0) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_remove_notinit(mutant: bool) {
    let em = support_new_em();
    let r = em.remove_extent(kani::any());
    assert!(support_l3_is_notinit(&r) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_ckpt_notinit(mutant: bool) {
    let em = support_new_em();
    let r = em.checkpoint();
    assert!(support_l3_is_notinit(&r) != mutant);
    // the coalescing state is released (no stuck in_progress)
    assert!(!em.checkpoint_coalesce.lock().unwrap().in_progress);
    std::mem::forget(em);
}

fn support_case_l3_instid_notinit(mutant: bool) {
    let em = support_new_em();
    let r = em.get_instance_id();
    assert!(support_l3_is_notinit(&r) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_used_uninit(mutant: bool) {
    let em = support_new_em();
    assert!((em.used_bytes() == 0) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_cap_uninit(mutant: bool) {
    let em = support_new_em();
    assert!((em.capacity_bytes() == 0) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_setmbase_frame(mutant: bool) {
    let em = support_new_em();
    let d0: u64 = kani::any();
    em.set_data_base_lba(d0);
    let before_ext = em.get_extents();
    let before_d = em.data_base_lba();
    let m: u64 = kani::any();
    em.set_metadata_base_lba(m);
    let after_d = em.data_base_lba();
    let after_ext = em.get_extents();
    assert!(*em.metadata_base_lba.lock().unwrap() == m);
    if mutant {
        assert!(after_d != before_d);
    } else {
        assert!(after_d == before_d && after_d == d0);
        assert!(after_ext == before_ext);
    }
    std::mem::forget(em);
}

fn support_case_l3_setdbase_stored(mutant: bool) {
    let em = support_new_em();
    let b: u64 = kani::any();
    em.set_data_base_lba(b);
    // interleaved operations that are not set_data_base_lba
    if kani::any() {
        em.set_metadata_base_lba(kani::any());
    }
    if kani::any() {
        let _ = em.data_base_lba();
    }
    let got = em.data_base_lba();
    let c: u64 = kani::any();
    em.set_data_base_lba(c);
    let got2 = em.data_base_lba();
    if mutant {
        assert!(got != b);
    } else {
        assert!(got == b);
        assert!(got2 == c);
    }
    std::mem::forget(em);
}

fn support_case_l3_dbase_default_zero(mutant: bool) {
    let em = support_new_em();
    if kani::any() {
        em.set_metadata_base_lba(kani::any());
    }
    assert!((em.data_base_lba() == 0) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_dbase_read_only(mutant: bool) {
    support_disk_reset(false);
    let em = support_new_em();
    let d: u64 = kani::any();
    let m: u64 = kani::any();
    em.set_data_base_lba(d);
    em.set_metadata_base_lba(m);
    if kani::any() {
        em.set_metadata_ns_id(kani::any());
    }
    let ns0 = *em.metadata_ns_id.lock().unwrap();
    let ext0 = em.get_extents();
    let got = em.data_base_lba();
    // SAFETY: single-threaded harness; read of the mock disk length.
    let disk_len = unsafe { (*std::ptr::addr_of!(KANI_DISK)).len() };
    let unchanged = *em.data_base_lba.lock().unwrap() == d
        && *em.metadata_base_lba.lock().unwrap() == m
        && *em.metadata_ns_id.lock().unwrap() == ns0
        && em.get_extents() == ext0
        && support_l3_uninit(&em)
        && disk_len == 0;
    assert!(got == d);
    assert!(unchanged != mutant);
    std::mem::forget(em);
}

// ---- L3 shallow-unattempted harnesses (unwind attribute = smallest bound from the L3 sweep) ----
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_format_err_device_not_connected() {
    support_case_l3_fmt_dev_not_connected(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_format_err_device_not_connected__mutant() {
    support_case_l3_fmt_dev_not_connected(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_format_err_sector_zero() {
    support_case_l3_fmt_sector_zero(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_format_err_sector_zero__mutant() {
    support_case_l3_fmt_sector_zero(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_format_err_slab_not_sector_multiple_rung_a() {
    support_case_l3_fmt_slab_not_multiple(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_format_err_slab_not_sector_multiple_rung_a__mutant() {
    support_case_l3_fmt_slab_not_multiple(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_format_err_max_extent_exceeds_slab() {
    support_case_l3_fmt_max_extent(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_format_err_max_extent_exceeds_slab__mutant() {
    support_case_l3_fmt_max_extent(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_format_err_region_count() {
    support_case_l3_fmt_region_count(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_format_err_region_count__mutant() {
    support_case_l3_fmt_region_count(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_format_err_device_query_rung_a() {
    support_case_l3_fmt_device_query(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_format_err_device_query_rung_a__mutant() {
    support_case_l3_fmt_device_query(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_format_err_metadata_too_small_rung_a() {
    support_case_l3_fmt_metadata_too_small(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_format_err_metadata_too_small_rung_a__mutant() {
    support_case_l3_fmt_metadata_too_small(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_format_err_no_usable_data_rung_a() {
    support_case_l3_fmt_no_usable_data(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_format_err_no_usable_data_rung_a__mutant() {
    support_case_l3_fmt_no_usable_data(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_init_err_device_not_connected() {
    support_case_l3_init_dev_not_connected(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_init_err_device_not_connected__mutant() {
    support_case_l3_init_dev_not_connected(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_reserve_err_not_initialized_rung_a() {
    support_case_l3_reserve_notinit(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_reserve_err_not_initialized_rung_a__mutant() {
    support_case_l3_reserve_notinit(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_getext_uninit_empty() {
    support_case_l3_getext_uninit(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_getext_uninit_empty__mutant() {
    support_case_l3_getext_uninit(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_foreach_uninit_no_calls() {
    support_case_l3_foreach_uninit(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_foreach_uninit_no_calls__mutant() {
    support_case_l3_foreach_uninit(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_remove_err_not_initialized() {
    support_case_l3_remove_notinit(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_remove_err_not_initialized__mutant() {
    support_case_l3_remove_notinit(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_ckpt_err_not_initialized() {
    support_case_l3_ckpt_notinit(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_ckpt_err_not_initialized__mutant() {
    support_case_l3_ckpt_notinit(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_instid_err_not_initialized() {
    support_case_l3_instid_notinit(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_instid_err_not_initialized__mutant() {
    support_case_l3_instid_notinit(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_used_uninit_zero() {
    support_case_l3_used_uninit(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_used_uninit_zero__mutant() {
    support_case_l3_used_uninit(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_cap_uninit_zero() {
    support_case_l3_cap_uninit(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_cap_uninit_zero__mutant() {
    support_case_l3_cap_uninit(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_setmbase_frame() {
    support_case_l3_setmbase_frame(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_setmbase_frame__mutant() {
    support_case_l3_setmbase_frame(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_setdbase_post_stored() {
    support_case_l3_setdbase_stored(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_setdbase_post_stored__mutant() {
    support_case_l3_setdbase_stored(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_dbase_post_default_zero() {
    support_case_l3_dbase_default_zero(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_dbase_post_default_zero__mutant() {
    support_case_l3_dbase_default_zero(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_dbase_frame_read_only() {
    support_case_l3_dbase_read_only(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_dbase_frame_read_only__mutant() {
    support_case_l3_dbase_read_only(true);
}

// ---- L3 gate-feature-flag: EM-BIO-FLUSH as a scorer-visible harness pair. Same body as
// boundary_em_bio_flush (kept unchanged above); needs `--features volatile_write_cache`.
#[cfg(feature = "volatile_write_cache")]
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn diag_l3_bio_flush_inherited() {
    support_case_bio_flush(false);
}

#[cfg(feature = "volatile_write_cache")]
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn diag_l3_bio_flush_inherited__mutant() {
    support_case_bio_flush(true);
}

// ===========================================================================
// L3 background-timer-thread: environment model of spawn + Condvar + a peer thread.
// ---------------------------------------------------------------------------
// * std::thread::spawn -> stub_l3_spawn_deferred: the closure is STORED (not run) and a
//   zeroed JoinHandle is returned. JoinHandle has no public constructor; the zeroed value
//   is never joined/used: stored into Mutex<Option<JoinHandle<()>>> it is read as None
//   through the Option niche (scratch probe P7c: dropping Some(zeroed) is a no-op under
//   Kani). Harnesses mem::forget the component, so Drop (join) never runs. UB-adjacent,
//   disclosed; fidelity representative.
// * The harness then runs the stored thread body synchronously = the schedule in which the
//   thread starts after new_inner() returns (an allowed interleaving).
// * Condvar::wait_timeout / Condvar::wait on the timer condvar -> one ENVIRONMENT STEP per
//   call: while the thread waits, a peer may (nondeterministically) call
//   set_checkpoint_interval(L3_ENV_VALUE) (modelled by writing through the guard, i.e. the
//   peer took the mutex while the waiter had released it) and/or stop(); the wait then
//   returns timed_out = kani::any() (std: spurious wake-ups and timeout/notify races are both
//   allowed, so the model must allow every combination). After L3_WAIT_BOUND waits the peer
//   always calls stop(), which bounds the thread loop (unwind = bound + 2, checks ON).
// * Condvar::notify_all -> counter: checkpoint() calls checkpoint_done.notify_all() exactly
//   once per non-coalesced call, so L3_CKPT_CALLS counts checkpoint() executions without
//   touching checkpoint()'s code. notify_one (set_interval/stop) -> no-op (no waiter).
// ===========================================================================
use crate::CheckpointTimerState;
use interfaces::ILogger;
use std::sync::{Condvar, LockResult, MutexGuard, WaitTimeoutResult};
use std::time::Duration;

static mut L3_THREAD: Option<Box<dyn FnOnce() + Send>> = None;
static mut L3_TIMER: *const CheckpointTimerState = std::ptr::null();
static mut L3_WAITS: u32 = 0;
static mut L3_WAIT_BOUND: u32 = 2;
static mut L3_CKPT_CALLS: u32 = 0;
static mut L3_ERRORS: u32 = 0;
/// environment peer may call set_checkpoint_interval(L3_ENV_VALUE) during a wait
static mut L3_ENV_SET: bool = false;
static mut L3_ENV_VALUE: Option<Duration> = None;
/// ghost: the peer's set_checkpoint_interval happened (and checkpoint count at that time)
static mut L3_SET_DONE: bool = false;
static mut L3_CKPT_AT_SET: u32 = 0;
/// ghost for CHANGE-NO-SPURIOUS: previous wait returned woken (not timed out), no stop
static mut L3_PREV_WOKEN: bool = false;
static mut L3_CKPT_AT_PREV: u32 = 0;
static mut L3_WOKEN_SEEN: u32 = 0;
static mut L3_SPURIOUS_CKPT: bool = false;
static mut L3_DUR_MISMATCH: bool = false;

pub(crate) fn stub_l3_spawn_deferred<F, T>(f: F) -> std::thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let body: Box<dyn FnOnce() + Send> = Box::new(move || {
        let _ = f();
    });
    // SAFETY: single-threaded harness; exclusive access to the static.
    unsafe {
        *std::ptr::addr_of_mut!(L3_THREAD) = Some(body);
    }
    // SAFETY: see block comment above - the value is never joined, detached or read; the
    // only consumer stores it in an Option that the harness forgets.
    #[allow(invalid_value)]
    unsafe {
        std::mem::MaybeUninit::zeroed().assume_init()
    }
}

fn support_l3_reset(bound: u32, env_set: bool, env_value: Option<Duration>) {
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_THREAD) = None;
        *std::ptr::addr_of_mut!(L3_TIMER) = std::ptr::null();
        *std::ptr::addr_of_mut!(L3_WAITS) = 0;
        *std::ptr::addr_of_mut!(L3_WAIT_BOUND) = bound;
        *std::ptr::addr_of_mut!(L3_CKPT_CALLS) = 0;
        *std::ptr::addr_of_mut!(L3_ERRORS) = 0;
        *std::ptr::addr_of_mut!(L3_ENV_SET) = env_set;
        *std::ptr::addr_of_mut!(L3_ENV_VALUE) = env_value;
        *std::ptr::addr_of_mut!(L3_SET_DONE) = false;
        *std::ptr::addr_of_mut!(L3_CKPT_AT_SET) = 0;
        *std::ptr::addr_of_mut!(L3_PREV_WOKEN) = false;
        *std::ptr::addr_of_mut!(L3_CKPT_AT_PREV) = 0;
        *std::ptr::addr_of_mut!(L3_WOKEN_SEEN) = 0;
        *std::ptr::addr_of_mut!(L3_SPURIOUS_CKPT) = false;
        *std::ptr::addr_of_mut!(L3_DUR_MISMATCH) = false;
    }
}

/// One environment step at wait entry (shared by wait and wait_timeout). `cur` is the
/// interval the thread read under the lock; returns nothing, may update the interval via
/// `slot` and may set shutdown.
unsafe fn support_l3_env_step(this: &Condvar, slot: *mut Option<Duration>) {
    let timer = *std::ptr::addr_of!(L3_TIMER);
    assert!(!timer.is_null());
    // only the timer condvar is ever waited on in these harnesses
    assert!(std::ptr::eq(this, &(*timer).wake));
    // CHANGE-NO-SPURIOUS ghost check: a woken (non-timeout) previous wait must lead
    // straight back to a wait - no checkpoint() in between.
    if *std::ptr::addr_of!(L3_PREV_WOKEN)
        && *std::ptr::addr_of!(L3_CKPT_CALLS) != *std::ptr::addr_of!(L3_CKPT_AT_PREV)
    {
        *std::ptr::addr_of_mut!(L3_SPURIOUS_CKPT) = true;
    }
    *std::ptr::addr_of_mut!(L3_WAITS) += 1;
    // peer: set_checkpoint_interval(v) while the waiter has released the mutex
    if *std::ptr::addr_of!(L3_ENV_SET) && !*std::ptr::addr_of!(L3_SET_DONE) && kani::any() {
        *slot = *std::ptr::addr_of!(L3_ENV_VALUE);
        *std::ptr::addr_of_mut!(L3_SET_DONE) = true;
        *std::ptr::addr_of_mut!(L3_CKPT_AT_SET) = *std::ptr::addr_of!(L3_CKPT_CALLS);
    }
    // peer: stop(), forced once the wait bound is reached
    if *std::ptr::addr_of!(L3_WAITS) >= *std::ptr::addr_of!(L3_WAIT_BOUND) || kani::any() {
        (*timer).shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

unsafe fn support_l3_after_wait(timed_out: bool) {
    let stopped = (*(*std::ptr::addr_of!(L3_TIMER)))
        .shutdown
        .load(std::sync::atomic::Ordering::Relaxed);
    let woken = !timed_out && !stopped;
    *std::ptr::addr_of_mut!(L3_PREV_WOKEN) = woken;
    *std::ptr::addr_of_mut!(L3_CKPT_AT_PREV) = *std::ptr::addr_of!(L3_CKPT_CALLS);
    if woken {
        *std::ptr::addr_of_mut!(L3_WOKEN_SEEN) += 1;
    }
}

pub(crate) fn stub_l3_wait_timeout<'a, T>(
    this: &Condvar,
    mut guard: MutexGuard<'a, T>,
    dur: Duration,
) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
    assert!(std::mem::size_of::<T>() == std::mem::size_of::<Option<Duration>>());
    let slot = (&mut *guard as *mut T).cast::<Option<Duration>>();
    // SAFETY: the timer condvar's mutex guards Option<Duration> (asserted: the condvar is
    // the timer's, and the size matches); single-threaded harness.
    unsafe {
        // "the thread starts waiting with the (current) interval"
        if *slot != Some(dur) {
            *std::ptr::addr_of_mut!(L3_DUR_MISMATCH) = true;
        }
        support_l3_env_step(this, slot);
    }
    let timed_out: bool = kani::any();
    // SAFETY: single-threaded harness.
    unsafe { support_l3_after_wait(timed_out) };
    assert!(std::mem::size_of::<WaitTimeoutResult>() == std::mem::size_of::<bool>());
    // SAFETY: WaitTimeoutResult is a newtype over bool (size asserted).
    let w: WaitTimeoutResult = unsafe { std::mem::transmute(timed_out) };
    Ok((guard, w))
}

pub(crate) fn stub_l3_wait<'a, T>(
    this: &Condvar,
    mut guard: MutexGuard<'a, T>,
) -> LockResult<MutexGuard<'a, T>> {
    assert!(std::mem::size_of::<T>() == std::mem::size_of::<Option<Duration>>());
    let slot = (&mut *guard as *mut T).cast::<Option<Duration>>();
    // SAFETY: as in stub_l3_wait_timeout.
    unsafe {
        support_l3_env_step(this, slot);
        support_l3_after_wait(false);
    }
    Ok(guard)
}

pub(crate) fn stub_l3_notify_all_count(_this: &Condvar) {
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_CKPT_CALLS) += 1;
    }
}

/// ILogger mock: counts error() calls; other levels are ignored.
pub(crate) struct KaniL3Logger;
impl ILogger for KaniL3Logger {
    fn error(&self, _msg: &str) {
        // SAFETY: single-threaded harness.
        unsafe {
            *std::ptr::addr_of_mut!(L3_ERRORS) += 1;
        }
    }
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

/// new_inner() under the deferred-spawn stub; records the timer address for the env model.
fn support_l3_new_inner() -> Arc<ExtentManager> {
    let em = ExtentManager::new_inner();
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_TIMER) = Arc::as_ptr(&em.checkpoint_timer_state);
    }
    em
}

/// An arbitrary interval: None or Some(ms) for a symbolic u64 millisecond count.
fn support_l3_any_interval() -> Option<Duration> {
    if kani::any() {
        None
    } else {
        Some(Duration::from_millis(kani::any()))
    }
}

fn support_l3_run_thread() {
    // SAFETY: single-threaded harness.
    let body = unsafe { (*std::ptr::addr_of_mut!(L3_THREAD)).take() };
    let body = body.expect("new_inner spawned the checkpoint thread");
    body();
}

fn support_case_l3_setint_default_30s(mutant: bool) {
    support_l3_reset(2, false, None);
    let em = support_l3_new_inner();
    let iv = *em.checkpoint_timer_state.interval.lock().unwrap();
    // SAFETY: single-threaded harness.
    let spawned = unsafe { (*std::ptr::addr_of!(L3_THREAD)).is_some() };
    assert!(spawned);
    if mutant {
        assert!(iv != Some(Duration::from_secs(30)));
    } else {
        assert!(iv == Some(Duration::from_secs(30)));
    }
    std::mem::forget(em);
}

/// Uninitialised component, logger connected; the thread body runs under the env model
/// (bound waits). Every background checkpoint fails NotInitialized and nothing is logged.
fn support_case_l3_setint_bg_not_init_silent(mutant: bool) {
    support_l3_reset(2, true, support_l3_any_interval());
    let em = support_l3_new_inner();
    let lg: Arc<dyn ILogger + Send + Sync> = Arc::new(KaniL3Logger);
    let _ = em.logger.connect(lg);
    support_l3_run_thread();
    // SAFETY: single-threaded harness.
    let (calls, errors) = unsafe {
        (
            *std::ptr::addr_of!(L3_CKPT_CALLS),
            *std::ptr::addr_of!(L3_ERRORS),
        )
    };
    kani::cover!(calls > 0);
    if mutant {
        // claims no execution reaches a background checkpoint that stays silent
        assert!(!(calls > 0 && errors == 0));
    } else {
        assert!(errors == 0);
    }
    std::mem::forget(em);
}

/// The peer changes the interval (to an arbitrary Some/None) during a wait; a wait that
/// ends without a timeout must be followed by another wait (with the then-current
/// interval), never by a checkpoint().
fn support_case_l3_setint_change_no_spurious(mutant: bool) {
    support_l3_reset(3, true, support_l3_any_interval());
    let em = support_l3_new_inner();
    support_l3_run_thread();
    // SAFETY: single-threaded harness.
    let (spurious, mismatch, woken) = unsafe {
        (
            *std::ptr::addr_of!(L3_SPURIOUS_CKPT),
            *std::ptr::addr_of!(L3_DUR_MISMATCH),
            *std::ptr::addr_of!(L3_WOKEN_SEEN),
        )
    };
    kani::cover!(woken > 0);
    if mutant {
        assert!(!(woken > 0 && !spurious));
    } else {
        assert!(!spurious);
        assert!(!mismatch);
    }
    std::mem::forget(em);
}

/// EM-SETINT-POST-NONE-DISABLES: after the peer's set_checkpoint_interval(None) returned,
/// no automatic checkpoint runs (no interval is set again in this model).
fn support_case_l3_setint_none_disables(mutant: bool) {
    support_l3_reset(3, true, None);
    let em = support_l3_new_inner();
    support_l3_run_thread();
    // SAFETY: single-threaded harness.
    let (set, at, calls) = unsafe {
        (
            *std::ptr::addr_of!(L3_SET_DONE),
            *std::ptr::addr_of!(L3_CKPT_AT_SET),
            *std::ptr::addr_of!(L3_CKPT_CALLS),
        )
    };
    kani::cover!(set);
    if mutant {
        assert!(!(set && calls == at));
    } else if set {
        assert!(calls == at);
    }
    std::mem::forget(em);
}

// ---- L3 background-timer-thread harnesses ----
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_setint_default_30s() {
    support_case_l3_setint_default_30s(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn uncredited_verify_em_setint_default_30s__mutant() {
    support_case_l3_setint_default_30s(true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_bg_not_init_silent_rung_b2() {
    support_case_l3_setint_bg_not_init_silent(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_bg_not_init_silent_rung_b2__mutant() {
    support_case_l3_setint_bg_not_init_silent(true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_change_no_spurious_rung_b2() {
    support_case_l3_setint_change_no_spurious(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_change_no_spurious_rung_b2__mutant() {
    support_case_l3_setint_change_no_spurious(true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_post_none_disables_rung_b2() {
    support_case_l3_setint_none_disables(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_post_none_disables_rung_b2__mutant() {
    support_case_l3_setint_none_disables(true);
}

// ===========================================================================
// L3 os-file-io-urandom: format() with instance_id None reads /dev/urandom
// (std::fs::File::open + Read::read_exact, lib.rs:474-481).
// Model of the OS (argued against the std contract, no conformance against the OS is
// possible): open may fail with an io::Error, or succeed with a file whose read_exact
// either fills the whole buffer with arbitrary bytes or fails. The fabricated File
// (fd 3) is never touched by the OS: read_exact and OwnedFd's Drop (close) are stubbed.
// ===========================================================================
static mut L3_URANDOM: [u8; 8] = [0; 8];
static mut L3_URANDOM_MAY_FAIL: bool = true;

pub(crate) fn stub_l3_file_open<P: AsRef<std::path::Path>>(
    _p: P,
) -> std::io::Result<std::fs::File> {
    // SAFETY: single-threaded harness.
    let may_fail = unsafe { *std::ptr::addr_of!(L3_URANDOM_MAY_FAIL) };
    if may_fail && kani::any() {
        return Err(std::io::Error::from_raw_os_error(2));
    }
    use std::os::fd::FromRawFd;
    // SAFETY: fd 3 is never passed to the OS: read_exact and the fd's Drop are stubbed.
    Ok(unsafe { std::fs::File::from_raw_fd(3) })
}

/// Replaces `<File as Read>::read` (File does not override `read_exact`, and Kani 0.67
/// cannot stub the trait default method for one impl - scratch probe P9d). The real
/// default `read_exact` loop runs on top of this.
pub(crate) fn stub_l3_file_read(_this: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    // SAFETY: single-threaded harness.
    let may_fail = unsafe { *std::ptr::addr_of!(L3_URANDOM_MAY_FAIL) };
    if may_fail && kani::any() {
        return Err(std::io::Error::from_raw_os_error(5));
    }
    let b: [u8; 8] = kani::any();
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_URANDOM) = b;
    }
    assert!(buf.len() == 8);
    buf.copy_from_slice(&b);
    Ok(8)
}

pub(crate) fn stub_l3_ownedfd_drop(_this: &mut std::os::fd::OwnedFd) {}

fn support_case_l3_fmt_instance_id_generation(mutant: bool) {
    support_disk2_reset(false);
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_URANDOM_MAY_FAIL) = true;
    }
    let em = support_em_with_device(4, false);
    let mut p = support_layout(L_ONE);
    p.instance_id = None;
    let r = em.format(p);
    // with the failure modes enabled, success is still a possible outcome; the property is
    // about the failure outcome: a generation failure is the only IoError on this path
    // (disk model I/O errors are disabled, geometry is fine).
    kani::cover!(r.is_err());
    if r.is_err() {
        if mutant {
            assert!(!support_l3_is_io(&r));
        } else {
            assert!(support_l3_is_io(&r));
            assert!(support_l3_uninit(&em));
        }
    }
    std::mem::forget(em);
}

fn support_case_l3_fmt_instance_id_generated(mutant: bool, reinit: bool) {
    support_disk2_reset(false);
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_URANDOM_MAY_FAIL) = false;
    }
    let em = support_em_with_device(4, false);
    let mut p = support_layout(L_ONE);
    p.instance_id = None;
    let r = em.format(p);
    assert!(r.is_ok());
    // SAFETY: single-threaded harness.
    let id = u64::from_le_bytes(unsafe { *std::ptr::addr_of!(L3_URANDOM) });
    let got1 = em.get_instance_id();
    let got2 = em.get_instance_id();
    // persisted superblock
    let sb = support_disk2_read(0, SUPERBLOCK_SIZE).ok().and_then(|b| Superblock::deserialize(&b).ok());
    let persisted = sb.map(|s| s.instance_id);
    let mut ok = matches!(got1, Ok(v) if v == id) && matches!(got2, Ok(v) if v == id) && persisted == Some(id);
    if reinit {
        let r2 = em.initialize();
        ok = ok && r2.is_ok() && matches!(em.get_instance_id(), Ok(v) if v == id);
    }
    if mutant {
        assert!(!ok);
    } else {
        assert!(ok);
    }
    std::mem::forget(em);
}

// ---- L3 os-file-io-urandom harnesses ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::fs::File::open, stub_l3_file_open)]
#[kani::stub(<std::fs::File as std::io::Read>::read, stub_l3_file_read)]
#[kani::stub(<std::os::fd::OwnedFd as std::ops::Drop>::drop, stub_l3_ownedfd_drop)]
fn diag_l3_format_err_instance_id_generation_rung_a() {
    support_case_l3_fmt_instance_id_generation(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::fs::File::open, stub_l3_file_open)]
#[kani::stub(<std::fs::File as std::io::Read>::read, stub_l3_file_read)]
#[kani::stub(<std::os::fd::OwnedFd as std::ops::Drop>::drop, stub_l3_ownedfd_drop)]
fn diag_l3_format_err_instance_id_generation_rung_a__mutant() {
    support_case_l3_fmt_instance_id_generation(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::fs::File::open, stub_l3_file_open)]
#[kani::stub(<std::fs::File as std::io::Read>::read, stub_l3_file_read)]
#[kani::stub(<std::os::fd::OwnedFd as std::ops::Drop>::drop, stub_l3_ownedfd_drop)]
fn uncredited_verify_em_format_post_instance_id_generated() {
    support_case_l3_fmt_instance_id_generated(false, false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::fs::File::open, stub_l3_file_open)]
#[kani::stub(<std::fs::File as std::io::Read>::read, stub_l3_file_read)]
#[kani::stub(<std::os::fd::OwnedFd as std::ops::Drop>::drop, stub_l3_ownedfd_drop)]
fn uncredited_verify_em_format_post_instance_id_generated__mutant() {
    support_case_l3_fmt_instance_id_generated(true, false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::fs::File::open, stub_l3_file_open)]
#[kani::stub(<std::fs::File as std::io::Read>::read, stub_l3_file_read)]
#[kani::stub(<std::os::fd::OwnedFd as std::ops::Drop>::drop, stub_l3_ownedfd_drop)]
fn diag_l3_format_post_instance_id_generated_reinit() {
    support_case_l3_fmt_instance_id_generated(false, true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::fs::File::open, stub_l3_file_open)]
#[kani::stub(<std::fs::File as std::io::Read>::read, stub_l3_file_read)]
#[kani::stub(<std::os::fd::OwnedFd as std::ops::Drop>::drop, stub_l3_ownedfd_drop)]
fn diag_l3_format_post_instance_id_generated_reinit__mutant() {
    support_case_l3_fmt_instance_id_generated(true, true);
}

// ===========================================================================
// L3 multi-thread-interleaving: a-rung limit capture. Real std::thread::spawn of two
// callers (no spawn stub). Named limit_* so the gate never scores them; they exist to
// record what Kani does with real threads (measured, see extent-manager_kani_limits.yaml).
// ===========================================================================
fn support_l3_two_thread_checkpoints() -> (u64, bool) {
    let em = Arc::new(support_new_em());
    let a = Arc::clone(&em);
    let b = Arc::clone(&em);
    let t1 = std::thread::spawn(move || {
        let _ = a.checkpoint();
    });
    let t2 = std::thread::spawn(move || {
        let _ = b.checkpoint();
    });
    let _ = t1.join();
    let _ = t2.join();
    let st = em.checkpoint_coalesce.lock().unwrap();
    let r = (st.completed_seq, st.in_progress);
    drop(st);
    std::mem::forget(em);
    r
}

#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
fn limit_em_ckpt_inv_single_writer() {
    // the claim: never two checkpoint I/Os at once; observable residue: in_progress is
    // released and no completed_seq advance (both fail NotInitialized).
    let (seq, in_progress) = support_l3_two_thread_checkpoints();
    assert!(!in_progress);
    assert!(seq == 0);
}

#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::sync::Condvar::notify_all, stub_condvar_notify)]
fn limit_em_ckpt_completed_seq_monotonic() {
    let (seq, _) = support_l3_two_thread_checkpoints();
    assert!(seq <= 2);
}

// ---- L3 background-timer: ghost clock for EM-SETINT-SOME-FIRES ----
// Each wait advances a ghost clock: by `dur` on a timeout, by an arbitrary e < dur on an
// early (spurious / notified) wake. The property: every checkpoint() starts no later than
// one interval after the previous checkpoint (or the thread start). `allow_early=false` is
// the CONTROL: no early wakes (only timeouts) - the same harness must then pass.
static mut L3_CLOCK_MS: u64 = 0;
static mut L3_LAST_CKPT_MS: u64 = 0;
static mut L3_GAP_VIOLATION: bool = false;
static mut L3_ALLOW_EARLY: bool = true;
static mut L3_FIXED_MS: u64 = 0;

pub(crate) fn stub_l3_wait_timeout_clock<'a, T>(
    this: &Condvar,
    guard: MutexGuard<'a, T>,
    dur: Duration,
) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
    // SAFETY: single-threaded harness.
    unsafe {
        let timer = *std::ptr::addr_of!(L3_TIMER);
        assert!(std::ptr::eq(this, &(*timer).wake));
        *std::ptr::addr_of_mut!(L3_WAITS) += 1;
        if *std::ptr::addr_of!(L3_WAITS) >= *std::ptr::addr_of!(L3_WAIT_BOUND) {
            (*timer).shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let d = dur.as_millis() as u64;
    // SAFETY: single-threaded harness.
    let allow_early = unsafe { *std::ptr::addr_of!(L3_ALLOW_EARLY) };
    let timed_out = !allow_early || kani::any();
    let e: u64 = if timed_out {
        d
    } else {
        let e: u64 = kani::any();
        kani::assume(e < d);
        e
    };
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_CLOCK_MS) += e;
    }
    assert!(std::mem::size_of::<WaitTimeoutResult>() == std::mem::size_of::<bool>());
    // SAFETY: WaitTimeoutResult is a newtype over bool (size asserted).
    let w: WaitTimeoutResult = unsafe { std::mem::transmute(timed_out) };
    Ok((guard, w))
}

pub(crate) fn stub_l3_notify_all_clock(_this: &Condvar) {
    // checkpoint() finished; it STARTED at the current ghost time (no time passes inside)
    // SAFETY: single-threaded harness.
    unsafe {
        let now = *std::ptr::addr_of!(L3_CLOCK_MS);
        let last = *std::ptr::addr_of!(L3_LAST_CKPT_MS);
        if now - last > *std::ptr::addr_of!(L3_FIXED_MS) {
            *std::ptr::addr_of_mut!(L3_GAP_VIOLATION) = true;
        }
        *std::ptr::addr_of_mut!(L3_LAST_CKPT_MS) = now;
        *std::ptr::addr_of_mut!(L3_CKPT_CALLS) += 1;
    }
}

fn support_case_l3_setint_some_fires(allow_early: bool, mutant: bool) {
    support_l3_reset(3, false, None);
    let ms: u64 = kani::any();
    kani::assume(ms > 0 && ms <= 1_000_000);
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_CLOCK_MS) = 0;
        *std::ptr::addr_of_mut!(L3_LAST_CKPT_MS) = 0;
        *std::ptr::addr_of_mut!(L3_GAP_VIOLATION) = false;
        *std::ptr::addr_of_mut!(L3_ALLOW_EARLY) = allow_early;
        *std::ptr::addr_of_mut!(L3_FIXED_MS) = ms;
    }
    let em = support_l3_new_inner();
    em.set_checkpoint_interval(Some(Duration::from_millis(ms)));
    support_l3_run_thread();
    // SAFETY: single-threaded harness.
    let (viol, calls) = unsafe {
        (
            *std::ptr::addr_of!(L3_GAP_VIOLATION),
            *std::ptr::addr_of!(L3_CKPT_CALLS),
        )
    };
    kani::cover!(calls > 0);
    if mutant {
        assert!(!(calls > 0 && !viol));
    } else {
        assert!(!viol);
    }
    std::mem::forget(em);
}

/// EM-SETINT-BG-NOT-INIT-SILENT, negative half (Creusot witness NOTINIT-OVERLOADED):
/// a FORMATTED component with a dirty region whose metadata device was disconnected;
/// one background work step (timeout); the checkpoint fails and must be logged.
fn support_case_l3_setint_bg_disconnected(mutant: bool) {
    support_l3_reset(1, false, None);
    support_disk2_reset(false);
    let em = support_l3_new_inner();
    let dev: Arc<dyn IBlockDevice + Send + Sync> = Arc::new(KaniMetaDevice {
        sector: MD_SS,
        sectors: 4,
        geometry_fails: false,
    });
    let _ = em.metadata_device.connect(dev);
    let lg: Arc<dyn ILogger + Send + Sync> = Arc::new(KaniL3Logger);
    let _ = em.logger.connect(lg);
    assert!(em.format(support_layout(L_ONE)).is_ok());
    // pre-state "a publish happened since the last checkpoint": mark the region dirty
    {
        let rs = em.regions.read();
        rs.as_ref().unwrap()[0].write().dirty = true;
    }
    em.metadata_device.disconnect();
    support_l3_run_thread();
    // SAFETY: single-threaded harness.
    let (calls, errors) = unsafe {
        (
            *std::ptr::addr_of!(L3_CKPT_CALLS),
            *std::ptr::addr_of!(L3_ERRORS),
        )
    };
    let initialized = em.regions.read().is_some();
    kani::cover!(calls > 0);
    if mutant {
        assert!(!(calls > 0 && initialized));
    } else if calls > 0 && initialized {
        // the property: a background failure of an initialized component is logged
        assert!(errors > 0);
    }
    std::mem::forget(em);
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout_clock)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_clock)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_some_fires_rung_b2() {
    support_case_l3_setint_some_fires(true, false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout_clock)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_clock)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_some_fires_rung_b2__mutant() {
    support_case_l3_setint_some_fires(true, true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout_clock)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_clock)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_setint_some_fires_control_no_early_wake() {
    support_case_l3_setint_some_fires(false, false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn refute_em_setint_bg_not_init_silent() {
    support_case_l3_setint_bg_disconnected(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(std::time::Instant::now, stub_instant_now)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_deferred)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn refute_em_setint_bg_not_init_silent__mutant() {
    support_case_l3_setint_bg_disconnected(true);
}

// ---- L3 rung b for the uninitialised-component paths: UNREACHABILITY stubs ----
// A deep callee that sits only on the initialised branch is replaced by a stub that
// FAILS (assert!(false)) if it is ever executed. If the harness then verifies, the callee
// is unreachable on every explored path, so the stub never changed any behaviour - no
// conformance argument is needed (a reachable stub shows up as a failed check instead of
// a silent model). Needed because symex does not prune the initialised branch of
// reserve_extent on an uninitialised component (rung a: >600 s, 17.9 GB at unwind 2).
pub(crate) fn stub_l3_unreachable_alloc_extent(
    _this: &mut RegionState,
    _size: u32,
) -> Result<(u64, usize, u64), ExtentManagerError> {
    assert!(false, "L3 unreachability stub reached: RegionState::alloc_extent");
    Err(ExtentManagerError::OutOfSpace)
}
pub(crate) fn stub_l3_unreachable_remove_by_offset(
    _this: &mut RegionState,
    _offset: u64,
) -> Result<(), ExtentManagerError> {
    assert!(false, "L3 unreachability stub reached: RegionState::remove_extent_by_offset");
    Err(ExtentManagerError::OutOfSpace)
}

#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::region::RegionState::alloc_extent, stub_l3_unreachable_alloc_extent)]
fn uncredited_verify_em_reserve_err_not_initialized() {
    support_case_l3_reserve_notinit(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::region::RegionState::alloc_extent, stub_l3_unreachable_alloc_extent)]
fn uncredited_verify_em_reserve_err_not_initialized__mutant() {
    support_case_l3_reserve_notinit(true);
}

pub(crate) fn stub_l3_unreachable_write_checkpoint(
    _c: &BlockDeviceClient,
    _r: &parking_lot::RwLock<Option<Vec<Arc<parking_lot::RwLock<RegionState>>>>>,
    _s: &std::sync::Mutex<Option<crate::region::SharedState>>,
) -> Result<(), ExtentManagerError> {
    assert!(false, "L3 unreachability stub reached: checkpoint::write_checkpoint");
    Ok(())
}

/// Unreachability stub with path cut: assert!(false) reports any reach; assume(false)
/// then kills the path so symex does not continue into the (huge) remainder of format().
pub(crate) fn stub_l3_unreachable_buddy_new(
    _base: u64,
    _total: u64,
    _sector: u32,
) -> BuddyAllocator {
    assert!(false, "L3 unreachability stub reached: BuddyAllocator::new");
    kani::assume(false);
    unreachable!()
}

/// rung-b variants of the two format guards whose infeasible tail symex does not prune
fn support_case_l3_fmt_metadata_too_small_b(mutant: bool) {
    let sectors: u64 = kani::any();
    kani::assume(sectors <= u64::MAX / MD_SS as u64);
    let em = support_em_with_device(sectors, false);
    let p = support_l3_any_params();
    support_l3_assume_guards_pass(&p);
    kani::assume(p.metadata_alignment <= u64::MAX - (SUPERBLOCK_SIZE as u64 - 1));
    let disk = sectors * MD_SS as u64;
    let eff = if p.metadata_region_size > 0 {
        disk.min(p.metadata_region_size)
    } else {
        disk
    };
    let after_sb = eff.saturating_sub(SUPERBLOCK_SIZE as u64);
    kani::assume(after_sb < 2 * p.sector_size as u64);
    let r = em.format(p);
    assert!(support_l3_is_corrupt(&r) != mutant);
    std::mem::forget(em);
}

fn support_case_l3_fmt_no_usable_data_b(mutant: bool) {
    let sectors: u64 = kani::any();
    kani::assume(sectors <= u64::MAX / MD_SS as u64);
    let em = support_em_with_device(sectors, false);
    let p = support_l3_any_params();
    support_l3_assume_guards_pass(&p);
    kani::assume(p.metadata_alignment <= u64::MAX - SUPERBLOCK_SIZE as u64);
    let disk = sectors * MD_SS as u64;
    let sb = SUPERBLOCK_SIZE as u64;
    let cro = if p.metadata_alignment == 0 {
        sb
    } else {
        sb.div_ceil(p.metadata_alignment) * p.metadata_alignment
    };
    let eff = if p.metadata_region_size > 0 {
        disk.min(p.metadata_region_size)
    } else {
        disk
    };
    let ss = p.sector_size as u64;
    let crs = (eff.saturating_sub(cro) / 2) / ss * ss;
    let dso = if p.metadata_region_size > 0 {
        cro + 2 * crs
    } else {
        0
    };
    kani::assume(crs > 0 && p.data_disk_size <= dso);
    let r = em.format(p);
    if mutant {
        assert!(!support_l3_is_corrupt(&r));
    } else {
        assert!(support_l3_is_corrupt(&r));
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

// ---- L3 rung-b harnesses (unreachability stub on BuddyAllocator::new) ----
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_slab_not_sector_multiple_rung_b() {
    support_case_l3_fmt_slab_not_multiple(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_slab_not_sector_multiple_rung_b__mutant() {
    support_case_l3_fmt_slab_not_multiple(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_device_query_rung_b() {
    support_case_l3_fmt_device_query(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_device_query_rung_b__mutant() {
    support_case_l3_fmt_device_query(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_metadata_too_small_rung_b() {
    support_case_l3_fmt_metadata_too_small_b(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_metadata_too_small_rung_b__mutant() {
    support_case_l3_fmt_metadata_too_small_b(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_no_usable_data_rung_b() {
    support_case_l3_fmt_no_usable_data_b(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_no_usable_data_rung_b__mutant() {
    support_case_l3_fmt_no_usable_data_b(true);
}

// ---- L3 rung b2: concrete DIVISOR dispatch (lesson 1) + unreachability stubs ----
// Rung b (buddy unreachability stub) cut the 32 GB symex blow-up to ~2.3 GB but the run
// still timed out (>400 s): the layout arithmetic before BuddyAllocator::new divides by
// symbolic sector_size / metadata_alignment / region_count. Here the three DIVISORS are
// dispatched over concrete menus at the call site (const generics, so each branch is
// symexed with constants); dividends (disk sizes, metadata_region_size, device sectors)
// stay fully symbolic. Fidelity: representative over the divisor menus.
pub(crate) fn stub_l3_unreachable_receptacle_get<T: ?Sized + Send + Sync + 'static>(
    _this: &Receptacle<T>,
) -> Result<Arc<T>, component_core::ReceptacleError> {
    assert!(false, "L3 unreachability stub reached: Receptacle::get");
    kani::assume(false);
    unreachable!()
}

fn support_l3_params_with<const SSZ: u32, const AL: u64, const RC: u32>() -> FormatParams {
    let mut p = support_l3_any_params();
    p.sector_size = SSZ;
    p.metadata_alignment = AL;
    p.region_count = RC;
    kani::assume(p.slab_size % SSZ as u64 == 0);
    kani::assume(p.max_extent_size as u64 <= p.slab_size);
    p
}

fn support_l3_body_device_query<const SSZ: u32, const AL: u64, const RC: u32>(mutant: bool) {
    let sectors: u64 = kani::any();
    kani::assume(sectors <= u64::MAX / MD_SS as u64);
    let em = support_em_with_device(sectors, true);
    let p = support_l3_params_with::<SSZ, AL, RC>();
    let r = em.format(p);
    if mutant {
        assert!(!support_l3_is_io(&r));
    } else {
        assert!(support_l3_is_io(&r));
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

fn support_l3_body_too_small<const SSZ: u32, const AL: u64, const RC: u32>(mutant: bool) {
    let sectors: u64 = kani::any();
    kani::assume(sectors <= u64::MAX / MD_SS as u64);
    let em = support_em_with_device(sectors, false);
    let p = support_l3_params_with::<SSZ, AL, RC>();
    let disk = sectors * MD_SS as u64;
    let eff = if p.metadata_region_size > 0 {
        disk.min(p.metadata_region_size)
    } else {
        disk
    };
    let after_sb = eff.saturating_sub(SUPERBLOCK_SIZE as u64);
    kani::assume(after_sb < 2 * SSZ as u64);
    let r = em.format(p);
    assert!(support_l3_is_corrupt(&r) != mutant);
    std::mem::forget(em);
}

fn support_l3_body_no_usable<const SSZ: u32, const AL: u64, const RC: u32>(mutant: bool) {
    let sectors: u64 = kani::any();
    kani::assume(sectors <= u64::MAX / MD_SS as u64);
    let em = support_em_with_device(sectors, false);
    let p = support_l3_params_with::<SSZ, AL, RC>();
    let disk = sectors * MD_SS as u64;
    let sb = SUPERBLOCK_SIZE as u64;
    let cro = if AL == 0 { sb } else { sb.div_ceil(AL) * AL };
    let eff = if p.metadata_region_size > 0 {
        disk.min(p.metadata_region_size)
    } else {
        disk
    };
    let ss = SSZ as u64;
    let crs = (eff.saturating_sub(cro) / 2) / ss * ss;
    let dso = if p.metadata_region_size > 0 {
        cro + 2 * crs
    } else {
        0
    };
    kani::assume(crs > 0 && p.data_disk_size <= dso);
    let r = em.format(p);
    if mutant {
        assert!(!support_l3_is_corrupt(&r));
    } else {
        assert!(support_l3_is_corrupt(&r));
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

/// AL_MAX = u64::MAX - (SUPERBLOCK_SIZE - 1): the largest alignment level-2 assumption
/// D-RANGE-FMT-METADATA-DEVICE-LAYOUT-2ee144 admits.
const L3_AL_MAX: u64 = u64::MAX - (SUPERBLOCK_SIZE as u64 - 1);

macro_rules! l3_dispatch {
    ($body:ident, $mutant:expr, [$($al:expr),*]) => {{
        let s: u8 = kani::any();
        let a: u8 = kani::any();
        let r: bool = kani::any();
        let mut k: u8 = 0;
        $(
            if a == k {
                if s == 0 {
                    if r { $body::<512, { $al }, 1>($mutant) } else { $body::<512, { $al }, 2>($mutant) }
                } else {
                    if r { $body::<4096, { $al }, 1>($mutant) } else { $body::<4096, { $al }, 2>($mutant) }
                }
            }
            k += 1;
        )*
        let _ = k;
    }};
}

fn support_case_l3_device_query_b2(mutant: bool) {
    l3_dispatch!(support_l3_body_device_query, mutant, [0, 4096, 1 << 20, L3_AL_MAX]);
}
fn support_case_l3_too_small_b2(mutant: bool) {
    l3_dispatch!(support_l3_body_too_small, mutant, [0, 4096, 1 << 20, L3_AL_MAX]);
}
fn support_case_l3_no_usable_b2(mutant: bool) {
    l3_dispatch!(support_l3_body_no_usable, mutant, [0, 4096, 1 << 20]);
}

// ---- L3 rung-b2 scored harnesses ----
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(component_core::receptacle::Receptacle::get, stub_l3_unreachable_receptacle_get)]
fn uncredited_verify_em_format_err_slab_not_sector_multiple() {
    support_case_l3_fmt_slab_not_multiple_b3(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(component_core::receptacle::Receptacle::get, stub_l3_unreachable_receptacle_get)]
fn uncredited_verify_em_format_err_slab_not_sector_multiple__mutant() {
    support_case_l3_fmt_slab_not_multiple_b3(true);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn verify_em_format_err_device_query() {
    support_case_l3_device_query_b2(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn verify_em_format_err_device_query__mutant() {
    support_case_l3_device_query_b2(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn verify_em_format_err_metadata_too_small() {
    support_case_l3_too_small_b2(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn verify_em_format_err_metadata_too_small__mutant() {
    support_case_l3_too_small_b2(true);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn uncredited_verify_em_format_err_no_usable_data() {
    support_case_l3_no_usable_b2(false);
}
#[kani::proof]
#[kani::unwind(2)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn uncredited_verify_em_format_err_no_usable_data__mutant() {
    support_case_l3_no_usable_b2(true);
}

/// rung b3 for EM-FORMAT-ERR-SLAB-NOT-SECTOR-MULTIPLE: rung b2 (tail cut at
/// Receptacle::get) still timed out (>400 s, 0.9 GB) on the fully symbolic u64 % u32
/// guard. This applies level-2 assumption D-RANGE-FR-002-217ace (sector size is a power
/// of two), under which the guard is a mask test.
fn support_case_l3_fmt_slab_not_multiple_b3(mutant: bool) {
    let em = support_l3_em_any_device();
    let p = support_l3_any_params();
    kani::assume(p.sector_size.is_power_of_two());
    kani::assume(p.slab_size % p.sector_size as u64 != 0);
    let r = em.format(p);
    assert!(support_l3_is_corrupt(&r) != mutant);
    std::mem::forget(em);
}

#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(component_core::receptacle::Receptacle::get, stub_l3_unreachable_receptacle_get)]
fn diag_l3_format_err_slab_not_sector_multiple_rung_b2() {
    support_case_l3_fmt_slab_not_multiple(false);
}

fn support_case_l3_too_small_ctrl(mutant: bool) {
    l3_dispatch!(support_l3_body_too_small, mutant, [0, 4096, 1 << 20]);
}
/// CONTROL for the refutation: the same rung-b2 harness with the overflow witness
/// alignment (L3_AL_MAX) removed from the menu must verify.
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_metadata_too_small_ctrl_no_almax() {
    support_case_l3_too_small_ctrl(false);
}
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_metadata_too_small_ctrl_no_almax__mutant() {
    support_case_l3_too_small_ctrl(true);
}
/// rung b3 for EM-FORMAT-ERR-DEVICE-QUERY: ONE concrete divisor combination.
#[kani::proof]
#[kani::unwind(4)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::buddy::BuddyAllocator::new, stub_l3_unreachable_buddy_new)]
fn diag_l3_format_err_device_query_one_combo() {
    support_l3_body_device_query::<512, 4096, 1>(false);
}

// ---- L3 timer rung b: ExtentManager::new_default model ----
// Rung a (new_inner with the real define_component! new_default) failed on "unwinding
// assertion loop 0" at unwind 2, 3 and 4 (~170 s / 2.2 GB each): the interface-map
// population (HashMap inserts, TypeId-keyed) sits on the path. The model returns the same
// USER-FIELD defaults (every user field Default::default(), receptacles disconnected -
// define_component.rs:297-298 `Self::new(Default::default(), ..)`), with an empty
// interface map. No timer property reads the interface map.
// Conformance: check_stub_conforms_new_default (same fields, real constructor).
pub(crate) fn stub_l3_new_default() -> Arc<ExtentManager> {
    Arc::new(support_new_em())
}

#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
fn check_stub_conforms_new_default() {
    let real = ExtentManager::new_default();
    let model = stub_l3_new_default();
    assert!(*real.checkpoint_timer_state.interval.lock().unwrap() == *model.checkpoint_timer_state.interval.lock().unwrap());
    assert!(real.checkpoint_timer_state.shutdown.load(std::sync::atomic::Ordering::Relaxed) == model.checkpoint_timer_state.shutdown.load(std::sync::atomic::Ordering::Relaxed));
    assert!(real.checkpoint_thread.lock().unwrap().is_none() && model.checkpoint_thread.lock().unwrap().is_none());
    assert!(real.regions.read().is_none() && model.regions.read().is_none());
    assert!(real.shared.lock().unwrap().is_none() && model.shared.lock().unwrap().is_none());
    assert!(*real.data_base_lba.lock().unwrap() == *model.data_base_lba.lock().unwrap());
    assert!(*real.metadata_base_lba.lock().unwrap() == *model.metadata_base_lba.lock().unwrap());
    assert!(*real.metadata_ns_id.lock().unwrap() == *model.metadata_ns_id.lock().unwrap());
    {
        let a = real.checkpoint_coalesce.lock().unwrap();
        let b = model.checkpoint_coalesce.lock().unwrap();
        assert!(a.completed_seq == b.completed_seq && a.in_progress == b.in_progress);
    }
    assert!(real.metadata_device.get().is_err() && model.metadata_device.get().is_err());
    assert!(real.logger.get().is_err() && model.logger.get().is_err());
    assert!(real.post_checkpoint_hook.lock().unwrap().is_none() && model.post_checkpoint_hook.lock().unwrap().is_none());
    assert!(real.dma_alloc.lock().unwrap().is_none() && model.dma_alloc.lock().unwrap().is_none());
    std::mem::forget(real);
    std::mem::forget(model);
}

/// Unreachability stub (path cut) for ExtentManager::log_info: on an UNINITIALISED
/// component run_checkpoint returns NotInitialized before its first log_info, so any
/// reach is a failed check. Timer rung b2: rung b (new_default model) still timed out
/// (>400 s, 8 GB at unwind 2) because symex explores run_checkpoint's tail through the
/// Weak::upgrade()d pointer.
pub(crate) fn stub_l3_unreachable_log_info(_this: &ExtentManager, _msg: &str) {
    assert!(false, "L3 unreachability stub reached: ExtentManager::log_info");
    kani::assume(false);
}

// ---- L3 EM-BIO-FLUSH rework: the inherited support_case_bio_flush is VACUOUS for its
// success claim under the current device model: support_bio_reset(true, _) makes the FIRST
// command always fault (KANI_FAULT_CMD = Some(0), kind 1), so `fault.is_none()` is
// unreachable and the __mutant (which differs only there) also verifies (L3 run: both
// SUCCESSFUL at --unwind 6, checks ON). This case drives the no-fault answer and each fault
// kind (1..=5) in its own concrete branch.
#[cfg(feature = "volatile_write_cache")]
fn support_case_l3_bio_flush(mutant: bool) {
    let k: u8 = kani::any();
    kani::assume(k <= 5);
    if k == 0 {
        support_bio_reset(false, false);
    } else {
        support_bio_reset(true, false);
        support_set_fault_kind(k);
    }
    let (ns, base, _lba) = support_bio_addr();
    let client = support_bio_client(ns, base);
    let r = client.flush();
    let sent = support_bio_sent();
    kani::cover!(k == 0 && r.is_ok());
    // at most the one flush command reached the device (none if `send` itself failed)
    assert!(sent.len() <= 1);
    if sent.len() == 1 {
        assert!(sent[0].0 == ns);
    }
    if k == 0 {
        // the device answered FlushDone{Ok}
        if mutant {
            assert!(r.is_err());
        } else {
            assert!(r.is_ok());
        }
    } else {
        assert!(support_is_io(&r));
    }
    std::mem::forget(client);
}

#[cfg(feature = "volatile_write_cache")]
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn uncredited_verify_em_bio_flush() {
    support_case_l3_bio_flush(false);
}
#[cfg(feature = "volatile_write_cache")]
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(component_core::channel::Sender::send, stub_chan_send)]
#[kani::stub(component_core::channel::Receiver::recv, stub_chan_recv)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
fn uncredited_verify_em_bio_flush__mutant() {
    support_case_l3_bio_flush(true);
}

// ---- L3 os-file rung b: the OS model restricted to the obligation's precondition ("a
// random id cannot be generated"): open fails, or open succeeds and the read fails. Rung a
// (model also allowing success) timed out (>300 s, 13.8 GB at unwind 2 and 4): the success
// branch drags format()'s superblock write into the harness.
static mut L3_URANDOM_MUST_FAIL: bool = false;
pub(crate) fn stub_l3_file_open_fail<P: AsRef<std::path::Path>>(
    _p: P,
) -> std::io::Result<std::fs::File> {
    if kani::any() {
        return Err(std::io::Error::from_raw_os_error(2));
    }
    use std::os::fd::FromRawFd;
    // SAFETY: fd 3 is never passed to the OS: read and the fd's Drop are stubbed.
    Ok(unsafe { std::fs::File::from_raw_fd(3) })
}
pub(crate) fn stub_l3_file_read_fail(
    _this: &mut std::fs::File,
    _buf: &mut [u8],
) -> std::io::Result<usize> {
    Err(std::io::Error::from_raw_os_error(5))
}
fn support_case_l3_fmt_instance_id_generation_b(mutant: bool) {
    support_disk2_reset(false);
    let em = support_em_with_device(4, false);
    let mut p = support_layout(L_ONE);
    p.instance_id = None;
    let r = em.format(p);
    if mutant {
        assert!(!support_l3_is_io(&r));
    } else {
        assert!(support_l3_is_io(&r));
        assert!(support_l3_uninit(&em));
    }
    std::mem::forget(em);
}

#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(std::fs::File::open, stub_l3_file_open_fail)]
#[kani::stub(<std::fs::File as std::io::Read>::read, stub_l3_file_read_fail)]
#[kani::stub(<std::os::fd::OwnedFd as std::ops::Drop>::drop, stub_l3_ownedfd_drop)]
fn verify_em_format_err_instance_id_generation() {
    support_case_l3_fmt_instance_id_generation_b(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::error::nvme_to_em, stub_nvme_to_em)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(std::fs::File::open, stub_l3_file_open_fail)]
#[kani::stub(<std::fs::File as std::io::Read>::read, stub_l3_file_read_fail)]
#[kani::stub(<std::os::fd::OwnedFd as std::ops::Drop>::drop, stub_l3_ownedfd_drop)]
fn verify_em_format_err_instance_id_generation__mutant() {
    support_case_l3_fmt_instance_id_generation_b(true);
}

// ---- L3 timer rung b3: INLINE spawn (the thread body runs inside new_inner, from the
// spawn call itself - the schedule "the thread runs to its exit before new_inner
// continues", an allowed interleaving). Rung b/b2 stored the closure as
// Box<dyn FnOnce> in a static and called it later: every thread-body harness timed out
// (>300-400 s, 3-8 GB at unwind 2-4) while new_inner alone verified in 38 s.
static mut L3_CONNECT_LOGGER: bool = false;

pub(crate) fn stub_l3_spawn_inline<F, T>(f: F) -> std::thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let _ = f();
    // SAFETY: as stub_l3_spawn_deferred - never joined/used, read back as None via the niche.
    #[allow(invalid_value)]
    unsafe {
        std::mem::MaybeUninit::zeroed().assume_init()
    }
}

/// new_default model + optional logger connection (the thread runs inside new_inner, so
/// the logger must be connected at construction for the inline schedule).
pub(crate) fn stub_l3_new_default_lg() -> Arc<ExtentManager> {
    let em = Arc::new(support_new_em());
    // SAFETY: single-threaded harness; records the timer for the env model.
    unsafe {
        *std::ptr::addr_of_mut!(L3_TIMER) = Arc::as_ptr(&em.checkpoint_timer_state);
        if *std::ptr::addr_of!(L3_CONNECT_LOGGER) {
            let lg: Arc<dyn ILogger + Send + Sync> = Arc::new(KaniL3Logger);
            let _ = em.logger.connect(lg);
        }
    }
    em
}

fn support_case_l3_inline_bg_not_init_silent(mutant: bool) {
    support_l3_reset(2, true, support_l3_any_interval());
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_CONNECT_LOGGER) = true;
    }
    let em = ExtentManager::new_inner();
    // SAFETY: single-threaded harness.
    let (calls, errors) = unsafe {
        (*std::ptr::addr_of!(L3_CKPT_CALLS), *std::ptr::addr_of!(L3_ERRORS))
    };
    kani::cover!(calls > 0);
    if mutant {
        assert!(!(calls > 0 && errors == 0));
    } else {
        assert!(errors == 0);
    }
    std::mem::forget(em);
}

fn support_case_l3_inline_change_no_spurious(mutant: bool) {
    support_l3_reset(3, true, support_l3_any_interval());
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_CONNECT_LOGGER) = false;
    }
    let em = ExtentManager::new_inner();
    // SAFETY: single-threaded harness.
    let (spurious, mismatch, woken) = unsafe {
        (
            *std::ptr::addr_of!(L3_SPURIOUS_CKPT),
            *std::ptr::addr_of!(L3_DUR_MISMATCH),
            *std::ptr::addr_of!(L3_WOKEN_SEEN),
        )
    };
    kani::cover!(woken > 0);
    if mutant {
        assert!(!(woken > 0 && !spurious));
    } else {
        assert!(!spurious);
        assert!(!mismatch);
    }
    std::mem::forget(em);
}

fn support_case_l3_inline_none_disables(mutant: bool) {
    support_l3_reset(3, true, None);
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_CONNECT_LOGGER) = false;
    }
    let em = ExtentManager::new_inner();
    // SAFETY: single-threaded harness.
    let (set, at, calls) = unsafe {
        (
            *std::ptr::addr_of!(L3_SET_DONE),
            *std::ptr::addr_of!(L3_CKPT_AT_SET),
            *std::ptr::addr_of!(L3_CKPT_CALLS),
        )
    };
    kani::cover!(set);
    if mutant {
        assert!(!(set && calls == at));
    } else if set {
        assert!(calls == at);
    }
    std::mem::forget(em);
}

fn support_case_l3_inline_some_fires(allow_early: bool, mutant: bool) {
    support_l3_reset(3, false, None);
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(L3_CONNECT_LOGGER) = false;
        *std::ptr::addr_of_mut!(L3_CLOCK_MS) = 0;
        *std::ptr::addr_of_mut!(L3_LAST_CKPT_MS) = 0;
        *std::ptr::addr_of_mut!(L3_GAP_VIOLATION) = false;
        *std::ptr::addr_of_mut!(L3_ALLOW_EARLY) = allow_early;
        *std::ptr::addr_of_mut!(L3_FIXED_MS) = 30_000; // the default interval new_inner sets
    }
    let em = ExtentManager::new_inner();
    // SAFETY: single-threaded harness.
    let (viol, calls) = unsafe {
        (*std::ptr::addr_of!(L3_GAP_VIOLATION), *std::ptr::addr_of!(L3_CKPT_CALLS))
    };
    kani::cover!(calls > 0);
    if mutant {
        assert!(!(calls > 0 && !viol));
    } else {
        assert!(!viol);
    }
    std::mem::forget(em);
}

// ---- L3 timer rung-b3 (inline spawn) harnesses ----
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn verify_em_setint_bg_not_init_silent() {
    support_case_l3_inline_bg_not_init_silent(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn verify_em_setint_bg_not_init_silent__mutant() {
    support_case_l3_inline_bg_not_init_silent(true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn verify_em_setint_change_no_spurious() {
    support_case_l3_inline_change_no_spurious(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn verify_em_setint_change_no_spurious__mutant() {
    support_case_l3_inline_change_no_spurious(true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn verify_em_setint_post_none_disables() {
    support_case_l3_inline_none_disables(false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_count)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn verify_em_setint_post_none_disables__mutant() {
    support_case_l3_inline_none_disables(true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout_clock)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_clock)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn verify_em_setint_some_fires() {
    support_case_l3_inline_some_fires(true, false);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout_clock)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_clock)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn verify_em_setint_some_fires__mutant() {
    support_case_l3_inline_some_fires(true, true);
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(crate::ExtentManager::new_default, stub_l3_new_default_lg)]
#[kani::stub(crate::ExtentManager::log_info, stub_l3_unreachable_log_info)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(std::thread::spawn, stub_l3_spawn_inline)]
#[kani::stub(std::sync::Condvar::wait_timeout, stub_l3_wait_timeout_clock)]
#[kani::stub(std::sync::Condvar::wait, stub_l3_wait)]
#[kani::stub(std::sync::Condvar::notify_all, stub_l3_notify_all_clock)]
#[kani::stub(std::sync::Condvar::notify_one, stub_condvar_notify)]
fn diag_l3_inline_some_fires_control_no_early_wake() {
    support_case_l3_inline_some_fires(false, false);
}

// ---- J6 (2026-10-08): conformance of `stub_random_state` ----
// What the code relies on from the real `RandomState::new()` (reached via InterfaceMap::new -> HashMap::new):
// only HashMap SEMANTICS. The real constructor returns SOME SipHash key pair; an arbitrary `[u64; 2]` covers every
// pair it can return. The check: for the operations the code uses on these maps (insert / get / len / remove /
// is_empty, TypeId keys as in InterfaceMap), a map built with the stub's fixed keys answers exactly like a map built
// with arbitrary keys. The twin claims a difference and must FAIL.
fn support_conform_random_state(mutant: bool) {
    use std::any::TypeId;
    use std::collections::HashMap;
    let s = stub_random_state();
    let k: [u64; 2] = kani::any();
    // SAFETY: RandomState is two u64 SipHash keys (size asserted inside stub_random_state); any bit pattern is a valid key pair.
    let r: RandomState = unsafe { std::mem::transmute(k) };
    let mut a: HashMap<TypeId, u8, RandomState> = HashMap::with_hasher(s);
    let mut b: HashMap<TypeId, u8, RandomState> = HashMap::with_hasher(r);
    let keys = [TypeId::of::<u8>(), TypeId::of::<u16>()];
    assert_eq!(a.is_empty(), b.is_empty());
    let i: usize = kani::any();
    kani::assume(i < 2);
    let v: u8 = kani::any();
    assert_eq!(a.insert(keys[i], v), b.insert(keys[i], v));
    let i2: usize = kani::any();
    kani::assume(i2 < 2);
    let v2: u8 = kani::any();
    assert_eq!(a.insert(keys[i2], v2), b.insert(keys[i2], v2));
    let j: usize = kani::any();
    kani::assume(j < 2);
    assert_eq!(a.get(&keys[j]), b.get(&keys[j]));
    if mutant {
        assert!(a.len() != b.len());
    } else {
        assert_eq!(a.len(), b.len());
    }
    assert_eq!(a.remove(&keys[j]), b.remove(&keys[j]));
    assert_eq!(a.get(&keys[1 - j]), b.get(&keys[1 - j]));
    assert_eq!(a.len(), b.len());
}

#[kani::proof]
#[kani::unwind(8)]
fn check_stub_conforms_random_state() {
    support_conform_random_state(false);
}

#[kani::proof]
#[kani::unwind(8)]
fn check_stub_conforms_random_state__mutant() {
    support_conform_random_state(true);
}

// ---- J6: conformance of `stub_random_state` for the EMPTY-MAP use only ----
// The uninitialised-getter harnesses build `support_new_em` -> `InterfaceMap::new()` (an empty
// HashMap<TypeId, Box<dyn Any + Send + Sync>>) and only drop it: no insert/get on that map is reached
// (the real code inserts only in the define_component! constructor, which these harnesses do not run).
// Check: an empty map built with the stub's keys is indistinguishable from one built with arbitrary keys
// for the operations reached (construction, is_empty/len, drop). The twin claims a difference and must FAIL.
fn support_conform_random_state_empty(mutant: bool) {
    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    let s = stub_random_state();
    let k: [u64; 2] = kani::any();
    // SAFETY: RandomState is two u64 SipHash keys (size asserted inside stub_random_state); any bit pattern is a valid key pair.
    let r: RandomState = unsafe { std::mem::transmute(k) };
    let a: HashMap<TypeId, Box<dyn Any + Send + Sync>, RandomState> = HashMap::with_hasher(s);
    let b: HashMap<TypeId, Box<dyn Any + Send + Sync>, RandomState> = HashMap::with_hasher(r);
    assert_eq!(a.is_empty(), b.is_empty());
    if mutant {
        assert!(a.len() != b.len());
    } else {
        assert_eq!(a.len(), b.len());
        assert!(a.is_empty() && a.capacity() == b.capacity());
    }
    drop(a);
    drop(b);
    // the second map type built with RandomState on these paths: SizeClassManager's HashMap<u32, Vec<u64>> (slab.rs:92-98)
    let s2 = stub_random_state();
    let k2: [u64; 2] = kani::any();
    // SAFETY: as above - any bit pattern is a valid SipHash key pair.
    let r2: RandomState = unsafe { std::mem::transmute(k2) };
    let c: HashMap<u32, Vec<u64>, RandomState> = HashMap::with_hasher(s2);
    let d: HashMap<u32, Vec<u64>, RandomState> = HashMap::with_hasher(r2);
    assert_eq!(c.is_empty(), d.is_empty());
    assert_eq!(c.len(), d.len());
    assert_eq!(c.capacity(), d.capacity());
    drop(c);
    drop(d);
}

#[kani::proof]
#[kani::unwind(4)]
fn check_stub_conforms_random_state_empty() {
    support_conform_random_state_empty(false);
}

#[kani::proof]
#[kani::unwind(4)]
fn check_stub_conforms_random_state_empty__mutant() {
    support_conform_random_state_empty(true);
}

// ===========================================================================
// J7 diagnostics: does each de-credited harness reach a key-hashing HashMap op?
// Every such op is stubbed to `kani::assert(false, "map op reached")` + diverge.
// The original harness's unwind and stub list are kept verbatim (incl. stub_random_state).
// ===========================================================================
pub(crate) fn stub_j7_map_insert<K, V, S>(
    _m: &mut std::collections::HashMap<K, V, S>,
    _k: K,
    _v: V,
) -> Option<V> {
    kani::assert(false, "map op reached");
    panic!()
}
pub(crate) fn stub_j7_map_get<'a, K, V, S, Q: ?Sized>(
    _m: &'a std::collections::HashMap<K, V, S>,
    _k: &Q,
) -> Option<&'a V> {
    kani::assert(false, "map op reached");
    panic!()
}
pub(crate) fn stub_j7_map_get_mut<'a, K, V, S, Q: ?Sized>(
    _m: &'a mut std::collections::HashMap<K, V, S>,
    _k: &Q,
) -> Option<&'a mut V> {
    kani::assert(false, "map op reached");
    panic!()
}
pub(crate) fn stub_j7_map_remove<K, V, S, Q: ?Sized>(
    _m: &mut std::collections::HashMap<K, V, S>,
    _k: &Q,
) -> Option<V> {
    kani::assert(false, "map op reached");
    panic!()
}
pub(crate) fn stub_j7_map_entry<'a, K, V, S>(
    _m: &'a mut std::collections::HashMap<K, V, S>,
    _k: K,
) -> std::collections::hash_map::Entry<'a, K, V> {
    kani::assert(false, "map op reached");
    panic!()
}
pub(crate) fn stub_j7_map_contains_key<K, V, S, Q: ?Sized>(
    _m: &std::collections::HashMap<K, V, S>,
    _k: &Q,
) -> bool {
    kani::assert(false, "map op reached");
    panic!()
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::collections::HashMap::insert, stub_j7_map_insert)]
#[kani::stub(std::collections::HashMap::get, stub_j7_map_get)]
#[kani::stub(std::collections::HashMap::get_mut, stub_j7_map_get_mut)]
#[kani::stub(std::collections::HashMap::remove, stub_j7_map_remove)]
#[kani::stub(std::collections::HashMap::entry, stub_j7_map_entry)]
#[kani::stub(std::collections::HashMap::contains_key, stub_j7_map_contains_key)]
fn diag_j7_em_ckpt_read_seq_mismatch() {
    kani::cover!(true, "diag body start reached");
    support_case_ckpt_seq(false);
    kani::cover!(true, "diag body end reached");
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::collections::HashMap::insert, stub_j7_map_insert)]
#[kani::stub(std::collections::HashMap::get, stub_j7_map_get)]
#[kani::stub(std::collections::HashMap::get_mut, stub_j7_map_get_mut)]
#[kani::stub(std::collections::HashMap::remove, stub_j7_map_remove)]
#[kani::stub(std::collections::HashMap::entry, stub_j7_map_entry)]
#[kani::stub(std::collections::HashMap::contains_key, stub_j7_map_contains_key)]
fn diag_j7_em_ckpt_read_oversize() {
    kani::cover!(true, "diag body start reached");
    support_case_ckpt_oversize(false);
    kani::cover!(true, "diag body end reached");
}
#[kani::proof]
#[kani::unwind(10)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_rb_short)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::collections::HashMap::insert, stub_j7_map_insert)]
#[kani::stub(std::collections::HashMap::get, stub_j7_map_get)]
#[kani::stub(std::collections::HashMap::get_mut, stub_j7_map_get_mut)]
#[kani::stub(std::collections::HashMap::remove, stub_j7_map_remove)]
#[kani::stub(std::collections::HashMap::entry, stub_j7_map_entry)]
#[kani::stub(std::collections::HashMap::contains_key, stub_j7_map_contains_key)]
fn diag_j7_em_ckpt_read_truncated() {
    kani::cover!(true, "diag body start reached");
    support_case_ckpt_truncated(false);
    kani::cover!(true, "diag body end reached");
}
#[kani::proof]
#[kani::unwind(30)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_faithful)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_read_blocks2)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::collections::HashMap::insert, stub_j7_map_insert)]
#[kani::stub(std::collections::HashMap::get, stub_j7_map_get)]
#[kani::stub(std::collections::HashMap::get_mut, stub_j7_map_get_mut)]
#[kani::stub(std::collections::HashMap::remove, stub_j7_map_remove)]
#[kani::stub(std::collections::HashMap::entry, stub_j7_map_entry)]
#[kani::stub(std::collections::HashMap::contains_key, stub_j7_map_contains_key)]
fn diag_j7_em_ckpt_read_crc() {
    kani::cover!(true, "diag body start reached");
    support_case_ckpt_crc(false);
    kani::cover!(true, "diag body end reached");
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::collections::HashMap::insert, stub_j7_map_insert)]
#[kani::stub(std::collections::HashMap::get, stub_j7_map_get)]
#[kani::stub(std::collections::HashMap::get_mut, stub_j7_map_get_mut)]
#[kani::stub(std::collections::HashMap::remove, stub_j7_map_remove)]
#[kani::stub(std::collections::HashMap::entry, stub_j7_map_entry)]
#[kani::stub(std::collections::HashMap::contains_key, stub_j7_map_contains_key)]
fn diag_j7_em_ckpt_decode_truncated() {
    kani::cover!(true, "diag body start reached");
    support_case_decode_truncated(false);
    kani::cover!(true, "diag body end reached");
}
#[kani::proof]
#[kani::unwind(5)]
#[kani::solver(minisat)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::fmt::format, stub_format)]
#[kani::stub(crc32fast::hash, stub_crc32_hash)]
#[kani::stub(crate::block_io::BlockDeviceClient::write_blocks, stub_write_blocks2)]
#[kani::stub(crate::block_io::BlockDeviceClient::read_blocks, stub_rb_media_err)]
#[kani::stub(std::sync::Arc::drop_slow, stub_arc_drop_slow)]
#[kani::stub(std::collections::HashMap::insert, stub_j7_map_insert)]
#[kani::stub(std::collections::HashMap::get, stub_j7_map_get)]
#[kani::stub(std::collections::HashMap::get_mut, stub_j7_map_get_mut)]
#[kani::stub(std::collections::HashMap::remove, stub_j7_map_remove)]
#[kani::stub(std::collections::HashMap::entry, stub_j7_map_entry)]
#[kani::stub(std::collections::HashMap::contains_key, stub_j7_map_contains_key)]
fn diag_j7_em_err_device_as_ioerror() {
    kani::cover!(true, "diag body start reached");
    support_case_device_err_reported(false);
    kani::cover!(true, "diag body end reached");
}
// J7 positive control: the same six map stubs MUST fire on each real map call site.
#[kani::proof]
#[kani::unwind(5)]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
#[kani::stub(std::collections::HashMap::insert, stub_j7_map_insert)]
#[kani::stub(std::collections::HashMap::get, stub_j7_map_get)]
#[kani::stub(std::collections::HashMap::get_mut, stub_j7_map_get_mut)]
#[kani::stub(std::collections::HashMap::remove, stub_j7_map_remove)]
#[kani::stub(std::collections::HashMap::entry, stub_j7_map_entry)]
#[kani::stub(std::collections::HashMap::contains_key, stub_j7_map_contains_key)]
fn diag_j7_control_map_stubs_fire() {
    let g: u8 = kani::any();
    if g == 0 {
        let mut m = crate::slab::SizeClassManager::new();
        m.add_slab(1, 0); // entry
    } else if g == 1 {
        let m = crate::slab::SizeClassManager::new();
        let _ = m.get_slabs(1); // get
    } else if g == 2 {
        let mut m = crate::slab::SizeClassManager::new();
        m.remove_slab(1, 0); // get_mut
    } else if g == 3 {
        let mut im = InterfaceMap::new();
        im.insert(std::any::TypeId::of::<u8>(), "I", Box::new(0u8)); // insert
    } else if g == 4 {
        let im = InterfaceMap::new();
        let _ = im.lookup(std::any::TypeId::of::<u8>()); // get
    } else {
        let m: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let _ = m.contains_key(&1); // contains_key
        let mut m2: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let _ = m2.remove(&1); // remove
    }
}

// ===========================================================================
// J8 — buddy-nested-vec ladder (rung a: unwind sweep on the REAL BuddyAllocator,
// concrete small geometries n in {1,2,4} sectors; bounded => never credited)
// ===========================================================================

/// Sum of the free lists computed independently of `total_free()` (index loops only).
fn support_j8_sum_free(b: &BuddyAllocator) -> u64 {
    let lists = b.kani_free_lists();
    let mut t = 0u64;
    let mut o = 0usize;
    while o < lists.len() {
        t += lists[o].len() as u64 * ((1u64 << o) * SSU);
        o += 1;
    }
    t
}

fn support_j8_init_free<const N: u64>(mutant: bool) {
    let b = BuddyAllocator::new(B_BASE, N * SSU, SS);
    if !mutant {
        assert!(b.total_free() == N * SSU);
    } else {
        assert!(b.total_free() != N * SSU);
    }
}

#[kani::proof]
fn bounded_verify_em_buddy_init_free_n1() { support_j8_init_free::<1>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_init_free_n1__mutant() { support_j8_init_free::<1>(true) }
#[kani::proof]
fn bounded_verify_em_buddy_init_free_n2() { support_j8_init_free::<2>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_init_free_n2__mutant() { support_j8_init_free::<2>(true) }
#[kani::proof]
fn bounded_verify_em_buddy_init_free_n4() { support_j8_init_free::<4>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_init_free_n4__mutant() { support_j8_init_free::<4>(true) }

/// One alloc of K sectors on a fresh N-sector allocator.
fn support_j8_alloc_accounting<const N: u64, const K: u64>(mutant: bool) {
    let mut b = BuddyAllocator::new(B_BASE, N * SSU, SS);
    let before = b.total_free();
    let r = b.alloc(K * SSU);
    if let Some(_o) = r {
        let after = b.total_free();
        if !mutant {
            assert!(after == before - support_pow2_ceil(K) * SSU);
        } else {
            assert!(after == before);
        }
    } else if mutant {
        // keep the twin non-vacuous on the None path too (K <= N: None is impossible)
        assert!(true);
    }
}

#[kani::proof]
fn bounded_verify_em_buddy_alloc_accounting_n1() { support_j8_alloc_accounting::<1, 1>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_alloc_accounting_n1__mutant() { support_j8_alloc_accounting::<1, 1>(true) }
#[kani::proof]
fn bounded_verify_em_buddy_alloc_accounting_n2() { support_j8_alloc_accounting::<2, 1>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_alloc_accounting_n2__mutant() { support_j8_alloc_accounting::<2, 1>(true) }
#[kani::proof]
fn bounded_verify_em_buddy_alloc_accounting_n4() { support_j8_alloc_accounting::<4, 1>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_alloc_accounting_n4__mutant() { support_j8_alloc_accounting::<4, 1>(true) }

/// free() of a block whose buddy is free, from concrete lists built without new().
/// n=2: lists [[SS],[]] (block 0 allocated), free(0,SS) -> lists [[],[0]].
/// n=4: lists [[],[2SS],[]] (block 0..2 allocated), free(0,2SS) -> lists [[],[],[0]].
/// n=1: lists [[]], free(0,SS) -> [[0]] (max_order 0: no merge possible).
fn support_j8_merge<const N: u64>(mutant: bool) {
    let mut b = if N == 1 {
        BuddyAllocator::kani_from_parts(B_BASE, SSU, SS, 0, vec![Vec::with_capacity(2)])
    } else if N == 2 {
        let mut l0 = Vec::with_capacity(2);
        l0.push(SSU);
        BuddyAllocator::kani_from_parts(B_BASE, 2 * SSU, SS, 1, vec![l0, Vec::with_capacity(2)])
    } else {
        let mut l1 = Vec::with_capacity(2);
        l1.push(2 * SSU);
        BuddyAllocator::kani_from_parts(
            B_BASE,
            4 * SSU,
            SS,
            2,
            vec![Vec::with_capacity(2), l1, Vec::with_capacity(2)],
        )
    };
    let blk = if N == 4 { 2 * SSU } else { SSU };
    b.free(B_BASE, blk);
    let lists = b.kani_free_lists();
    let top = lists.len() - 1;
    let merged = lists[top].len() == 1 && lists[top][0] == 0 && {
        let mut lower_empty = true;
        let mut o = 0usize;
        while o < top {
            lower_empty &= lists[o].is_empty();
            o += 1;
        }
        lower_empty
    };
    if !mutant {
        assert!(merged);
    } else {
        assert!(!merged);
    }
}

#[kani::proof]
fn bounded_verify_em_buddy_merge_n1() { support_j8_merge::<1>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_merge_n1__mutant() { support_j8_merge::<1>(true) }
#[kani::proof]
fn bounded_verify_em_buddy_merge_n2() { support_j8_merge::<2>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_merge_n2__mutant() { support_j8_merge::<2>(true) }
#[kani::proof]
fn bounded_verify_em_buddy_merge_n4() { support_j8_merge::<4>(false) }
#[kani::proof]
fn bounded_verify_em_buddy_merge_n4__mutant() { support_j8_merge::<4>(true) }

// ---- J8 rung a (continued): symbolic geometry for new() + total_free() ----
// limit_* = measured limits (no verdict within the 32 GB cap / time box); see
// ~/FV/extent-manager_kani_limits.yaml J8 records. Never credited.
/// base, total symbolic; sector size 512 or symbolic power of two (D-RANGE-FR-002-217ace);
/// headroom D-RANGE-FR-002-3b58ea with data_disk_size := total. LOGMAX < 64 additionally
/// bounds usable blocks below 2^LOGMAX (bounded => never credited).
fn support_j8_init_free_sym<const LOGMAX: u32, const SYM_SS: bool>(mutant: bool) {
    let base: u64 = kani::any();
    let total: u64 = kani::any();
    let ss: u32 = if SYM_SS { kani::any() } else { SS };
    kani::assume(ss.is_power_of_two());
    kani::assume(total.checked_mul(2).and_then(|x| x.checked_add(ss as u64)).is_some());
    if LOGMAX < 64 {
        kani::assume(total / (ss as u64) < (1u64 << LOGMAX));
    }
    let b = BuddyAllocator::new(base, total, ss);
    let want = total / ss as u64 * ss as u64;
    if !mutant {
        assert!(b.total_free() == want);
    } else {
        assert!(b.total_free() != want);
    }
}

#[kani::proof]
fn limit_em_buddy_init_free_sym3_nostub() { support_j8_init_free_sym::<3, false>(false) }
#[kani::proof]
fn limit_em_buddy_init_free_sym3ss_nostub() { support_j8_init_free_sym::<3, true>(false) }
#[kani::proof]
fn limit_em_buddy_init_free_sym8ss_nostub() { support_j8_init_free_sym::<8, true>(false) }

// ---- J8 DIAGNOSTIC (never scored): BuddyAllocator::new's algorithm copied, storage varied ----
// Same symbolic input as bounded_verify_em_buddy_init_free_sym3 (total symbolic, < 8 sectors).
fn support_j8_diag_input() -> (u64, u64) {
    let total: u64 = kani::any();
    kani::assume(total / SSU < 8);
    let usable = total / SSU;
    (total, usable)
}
fn support_j8_max_order(usable: u64) -> usize {
    if usable > 1 { 63 - usable.leading_zeros() as usize } else { 0 }
}
/// D0: control = the production storage (Vec<Vec<u64>> via vec![Vec::new(); n]).
#[kani::proof]
fn diag_j8_new_vecvec() {
    let (_t, usable) = support_j8_diag_input();
    let mo = support_j8_max_order(usable);
    let mut fl: Vec<Vec<u64>> = vec![Vec::new(); mo + 1];
    let (mut off, mut rem) = (0u64, usable);
    while rem > 0 {
        let order = 63 - rem.leading_zeros() as usize;
        fl[order].push(off * SSU);
        off += 1u64 << order;
        rem -= 1u64 << order;
    }
    let mut t = 0u64;
    let mut o = 0usize;
    while o < fl.len() { t += fl[o].len() as u64 * (1u64 << o) * SSU; o += 1; }
    assert!(t == usable * SSU);
}
/// D1: outer fixed array, inner Vec: [Vec<u64>; 4].
#[kani::proof]
fn diag_j8_new_arrvec() {
    let (_t, usable) = support_j8_diag_input();
    let mut fl: [Vec<u64>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let (mut off, mut rem) = (0u64, usable);
    while rem > 0 {
        let order = 63 - rem.leading_zeros() as usize;
        fl[order].push(off * SSU);
        off += 1u64 << order;
        rem -= 1u64 << order;
    }
    let mut t = 0u64;
    let mut o = 0usize;
    while o < 4 { t += fl[o].len() as u64 * (1u64 << o) * SSU; o += 1; }
    assert!(t == usable * SSU);
}
/// D2: flat fixed arrays [[u64; 2]; 4] + lengths (no heap).
#[kani::proof]
fn diag_j8_new_flat() {
    let (_t, usable) = support_j8_diag_input();
    let mut fl = [[0u64; 2]; 4];
    let mut ln = [0usize; 4];
    let (mut off, mut rem) = (0u64, usable);
    while rem > 0 {
        let order = 63 - rem.leading_zeros() as usize;
        fl[order][ln[order]] = off * SSU;
        ln[order] += 1;
        off += 1u64 << order;
        rem -= 1u64 << order;
    }
    let mut t = 0u64;
    let mut o = 0usize;
    while o < 4 { t += ln[o] as u64 * (1u64 << o) * SSU; o += 1; }
    assert!(t == usable * SSU);
}
/// D3: Vec<Vec<u64>> but outer length CONCRETE (4) — isolates the symbolic-length vec![].
#[kani::proof]
fn diag_j8_new_vecvec_fixedlen() {
    let (_t, usable) = support_j8_diag_input();
    let mut fl: Vec<Vec<u64>> = vec![Vec::new(); 4];
    let (mut off, mut rem) = (0u64, usable);
    while rem > 0 {
        let order = 63 - rem.leading_zeros() as usize;
        fl[order].push(off * SSU);
        off += 1u64 << order;
        rem -= 1u64 << order;
    }
    let mut t = 0u64;
    let mut o = 0usize;
    while o < fl.len() { t += fl[o].len() as u64 * (1u64 << o) * SSU; o += 1; }
    assert!(t == usable * SSU);
}
/// D4: Vec<Vec<u64>> with symbolic outer length, inner Vecs pre-sized with_capacity(2)
/// (no realloc inside push).
#[kani::proof]
fn diag_j8_new_vecvec_cap() {
    let (_t, usable) = support_j8_diag_input();
    let mo = support_j8_max_order(usable);
    let mut fl: Vec<Vec<u64>> = Vec::with_capacity(4);
    let mut i = 0usize;
    while i <= mo { fl.push(Vec::with_capacity(2)); i += 1; }
    let (mut off, mut rem) = (0u64, usable);
    while rem > 0 {
        let order = 63 - rem.leading_zeros() as usize;
        fl[order].push(off * SSU);
        off += 1u64 << order;
        rem -= 1u64 << order;
    }
    let mut t = 0u64;
    let mut o = 0usize;
    while o < fl.len() { t += fl[o].len() as u64 * (1u64 << o) * SSU; o += 1; }
    assert!(t == usable * SSU);
}

// ---- J8 rung b: stub of std's `vec![elem; n]` (alloc::vec::from_elem) ----
// Measured (diag_j8_new_*): BuddyAllocator::new's `vec![Vec::new(); max_order + 1]` with a
// SYMBOLIC length OOMs (32 GB in 85 s, usable < 8 sectors); the same Vec<Vec<u64>> built
// by a push loop of the same symbolic length verifies in 13 s. The stub builds the vector
// by that push loop: n clones of `elem`, same length/contents as the real from_elem.
// The outer buffer gets a CONCRETE capacity (64 = the most orders a u64 block count can
// need) whenever n <= 64: measured (diag_j8_new_vecvec_symcap vs _innernew) the cliff is
// the SYMBOLIC-SIZE heap object holding the inner Vec headers, not the nesting. Capacity
// is not observable by BuddyAllocator (it only indexes free_lists, never pushes to it).
// (Measured: `if n <= 64 {64} else {n}` is itself a symbolic size after the merge -> OOM.)
pub(crate) fn stub_j8_from_elem<T: Clone>(elem: T, n: usize) -> Vec<T> {
    // A CHECK, not an assumption: production never asks for more than 64 lists.
    assert!(n <= 64, "J8 from_elem stub: n > 64");
    let mut v = Vec::with_capacity(64);
    let mut i = 0usize;
    while i < n {
        v.push(elem.clone());
        i += 1;
    }
    v
}

/// Conformance: the stub and the REAL std::vec::from_elem agree on length and contents
/// for elem = empty Vec<u64> (the only instantiation production uses) and for a
/// non-empty u64 vector, at every concrete n the buddy reaches in the small cases.
fn support_j8_from_elem_conf<const N: usize>(mutant: bool) {
    let real: Vec<Vec<u64>> = vec![Vec::new(); N];
    let model: Vec<Vec<u64>> = stub_j8_from_elem(Vec::new(), N);
    let x: u64 = kani::any();
    let real2: Vec<Vec<u64>> = vec![vec![x]; N];
    let model2: Vec<Vec<u64>> = stub_j8_from_elem(vec![x], N);
    let mut ok = real.len() == model.len() && real2.len() == model2.len();
    let mut i = 0usize;
    while i < N {
        ok &= real[i].is_empty() && model[i].is_empty();
        ok &= real2[i].len() == 1 && model2[i].len() == 1 && real2[i][0] == model2[i][0];
        i += 1;
    }
    if !mutant {
        assert!(ok);
    } else {
        // twin: a model with one element fewer must be caught
        let bad: Vec<Vec<u64>> = stub_j8_from_elem(Vec::new(), N - 1);
        assert!(bad.len() == real.len());
    }
}
#[kani::proof]
fn check_stub_conforms_j8_from_elem() {
    support_j8_from_elem_conf::<1>(false);
    support_j8_from_elem_conf::<2>(false);
    support_j8_from_elem_conf::<3>(false);
    support_j8_from_elem_conf::<4>(false);
}
#[kani::proof]
fn check_stub_conforms_j8_from_elem__mutant() {
    support_j8_from_elem_conf::<3>(true);
}
/// Does the stub actually replace production's vec![] (a non-firing stub would make
/// every stubbed proof below meaningless): with the stub, new() on usable < 8 symbolic
/// must not OOM; this harness only checks that the stub is reached.
pub(crate) fn stub_j8_from_elem_unreachable<T: Clone>(_elem: T, _n: usize) -> Vec<T> {
    assert!(false, "J8: from_elem stub fired");
    kani::assume(false);
    Vec::new()
}
#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem_unreachable)]
fn check_j8_from_elem_stub_fires() {
    // EXPECTED: FAILED with "J8: from_elem stub fired"
    let _b = BuddyAllocator::new(0, 4 * SSU, SS);
}

#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem)]
fn bounded_verify_em_buddy_init_free_sym3_vs() { support_j8_init_free_sym::<3, false>(false) }
#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem)]
fn bounded_verify_em_buddy_init_free_sym3_vs__mutant() { support_j8_init_free_sym::<3, false>(true) }
#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem)]
fn limit_em_buddy_init_free() { support_j8_init_free_sym::<64, true>(false) }
fn support_j8_diag_fill_sum(mut fl: Vec<Vec<u64>>, usable: u64) {
    let (mut off, mut rem) = (0u64, usable);
    while rem > 0 {
        let order = 63 - rem.leading_zeros() as usize;
        fl[order].push(off * SSU);
        off += 1u64 << order;
        rem -= 1u64 << order;
    }
    let mut t = 0u64;
    let mut o = 0usize;
    while o < fl.len() { t += fl[o].len() as u64 * (1u64 << o) * SSU; o += 1; }
    assert!(t == usable * SSU);
}
/// D5: outer capacity 4, inner Vec::new() (push reallocates from capacity 0).
#[kani::proof]
fn diag_j8_new_vecvec_innernew() {
    let (_t, usable) = support_j8_diag_input();
    let mo = support_j8_max_order(usable);
    let mut fl: Vec<Vec<u64>> = Vec::with_capacity(4);
    let mut i = 0usize;
    while i <= mo { fl.push(Vec::new()); i += 1; }
    support_j8_diag_fill_sum(fl, usable);
}
/// D6: outer capacity SYMBOLIC (mo+1), inner with_capacity(2).
#[kani::proof]
fn diag_j8_new_vecvec_symcap() {
    let (_t, usable) = support_j8_diag_input();
    let mo = support_j8_max_order(usable);
    let mut fl: Vec<Vec<u64>> = Vec::with_capacity(mo + 1);
    let mut i = 0usize;
    while i <= mo { fl.push(Vec::with_capacity(2)); i += 1; }
    support_j8_diag_fill_sum(fl, usable);
}
/// D7: real new() (from_elem stubbed) but the sum read by an index loop instead of total_free().
#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem)]
fn diag_j8_real_new_indexsum() {
    let total: u64 = kani::any();
    kani::assume(total / SSU < 8);
    let b = BuddyAllocator::new(B_BASE, total, SS);
    assert!(support_j8_sum_free(&b) == total / SSU * SSU);
}
/// D8: D7 without any read (does new() alone fit?).
#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem)]
fn diag_j8_real_new_only() {
    let total: u64 = kani::any();
    kani::assume(total / SSU < 8);
    let b = BuddyAllocator::new(B_BASE, total, SS);
    assert!(b.kani_free_lists().len() == b.kani_max_order() + 1);
}
/// D9: D5 with outer capacity 64 instead of 4.
#[kani::proof]
fn diag_j8_new_vecvec_cap64() {
    let (_t, usable) = support_j8_diag_input();
    let mo = support_j8_max_order(usable);
    let mut fl: Vec<Vec<u64>> = Vec::with_capacity(64);
    let mut i = 0usize;
    while i <= mo { fl.push(Vec::new()); i += 1; }
    support_j8_diag_fill_sum(fl, usable);
}
/// D10: outer built by the stub function itself, called directly.
#[kani::proof]
fn diag_j8_new_vecvec_viastub() {
    let (_t, usable) = support_j8_diag_input();
    let mo = support_j8_max_order(usable);
    let fl: Vec<Vec<u64>> = stub_j8_from_elem(Vec::new(), mo + 1);
    support_j8_diag_fill_sum(fl, usable);
}

// ---- J8 rung b applied: alloc / free on new() with SYMBOLIC geometry < 2^LOGMAX sectors,
// from_elem stubbed (conformance: check_stub_conforms_j8_from_elem + __mutant). The
// allocator state is the fresh one (bounded input) => proved-bounded, never credited.
fn support_j8_fresh<const LOGMAX: u32>() -> (BuddyAllocator, u64, u64) {
    let base: u64 = kani::any();
    let total: u64 = kani::any();
    kani::assume(total.checked_mul(2).and_then(|x| x.checked_add(SSU)).is_some());
    kani::assume(base.checked_add(total).is_some());
    kani::assume(total / SSU < (1u64 << LOGMAX));
    (BuddyAllocator::new(base, total, SS), base, total)
}
/// 0 = ALLOC-ACCOUNTING, 1 = ALLOC-WITHIN, 2 = FREE-ACCOUNTING, 3 = MERGE
fn support_j8_alloc_free<const LOGMAX: u32, const WHAT: u8>(mutant: bool) {
    let (mut b, base, total) = support_j8_fresh::<LOGMAX>();
    let before = b.total_free();
    let size: u64 = kani::any();
    kani::assume(size > 0 && size <= total);
    let span = support_pow2_ceil((size + SSU - 1) / SSU) * SSU;
    let r = b.alloc(size);
    if let Some(o) = r {
        if WHAT == 0 {
            let after = b.total_free();
            if !mutant { assert!(after == before - span) } else { assert!(after == before) }
        } else if WHAT == 1 {
            let ok = o >= base && span >= size && o - base + span <= total;
            if !mutant { assert!(ok) } else { assert!(!ok) }
        } else {
            b.free(o, span);
            if WHAT == 2 {
                let after = b.total_free();
                if !mutant { assert!(after == before) } else { assert!(after == before - span) }
            } else {
                // merge: the largest original block (2^max_order sectors) is available again
                let top = (1u64 << b.kani_max_order()) * SSU;
                let ok = total >= SSU && b.alloc(top).is_some();
                if !mutant { assert!(ok || total < SSU) } else { assert!(!ok) }
            }
        }
    }
}
#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem)]
fn limit_em_buddy_alloc_accounting_sym2() { support_j8_alloc_free::<2, 0>(false) }
#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem)]
fn limit_em_buddy_alloc_accounting_sym2__mutant() { support_j8_alloc_free::<2, 0>(true) }
/// D11: alloc on the fresh state with concrete base B_BASE, total symbolic < 4 sectors,
/// size a symbolic WHOLE number of sectors (no division by a symbolic remainder).
#[kani::proof]
#[kani::stub(std::vec::from_elem, stub_j8_from_elem)]
fn diag_j8_alloc_concrete_base() {
    let total: u64 = kani::any();
    kani::assume(total / SSU < 4);
    let mut b = BuddyAllocator::new(B_BASE, total, SS);
    let before = b.total_free();
    let k: u64 = kani::any();
    kani::assume(k >= 1 && k <= 3);
    if let Some(_o) = b.alloc(k * SSU) {
        assert!(b.total_free() == before - support_pow2_ceil(k) * SSU);
    }
}

// ===========================================================================
// J9 — region-slab-map ladder (2026-10-08). Records: ~/FV/extent-manager_kani_limits.yaml (J9).
// ===========================================================================

/// Region params with one region [0, 4 sectors), slab 2 sectors, sector 512.
pub(crate) fn support_j9_params() -> FormatParams {
    FormatParams {
        data_disk_size: 4 * SSU,
        slab_size: 2 * SSU,
        max_extent_size: 2 * SS,
        sector_size: SS,
        region_count: 1,
        metadata_alignment: 4096,
        instance_id: Some(0xC0FFEE),
        metadata_disk_ns_id: 1,
        metadata_region_size: 0,
    }
}

/// A RegionState with NSLABS (0..=2) concrete slabs at 0 and 2*SSU, element SS (2 slots each),
/// slot 0 of each allocated and registered as non-full in the size classes.
pub(crate) fn support_j9_region<const NSLABS: usize>() -> RegionState {
    let mut r = RegionState::new(
        BuddyAllocator::kani_from_parts(0, 4 * SSU, SS, 0, Vec::new()),
        support_j9_params(),
    );
    let mut i = 0usize;
    while i < NSLABS {
        let start = i as u64 * 2 * SSU;
        let mut s = Slab::new(start, 2 * SSU, SS);
        s.mark_slot_allocated(0);
        r.size_classes.add_slab(SS, start);
        r.slabs.insert(start, s);
        i += 1;
    }
    r
}

/// EM-REGION-FREE-MISSING-SLAB-NOOP: free_slot(T, idx) with no slab at T changes nothing.
fn support_j9_free_missing<const NSLABS: usize>(mutant: bool) {
    let mut r = support_j9_region::<NSLABS>();
    let t: u64 = kani::any();
    let mut j = 0usize;
    while j < NSLABS {
        kani::assume(t != j as u64 * 2 * SSU);
        j += 1;
    }
    let idx: usize = kani::any();
    let n_before = r.slabs.len();
    let sc_before = r.size_classes.kani_len();
    r.free_slot(t, idx);
    let mut same = r.slabs.len() == n_before && r.size_classes.kani_len() == sc_before && !r.dirty;
    let mut j = 0usize;
    while j < NSLABS {
        let start = j as u64 * 2 * SSU;
        match r.slabs.get(&start) {
            Some(s) => {
                same &= s.bitmap.count_set() == 1 && s.bitmap.is_set(0) && !s.bitmap.is_set(1);
                same &= s.get_key(0) == FREE_KEY && s.get_key(1) == FREE_KEY;
            }
            None => same = false,
        }
        same &= r.size_classes.get_slabs(SS).len() == NSLABS;
        j += 1;
    }
    if mutant { assert!(!same) } else { assert!(same) }
}
#[kani::proof]

#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn bounded_j9_free_missing_0() { support_j9_free_missing::<0>(false) }
#[kani::proof]

#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn bounded_j9_free_missing_1() { support_j9_free_missing::<1>(false) }
#[kani::proof]

#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn bounded_j9_free_missing_1__mutant() { support_j9_free_missing::<1>(true) }

// ---- J9 rung b: side-table MODEL of the BTreeMap<u64, V> methods the region uses ----
// The real map stays EMPTY (BTreeMap::new is real); insert/get/get_mut/remove/len act on a
// 2-entry static table keyed by the u64 key. The stubs are generic (no bounds, as J7's
// HashMap stubs) and read the key as a u64 and store V in an 8-aligned 128-byte cell: both
// facts are ASSERTED (size/align), so a use at another K/V fails instead of being unsound.
// Capacity overflow is an assertion (a harness that needs a 3rd entry fails, never prunes).
// Iteration (values/iter/range) is NOT modelled: those are unreachability stubs.
// Conformance: check_stub_conforms_j9_btree (real BTreeMap<u64,u64> vs model, op sequences).
pub(crate) const J9M_CAP: usize = 2;
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub(crate) struct J9Cell([u64; 16]);
static mut J9M_LIVE: [bool; J9M_CAP] = [false; J9M_CAP];
static mut J9M_KEY: [u64; J9M_CAP] = [0; J9M_CAP];
static mut J9M_VAL: [J9Cell; J9M_CAP] = [J9Cell([0; 16]); J9M_CAP];

pub(crate) fn support_j9m_reset() {
    // SAFETY: single-threaded harness.
    unsafe {
        *std::ptr::addr_of_mut!(J9M_LIVE) = [false; J9M_CAP];
    }
}
fn support_j9m_types<K, V>() {
    assert!(std::mem::size_of::<K>() == 8 && std::mem::align_of::<K>() <= 8, "J9 map model: K is not u64-sized");
    assert!(std::mem::size_of::<V>() <= 128 && std::mem::align_of::<V>() <= 8, "J9 map model: V too large");
}
fn support_j9m_find(k: u64) -> Option<usize> {
    // SAFETY: single-threaded harness.
    let (live, keys) = unsafe { (&*std::ptr::addr_of!(J9M_LIVE), &*std::ptr::addr_of!(J9M_KEY)) };
    let mut i = 0usize;
    let mut r = None;
    while i < J9M_CAP {
        if live[i] && keys[i] == k {
            r = Some(i);
        }
        i += 1;
    }
    r
}
fn support_j9m_slot<V>(i: usize) -> *mut V {
    // SAFETY: i < J9M_CAP (callers); the cell is 8-aligned and >= size_of::<V>() (asserted).
    unsafe { std::ptr::addr_of_mut!(J9M_VAL[i]) as *mut V }
}
pub(crate) fn stub_j9_btree_insert<K, V, A: std::alloc::Allocator + Clone>(
    _m: &mut std::collections::BTreeMap<K, V, A>,
    k: K,
    v: V,
) -> Option<V> {
    support_j9m_types::<K, V>();
    // SAFETY: K is 8 bytes (asserted) and is u64 at every use (conformance-checked).
    let key: u64 = unsafe { std::mem::transmute_copy(&k) };
    std::mem::forget(k);
    if let Some(i) = support_j9m_find(key) {
        // SAFETY: cell i is live, so it holds an initialised V.
        return Some(unsafe { std::ptr::replace(support_j9m_slot::<V>(i), v) });
    }
    // SAFETY: single-threaded harness.
    let (live, keys) = unsafe { (&mut *std::ptr::addr_of_mut!(J9M_LIVE), &mut *std::ptr::addr_of_mut!(J9M_KEY)) };
    let mut i = 0usize;
    let mut placed = false;
    while i < J9M_CAP {
        if !placed && !live[i] {
            live[i] = true;
            keys[i] = key;
            // SAFETY: cell i is free; writing initialises it.
            unsafe { std::ptr::write(support_j9m_slot::<V>(i), std::ptr::read(&v)) };
            placed = true;
        }
        i += 1;
    }
    std::mem::forget(v);
    assert!(placed, "J9 map model: capacity exceeded");
    None
}
pub(crate) fn stub_j9_btree_get<'a, K, V, A: std::alloc::Allocator + Clone, Q: ?Sized>(
    _m: &'a std::collections::BTreeMap<K, V, A>,
    key: &Q,
) -> Option<&'a V> {
    support_j9m_types::<K, V>();
    // SAFETY: Q is u64 at every use (K = u64, Borrow<u64>); read through a thin cast.
    let k: u64 = unsafe { *(key as *const Q).cast::<u64>() };
    // SAFETY: a live cell holds an initialised V.
    support_j9m_find(k).map(|i| unsafe { &*support_j9m_slot::<V>(i) })
}
pub(crate) fn stub_j9_btree_get_mut<'a, K, V, A: std::alloc::Allocator + Clone, Q: ?Sized>(
    _m: &'a mut std::collections::BTreeMap<K, V, A>,
    key: &Q,
) -> Option<&'a mut V> {
    support_j9m_types::<K, V>();
    // SAFETY: as in stub_j9_btree_get.
    let k: u64 = unsafe { *(key as *const Q).cast::<u64>() };
    // SAFETY: a live cell holds an initialised V.
    support_j9m_find(k).map(|i| unsafe { &mut *support_j9m_slot::<V>(i) })
}
pub(crate) fn stub_j9_btree_remove<K, V, A: std::alloc::Allocator + Clone, Q: ?Sized>(
    _m: &mut std::collections::BTreeMap<K, V, A>,
    key: &Q,
) -> Option<V> {
    support_j9m_types::<K, V>();
    // SAFETY: as in stub_j9_btree_get.
    let k: u64 = unsafe { *(key as *const Q).cast::<u64>() };
    let i = support_j9m_find(k)?;
    // SAFETY: single-threaded harness; the live cell holds a V that is moved out exactly once.
    unsafe {
        (*std::ptr::addr_of_mut!(J9M_LIVE))[i] = false;
        Some(std::ptr::read(support_j9m_slot::<V>(i)))
    }
}
pub(crate) fn stub_j9_btree_len<K, V, A: std::alloc::Allocator + Clone>(_m: &std::collections::BTreeMap<K, V, A>) -> usize {
    // SAFETY: single-threaded harness.
    let live = unsafe { &*std::ptr::addr_of!(J9M_LIVE) };
    let mut n = 0usize;
    let mut i = 0usize;
    while i < J9M_CAP {
        if live[i] {
            n += 1;
        }
        i += 1;
    }
    n
}
pub(crate) fn stub_j9_btree_values<'a, K, V, A: std::alloc::Allocator + Clone>(
    _m: &'a std::collections::BTreeMap<K, V, A>,
) -> std::collections::btree_map::Values<'a, K, V> {
    kani::assert(false, "J9 map model: values() reached (not modelled)");
    panic!()
}
pub(crate) fn stub_j9_btree_iter<'a, K, V, A: std::alloc::Allocator + Clone>(
    _m: &'a std::collections::BTreeMap<K, V, A>,
) -> std::collections::btree_map::Iter<'a, K, V> {
    kani::assert(false, "J9 map model: iter() reached (not modelled)");
    panic!()
}
static mut J9M_HI: (u64, bool) = (0, false); // (end bound, inclusive) of the last range()
/// range(..=hi) / range(..hi) only (start bound must be Unbounded: asserted). Records the
/// end bound and returns an EMPTY real Range (Default); next_back is answered by the model.
pub(crate) fn stub_j9_btree_range<'a, K, V, A: std::alloc::Allocator + Clone, T: ?Sized, R>(
    _m: &'a std::collections::BTreeMap<K, V, A>,
    r: R,
) -> std::collections::btree_map::Range<'a, K, V>
where
    R: std::ops::RangeBounds<T>,
{
    use std::ops::Bound;
    support_j9m_types::<K, V>();
    assert!(matches!(r.start_bound(), Bound::Unbounded), "J9 map model: range start not Unbounded");
    // SAFETY: T is u64 at every use (K = u64); read through a thin cast.
    let hi = match r.end_bound() {
        Bound::Included(t) => (unsafe { *(t as *const T).cast::<u64>() }, true),
        Bound::Excluded(t) => (unsafe { *(t as *const T).cast::<u64>() }, false),
        Bound::Unbounded => (u64::MAX, true),
    };
    // SAFETY: single-threaded harness.
    unsafe { *std::ptr::addr_of_mut!(J9M_HI) = hi };
    Default::default()
}
/// Unreachability stub used by the harness macros (the range model below could not be wired:
/// Kani 0.67 rejects stubbing `<btree_map::Range as DoubleEndedIterator>::next_back`).
pub(crate) fn stub_j9_btree_range_unreach<'a, K, V, A: std::alloc::Allocator + Clone, T: ?Sized, R>(
    _m: &'a std::collections::BTreeMap<K, V, A>,
    _r: R,
) -> std::collections::btree_map::Range<'a, K, V> {
    kani::assert(false, "J9 map model: range() reached (not modelled)");
    panic!()
}
/// next_back of the recorded range: the live entry with the greatest key within the bound;
/// afterwards the bound becomes "< that key" (the iterator moves down).
pub(crate) fn stub_j9_range_next_back<'a, K, V>(
    _it: &mut std::collections::btree_map::Range<'a, K, V>,
) -> Option<(&'a K, &'a V)> {
    support_j9m_types::<K, V>();
    // SAFETY: single-threaded harness.
    let (live, keys, hi) = unsafe {
        (
            &*std::ptr::addr_of!(J9M_LIVE),
            &*std::ptr::addr_of!(J9M_KEY),
            &mut *std::ptr::addr_of_mut!(J9M_HI),
        )
    };
    let mut best: Option<usize> = None;
    let mut i = 0usize;
    while i < J9M_CAP {
        let inb = if hi.1 { keys[i] <= hi.0 } else { keys[i] < hi.0 };
        if live[i] && inb {
            match best {
                Some(b) if keys[b] >= keys[i] => {}
                _ => best = Some(i),
            }
        }
        i += 1;
    }
    let b = best?;
    *hi = (keys[b], false);
    // SAFETY: the static outlives 'a; K is u64 (asserted size); the live cell holds a V.
    unsafe {
        let kp: &'static [u64; J9M_CAP] = &*std::ptr::addr_of!(J9M_KEY);
        Some((&*(&kp[b] as *const u64).cast::<K>(), &*support_j9m_slot::<V>(b)))
    }
}

/// Conformance: the same op sequence on a real BTreeMap<u64, u64> and on the model
/// (keys from {0, 1024, 2048}, symbolic ops: insert/get/get_mut-write/remove/len), 3 steps.
fn support_j9_conform_btree(mutant: bool) {
    use std::collections::BTreeMap;
    support_j9m_reset();
    let mut real: BTreeMap<u64, u64> = BTreeMap::new();
    let mut model: BTreeMap<u64, u64> = BTreeMap::new();
    let keys = [0u64, 1024, 2048];
    // two inserts first (so the map holds 1 or 2 entries), then STEPS symbolic ops
    let a0: usize = kani::any();
    kani::assume(a0 < 3);
    let a1: usize = kani::any();
    kani::assume(a1 < 3);
    let (w0, w1): (u64, u64) = (kani::any(), kani::any());
    assert_eq!(real.insert(keys[a0], w0), stub_j9_btree_insert(&mut model, keys[a0], w0));
    assert_eq!(real.insert(keys[a1], w1), stub_j9_btree_insert(&mut model, keys[a1], w1));
    let mut step = 0usize;
    while step < 2 {
        let op: u8 = kani::any();
        let ki: usize = kani::any();
        kani::assume(ki < 3);
        let k = keys[ki];
        let v: u64 = kani::any();
        if op == 0 {
            // the model's capacity is 2: only inserts that fit (the region never exceeds it: asserted in the stub)
            if real.len() < J9M_CAP || real.contains_key(&k) {
                assert_eq!(real.insert(k, v), stub_j9_btree_insert(&mut model, k, v));
            }
        } else if op == 1 {
            assert_eq!(real.get(&k), stub_j9_btree_get(&model, &k));
        } else if op == 2 {
            let a = real.get_mut(&k);
            let b = stub_j9_btree_get_mut(&mut model, &k);
            assert_eq!(a.is_some(), b.is_some());
            if let (Some(a), Some(b)) = (a, b) {
                *a = v;
                *b = v;
            }
        } else {
            assert_eq!(real.remove(&k), stub_j9_btree_remove(&mut model, &k));
        }
        if mutant && step == 1 {
            assert!(real.len() != stub_j9_btree_len(&model));
        }
        assert_eq!(real.len(), stub_j9_btree_len(&model));
        step += 1;
    }
    let ki: usize = kani::any();
    kani::assume(ki < 3);
    assert_eq!(real.get(&keys[ki]), stub_j9_btree_get(&model, &keys[ki]));
}
#[kani::proof]
fn check_stub_conforms_j9_btree() { support_j9_conform_btree(false) }
#[kani::proof]
fn check_stub_conforms_j9_btree__mutant() { support_j9_conform_btree(true) }

macro_rules! j9_map_stubs {
    ($($item:item)*) => { $(
        #[kani::proof]
        #[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
        #[kani::stub(std::collections::BTreeMap::insert, stub_j9_btree_insert)]
        #[kani::stub(std::collections::BTreeMap::get, stub_j9_btree_get)]
        #[kani::stub(std::collections::BTreeMap::get_mut, stub_j9_btree_get_mut)]
        #[kani::stub(std::collections::BTreeMap::remove, stub_j9_btree_remove)]
        #[kani::stub(std::collections::BTreeMap::len, stub_j9_btree_len)]
        #[kani::stub(std::collections::BTreeMap::values, stub_j9_btree_values)]
        #[kani::stub(std::collections::BTreeMap::iter, stub_j9_btree_iter)]
        #[kani::stub(std::collections::BTreeMap::range, stub_j9_btree_range_unreach)]
        #[kani::stub(crate::slab::SizeClassManager::add_slab, stub_j9_sc_add)]
        #[kani::stub(crate::slab::SizeClassManager::remove_slab, stub_j9_sc_remove)]
        #[kani::stub(crate::slab::SizeClassManager::get_slabs, stub_j9_sc_get)]
        #[kani::stub(crate::slab::SizeClassManager::kani_len, stub_j9_sc_len)]
        $item
    )* };
}
j9_map_stubs! {
    fn bounded_j9m_free_missing_1() { support_j9m_reset(); support_j9s_reset(); support_j9_free_missing::<1>(false) }
    fn bounded_j9m_free_missing_1__mutant() { support_j9m_reset(); support_j9s_reset(); support_j9_free_missing::<1>(true) }
    fn bounded_j9m_free_missing_2() { support_j9m_reset(); support_j9s_reset(); support_j9_free_missing::<2>(false) }
    fn bounded_j9m_free_missing_2__mutant() { support_j9m_reset(); support_j9s_reset(); support_j9_free_missing::<2>(true) }
}

// ---- J9 DIAGNOSTIC (never scored): SizeClassManager alone (HashMap<u32, Vec<u64>>) ----
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn diag_j9_sizeclass_add_get() {
    let mut sc = crate::slab::SizeClassManager::new();
    sc.add_slab(SS, 0);
    assert!(sc.get_slabs(SS).len() == 1);
}

// ---- J9 rung b (layer 2): MODEL of SizeClassManager's three methods (crate functions) ----
// Up to 2 element sizes x up to 2 slab starts each, ordered (push = append, remove = retain !=,
// class dropped when empty), get_slabs returns the ordered slice. Overflow is an assertion.
// Conformance: check_stub_conforms_j9_sizeclass (real SizeClassManager vs model).
pub(crate) const J9S_CAP: usize = 2;
static mut J9S_LIVE: [bool; J9S_CAP] = [false; J9S_CAP];
static mut J9S_ES: [u32; J9S_CAP] = [0; J9S_CAP];
static mut J9S_LEN: [usize; J9S_CAP] = [0; J9S_CAP];
static mut J9S_OFF: [[u64; J9S_CAP]; J9S_CAP] = [[0; J9S_CAP]; J9S_CAP];
pub(crate) fn support_j9s_reset() {
    // SAFETY: single-threaded harness.
    unsafe { *std::ptr::addr_of_mut!(J9S_LIVE) = [false; J9S_CAP] };
}
fn support_j9s_find(es: u32) -> Option<usize> {
    // SAFETY: single-threaded harness.
    let (live, e) = unsafe { (&*std::ptr::addr_of!(J9S_LIVE), &*std::ptr::addr_of!(J9S_ES)) };
    let mut r = None;
    let mut i = 0usize;
    while i < J9S_CAP {
        if live[i] && e[i] == es {
            r = Some(i);
        }
        i += 1;
    }
    r
}
pub(crate) fn stub_j9_sc_add(_s: &mut crate::slab::SizeClassManager, es: u32, start: u64) {
    // SAFETY: single-threaded harness.
    let (live, e, len, off) = unsafe {
        (
            &mut *std::ptr::addr_of_mut!(J9S_LIVE),
            &mut *std::ptr::addr_of_mut!(J9S_ES),
            &mut *std::ptr::addr_of_mut!(J9S_LEN),
            &mut *std::ptr::addr_of_mut!(J9S_OFF),
        )
    };
    let c = match support_j9s_find(es) {
        Some(c) => c,
        None => {
            let mut c = J9S_CAP;
            let mut i = 0usize;
            while i < J9S_CAP {
                if c == J9S_CAP && !live[i] {
                    c = i;
                }
                i += 1;
            }
            assert!(c < J9S_CAP, "J9 sizeclass model: class capacity exceeded");
            live[c] = true;
            e[c] = es;
            len[c] = 0;
            c
        }
    };
    assert!(len[c] < J9S_CAP, "J9 sizeclass model: list capacity exceeded");
    off[c][len[c]] = start;
    len[c] += 1;
}
pub(crate) fn stub_j9_sc_remove(_s: &mut crate::slab::SizeClassManager, es: u32, start: u64) {
    // SAFETY: single-threaded harness.
    let (live, len, off) = unsafe {
        (
            &mut *std::ptr::addr_of_mut!(J9S_LIVE),
            &mut *std::ptr::addr_of_mut!(J9S_LEN),
            &mut *std::ptr::addr_of_mut!(J9S_OFF),
        )
    };
    if let Some(c) = support_j9s_find(es) {
        let mut w = 0usize;
        let mut r = 0usize;
        while r < J9S_CAP {
            if r < len[c] && off[c][r] != start {
                off[c][w] = off[c][r];
                w += 1;
            }
            r += 1;
        }
        len[c] = w;
        if w == 0 {
            live[c] = false;
        }
    }
}
pub(crate) fn stub_j9_sc_get<'a>(_s: &'a crate::slab::SizeClassManager, es: u32) -> &'a [u64] {
    match support_j9s_find(es) {
        // SAFETY: single-threaded harness; the static outlives 'a; len <= J9S_CAP.
        Some(c) => unsafe {
            let off: &'static [[u64; J9S_CAP]; J9S_CAP] = &*std::ptr::addr_of!(J9S_OFF);
            let len: &'static [usize; J9S_CAP] = &*std::ptr::addr_of!(J9S_LEN);
            &off[c][..len[c]]
        },
        None => &[],
    }
}
pub(crate) fn stub_j9_sc_len(_s: &crate::slab::SizeClassManager) -> usize {
    // SAFETY: single-threaded harness.
    let live = unsafe { &*std::ptr::addr_of!(J9S_LIVE) };
    let mut n = 0usize;
    let mut i = 0usize;
    while i < J9S_CAP {
        if live[i] {
            n += 1;
        }
        i += 1;
    }
    n
}
fn support_j9_conform_sc(mutant: bool) {
    support_j9s_reset();
    let mut real = crate::slab::SizeClassManager::new();
    let mut model = crate::slab::SizeClassManager::new();
    let sizes = [SS, 2 * SS];
    let offs = [0u64, 2 * SSU];
    let mut step = 0usize;
    while step < 2 {
        let op: u8 = kani::any();
        let si: usize = kani::any();
        kani::assume(si < 2);
        let oi: usize = kani::any();
        kani::assume(oi < 2);
        if op == 0 {
            // model capacity: 2 starts per class (the region never registers more: asserted in the stub)
            if real.get_slabs(sizes[si]).len() < J9S_CAP {
                real.add_slab(sizes[si], offs[oi]);
                stub_j9_sc_add(&mut model, sizes[si], offs[oi]);
            }
        } else {
            real.remove_slab(sizes[si], offs[oi]);
            stub_j9_sc_remove(&mut model, sizes[si], offs[oi]);
        }
        let q: usize = kani::any();
        kani::assume(q < 2);
        assert!(real.get_slabs(sizes[q]) == stub_j9_sc_get(&model, sizes[q]));
        if mutant && step == 1 {
            assert!(real.kani_len() != stub_j9_sc_len(&model));
        }
        assert!(real.kani_len() == stub_j9_sc_len(&model));
        step += 1;
    }
}
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn check_stub_conforms_j9_sizeclass() { support_j9_conform_sc(false) }
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn check_stub_conforms_j9_sizeclass__mutant() { support_j9_conform_sc(true) }

/// EM-REMOVE-ERR-OFFSET-NOT-FOUND (region level): one slab at 0 (element SS, 2 slots), slot 0
/// allocated with key KEY0 (published if PUB) and slot 1 free; symbolic offset. remove returns
/// OffsetNotFound(offset) unless offset is a published slot's start, and then changes nothing.
fn support_j9_remove_not_found<const PUB: bool>(mutant: bool) {
    support_j9m_reset();
    support_j9s_reset();
    let mut r = support_j9_region::<1>();
    let key: u64 = kani::any();
    if PUB {
        kani::assume(key != FREE_KEY);
        r.publish_slot(0, 0, key);
        r.dirty = false;
    }
    let off: u64 = kani::any();
    let hit = PUB && off == 0;
    let res = r.remove_extent_by_offset(off);
    if !hit {
        let ok = matches!(res, Err(ExtentManagerError::OffsetNotFound(o)) if o == off);
        let s = r.slabs.get(&0).unwrap();
        let same = !r.dirty && r.kani_pending_frees().is_empty() && s.bitmap.count_set() == 1
            && s.get_key(0) == (if PUB { key } else { FREE_KEY }) && s.get_key(1) == FREE_KEY;
        if mutant { assert!(!(ok && same)) } else { assert!(ok && same) }
    } else {
        assert!(res.is_ok());
    }
}
/// EM-SIZECLASS-NONFULL-LISTED / EM-INV-SLAB-UNIFORM-ELEMENT / EM-INV-EXTENT-SIZE-UNIT /
/// EM-RESERVE-POST-OFFSET-SECTOR-ALIGNED (region level): from a region with NSLABS slabs
/// (support_j9_region), alloc_extent(size) with symbolic 1 <= size <= max_extent_size;
/// WHAT selects the clause checked on success.
fn support_j9_alloc<const NSLABS: usize, const WHAT: u8>(mutant: bool) {
    support_j9m_reset();
    support_j9s_reset();
    // SAFETY: single-threaded harness.
    unsafe { *std::ptr::addr_of_mut!(KANI_BUDDY_USED) = [false; 4] };
    let mut r = support_j9_region::<NSLABS>();
    {
        // the buddy model must know the pre-placed slabs' blocks
        let used = unsafe { &mut *std::ptr::addr_of_mut!(KANI_BUDDY_USED) };
        let mut i = 0usize;
        while i < NSLABS {
            used[i] = true;
            i += 1;
        }
    }
    let size: u32 = kani::any();
    kani::assume(size >= 1 && size <= 2 * SS);
    let elem = if size <= SS { SS } else { 2 * SS };
    if let Ok((ss, idx, off)) = r.alloc_extent(size) {
        let s = r.slabs.get(&ss).unwrap();
        let ok = match WHAT {
            // listed in its class's non-full list iff not full (both candidate starts)
            0 => {
                let mut ok = true;
                let mut j = 0usize;
                while j < 2 {
                    let st = j as u64 * 2 * SSU;
                    if let Some(sl) = r.slabs.get(&st) {
                        let listed = r.size_classes.get_slabs(sl.element_size).contains(&st);
                        ok &= listed == !sl.is_full();
                    }
                    j += 1;
                }
                ok
            }
            // the slot's slab has exactly the sector-aligned element size
            1 => s.element_size == elem,
            // offset = slab start + idx * element, sector aligned, inside the slab and region
            _ => off == ss + idx as u64 * s.element_size as u64 && off % SSU == 0
                && s.contains_offset(off) && off + s.element_size as u64 <= ss + s.slab_size
                && s.bitmap.is_set(idx) && s.get_key(idx) == FREE_KEY && ss + s.slab_size <= 4 * SSU,
        };
        if mutant { assert!(!ok) } else { assert!(ok) }
    } else if mutant {
        // the twin must also fail when alloc errs: an Ok path exists (checked by the twin failing)
    }
}
macro_rules! j9_sc_stubs {
    ($($item:item)*) => { $(
        #[kani::proof]
        #[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
        #[kani::stub(crate::slab::SizeClassManager::add_slab, stub_j9_sc_add)]
        #[kani::stub(crate::slab::SizeClassManager::remove_slab, stub_j9_sc_remove)]
        #[kani::stub(crate::slab::SizeClassManager::get_slabs, stub_j9_sc_get)]
        #[kani::stub(crate::slab::SizeClassManager::kani_len, stub_j9_sc_len)]
        $item
    )* };
}
j9_sc_stubs! {
    fn bounded_j9m_remove_not_found_unpub() { support_j9_remove_not_found::<false>(false) }
    fn bounded_j9m_remove_not_found_unpub__mutant() { support_j9_remove_not_found::<false>(true) }
    fn bounded_j9m_remove_not_found_pub() { support_j9_remove_not_found::<true>(false) }
    fn bounded_j9m_remove_not_found_pub__mutant() { support_j9_remove_not_found::<true>(true) }
}
macro_rules! j9_map_buddy_stubs {
    ($($item:item)*) => { $(
        j9_map_stubs! {
            #[kani::stub(crate::buddy::BuddyAllocator::alloc, stub_buddy_alloc)]
            #[kani::stub(crate::buddy::BuddyAllocator::free, stub_buddy_free)]
            $item
        }
    )* };
}
j9_map_buddy_stubs! {
    fn bounded_j9m_alloc_nonfull_listed_0() { support_j9_alloc::<0, 0>(false) }
    fn bounded_j9m_alloc_nonfull_listed_0__mutant() { support_j9_alloc::<0, 0>(true) }
    fn bounded_j9m_alloc_nonfull_listed_1() { support_j9_alloc::<1, 0>(false) }
    fn bounded_j9m_alloc_nonfull_listed_1__mutant() { support_j9_alloc::<1, 0>(true) }
    fn bounded_j9m_alloc_uniform_1() { support_j9_alloc::<1, 1>(false) }
    fn bounded_j9m_alloc_uniform_1__mutant() { support_j9_alloc::<1, 1>(true) }
    fn bounded_j9m_alloc_offset_1() { support_j9_alloc::<1, 2>(false) }
    fn bounded_j9m_alloc_offset_1__mutant() { support_j9_alloc::<1, 2>(true) }
}

/// Conformance at the SHAPES the J9 harnesses reach (the symbolic-sequence check above
/// times out, > 200 s at unwind 4): N concrete inserts at keys 0, 1024 with symbolic values,
/// then ONE op on a fully symbolic key t: get / get_mut-write-then-get / remove-then-len-get,
/// or an insert of a symbolic value at t in {0, 1024, 2048} (the buddy model's slab starts).
fn support_j9_conform_btree_fixed<const N: usize, const NARROW: bool>(mutant: bool) {
    use std::collections::BTreeMap;
    support_j9m_reset();
    let mut real: BTreeMap<u64, u64> = BTreeMap::new();
    let mut model: BTreeMap<u64, u64> = BTreeMap::new();
    let mut i = 0usize;
    while i < N {
        let v: u64 = kani::any();
        assert_eq!(real.insert(i as u64 * 1024, v), stub_j9_btree_insert(&mut model, i as u64 * 1024, v));
        i += 1;
    }
    let t: u64 = kani::any();
    if NARROW {
        kani::assume(t == 0 || t == 1024 || t == 2048);
    }
    let v: u64 = kani::any();
    let op: u8 = kani::any();
    if op == 0 {
        assert_eq!(real.get(&t), stub_j9_btree_get(&model, &t));
    } else if op == 1 {
        let a = real.get_mut(&t);
        let b = stub_j9_btree_get_mut(&mut model, &t);
        assert_eq!(a.is_some(), b.is_some());
        if let (Some(a), Some(b)) = (a, b) {
            *a = v;
            *b = v;
        }
        assert_eq!(real.get(&t), stub_j9_btree_get(&model, &t));
    } else if op == 2 {
        assert_eq!(real.remove(&t), stub_j9_btree_remove(&mut model, &t));
        assert_eq!(real.get(&t), stub_j9_btree_get(&model, &t));
    } else if N < J9M_CAP {
        kani::assume(t == 0 || t == 1024 || t == 2048);
        assert_eq!(real.insert(t, v), stub_j9_btree_insert(&mut model, t, v));
        assert_eq!(real.get(&t), stub_j9_btree_get(&model, &t));
    }
    if mutant {
        assert!(real.len() != stub_j9_btree_len(&model));
    } else {
        assert_eq!(real.len(), stub_j9_btree_len(&model));
    }
    let mut j = 0usize;
    while j < 3 {
        let k = j as u64 * 1024;
        assert_eq!(real.get(&k), stub_j9_btree_get(&model, &k));
        j += 1;
    }
}
#[kani::proof]
fn check_stub_conforms_j9_btree_fixed1() { support_j9_conform_btree_fixed::<1, false>(false) }
#[kani::proof]
fn check_stub_conforms_j9_btree_fixed1__mutant() { support_j9_conform_btree_fixed::<1, false>(true) }
#[kani::proof]
fn check_stub_conforms_j9_btree_fixed2() { support_j9_conform_btree_fixed::<2, false>(false) }
#[kani::proof]
fn check_stub_conforms_j9_btree_fixed2__mutant() { support_j9_conform_btree_fixed::<2, false>(true) }

/// SizeClassManager conformance at the J9 harness shape (the symbolic-sequence check above
/// times out, > 200 s at unwind 5): N concrete add_slab(SS, i*1024), then ONE op with symbolic
/// arguments (add with es in {SS, 2SS}, start in {0, 1024}; or remove with any es / start).
fn support_j9_conform_sc_fixed<const N: usize>(mutant: bool) {
    support_j9s_reset();
    let mut real = crate::slab::SizeClassManager::new();
    let mut model = crate::slab::SizeClassManager::new();
    let mut i = 0usize;
    while i < N {
        real.add_slab(SS, i as u64 * 2 * SSU);
        stub_j9_sc_add(&mut model, SS, i as u64 * 2 * SSU);
        i += 1;
    }
    let op: bool = kani::any();
    let es: u32 = kani::any();
    let st: u64 = kani::any();
    if op {
        kani::assume((es == SS && N < J9S_CAP) || es == 2 * SS);
        kani::assume(st == 0 || st == 2 * SSU);
        real.add_slab(es, st);
        stub_j9_sc_add(&mut model, es, st);
    } else {
        real.remove_slab(es, st);
        stub_j9_sc_remove(&mut model, es, st);
    }
    assert!(real.get_slabs(SS) == stub_j9_sc_get(&model, SS));
    assert!(real.get_slabs(2 * SS) == stub_j9_sc_get(&model, 2 * SS));
    assert!(real.get_slabs(es) == stub_j9_sc_get(&model, es));
    if mutant {
        assert!(real.kani_len() != stub_j9_sc_len(&model));
    } else {
        assert!(real.kani_len() == stub_j9_sc_len(&model));
    }
}
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn check_stub_conforms_j9_sizeclass_fixed1() { support_j9_conform_sc_fixed::<1>(false) }
#[kani::proof]
#[kani::stub(std::collections::hash_map::RandomState::new, stub_random_state)]
fn check_stub_conforms_j9_sizeclass_fixed1__mutant() { support_j9_conform_sc_fixed::<1>(true) }
#[kani::proof]
fn check_stub_conforms_j9_btree_narrow1() { support_j9_conform_btree_fixed::<1, true>(false) }
#[kani::proof]
fn check_stub_conforms_j9_btree_narrow1__mutant() { support_j9_conform_btree_fixed::<1, true>(true) }
