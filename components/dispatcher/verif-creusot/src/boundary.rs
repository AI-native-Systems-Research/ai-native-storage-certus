// boundary.rs — contract-carrying `#[trusted]` boundaries for every consumed receptacle.
// Each contract states what the CONSUMED interface guarantees (as implemented by the
// dispatch-map / memory-tier components at this pin); outcomes the dispatcher cannot control are
// oracle reads. These are promises, not proofs: see ../verif/creusot_advisory.yaml.

// ---------------------------------------------------------------------------
// IDispatchMap (dispatch-map/src/lib.rs at this pin)
// ---------------------------------------------------------------------------

/// `lookup`: waits for `write_ref == 0` (in a single-threaded run a held write ref never clears,
/// so it times out); absent -> NotExist (no ref); else takes a read ref and reports the location.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(Lr::NotExist) => !has(*s.g, k) && *(^s).g == *s.g,
    Ok(Lr::Mismatch) => false,
    Ok(r) => has(*s.g, k) && ent(*s.g, k).wr@ == 0 && ent(*s.g, k).rr@ < u32::MAX@
             && r == lr_of(ent(*s.g, k).loc)
             && ent_rr(*s.g, *(^s).g, k, 1) && only_dm(*s.g, *(^s).g),
    Err(DmE::Timeout) => has(*s.g, k) && ent(*s.g, k).wr@ > 0 && *(^s).g == *s.g,
    Err(DmE::Overflow) => has(*s.g, k) && ent(*s.g, k).wr@ == 0 && ent(*s.g, k).rr@ == u32::MAX@ && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn dm_lookup(s: &mut S, k: u64) -> Result<Lr, DmE> {
    unimplemented!()
}

/// `take_read`.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && ent(*s.g, k).wr@ == 0 && ent(*s.g, k).rr@ < u32::MAX@
              && ent_rr(*s.g, *(^s).g, k, 1) && only_dm(*s.g, *(^s).g),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == *s.g,
    Err(DmE::Timeout) => has(*s.g, k) && ent(*s.g, k).wr@ > 0 && *(^s).g == *s.g,
    Err(DmE::Overflow) => has(*s.g, k) && ent(*s.g, k).wr@ == 0 && ent(*s.g, k).rr@ == u32::MAX@ && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn dm_take_read(s: &mut S, k: u64) -> Result<(), DmE> {
    unimplemented!()
}

/// `release_read`. `own` is ghost: how many read refs the caller itself acquired on `k`.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && ent(*s.g, k).rr@ > 0
              && ent_rr(*s.g, *(^s).g, k, -1) && only_dm_log(*s.g, *(^s).g, Ev::RelRead { key: k, own: *own }),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == lg(*s.g, Ev::RelRead { key: k, own: *own }),
    Err(DmE::Underflow) => has(*s.g, k) && ent(*s.g, k).rr@ == 0 && *(^s).g == lg(*s.g, Ev::RelRead { key: k, own: *own }),
    Err(_) => false,
})]
pub fn dm_release_read(s: &mut S, k: u64, own: Snapshot<Int>) -> Result<(), DmE> {
    unimplemented!()
}

/// `release_write` (sets write_ref to 0). `own` is ghost: how many write refs the caller holds.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && ent(*s.g, k).wr@ > 0
              && *(^s).g == lg(W { dm: (*s.g).dm.insert(k, Ent { wr: 0u32, ..ent(*s.g, k) }), ..*s.g }, Ev::RelWrite { key: k, own: *own }),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == lg(*s.g, Ev::RelWrite { key: k, own: *own }),
    Err(DmE::Underflow) => has(*s.g, k) && ent(*s.g, k).wr@ == 0 && *(^s).g == lg(*s.g, Ev::RelWrite { key: k, own: *own }),
    Err(_) => false,
})]
pub fn dm_release_write(s: &mut S, k: u64, own: Snapshot<Int>) -> Result<(), DmE> {
    unimplemented!()
}

