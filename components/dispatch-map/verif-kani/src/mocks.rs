// Providers for the three receptacles, and the Kani stubs every harness uses.
//
// RECEPTACLE PROVIDERS (mocks of the CONSUMED interfaces, not of dispatch-map):
//   MockEp      IEvictionPolicy. Records every call (create_pool, track, touch, remove,
//               get_eviction_candidates) in a fixed-size log so harnesses can state the
//               eviction-tracking obligations. It honours the IEvictionPolicy contract as
//               documented in interfaces/src/ieviction_policy.rs: `track` of a key already
//               tracked in the pool returns the existing handle; `get_eviction_candidates`
//               returns up to `n` keys currently tracked in that pool (which ones is left
//               NONDETERMINISTIC — the choice and order are "policy-defined").
//               Handle indices and the pool id are symbolic, so no proof can lean on a
//               particular numbering.
//   MockEm      IExtentManager. `for_each_extent` replays a harness-chosen list of extents
//               (symbolic offsets/sizes, concrete distinct keys). Every other method is
//               unreachable from IDispatchMap and panics if reached.
//   MockLogger  ILogger that discards messages. Most harnesses leave the logger receptacle
//               DISCONNECTED (a configuration the code supports: every log call is behind
//               `if let Ok(logger) = self.logger.get()`), which keeps `format!` out of the
//               formula; harnesses that need the connected path say so.
//
// STUBS (all disclosed in the advisory notes of the properties that use them):
//   concrete_state   RandomState::new -> fixed [0,0] SipHash seed (KD-HASHMAP-RANDOMSTATE).
//                    Sound for map SEMANTICS (insert/get/get_mut/remove/contains_key); nothing
//                    here asserts iteration order or hash values.
//   instant_now      Instant::now -> a fixed instant. Kani cannot execute clock_gettime. With a
//                    fixed clock, `deadline - now` is always the full 2 s timeout, so the code
//                    takes the "not yet expired" branch and goes to the condvar.
//   condvar_wait_timeout  Condvar::wait_timeout -> returns the SAME guard immediately, reporting
//                    `timed_out() == true`. This is the single-threaded model: no other thread
//                    exists to change the entry or to notify, so the only way the wait can end
//                    is by its deadline. The real `wait_for` loop then re-checks the predicate
//                    and returns (false, guard). Interleavings are NOT modelled (route: Loom).
//   condvar_notify_all  Condvar::notify_all -> increments NOTIFY_COUNT (no waiters exist in a
//                    single-threaded run, so waking nobody is exactly the real effect), letting
//                    harnesses observe THAT an operation wakes waiters.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LockResult, Mutex, MutexGuard, WaitTimeoutResult};
use std::time::{Duration, Instant};

use interfaces::{
    BlockSemantics, CacheKey, EvictionHandle, EvictionPolicyError, Extent, ExtentKey,
    ExtentManagerError, FormatParams, IDispatchMap, IEvictionPolicy, IExtentManager, ILogger,
    PoolId, WriteHandle,
};

use crate::entry::{DispatchEntry, Location};
use crate::{DispatchMapComponent, DispatchMapState};

// ------------------------------------------------------------------------------------- stubs

pub fn concrete_state() -> std::collections::hash_map::RandomState {
    let keys: [u64; 2] = [0, 0];
    unsafe { std::mem::transmute(keys) }
}

pub fn instant_now() -> Instant {
    // Instant on Linux is a Timespec { tv_sec: i64, tv_nsec: u32-in-[0,1e9) } (16 bytes);
    // all-zero is a valid value of every field.
    let raw: [u64; 2] = [0, 0];
    unsafe { std::mem::transmute(raw) }
}

pub static NOTIFY_COUNT: AtomicU32 = AtomicU32::new(0);
pub static WAIT_COUNT: AtomicU32 = AtomicU32::new(0);

