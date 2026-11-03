// bg.rs — one batch of the background memory-tier evictor (background.rs:471) and SSD evictor
// (background.rs:279), ChannelPool::checkout (lib.rs:310), StagingPool (pipeline.rs:77), and the
// queue-depth split of scatter_gather_multi_drive_zero_copy (pipeline.rs:853-875).

/// One demotion pass of `MemoryTierEvictor::evictor_loop` over `scan_size` oldest keys.
#[ensures((^s).p == s.p && evict_frame(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq && mono_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(forall<k: u64> has(*s.g, k) && !evictable(*s.g, k) ==> (*(^s).g).dm.get(k) == (*s.g).dm.get(k) && (*(^s).g).mt.get(k) == (*s.g).mt.get(k))]
pub fn mt_evictor_pass(s: &mut S, scan_size: usize) -> u32 {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let candidates = mt_oldest(s, scan_size);
    let mut demoted: u32 = 0;
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= candidates@.len() && evict_frame(*g0, *s.g) && s.g.wq == g0.wq && mono_ctr(g0.c, s.g.c))]
    while i < candidates.len() {
        let key = candidates[i];
        i += 1;
        match dm_try_evict(s, key) {
            Ok(()) => {}
            Err(_) => continue,
        }
        match mt_remove(s, key) {
            Ok(()) => {}
            Err(_) => continue,
        }
        // EvictionEvent { key, reason: Demoted } via try_send (not modelled)
        demoted = if demoted < u32::MAX { demoted + 1 } else { demoted };
        rec_eviction_from_memory(s);
        if below_low_watermark(s) {
            break;
        }
    }
    demoted
}

/// `BackgroundEvictor::get_evictable_offset` (background.rs:384).
#[ensures((^s).p == s.p && *(^s).g == W { dm: (*(^s).g).dm, log: (*(^s).g).log, ..*s.g } && log_ext(*s.g, *(^s).g))]
#[ensures(forall<j: u64> (*(^s).g).dm.get(j) == (*s.g).dm.get(j))]
#[ensures(match result { Some(off) => has(*s.g, k) && ent(*s.g, k).loc == Loc::Blk { off }, None => true })]
pub fn get_evictable_offset(s: &mut S, k: u64) -> Option<u64> {
    match dm_lookup(s, k) {
        Ok(Lr::Blk { off }) => {
            let _ = dm_release_read(s, k, snapshot!(1));
            Some(off)
        }
        Ok(Lr::Mem { .. }) => {
            let _ = dm_release_read(s, k, snapshot!(1));
            None
        }
        // NotExist (and Mismatch, which `lookup` never answers): the lookup
        // took no read reference — it only pins an entry it finds — so none
        // is released. Releasing would drop a reference another caller holds.
        Ok(_) => None,
        Err(_) => None,
    }
}

/// One eviction pass of `BackgroundEvictor::evictor_loop` over `batch_size` oldest keys.
#[requires(s.p.nd@ > 0)]
#[ensures((^s).p == s.p)]
#[ensures(forall<k: u64> has(*s.g, k) && is_mem(ent(*s.g, k).loc) ==> (*(^s).g).dm.get(k) == (*s.g).dm.get(k))]
#[ensures(forall<k: u64> has(*s.g, k) && (ent(*s.g, k).rr@ > 0 || ent(*s.g, k).wr@ > 0) ==> (*(^s).g).dm.get(k) == (*s.g).dm.get(k))]
#[ensures(forall<k: u64> has(*(^s).g, k) ==> has(*s.g, k))]
pub fn ssd_evictor_pass(s: &mut S, batch_size: usize) -> u32 {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let candidates = dm_oldest(s, batch_size);
    let mut evicted: u32 = 0;
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= candidates@.len())]
    #[invariant(forall<k: u64> has(*g0, k) && is_mem(ent(*g0, k).loc) ==> s.g.dm.get(k) == g0.dm.get(k))]
    #[invariant(forall<k: u64> has(*g0, k) && (ent(*g0, k).rr@ > 0 || ent(*g0, k).wr@ > 0) ==> s.g.dm.get(k) == g0.dm.get(k))]
    #[invariant(forall<k: u64> has(*s.g, k) ==> has(*g0, k))]
    #[invariant(forall<k: u64> has(*s.g, k) ==> s.g.dm.get(k) == g0.dm.get(k))]
    while i < candidates.len() {
        let key = candidates[i];
        i += 1;
        let offset = match get_evictable_offset(s, key) {
            Some(o) => o,
            None => continue,
        };
        // Remove from the memory tier (no-op if not present), then from the dispatch map.
        let _ = mt_remove(s, key);
        match dm_remove(s, key) {
            Ok(()) => {}
            Err(_) => continue,
        }
        let n = max1(s.p.nd);
        let drive_idx = drive_index(key, n);
        if drive_idx < s.p.nd {
            em_remove_extent(s, drive_idx, offset);
        }
        evicted = if evicted < u32::MAX { evicted + 1 } else { evicted };
        if below_low_watermark(s) {
            break;
        }
    }
    evicted
}

