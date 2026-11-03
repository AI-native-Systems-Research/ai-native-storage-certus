// lookup.rs — serve_memory_tier_to_gpu (lib.rs:1262), promote_and_serve (lib.rs:750),
// serve_cold_staged (lib.rs:919), lookup_async (lib.rs:2994), lookup (lib.rs:2197).

#[logic(open)]
pub fn min_int(a: Int, b: Int) -> Int {
    pearlite! { if a <= b { a } else { b } }
}

/// Bytes region `l` receives when the slot of `size` bytes is scattered over `r`.
#[logic(open)]
pub fn chunk_of(r: Seq<Ipc>, l: Int, size: Int) -> Int {
    pearlite! { min_int(r[l].size@, if size - pre(r, l) > 0 { size - pre(r, l) } else { 0 }) }
}

/// The H2D events for regions `0..m` sit at `base..base+m`: region l reads `chunk_of` bytes
/// starting at `ptr + pre(r, l)`.
#[logic(open)]
pub fn scattered(w: W, base: Int, r: Seq<Ipc>, m: Int, ptr: u64, size: u32, stream: u64) -> bool {
    pearlite! { 0 <= base && base + m <= w.log.len() && forall<l: Int> 0 <= l && l < m ==>
        match w.log[base + l] { Ev::H2D { src, dst, len, stream: st } =>
            src@ == ptr@ + pre(r, l) && dst == r[l].addr && len@ == chunk_of(r, l, size@) && len@ > 0 && st == stream,
            _ => false } }
}

/// Every H2D logged between `a` and `b` reads inside the slot `[ptr, ptr+size)`.
#[logic(open)]
pub fn h2d_within(a: W, b: W, ptr: u64, size: u32) -> bool {
    pearlite! { forall<i: Int> a.log.len() <= i && i < b.log.len() ==>
        match b.log[i] { Ev::H2D { src, len, .. } => ptr@ <= src@ && src@ + len@ <= ptr@ + size@, _ => true } }
}

/// An H2D copy of `len` bytes from `src` to `dst` was logged between `a` and `b`.
#[logic(open)]
pub fn h2d_logged(a: W, b: W, src: u64, dst: u64, len: Int) -> bool {
    pearlite! { exists<i: Int> a.log.len() <= i && i < b.log.len() &&
        match b.log[i] { Ev::H2D { src: x, dst: y, len: z, .. } => x == src && y == dst && z@ == len, _ => false } }
}

#[logic(open)]
pub fn h2d_or_sync(e: Ev, st: u64) -> bool {
    pearlite! { match e { Ev::H2D { stream, .. } => stream == st, Ev::Sync { stream } => stream == st, _ => false } }
}

pub struct ServeG {
    pub base: Int,
    /// regions actually copied (the loop stops at the first region with nothing left to copy)
    pub m: Int,
    /// every GPU call (copies, and the sync when requested) succeeded
    pub gpu_ok: bool,
}

#[ensures((^s).p == s.p && *(^s).g == W { log: (*(^s).g).log, tick: (*(^s).g).tick, ..*s.g } && log_ext(*s.g, *(^s).g))]
#[ensures(result.1.base == (*s.g).log.len() && 0 <= result.1.m && result.1.m <= regions@.len())]
#[ensures(scattered(*(^s).g, result.1.base, regions@, result.1.m, pointer, size, warm_raw))]
#[ensures(result.1.m < regions@.len() && result.0 == Ok(()) ==> chunk_of(regions@, result.1.m, size@) == 0)]
#[ensures(h2d_within(*s.g, *(^s).g, pointer, size) && own_reads(*s.g, *(^s).g) && own_writes(*s.g, *(^s).g) && no_reg(*s.g, *(^s).g))]
#[ensures(result.0 == Ok(()) ==> result.1.gpu_ok)]
#[ensures(all_from((*s.g).og, (*s.g).tick) ==> result.0 == Ok(()))]
#[ensures(forall<i: Int> (*s.g).log.len() <= i && i < (*(^s).g).log.len() ==> h2d_or_sync((*(^s).g).log[i], warm_raw))]
#[ensures(result.0 != Ok(()) ==> match result.0 { Err(DE::Io(_)) => !result.1.gpu_ok, _ => false })]
#[ensures(result.0 == Ok(()) && synchronize && warm_raw@ != 0 ==>
    (*(^s).g).log[(*(^s).g).log.len() - 1] == Ev::Sync { stream: warm_raw } && (*(^s).g).log.len() == result.1.base + result.1.m + 1)]
