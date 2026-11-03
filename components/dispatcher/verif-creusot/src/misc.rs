// misc.rs — promote_to_memory_tier (lib.rs:3636), clear_memory_tier (lib.rs:3861),
// read_write_stats (lib.rs:3866), tier_event_stats (lib.rs:3878), DispatcherConfig::default,
// ServedBy::is_hit.

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Cold {
    pub key: u64,
    pub off: u64,
    pub size: u32,
}

/// The reference counts and existence of every entry are unchanged; locations may move between
/// block-device and memory-tier state only.
#[logic(open)]
pub fn refs_kept(a: W, b: W) -> bool {
    pearlite! { forall<k: u64> (has(a, k) == has(b, k)) && (has(a, k) ==> ent(b, k).rr == ent(a, k).rr && ent(b, k).wr == ent(a, k).wr) }
}

/// The SSD-read / promote step for one cold entry (shared by both promote_to_memory_tier paths).
/// `read` = true: the entry's data is read from drive `d` first (hardware path).
#[ensures((^s).p == s.p && refs_kept(*s.g, *(^s).g) && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g))]
#[ensures(no_dma(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq && mono_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(forall<k: u64> has(*s.g, k) && is_blk(ent(*s.g, k).loc) && is_mem(ent(*(^s).g, k).loc) ==> k == e.key)]
#[ensures(result ==> has(*(^s).g, e.key) && is_mem(ent(*(^s).g, e.key).loc) && (read ==> logged(*s.g, *(^s).g, Ev::SsdRead { drive: d, key: e.key })))]
#[ensures(!read ==> no_ssd(*s.g, *(^s).g))]
#[ensures(has(*s.g, e.key) && is_blk(ent(*s.g, e.key).loc) && !mt_has(*s.g, e.key) && (*s.g).used + e.size@ <= (*s.g).cap
    && all_from((*s.g).ob, (*s.g).tick) && (read ==> all_from((*s.g).os, (*s.g).tick)) ==>
    (e.size@ > 0 ==> result) && (e.size@ == 0 ==> !result && mt_has(*(^s).g, e.key) && is_blk(ent(*(^s).g, e.key).loc)))]
pub fn promote_one(s: &mut S, e: Cold, read: bool, d: usize) -> bool {
    let max_attempts = s.p.max_att;
    match evict_for_space(s, e.size, max_attempts) {
        Ok(()) => {}
        Err(_) => return false,
    }
    let g_ins = snapshot!(*s.g);
    let mem_ptr = match mt_insert(s, e.key, e.size) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let rd_at = snapshot!(s.g.log.len());
    if read {
        if !ssd_read_dram(s, d, e.key) {
            let _ = mt_remove(s, e.key);
            return false;
        }
    }
    let g1 = snapshot!(*s.g);
    proof_assert!(mt_has(*g1, e.key) && g1.dm == g_ins.dm);
    match dm_promote(s, e.key, mem_ptr, e.size) {
        Ok(()) => {
            rec_promotion_to_memory(s);
            proof_assert!(read ==> s.g.log[*rd_at] == g1.log[*rd_at]);
            true
        }
        Err(_) => false,
    }
}

/// promote_to_memory_tier's classification pass (lib.rs:3657-3672).
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).tick == (*s.g).tick)]
#[ensures(forall<j: u64> (*(^s).g).dm.get(j) == (*s.g).dm.get(j))]
#[ensures((*(^s).g).mt == (*s.g).mt && (*(^s).g).used == (*s.g).used && (*(^s).g).wq == (*s.g).wq && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(no_dma(*s.g, *(^s).g) && no_ssd(*s.g, *(^s).g))]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> has(*s.g, result@[i].key) && is_blk(ent(*s.g, result@[i].key).loc)
    && result@[i].size == ent(*s.g, result@[i].key).es && ent(*s.g, result@[i].key).loc == Loc::Blk { off: result@[i].off })]
#[ensures(keys@.len() == 1 && has(*s.g, keys@[0]) && is_blk(ent(*s.g, keys@[0]).loc) && ent(*s.g, keys@[0]).wr@ == 0
    && ent(*s.g, keys@[0]).rr@ < u32::MAX@ ==> result@.len() == 1 && result@[0].key == keys@[0])]
