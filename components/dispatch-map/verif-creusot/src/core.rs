// core.rs — mirror types, trusted boundaries, and one mirror fn per IDispatchMap method.
// Line references are to components/dispatch-map/src/*.rs at pin 08a5ae88.

use creusot_std::{logic::FMap, prelude::*};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ===========================================================================
// Mirror types
// ===========================================================================

/// `interfaces::EvictionHandle` (ieviction_policy.rs:9-12), fields made public for the model.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Handle {
    pub pool_id: u32,
    pub index: u32,
}

/// `entry::Location` (entry.rs:9-19), verbatim (the raw `*mut u8` is kept as a plain value).
pub enum Location {
    BlockDevice { offset: u64 },
    MemoryTier { pointer: *mut u8, size: u32, ssd_offset: Option<u64> },
}

/// `entry::DispatchEntry` (entry.rs:28-43) with the `integrity-check` feature ON (the
/// `checksum` field exists). `reuse_count: AtomicU32` is mirrored as a plain `u32` updated with
/// `wrapping_add` (that is what `fetch_add(1, Relaxed)` does under the map's Mutex).
pub struct Entry {
    pub location: Location,
    pub size_blocks: u32,
    pub read_ref: u32,
    pub write_ref: u32,
    pub eviction_handle: Handle,
    pub reuse_count: u32,
    pub checksum: u32,
}

/// `interfaces::LookupResult` (idispatch_map.rs:11-30).
pub enum LookupResult {
    NotExist,
    MismatchSize,
    BlockDevice { offset: u64 },
    MemoryTier { pointer: *mut u8, size: u32 },
}

/// `interfaces::DispatchMapError` (idispatch_map.rs:39-62). The `String` payloads of
/// `NotInitialized` / `InvalidState` / `AllocationFailed` are dropped (message text is not part of
/// any level-2 obligation; string CONTENT is IX-CREUSOT-STRING-CONTENT anyway).
pub enum DmError {
    KeyNotFound(u64),
    AlreadyExists(u64),
    ActiveReferences(u64),
    Timeout(u64),
    AllocationFailed,
    InvalidSize,
    NotInitialized,
    RefCountUnderflow(u64),
    RefCountOverflow(u64),
    NoWriteReference(u64),
    InvalidState,
}

/// Outcome of a mirror fn whose real code may `unwrap()` a missing receptacle or a failed
/// `track` (lib.rs:55, :93, :392, :399, :573, :579). `Panic` stands for that unwrap aborting.
pub enum Out<T> {
    Ret(T),
    Panic,
}

/// `interfaces::Extent` (iextent_manager.rs:11-15).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Extent {
    pub key: u64,
    pub size: u32,
    pub offset: u64,
}

/// The tracking error of `IEvictionPolicy::track/touch/remove`.
pub enum EpError {
    InvalidPool,
    InvalidHandle,
}

// ===========================================================================
// Trusted boundary 1: std HashMap  ->  logic FMap ghost mirror (KD-STD-CONTAINER-NO-SPECS)
// ===========================================================================

/// `state::Inner.entries: HashMap<CacheKey, DispatchEntry>` (state.rs:12-14).
pub struct Entries(HashMap<u64, Entry>);

impl View for Entries {
    type ViewTy = FMap<u64, Entry>;
    #[logic(opaque)]
    fn view(self) -> FMap<u64, Entry> {
        dead
    }
}

impl Entries {
    #[trusted]
    #[ensures(forall<j: u64> result@.get(j) == None)]
    pub fn new() -> Self {
        Entries(HashMap::new())
    }

    #[trusted]
    #[ensures(result == (*self)@.contains(*k))]
    pub fn contains_key(&self, k: &u64) -> bool {
        self.0.contains_key(k)
    }

    #[trusted]
    #[ensures(match result { Some(e) => (*self)@.get(*k) == Some(*e), None => (*self)@.get(*k) == None })]
    pub fn get(&self, k: &u64) -> Option<&Entry> {
        self.0.get(k)
    }

    #[trusted]
    #[ensures(match result {
        Some(e) => (*self)@.get(*k) == Some(*e) && (^self)@.get(*k) == Some(^e)
                   && (forall<j: u64> j != *k ==> (^self)@.get(j) == (*self)@.get(j)),
        None => (*self)@.get(*k) == None && (forall<j: u64> (^self)@.get(j) == (*self)@.get(j)) })]
    pub fn get_mut(&mut self, k: &u64) -> Option<&mut Entry> {
        self.0.get_mut(k)
    }

    #[trusted]
    #[ensures((^self)@.get(k) == Some(v))]
    #[ensures(forall<j: u64> j != k ==> (^self)@.get(j) == (*self)@.get(j))]
    pub fn insert(&mut self, k: u64, v: Entry) -> Option<Entry> {
        self.0.insert(k, v)
    }

    #[trusted]
    #[ensures((^self)@.get(*k) == None)]
    #[ensures(forall<j: u64> j != *k ==> (^self)@.get(j) == (*self)@.get(j))]
    #[ensures(result == (*self)@.get(*k))]
    pub fn remove(&mut self, k: &u64) -> Option<Entry> {
        self.0.remove(k)
    }
}

// ===========================================================================
// Trusted boundary 2: the consumed IEvictionPolicy (ieviction_policy.rs:73-111)
// ===========================================================================

/// Logical state of the connected eviction-policy component, as far as its interface declares
/// it: how many pools exist, and which handle tracks which (pool, key). Ranking (recency /
/// frequency) is NOT modelled: `touch` is observably a no-op on this state.
pub struct Ep {
    pub npools: Snapshot<Int>,
    pub tracked: Snapshot<FMap<Handle, (u32, u64)>>,
}

impl Ep {
    /// `create_pool` — a new, independent (hence empty) pool.
    #[trusted]
    #[ensures(result@ < *(^self).npools && *(*self).npools <= *(^self).npools)]
    #[ensures(*(^self).tracked == *(*self).tracked)]
    #[ensures(forall<h: Handle> (*(*self).tracked).contains(h) ==> ((*(*self).tracked).lookup(h)).0 != result)]
    pub fn create_pool(&mut self) -> u32 {
        unimplemented!()
    }

    /// `track` — Ok with a handle into `pool` for an existing pool, `InvalidPool` otherwise.
    /// Re-tracking a key returns the existing handle (the interface's idempotence clause), so a
    /// returned handle that was already tracked was tracking this very (pool, key).
    #[trusted]
    #[ensures(match result {
        Ok(h) => pool@ < *(*self).npools && h.pool_id == pool
                 && *(^self).tracked == (*(*self).tracked).insert(h, (pool, key))
                 && ((*(*self).tracked).contains(h) ==> (*(*self).tracked).lookup(h) == (pool, key)),
        Err(_) => pool@ >= *(*self).npools && ^self == *self })]
    #[ensures(*(^self).npools == *(*self).npools)]
    pub fn track(&mut self, pool: u32, key: u64) -> Result<Handle, EpError> {
        unimplemented!()
    }

    /// `touch` — updates ranking only.
    #[trusted]
    #[ensures(^self == *self)]
    pub fn touch(&mut self, h: Handle) -> Result<(), EpError> {
        unimplemented!()
    }

    /// `remove` — stop tracking the entry.
    #[trusted]
    #[ensures(*(^self).tracked == (*(*self).tracked).remove(h))]
    #[ensures(*(^self).npools == *(*self).npools)]
    pub fn remove(&mut self, h: Handle) -> Result<(), EpError> {
        unimplemented!()
    }

    /// `get_eviction_candidates` — up to `n` keys tracked in `pool`.
    #[trusted]
    #[ensures(result@.len() <= n@)]
    #[ensures(forall<i: Int> 0 <= i && i < result@.len() ==>
              exists<h: Handle> (*(*self).tracked).get(h) == Some((pool, result@[i])))]
    pub fn get_eviction_candidates(&self, pool: u32, n: usize) -> Vec<u64> {
        unimplemented!()
    }
}