pub fn condvar_notify_all(_cv: &Condvar) {
    NOTIFY_COUNT.fetch_add(1, Ordering::SeqCst);
}

pub fn condvar_wait_timeout<'a, T>(
    _cv: &Condvar,
    guard: MutexGuard<'a, T>,
    _dur: Duration,
) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
    WAIT_COUNT.fetch_add(1, Ordering::SeqCst);
    // WaitTimeoutResult is a newtype over bool; `true` = timed out.
    let r: WaitTimeoutResult = unsafe { std::mem::transmute(true) };
    Ok((guard, r))
}

pub fn notifies() -> u32 {
    NOTIFY_COUNT.load(Ordering::SeqCst)
}
pub fn waits() -> u32 {
    WAIT_COUNT.load(Ordering::SeqCst)
}

// ------------------------------------------------------------------------- eviction policy

pub const MAXT: usize = 3;

#[derive(Clone, Copy)]
pub struct TrackRec {
    pub active: bool,
    pub pool: PoolId,
    pub key: CacheKey,
    pub handle: EvictionHandle,
}

pub struct EpLog {
    pub pool_base: PoolId,
    pub pools_created: u32,
    pub tracks: [Option<TrackRec>; MAXT],
    pub ntracks: usize,
    pub track_calls: u32,
    pub touch_calls: u32,
    pub last_touch: Option<EvictionHandle>,
    pub remove_calls: u32,
    pub last_remove: Option<EvictionHandle>,
    pub cand_calls: u32,
    pub other_pool_calls: u32,
}

pub struct MockEp {
    pub log: Mutex<EpLog>,
}

impl MockEp {
    pub fn new() -> Self {
        MockEp {
            log: Mutex::new(EpLog {
                pool_base: kani::any(),
                pools_created: 0,
                tracks: [None; MAXT],
                ntracks: 0,
                track_calls: 0,
                touch_calls: 0,
                last_touch: None,
                remove_calls: 0,
                last_remove: None,
                cand_calls: 0,
                other_pool_calls: 0,
            }),
        }
    }

    /// Is `key` actively tracked in `pool`?
    pub fn is_tracked(&self, pool: PoolId, key: CacheKey) -> bool {
        self.handle_of(pool, key).is_some()
    }

    /// The handle under which `key` is actively tracked in `pool`, if any.
    pub fn handle_of(&self, pool: PoolId, key: CacheKey) -> Option<EvictionHandle> {
        let l = self.log.lock().unwrap();
        find_key(&l, pool, key).map(|i| l.tracks[i].unwrap().handle)
    }

    pub fn pools_created(&self) -> u32 {
        self.log.lock().unwrap().pools_created
    }
    pub fn track_calls(&self) -> u32 {
        self.log.lock().unwrap().track_calls
    }
    pub fn touch_calls(&self) -> u32 {
        self.log.lock().unwrap().touch_calls
    }
    pub fn remove_calls(&self) -> u32 {
        self.log.lock().unwrap().remove_calls
    }
    pub fn cand_calls(&self) -> u32 {
        self.log.lock().unwrap().cand_calls
    }
    pub fn last_remove(&self) -> Option<EvictionHandle> {
        self.log.lock().unwrap().last_remove
    }
    pub fn last_touch(&self) -> Option<EvictionHandle> {
        self.log.lock().unwrap().last_touch
    }
    /// Number of track/get_eviction_candidates calls that named a pool other than the first
    /// one this mock handed out.
    pub fn other_pool_calls(&self) -> u32 {
        self.log.lock().unwrap().other_pool_calls
    }
    pub fn pool(&self) -> PoolId {
        self.log.lock().unwrap().pool_base
    }

