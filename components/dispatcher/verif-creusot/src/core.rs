// core.rs — mirror types, the ghost world, and the contract-carrying `#[trusted]` boundaries
// that stand for every consumed receptacle (IDispatchMap, IMemoryTier, IGpuServices,
// IBlockDevice / IExtentManager / partition manager, IRemoteLookup, threads, clocks).
// Line references are to components/dispatcher/src/*.rs at pin 7d280cd3.
//
// MODEL: a single-threaded (sequential) execution of one dispatcher call. Each external
// decision the dispatcher cannot control (a GPU copy failing, the clock passing a deadline, the
// keys a memory tier reports as oldest, ...) is read from a per-call ORACLE indexed by a tick
// that every such call advances (`ob`, `ob2`, `ov`, `ok`). A `verify_*` driver starts from a
// symbolic state, so it covers EVERY sequence of external outcomes; a `refute_*` driver builds
// one concrete state + oracle and shows the property fails there.

use creusot_std::{logic::{FMap, Mapping, Seq}, prelude::*};

// ===========================================================================
// Mirror types
// ===========================================================================

/// `dispatch-map` `Location` (as observable through `IDispatchMap`).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Loc {
    Mem { ptr: u64, size: u32, ssd: Option<u64> },
    Blk { off: u64 },
}

/// One dispatch-map entry: location, the value `entry_size` reports (`es`), read/write refs.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Ent {
    pub loc: Loc,
    pub es: u32,
    pub rr: u32,
    pub wr: u32,
}

/// One memory-tier slot.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Slot {
    pub ptr: u64,
    pub size: u32,
}

/// `interfaces::LookupResult` (idispatch_map.rs:11-30).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Lr {
    NotExist,
    Mismatch,
    Blk { off: u64 },
    Mem { ptr: u64, size: u32 },
}

/// `interfaces::DispatchMapError` (payload strings dropped).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum DmE {
    KeyNotFound,
    AlreadyExists,
    ActiveRefs,
    Timeout,
    InvalidSize,
    Underflow,
    Overflow,
    NoWrite,
    InvalidState,
    Other,
}

/// `interfaces::MemoryTierError`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum MtE {
    PoolFull,
    AlreadyExists,
    KeyNotFound,
    Other,
}

/// Which dependency a `NotInitialized` names (the message text is not modelled; its subject is).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Ni {
    Dispatcher,
    DispatchMap,
    MemoryTier,
    Gpu,
    Spdk,
    Logger,
    Ring,
}

/// What an `IoError` reports.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum IoK {
    /// lib.rs:1775 — drive index, expected and found partition counts (+ remediation hint).
    Partition { drive: usize, expected: usize, found: usize },
    /// creating / wiring / formatting / recovering data drive `i`.
    Drive(usize),
    Gpu,
    Sync,
    Ssd,
    Promote,
    Dm,
    Remote,
    Stream,
    Channel,
    Other,
}

/// `interfaces::DispatcherError` (payload strings dropped, subjects kept).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum DE {
    NotInit(Ni),
    InvalidParam,
    KeyNotFound(u64),
    AlreadyExists(u64),
    Alloc,
    Io(IoK),
    Timeout,
}

/// `interfaces::ServedBy`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum ServedBy {
    Dram,
    Ssd,
    Remote,
    Miss,
    SizeMismatch,
    Error,
}

/// `interfaces::IpcHandle` (the GPU address as an integer; 0 = null).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Ipc {
    pub addr: u64,
    pub size: u32,
}

/// `background::WriteJob`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Job {
    pub key: u64,
    pub size: u32,
    pub dev: usize,
}

/// A recovered extent reported by `IExtentManager::for_each_extent`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Ext {
    pub key: u64,
    pub off: u64,
    pub size: u32,
}

/// `interfaces::RemoteLookupError` (outcome of one forwarded key).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Rl {
    Ok,
    NotFound,
    Transport,
}