// ===========================================================================
// Trusted boundary 3: the consumed IExtentManager::for_each_extent (iextent_manager.rs:189)
// modelled as the sequence of extents it reports, visited once each, in order.
// ===========================================================================

pub struct Em {
    pub extents: Vec<Extent>,
}

// ===========================================================================
// Trusted boundary 4: Condvar notify / wait (state.rs:20, :67) — `epoch` counts notify_all calls
// ===========================================================================

pub struct Cv {
    pub epoch: Snapshot<Int>,
}

impl Cv {
    #[trusted]
    #[ensures(*(^self).epoch == *(*self).epoch + 1)]
    pub fn notify_all(&mut self) {}
}

// ===========================================================================
// The component state: DispatchMapComponent { state: DispatchMapState, receptacles }
// ===========================================================================

pub struct Dm {
    /// `state.inner` (the Mutex is a trusted boundary: every method body below is one critical
    /// section, executed with the lock held, exactly where the real code holds it).
    pub entries: Entries,
    /// `state.pool_id` (state.rs:21).
    pub pool_id: Option<u32>,
    /// `eviction_policy` receptacle: `None` = not connected.
    pub ep: Option<Ep>,
    /// `extent_manager` receptacle: `None` = not bound.
    pub em: Option<Em>,
    /// `state.condvar`.
    pub cv: Cv,
}

// ===========================================================================
// Logic vocabulary
// ===========================================================================

#[logic(open)]
pub fn has(d: Dm, k: u64) -> bool {
    pearlite! { d.entries@.contains(k) }
}

#[logic(open)]
pub fn ent(d: Dm, k: u64) -> Entry {
    pearlite! { d.entries@.lookup(k) }
}

#[logic(open)]
pub fn same_map(a: Dm, b: Dm) -> bool {
    pearlite! { forall<j: u64> a.entries@.get(j) == b.entries@.get(j) }
}

#[logic(open)]
pub fn others_same(a: Dm, b: Dm, k: u64) -> bool {
    pearlite! { forall<j: u64> j != k ==> a.entries@.get(j) == b.entries@.get(j) }
}

/// Everything outside the entry map is identical (pool, both receptacles, condvar).
#[logic(open)]
pub fn rest_same(a: Dm, b: Dm) -> bool {
    pearlite! { a.pool_id == b.pool_id && a.ep == b.ep && a.em == b.em && a.cv == b.cv }
}

/// As `rest_same`, but the condvar was notified exactly once.
#[logic(open)]
pub fn rest_notified(a: Dm, b: Dm) -> bool {
    pearlite! { b.pool_id == a.pool_id && b.ep == a.ep && b.em == a.em && *b.cv.epoch == *a.cv.epoch + 1 }
}

#[logic(open)]
pub fn trk(d: Dm) -> FMap<Handle, (u32, u64)> {
    pearlite! { match d.ep { Some(e) => *e.tracked, None => FMap::empty() } }
}

#[logic(open)]
pub fn npools(d: Dm) -> Int {
    pearlite! { match d.ep { Some(e) => *e.npools, None => 0 } }
}

#[logic(open)]
pub fn pool_fresh(d: Dm, p: u32) -> bool {
    pearlite! { forall<h: Handle> trk(d).contains(h) ==> (trk(d).lookup(h)).0 != p }
}

/// Fields other than the two reference counts (and reuse_count) are unchanged.
#[logic(open)]
pub fn keep_meta(a: Entry, b: Entry) -> bool {
    pearlite! { a.location == b.location && a.size_blocks == b.size_blocks
                && a.eviction_handle == b.eviction_handle && a.checksum == b.checksum }
}

/// Everything but the location is unchanged.
#[logic(open)]
pub fn keep_but_loc(a: Entry, b: Entry) -> bool {
    pearlite! { a.size_blocks == b.size_blocks && a.read_ref == b.read_ref && a.write_ref == b.write_ref
                && a.eviction_handle == b.eviction_handle && a.checksum == b.checksum
                && a.reuse_count == b.reuse_count }
}

#[logic(open)]
pub fn is_mt(l: Location) -> bool {
    pearlite! { match l { Location::MemoryTier { .. } => true, Location::BlockDevice { .. } => false } }
}

#[logic(open)]
pub fn ssd_of(l: Location) -> Option<u64> {
    pearlite! { match l { Location::MemoryTier { ssd_offset, .. } => ssd_offset, Location::BlockDevice { .. } => None } }
}

#[logic(open)]
pub fn lr_of(l: Location) -> LookupResult {
    pearlite! { match l {
        Location::BlockDevice { offset } => LookupResult::BlockDevice { offset },
        Location::MemoryTier { pointer, size, .. } => LookupResult::MemoryTier { pointer, size },
    } }
}

/// `size.div_ceil(4096)` (lib.rs:406, :494).
#[logic(open)]
pub fn ceil_blocks(size: Int) -> Int {
    pearlite! { (size + 4095) / 4096 }
}

// ---------------------------------------------------------------------------
// The maintained well-formedness invariant (conjunction of named halves)
// ---------------------------------------------------------------------------

/// Per-entry facts: at most one writer, writer excludes readers, block count small enough that
/// `size_blocks * 4096` fits in u32, and a memory-tier entry has a non-zero byte size whose
/// rounded-up block count is the recorded one.
#[logic(open)]
pub fn entry_ok(e: Entry) -> bool {
    pearlite! {
        e.write_ref@ <= 1
        && (e.write_ref@ != 0 ==> e.read_ref@ == 0)
        && e.size_blocks@ <= 1048575
        && match e.location {
            Location::MemoryTier { size, .. } => size@ > 0 && e.size_blocks@ == ceil_blocks(size@),
            Location::BlockDevice { .. } => true,
        }
    }
}

#[logic(open)]
pub fn wf_entries(d: Dm) -> bool {
    pearlite! { forall<k: u64> has(d, k) ==> entry_ok(ent(d, k)) }
}

/// The pool, once created, is a pool of the connected eviction policy.
#[logic(open)]
pub fn wf_pool(d: Dm) -> bool {
    pearlite! { match d.pool_id { Some(p) => d.ep != None && p@ < npools(d), None => true } }
}

/// Every entry is tracked, in the dispatch map's pool, under its own key, by its own handle.
#[logic(open)]
pub fn wf_fwd(d: Dm) -> bool {
    pearlite! { forall<k: u64> has(d, k) ==>
        match d.pool_id { Some(p) => trk(d).get(ent(d, k).eviction_handle) == Some((p, k)), None => false } }
}

