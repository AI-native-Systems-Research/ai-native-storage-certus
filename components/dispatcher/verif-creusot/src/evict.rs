// evict.rs — the foreground eviction stack (lib.rs:1050-1224) and reserve_memory (lib.rs:3318).

// ---------------------------------------------------------------------------
// Counter bumps (TierEventCounters::record_*, lib.rs:146-246): fetch_add on a u64 atomic,
// modelled as an unbounded ghost count (wrap-around after 2^64 events is not modelled).
// ---------------------------------------------------------------------------

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { blk_pin: (*s.g).c.blk_pin + 1, ..(*s.g).c }, ..*s.g })]
pub fn rec_blocked_by_pin(s: &mut S) {
    s.g = snapshot!(W { c: Ctr { blk_pin: s.g.c.blk_pin + 1, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { blk_unp: (*s.g).c.blk_unp + 1, ..(*s.g).c }, ..*s.g })]
pub fn rec_blocked_unpersisted(s: &mut S) {
    s.g = snapshot!(W { c: Ctr { blk_unp: s.g.c.blk_unp + 1, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { exhausted: (*s.g).c.exhausted + 1, ..(*s.g).c }, ..*s.g })]
pub fn rec_scan_exhausted(s: &mut S) {
    s.g = snapshot!(W { c: Ctr { exhausted: s.g.c.exhausted + 1, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { ev_mem: (*s.g).c.ev_mem + 1, ..(*s.g).c }, ..*s.g })]
pub fn rec_eviction_from_memory(s: &mut S) {
    s.g = snapshot!(W { c: Ctr { ev_mem: s.g.c.ev_mem + 1, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { already: (*s.g).c.already + 1, ..(*s.g).c }, ..*s.g })]
pub fn rec_store_already_resident(s: &mut S) {
    s.g = snapshot!(W { c: Ctr { already: s.g.c.already + 1, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { bp: (*s.g).c.bp + 1, ..(*s.g).c }, ..*s.g })]
pub fn rec_store_backpressure(s: &mut S) {
    s.g = snapshot!(W { c: Ctr { bp: s.g.c.bp + 1, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { drop_full: (*s.g).c.drop_full + 1, ..(*s.g).c }, ..*s.g })]
pub fn rec_store_drop_on_full(s: &mut S) {
    s.g = snapshot!(W { c: Ctr { drop_full: s.g.c.drop_full + 1, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { promo_mem: (*s.g).c.promo_mem + 1, ..(*s.g).c }, ..*s.g })]
pub fn rec_promotion_to_memory(s: &mut S) {
    s.g = snapshot!(W { c: Ctr { promo_mem: s.g.c.promo_mem + 1, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { promo_gpu: (*s.g).c.promo_gpu + n@, ..(*s.g).c }, ..*s.g })]
pub fn rec_promotions_to_gpu(s: &mut S, n: u64) {
    s.g = snapshot!(W { c: Ctr { promo_gpu: s.g.c.promo_gpu + n@, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures(a@ * b@ <= usize::MAX@ && result@ == a@ * b@)]
#[trusted]
pub fn pmul(a: usize, b: usize) -> usize {
    a * b
}

/// Only the eviction-relevant counters moved, and none decreased.
#[logic(open)]
pub fn ev_ctr(a: Ctr, b: Ctr) -> bool {
    pearlite! { a.promo_mem == b.promo_mem && a.promo_gpu == b.promo_gpu && a.ev_ssd == b.ev_ssd
        && a.hit_dram == b.hit_dram && a.hit_ssd == b.hit_ssd && a.r_hit == b.r_hit && a.r_miss == b.r_miss
        && a.bp == b.bp && a.drop_full == b.drop_full && a.already == b.already
        && a.ev_mem <= b.ev_mem && a.blk_pin <= b.blk_pin && a.exhausted <= b.exhausted && a.blk_unp <= b.blk_unp }
}

// ---------------------------------------------------------------------------
// evict_one_clean (lib.rs:1050)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p)]
#[ensures(evict_frame(*s.g, *(^s).g) && ev_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(result ==> (*(^s).g).c.ev_mem == (*s.g).c.ev_mem + 1 && (*(^s).g).c.exhausted == (*s.g).c.exhausted)]
#[ensures(!result ==> (*(^s).g).c.ev_mem == (*s.g).c.ev_mem && (*(^s).g).c.exhausted == (*s.g).c.exhausted + 1)]
#[ensures(scan@ >= 1 && (*s.g).ok[(*s.g).tick].len() >= 1 && evictable(*s.g, cand0(*s.g)) ==> result
    && (*(^s).g).dm == (*s.g).dm.insert(cand0(*s.g), dem_ent(ent(*s.g, cand0(*s.g))))
    && (*(^s).g).mt == (*s.g).mt.remove(cand0(*s.g))
    && (*(^s).g).used == (if (*s.g).used >= (*s.g).mt.lookup(cand0(*s.g)).size@ { (*s.g).used - (*s.g).mt.lookup(cand0(*s.g)).size@ } else { 0 }))]
pub fn evict_one_clean(s: &mut S, scan: usize) -> bool {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let cands = mt_oldest(s, scan);
    let g1 = snapshot!(*s.g);
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= cands@.len())]
    #[invariant(i@ == 0 ==> *s.g == *g1)]
    #[invariant(scan@ >= 1 && g0.ok[g0.tick].len() >= 1 && evictable(*g0, cand0(*g0)) ==> i@ == 0)]
    #[invariant(evict_frame(*g0, *s.g) && ev_ctr(g0.c, (*s.g).c))]
    #[invariant((*s.g).c.ev_mem == g0.c.ev_mem && (*s.g).c.exhausted == g0.c.exhausted)]
    while i < cands.len() {
        let cand = cands[i];
        match dm_try_evict(s, cand) {
            Ok(()) => {
                let _ = mt_remove(s, cand);
                // emit_eviction: try_send on the bounded event channel (not modelled).
                rec_eviction_from_memory(s);
                return true;
            }
            Err(DmE::ActiveRefs) => {
                rec_blocked_by_pin(s);
            }
            Err(_) => {
                rec_blocked_unpersisted(s);
            }
        }
        i += 1;
    }
    rec_scan_exhausted(s);
    false
}

// ---------------------------------------------------------------------------
// evict_for_space (lib.rs:1123)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p)]
#[ensures(evict_frame(*s.g, *(^s).g) && ev_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(match result { Ok(()) => (*(^s).g).used + needed@ <= (*(^s).g).cap, Err(e) => e == DE::Alloc })]
#[ensures((*s.g).used + needed@ <= (*s.g).cap ==> result == Ok(()) && *(^s).g == *s.g)]
#[ensures(max_attempts@ == 0 && (*s.g).used + needed@ > (*s.g).cap ==> result == Err(DE::Alloc) && *(^s).g == *s.g)]
#[ensures(max_attempts@ >= 1 && (*s.g).used + needed@ > (*s.g).cap && (*s.g).ok[(*s.g).tick].len() >= 1
    && evictable(*s.g, cand0(*s.g)) && mt_has(*s.g, cand0(*s.g))
    && (*s.g).used >= (*s.g).mt.lookup(cand0(*s.g)).size@
    && (*s.g).used - (*s.g).mt.lookup(cand0(*s.g)).size@ + needed@ <= (*s.g).cap
    ==> result == Ok(()) && demoted_k(*s.g, *(^s).g, cand0(*s.g))
        && forall<k: u64> k != cand0(*s.g) ==> (*(^s).g).dm.get(k) == (*s.g).dm.get(k) && (*(^s).g).mt.get(k) == (*s.g).mt.get(k))]
pub fn evict_for_space(s: &mut S, needed: u32, max_attempts: usize) -> Result<(), DE> {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut attempts: usize = 0;
    #[invariant(s.p == *p0)]
    #[invariant(evict_frame(*g0, *s.g) && ev_ctr(g0.c, (*s.g).c))]
    #[invariant(attempts@ == 0 ==> *s.g == *g0)]
    #[invariant(g0.used + needed@ <= g0.cap ==> attempts@ == 0)]
    #[invariant(max_attempts@ == 0 ==> attempts@ == 0)]
    #[invariant(attempts@ >= 1 && max_attempts@ >= 1 && g0.used + needed@ > g0.cap && g0.ok[g0.tick].len() >= 1
        && evictable(*g0, cand0(*g0)) && mt_has(*g0, cand0(*g0))
        && g0.used >= g0.mt.lookup(cand0(*g0)).size@
        && g0.used - g0.mt.lookup(cand0(*g0)).size@ + needed@ <= g0.cap
        ==> (*s.g).used + needed@ <= (*s.g).cap && demoted_k(*g0, *s.g, cand0(*g0))
            && forall<k: u64> k != cand0(*g0) ==> (*s.g).dm.get(k) == g0.dm.get(k) && (*s.g).mt.get(k) == g0.mt.get(k))]
    loop {
        let u = mt_used(s);
        let c = mt_capacity(s);
        if padd(u, needed as usize) <= c {
            break;
        }
        attempts = padd(attempts, 1);
        if attempts > max_attempts {
            return Err(DE::Alloc);
        }
        let m = pmul(4, attempts);
        let scan = if m < 1024 { m } else { 1024 };
        if !evict_one_clean(s, scan) {
            return Err(DE::Alloc);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// evict_and_insert (lib.rs:1176)
// ---------------------------------------------------------------------------

/// `b` is `a` after pin-safe clean demotions plus a fresh slot `Slot{ptr,size}` for `key`.
#[logic(open)]
pub fn eai_frame(a: W, b: W, key: u64, ptr: u64, size: u32) -> bool {
    pearlite! { b.mt.get(key) == Some(Slot { ptr, size }) && b.used <= b.cap
        && b.log.len() > a.log.len() && b.log[b.log.len() - 1] == Ev::MtInsert { key }
        && (b.dm.get(key) == a.dm.get(key) || demoted_k(a, W { mt: b.mt.remove(key), ..b }, key))
        && (forall<k: u64> k != key ==> (b.dm.get(k) == a.dm.get(k) && b.mt.get(k) == a.mt.get(k)) || demoted_k(a, b, k))
        && b.cap == a.cap && b.wq == a.wq && b.pool == a.pool && b.cluster == a.cluster
        && b.threads == a.threads && log_ext(a, b) && demote_before_free(a, b) && no_dma(a, b) && no_reg(a, b) && no_ssd(a, b)
        && own_writes(a, b) && own_reads(a, b) && b.tick >= a.tick && b.ob == a.ob && b.ob2 == a.ob2 && b.og == a.og && b.os == a.os && b.ov == a.ov && b.ok == a.ok && b.ext == a.ext && b.rw == a.rw && b.cfree == a.cfree }
}

#[ensures((^s).p == s.p)]
#[ensures(match result {
    Ok(ptr) => eai_frame(*s.g, *(^s).g, key, ptr, size),
    Err(DE::Alloc) => evict_frame(*s.g, *(^s).g),
    Err(DE::AlreadyExists(k)) => k == key && evict_frame(*s.g, *(^s).g) && mt_has(*(^s).g, key),
    Err(_) => false,
})]
#[ensures(ev_ctr_a((*s.g).c, (*(^s).g).c))]
#[ensures(all_from((*s.g).ob, (*s.g).tick) && (*s.g).used + size@ <= (*s.g).cap && !mt_has(*s.g, key) ==>
    match result { Ok(_) => (*(^s).g).dm == (*s.g).dm, _ => false })]
#[ensures(max_attempts@ == 0 && (*s.g).used + size@ > (*s.g).cap ==> result == Err(DE::Alloc) && *(^s).g == *s.g)]
#[ensures(match result { Err(DE::AlreadyExists(_)) => (*(^s).g).c.already == (*s.g).c.already + 1, _ => (*(^s).g).c.already == (*s.g).c.already })]
pub fn evict_and_insert(s: &mut S, key: u64, size: u32, max_attempts: usize) -> Result<u64, DE> {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut attempts: usize = 0;
    #[invariant(s.p == *p0)]
    #[invariant(evict_frame(*g0, *s.g) && ev_ctr(g0.c, (*s.g).c))]
    #[invariant(attempts@ == 0 || attempts@ < max_attempts@)]
    #[invariant(attempts@ == 0 ==> *s.g == *g0)]
    #[invariant(all_from(g0.ob, g0.tick) && g0.used + size@ <= g0.cap && !mt_has(*g0, key) ==> attempts@ == 0)]
    #[invariant(max_attempts@ == 0 ==> attempts@ == 0)]
    loop {
        match evict_for_space(s, size, max_attempts - attempts) {
            Ok(()) => {}
            Err(e) => return Err(e),
        }
        proof_assert!(attempts@ == 0 && g0.used + size@ <= g0.cap ==> *s.g == *g0);
        proof_assert!(attempts@ == 0 && all_from(g0.ob, g0.tick) && g0.used + size@ <= g0.cap ==> o1(*s.g));
        let gi = snapshot!(*s.g);
        match mt_insert(s, key, size) {
            Ok(ptr) => {
                proof_assert!(s.g.dm == gi.dm);
                return Ok(ptr);
            }
            Err(MtE::PoolFull) => {
                attempts += 1;
                if attempts >= max_attempts {
                    return Err(DE::Alloc);
                }
                let scan = pmul(4, attempts);
                if !evict_one_clean(s, scan) {
                    return Err(DE::Alloc);
                }
            }
            Err(MtE::AlreadyExists) => {
                rec_store_already_resident(s);
                return Err(DE::AlreadyExists(key));
            }
            Err(_) => {
                return Err(DE::Alloc);
            }
        }
    }
}

/// As `ev_ctr` but the store-already-resident counter may also move up.
#[logic(open)]
pub fn ev_ctr_a(a: Ctr, b: Ctr) -> bool {
    pearlite! { a.promo_mem == b.promo_mem && a.promo_gpu == b.promo_gpu && a.ev_ssd == b.ev_ssd
        && a.hit_dram == b.hit_dram && a.hit_ssd == b.hit_ssd && a.r_hit == b.r_hit && a.r_miss == b.r_miss
        && a.bp == b.bp && a.drop_full == b.drop_full && a.already <= b.already
        && a.ev_mem <= b.ev_mem && a.blk_pin <= b.blk_pin && a.exhausted <= b.exhausted && a.blk_unp <= b.blk_unp }
}

// ---------------------------------------------------------------------------
// reserve_memory (lib.rs:3318)
// ---------------------------------------------------------------------------

/// A caller-supplied backpressure deadline (`Option<Instant>`): `past` = it had already passed
/// when the call began (time is monotone, so every later clock check sees it passed too).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Dl {
    pub past: bool,
}