#[ensures(forall<i: Int> 0 <= i && i < keys@.len() && has(*s.g, keys@[i]) && is_mem(ent(*s.g, keys@[i]).loc) && ent(*s.g, keys@[i]).wr@ == 0
    && ent(*s.g, keys@[i]).rr@ < u32::MAX@ ==> logged(*s.g, *(^s).g, Ev::DmTouch { key: keys@[i] }) && logged(*s.g, *(^s).g, Ev::MtTouch { key: keys@[i] }))]
pub fn classify(s: &mut S, keys: &[u64]) -> Vec<Cold> {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut cold: Vec<Cold> = Vec::new();
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= keys@.len() && log_ext(*g0, *s.g) && same_env(*g0, *s.g) && s.g.tick == g0.tick)]
    #[invariant(forall<j: u64> s.g.dm.get(j) == g0.dm.get(j))]
    #[invariant(s.g.mt == g0.mt && s.g.used == g0.used && s.g.wq == g0.wq && same_ctr(g0.c, s.g.c) && no_dma(*g0, *s.g) && no_ssd(*g0, *s.g))]
    #[invariant(forall<q: Int> 0 <= q && q < cold@.len() ==> has(*g0, cold@[q].key) && is_blk(ent(*g0, cold@[q].key).loc)
        && cold@[q].size == ent(*g0, cold@[q].key).es && ent(*g0, cold@[q].key).loc == Loc::Blk { off: cold@[q].off })]
    #[invariant(keys@.len() == 1 && i@ == 1 && has(*g0, keys@[0]) && is_blk(ent(*g0, keys@[0]).loc) && ent(*g0, keys@[0]).wr@ == 0
        && ent(*g0, keys@[0]).rr@ < u32::MAX@ ==> cold@.len() == 1 && cold@[0].key == keys@[0])]
    #[invariant(keys@.len() == 1 && i@ == 0 ==> cold@.len() == 0)]
    #[invariant(forall<q: Int> 0 <= q && q < i@ && has(*g0, keys@[q]) && is_mem(ent(*g0, keys@[q]).loc) && ent(*g0, keys@[q]).wr@ == 0
        && ent(*g0, keys@[q]).rr@ < u32::MAX@ ==> logged(*g0, *s.g, Ev::DmTouch { key: keys@[q] }) && logged(*g0, *s.g, Ev::MtTouch { key: keys@[q] }))]
    while i < keys.len() {
        let key = keys[i];
        let gl = snapshot!(*s.g);
        match dm_lookup(s, key) {
            Ok(Lr::Blk { off }) => {
                let _ = dm_release_read(s, key, snapshot!(1));
                match dm_entry_size(s, key) {
                    Ok(size) => cold.push(Cold { key, off, size }),
                    Err(_) => {}
                }
            }
            Ok(Lr::Mem { .. }) => {
                let _ = dm_release_read(s, key, snapshot!(1));
                let t_at = snapshot!(s.g.log.len());
                let _ = dm_touch(s, key);
                mt_touch(s, key);
                proof_assert!(s.g.log[*t_at] == Ev::DmTouch { key } && s.g.log[*t_at + 1] == Ev::MtTouch { key });
            }
            _ => {}
        }
        proof_assert!(forall<j: u64> s.g.dm.get(j) == gl.dm.get(j));
        i += 1;
    }
    cold
}

/// promote_to_memory_tier's promotion pass for the classified cold entries (lib.rs:3674-3791).
/// The per-drive `std::thread::scope` workers are run one after another (drive order); their
/// interleaving is not modelled.
#[ensures((^s).p == s.p && refs_kept(*s.g, *(^s).g) && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g))]
#[ensures(no_dma(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq && mono_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(s.p.nd@ == 0 ==> no_ssd(*s.g, *(^s).g))]
#[ensures(forall<k: u64> has(*s.g, k) && is_blk(ent(*s.g, k).loc) && is_mem(ent(*(^s).g, k).loc) ==> exists<i: Int> 0 <= i && i < cold@.len() && cold@[i].key == k)]
#[ensures(cold@.len() == 1 && has(*s.g, cold@[0].key) && is_blk(ent(*s.g, cold@[0].key).loc) && !mt_has(*s.g, cold@[0].key)
    && (*s.g).used + cold@[0].size@ <= (*s.g).cap && all_from((*s.g).ob, (*s.g).tick) && all_from((*s.g).os, (*s.g).tick) ==>
    (cold@[0].size@ > 0 ==> is_mem(ent(*(^s).g, cold@[0].key).loc))
    && (cold@[0].size@ == 0 ==> mt_has(*(^s).g, cold@[0].key) && is_blk(ent(*(^s).g, cold@[0].key).loc)))]