/// Every handle tracked in the dispatch map's pool belongs to a live entry holding it.
#[logic(open)]
pub fn wf_back(d: Dm) -> bool {
    pearlite! { match d.pool_id {
        Some(p) => forall<h: Handle> trk(d).contains(h) && (trk(d).lookup(h)).0 == p ==>
                     has(d, (trk(d).lookup(h)).1) && ent(d, (trk(d).lookup(h)).1).eviction_handle == h,
        None => true } }
}

#[logic(open)]
pub fn wf(d: Dm) -> bool {
    pearlite! { wf_entries(d) && wf_pool(d) && wf_fwd(d) && wf_back(d) }
}

// ---------------------------------------------------------------------------
// Declared level-2 assumptions (verif/unified_properties.yaml: level2_assumptions)
// ---------------------------------------------------------------------------

/// D-RANGE-FR-003-8e9f6a: `size <= u32::MAX - 4095`.
#[logic(open)]
pub fn a_fr003(size: u32) -> bool {
    pearlite! { size@ <= u32::MAX@ - 4095 }
}

/// D-RANGE-FR-023-388782: `size_blocks <= u32::MAX / 4096`.
#[logic(open)]
pub fn a_fr023(size_blocks: u32) -> bool {
    pearlite! { size_blocks@ <= u32::MAX@ / 4096 }
}

/// D-RANGE-FR-023-388782 applied to every extent the extent manager reports (initialize).
#[logic(open)]
pub fn a_fr023_em(d: Dm) -> bool {
    pearlite! { match d.em { Some(em) => forall<i: Int> 0 <= i && i < em.extents@.len() ==> a_fr023(em.extents@[i].size),
                             None => true } }
}

/// D-RANGE-AS-1.3-193445: `!pointer.is_null()`.
#[logic(open)]
pub fn a_as13(pointer: *mut u8) -> bool {
    pearlite! { !pointer.is_null_logic() }
}

/// D-RANGE-FR-012-437b1e: `self.eviction_policy.get().is_ok()`.
#[logic(open)]
pub fn a_fr012(d: Dm) -> bool {
    pearlite! { d.ep != None }
}

/// D-RANGE-FR-020-fd9bf8: `for every extent e reported by for_each_extent:
/// !inner.entries.contains_key(&e.key)` — read where the assume_rust places it, inside the
/// callback: when extent i is visited, its key is not in the map. Since visiting extents
/// 0..i-1 inserted their keys, that is: no reported key is in the map when initialize starts,
/// and no two reported extents share a key.
#[logic(open)]
pub fn a_fr020(d: Dm) -> bool {
    pearlite! { match d.em {
        Some(em) => (forall<i: Int> 0 <= i && i < em.extents@.len() ==> !has(d, em.extents@[i].key))
                    && (forall<i: Int, j: Int> 0 <= i && i < j && j < em.extents@.len() ==>
                            em.extents@[i].key != em.extents@[j].key),
        None => true } }
}

#[logic(open)]
pub fn reported(d: Dm, k: u64) -> bool {
    pearlite! { match d.em { Some(em) => exists<i: Int> 0 <= i && i < em.extents@.len() && em.extents@[i].key == k,
                             None => false } }
}

// ===========================================================================
// Trusted boundary 5: the time-bounded Condvar wait (state.rs:73)
// ===========================================================================

/// `self.condvar.wait_timeout(guard, remaining)`: releases the lock, lets other callers run
/// whole critical sections of this same component (which preserve `wf`, as the per-method
/// modules prove), reacquires, and reports whether the wait timed out.
#[trusted]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn condvar_wait_timeout(d: &mut Dm, remaining: Duration) -> bool {
    unimplemented!()
}

/// The two wait predicates (lib.rs:123-126 / :204, and :235-238).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Pred {
    NoWriter,
    NoRefs,
}

#[logic(open)]
pub fn pred_holds(d: Dm, key: u64, p: Pred) -> bool {
    pearlite! { match d.entries@.get(key) {
        None => true,
        Some(e) => match p { Pred::NoWriter => e.write_ref@ == 0,
                             Pred::NoRefs => e.read_ref@ == 0 && e.write_ref@ == 0 } } }
}

/// The predicate closures (lib.rs:123-126, :204, :235-238).
#[ensures(result == pred_holds(*d, key, p))]
pub fn check_pred(d: &Dm, key: u64, p: Pred) -> bool {
    match d.entries.get(&key) {
        None => true,
        Some(e) => match p {
            Pred::NoWriter => e.write_ref == 0,
            Pred::NoRefs => e.read_ref == 0 && e.write_ref == 0,
        },
    }
}

/// `DispatchMapState::wait_for` (state.rs:53-79), line-faithful.
/// Returns with the lock held; the result is the predicate evaluated on the state it returns.
#[ensures(result == pred_holds(^d, key, p))]
#[ensures(pred_holds(*d, key, p) ==> ^d == *d)]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn wait_for(d: &mut Dm, timeout: Duration, key: u64, p: Pred) -> bool {
    let old = snapshot!(*d);
    let deadline = Instant::now() + timeout;
    #[invariant(pred_holds(*old, key, p) ==> *d == *old)]
    #[invariant(wf(*old) ==> wf(*d))]
    loop {
        if check_pred(d, key, p) {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let timed_out = condvar_wait_timeout(d, remaining);
        if timed_out && !check_pred(d, key, p) {
            return false;
        }
    }
}

// ===========================================================================
// Mirror of the methods
// ===========================================================================

/// `DispatchMapState::new` + `DispatchMapComponent::new` (state.rs:29-37): empty map, no pool.
#[ensures(forall<k: u64> !has(result, k))]
#[ensures(result.pool_id == None && result.ep == ep && result.em == em)]
#[ensures(wf(result))]
pub fn dm_new(ep: Option<Ep>, em: Option<Em>, cv: Cv) -> Dm {
    Dm { entries: Entries::new(), pool_id: None, ep, em, cv }
}

/// `get_pool_id` (lib.rs:50-59).
#[ensures((^d).entries == (*d).entries && (^d).em == (*d).em && (^d).cv == (*d).cv)]
#[ensures(trk(^d) == trk(*d) && npools(*d) <= npools(^d) && ((^d).ep == None) == ((*d).ep == None))]
#[ensures(match (*d).pool_id { Some(p) => result == Out::Ret(p) && ^d == *d, None => true })]
#[ensures((*d).pool_id == None && (*d).ep == None ==> result == Out::Panic && ^d == *d)]
#[ensures((*d).pool_id == None && (*d).ep != None ==>
          match result { Out::Ret(p) => (^d).pool_id == Some(p) && p@ < npools(^d) && pool_fresh(*d, p),
                         Out::Panic => false })]
#[ensures(match result { Out::Ret(p) => (^d).pool_id == Some(p), Out::Panic => ^d == *d })]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn get_pool_id(d: &mut Dm) -> Out<u32> {
    if let Some(id) = d.pool_id {
        return Out::Ret(id);
    }
    match &mut d.ep {
        None => Out::Panic,
        Some(ep) => {
            let id = ep.create_pool();
            d.pool_id = Some(id);
            Out::Ret(id)
        }
    }
}