#[ensures(result.0 == Ok(()) && !(synchronize && warm_raw@ != 0) ==> (*(^s).g).log.len() == result.1.base + result.1.m)]
pub fn serve_memory_tier_to_gpu(s: &mut S, pointer: u64, size: u32, regions: &[Ipc], warm_raw: u64, synchronize: bool)
    -> (Result<(), DE>, Snapshot<ServeG>) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let base = snapshot!(s.g.log.len());
    let mut off: usize = 0;
    let mut i: usize = 0;
    proof_assert!(pre_zero(regions@) == ());
    #[invariant(s.p == *p0 && i@ <= regions@.len() && off@ == pre(regions@, i@))]
    #[invariant(*s.g == W { log: s.g.log, tick: s.g.tick, ..*g0 } && log_ext(*g0, *s.g))]
    #[invariant(s.g.log.len() == *base + i@ && scattered(*s.g, *base, regions@, i@, pointer, size, warm_raw))]
    #[invariant(i@ > 0 ==> pre(regions@, i@ - 1) < size@)]
    #[invariant(s.g.tick >= g0.tick && s.g.og == g0.og)]
    #[invariant(forall<q: Int> g0.log.len() <= q && q < s.g.log.len() ==> h2d_or_sync(s.g.log[q], warm_raw))]
    #[invariant(h2d_within(*g0, *s.g, pointer, size) && own_reads(*g0, *s.g) && own_writes(*g0, *s.g) && no_reg(*g0, *s.g))]
    while i < regions.len() {
        let region = regions[i];
        proof_assert!(pre_step(regions@, i@) == ());
        let rest = if (size as usize) > off { size as usize - off } else { 0 };
        let copy_size = if (region.size as usize) < rest { region.size as usize } else { rest };
        if copy_size == 0 {
            break;
        }
        let src = padd64(pointer, off as u64);
        let ok = if warm_raw != 0 {
            gpu_h2d_async(s, src, region.addr, copy_size, warm_raw)
        } else {
            // DmaBuffer::from_raw(slot + off, aligned, noop_free) + dma_copy_to_device
            gpu_dma_to_device(s, src, region.addr, copy_size)
        };
        if !ok {
            return (Err(DE::Io(IoK::Gpu)), snapshot!(ServeG { base: *base, m: i@, gpu_ok: false }));
        }
        off = padd(off, region.size as usize);
        i += 1;
    }
    let m = snapshot!(i@);
    if warm_raw != 0 && synchronize {
        if !gpu_sync(s, warm_raw) {
            return (Err(DE::Io(IoK::Sync)), snapshot!(ServeG { base: *base, m: *m, gpu_ok: false }));
        }
    }
    (Ok(()), snapshot!(ServeG { base: *base, m: *m, gpu_ok: true }))
}

// ---------------------------------------------------------------------------
// promote_and_serve (lib.rs:750)
// ---------------------------------------------------------------------------

/// The counters the read path may move (none decreases; only the listed ones change).
#[logic(open)]
pub fn rd_ctr(a: Ctr, b: Ctr) -> bool {
    pearlite! { a.ev_ssd == b.ev_ssd && a.hit_dram == b.hit_dram && a.hit_ssd == b.hit_ssd
        && a.r_hit == b.r_hit && a.r_miss == b.r_miss && a.bp == b.bp && a.drop_full == b.drop_full
        && a.promo_mem <= b.promo_mem && a.promo_gpu <= b.promo_gpu && a.already <= b.already
        && a.ev_mem <= b.ev_mem && a.blk_pin <= b.blk_pin && a.exhausted <= b.exhausted && a.blk_unp <= b.blk_unp }
}

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Path {
    /// evict_and_insert failed and no staging fallback applied
    NoSlot,
    /// memory-tier-only mode (no data drives)
    NoDrives,
    /// read into a fresh memory-tier slot
    Slot,
    /// served through a staging buffer (tier saturated)
    Staged,
}