/// `Instant::now() < deadline`.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == nx(*s.g))]
#[ensures(result == (!dl.past && o1(*s.g)))]
pub fn before_deadline(s: &mut S, dl: Dl) -> bool {
    unimplemented!()
}

/// The counters reserve_memory may move.
#[logic(open)]
pub fn rsv_ctr(a: Ctr, b: Ctr) -> bool {
    pearlite! { a.promo_mem == b.promo_mem && a.promo_gpu == b.promo_gpu && a.ev_ssd == b.ev_ssd
        && a.hit_dram == b.hit_dram && a.hit_ssd == b.hit_ssd && a.r_hit == b.r_hit && a.r_miss == b.r_miss
        && a.drop_full == b.drop_full && a.bp <= b.bp && a.already <= b.already
        && a.ev_mem <= b.ev_mem && a.blk_pin <= b.blk_pin && a.exhausted <= b.exhausted && a.blk_unp <= b.blk_unp }
}

/// Ghost outcome of reserve_memory: how many `evict_and_insert` attempts it made.
pub struct RsvG {
    pub attempts: Int,
}

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result.0 == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && size@ == 0 ==> result.0 == Err(DE::InvalidParam) && *(^s).g == *s.g)]
#[ensures(rsv_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(s.p.init && size@ > 0 && s.p.has_dm && s.p.has_mt ==> (match result.0 {
    Ok(ptr) => eai_frame(*s.g, *(^s).g, key, ptr, size),
    Err(DE::Alloc) => evict_frame(*s.g, *(^s).g),
    Err(DE::AlreadyExists(k)) => k == key && evict_frame(*s.g, *(^s).g) && mt_has(*(^s).g, key),
    Err(_) => false,
  }) && result.1.attempts >= 1
     && (*(^s).g).c.bp == (*s.g).c.bp + result.1.attempts - 1
     && ((deadline == None && s.p.bp_ms@ == 0) || (match deadline { Some(d) => d.past, None => false }) ==> result.1.attempts == 1))]