/// `initialize` (lib.rs:67-116). Logging is not modelled.
#[ensures((*d).pool_id == None && (*d).ep == None ==> result == Out::Panic)]
#[ensures(wf(*d) && (*d).ep != None ==> result != Out::Panic)]
#[ensures((*d).ep == None && (*d).pool_id != None ==> result == Out::Ret(Err(DmError::NotInitialized)) && ^d == *d)]
#[ensures((*d).ep != None && (*d).em == None ==> result == Out::Ret(Ok(())) && same_map(^d, *d))]
#[ensures((*d).ep != None && match (*d).em { Some(em) => em.extents@.len() == 0, None => false } ==>
          result == Out::Ret(Ok(())) && same_map(^d, *d))]
#[ensures(result != Out::Panic && (*d).pool_id != None ==> npools(^d) == npools(*d))]
#[ensures(result != Out::Panic ==> forall<h: Handle> trk(^d).contains(h) && !trk(*d).contains(h) ==>
          Some((trk(^d).lookup(h)).0) == (^d).pool_id)]
#[ensures(result != Out::Panic ==> forall<h: Handle> trk(*d).contains(h) ==> trk(^d).get(h) == trk(*d).get(h))]
#[ensures(match result { Out::Ret(Err(e)) => e == DmError::NotInitialized && (*d).ep == None, _ => true })]
#[ensures(forall<k: u64> !reported(*d, k) ==> (^d).entries@.get(k) == (*d).entries@.get(k))]
#[ensures(wf(*d) && result == Out::Panic ==> ^d == *d)]
#[ensures(result != Out::Panic ==> (^d).em == (*d).em && (^d).cv == (*d).cv)]
#[ensures(result != Out::Panic ==> match (*d).pool_id { Some(p) => (^d).pool_id == Some(p), None => true })]
#[ensures(match result {
    Out::Ret(Ok(())) => match (*d).em {
        Some(em) => (^d).pool_id != None
            && (forall<i: Int> 0 <= i && i < em.extents@.len() ==> has(^d, em.extents@[i].key))
            && (a_fr020(*d) ==> forall<i: Int> 0 <= i && i < em.extents@.len() ==>
                    ent(^d, em.extents@[i].key).location == Location::BlockDevice { offset: em.extents@[i].offset }
                    && ent(^d, em.extents@[i].key).size_blocks == em.extents@[i].size
                    && ent(^d, em.extents@[i].key).read_ref@ == 0
                    && ent(^d, em.extents@[i].key).write_ref@ == 0
                    && ent(^d, em.extents@[i].key).checksum@ == 0
                    && Some(trk(^d).lookup(ent(^d, em.extents@[i].key).eviction_handle)) == (^d).pool_id.map_logic(|p| (p, em.extents@[i].key))
                    && trk(^d).contains(ent(^d, em.extents@[i].key).eviction_handle)),
        None => true },
    _ => true })]
#[ensures(wf(*d) && a_fr020(*d) && a_fr023_em(*d) && result != Out::Panic ==> wf_entries(^d))]
#[ensures(wf(*d) && a_fr020(*d) && a_fr023_em(*d) && result != Out::Panic ==> wf_pool(^d))]
#[ensures(wf(*d) && a_fr020(*d) && a_fr023_em(*d) && result != Out::Panic ==> wf_fwd(^d))]
#[ensures(wf(*d) && a_fr020(*d) && a_fr023_em(*d) && result != Out::Panic ==> wf_back(^d))]
pub fn dm_initialize(d: &mut Dm) -> Out<Result<(), DmError>> {
    let o = snapshot!(*d);
    let pool_id = match get_pool_id(d) {
        Out::Ret(p) => p,
        Out::Panic => return Out::Panic,
    };
    let o1 = snapshot!(*d);
    let entries = &mut d.entries;
    let ep = match &mut d.ep {
        Some(ep) => ep,
        None => return Out::Ret(Err(DmError::NotInitialized)),
    };
    let em = match &d.em {
        None => return Out::Ret(Ok(())),
        Some(em) => em,
    };
    let exts = &em.extents;
    let mut count: u64 = 0;
    let mut i: usize = 0;
    #[invariant(i@ <= exts@.len() && count@ == i@)]
    #[invariant(*ep.npools == npools(*o1))]
    #[invariant(forall<k: u64> (forall<j: Int> 0 <= j && j < i@ ==> exts@[j].key != k) ==>
                entries@.get(k) == (*o1).entries@.get(k))]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> entries@.contains(exts@[j].key))]
    #[invariant(forall<h: Handle> trk(*o1).contains(h) ==> (*ep.tracked).get(h) == trk(*o1).get(h))]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==>
                (*ep.tracked).get((entries@.lookup(exts@[j].key)).eviction_handle) == Some((pool_id, exts@[j].key)))]
    #[invariant(a_fr020(*o) ==> forall<j: Int> 0 <= j && j < i@ ==>
                (entries@.lookup(exts@[j].key)).location == Location::BlockDevice { offset: exts@[j].offset }
                && (entries@.lookup(exts@[j].key)).size_blocks == exts@[j].size
                && (entries@.lookup(exts@[j].key)).read_ref@ == 0
                && (entries@.lookup(exts@[j].key)).write_ref@ == 0
                && (entries@.lookup(exts@[j].key)).checksum@ == 0)]
    #[invariant(forall<h: Handle> (*ep.tracked).contains(h) && !trk(*o1).contains(h) ==>
                ((*ep.tracked).lookup(h)).0 == pool_id)]
    #[invariant(a_fr020(*o) ==> forall<h: Handle> (*ep.tracked).contains(h) && !trk(*o1).contains(h) ==>
                entries@.contains(((*ep.tracked).lookup(h)).1)
                && (entries@.lookup(((*ep.tracked).lookup(h)).1)).eviction_handle == h)]
    while i < exts.len() {
        let extent = &exts[i];
        let eviction_handle = match ep.track(pool_id, extent.key) {
            Ok(h) => h,
            Err(_) => return Out::Panic,
        };
        let entry = Entry {
            location: Location::BlockDevice { offset: extent.offset },
            size_blocks: extent.size,
            read_ref: 0,
            write_ref: 0,
            eviction_handle,
            reuse_count: 0,
            checksum: 0,
        };
        let _ = entries.insert(extent.key, entry);
        count += 1;
        i += 1;
    }
    Out::Ret(Ok(()))
}

/// `lookup` (lib.rs:118-164). Returns, as ghost, the state at the moment the wait returned
/// with the lock held (the linearization point of the call).
#[ensures(pred_holds(*d, key, Pred::NoWriter) ==> *result.1 == *d)]
#[ensures(wf(*d) ==> wf(*result.1))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
#[ensures(rest_same(*result.1, ^d))]
#[ensures(!pred_holds(*result.1, key, Pred::NoWriter) ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1))]
#[ensures(pred_holds(*result.1, key, Pred::NoWriter) && !has(*result.1, key) ==>
          result.0 == Ok(LookupResult::NotExist) && same_map(^d, *result.1))]
#[ensures(pred_holds(*result.1, key, Pred::NoWriter) && has(*result.1, key) && ent(*result.1, key).read_ref == u32::MAX ==>
          result.0 == Err(DmError::RefCountOverflow(key)) && same_map(^d, *result.1))]
#[ensures(pred_holds(*result.1, key, Pred::NoWriter) && has(*result.1, key) && ent(*result.1, key).read_ref@ < u32::MAX@ ==>
          result.0 == Ok(lr_of(ent(*result.1, key).location)) && has(^d, key)
          && ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1
          && ent(^d, key).write_ref == ent(*result.1, key).write_ref
          && keep_meta(ent(^d, key), ent(*result.1, key)) && others_same(^d, *result.1, key))]