    /// Pre-register `key` in `pool` under `handle` (models an entry the harness pre-seeds).
    pub fn seed_track(&self, pool: PoolId, key: CacheKey, handle: EvictionHandle) {
        let mut l = self.log.lock().unwrap();
        // handles are unique among live tracked entries (see `track`)
        kani::assume(!is_handle(&l.tracks[0], handle));
        kani::assume(!is_handle(&l.tracks[1], handle));
        kani::assume(!is_handle(&l.tracks[2], handle));
        let n = l.ntracks;
        assert!(n < MAXT);
        l.tracks[n] = Some(TrackRec { active: true, pool, key, handle });
        l.ntracks = n + 1;
    }
}

fn is_key(t: &Option<TrackRec>, pool: PoolId, key: CacheKey) -> bool {
    match t {
        Some(t) => t.active && t.pool == pool && t.key == key,
        None => false,
    }
}
fn is_key_in(t: &Option<TrackRec>, pool: PoolId) -> bool {
    match t {
        Some(t) => t.active && t.pool == pool,
        None => false,
    }
}
fn is_handle(t: &Option<TrackRec>, h: EvictionHandle) -> bool {
    match t {
        Some(t) => t.active && t.handle == h,
        None => false,
    }
}
/// Slot actively tracking `key` in `pool` (unrolled over MAXT = 3).
fn find_key(l: &EpLog, pool: PoolId, key: CacheKey) -> Option<usize> {
    if is_key(&l.tracks[0], pool, key) {
        Some(0)
    } else if is_key(&l.tracks[1], pool, key) {
        Some(1)
    } else if is_key(&l.tracks[2], pool, key) {
        Some(2)
    } else {
        None
    }
}

impl IEvictionPolicy for MockEp {
    fn create_pool(&self) -> PoolId {
        let mut l = self.log.lock().unwrap();
        let id = l.pool_base.wrapping_add(l.pools_created);
        l.pools_created += 1;
        id
    }

    fn track(
        &self,
        pool: PoolId,
        key: CacheKey,
        _semantics: BlockSemantics,
    ) -> Result<EvictionHandle, EvictionPolicyError> {
        let mut l = self.log.lock().unwrap();
        l.track_calls += 1;
        if pool != l.pool_base {
            l.other_pool_calls += 1;
        }
        // Contract: re-registering a key already tracked in `pool` returns the existing handle.
        if let Some(i) = find_key(&l, pool, key) {
            return Ok(l.tracks[i].unwrap().handle);
        }
        let handle = EvictionHandle::new(pool, kani::any());
        // Contract (IEvictionPolicy: "a handle for O(1) touch/remove"): a handle identifies ONE
        // tracked entry, so a fresh handle never collides with a live one.
        kani::assume(!is_handle(&l.tracks[0], handle));
        kani::assume(!is_handle(&l.tracks[1], handle));
        kani::assume(!is_handle(&l.tracks[2], handle));
        let n = l.ntracks;
        assert!(n < MAXT, "mock eviction log full: raise MAXT");
        l.tracks[n] = Some(TrackRec { active: true, pool, key, handle });
        l.ntracks = n + 1;
        Ok(handle)
    }

    fn touch(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        let mut l = self.log.lock().unwrap();
        l.touch_calls += 1;
        l.last_touch = Some(handle);
        Ok(())
    }

    fn batch_touch(&self, _handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError> {
        unreachable!("batch_touch is not called by dispatch-map")
    }

    fn remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        let mut l = self.log.lock().unwrap();
        l.remove_calls += 1;
        l.last_remove = Some(handle);
        // unrolled over the MAXT = 3 slots (no loop, so no unwind bound is spent here)
        let hit = if is_handle(&l.tracks[0], handle) {
            Some(0)
        } else if is_handle(&l.tracks[1], handle) {
            Some(1)
        } else if is_handle(&l.tracks[2], handle) {
            Some(2)
        } else {
            None
        };
        match hit {
            Some(i) => {
                let mut t = l.tracks[i].unwrap();
                t.active = false;
                l.tracks[i] = Some(t);
                Ok(())
            }
            None => Err(EvictionPolicyError::InvalidHandle),
        }
    }