#[ensures(match result.0 { Ok(_) => true, Err(DE::Alloc) => true, Err(DE::AlreadyExists(_)) => true, _ => (*(^s).g).c.bp == (*s.g).c.bp })]
#[ensures(s.p.init && size@ > 0 && s.p.has_dm && s.p.has_mt && deadline == None && s.p.bp_ms@ == 0
    && all_from((*s.g).ob, (*s.g).tick) && (*s.g).used + size@ <= (*s.g).cap && !mt_has(*s.g, key) ==>
    match result.0 { Ok(_) => (*(^s).g).dm == (*s.g).dm, _ => false })]
#[ensures(s.p.init && size@ > 0 && s.p.has_dm && s.p.has_mt && deadline == None && s.p.bp_ms@ == 0
    && s.p.max_att@ == 0 && (*s.g).used + size@ > (*s.g).cap ==> result.0 == Err(DE::Alloc) && *(^s).g == *s.g)]
pub fn reserve_memory(s: &mut S, key: u64, size: u32, deadline: Option<Dl>) -> (Result<u64, DE>, Snapshot<RsvG>) {
    match ensure_initialized(s) {
        Err(e) => return (Err(e), snapshot!(RsvG { attempts: 0 })),
        Ok(()) => {}
    }
    if size == 0 {
        return (Err(DE::InvalidParam), snapshot!(RsvG { attempts: 0 }));
    }
    if !s.p.has_dm {
        return (Err(DE::NotInit(Ni::DispatchMap)), snapshot!(RsvG { attempts: 0 }));
    }
    if !s.p.has_mt {
        return (Err(DE::NotInit(Ni::MemoryTier)), snapshot!(RsvG { attempts: 0 }));
    }
    let max_attempts = s.p.max_att;
    let dl = match deadline {
        Some(inst) => inst,
        None => {
            if s.p.bp_ms == 0 {
                let r = evict_and_insert(s, key, size, max_attempts);
                return (r, snapshot!(RsvG { attempts: 1 }));
            }
            // Instant::now() + budget, budget > 0: not yet passed.
            Dl { past: false }
        }
    };
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut n: Snapshot<Int> = snapshot!(0);
    #[invariant(s.p == *p0 && *n >= 0 && (*n == 0 ==> *s.g == *g0))]
    #[invariant(*n > 0 ==> evict_frame(*g0, *s.g))]
    #[invariant(rsv_ctr(g0.c, (*s.g).c) && (*s.g).c.bp == g0.c.bp + *n)]
    #[invariant(dl.past ==> *n == 0)]
    loop {
        n = snapshot!(*n + 1);
        match evict_and_insert(s, key, size, max_attempts) {
            Ok(mem_ptr) => return (Ok(mem_ptr), snapshot!(RsvG { attempts: *n })),
            Err(DE::Alloc) => {
                if before_deadline(s, dl) {
                    rec_store_backpressure(s);
                    // std::thread::sleep(POLL)
                } else {
                    return (Err(DE::Alloc), snapshot!(RsvG { attempts: *n }));
                }
            }
            Err(e) => return (Err(e), snapshot!(RsvG { attempts: *n })),
        }
    }
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { r_hit: (*s.g).c.r_hit + hits@, r_miss: (*s.g).c.r_miss + misses@, ..(*s.g).c }, ..*s.g })]
pub fn rec_remote_lookup(s: &mut S, hits: u64, misses: u64) {
    s.g = snapshot!(W { c: Ctr { r_hit: s.g.c.r_hit + hits@, r_miss: s.g.c.r_miss + misses@, ..s.g.c }, ..*s.g });
}

#[check(terminates)]
#[ensures((^s).p == s.p && *(^s).g == W { c: Ctr { hit_dram: (*s.g).c.hit_dram + dram@, hit_ssd: (*s.g).c.hit_ssd + ssd@, ..(*s.g).c }, ..*s.g })]
pub fn rec_local_hits(s: &mut S, dram: u64, ssd: u64) {
    s.g = snapshot!(W { c: Ctr { hit_dram: s.g.c.hit_dram + dram@, hit_ssd: s.g.c.hit_ssd + ssd@, ..s.g.c }, ..*s.g });
}