pub fn dm_lookup(d: &mut Dm, key: u64) -> (Result<LookupResult, DmError>, Snapshot<Dm>) {
    let satisfied = wait_for(d, Duration::from_millis(2000), key, Pred::NoWriter);
    let mid = snapshot!(*d);
    if !satisfied {
        return (Err(DmError::Timeout(key)), mid);
    }
    let entry = match d.entries.get_mut(&key) {
        None => return (Ok(LookupResult::NotExist), mid),
        Some(e) => e,
    };
    entry.read_ref = match entry.read_ref.checked_add(1) {
        Some(v) => v,
        None => return (Err(DmError::RefCountOverflow(key)), mid),
    };
    entry.reuse_count = entry.reuse_count.wrapping_add(1);
    let handle = entry.eviction_handle;
    let result = match &entry.location {
        Location::BlockDevice { offset } => LookupResult::BlockDevice { offset: *offset },
        Location::MemoryTier { pointer, size, .. } => LookupResult::MemoryTier { pointer: *pointer, size: *size },
    };
    match &mut d.ep {
        Some(ep) => {
            let _ = ep.touch(handle);
        }
        None => {}
    }
    (Ok(result), mid)
}

/// `convert_to_storage` (lib.rs:166-198).
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && !is_mt(ent(*d, key).location) ==> result == Err(DmError::InvalidState) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && is_mt(ent(*d, key).location) ==> result == Ok(()) && rest_notified(*d, ^d) && has(^d, key)
          && others_same(^d, *d, key)
          && ent(^d, key).location == match ent(*d, key).location {
                 Location::MemoryTier { pointer, size, .. } => Location::MemoryTier { pointer, size, ssd_offset: Some(offset) },
                 Location::BlockDevice { offset: o } => Location::BlockDevice { offset: o } }
          && ent(^d, key).read_ref@ == (if ent(*d, key).read_ref@ > 0 { ent(*d, key).read_ref@ - 1 } else { 0 })
          && ent(^d, key).write_ref == ent(*d, key).write_ref
          && ent(^d, key).size_blocks == ent(*d, key).size_blocks
          && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle
          && ent(^d, key).checksum == ent(*d, key).checksum
          && ent(^d, key).reuse_count == ent(*d, key).reuse_count)]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_convert_to_storage(d: &mut Dm, key: u64, offset: u64) -> Result<(), DmError> {
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    match &mut entry.location {
        Location::MemoryTier { ssd_offset, .. } => {
            *ssd_offset = Some(offset);
        }
        Location::BlockDevice { .. } => {
            return Err(DmError::InvalidState);
        }
    }
    if entry.read_ref > 0 {
        entry.read_ref -= 1;
    }
    d.cv.notify_all();
    Ok(())
}

/// `take_read` (lib.rs:200-227).
#[ensures(pred_holds(*d, key, Pred::NoWriter) ==> *result.1 == *d)]
#[ensures(wf(*d) ==> wf(*result.1))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
#[ensures(rest_same(*result.1, ^d))]
#[ensures(!pred_holds(*result.1, key, Pred::NoWriter) ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1))]
#[ensures(pred_holds(*result.1, key, Pred::NoWriter) && !has(*result.1, key) ==>
          result.0 == Err(DmError::KeyNotFound(key)) && same_map(^d, *result.1))]
#[ensures(pred_holds(*result.1, key, Pred::NoWriter) && has(*result.1, key) && ent(*result.1, key).read_ref == u32::MAX ==>
          result.0 == Err(DmError::RefCountOverflow(key)) && same_map(^d, *result.1))]
#[ensures(pred_holds(*result.1, key, Pred::NoWriter) && has(*result.1, key) && ent(*result.1, key).read_ref@ < u32::MAX@ ==>
          result.0 == Ok(()) && has(^d, key)
          && ent(^d, key).read_ref@ == ent(*result.1, key).read_ref@ + 1
          && ent(^d, key).write_ref == ent(*result.1, key).write_ref
          && keep_meta(ent(^d, key), ent(*result.1, key)) && others_same(^d, *result.1, key))]
pub fn dm_take_read(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    let satisfied = wait_for(d, Duration::from_millis(2000), key, Pred::NoWriter);
    let mid = snapshot!(*d);
    if !satisfied {
        return (Err(DmError::Timeout(key)), mid);
    }
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return (Err(DmError::KeyNotFound(key)), mid),
    };
    entry.read_ref = match entry.read_ref.checked_add(1) {
        Some(v) => v,
        None => return (Err(DmError::RefCountOverflow(key)), mid),
    };
    entry.reuse_count = entry.reuse_count.wrapping_add(1);
    (Ok(()), mid)
}

/// `take_write` (lib.rs:229-255).
#[ensures(pred_holds(*d, key, Pred::NoRefs) ==> *result.1 == *d)]
#[ensures(wf(*d) ==> wf(*result.1))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
#[ensures(rest_same(*result.1, ^d))]
#[ensures(!pred_holds(*result.1, key, Pred::NoRefs) ==> result.0 == Err(DmError::Timeout(key)) && same_map(^d, *result.1))]
#[ensures(pred_holds(*result.1, key, Pred::NoRefs) && !has(*result.1, key) ==>
          result.0 == Err(DmError::KeyNotFound(key)) && same_map(^d, *result.1))]
#[ensures(pred_holds(*result.1, key, Pred::NoRefs) && has(*result.1, key) ==>
          result.0 == Ok(()) && has(^d, key)
          && ent(^d, key).write_ref@ == 1
          && ent(^d, key).read_ref == ent(*result.1, key).read_ref
          && ent(^d, key).reuse_count == ent(*result.1, key).reuse_count
          && keep_meta(ent(^d, key), ent(*result.1, key)) && others_same(^d, *result.1, key))]
pub fn dm_take_write(d: &mut Dm, key: u64) -> (Result<(), DmError>, Snapshot<Dm>) {
    let satisfied = wait_for(d, Duration::from_millis(2000), key, Pred::NoRefs);
    let mid = snapshot!(*d);
    if !satisfied {
        return (Err(DmError::Timeout(key)), mid);
    }
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return (Err(DmError::KeyNotFound(key)), mid),
    };
    entry.write_ref = 1;
    (Ok(()), mid)
}