/// `downgrade_reference`: write ref -> read ref.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && ent(*s.g, k).wr@ > 0 && ent(*s.g, k).rr@ < u32::MAX@
              && has(*(^s).g, k) && ent(*(^s).g, k).loc == ent(*s.g, k).loc && ent(*(^s).g, k).es == ent(*s.g, k).es
              && ent(*(^s).g, k).wr@ == 0 && ent(*(^s).g, k).rr@ == ent(*s.g, k).rr@ + 1
              && others(*s.g, *(^s).g, k) && only_dm(*s.g, *(^s).g),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == *s.g,
    Err(DmE::NoWrite) => has(*s.g, k) && ent(*s.g, k).wr@ == 0 && *(^s).g == *s.g,
    Err(DmE::Overflow) => has(*s.g, k) && ent(*s.g, k).wr@ > 0 && ent(*s.g, k).rr@ == u32::MAX@
              && *(^s).g == W { dm: (*s.g).dm.insert(k, Ent { wr: 0u32, ..ent(*s.g, k) }), ..*s.g },
    Err(_) => false,
})]
pub fn dm_downgrade(s: &mut S, k: u64) -> Result<(), DmE> {
    unimplemented!()
}

/// `remove`: refuses a referenced entry.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && ent(*s.g, k).rr@ == 0 && ent(*s.g, k).wr@ == 0
              && *(^s).g == lg(W { dm: (*s.g).dm.remove(k), ..*s.g }, Ev::DmRemove { key: k }),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == *s.g,
    Err(DmE::ActiveRefs) => has(*s.g, k) && (ent(*s.g, k).rr@ > 0 || ent(*s.g, k).wr@ > 0) && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn dm_remove(s: &mut S, k: u64) -> Result<(), DmE> {
    unimplemented!()
}

/// `touch`: refreshes the eviction timestamp only.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && *(^s).g == lg(*s.g, Ev::DmTouch { key: k }),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn dm_touch(s: &mut S, k: u64) -> Result<(), DmE> {
    unimplemented!()
}

/// `entry_size`.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g)]
#[ensures(match result {
    Ok(v) => has(*s.g, k) && v == ent(*s.g, k).es,
    Err(DmE::KeyNotFound) => !has(*s.g, k),
    Err(_) => false,
})]
pub fn dm_entry_size(s: &mut S, k: u64) -> Result<u32, DmE> {
    unimplemented!()
}

/// `create_memory_tier_entry`: a new memory-tier entry holding one write reference. Its
/// eviction-policy registration may fail (oracle) — the real component `unwrap`s it.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => size@ > 0 && !has(*s.g, k) && o1(*s.g)
              && *(^s).g == lg(nx(W { dm: (*s.g).dm.insert(k, Ent { loc: Loc::Mem { ptr, size, ssd: None }, es: size, rr: 0u32, wr: 1u32 }), ..*s.g }), Ev::DmCreate { key: k }),
    Err(DmE::InvalidSize) => size@ == 0 && *(^s).g == nx(*s.g),
    Err(DmE::AlreadyExists) => size@ > 0 && has(*s.g, k) && *(^s).g == nx(*s.g),
    Err(DmE::Other) => size@ > 0 && !has(*s.g, k) && !o1(*s.g) && *(^s).g == nx(*s.g),
    Err(_) => false,
})]
pub fn dm_create(s: &mut S, k: u64, ptr: u64, size: u32) -> Result<(), DmE> {
    unimplemented!()
}