// ---------------------------------------------------------------------------
// ChannelPool::checkout (lib.rs:310)
// ---------------------------------------------------------------------------

/// Pending (undelivered) completions of channel `c`, kept in the ghost `cfree` map at an
/// offset that no drive index reaches.
#[logic(open)]
pub fn pending(w: W, c: u64) -> Int {
    pearlite! { w.cfree[c@ + 1000000] }
}

/// No channel is in the pool's free list twice (each pooled channel is idle and owned by the pool).
#[logic(open)]
pub fn nodup(v: Seq<u64>) -> bool {
    pearlite! { forall<i: Int, j: Int> 0 <= i && i < j && j < v.len() ==> v[i] != v[j] }
}

#[requires(nodup(free@))]
#[ensures((^s).p == s.p && nodup((^free)@))]
#[ensures(match result {
    Some(c) => (forall<i: Int> 0 <= i && i < (^free)@.len() ==> (^free)@[i] != c) && pending(*(^s).g, c) <= 0
               && (^free)@.len() <= free@.len(),
    None => (^free)@ == free@,
})]
pub fn chan_pool_checkout(s: &mut S, free: &mut Vec<u64>) -> Option<u64> {
    let reused = free.pop();
    let channels = match reused {
        Some(ch) => ch,
        None => match connect_client(s, free) {
            Some(c) => c,
            None => return None,
        },
    };
    // Discard any late completions left by a prior user.
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    #[invariant(s.p == *p0)]
    while chan_try_recv(s, channels) {}
    Some(channels)
}

/// `ChannelLease::drop` (lib.rs:343): the leased channel goes back to the pool.
#[requires(nodup(free@) && forall<i: Int> 0 <= i && i < free@.len() ==> free@[i] != c)]
#[ensures(nodup((^free)@) && (^free)@ == free@.push_back(c))]
pub fn chan_pool_return(free: &mut Vec<u64>, c: u64) {
    free.push(c);
}

// ---------------------------------------------------------------------------
// StagingPool (pipeline.rs:77-161)
// ---------------------------------------------------------------------------

pub struct Stg {
    pub slots: usize,
    pub free: Vec<usize>,
    /// leased buffers (ghost)
    pub leased: Snapshot<Int>,
}

#[logic(open)]
pub fn stg_inv(p: Stg) -> bool {
    pearlite! { *p.leased >= 0 && *p.leased + p.free@.len() == p.slots@
        && forall<i: Int> 0 <= i && i < p.free@.len() ==> p.free@[i]@ < p.slots@ }
}

/// `StagingPool::new(gpu, slots, buf_bytes)`: every buffer free.
#[ensures(stg_inv(result) && *result.leased == 0 && result.slots == slots)]
pub fn stg_new(slots: usize) -> Stg {
    let mut free: Vec<usize> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= slots@ && free@.len() == i@ && forall<j: Int> 0 <= j && j < i@ ==> free@[j]@ < slots@)]
    while i < slots {
        free.push(i);
        i += 1;
    }
    Stg { slots, free, leased: snapshot!(0) }
}