/// `release_read` (lib.rs:257-275).
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && ent(*d, key).read_ref@ == 0 ==> result == Err(DmError::RefCountUnderflow(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && ent(*d, key).read_ref@ > 0 ==> result == Ok(()) && rest_notified(*d, ^d) && has(^d, key)
          && ent(^d, key).read_ref@ == ent(*d, key).read_ref@ - 1
          && ent(^d, key).write_ref == ent(*d, key).write_ref
          && ent(^d, key).reuse_count == ent(*d, key).reuse_count
          && keep_meta(ent(^d, key), ent(*d, key)) && others_same(^d, *d, key))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_release_read(d: &mut Dm, key: u64) -> Result<(), DmError> {
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    if entry.read_ref == 0 {
        return Err(DmError::RefCountUnderflow(key));
    }
    entry.read_ref -= 1;
    d.cv.notify_all();
    Ok(())
}

/// `release_write` (lib.rs:277-295).
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && ent(*d, key).write_ref@ == 0 ==> result == Err(DmError::RefCountUnderflow(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && ent(*d, key).write_ref@ != 0 ==> result == Ok(()) && rest_notified(*d, ^d) && has(^d, key)
          && ent(^d, key).write_ref@ == 0
          && ent(^d, key).read_ref == ent(*d, key).read_ref
          && ent(^d, key).reuse_count == ent(*d, key).reuse_count
          && keep_meta(ent(^d, key), ent(*d, key)) && others_same(^d, *d, key))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_release_write(d: &mut Dm, key: u64) -> Result<(), DmError> {
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    if entry.write_ref == 0 {
        return Err(DmError::RefCountUnderflow(key));
    }
    entry.write_ref = 0;
    d.cv.notify_all();
    Ok(())
}

/// `downgrade_reference` (lib.rs:297-322). NOTE the order at lib.rs:308-312: `write_ref = 0`
/// is assigned BEFORE the `checked_add(..)?` that can return RefCountOverflow.
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && ent(*d, key).write_ref@ == 0 ==> result == Err(DmError::NoWriteReference(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && ent(*d, key).write_ref@ != 0 && ent(*d, key).read_ref == u32::MAX ==>
          result == Err(DmError::RefCountOverflow(key)) && rest_same(*d, ^d) && has(^d, key)
          && ent(^d, key).write_ref@ == 0
          && ent(^d, key).read_ref == ent(*d, key).read_ref
          && ent(^d, key).reuse_count == ent(*d, key).reuse_count
          && keep_meta(ent(^d, key), ent(*d, key)) && others_same(^d, *d, key))]
#[ensures(has(*d, key) && ent(*d, key).write_ref@ != 0 && ent(*d, key).read_ref@ < u32::MAX@ ==>
          result == Ok(()) && rest_notified(*d, ^d) && has(^d, key)
          && ent(^d, key).write_ref@ == 0
          && ent(^d, key).read_ref@ == ent(*d, key).read_ref@ + 1
          && keep_meta(ent(^d, key), ent(*d, key)) && others_same(^d, *d, key))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_downgrade_reference(d: &mut Dm, key: u64) -> Result<(), DmError> {
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    if entry.write_ref == 0 {
        return Err(DmError::NoWriteReference(key));
    }
    entry.write_ref = 0;
    entry.read_ref = match entry.read_ref.checked_add(1) {
        Some(v) => v,
        None => return Err(DmError::RefCountOverflow(key)),
    };
    entry.reuse_count = entry.reuse_count.wrapping_add(1);
    d.cv.notify_all();
    Ok(())
}

/// `remove` (lib.rs:324-347).
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && (ent(*d, key).read_ref@ > 0 || ent(*d, key).write_ref@ > 0) ==>
          result == Err(DmError::ActiveReferences(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && ent(*d, key).read_ref@ == 0 && ent(*d, key).write_ref@ == 0 ==>
          result == Ok(()) && !has(^d, key) && others_same(^d, *d, key)
          && (^d).pool_id == (*d).pool_id && (^d).em == (*d).em && (^d).cv == (*d).cv
          && ((^d).ep == None) == ((*d).ep == None) && npools(^d) == npools(*d)
          && !trk(^d).contains(ent(*d, key).eviction_handle)
          && (forall<h: Handle> h != ent(*d, key).eviction_handle ==> trk(^d).get(h) == trk(*d).get(h)))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_remove(d: &mut Dm, key: u64) -> Result<(), DmError> {
    let entry = match d.entries.get(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    if entry.read_ref > 0 || entry.write_ref > 0 {
        return Err(DmError::ActiveReferences(key));
    }
    let handle = entry.eviction_handle;
    let _ = d.entries.remove(&key);
    match &mut d.ep {
        Some(ep) => {
            let _ = ep.remove(handle);
        }
        None => {}
    }
    Ok(())
}

/// `touch` (lib.rs:349-361).
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)))]
#[ensures(has(*d, key) ==> result == Ok(()))]
#[ensures(same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_touch(d: &mut Dm, key: u64) -> Result<(), DmError> {
    let entry = match d.entries.get(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    let handle = entry.eviction_handle;
    match &mut d.ep {
        Some(ep) => {
            let _ = ep.touch(handle);
        }
        None => {}
    }
    Ok(())
}

/// `entry_size` (lib.rs:363-370). `entry.size_blocks * 4096` is u32 arithmetic with no check;
/// it is mirrored as `checked_mul` whose `None` arm is the overflow (`Panic` = the debug-build
/// overflow panic; a release build would instead return a wrapped value).
#[ensures(!has(*d, key) ==> result == Out::Ret(Err(DmError::KeyNotFound(key))))]
#[ensures(has(*d, key) && ent(*d, key).size_blocks@ * 4096 <= u32::MAX@ ==>
          match result { Out::Ret(Ok(v)) => v@ == ent(*d, key).size_blocks@ * 4096, _ => false })]
#[ensures(has(*d, key) && ent(*d, key).size_blocks@ * 4096 > u32::MAX@ ==> result == Out::Panic)]
#[ensures(^d == *d)]
pub fn dm_entry_size(d: &mut Dm, key: u64) -> Out<Result<u32, DmError>> {
    let entry = match d.entries.get(&key) {
        Some(e) => e,
        None => return Out::Ret(Err(DmError::KeyNotFound(key))),
    };
    match entry.size_blocks.checked_mul(4096) {
        Some(v) => Out::Ret(Ok(v)),
        None => Out::Panic,
    }
}

/// `oldest_keys` (lib.rs:372-379).
#[ensures(same_map(^d, *d) && (^d).em == (*d).em && (^d).cv == (*d).cv && trk(^d) == trk(*d))]
#[ensures(match (*d).pool_id { Some(p) => (^d).pool_id == Some(p), None => true })]
#[ensures((*d).pool_id == None && (*d).ep == None ==> result == Out::Panic)]
#[ensures((*d).pool_id != None ==> npools(^d) == npools(*d))]
#[ensures((*d).pool_id != None && (*d).ep == None ==> match result { Out::Ret(v) => v@.len() == 0, Out::Panic => false })]
#[ensures((*d).ep != None ==> result != Out::Panic)]
#[ensures(match result {
    Out::Ret(v) => v@.len() <= n@
        && forall<i: Int> 0 <= i && i < v@.len() ==>
            exists<h: Handle> Some(trk(^d).lookup(h)) == (^d).pool_id.map_logic(|p| (p, v@[i])) && trk(^d).contains(h),
    Out::Panic => true })]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
#[ensures(result == Out::Panic ==> ^d == *d)]
pub fn dm_oldest_keys(d: &mut Dm, n: usize) -> Out<Vec<u64>> {
    let pool_id = match get_pool_id(d) {
        Out::Ret(p) => p,
        Out::Panic => return Out::Panic,
    };
    match &d.ep {
        Some(ep) => Out::Ret(ep.get_eviction_candidates(pool_id, n)),
        None => Out::Ret(Vec::new()),
    }
}