/// `promote_block_to_memory_tier`: in-place BlockDevice -> MemoryTier, refs kept, offset retained.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => size@ > 0 && has(*s.g, k) && is_blk(ent(*s.g, k).loc)
              && *(^s).g == lg(W { dm: (*s.g).dm.insert(k, Ent { loc: Loc::Mem { ptr, size, ssd: match ent(*s.g, k).loc { Loc::Blk { off } => Some(off), _ => None } }, ..ent(*s.g, k) }), ..*s.g }, Ev::DmPromote { key: k }),
    Err(DmE::InvalidSize) => size@ == 0 && *(^s).g == *s.g,
    Err(DmE::KeyNotFound) => size@ > 0 && !has(*s.g, k) && *(^s).g == *s.g,
    Err(DmE::InvalidState) => size@ > 0 && has(*s.g, k) && is_mem(ent(*s.g, k).loc) && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn dm_promote(s: &mut S, k: u64, ptr: u64, size: u32) -> Result<(), DmE> {
    unimplemented!()
}

/// `convert_to_storage`: records the SSD offset on a memory-tier entry and drops one read ref.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && is_mem(ent(*s.g, k).loc)
              && has(*(^s).g, k) && ent(*(^s).g, k).loc == (match ent(*s.g, k).loc { Loc::Mem { ptr, size, .. } => Loc::Mem { ptr, size, ssd: Some(off) }, l => l })
              && ent(*(^s).g, k).es == ent(*s.g, k).es && ent(*(^s).g, k).wr == ent(*s.g, k).wr
              && ent(*(^s).g, k).rr@ == (if ent(*s.g, k).rr@ > 0 { ent(*s.g, k).rr@ - 1 } else { 0 })
              && others(*s.g, *(^s).g, k) && only_dm(*s.g, *(^s).g),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == *s.g,
    Err(DmE::InvalidState) => has(*s.g, k) && is_blk(ent(*s.g, k).loc) && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn dm_convert_to_storage(s: &mut S, k: u64, off: u64) -> Result<(), DmE> {
    unimplemented!()
}

/// `try_evict_to_block`.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && ent(*s.g, k).rr@ == 0 && ent(*s.g, k).wr@ == 0 && persisted(ent(*s.g, k).loc)
              && *(^s).g == lg(W { dm: (*s.g).dm.insert(k, Ent { loc: demoted_loc(ent(*s.g, k).loc), ..ent(*s.g, k) }), ..*s.g }, Ev::DmDemote { key: k }),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == *s.g,
    Err(DmE::ActiveRefs) => has(*s.g, k) && (ent(*s.g, k).rr@ > 0 || ent(*s.g, k).wr@ > 0) && *(^s).g == *s.g,
    Err(DmE::InvalidState) => has(*s.g, k) && ent(*s.g, k).rr@ == 0 && ent(*s.g, k).wr@ == 0
              && !persisted(ent(*s.g, k).loc) && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn dm_try_evict(s: &mut S, k: u64) -> Result<(), DmE> {
    unimplemented!()
}

/// `convert_memory_tier_to_block` (dispatch-map lib.rs:441): flips a resident entry that
/// carries its write-through copy to its block-device state, keeping every reference count
/// (unlike `try_evict_to_block`, which refuses a referenced entry).
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => has(*s.g, k) && persisted(ent(*s.g, k).loc)
              && has(*(^s).g, k) && ent(*(^s).g, k).loc == demoted_loc(ent(*s.g, k).loc)
              && ent(*(^s).g, k).es == ent(*s.g, k).es && ent(*(^s).g, k).rr == ent(*s.g, k).rr
              && ent(*(^s).g, k).wr == ent(*s.g, k).wr
              && others(*s.g, *(^s).g, k) && only_dm(*s.g, *(^s).g),
    Err(DmE::KeyNotFound) => !has(*s.g, k) && *(^s).g == *s.g,
    Err(DmE::InvalidState) => has(*s.g, k) && !persisted(ent(*s.g, k).loc) && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn dm_convert_memory_tier_to_block(s: &mut S, k: u64) -> Result<(), DmE> {
    unimplemented!()
}

/// `WriteThroughPins::record_write` (background.rs:365): a HashSet insert / remove.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == W { wf: (*s.g).wf.set(k, !landed), ..*s.g })]
pub fn pins_record_write(s: &mut S, k: u64, landed: bool) {
    unimplemented!()
}