pub struct PsG {
    pub path: Path,
    pub ptr: u64,
    /// the SSD read (and its GPU delivery) succeeded
    pub read_ok: bool,
    /// the in-place dispatch-map promotion succeeded
    pub promoted: bool,
    /// state right after the slot was obtained
    pub mid: W,
}

#[ensures((^s).p == s.p)]
#[ensures(rd_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu)]
#[ensures(log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq && own_reads(*s.g, *(^s).g))]
#[ensures(match result.1.path {
    Path::NoDrives => logged(result.1.mid, *(^s).g, Ev::RelWrite { key, own: if result.1.promoted { 1 } else { 0 } }),
    _ => own_writes(*s.g, *(^s).g) })]
#[ensures((*(^s).g).c.promo_mem == (*s.g).c.promo_mem + (if result.1.promoted { 1 } else { 0 }))]
#[ensures(match result.1.path {
    Path::NoSlot => (match result.0 { Err(DE::Alloc) => true, Err(DE::AlreadyExists(k)) => k == key, _ => false })
                    && evict_frame(*s.g, *(^s).g) && !result.1.promoted,
    Path::Staged => s.p.nd@ > 0 && s.p.ring && s.p.stg && h.size@ <= s.p.stg_bytes@ && evict_frame(*s.g, result.1.mid)
                    && !result.1.promoted && (*(^s).g).dm == result.1.mid.dm && (*(^s).g).mt == result.1.mid.mt
                    && (result.0 == Ok(()) ==> result.1.read_ok),
    Path::NoDrives => s.p.nd@ == 0 && eai_frame(*s.g, result.1.mid, key, result.1.ptr, h.size),
    Path::Slot => s.p.nd@ > 0 && eai_frame(*s.g, result.1.mid, key, result.1.ptr, h.size)
                  && (result.0 == Ok(()) ==> result.1.read_ok && result.1.promoted)
                  && (result.1.promoted ==> has(*(^s).g, key) && ent(*(^s).g, key).loc == Loc::Mem { ptr: result.1.ptr, size: h.size,
                        ssd: match ent(result.1.mid, key).loc { Loc::Blk { off } => Some(off), _ => None } })
                  && (!result.1.promoted ==> (*(^s).g).dm == result.1.mid.dm && (*(^s).g).mt == result.1.mid.mt),
})]
#[ensures(s.p.nd@ > 0 && result.0 == Ok(()) && match result.1.path { Path::Slot => true, Path::Staged => true, _ => false } ==>
    logged(result.1.mid, *(^s).g, Ev::SsdRead { drive: di(key, s.p.nd), key }))]
#[ensures(result.1.read_ok && s.p.nd@ > 0 ==> h2d_logged(result.1.mid, *(^s).g, result.1.ptr, h.addr, h.size@))]
#[ensures(match result.1.path { Path::NoDrives => (result.1.promoted ==> !has(result.1.mid, key))
        && (!result.1.promoted && has(result.1.mid, key) && ent(result.1.mid, key).wr@ == 0 ==> (*(^s).g).dm.get(key) == result.1.mid.dm.get(key))
        && no_ssd(*s.g, *(^s).g) && (all_from((*s.g).og, (*s.g).tick) ==> result.0 == Ok(())), _ => true })]
#[ensures(all_from((*s.g).ob, (*s.g).tick) && (*s.g).used + h.size@ <= (*s.g).cap && !mt_has(*s.g, key) ==>
    match result.1.path { Path::NoDrives => s.p.nd@ == 0, Path::Slot => s.p.nd@ > 0 && (*s.g).dm == result.1.mid.dm, _ => false })]
#[ensures(match result.1.path { Path::NoSlot => result.0 == Err(DE::Alloc) ==> !(s.p.nd@ > 0 && s.p.ring && s.p.stg && h.size@ <= s.p.stg_bytes@), _ => true })]
#[ensures(match result.1.path { Path::NoDrives => (result.0 == Ok(())) == result.1.read_ok && (!result.1.read_ok ==> result.0 == Err(DE::Io(IoK::Gpu))), _ => true })]
#[ensures(match result.1.path { Path::Slot => s.p.ring && all_from((*s.g).ob, (*s.g).tick) && (forall<t: Int> !(*s.g).os[t]) ==>
        !result.1.read_ok && match result.0 { Err(_) => true, _ => false }, _ => true })]