/// `create_memory_tier_entry` (lib.rs:381-424).
#[ensures(size@ == 0 ==> result == Out::Ret(Err(DmError::InvalidSize)) && ^d == *d)]
#[ensures(size@ != 0 && (*d).ep == None ==> result == Out::Panic)]
#[ensures(size@ != 0 && wf(*d) && (*d).ep != None ==> result != Out::Panic)]
#[ensures(result != Out::Panic && (*d).pool_id != None ==> npools(^d) == npools(*d))]
#[ensures(result != Out::Panic ==> forall<h: Handle> trk(^d).contains(h) && !trk(*d).contains(h) ==>
          Some((trk(^d).lookup(h)).0) == (^d).pool_id)]
#[ensures(result != Out::Panic ==> forall<h: Handle> trk(*d).contains(h) ==> trk(^d).get(h) == trk(*d).get(h))]
#[ensures(result != Out::Panic ==> (^d).em == (*d).em && (^d).cv == (*d).cv)]
#[ensures(result != Out::Panic ==> match (*d).pool_id { Some(p) => (^d).pool_id == Some(p), None => true })]
#[ensures(size@ != 0 && has(*d, key) && result != Out::Panic ==>
          result == Out::Ret(Err(DmError::AlreadyExists(key))) && same_map(^d, *d) && trk(^d) == trk(*d))]
#[ensures(match result {
    Out::Ret(Ok(())) => size@ != 0 && !has(*d, key) && has(^d, key) && others_same(^d, *d, key)
        && ent(^d, key).location == Location::MemoryTier { pointer, size, ssd_offset: None }
        && ent(^d, key).size_blocks@ == ceil_blocks(size@)
        && ent(^d, key).read_ref@ == 0 && ent(^d, key).write_ref@ == 1
        && ent(^d, key).checksum@ == 0
        && (^d).pool_id != None
        && Some(trk(^d).lookup(ent(^d, key).eviction_handle)) == (^d).pool_id.map_logic(|p| (p, key))
        && trk(^d).contains(ent(^d, key).eviction_handle),
    Out::Ret(Err(e)) => same_map(^d, *d) && trk(^d) == trk(*d)
        && (e == DmError::InvalidSize || e == DmError::AlreadyExists(key)),
    Out::Panic => true })]
#[ensures(wf(*d) && a_fr003(size) && result != Out::Panic ==> wf_entries(^d))]
#[ensures(wf(*d) && a_fr003(size) && result != Out::Panic ==> wf_pool(^d))]
#[ensures(wf(*d) && a_fr003(size) && result != Out::Panic ==> wf_fwd(^d))]
#[ensures(wf(*d) && a_fr003(size) && result != Out::Panic ==> wf_back(^d))]
#[ensures(result == Out::Panic ==> ^d == *d)]
pub fn dm_create_memory_tier_entry(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Out<Result<(), DmError>> {
    if size == 0 {
        return Out::Ret(Err(DmError::InvalidSize));
    }
    let pool_id = match get_pool_id(d) {
        Out::Ret(p) => p,
        Out::Panic => return Out::Panic,
    };
    let ep = match &mut d.ep {
        Some(ep) => ep,
        None => return Out::Panic,
    };
    if d.entries.contains_key(&key) {
        return Out::Ret(Err(DmError::AlreadyExists(key)));
    }
    let eviction_handle = match ep.track(pool_id, key) {
        Ok(h) => h,
        Err(_) => return Out::Panic,
    };
    let entry = Entry {
        location: Location::MemoryTier { pointer, size, ssd_offset: None },
        size_blocks: size.div_ceil(4096),
        read_ref: 0,
        write_ref: 1,
        eviction_handle,
        reuse_count: 0,
        checksum: 0,
    };
    let _ = d.entries.insert(key, entry);
    Out::Ret(Ok(()))
}

/// `convert_memory_tier_to_block` (lib.rs:426-465).
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) && ssd_of(ent(*d, key).location) == None ==> result == Err(DmError::InvalidState) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(has(*d, key) ==> match ssd_of(ent(*d, key).location) {
    Some(off) => result == Ok(()) && rest_notified(*d, ^d) && has(^d, key) && others_same(^d, *d, key)
                 && ent(^d, key).location == Location::BlockDevice { offset: off }
                 && keep_but_loc(ent(^d, key), ent(*d, key)),
    None => true })]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_convert_memory_tier_to_block(d: &mut Dm, key: u64) -> Result<(), DmError> {
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    match &entry.location {
        Location::MemoryTier { ssd_offset: Some(offset), .. } => {
            let offset = *offset;
            entry.location = Location::BlockDevice { offset };
        }
        Location::MemoryTier { ssd_offset: None, .. } => {
            return Err(DmError::InvalidState);
        }
        _ => {
            return Err(DmError::InvalidState);
        }
    }
    d.cv.notify_all();
    Ok(())
}

/// `promote_block_to_memory_tier` (lib.rs:467-513).
#[ensures(size@ == 0 ==> result == Err(DmError::InvalidSize) && ^d == *d)]
#[ensures(size@ != 0 && !has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(size@ != 0 && has(*d, key) && is_mt(ent(*d, key).location) ==> result == Err(DmError::InvalidState) && same_map(^d, *d) && rest_same(*d, ^d))]
#[ensures(size@ != 0 && has(*d, key) ==> match ent(*d, key).location {
    Location::BlockDevice { offset } => result == Ok(()) && rest_notified(*d, ^d) && has(^d, key) && others_same(^d, *d, key)
        && ent(^d, key).location == Location::MemoryTier { pointer, size, ssd_offset: Some(offset) }
        && ent(^d, key).size_blocks@ == ceil_blocks(size@)
        && ent(^d, key).read_ref == ent(*d, key).read_ref
        && ent(^d, key).write_ref == ent(*d, key).write_ref
        && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle
        && ent(^d, key).checksum == ent(*d, key).checksum
        && ent(^d, key).reuse_count == ent(*d, key).reuse_count,
    Location::MemoryTier { .. } => true })]
#[ensures(wf(*d) && a_fr003(size) ==> wf_entries(^d))]
#[ensures(wf(*d) && a_fr003(size) ==> wf_pool(^d))]
#[ensures(wf(*d) && a_fr003(size) ==> wf_fwd(^d))]
#[ensures(wf(*d) && a_fr003(size) ==> wf_back(^d))]
pub fn dm_promote_block_to_memory_tier(d: &mut Dm, key: u64, pointer: *mut u8, size: u32) -> Result<(), DmError> {
    if size == 0 {
        return Err(DmError::InvalidSize);
    }
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    match &entry.location {
        Location::BlockDevice { offset } => {
            let offset = *offset;
            entry.location = Location::MemoryTier { pointer, size, ssd_offset: Some(offset) };
            entry.size_blocks = size.div_ceil(4096);
        }
        Location::MemoryTier { .. } => {
            return Err(DmError::InvalidState);
        }
    }
    d.cv.notify_all();
    Ok(())
}

#[logic(open)]
pub fn evictable(e: Entry) -> bool {
    pearlite! { e.read_ref@ == 0 && e.write_ref@ == 0 && is_mt(e.location) && ssd_of(e.location) != None }
}

/// `is_evictable` (lib.rs:515-531).
#[ensures(result == (has(*d, key) && evictable(ent(*d, key))))]
#[ensures(^d == *d)]
pub fn dm_is_evictable(d: &mut Dm, key: u64) -> bool {
    match d.entries.get(&key) {
        Some(entry) => {
            entry.read_ref == 0
                && entry.write_ref == 0
                && match entry.location {
                    Location::MemoryTier { ssd_offset: Some(_), .. } => true,
                    _ => false,
                }
        }
        None => false,
    }
}