pub fn promote_cold(s: &mut S, cold: &Vec<Cold>) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    if s.p.nd == 0 {
        let mut i: usize = 0;
        #[invariant(s.p == *p0 && i@ <= cold@.len() && refs_kept(*g0, *s.g) && log_ext(*g0, *s.g) && same_env(*g0, *s.g))]
        #[invariant(no_dma(*g0, *s.g) && no_ssd(*g0, *s.g) && s.g.wq == g0.wq && mono_ctr(g0.c, s.g.c))]
        #[invariant(forall<k: u64> has(*g0, k) && is_blk(ent(*g0, k).loc) && is_mem(ent(*s.g, k).loc) ==> exists<q: Int> 0 <= q && q < i@ && cold@[q].key == k)]
        #[invariant(i@ == 0 ==> *s.g == *g0)]
        #[invariant(cold@.len() == 1 && i@ == 1 && has(*g0, cold@[0].key) && is_blk(ent(*g0, cold@[0].key).loc) && !mt_has(*g0, cold@[0].key)
            && g0.used + cold@[0].size@ <= g0.cap && all_from(g0.ob, g0.tick) && all_from(g0.os, g0.tick) ==>
            (cold@[0].size@ > 0 ==> is_mem(ent(*s.g, cold@[0].key).loc))
            && (cold@[0].size@ == 0 ==> mt_has(*s.g, cold@[0].key) && is_blk(ent(*s.g, cold@[0].key).loc)))]
        while i < cold.len() {
            let gi = snapshot!(*s.g);
            let _ = promote_one(s, cold[i], false, 0);
            proof_assert!(forall<k: u64> has(*gi, k) && is_blk(ent(*gi, k).loc) && is_mem(ent(*s.g, k).loc) ==> k == cold@[i@].key);
            i += 1;
        }
        return;
    }
    let n = s.p.nd;
    proof_assert!(cold@.len() >= 1 ==> di_lt(cold@[0].key, n) == ());
    let mut d: usize = 0;
    #[invariant(s.p == *p0 && d@ <= n@ && refs_kept(*g0, *s.g) && log_ext(*g0, *s.g) && same_env(*g0, *s.g))]
    #[invariant(no_dma(*g0, *s.g) && s.g.wq == g0.wq && mono_ctr(g0.c, s.g.c))]
    #[invariant(forall<k: u64> has(*g0, k) && is_blk(ent(*g0, k).loc) && is_mem(ent(*s.g, k).loc) ==> exists<q: Int> 0 <= q && q < cold@.len() && cold@[q].key == k)]
    #[invariant(cold@.len() == 1 && d@ <= di(cold@[0].key, n)@ ==> *s.g == W { tick: s.g.tick, ..*g0 } && s.g.tick >= g0.tick)]
    #[invariant(cold@.len() == 1 && d@ > di(cold@[0].key, n)@ && has(*g0, cold@[0].key) && is_blk(ent(*g0, cold@[0].key).loc) && !mt_has(*g0, cold@[0].key)
        && g0.used + cold@[0].size@ <= g0.cap && all_from(g0.ob, g0.tick) && all_from(g0.os, g0.tick) ==>
        (cold@[0].size@ > 0 ==> is_mem(ent(*s.g, cold@[0].key).loc))
        && (cold@[0].size@ == 0 ==> mt_has(*s.g, cold@[0].key) && is_blk(ent(*s.g, cold@[0].key).loc)))]
    while d < n {
        let gd = snapshot!(*s.g);
        // Exclusive channel for this drive's prefetch worker; a failed checkout skips the drive.
        let ok = chan_checkout(s, d);
        proof_assert!(cold@.len() == 1 && d@ <= di(cold@[0].key, n)@ && all_from(g0.ob, g0.tick) ==> ok);
        if ok {
            let mut i: usize = 0;
            let gc = snapshot!(*s.g);
            proof_assert!(cold@.len() == 1 && d@ <= di(cold@[0].key, n)@ ==> *gc == W { tick: gc.tick, ..*g0 } && gc.tick >= g0.tick);
            proof_assert!(all_from(g0.ob, g0.tick) && gc.tick >= g0.tick && gc.ob == g0.ob ==> all_from(gc.ob, gc.tick));
            proof_assert!(all_from(g0.os, g0.tick) && gc.tick >= g0.tick && gc.os == g0.os ==> all_from(gc.os, gc.tick));
            #[invariant(s.p == *p0 && i@ <= cold@.len() && refs_kept(*g0, *s.g) && log_ext(*g0, *s.g) && same_env(*g0, *s.g))]
            #[invariant(no_dma(*g0, *s.g) && s.g.wq == g0.wq && mono_ctr(g0.c, s.g.c))]
            #[invariant(forall<k: u64> has(*g0, k) && is_blk(ent(*g0, k).loc) && is_mem(ent(*s.g, k).loc) ==> exists<q: Int> 0 <= q && q < cold@.len() && cold@[q].key == k)]
            #[invariant(cold@.len() == 1 && i@ == 0 ==> *s.g == *gc)]
            #[invariant(cold@.len() == 1 && i@ == 1 && d@ != di(cold@[0].key, n)@ ==> *s.g == *gc)]
            #[invariant(cold@.len() == 1 && i@ == 1 && d@ == di(cold@[0].key, n)@ && has(*gc, cold@[0].key) && is_blk(ent(*gc, cold@[0].key).loc) && !mt_has(*gc, cold@[0].key)
                && gc.used + cold@[0].size@ <= gc.cap && all_from(gc.ob, gc.tick) && all_from(gc.os, gc.tick) ==>
                (cold@[0].size@ > 0 ==> is_mem(ent(*s.g, cold@[0].key).loc))
                && (cold@[0].size@ == 0 ==> mt_has(*s.g, cold@[0].key) && is_blk(ent(*s.g, cold@[0].key).loc)))]
            while i < cold.len() {
                let e = cold[i];
                if drive_index(e.key, n) == d {
                    let gi = snapshot!(*s.g);
                    let _ = promote_one(s, e, true, d);
                    proof_assert!(forall<k: u64> has(*gi, k) && is_blk(ent(*gi, k).loc) && is_mem(ent(*s.g, k).loc) ==> k == e.key);
                }
                i += 1;
            }
        }
        d += 1;
    }
}