pub fn promote_and_serve(s: &mut S, key: u64, offset: u64, h: Ipc, device: i32) -> (Result<(), DE>, Snapshot<PsG>) {
    let g0 = snapshot!(*s.g);
    let total_bytes = h.size as usize;
    let max_attempts = s.p.max_att;
    let has_drives = s.p.nd > 0;
    let (mem_ptr, staged) = match evict_and_insert(s, key, h.size, max_attempts) {
        Ok(p) => (p, false),
        Err(DE::Alloc) if has_drives => {
            if s.p.ring && s.p.stg && total_bytes <= s.p.stg_bytes {
                let lease = stg_checkout(s);
                (lease, true)
            } else {
                return (Err(DE::Alloc), snapshot!(PsG { path: Path::NoSlot, ptr: 0u64, read_ok: false, promoted: false, mid: *s.g }));
            }
        }
        Err(e) => return (Err(e), snapshot!(PsG { path: Path::NoSlot, ptr: 0u64, read_ok: false, promoted: false, mid: *s.g })),
    };
    let mid = snapshot!(*s.g);
    if s.p.nd == 0 {
        // No hardware: copy the slot to the GPU, then register the promoted entry.
        let result = gpu_dma_to_device(s, mem_ptr, h.addr, total_bytes);
        let created = dm_create(s, key, mem_ptr, h.size);
        let mut promoted = false;
        match created {
            Ok(()) => {
                rec_promotion_to_memory(s);
                promoted = true;
            }
            Err(_) => {}
        }
        let own = snapshot!(if promoted { 1 } else { 0 });
        let rw_at = snapshot!(s.g.log.len());
        let _ = dm_release_write(s, key, own);
        proof_assert!(s.g.log[*rw_at] == Ev::RelWrite { key, own: *own });
        let path = Path::NoDrives;
        if result {
            return (Ok(()), snapshot!(PsG { path, ptr: mem_ptr, read_ok: true, promoted, mid: *mid }));
        } else {
            return (Err(DE::Io(IoK::Gpu)), snapshot!(PsG { path, ptr: mem_ptr, read_ok: false, promoted, mid: *mid }));
        }
    }
    let path = if staged { Path::Staged } else { Path::Slot };
    let idx = drive_index(key, s.p.nd);
    if !chan_checkout(s, idx) {
        return (Err(DE::Io(IoK::Channel)), snapshot!(PsG { path, ptr: mem_ptr, read_ok: false, promoted: false, mid: *mid }));
    }
    let _chunk_size = if s.p.ring { s.p.chunk } else { 131072 };
    match device_streams_for(s, device) {
        Some(_) => {}
        None => {
            if !s.p.ring {
                return (Err(DE::NotInit(Ni::Ring)), snapshot!(PsG { path, ptr: mem_ptr, read_ok: false, promoted: false, mid: *mid }));
            }
        }
    }
    let read_at = snapshot!(s.g.log.len());
    if !pipe_read_gpu(s, idx, key, mem_ptr, h.addr, total_bytes) {
        return (Err(DE::Io(IoK::Ssd)), snapshot!(PsG { path, ptr: mem_ptr, read_ok: false, promoted: false, mid: *mid }));
    }
    let after_read = snapshot!(*s.g);
    proof_assert!(s.g.log[*read_at] == Ev::SsdRead { drive: idx, key });
    proof_assert!(s.g.log[*read_at + 1] == Ev::H2D { src: mem_ptr, dst: h.addr, len: total_bytes, stream: 1u64 });
    if staged {
        // drop(staging_lease): leave the entry as BlockDevice, do NOT promote.
        return (Ok(()), snapshot!(PsG { path, ptr: mem_ptr, read_ok: true, promoted: false, mid: *mid }));
    }
    match dm_promote(s, key, mem_ptr, h.size) {
        Ok(()) => {}
        Err(_) => return (Err(DE::Io(IoK::Promote)), snapshot!(PsG { path, ptr: mem_ptr, read_ok: true, promoted: false, mid: *mid })),
    }
    rec_promotion_to_memory(s);
    proof_assert!(s.g.log[*read_at] == after_read.log[*read_at]);
    proof_assert!(s.g.log[*read_at + 1] == after_read.log[*read_at + 1]);
    (Ok(()), snapshot!(PsG { path, ptr: mem_ptr, read_ok: true, promoted: true, mid: *mid }))
}