/// Observable effects, in call order.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Ev {
    /// memory-tier -> GPU copy (`memcpy_h2d_async` when `stream != 0`, `dma_copy_to_device` else).
    H2D { src: u64, dst: u64, len: usize, stream: u64 },
    /// GPU -> memory-tier copy (`memcpy_d2h_async`).
    D2H { src: u64, dst: u64, len: usize, stream: u64 },
    Sync { stream: u64 },
    MtTouch { key: u64 },
    MtGet { key: u64 },
    MtBatchTouch,
    MtRemove { key: u64 },
    MtInsert { key: u64 },
    DmTouch { key: u64 },
    DmDemote { key: u64 },
    DmRemove { key: u64 },
    DmCreate { key: u64 },
    DmPromote { key: u64 },
    /// `release_read(key)`; `own` = number of read references this call acquired itself.
    RelRead { key: u64, own: Int },
    /// `release_write(key)`; `own` = number of write references this call acquired itself.
    RelWrite { key: u64, own: Int },
    Enq { job: Job },
    ExtFree { drive: usize, off: u64 },
    Recover { key: u64, off: u64, size: u32 },
    EmFormat { drive: usize },
    EmInit { drive: usize },
    Checkpoint { drive: usize },
    DevSignal { drive: usize },
    DevShutdown { drive: usize },
    DevDetach { drive: usize },
    Register,
    Unregister,
    Join,
    Leave,
    SsdRead { drive: usize, key: u64 },
    SsdWrite { drive: usize, key: u64 },
    RemoteFwd { key: u64, size: u32 },
    Spawn,
    Cpu { drive: usize, cpu: Option<usize> },
}

/// Cumulative tier event counters (`TierEventCounters`, lib.rs:106-144).
pub struct Ctr {
    pub promo_mem: Int,
    pub promo_gpu: Int,
    pub ev_mem: Int,
    pub ev_ssd: Int,
    pub hit_dram: Int,
    pub hit_ssd: Int,
    pub r_hit: Int,
    pub r_miss: Int,
    pub bp: Int,
    pub drop_full: Int,
    pub already: Int,
    pub blk_pin: Int,
    pub exhausted: Int,
    pub blk_unp: Int,
}

/// The ghost world: every consumed component's observable state plus the oracles.
pub struct W {
    pub dm: FMap<u64, Ent>,
    pub mt: FMap<u64, Slot>,
    pub used: Int,
    pub cap: Int,
    pub c: Ctr,
    /// pending (enqueued, not yet processed) background write-through jobs.
    pub wq: Seq<Job>,
    pub log: Seq<Ev>,
    pub tick: Int,
    pub ob: Mapping<Int, bool>,
    pub ob2: Mapping<Int, bool>,
    /// GPU copy / synchronize outcomes
    pub og: Mapping<Int, bool>,
    /// SSD read / write outcomes
    pub os: Mapping<Int, bool>,
    pub ov: Mapping<Int, u64>,
    pub ok: Mapping<Int, Seq<u64>>,
    /// `IExtentManager::for_each_extent` of drive i.
    pub ext: Mapping<Int, Seq<Ext>>,
    /// `IMemoryTier::pool_info`.
    pub pool: bool,
    /// cluster membership (IRemoteLookup join/leave)
    pub cluster: bool,
    /// running background threads that the dispatcher owns (writer threads + evictors).
    pub threads: Int,
    /// one per-drive read/write counter (`IBlockDevice::read_write_stats`, every field alike).
    pub rw: Mapping<Int, u64>,
    /// pooled (free) client channels of drive i's ChannelPool
    pub cfree: Mapping<Int, Int>,
    /// `WriteThroughPins::failed`: keys whose latest write-through did not land.
    pub wf: Mapping<u64, bool>,
}

/// Dispatcher program fields (`define_component!` fields, lib.rs:416-432) and receptacle bindings.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct P {
    pub init: bool,
    pub has_dm: bool,
    pub has_mt: bool,
    pub has_gpu: bool,
    pub has_spdk: bool,
    pub has_factory: bool,
    pub has_rl: bool,
    pub has_logger: bool,
    pub max_att: usize,
    pub bp_ms: u64,
    /// `data_drives.len()`
    pub nd: usize,
    pub writer: bool,
    pub ssd_ev: bool,
    pub mt_ev: bool,
    /// `pipeline_ring.is_some()` and its fields.
    pub ring: bool,
    pub chunk: usize,
    pub stg: bool,
    pub stg_bytes: usize,
    pub warm: u64,
}

pub struct S {
    pub p: P,
    pub g: Snapshot<W>,
}

// ===========================================================================
// Logic vocabulary
// ===========================================================================

#[logic(open)]
pub fn nx(w: W) -> W {
    pearlite! { W { tick: w.tick + 1, ..w } }
}

#[logic(open)]
pub fn nx2(w: W) -> W {
    pearlite! { W { tick: w.tick + 2, ..w } }
}

#[logic(open)]
pub fn lg(w: W, e: Ev) -> W {
    pearlite! { W { log: w.log.push_back(e), ..w } }
}