/// `WriteThroughPins::failed_writes(|key| mt.contains(key))` (background.rs:376): forgets the
/// failed keys no longer resident and counts the rest.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == W { wf: |k: u64| (*s.g).wf[k] && mt_has(*s.g, k), ..*s.g })]
#[ensures(result@ == 0 ==> forall<k: u64> mt_has(*s.g, k) ==> !(*s.g).wf[k])]
#[ensures(result@ > 0 ==> exists<k: u64> mt_has(*s.g, k) && (*s.g).wf[k])]
pub fn pins_failed_writes(s: &mut S) -> usize {
    unimplemented!()
}

/// `is_evictable`.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g)]
#[ensures(result == (has(*s.g, k) && ent(*s.g, k).rr@ == 0 && ent(*s.g, k).wr@ == 0 && persisted(ent(*s.g, k).loc)))]
pub fn dm_is_evictable(s: &mut S, k: u64) -> bool {
    unimplemented!()
}

/// `recover_extent`: inserts a BlockDevice entry for an absent key.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => !has(*s.g, k) && *(^s).g == lg(W { dm: (*s.g).dm.insert(k, Ent { loc: Loc::Blk { off }, es: size, rr: 0u32, wr: 0u32 }), ..*s.g }, Ev::Recover { key: k, off, size }),
    Err(DmE::AlreadyExists) => has(*s.g, k) && *(^s).g == lg(*s.g, Ev::Recover { key: k, off, size }),
    Err(_) => false,
})]
pub fn dm_recover(s: &mut S, k: u64, off: u64, size: u32) -> Result<(), DmE> {
    unimplemented!()
}

/// `oldest_keys` (eviction-policy order, left abstract: an oracle list of at most n keys).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(result@ == (*s.g).ok[(*s.g).tick] && result@.len() <= n@)]
pub fn dm_oldest(s: &mut S, n: usize) -> Vec<u64> {
    unimplemented!()
}

// ---------------------------------------------------------------------------
// IMemoryTier
// ---------------------------------------------------------------------------

/// `insert`: AlreadyExists for a resident key; otherwise a slot (never over-committing the
/// capacity) or PoolFull / another error, as the tier decides.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(ptr) => !mt_has(*s.g, k) && o1(*s.g) && (*s.g).used + size@ <= (*s.g).cap
               && *(^s).g == lg(nx(W { mt: (*s.g).mt.insert(k, Slot { ptr, size }), used: (*s.g).used + size@, ..*s.g }), Ev::MtInsert { key: k }),
    Err(MtE::AlreadyExists) => mt_has(*s.g, k) && *(^s).g == nx(*s.g),
    Err(MtE::PoolFull) => !mt_has(*s.g, k) && !o1(*s.g) && o2(*s.g) && *(^s).g == nx(*s.g),
    Err(MtE::Other) => !mt_has(*s.g, k) && !o1(*s.g) && !o2(*s.g) && *(^s).g == nx(*s.g),
    Err(_) => false,
})]
pub fn mt_insert(s: &mut S, k: u64, size: u32) -> Result<u64, MtE> {
    unimplemented!()
}

/// `get` (refreshes the slot's eviction-order position).
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Some((p, z)) => mt_has(*s.g, k) && (*s.g).mt.lookup(k) == Slot { ptr: p, size: z } && *(^s).g == lg(*s.g, Ev::MtGet { key: k }),
    None => !mt_has(*s.g, k) && *(^s).g == *s.g,
})]
pub fn mt_get(s: &mut S, k: u64) -> Option<(u64, u32)> {
    unimplemented!()
}

/// `peek` (no eviction-order refresh).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g)]
#[ensures(match result {
    Some((p, z)) => mt_has(*s.g, k) && (*s.g).mt.lookup(k) == Slot { ptr: p, size: z },
    None => !mt_has(*s.g, k),
})]
pub fn mt_peek(s: &mut S, k: u64) -> Option<(u64, u32)> {
    unimplemented!()
}