    fn identify_next_to_evict(&self, _pool: PoolId) -> Option<CacheKey> {
        unreachable!("identify_next_to_evict is not called by dispatch-map")
    }

    fn get_eviction_candidates(&self, pool: PoolId, n: usize) -> Vec<CacheKey> {
        let mut l = self.log.lock().unwrap();
        l.cand_calls += 1;
        if pool != l.pool_base {
            l.other_pool_calls += 1;
        }
        // Contract: up to n keys currently tracked in `pool`; the choice is policy-defined.
        // Unrolled over the MAXT = 3 slots.
        let mut v = Vec::new();
        if v.len() < n && is_key_in(&l.tracks[0], pool) && kani::any::<bool>() {
            v.push(l.tracks[0].unwrap().key);
        }
        if v.len() < n && is_key_in(&l.tracks[1], pool) && kani::any::<bool>() {
            v.push(l.tracks[1].unwrap().key);
        }
        if v.len() < n && is_key_in(&l.tracks[2], pool) && kani::any::<bool>() {
            v.push(l.tracks[2].unwrap().key);
        }
        v
    }

    fn len(&self, _pool: PoolId) -> usize {
        unreachable!("len is not called by dispatch-map")
    }

    fn clear_pool(&self, _pool: PoolId) {
        unreachable!("clear_pool is not called by dispatch-map")
    }
}

// --------------------------------------------------------------------------- extent manager

pub struct MockEm {
    pub extents: Vec<Extent>,
}

impl IExtentManager for MockEm {
    fn format(&self, _params: FormatParams) -> Result<(), ExtentManagerError> {
        unreachable!()
    }
    fn initialize(&self) -> Result<(), ExtentManagerError> {
        unreachable!()
    }
    fn reserve_extent(&self, _key: ExtentKey, _size: u32) -> Result<WriteHandle, ExtentManagerError> {
        unreachable!()
    }
    fn get_extents(&self) -> Vec<Extent> {
        unreachable!()
    }
    fn for_each_extent(&self, cb: &mut dyn FnMut(&Extent)) {
        // at most 2 extents, unrolled
        assert!(self.extents.len() <= 2);
        if self.extents.len() >= 1 {
            cb(&self.extents[0]);
        }
        if self.extents.len() >= 2 {
            cb(&self.extents[1]);
        }
    }
    fn remove_extent(&self, _offset: u64) -> Result<(), ExtentManagerError> {
        unreachable!()
    }
    fn checkpoint(&self) -> Result<(), ExtentManagerError> {
        unreachable!()
    }
    fn get_instance_id(&self) -> Result<u64, ExtentManagerError> {
        unreachable!()
    }
    fn set_checkpoint_interval(&self, _interval: Option<Duration>) {
        unreachable!()
    }
    fn used_bytes(&self) -> u64 {
        unreachable!()
    }
    fn capacity_bytes(&self) -> u64 {
        unreachable!()
    }
    fn set_metadata_base_lba(&self, _base_lba: u64) {
        unreachable!()
    }
    fn set_data_base_lba(&self, _base_lba: u64) {
        unreachable!()
    }
    fn data_base_lba(&self) -> u64 {
        unreachable!()
    }
}

// ---------------------------------------------------------------------------------- logger

pub struct MockLogger;
impl ILogger for MockLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

// --------------------------------------------------------------------------------- builders

/// A component with a connected eviction policy (the mock) and nothing else.
///
/// The component is a plain LOCAL value (see `component_local`), not the `Arc<Self>` the
/// generated `new()` returns. Measured: through the Arc, CBMC loses track of the receptacle
/// contents and expands every `if let Ok(x) = self.<receptacle>.get()` branch over all
/// candidate vtables — a two-line release_read harness then exceeds 200 s; as a local the same
/// harness verifies in 4.6 s. The IDispatchMap methods take `&self`, so this changes nothing
/// about what executes.
pub fn component_with_ep() -> (DispatchMapComponent, Arc<MockEp>) {
    let c = component_local();
    let ep = Arc::new(MockEp::new());
    let dynep: Arc<dyn IEvictionPolicy + Send + Sync> = ep.clone();
    c.eviction_policy.connect(dynep).unwrap();
    (c, ep)
}