#[logic(open)]
pub fn o1(w: W) -> bool {
    pearlite! { w.ob[w.tick] }
}

#[logic(open)]
pub fn o_gpu(w: W) -> bool {
    pearlite! { w.og[w.tick] }
}

#[logic(open)]
pub fn o_ssd(w: W) -> bool {
    pearlite! { w.os[w.tick] }
}

/// Every future answer of oracle `m` (from tick `t` on) is true.
#[logic(open)]
pub fn all_from(m: Mapping<Int, bool>, t: Int) -> bool {
    pearlite! { forall<i: Int> i >= t ==> m[i] }
}

#[logic(open)]
pub fn o2(w: W) -> bool {
    pearlite! { w.ob2[w.tick] }
}

#[logic(open)]
pub fn has(w: W, k: u64) -> bool {
    pearlite! { w.dm.contains(k) }
}

#[logic(open)]
pub fn ent(w: W, k: u64) -> Ent {
    pearlite! { w.dm.lookup(k) }
}

#[logic(open)]
pub fn is_mem(l: Loc) -> bool {
    pearlite! { match l { Loc::Mem { .. } => true, Loc::Blk { .. } => false } }
}

#[logic(open)]
pub fn is_blk(l: Loc) -> bool {
    pearlite! { match l { Loc::Mem { .. } => false, Loc::Blk { .. } => true } }
}

#[logic(open)]
pub fn ssd_of(l: Loc) -> Option<u64> {
    pearlite! { match l { Loc::Mem { ssd, .. } => ssd, Loc::Blk { .. } => None } }
}

#[logic(open)]
pub fn persisted(l: Loc) -> bool {
    pearlite! { match l { Loc::Mem { ssd, .. } => ssd != None, Loc::Blk { .. } => false } }
}

#[logic(open)]
pub fn lr_of(l: Loc) -> Lr {
    pearlite! { match l {
        Loc::Blk { off } => Lr::Blk { off },
        Loc::Mem { ptr, size, .. } => Lr::Mem { ptr, size },
    } }
}

#[logic(open)]
pub fn demoted_loc(l: Loc) -> Loc {
    pearlite! { match l {
        Loc::Mem { ssd: Some(o), .. } => Loc::Blk { off: o },
        _ => l,
    } }
}

#[logic(open)]
pub fn rr_set(e: Ent, r: u32) -> Ent {
    pearlite! { Ent { rr: r, ..e } }
}

#[logic(open)]
pub fn mt_has(w: W, k: u64) -> bool {
    pearlite! { w.mt.contains(k) }
}

/// The memory tier holds no slot for `k` that the dispatch map does not point to.
#[logic(open)]
pub fn no_orphan(w: W, k: u64) -> bool {
    pearlite! { mt_has(w, k) ==>
        (has(w, k) && match ent(w, k).loc { Loc::Mem { ptr, .. } => ptr == w.mt.lookup(k).ptr, _ => false }) }
}

/// `w2` differs from `w1` only by appended log events and advanced oracle tick.
#[logic(open)]
pub fn same_state(w1: W, w2: W) -> bool {
    pearlite! { w2.dm == w1.dm && w2.mt == w1.mt && w2.used == w1.used && w2.cap == w1.cap
        && w2.wq == w1.wq && w2.pool == w1.pool && w2.cluster == w1.cluster && w2.threads == w1.threads }
}

/// The counters are unchanged.
#[logic(open)]
pub fn same_ctr(a: Ctr, b: Ctr) -> bool {
    pearlite! { a.promo_mem == b.promo_mem && a.promo_gpu == b.promo_gpu && a.ev_mem == b.ev_mem
        && a.ev_ssd == b.ev_ssd && a.hit_dram == b.hit_dram && a.hit_ssd == b.hit_ssd
        && a.r_hit == b.r_hit && a.r_miss == b.r_miss && a.bp == b.bp && a.drop_full == b.drop_full
        && a.already == b.already && a.blk_pin == b.blk_pin && a.exhausted == b.exhausted
        && a.blk_unp == b.blk_unp }
}

/// No counter decreased.
#[logic(open)]
pub fn mono_ctr(a: Ctr, b: Ctr) -> bool {
    pearlite! { a.promo_mem <= b.promo_mem && a.promo_gpu <= b.promo_gpu && a.ev_mem <= b.ev_mem
        && a.ev_ssd <= b.ev_ssd && a.hit_dram <= b.hit_dram && a.hit_ssd <= b.hit_ssd
        && a.r_hit <= b.r_hit && a.r_miss <= b.r_miss && a.bp <= b.bp && a.drop_full <= b.drop_full
        && a.already <= b.already && a.blk_pin <= b.blk_pin && a.exhausted <= b.exhausted
        && a.blk_unp <= b.blk_unp }
}