/// `remove`.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(()) => mt_has(*s.g, k)
              && *(^s).g == lg(W { mt: (*s.g).mt.remove(k), used: if (*s.g).used >= (*s.g).mt.lookup(k).size@ { (*s.g).used - (*s.g).mt.lookup(k).size@ } else { 0 }, ..*s.g }, Ev::MtRemove { key: k }),
    Err(MtE::KeyNotFound) => !mt_has(*s.g, k) && *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn mt_remove(s: &mut S, k: u64) -> Result<(), MtE> {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(*s.g, Ev::MtTouch { key: k }))]
pub fn mt_touch(s: &mut S, k: u64) {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(*s.g, Ev::MtBatchTouch))]
pub fn mt_batch_touch(s: &mut S, keys: &Vec<u64>) {
    unimplemented!()
}

/// `oldest_keys(n)`: up to n resident keys (order = the tier's eviction order, left abstract);
/// empty only when the tier is empty.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(result@ == (*s.g).ok[(*s.g).tick] && result@.len() <= n@)]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> mt_has(*s.g, result@[i]))]
#[ensures(n@ > 0 ==> (result@.len() == 0 ==> (*s.g).mt.len() == 0))]
pub fn mt_oldest(s: &mut S, n: usize) -> Vec<u64> {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g)]
#[ensures(result@ == (*s.g).used)]
pub fn mt_used(s: &mut S) -> usize {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g)]
#[ensures(result@ == (*s.g).cap)]
pub fn mt_capacity(s: &mut S) -> usize {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g)]
#[ensures(result == (*s.g).pool)]
pub fn mt_pool_info(s: &mut S) -> bool {
    unimplemented!()
}

// ---------------------------------------------------------------------------
// IGpuServices
// ---------------------------------------------------------------------------

/// `memcpy_h2d_async` (stream != 0) — queued; success is the GPU's decision.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::H2D { src, dst, len, stream }))]
#[ensures(result == o_gpu(*s.g))]
pub fn gpu_h2d_async(s: &mut S, src: u64, dst: u64, len: usize, stream: u64) -> bool {
    unimplemented!()
}

/// `dma_copy_to_device` (synchronous copy through a DmaBuffer wrapper; logged on stream 0).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::H2D { src, dst, len, stream: 0u64 }))]
#[ensures(result == o_gpu(*s.g))]
pub fn gpu_dma_to_device(s: &mut S, src: u64, dst: u64, len: usize) -> bool {
    unimplemented!()
}

/// `memcpy_d2h_async`.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::D2H { src, dst, len, stream }))]
#[ensures(result == o_gpu(*s.g))]
pub fn gpu_d2h_async(s: &mut S, src: u64, dst: u64, len: usize, stream: u64) -> bool {
    unimplemented!()
}

/// `stream_synchronize`.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::Sync { stream }))]
#[ensures(result == o_gpu(*s.g))]
pub fn gpu_sync(s: &mut S, stream: u64) -> bool {
    unimplemented!()
}

/// `create_stream`.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(match result { Some(v) => o1(*s.g) && v == (*s.g).ov[(*s.g).tick], None => !o1(*s.g) })]
pub fn gpu_create_stream(s: &mut S) -> Option<u64> {
    unimplemented!()
}

/// `set_batch_device(gpu, addr)` (lib.rs:508): the device owning `addr`, or -1.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(result@ >= -1)]
pub fn set_batch_device(s: &mut S, addr: u64) -> i32 {
    unimplemented!()
}

/// `device_streams_for(gpu, device)` (lib.rs:470): None for device < 0 or failed setup.
/// When it returns a set, its streams were created on `device` — in particular
/// `warm` is non-null and bound to that device (`W.sdev`).
pub struct Ds {
    pub warm: u64,
    pub store: u64,
    pub pipe0: u64,
    pub pipe1: u64,
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(device@ < 0 ==> result == None)]
#[ensures(match result { Some(ds) => device@ >= 0 && ds.warm@ != 0 && (*s.g).sdev[ds.warm] == device, None => true })]
pub fn device_streams_for(s: &mut S, device: i32) -> Option<Ds> {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(*s.g, Ev::Unregister))]
pub fn gpu_unregister(s: &mut S) {
    unimplemented!()
}