/// `promote_to_memory_tier` (lib.rs:3636).
#[ensures((^s).p == s.p)]
#[ensures(!(s.p.init && s.p.has_dm && s.p.has_mt) ==> *(^s).g == *s.g)]
#[ensures(refs_kept(*s.g, *(^s).g) && log_ext(*s.g, *(^s).g) && no_dma(*s.g, *(^s).g) && mono_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).wq == (*s.g).wq)]
#[ensures(forall<k: u64> !has(*s.g, k) ==> !has(*(^s).g, k))]
#[ensures(s.p.nd@ == 0 ==> no_ssd(*s.g, *(^s).g))]
#[ensures(s.p.init && s.p.has_dm && s.p.has_mt && keys@.len() == 1 && has(*s.g, keys@[0]) && is_blk(ent(*s.g, keys@[0]).loc)
    && ent(*s.g, keys@[0]).wr@ == 0 && ent(*s.g, keys@[0]).rr@ < u32::MAX@ && !mt_has(*s.g, keys@[0])
    && (*s.g).used + ent(*s.g, keys@[0]).es@ <= (*s.g).cap && all_from((*s.g).ob, (*s.g).tick) && all_from((*s.g).os, (*s.g).tick) ==>
    (ent(*s.g, keys@[0]).es@ > 0 ==> is_mem(ent(*(^s).g, keys@[0]).loc))
    && (ent(*s.g, keys@[0]).es@ == 0 ==> mt_has(*(^s).g, keys@[0]) && is_blk(ent(*(^s).g, keys@[0]).loc)))]