/// The log of `b` extends the log of `a`.
#[logic(open)]
pub fn log_ext(a: W, b: W) -> bool {
    pearlite! { a.log.len() <= b.log.len() && forall<i: Int> 0 <= i && i < a.log.len() ==> b.log[i] == a.log[i] }
}

/// No event logged between `a` and `b` is a DMA transfer.
#[logic(open)]
pub fn no_dma(a: W, b: W) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==>
        match b.log[i] { Ev::H2D { .. } => false, Ev::D2H { .. } => false, _ => true } }
}

/// No event logged between `a` and `b` refreshes the memory-tier LRU position of `k`.
#[logic(open)]
pub fn no_lru(a: W, b: W, k: u64) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==>
        b.log[i] != Ev::MtTouch { key: k } && b.log[i] != Ev::MtGet { key: k } && b.log[i] != Ev::MtBatchTouch }
}

/// Every `release_write` logged between `a` and `b` releases a write reference the caller acquired.
#[logic(open)]
pub fn own_writes(a: W, b: W) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==>
        match b.log[i] { Ev::RelWrite { own, .. } => own > 0, _ => true } }
}

/// Every `release_read` logged between `a` and `b` releases a read reference the caller acquired.
#[logic(open)]
pub fn own_reads(a: W, b: W) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==>
        match b.log[i] { Ev::RelRead { own, .. } => own > 0, _ => true } }
}

/// Every memory-tier slot freed between `a` and `b` was freed immediately after the dispatch map
/// demoted the same key (so the map no longer pointed at it).
#[logic(open)]
pub fn demote_before_free(a: W, b: W) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==>
        match b.log[i] { Ev::MtRemove { key } => i > a.log.len() && b.log[i - 1] == Ev::DmDemote { key }, _ => true } }
}

/// Event `e` was logged between `a` and `b`.
#[logic(open)]
pub fn logged(a: W, b: W, e: Ev) -> bool {
    pearlite! { exists<i: Int> a.log.len() <= i && i < b.log.len() && b.log[i] == e }
}

/// Count of events in `s` equal to `e`.
#[logic]
#[variant(s.len())]
pub fn cnt(s: Seq<Ev>, e: Ev) -> Int {
    pearlite! { if s.len() == 0 { 0 } else { cnt(s.subsequence(0, s.len() - 1), e) + if s[s.len() - 1] == e { 1 } else { 0 } } }
}

// ---------------------------------------------------------------------------
// Eviction frame: `b` is `a` after zero or more pin-safe clean demotions.
// ---------------------------------------------------------------------------

/// Key `k` was demoted cleanly: it was a persisted, unreferenced memory-tier entry, and is now the
/// same entry in block-device state at its SSD offset, with its slot freed.
#[logic(open)]
pub fn demoted_k(a: W, b: W, k: u64) -> bool {
    pearlite! { has(a, k) && persisted(ent(a, k).loc) && ent(a, k).rr@ == 0 && ent(a, k).wr@ == 0
        && b.dm.get(k) == Some(Ent { loc: demoted_loc(ent(a, k).loc), ..ent(a, k) })
        && !mt_has(b, k) }
}

#[logic(open)]
pub fn evict_frame(a: W, b: W) -> bool {
    pearlite! { (forall<k: u64> (b.dm.get(k) == a.dm.get(k) && b.mt.get(k) == a.mt.get(k)) || demoted_k(a, b, k))
        && b.cap == a.cap && b.wq == a.wq && b.pool == a.pool && b.cluster == a.cluster
        && b.threads == a.threads && log_ext(a, b) && demote_before_free(a, b) && no_dma(a, b) && no_reg(a, b) && no_ssd(a, b)
        && own_writes(a, b) && own_reads(a, b) && b.tick >= a.tick && b.ob == a.ob && b.ob2 == a.ob2 && b.og == a.og && b.os == a.os && b.ov == a.ov && b.ok == a.ok
        && b.ext == a.ext && b.rw == a.rw && b.cfree == a.cfree }
}

// ===========================================================================
// Helpers that stand for a Rust panic (debug build): a call that would overflow / unwrap None
// diverges, so a returning execution has the arithmetic result. (Partial correctness.)
// ===========================================================================