/// `register_host_memory` on the memory-tier pool: the attempt is always logged
/// (`Ev::Register`); a failure (the `o1` oracle answering false) is reported as a
/// warning — `Ev::RegWarn` follows the attempt (FR-034) — and is not fatal.
#[trusted]
#[ensures((^s).p == s.p && result == o1(*s.g))]
#[ensures(if o1(*s.g) { *(^s).g == lg(nx(*s.g), Ev::Register) } else { *(^s).g == lg(lg(nx(*s.g), Ev::Register), Ev::RegWarn) })]
pub fn gpu_register(s: &mut S) -> bool {
    unimplemented!()
}

// ---------------------------------------------------------------------------
// Block devices, extent managers, pipelines (hardware I/O)
// ---------------------------------------------------------------------------

/// A cold SSD read of one object (pipelined_multi_object_zero_copy / pipelined_ssd_to_gpu /
/// pipelined_ssd_to_dram_only / one job of scatter_gather): success is the hardware's decision.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::SsdRead { drive, key }))]
#[ensures(result == o_ssd(*s.g))]
pub fn ssd_read(s: &mut S, drive: usize, key: u64) -> bool {
    unimplemented!()
}

/// `write_buffer_to_ssd` (lib.rs:666): MDTS-segmented synchronous write.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::SsdWrite { drive, key }))]
#[ensures(result == o_ssd(*s.g))]
pub fn ssd_write(s: &mut S, drive: usize, key: u64) -> bool {
    unimplemented!()
}

/// `IExtentManager::reserve_extent` + its handle's offset; `publish` is folded into success.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(match result { Some(o) => o1(*s.g) && o == (*s.g).ov[(*s.g).tick], None => !o1(*s.g) })]
pub fn em_reserve(s: &mut S, drive: usize, key: u64) -> Option<u64> {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(*s.g, Ev::ExtFree { drive, off }))]
pub fn em_remove_extent(s: &mut S, drive: usize, off: u64) {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(*s.g, Ev::Checkpoint { drive }))]
pub fn em_checkpoint(s: &mut S, drive: usize) {
    unimplemented!()
}

/// `for_each_extent` of drive i: the recovered extents, in the order the manager visits them.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g)]
#[ensures(result@ == (*s.g).ext[drive@])]
pub fn em_extents(s: &mut S, drive: usize) -> Vec<Ext> {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(*s.g, Ev::DevSignal { drive }))]
pub fn bd_signal_stop(s: &mut S, drive: usize) {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(*s.g, Ev::DevShutdown { drive }))]
pub fn bd_shutdown(s: &mut S, drive: usize) {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(*s.g, Ev::DevDetach { drive }))]
pub fn bd_detach(s: &mut S, drive: usize) {
    unimplemented!()
}

// ---------------------------------------------------------------------------
// IRemoteLookup
// ---------------------------------------------------------------------------

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(W { cluster: true, ..nx(*s.g) }, Ev::Join))]
#[ensures(result == o1(*s.g))]
pub fn rl_join(s: &mut S) -> bool {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(W { cluster: false, ..nx(*s.g) }, Ev::Leave))]
#[ensures(result == o1(*s.g))]
pub fn rl_leave(s: &mut S) -> bool {
    unimplemented!()
}

/// One cold object read SSD -> host buffer -> GPU (`pipelined_multi_object_zero_copy` with one
/// job / `pipelined_ssd_to_gpu_zero_copy`): NVMe segments then one coalesced H2D of the object.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(result ==> *(^s).g == lg(lg(nx(*s.g), Ev::SsdRead { drive, key }), Ev::H2D { src: buf, dst, len, stream: 1u64 }))]
#[ensures(!result ==> *(^s).g == lg(nx(*s.g), Ev::SsdRead { drive, key }))]
#[ensures(result == o_ssd(*s.g))]
pub fn pipe_read_gpu(s: &mut S, drive: usize, key: u64, buf: u64, dst: u64, len: usize) -> bool {
    unimplemented!()
}