/// A component with NO receptacle connected.
pub fn component_bare() -> DispatchMapComponent {
    component_local()
}

pub fn connect_em(c: &DispatchMapComponent, extents: Vec<Extent>) {
    let em: Arc<dyn IExtentManager + Send + Sync> = Arc::new(MockEm { extents });
    c.extent_manager.connect(em).unwrap();
}

pub fn connect_logger(c: &DispatchMapComponent) {
    let lg: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
    c.logger.connect(lg).unwrap();
}

/// An arbitrary entry: any location (block device at any offset, or memory tier with any
/// pointer/size/SSD copy), any reference counts, any handle, any block count and checksum.
/// Harnesses narrow it only with `kani::assume` of what their obligation states.
pub fn any_entry() -> DispatchEntry {
    let location = if kani::any() {
        Location::BlockDevice { offset: kani::any() }
    } else {
        let addr: usize = kani::any();
        Location::MemoryTier {
            pointer: addr as *mut u8,
            size: kani::any(),
            ssd_offset: if kani::any() { Some(kani::any()) } else { None },
        }
    };
    DispatchEntry {
        location,
        size_blocks: kani::any(),
        read_ref: kani::any(),
        write_ref: kani::any(),
        eviction_handle: EvictionHandle::new(kani::any(), kani::any()),
        reuse_count: AtomicU32::new(kani::any()),
        checksum: kani::any(),
    }
}

/// Insert `e` under `key` directly into the real map (pre-state construction).
pub fn seed(c: &DispatchMapComponent, key: CacheKey, e: DispatchEntry) {
    c.state.inner.lock().unwrap().entries.insert(key, e);
}

/// A plain-data snapshot of an entry, for before/after frame comparisons.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Snap {
    pub is_mem: bool,
    pub offset: u64,
    pub pointer: usize,
    pub size: u32,
    pub ssd: Option<u64>,
    pub size_blocks: u32,
    pub read_ref: u32,
    pub write_ref: u32,
    pub handle: EvictionHandle,
    pub checksum: u32,
}

pub fn snap_entry(e: &DispatchEntry) -> Snap {
    let (is_mem, offset, pointer, size, ssd) = match &e.location {
        Location::BlockDevice { offset } => (false, *offset, 0usize, 0u32, None),
        Location::MemoryTier { pointer, size, ssd_offset } => {
            (true, 0u64, *pointer as usize, *size, *ssd_offset)
        }
    };
    Snap {
        is_mem,
        offset,
        pointer,
        size,
        ssd,
        size_blocks: e.size_blocks,
        read_ref: e.read_ref,
        write_ref: e.write_ref,
        handle: e.eviction_handle,
        checksum: e.checksum,
    }
}

/// Snapshot of the entry for `key`, or None if absent.
pub fn snap(c: &DispatchMapComponent, key: CacheKey) -> Option<Snap> {
    c.state.inner.lock().unwrap().entries.get(&key).map(snap_entry)
}

pub fn len(c: &DispatchMapComponent) -> usize {
    c.state.inner.lock().unwrap().entries.len()
}

pub fn pool_of(c: &DispatchMapComponent) -> Option<PoolId> {
    *c.state.pool_id.lock().unwrap()
}