#[ensures(s.p.init && s.p.has_dm && s.p.has_mt ==> forall<i: Int> 0 <= i && i < keys@.len() && has(*s.g, keys@[i]) && is_mem(ent(*s.g, keys@[i]).loc)
    && ent(*s.g, keys@[i]).wr@ == 0 && ent(*s.g, keys@[i]).rr@ < u32::MAX@ ==> logged(*s.g, *(^s).g, Ev::DmTouch { key: keys@[i] }))]
pub fn promote_to_memory_tier(s: &mut S, keys: &[u64]) {
    if !s.p.init {
        return;
    }
    if !s.p.has_dm {
        return;
    }
    if !s.p.has_mt {
        return;
    }
    let cold = classify(s, keys);
    if cold.len() == 0 {
        return;
    }
    promote_cold(s, &cold);
}

// ---------------------------------------------------------------------------
// clear_memory_tier (lib.rs:3861)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && s.p.has_dm && s.p.has_mt ==> match result { Ok(_) => (*(^s).g).mt.len() == 0, Err(DE::Timeout) => true, Err(_) => false })]
#[ensures(forall<k: u64> has(*s.g, k) && (ent(*s.g, k).rr@ > 0 || ent(*s.g, k).wr@ > 0) ==> (*(^s).g).dm.get(k) == (*s.g).dm.get(k))]
#[ensures(forall<k: u64> has(*s.g, k) && (ent(*s.g, k).rr@ > 0 || ent(*s.g, k).wr@ > 0) && mt_has(*s.g, k) ==> mt_has(*(^s).g, k))]
#[ensures(log_ext(*s.g, *(^s).g) && mono_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).wq == (*s.g).wq && same_env(*s.g, *(^s).g))]
pub fn clear_memory_tier(s: &mut S) -> Result<usize, DE> {
    match ensure_initialized(s) {
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    if !s.p.has_dm {
        return Err(DE::NotInit(Ni::DispatchMap));
    }
    if !s.p.has_mt {
        return Err(DE::NotInit(Ni::MemoryTier));
    }
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut count: usize = 0;
    let mut pinned: usize = 0;
    #[invariant(s.p == *p0 && log_ext(*g0, *s.g) && mono_ctr(g0.c, s.g.c) && s.g.wq == g0.wq && same_env(*g0, *s.g))]
    #[invariant(forall<k: u64> has(*g0, k) && (ent(*g0, k).rr@ > 0 || ent(*g0, k).wr@ > 0) ==> s.g.dm.get(k) == g0.dm.get(k))]
    #[invariant(forall<k: u64> has(*g0, k) && (ent(*g0, k).rr@ > 0 || ent(*g0, k).wr@ > 0) && mt_has(*g0, k) ==> mt_has(*s.g, k))]
    loop {
        let candidates = mt_oldest(s, padd(64, pinned));
        if candidates.len() == 0 {
            break;
        }
        let mut cleared: usize = 0;
        pinned = 0;
        let mut i: usize = 0;
        #[invariant(s.p == *p0 && i@ <= candidates@.len() && log_ext(*g0, *s.g) && mono_ctr(g0.c, s.g.c) && s.g.wq == g0.wq && same_env(*g0, *s.g))]
        #[invariant(forall<k: u64> has(*g0, k) && (ent(*g0, k).rr@ > 0 || ent(*g0, k).wr@ > 0) ==> s.g.dm.get(k) == g0.dm.get(k))]
        #[invariant(forall<k: u64> has(*g0, k) && (ent(*g0, k).rr@ > 0 || ent(*g0, k).wr@ > 0) && mt_has(*g0, k) ==> mt_has(*s.g, k))]
        while i < candidates.len() {
            let key = candidates[i];
            let done = match dm_try_evict(s, key) {
                Ok(()) => true,
                Err(DmE::ActiveRefs) => false,
                // Entry not evictable (no SSD copy) — force remove; a pin refuses it.
                Err(_) => match dm_remove(s, key) {
                    Err(DmE::ActiveRefs) => false,
                    _ => true,
                },
            };
            if done {
                let _ = mt_remove(s, key);
                count = padd(count, 1);
                cleared = padd(cleared, 1);
            } else {
                pinned = padd(pinned, 1);
            }
            i += 1;
        }
        if cleared == 0 {
            if timed_out_5s(s) {
                return Err(DE::Timeout);
            }
            // std::thread::sleep(50 us)
        }
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// read_write_stats (lib.rs:3866)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p && *(^s).g == *s.g)]
#[ensures(result@ == rwsum((*s.g).rw, s.p.nd@))]
pub fn read_write_stats(s: &mut S) -> u64 {
    let mut agg: u64 = 0;
    let mut i: usize = 0;
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    #[invariant(s.p == *p0 && *s.g == *g0 && i@ <= s.p.nd@ && agg@ == rwsum(g0.rw, i@))]
    while i < s.p.nd {
        let v = bd_rw_stats(s, i);
        agg = padd64(agg, v);
        i += 1;
    }
    agg
}

// ---------------------------------------------------------------------------
// tier_event_stats (lib.rs:3878)
// ---------------------------------------------------------------------------

pub struct Stats {
    pub c: Ctr,
    pub oldest_sampled: Int,
    pub oldest_persisted: Int,
}

/// `is_evictable` count over keys `ks[0..i]` in state `w`.
#[logic]
#[variant(i)]
pub fn n_evictable(w: W, ks: Seq<u64>, i: Int) -> Int {
    pearlite! { if i <= 0 { 0 } else { n_evictable(w, ks, i - 1) + if evictable(w, ks[i - 1]) { 1 } else { 0 } } }
}

#[logic]
#[requires(0 <= i)]
#[ensures(0 <= n_evictable(w, ks, i) && n_evictable(w, ks, i) <= i)]
#[variant(i)]
pub fn n_evictable_le(w: W, ks: Seq<u64>, i: Int) {
    pearlite! { if i <= 0 { () } else { n_evictable_le(w, ks, i - 1) } }
}

#[ensures((^s).p == s.p && *(^s).g == W { tick: (*(^s).g).tick, ..*s.g })]
#[ensures(same_ctr(result.c, (*s.g).c))]
#[ensures(result.oldest_persisted <= result.oldest_sampled && 0 <= result.oldest_persisted)]
#[ensures(s.p.has_dm && s.p.has_mt ==> result.oldest_sampled == (*s.g).ok[(*s.g).tick].len()
    && result.oldest_persisted == n_evictable(*s.g, (*s.g).ok[(*s.g).tick], result.oldest_sampled))]
#[ensures(!(s.p.has_dm && s.p.has_mt) ==> result.oldest_sampled == 0 && result.oldest_persisted == 0)]
pub fn tier_event_stats(s: &mut S) -> Snapshot<Stats> {
    let g0 = snapshot!(*s.g);
    // TierEventCounters::snapshot: the cumulative counters, gauges zero.
    let mut sampled: u64 = 0;
    let mut persisted: u64 = 0;
    if s.p.has_dm && s.p.has_mt {
        let keys = mt_oldest(s, 64);
        sampled = keys.len() as u64;
        let mut i: usize = 0;
        let g1 = snapshot!(*s.g);
        let p1 = snapshot!(s.p);
        proof_assert!(n_evictable_le(*g0, keys@, 0) == ());
        #[invariant(s.p == *p1 && *s.g == *g1 && i@ <= keys@.len() && persisted@ == n_evictable(*g0, keys@, i@) && persisted@ <= i@)]
        while i < keys.len() {
            proof_assert!(n_evictable_le(*g0, keys@, i@ + 1) == ());
            if dm_is_evictable(s, keys[i]) {
                persisted += 1;
            }
            i += 1;
        }
    }
    snapshot!(Stats { c: g0.c, oldest_sampled: sampled@, oldest_persisted: persisted@ })
}

// ---------------------------------------------------------------------------
// DispatcherConfig::default (interfaces/src/idispatcher.rs:102) and ServedBy::is_hit
// ---------------------------------------------------------------------------

/// An `f64` config field, carried as its IEEE-754 bit pattern (Creusot cannot translate f64).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct F64Bits {
    pub bits: u64,
}