/// `ChannelPool::checkout` (lib.rs:310): reuse a pooled channel or connect a fresh one (which
/// can fail); the lease returns it on drop.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(result == o1(*s.g))]
pub fn chan_checkout(s: &mut S, drive: usize) -> bool {
    unimplemented!()
}

/// `StagingPool::checkout` (pipeline.rs:128): blocks until a buffer is free; returns its address.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(result == (*s.g).ov[(*s.g).tick])]
pub fn stg_checkout(s: &mut S) -> u64 {
    unimplemented!()
}

/// `pipelined_ssd_to_dram_only` (pipeline.rs:1166): NVMe reads into a host slot, no GPU work.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::SsdRead { drive, key }))]
#[ensures(result == o_ssd(*s.g))]
pub fn ssd_read_dram(s: &mut S, drive: usize, key: u64) -> bool {
    unimplemented!()
}

/// `IBlockDevice::read_write_stats` of drive i (one representative counter; every field of
/// `ReadWriteStats` is merged the same way).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g && result == (*s.g).rw[drive@])]
pub fn bd_rw_stats(s: &mut S, drive: usize) -> u64 {
    unimplemented!()
}

/// `self.rw_stats_baseline.read().clone()` (lib.rs): the dispatcher's baseline
/// read/write counter — the cumulative total of the drives already dropped
/// (one representative field, like `bd_rw_stats`; trusted to fit the u64).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g && result@ == (*s.g).rw_base)]
pub fn bd_rw_base(s: &mut S) -> u64 {
    unimplemented!()
}

/// One job of `scatter_gather_multi_drive_zero_copy` (pipeline.rs:771): its NVMe segments, and
/// for a single-region object (`dst != 0`) the one coalesced H2D of the whole slot.
#[trusted]
#[ensures((^s).p == s.p)]
#[ensures(result && dst@ != 0 ==> *(^s).g == lg(lg(nx(*s.g), Ev::SsdRead { drive, key }), Ev::H2D { src: buf, dst, len, stream: 1u64 }))]
#[ensures(!(result && dst@ != 0) ==> *(^s).g == lg(nx(*s.g), Ev::SsdRead { drive, key }))]
#[ensures(result == o_ssd(*s.g))]
pub fn sg_job(s: &mut S, drive: usize, key: u64, buf: u64, dst: u64, len: usize) -> bool {
    unimplemented!()
}

/// `IRemoteLookup::batch_lookup(&[(key, size)])`: one answer per forwarded key (the peers'
/// decision); on success the value is resident in the local memory tier (not modelled: the
/// dispatcher re-checks residency with a dispatch-map lookup, below).
#[trusted]
#[ensures((^s).p == s.p && result@.len() == fwd@.len())]
#[ensures(*(^s).g == W { log: (*(^s).g).log, tick: (*s.g).tick + 1, ..*s.g })]
#[ensures((*(^s).g).log.len() == (*s.g).log.len() + fwd@.len()
    && (forall<i: Int> 0 <= i && i < (*s.g).log.len() ==> (*(^s).g).log[i] == (*s.g).log[i])
    && forall<i: Int> 0 <= i && i < fwd@.len() ==> (*(^s).g).log[(*s.g).log.len() + i] == Ev::RemoteFwd { key: fwd@[i].0, size: fwd@[i].1 })]
pub fn rl_batch_lookup(s: &mut S, fwd: &Vec<(u64, u32)>) -> Vec<Rl> {
    unimplemented!()
}

/// The dispatch-map lookup inside `serve_concurrently_promoted` (lib.rs:1360), while a
/// concurrent promotion may be flipping the entry: its answer is the environment's (oracle).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
pub fn dm_lookup_racing(s: &mut S, k: u64) -> Result<Lr, DmE> {
    unimplemented!()
}