#[trusted]
#[check(terminates)]
#[ensures(a@ + b@ <= usize::MAX@ && result@ == a@ + b@)]
pub fn padd(a: usize, b: usize) -> usize {
    a + b
}

#[trusted]
#[check(terminates)]
#[ensures(a@ + b@ <= u64::MAX@ && result@ == a@ + b@)]
pub fn padd64(a: u64, b: u64) -> u64 {
    a + b
}

/// `Option::unwrap` (panics on None).
#[trusted]
#[check(terminates)]
#[ensures(o == Some(result))]
pub fn unwrap_or_panic<T>(o: Option<T>) -> T {
    o.unwrap()
}

/// `iter().map(|r| r.size as usize).sum()` (lib.rs:3441): panics on overflow in a debug build.
#[logic]
#[variant(r.len())]
pub fn sum_sz(r: Seq<Ipc>) -> Int {
    pearlite! { if r.len() == 0 { 0 } else { sum_sz(r.subsequence(0, r.len() - 1)) + r[r.len() - 1].size@ } }
}

#[logic]
#[ensures(sum_sz(r) >= 0)]
#[variant(r.len())]
pub fn sum_sz_nonneg(r: Seq<Ipc>) {
    pearlite! { if r.len() == 0 { () } else { sum_sz_nonneg(r.subsequence(0, r.len() - 1)) } }
}

/// Prefix sums of region sizes: `pre(r, i)` = total size of regions `0..i`.
#[logic(open)]
pub fn pre(r: Seq<Ipc>, i: Int) -> Int {
    pearlite! { sum_sz(r.subsequence(0, i)) }
}

#[logic]
#[requires(0 <= i && i < r.len())]
#[ensures(pre(r, i + 1) == pre(r, i) + r[i].size@)]
pub fn pre_step(r: Seq<Ipc>, i: Int) {
    proof_assert! { r.subsequence(0, i + 1).subsequence(0, i) == r.subsequence(0, i) };
}

#[logic]
#[ensures(pre(r, r.len()) == sum_sz(r))]
pub fn pre_full(r: Seq<Ipc>) {
    proof_assert! { r.subsequence(0, r.len()) == r };
}

#[logic]
#[ensures(pre(r, 0) == 0)]
pub fn pre_zero(r: Seq<Ipc>) {
    proof_assert! { r.subsequence(0, 0).len() == 0 };
}

#[logic]
#[requires(0 <= i && i <= j && j <= r.len())]
#[ensures(pre(r, i) <= pre(r, j))]
#[variant(j - i)]
pub fn pre_mono(r: Seq<Ipc>, i: Int, j: Int) {
    if i < j {
        pre_mono(r, i, j - 1);
        pre_step(r, j - 1);
    }
}

// ---------------------------------------------------------------------------
// Dispatch-map update shapes
// ---------------------------------------------------------------------------

#[logic(open)]
pub fn others(a: W, b: W, k: u64) -> bool {
    pearlite! { forall<j: u64> j != k ==> b.dm.get(j) == a.dm.get(j) }
}

/// Only the dispatch map changed.
#[logic(open)]
pub fn only_dm(a: W, b: W) -> bool {
    pearlite! { b == W { dm: b.dm, ..a } }
}

/// Only the dispatch map changed, and one event was logged.
#[logic(open)]
pub fn only_dm_log(a: W, b: W, e: Ev) -> bool {
    pearlite! { b == W { dm: b.dm, log: a.log.push_back(e), ..a } }
}

/// Entry `k` keeps location, size and write ref; its read ref changed by `d`; others unchanged.
#[logic(open)]
pub fn ent_rr(a: W, b: W, k: u64, d: Int) -> bool {
    pearlite! { has(a, k) && has(b, k) && ent(b, k).loc == ent(a, k).loc && ent(b, k).es == ent(a, k).es
        && ent(b, k).wr == ent(a, k).wr && ent(b, k).rr@ == ent(a, k).rr@ + d && others(a, b, k) }
}

/// The maintained dispatcher invariant: an initialized dispatcher has its two required
/// dependencies bound and its background writer running (initialize establishes it; every other
/// operation preserves it; shutdown clears `init`).
#[logic(open)]
pub fn pinv(p: P) -> bool {
    pearlite! { (p.init ==> (p.has_dm && p.has_mt && p.writer)) && (!p.init ==> !p.ssd_ev && !p.mt_ev) }
}