/// `alloc::fmt::format` (what `format!` expands to) -> an empty String. Every `format!` in
/// dispatch-map builds a log message handed to `ILogger`, whose provider here (MockLogger, or
/// a disconnected receptacle) discards it; no level-2 property is about log text. Without this
/// stub CBMC symbolically executes core::fmt (digit loops, `dyn fmt::Write`) on every branch
/// that could log, which by itself takes a two-line release_read harness past 200 s.
pub fn fmt_format(_args: std::fmt::Arguments<'_>) -> String {
    String::new()
}

/// The component as a plain local value (not behind the `Arc` the generated `new()` returns).
/// Same struct, same fields, same receptacle values (all disconnected) as `new()` produces.
pub fn component_local() -> DispatchMapComponent {
    DispatchMapComponent {
        __version: "0.2.0",
        state: DispatchMapState::new(),
        logger: component_framework::receptacle::Receptacle::new(),
        extent_manager: component_framework::receptacle::Receptacle::new(),
        eviction_policy: component_framework::receptacle::Receptacle::new(),
    }
}

// ------------------------------------------------------------------------- shared harness kit

/// Round a byte size up to whole 4 KiB blocks, in u64 (no wrap): the obligation's arithmetic,
/// stated independently of the code's `div_ceil`.
pub fn ceil_blocks(size: u32) -> u64 {
    (size as u64 + 4095) / 4096
}

/// A symbolic memory pointer (any address).
pub fn any_ptr() -> *mut u8 {
    let a: usize = kani::any();
    a as *mut u8
}

/// Make the pool exist (as `get_pool_id` would on first use) and return its id. Harnesses that
/// pre-seed entries call this so the seeded state is one the component could be in.
pub fn ensure_pool(c: &DispatchMapComponent, ep: &MockEp) -> PoolId {
    let id = ep.create_pool();
    *c.state.pool_id.lock().unwrap() = Some(id);
    id
}

/// Seed `key` and register it with the eviction policy under the entry's own handle.
pub fn seed_tracked(c: &DispatchMapComponent, ep: &MockEp, pool: PoolId, key: CacheKey, e: DispatchEntry) {
    let h = e.eviction_handle;
    ep.seed_track(pool, key, h);
    seed(c, key, e);
}

pub const OP_LOOKUP: u8 = 0;
pub const OP_CONVERT_TO_STORAGE: u8 = 1;
pub const OP_TAKE_READ: u8 = 2;
pub const OP_TAKE_WRITE: u8 = 3;
pub const OP_RELEASE_READ: u8 = 4;
pub const OP_RELEASE_WRITE: u8 = 5;
pub const OP_DOWNGRADE: u8 = 6;
pub const OP_REMOVE: u8 = 7;
pub const OP_TOUCH: u8 = 8;
pub const OP_ENTRY_SIZE: u8 = 9;
pub const OP_OLDEST_KEYS: u8 = 10;
pub const OP_CREATE: u8 = 11;
pub const OP_CONVERT_MTB: u8 = 12;
pub const OP_PROMOTE: u8 = 13;
pub const OP_IS_EVICTABLE: u8 = 14;
pub const OP_TRY_EVICT: u8 = 15;
pub const OP_RECOVER: u8 = 16;
pub const OP_SET_CHECKSUM: u8 = 17;
pub const OP_GET_CHECKSUM: u8 = 18;
pub const OP_INITIALIZE: u8 = 19;
pub const N_OPS: u8 = 20;

/// Run ONE IDispatchMap operation, chosen symbolically from `ops`, on key `k` with symbolic
/// arguments. Returns (op, returned-an-error). The only argument constraints are the declared
/// level-2 input-range assumptions: D-RANGE-FR-003-8e9f6a (memory-tier byte size
/// <= u32::MAX - 4095) and D-RANGE-FR-023-388782 (recovered block count <= u32::MAX / 4096);
/// they keep the created state inside the range the specification and code agree on.
pub fn run_op(c: &DispatchMapComponent, k: CacheKey, ops: &[u8]) -> (u8, bool) {
    let i: usize = kani::any();
    kani::assume(i < ops.len());
    run_op_const(c, k, ops[i])
}