// ---------------------------------------------------------------------------
// lookup_async (lib.rs:2994) and lookup (lib.rs:2197)
// ---------------------------------------------------------------------------

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum LPath {
    Early,
    Miss,
    Timeout,
    Warm,
    Cold,
}

pub struct LaG {
    pub path: LPath,
    /// state right after the dispatch-map lookup
    pub after_lookup: W,
    /// warm path: serve record; cold path: promote_and_serve record
    pub warm: u64,
    pub sv: ServeG,
    pub ps: PsG,
}

#[requires(pinv(s.p))]
#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result.0 == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(rd_ctr((*s.g).c, (*(^s).g).c) && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq)]
#[ensures(match result.0 { Ok(st) => st@ == 0, _ => true })]
#[ensures(s.p.init && s.p.has_gpu ==> match result.1.path {
    LPath::Early => false,
    LPath::Miss => !has(*s.g, key) && result.0 == Err(DE::KeyNotFound(key)) && *(^s).g == W { tick: (*(^s).g).tick, ..*s.g },
    LPath::Timeout => has(*s.g, key) && (ent(*s.g, key).wr@ > 0 || ent(*s.g, key).rr@ == u32::MAX@)
                      && result.0 == Err(DE::KeyNotFound(key)) && *(^s).g == W { tick: (*(^s).g).tick, ..*s.g },
    LPath::Warm => has(*s.g, key) && is_mem(ent(*s.g, key).loc) && ent(*s.g, key).wr@ == 0 && ent(*s.g, key).rr@ < u32::MAX@
        && (match ent(*s.g, key).loc { Loc::Mem { ptr, size, .. } =>
              scattered(*(^s).g, result.1.sv.base, Seq::singleton(h), result.1.sv.m, ptr, size, result.1.warm)
              && h2d_within(*s.g, *(^s).g, ptr, size)
              && (min_int(h.size@, size@) > 0 && result.0 == Ok(0u64) ==> result.1.sv.m == 1)
              && (result.1.sv.m == 1 ==> h2d_logged(*s.g, *(^s).g, ptr, h.addr, min_int(h.size@, size@))),
            _ => false })
        && (result.0 == Ok(0u64)) == result.1.sv.gpu_ok
        && (match result.0 { Ok(_) => true, Err(DE::Io(_)) => true, _ => false })
        && (forall<j: u64> (*(^s).g).dm.get(j) == (*s.g).dm.get(j)) && (*(^s).g).mt == (*s.g).mt
        && logged(*s.g, *(^s).g, Ev::MtTouch { key }) && logged(*s.g, *(^s).g, Ev::RelRead { key, own: 1 })
        && (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu + (if result.1.sv.gpu_ok { 1 } else { 0 })
        && (*(^s).g).c.promo_mem == (*s.g).c.promo_mem
        && own_reads(*s.g, *(^s).g) && own_writes(*s.g, *(^s).g),
    LPath::Cold => has(*s.g, key) && is_blk(ent(*s.g, key).loc) && ent(*s.g, key).wr@ == 0 && ent(*s.g, key).rr@ < u32::MAX@
        && (match result.0 { Ok(_) => result.1.ps.read_ok || s.p.nd@ == 0, _ => true })
        && (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu + (if result.0 == Ok(0u64) { 1 } else { 0 })
        && (*(^s).g).c.promo_mem == (*s.g).c.promo_mem + (if result.1.ps.promoted { 1 } else { 0 }),
})]
#[ensures(s.p.init && s.p.has_gpu ==> match result.1.path { LPath::Cold => promote_post(*s.g, result.1.after_lookup, *(^s).g, s.p, key, h, result.0, result.1.ps), _ => true })]
pub fn lookup_async(s: &mut S, key: u64, h: Ipc) -> (Result<u64, DE>, Snapshot<LaG>) {
    let g0 = snapshot!(*s.g);
    let dsv = snapshot!(ServeG { base: 0, m: 0, gpu_ok: false });
    let dps = snapshot!(PsG { path: Path::NoSlot, ptr: 0u64, read_ok: false, promoted: false, mid: *g0 });
    match ensure_initialized(s) {
        Err(e) => return (Err(e), snapshot!(LaG { path: LPath::Early, after_lookup: *g0, warm: 0u64, sv: *dsv, ps: *dps })),
        Ok(()) => {}
    }
    if !s.p.has_dm {
        return (Err(DE::NotInit(Ni::DispatchMap)), snapshot!(LaG { path: LPath::Early, after_lookup: *g0, warm: 0u64, sv: *dsv, ps: *dps }));
    }
    if !s.p.has_mt {
        return (Err(DE::NotInit(Ni::MemoryTier)), snapshot!(LaG { path: LPath::Early, after_lookup: *g0, warm: 0u64, sv: *dsv, ps: *dps }));
    }
    let result = dm_lookup(s, key);
    let g1 = snapshot!(*s.g);
    if !s.p.has_gpu {
        return (Err(DE::NotInit(Ni::Gpu)), snapshot!(LaG { path: LPath::Early, after_lookup: *g1, warm: 0u64, sv: *dsv, ps: *dps }));
    }
    let null_stream: u64 = 0;
    let device = set_batch_device(s, h.addr);
    let warm_raw = match device_streams_for(s, device) {
        Some(d) => d.warm,
        None => 0,
    };
    match result {
        Ok(Lr::NotExist) => (Err(DE::KeyNotFound(key)), snapshot!(LaG { path: LPath::Miss, after_lookup: *g1, warm: warm_raw, sv: *dsv, ps: *dps })),
        Ok(Lr::Mismatch) => {
            let _ = dm_release_read(s, key, snapshot!(1));
            (Err(DE::InvalidParam), snapshot!(LaG { path: LPath::Early, after_lookup: *g1, warm: warm_raw, sv: *dsv, ps: *dps }))
        }
        Ok(Lr::Mem { ptr, size }) => {
            let regions = [h];
            proof_assert!(regions@ == Seq::singleton(h));
            proof_assert!(pre_zero(regions@) == ());
            let gs = snapshot!(*s.g);
            let (r, sv) = serve_memory_tier_to_gpu(s, ptr, size, &regions, warm_raw, true);
            let h2d_ev = snapshot!(s.g.log[sv.base]);
            proof_assert!(sv.m == 1 ==> match *h2d_ev { Ev::H2D { src, dst, len, .. } => src == ptr && dst == h.addr && len@ == min_int(h.size@, size@), _ => false });
            let rel_at = snapshot!(s.g.log.len());
            let _ = dm_release_read(s, key, snapshot!(1));
            proof_assert!(s.g.log[*rel_at] == Ev::RelRead { key, own: 1 });
            let touch_at = snapshot!(s.g.log.len());
            mt_touch(s, key);
            proof_assert!(s.g.log[*rel_at] == Ev::RelRead { key, own: 1 });
            proof_assert!(s.g.log[*touch_at] == Ev::MtTouch { key });
            proof_assert!(sv.m == 1 ==> s.g.log[sv.base] == *h2d_ev && g0.log.len() <= sv.base && sv.base < s.g.log.len());
            match r {
                Ok(()) => {
                    rec_promotions_to_gpu(s, 1);
                    (Ok(null_stream), snapshot!(LaG { path: LPath::Warm, after_lookup: *g1, warm: warm_raw, sv: *sv, ps: *dps }))
                }
                Err(e) => (Err(e), snapshot!(LaG { path: LPath::Warm, after_lookup: *g1, warm: warm_raw, sv: *sv, ps: *dps })),
            }
        }
        Ok(Lr::Blk { off }) => {
            let _ = dm_release_read(s, key, snapshot!(1));
            let g2 = snapshot!(*s.g);
            proof_assert!(forall<j: u64> g2.dm.get(j) == g0.dm.get(j));
            let (r, ps) = promote_and_serve(s, key, off, h, device);
            match r {
                Ok(()) => {
                    let b0 = snapshot!(*s.g);
                    proof_assert!(promote_post(*g0, *g2, *b0, s.p, key, h, Ok(0u64), *ps));
                    rec_promotions_to_gpu(s, 1);
                    proof_assert!(s.g.dm == b0.dm && s.g.mt == b0.mt && s.g.log == b0.log && s.g.tick == b0.tick);
                    (Ok(null_stream), snapshot!(LaG { path: LPath::Cold, after_lookup: *g2, warm: warm_raw, sv: *dsv, ps: *ps }))
                }
                Err(e) => (Err(e), snapshot!(LaG { path: LPath::Cold, after_lookup: *g2, warm: warm_raw, sv: *dsv, ps: *ps })),
            }
        }
        Err(_) => (Err(DE::KeyNotFound(key)), snapshot!(LaG { path: LPath::Timeout, after_lookup: *g1, warm: warm_raw, sv: *dsv, ps: *dps })),
    }
}