/// The level-2 assumption D-RANGE-US-5AS-4-a11766, at the background write-through: in
/// memory-tier-only mode every key that reaches it is at most u64::MAX / 4096.
#[logic(open)]
pub fn a_us5(p: P, k: u64) -> bool {
    pearlite! { p.nd@ == 0 ==> k@ <= u64::MAX@ / 4096 }
}

#[logic(open)]
pub fn a_us5_q(p: P, q: Seq<Job>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < q.len() ==> a_us5(p, q[i].key) }
}

/// The splitmix64 drive placement `drive_index(key, n)` (lib.rs:581), as a logic name.
#[logic(opaque)]
pub fn di(k: u64, n: usize) -> usize {
    dead
}

/// `drive_index(k, n) = splitmix64(k) % n < n` — the same fact the trusted `drive_index`
/// program contract states, made available where the function is not called.
#[trusted]
#[logic]
#[requires(n@ > 0)]
#[ensures(di(k, n)@ < n@)]
pub fn di_lt(k: u64, n: usize) {}

#[logic(open)]
pub fn is_cp(e: Ev) -> bool {
    pearlite! { match e { Ev::Checkpoint { .. } => true, _ => false } }
}

#[logic(open)]
pub fn is_dev(e: Ev) -> bool {
    pearlite! { match e { Ev::DevSignal { .. } => true, Ev::DevShutdown { .. } => true, Ev::DevDetach { .. } => true, _ => false } }
}

#[logic(open)]
pub fn is_devsd(e: Ev) -> bool {
    pearlite! { match e { Ev::DevShutdown { .. } => true, _ => false } }
}

/// Between positions `a` and the end of `w`'s log, every checkpoint precedes every device stop.
#[logic(open)]
pub fn cp_before_dev(w: W, a: Int) -> bool {
    pearlite! { forall<i: Int, j: Int> a <= i && i < w.log.len() && a <= j && j < w.log.len()
        && is_cp(w.log[i]) && is_dev(w.log[j]) ==> i < j }
}

/// Sum of the per-drive read/write counters of drives 0..n.
#[logic]
#[variant(n)]
pub fn rwsum(m: Mapping<Int, u64>, n: Int) -> Int {
    pearlite! { if n <= 0 { 0 } else { rwsum(m, n - 1) + m[n - 1]@ } }
}

/// No checkpoint or device-stop event was logged between `a` and `b`.
#[logic(open)]
pub fn no_cpdev(a: W, b: W) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==> !is_cp(b.log[i]) && !is_dev(b.log[i]) }
}

/// The environment parts no dispatcher data path changes (receptacle configuration + oracles).
#[logic(open)]
pub fn same_env(a: W, b: W) -> bool {
    pearlite! { b.pool == a.pool && b.cluster == a.cluster && b.cap == a.cap && b.ext == a.ext
        && b.rw == a.rw && b.ob == a.ob && b.ob2 == a.ob2 && b.og == a.og && b.os == a.os && b.ov == a.ov && b.ok == a.ok && b.threads == a.threads && b.cfree == a.cfree }
}

/// No registration (dispatch-map create) and no write-through enqueue was logged between a and b.
#[logic(open)]
pub fn no_reg(a: W, b: W) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==>
        match b.log[i] { Ev::DmCreate { .. } => false, Ev::Enq { .. } => false, _ => true } }
}

/// No SSD read was logged between `a` and `b`.
#[logic(open)]
pub fn no_ssd(a: W, b: W) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==>
        match b.log[i] { Ev::SsdRead { .. } => false, _ => true } }
}

/// `c` can be cleanly demoted (try_evict_to_block succeeds).
#[logic(open)]
pub fn evictable(w: W, c: u64) -> bool {
    pearlite! { has(w, c) && persisted(ent(w, c).loc) && ent(w, c).rr@ == 0 && ent(w, c).wr@ == 0 }
}

#[logic(open)]
pub fn dem_ent(e: Ent) -> Ent {
    pearlite! { Ent { loc: demoted_loc(e.loc), ..e } }
}

/// The first key of the oldest-keys answer at the current tick.
#[logic(open)]
pub fn cand0(w: W) -> u64 {
    pearlite! { w.ok[w.tick][0] }
}

/// A logged event stays logged as the log grows, and relative to any earlier starting point.
#[logic]
#[requires(logged(a, b, e) && log_ext(b, c) && g.log.len() <= a.log.len())]
#[ensures(logged(g, c, e))]
pub fn logged_ext(g: W, a: W, b: W, c: W, e: Ev) {}