/// `try_evict_to_block` (lib.rs:533-564).
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d))]
#[ensures(has(*d, key) && !evictable(ent(*d, key)) ==> result == Err(DmError::InvalidState) && same_map(^d, *d))]
#[ensures(has(*d, key) && evictable(ent(*d, key)) ==> result == Ok(()) && has(^d, key) && others_same(^d, *d, key)
          && Some(ent(^d, key).location) == ssd_of(ent(*d, key).location).map_logic(|o| Location::BlockDevice { offset: o })
          && keep_but_loc(ent(^d, key), ent(*d, key)))]
#[ensures(rest_same(*d, ^d))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_try_evict_to_block(d: &mut Dm, key: u64) -> Result<(), DmError> {
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    if entry.read_ref != 0 || entry.write_ref != 0 {
        return Err(DmError::InvalidState);
    }
    match &entry.location {
        Location::MemoryTier { ssd_offset: Some(offset), .. } => {
            let offset = *offset;
            entry.location = Location::BlockDevice { offset };
            Ok(())
        }
        Location::MemoryTier { ssd_offset: None, .. } => Err(DmError::InvalidState),
        _ => Err(DmError::InvalidState),
    }
}

/// `recover_extent` (lib.rs:566-592).
#[ensures((*d).ep == None ==> result == Out::Panic)]
#[ensures(wf(*d) && (*d).ep != None ==> result != Out::Panic)]
#[ensures(result != Out::Panic && (*d).pool_id != None ==> npools(^d) == npools(*d))]
#[ensures(result != Out::Panic ==> forall<h: Handle> trk(^d).contains(h) && !trk(*d).contains(h) ==>
          Some((trk(^d).lookup(h)).0) == (^d).pool_id)]
#[ensures(result != Out::Panic ==> forall<h: Handle> trk(*d).contains(h) ==> trk(^d).get(h) == trk(*d).get(h))]
#[ensures(result != Out::Panic ==> (^d).em == (*d).em && (^d).cv == (*d).cv)]
#[ensures(result != Out::Panic ==> match (*d).pool_id { Some(p) => (^d).pool_id == Some(p), None => true })]
#[ensures(has(*d, key) && result != Out::Panic ==>
          result == Out::Ret(Err(DmError::AlreadyExists(key))) && same_map(^d, *d) && trk(^d) == trk(*d))]
#[ensures(match result {
    Out::Ret(Ok(())) => !has(*d, key) && has(^d, key) && others_same(^d, *d, key)
        && ent(^d, key).location == Location::BlockDevice { offset }
        && ent(^d, key).size_blocks == size_blocks
        && ent(^d, key).read_ref@ == 0 && ent(^d, key).write_ref@ == 0
        && ent(^d, key).checksum@ == 0
        && (^d).pool_id != None
        && Some(trk(^d).lookup(ent(^d, key).eviction_handle)) == (^d).pool_id.map_logic(|p| (p, key))
        && trk(^d).contains(ent(^d, key).eviction_handle),
    Out::Ret(Err(e)) => same_map(^d, *d) && trk(^d) == trk(*d) && e == DmError::AlreadyExists(key),
    Out::Panic => true })]
#[ensures(wf(*d) && a_fr023(size_blocks) && result != Out::Panic ==> wf_entries(^d))]
#[ensures(wf(*d) && a_fr023(size_blocks) && result != Out::Panic ==> wf_pool(^d))]
#[ensures(wf(*d) && a_fr023(size_blocks) && result != Out::Panic ==> wf_fwd(^d))]
#[ensures(wf(*d) && a_fr023(size_blocks) && result != Out::Panic ==> wf_back(^d))]
#[ensures(result == Out::Panic ==> ^d == *d)]
pub fn dm_recover_extent(d: &mut Dm, key: u64, offset: u64, size_blocks: u32) -> Out<Result<(), DmError>> {
    let pool_id = match get_pool_id(d) {
        Out::Ret(p) => p,
        Out::Panic => return Out::Panic,
    };
    let ep = match &mut d.ep {
        Some(ep) => ep,
        None => return Out::Panic,
    };
    if d.entries.contains_key(&key) {
        return Out::Ret(Err(DmError::AlreadyExists(key)));
    }
    let eviction_handle = match ep.track(pool_id, key) {
        Ok(h) => h,
        Err(_) => return Out::Panic,
    };
    let entry = Entry {
        location: Location::BlockDevice { offset },
        size_blocks,
        read_ref: 0,
        write_ref: 0,
        eviction_handle,
        reuse_count: 0,
        checksum: 0,
    };
    let _ = d.entries.insert(key, entry);
    Out::Ret(Ok(()))
}

/// `set_checksum` (lib.rs:594-603).
#[ensures(!has(*d, key) ==> result == Err(DmError::KeyNotFound(key)) && same_map(^d, *d))]
#[ensures(has(*d, key) ==> result == Ok(()) && has(^d, key) && others_same(^d, *d, key)
          && ent(^d, key).checksum == checksum
          && ent(^d, key).location == ent(*d, key).location
          && ent(^d, key).size_blocks == ent(*d, key).size_blocks
          && ent(^d, key).read_ref == ent(*d, key).read_ref
          && ent(^d, key).write_ref == ent(*d, key).write_ref
          && ent(^d, key).eviction_handle == ent(*d, key).eviction_handle
          && ent(^d, key).reuse_count == ent(*d, key).reuse_count)]
#[ensures(rest_same(*d, ^d))]
#[ensures(wf(*d) ==> wf_entries(^d))]
#[ensures(wf(*d) ==> wf_pool(^d))]
#[ensures(wf(*d) ==> wf_fwd(^d))]
#[ensures(wf(*d) ==> wf_back(^d))]
pub fn dm_set_checksum(d: &mut Dm, key: u64, checksum: u32) -> Result<(), DmError> {
    let entry = match d.entries.get_mut(&key) {
        Some(e) => e,
        None => return Err(DmError::KeyNotFound(key)),
    };
    entry.checksum = checksum;
    Ok(())
}

/// `get_checksum` (lib.rs:605-615): `.map(|e| e.checksum).filter(|&c| c != 0)`.
#[ensures(!has(*d, key) ==> result == None)]
#[ensures(has(*d, key) && ent(*d, key).checksum@ == 0 ==> result == None)]
#[ensures(has(*d, key) && ent(*d, key).checksum@ != 0 ==> result == Some(ent(*d, key).checksum))]
#[ensures(^d == *d)]
pub fn dm_get_checksum(d: &mut Dm, key: u64) -> Option<u32> {
    match d.entries.get(&key) {
        None => None,
        Some(e) => {
            if e.checksum != 0 {
                Some(e.checksum)
            } else {
                None
            }
        }
    }
}

// `u32::div_ceil` has no creusot-std spec; this states the std documentation's meaning.
extern_spec! {
    impl u32 {
        #[requires(rhs@ > 0)]
        #[ensures(result@ == (self@ + rhs@ - 1) / rhs@)]
        fn div_ceil(self, rhs: u32) -> u32;
    }
}