pub struct Cfg {
    pub max_cache_entries: usize,
    pub format_on_init: bool,
    pub ssd_eviction_threshold: F64Bits,
    pub ssd_eviction_low_watermark: F64Bits,
    pub ssd_eviction_batch_size: usize,
    pub ssd_eviction_interval_secs: u64,
    pub poller_base_cpu: Option<usize>,
    pub max_eviction_attempts: usize,
    pub store_backpressure_ms: u64,
    pub backfill_delay_ms: u64,
    pub metadata_partition_size: u64,
    pub extended_metadata_partition_size: u64,
    pub memory_tier_eviction_threshold: F64Bits,
    pub memory_tier_eviction_low_watermark: F64Bits,
    pub memory_tier_eviction_batch_size: usize,
    pub memory_tier_eviction_interval_secs: u64,
    pub cold_staging_slots: usize,
    pub cold_staging_buf_bytes: usize,
}

/// IEEE-754 binary64 bit patterns of the default literals: 0.9, 0.8, 0.0, 0.70.
pub const F_0_9: u64 = 0x3FEC_CCCC_CCCC_CCCD;
pub const F_0_8: u64 = 0x3FE9_9999_9999_999A;
pub const F_0_0: u64 = 0x0000_0000_0000_0000;
pub const F_0_7: u64 = 0x3FE6_6666_6666_6666;