/// `checkout`: blocks (forever, in a single-threaded run) while no buffer is free.
#[requires(stg_inv(*p))]
#[ensures(stg_inv(^p) && *(^p).leased == *p.leased + 1 && *(^p).leased <= p.slots@ && result@ < p.slots@)]
pub fn stg_pool_checkout(p: &mut Stg) -> usize {
    let p0 = snapshot!(*p);
    #[invariant(*p == *p0)]
    while p.free.len() == 0 {}
    let idx = unwrap_or_panic(p.free.pop());
    p.leased = snapshot!(*p.leased + 1);
    idx
}


/// `StagingLease::drop`: the buffer goes back to the free list.
#[requires(stg_inv(*p) && *p.leased > 0 && idx@ < p.slots@)]
#[ensures(stg_inv(^p) && *(^p).leased == *p.leased - 1)]
pub fn stg_pool_release(p: &mut Stg, idx: usize) {
    p.free.push(idx);
    p.leased = snapshot!(*p.leased - 1);
}

// ---------------------------------------------------------------------------
// Queue-depth split (pipeline.rs:853-875)
// ---------------------------------------------------------------------------

/// `per_drive_qd` before the over-budget reduction, for active drives with `segs[d]` NVMe
/// segments each (`ds.work.len() == ds.total_segments`); `grand_total_segments > 0`.
#[requires(segs@.len() > 0 && forall<d: Int> 0 <= d && d < segs@.len() ==> segs@[d]@ >= 1)]
#[ensures(result@.len() == segs@.len())]
#[ensures(forall<d: Int> 0 <= d && d < segs@.len() ==>
    result@[d]@ == min_int(if max_qd@ * segs@[d]@ / qsum(segs@, segs@.len()) >= 1 { max_qd@ * segs@[d]@ / qsum(segs@, segs@.len()) } else { 1 }, segs@[d]@))]
#[ensures(segs@.len() == 1 && max_qd@ >= 1 ==> result@[0]@ == min_int(max_qd@, segs@[0]@))]
pub fn qd_shares(segs: &Vec<usize>, max_qd: usize) -> Vec<usize> {
    let mut total: usize = 0;
    let mut i: usize = 0;
    #[invariant(i@ <= segs@.len() && total@ == qsum(segs@, i@) && (i@ > 0 ==> total@ >= 1))]
    while i < segs.len() {
        total = padd(total, segs[i]);
        i += 1;
    }
    proof_assert!(segs@.len() == 1 ==> qsum(segs@, 1) == segs@[0]@ && div_mul(max_qd@, segs@[0]@) == ());
    let mut qd: Vec<usize> = Vec::new();
    let mut d: usize = 0;
    #[invariant(d@ <= segs@.len() && qd@.len() == d@ && total@ == qsum(segs@, segs@.len()) && total@ >= 1)]
    #[invariant(forall<j: Int> 0 <= j && j < d@ ==>
        qd@[j]@ == min_int(if max_qd@ * segs@[j]@ / total@ >= 1 { max_qd@ * segs@[j]@ / total@ } else { 1 }, segs@[j]@))]
    while d < segs.len() {
        // (max_queue_depth as u64 * ds.total_segments as u64 / grand_total_segments as u64) as usize
        let prod = pmul64(max_qd as u64, segs[d] as u64);
        let share = (prod / (total as u64)) as usize;
        let share = if share > 1 { share } else { 1 };
        let share = if share < segs[d] { share } else { segs[d] };
        qd.push(share);
        d += 1;
    }
    qd
}

#[trusted]
#[check(terminates)]
#[ensures(a@ * b@ <= u64::MAX@ && result@ == a@ * b@)]
pub fn pmul64(a: u64, b: u64) -> u64 {
    a * b
}

#[logic]
#[requires(b > 0 && a >= 0)]
#[ensures((a * b) / b == a)]
pub fn div_mul(a: Int, b: Int) {}

#[logic]
#[variant(n)]
pub fn qsum(s: Seq<usize>, n: Int) -> Int {
    pearlite! { if n <= 0 { 0 } else { qsum(s, n - 1) + s[n - 1]@ } }
}