/// What promote_and_serve guarantees, restated from state `a` (= after the lookup released its
/// read ref) for lookup_async's cold path. `g` is the call's initial state.
#[logic(open)]
pub fn promote_post(g: W, a: W, b: W, p: P, key: u64, h: Ipc, r: Result<u64, DE>, ps: PsG) -> bool {
    pearlite! {
        (forall<j: u64> a.dm.get(j) == g.dm.get(j)) && a.mt == g.mt && log_ext(g, a) && own_reads(g, a) && own_writes(g, a)
        && match ps.path {
            Path::NoSlot => (match r { Err(DE::Alloc) => true, Err(DE::AlreadyExists(k)) => k == key, _ => false }) && evict_frame(a, b) && !ps.promoted,
            Path::Staged => p.nd@ > 0 && p.ring && p.stg && h.size@ <= p.stg_bytes@ && evict_frame(a, ps.mid)
                            && !ps.promoted && b.dm == ps.mid.dm && b.mt == ps.mid.mt && (r == Ok(0u64) ==> ps.read_ok),
            Path::NoDrives => p.nd@ == 0 && eai_frame(a, ps.mid, key, ps.ptr, h.size)
                              && logged(ps.mid, b, Ev::RelWrite { key, own: if ps.promoted { 1 } else { 0 } }),
            Path::Slot => p.nd@ > 0 && eai_frame(a, ps.mid, key, ps.ptr, h.size)
                          && (r == Ok(0u64) ==> ps.read_ok && ps.promoted)
                          && (ps.promoted ==> has(b, key) && ent(b, key).loc == Loc::Mem { ptr: ps.ptr, size: h.size,
                                ssd: match ent(ps.mid, key).loc { Loc::Blk { off } => Some(off), _ => None } })
                          && (!ps.promoted ==> b.dm == ps.mid.dm && b.mt == ps.mid.mt),
        }
        && (p.nd@ > 0 && r == Ok(0u64) && match ps.path { Path::Slot => true, Path::Staged => true, _ => false } ==>
              logged(ps.mid, b, Ev::SsdRead { drive: di(key, p.nd), key }))
        && (ps.read_ok && p.nd@ > 0 ==> h2d_logged(ps.mid, b, ps.ptr, h.addr, h.size@))
        && (match ps.path { Path::NoDrives => true, _ => own_writes(a, b) })
    }
}

/// `lookup` = `lookup_async` + synchronize the returned stream if it is non-null.
#[requires(pinv(s.p))]
#[ensures((^s).p == s.p)]
#[ensures(result.0 == match (*result.1).0 { Ok(_) => Ok(()), Err(e) => Err(e) })]
#[ensures(*(^s).g == (*result.1).2)]
#[ensures(match (*result.1).0 { Ok(st) => st@ == 0, _ => true })]
pub fn lookup(s: &mut S, key: u64, h: Ipc) -> (Result<(), DE>, Snapshot<(Result<u64, DE>, LaG, W)>) {
    let (r, g) = lookup_async(s, key, h);
    let after = snapshot!(*s.g);
    match r {
        Err(e) => (Err(e), snapshot!((r, *g, *after))),
        Ok(stream) => {
            if stream != 0 {
                if !s.p.has_gpu {
                    return (Err(DE::NotInit(Ni::Gpu)), snapshot!((r, *g, *after)));
                }
                if !gpu_sync(s, stream) {
                    return (Err(DE::Io(IoK::Sync)), snapshot!((r, *g, *after)));
                }
            }
            (Ok(()), snapshot!((r, *g, *after)))
        }
    }
}