/// The same, for an operation fixed at the call site. Harnesses over a SUBSET of operations
/// choose among `run_op_const(c, k, OP_X)` calls with literal OP_X, so symbolic execution only
/// enters the arms that can run (with `run_op`'s symbolic index it enters all 20).
pub fn run_op_const(c: &DispatchMapComponent, k: CacheKey, op: u8) -> (u8, bool) {
    let err = match op {
        OP_LOOKUP => c.lookup(k).is_err(),
        OP_CONVERT_TO_STORAGE => c.convert_to_storage(k, kani::any()).is_err(),
        OP_TAKE_READ => c.take_read(k).is_err(),
        OP_TAKE_WRITE => c.take_write(k).is_err(),
        OP_RELEASE_READ => c.release_read(k).is_err(),
        OP_RELEASE_WRITE => c.release_write(k).is_err(),
        OP_DOWNGRADE => c.downgrade_reference(k).is_err(),
        OP_REMOVE => c.remove(k).is_err(),
        OP_TOUCH => c.touch(k).is_err(),
        OP_ENTRY_SIZE => c.entry_size(k).is_err(),
        OP_OLDEST_KEYS => {
            let n: usize = kani::any();
            kani::assume(n <= 4);
            let _ = c.oldest_keys(n);
            false
        }
        OP_CREATE => {
            let size: u32 = kani::any();
            kani::assume(size <= u32::MAX - 4095); // D-RANGE-FR-003-8e9f6a
            c.create_memory_tier_entry(k, any_ptr(), size).is_err()
        }
        OP_CONVERT_MTB => c.convert_memory_tier_to_block(k).is_err(),
        OP_PROMOTE => {
            let size: u32 = kani::any();
            kani::assume(size <= u32::MAX - 4095); // D-RANGE-FR-003-8e9f6a
            c.promote_block_to_memory_tier(k, any_ptr(), size).is_err()
        }
        OP_IS_EVICTABLE => {
            let _ = c.is_evictable(k);
            false
        }
        OP_TRY_EVICT => c.try_evict_to_block(k).is_err(),
        OP_RECOVER => {
            let sb: u32 = kani::any();
            kani::assume(sb <= u32::MAX / 4096); // D-RANGE-FR-023-388782
            c.recover_extent(k, kani::any(), sb).is_err()
        }
        OP_SET_CHECKSUM => c.set_checksum(k, kani::any()).is_err(),
        OP_GET_CHECKSUM => {
            let _ = c.get_checksum(k);
            false
        }
        _ => c.initialize().is_err(),
    };
    (op, err)
}

/// Every operation, in a slice `run_op` can choose from.
pub const ALL_OPS: [u8; 20] = [
    OP_LOOKUP, OP_CONVERT_TO_STORAGE, OP_TAKE_READ, OP_TAKE_WRITE, OP_RELEASE_READ,
    OP_RELEASE_WRITE, OP_DOWNGRADE, OP_REMOVE, OP_TOUCH, OP_ENTRY_SIZE, OP_OLDEST_KEYS,
    OP_CREATE, OP_CONVERT_MTB, OP_PROMOTE, OP_IS_EVICTABLE, OP_TRY_EVICT, OP_RECOVER,
    OP_SET_CHECKSUM, OP_GET_CHECKSUM, OP_INITIALIZE,
];

/// An extent manager reporting `n <= 2` extents, keys 1 and 2 (distinct, so no key is reported
/// twice), symbolic offsets and block counts.
pub fn any_extents() -> Vec<Extent> {
    let n: usize = kani::any();
    kani::assume(n <= 2);
    let mut v = Vec::new();
    if n >= 1 {
        v.push(Extent { key: 1, size: kani::any(), offset: kani::any() });
    }
    if n >= 2 {
        v.push(Extent { key: 2, size: kani::any(), offset: kani::any() });
    }
    v
}