#[ensures(result.max_cache_entries@ == 10000 && result.format_on_init
    && result.ssd_eviction_threshold.bits@ == 0x3FEC_CCCC_CCCC_CCCD && result.ssd_eviction_low_watermark.bits@ == 0x3FE9_9999_9999_999A
    && result.ssd_eviction_batch_size@ == 64 && result.ssd_eviction_interval_secs@ == 5 && result.poller_base_cpu == None
    && result.max_eviction_attempts@ == 2048 && result.store_backpressure_ms@ == 5000 && result.backfill_delay_ms@ == 10
    && result.metadata_partition_size@ == 128 * 1024 * 1024 && result.extended_metadata_partition_size@ == 128 * 1024 * 1024
    && result.memory_tier_eviction_threshold.bits@ == 0 && result.memory_tier_eviction_low_watermark.bits@ == 0x3FE6_6666_6666_6666
    && result.memory_tier_eviction_batch_size@ == 64 && result.memory_tier_eviction_interval_secs@ == 2
    && result.cold_staging_slots@ == 64 && result.cold_staging_buf_bytes@ == 4 * 1024 * 1024)]
pub fn config_default() -> Cfg {
    Cfg {
        max_cache_entries: 10000,
        format_on_init: true,
        ssd_eviction_threshold: F64Bits { bits: F_0_9 },
        ssd_eviction_low_watermark: F64Bits { bits: F_0_8 },
        ssd_eviction_batch_size: 64,
        ssd_eviction_interval_secs: 5,
        poller_base_cpu: None,
        max_eviction_attempts: 2048,
        store_backpressure_ms: 5000,
        backfill_delay_ms: 10,
        metadata_partition_size: 128 * 1024 * 1024,
        extended_metadata_partition_size: 128 * 1024 * 1024,
        memory_tier_eviction_threshold: F64Bits { bits: F_0_0 },
        memory_tier_eviction_low_watermark: F64Bits { bits: F_0_7 },
        memory_tier_eviction_batch_size: 64,
        memory_tier_eviction_interval_secs: 2,
        cold_staging_slots: 64,
        cold_staging_buf_bytes: 4 * 1024 * 1024,
    }
}

/// `ServedBy::is_hit` (interfaces/src/idispatcher.rs).
#[check(terminates)]
#[ensures(result == match sb { ServedBy::Dram => true, ServedBy::Ssd => true, ServedBy::Remote => true, _ => false })]
pub fn served_by_is_hit(sb: ServedBy) -> bool {
    match sb {
        ServedBy::Dram | ServedBy::Ssd | ServedBy::Remote => true,
        ServedBy::Miss | ServedBy::SizeMismatch | ServedBy::Error => false,
    }
}