/// `start.elapsed() >= Duration::from_secs(5)` (oracle clock).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g) && result == o1(*s.g))]
pub fn timed_out_5s(s: &mut S) -> bool {
    unimplemented!()
}

// ---------------------------------------------------------------------------
// Drive creation (lib.rs:1469-1870): PCI parsing, block-device factory/SPDK component, extent
// manager, partition manager. Each step's success is the environment's (oracle `ob`).
// ---------------------------------------------------------------------------

/// `parse_pci_addr(&config.data_pci_addrs[i])` — whether the string has the
/// "domain:bus:device.function" hex shape (string content: not modelled, oracle).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g) && result == o1(*s.g))]
pub fn parse_pci(s: &mut S, i: usize) -> bool {
    unimplemented!()
}

/// `create_block_device` + extent-manager construction/wiring + binds (lib.rs:1629-1716).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g) && result == o1(*s.g))]
pub fn create_drive(s: &mut S, i: usize) -> bool {
    unimplemented!()
}

/// `DiskPartitionManager::initialize_or_format(format_on_init, ..)` (disk-partition-manager
/// lib.rs:45): Ok((table, formatted)). With format_on_init = false it still formats a disk
/// whose table is missing or corrupt (formatted = true).
pub struct PartOut {
    pub nparts: usize,
    pub formatted: bool,
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(match result {
    Some(po) => o1(*s.g) && po.nparts@ == (*s.g).ov[(*s.g).tick]@ && (fmt ==> po.formatted) && (!fmt ==> po.formatted == o2(*s.g)),
    None => !o1(*s.g),
})]
pub fn part_init(s: &mut S, i: usize, fmt: bool) -> Option<PartOut> {
    unimplemented!()
}

/// `iem.format(params)` / `iem.initialize()` (recover).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::EmFormat { drive: i }) && result == o1(*s.g))]
pub fn em_format(s: &mut S, i: usize) -> bool {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(nx(*s.g), Ev::EmInit { drive: i }) && result == o1(*s.g))]
pub fn em_initialize(s: &mut S, i: usize) -> bool {
    unimplemented!()
}

/// `ChannelPool::new(ibd)` (lib.rs:301): an empty free list.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == W { cfree: (*s.g).cfree.set(i@, 0), ..*s.g })]
pub fn chanpool_new(s: &mut S, i: usize) {
    unimplemented!()
}

/// `gpu.create_stream()` for the warm stream / `PipelineRing::new` (non-fatal on failure).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g) && result == o1(*s.g))]
pub fn ring_new(s: &mut S) -> bool {
    unimplemented!()
}

/// The evictor's `util_now < low_watermark` re-check (f64 utilization: oracle).
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g) && result == o1(*s.g))]
pub fn below_low_watermark(s: &mut S) -> bool {
    unimplemented!()
}

/// `ClientChannels::completion_rx.try_recv().is_ok()` on channel `ch` (one stale completion
/// is dequeued per Ok).
#[trusted]
#[ensures((^s).p == s.p && result == ((*s.g).cfree[ch@ + 1000000] > 0))]
#[ensures(result ==> *(^s).g == W { cfree: (*s.g).cfree.set(ch@ + 1000000, (*s.g).cfree[ch@ + 1000000] - 1), ..*s.g })]
#[ensures(!result ==> *(^s).g == *s.g)]
pub fn chan_try_recv(s: &mut S, ch: u64) -> bool {
    unimplemented!()
}

/// `block_dev.connect_client()`: a fresh channel (never pooled before), or an error.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(match result { Some(c) => o1(*s.g) && forall<i: Int> 0 <= i && i < free@.len() ==> free@[i] != c, None => !o1(*s.g) })]
pub fn connect_client(s: &mut S, free: &Vec<u64>) -> Option<u64> {
    unimplemented!()
}
