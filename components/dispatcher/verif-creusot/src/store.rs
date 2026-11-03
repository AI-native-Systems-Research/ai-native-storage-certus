// store.rs — copy_gpu_to_memory_async (lib.rs:3422), copy_gpu_to_memory_completed (lib.rs:3489),
// populate (lib.rs:3143), batch_populate (lib.rs:3211).

/// `regions.iter().map(|r| r.size as usize).sum()` (panics on overflow in a debug build).
#[ensures(result@ == sum_sz(r@))]
pub fn region_total(r: &[Ipc]) -> usize {
    let mut t: usize = 0;
    let mut i: usize = 0;
    proof_assert!(pre_zero(r@) == ());
    #[invariant(i@ <= r@.len() && t@ == pre(r@, i@))]
    while i < r.len() {
        proof_assert!(pre_step(r@, i@) == ());
        t = padd(t, r[i].size as usize);
        i += 1;
    }
    proof_assert!(pre_full(r@) == ());
    t
}

/// Ghost record of copy_gpu_to_memory_async: where its first D2H was logged and the stream used.
pub struct CopyG {
    pub base: Int,
    pub stream: u64,
    pub ptr: u64,
    /// the null-stream path resolved a store stream and synchronized it
    pub synced: bool,
    /// a GPU->host copy submission failed
    pub failed: bool,
}

/// D2H events for regions `0..n` sit at `base..base+n`, gathered contiguously from `ptr`.
#[logic(open)]
pub fn gathered(w: W, base: Int, r: Seq<Ipc>, n: Int, ptr: u64, stream: u64) -> bool {
    pearlite! { 0 <= base && base + n <= w.log.len() && forall<l: Int> 0 <= l && l < n ==>
        match w.log[base + l] { Ev::D2H { src, dst, len, stream: st } =>
            src == r[l].addr && dst@ == ptr@ + pre(r, l) && len@ == r[l].size@ && st == stream, _ => false } }
}

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result.0 == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures((*(^s).g).dm == (*s.g).dm && same_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).wq == (*s.g).wq && same_env(*s.g, *(^s).g))]
#[ensures(log_ext(*s.g, *(^s).g) && no_reg(*s.g, *(^s).g))]
#[ensures(forall<j: u64> j != key ==> (*(^s).g).mt.get(j) == (*s.g).mt.get(j))]
#[ensures(s.p.init && s.p.has_mt ==> match result.0 {
    Err(DE::KeyNotFound(k)) => k == key && !mt_has(*s.g, key) && no_dma(*s.g, *(^s).g) && (*(^s).g).mt == (*s.g).mt,
    Err(DE::InvalidParam) => mt_has(*s.g, key) && sum_sz(regions@) > (*s.g).mt.lookup(key).size@
                             && no_dma(*s.g, *(^s).g) && (*(^s).g).mt == (*s.g).mt,
    Err(DE::NotInit(n)) => n == Ni::Gpu && !s.p.has_gpu && (*(^s).g).mt == (*s.g).mt,
    Err(DE::Io(IoK::Gpu)) => mt_has(*s.g, key) && !mt_has(*(^s).g, key),
    Err(DE::Io(IoK::Sync)) => stream@ == 0 && (*(^s).g).mt == (*s.g).mt,
    Ok(()) => mt_has(*s.g, key) && sum_sz(regions@) <= (*s.g).mt.lookup(key).size@ && (*(^s).g).mt == (*s.g).mt
              && result.1.ptr == (*s.g).mt.lookup(key).ptr
              && gathered(*(^s).g, result.1.base, regions@, regions@.len(), result.1.ptr, result.1.stream)
              && (stream@ != 0 ==> result.1.stream == stream && !result.1.synced
                    && (*(^s).g).log.len() == result.1.base + regions@.len())
              && (stream@ == 0 ==> result.1.synced && (*(^s).g).log.len() == result.1.base + regions@.len() + 1
                    && (*(^s).g).log[result.1.base + regions@.len()] == Ev::Sync { stream: result.1.stream }),
    Err(_) => false,
})]
#[ensures(forall<i: Int> (*s.g).log.len() <= i && i < (*(^s).g).log.len() ==>
    match (*(^s).g).log[i] { Ev::D2H { stream: st, .. } => st == result.1.stream, Ev::H2D { .. } => false, _ => true })]
#[ensures(result.1.failed ==> result.0 == Err(DE::Io(IoK::Gpu)))]
#[ensures(result.0 == Err(DE::Io(IoK::Gpu)) ==> result.1.failed)]
pub fn copy_gpu_to_memory_async(s: &mut S, key: u64, regions: &[Ipc], stream: u64) -> (Result<(), DE>, Snapshot<CopyG>) {
    let g0 = snapshot!(*s.g);
    let dummy = snapshot!(CopyG { base: 0, stream: 0u64, ptr: 0u64, synced: false, failed: false });
    match ensure_initialized(s) {
        Err(e) => return (Err(e), dummy),
        Ok(()) => {}
    }
    if !s.p.has_mt {
        return (Err(DE::NotInit(Ni::MemoryTier)), dummy);
    }
    let (mem_ptr, slot_size) = match mt_get(s, key) {
        Some(v) => v,
        None => return (Err(DE::KeyNotFound(key)), dummy),
    };
    let total = region_total(regions);
    if total > slot_size as usize {
        return (Err(DE::InvalidParam), dummy);
    }
    if !s.p.has_gpu {
        return (Err(DE::NotInit(Ni::Gpu)), dummy);
    }
    let (effective_stream, needs_sync) = if stream == 0 {
        let device = if regions.len() > 0 { set_batch_device(s, regions[0].addr) } else { -1 };
        let store_raw = match device_streams_for(s, device) {
            Some(d) => d.store,
            None => 0,
        };
        (store_raw, true)
    } else {
        (stream, false)
    };
    let base = snapshot!(s.g.log.len());
    let g1 = snapshot!(*s.g);
    proof_assert!(forall<q: Int> g0.log.len() <= q && q < s.g.log.len() ==> s.g.log[q] == Ev::MtGet { key });
    let p0 = snapshot!(s.p);
    let mut off: usize = 0;
    let mut i: usize = 0;
    proof_assert!(pre_zero(regions@) == ());
    #[invariant(s.p == *p0 && i@ <= regions@.len() && off@ == pre(regions@, i@))]
    #[invariant(off@ <= sum_sz(regions@))]
    #[invariant(*s.g == W { log: s.g.log, tick: s.g.tick, ..*g1 } && log_ext(*g1, *s.g))]
    #[invariant(s.g.log.len() == *base + i@ && gathered(*s.g, *base, regions@, i@, mem_ptr, effective_stream))]
    #[invariant(forall<q: Int> g0.log.len() <= q && q < s.g.log.len() ==>
        match s.g.log[q] { Ev::D2H { stream: st, .. } => st == effective_stream, Ev::H2D { .. } => false,
                           Ev::DmCreate { .. } => false, Ev::Enq { .. } => false, _ => true })]
    while i < regions.len() {
        let region = regions[i];
        proof_assert!(pre_step(regions@, i@) == ());
        proof_assert!(pre_mono(regions@, i@ + 1, regions@.len()) == ());
        proof_assert!(pre_full(regions@) == ());
        let dst = padd64(mem_ptr, off as u64);
        if !gpu_d2h_async(s, region.addr, dst, region.size as usize, effective_stream) {
            let _ = mt_remove(s, key);
            return (Err(DE::Io(IoK::Gpu)), snapshot!(CopyG { base: *base, stream: effective_stream, ptr: mem_ptr, synced: false, failed: true }));
        }
        off = padd(off, region.size as usize);
        i += 1;
    }
    if needs_sync {
        if !gpu_sync(s, effective_stream) {
            return (Err(DE::Io(IoK::Sync)), snapshot!(CopyG { base: *base, stream: effective_stream, ptr: mem_ptr, synced: true, failed: false }));
        }
    }
    (Ok(()), snapshot!(CopyG { base: *base, stream: effective_stream, ptr: mem_ptr, synced: needs_sync, failed: false }))
}

// ---------------------------------------------------------------------------
// copy_gpu_to_memory_completed (lib.rs:3489)
// ---------------------------------------------------------------------------

#[logic(open)]
pub fn wjob(p: P, key: u64, size: u32) -> Job {
    pearlite! { Job { key, size, dev: di(key, if p.nd@ == 0 { 1usize } else { p.nd }) } }
}

#[requires(pinv(s.p))]
#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(same_ctr((*s.g).c, (*(^s).g).c) && same_env(*s.g, *(^s).g) && log_ext(*s.g, *(^s).g) && no_dma(*s.g, *(^s).g))]
#[ensures(others(*s.g, *(^s).g, key) && forall<j: u64> j != key ==> (*(^s).g).mt.get(j) == (*s.g).mt.get(j))]
#[ensures(s.p.init ==> match result {
    Err(DE::KeyNotFound(k)) => k == key && !mt_has(*s.g, key) && *(^s).g == *s.g,
    Err(DE::AlreadyExists(k)) => k == key && mt_has(*s.g, key) && has(*s.g, key) && !mt_has(*(^s).g, key)
                                 && (*(^s).g).dm == (*s.g).dm && (*(^s).g).wq == (*s.g).wq,
    Err(DE::Io(IoK::Dm)) => mt_has(*s.g, key) && (!has(*s.g, key) || size@ == 0) && !mt_has(*(^s).g, key)
                            && (*(^s).g).dm == (*s.g).dm && (*(^s).g).wq == (*s.g).wq,
    Ok(()) => mt_has(*s.g, key) && !has(*s.g, key) && (*(^s).g).mt == (*s.g).mt
              && (*(^s).g).dm.get(key) == Some(Ent { loc: Loc::Mem { ptr: (*s.g).mt.lookup(key).ptr, size, ssd: None }, es: size, rr: 1u32, wr: 0u32 })
              && (*(^s).g).wq == (*s.g).wq.push_back(wjob(s.p, key, size))
              && (*(^s).g).log[(*(^s).g).log.len() - 1] == Ev::Enq { job: wjob(s.p, key, size) },
    Err(_) => false,
})]
pub fn copy_gpu_to_memory_completed(s: &mut S, key: u64, size: u32) -> Result<(), DE> {
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
    let (mem_ptr, _) = match mt_get(s, key) {
        Some(v) => v,
        None => return Err(DE::KeyNotFound(key)),
    };
    let gc = snapshot!(*s.g);
    match dm_create(s, key, mem_ptr, size) {
        Ok(()) => {
            proof_assert!(!has(*gc, key) && has(*s.g, key) && ent(*s.g, key).wr@ == 1 && ent(*s.g, key).rr@ == 0);
        }
        Err(DmE::AlreadyExists) => {
            let _ = mt_remove(s, key);
            return Err(DE::AlreadyExists(key));
        }
        Err(_) => {
            let _ = mt_remove(s, key);
            return Err(DE::Io(IoK::Dm));
        }
    }
    match dm_downgrade(s, key) {
        Ok(()) => {}
        Err(_) => return Err(DE::Io(IoK::Dm)),
    }
    let num_drives = max1(s.p.nd);
    if s.p.writer {
        writer_enqueue(s, Job { key, size, dev: drive_index(key, num_drives) });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// populate (lib.rs:3143)
// ---------------------------------------------------------------------------

/// Ghost record of populate's path.
pub struct PopG {
    /// reserve_memory found room (a slot was allocated for the key)
    pub reserved: bool,
    /// the D2H copy submission or its stream synchronization failed
    pub copy_failed: bool,
    /// the state right after the reservation
    pub mid: W,
}

#[requires(pinv(s.p))]
#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result.0 == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && h.size@ == 0 ==> result.0 == Err(DE::InvalidParam) && *(^s).g == *s.g)]
#[ensures(result.0 != Err(DE::Alloc))]
#[ensures(s.p.init && h.size@ > 0 ==> match result.0 {
    Ok(()) => (result.1.reserved
                 && has(*(^s).g, key) && is_mem(ent(*(^s).g, key).loc) && ent(*(^s).g, key).wr@ == 0 && ent(*(^s).g, key).rr@ == 1
                 && mt_has(*(^s).g, key) && ent(*(^s).g, key).loc == Loc::Mem { ptr: (*(^s).g).mt.lookup(key).ptr, size: h.size, ssd: None }
                 && (*(^s).g).wq == (*s.g).wq.push_back(wjob(s.p, key, h.size))
                 && (forall<k: u64> k != key ==> ((*(^s).g).dm.get(k) == (*s.g).dm.get(k) && (*(^s).g).mt.get(k) == (*s.g).mt.get(k)) || demoted_k(*s.g, *(^s).g, k))
                 && result.1.mid.used <= result.1.mid.cap && result.1.mid.mt.get(key) != None
                 && !has(*s.g, key))
            || (!result.1.reserved && (*(^s).g).c.drop_full == (*s.g).c.drop_full + 1 && evict_frame(*s.g, *(^s).g)
                 && (!has(*s.g, key) ==> !has(*(^s).g, key))),
    Err(DE::Io(_)) => result.1.copy_failed || result.1.reserved,
    _ => true,
})]
#[ensures(result.1.copy_failed ==> match result.0 { Err(DE::Io(_)) => true, _ => false })]
#[ensures(s.p.init && s.p.has_gpu && h.size@ > 0 && result.1.reserved && match result.0 { Err(DE::Io(IoK::Sync)) => true, _ => false } ==> mt_has(*(^s).g, key))]
#[ensures(s.p.init && h.size@ > 0 && !s.p.has_gpu && result.1.reserved ==> result.0 == Err(DE::NotInit(Ni::Gpu)) && mt_has(*(^s).g, key))]
#[ensures(s.p.init && h.size@ > 0 && s.p.bp_ms@ == 0 && all_from((*s.g).ob, (*s.g).tick) && (*s.g).used + h.size@ <= (*s.g).cap
    && !mt_has(*s.g, key) ==> result.1.reserved)]
#[ensures(s.p.init && h.size@ > 0 && s.p.bp_ms@ == 0 && s.p.max_att@ == 0 && (*s.g).used + h.size@ > (*s.g).cap ==>
    result.0 == Ok(()) && !result.1.reserved && (*(^s).g).dm == (*s.g).dm)]
pub fn populate(s: &mut S, key: u64, h: Ipc) -> (Result<(), DE>, Snapshot<PopG>) {
    let g0 = snapshot!(*s.g);
    match ensure_initialized(s) {
        Err(e) => return (Err(e), snapshot!(PopG { reserved: false, copy_failed: false, mid: *g0 })),
        Ok(()) => {}
    }
    if h.size == 0 {
        return (Err(DE::InvalidParam), snapshot!(PopG { reserved: false, copy_failed: false, mid: *g0 }));
    }
    let size: u32 = h.size;
    // Phase 1: reserve (own backpressure budget, no shared deadline).
    let (r, _rg) = reserve_memory(s, key, size, None);
    match r {
        Ok(_) => {}
        Err(DE::Alloc) => {
            rec_store_drop_on_full(s);
            return (Ok(()), snapshot!(PopG { reserved: false, copy_failed: false, mid: *s.g }));
        }
        Err(e) => return (Err(e), snapshot!(PopG { reserved: false, copy_failed: false, mid: *s.g })),
    }
    let mid = snapshot!(*s.g);
    // Phase 2: D2H into the reserved slot on the device's store stream, then sync.
    if !s.p.has_gpu {
        return (Err(DE::NotInit(Ni::Gpu)), snapshot!(PopG { reserved: true, copy_failed: false, mid: *mid }));
    }
    let device = set_batch_device(s, h.addr);
    let store_raw = match device_streams_for(s, device) {
        Some(d) => d.store,
        None => 0,
    };
    let (cr, _cg) = copy_gpu_to_memory_async(s, key, &[h], store_raw);
    match cr {
        Ok(()) => {}
        Err(DE::Io(k)) => return (Err(DE::Io(k)), snapshot!(PopG { reserved: true, copy_failed: true, mid: *mid })),
        Err(e) => return (Err(e), snapshot!(PopG { reserved: true, copy_failed: false, mid: *mid })),
    }
    if !gpu_sync(s, store_raw) {
        return (Err(DE::Io(IoK::Sync)), snapshot!(PopG { reserved: true, copy_failed: true, mid: *mid }));
    }
    // Phase 3: register in the dispatch map and enqueue the write-through.
    match copy_gpu_to_memory_completed(s, key, size) {
        Ok(()) => {}
        Err(e) => return (Err(e), snapshot!(PopG { reserved: true, copy_failed: false, mid: *mid })),
    }
    (Ok(()), snapshot!(PopG { reserved: true, copy_failed: false, mid: *mid }))
}

// ---------------------------------------------------------------------------
// batch_populate (lib.rs:3211)
// ---------------------------------------------------------------------------

/// One `(CacheKey, IpcHandle)` batch entry.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Pe {
    pub key: u64,
    pub h: Ipc,
}

/// How the first pass disposed of an entry (ghost).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Kind {
    Unseen,
    Zero,
    Dropped,
    RsvErr,
    CopyErr,
    Sub,
}

pub struct BpG {
    pub kind: Seq<Kind>,
    /// release_memory(key) returned Ok for this entry (its slot is gone at that point)
    pub rel: Seq<bool>,
    /// copy_gpu_to_memory_completed was called for this entry
    pub reg: Seq<bool>,
    /// position of the batch stream_synchronize event (-1 if none)
    pub sync_at: Int,
    pub sync_ok: bool,
    pub store: u64,
}

#[logic(open)]
pub fn is_err(r: Option<Result<(), DE>>) -> bool {
    pearlite! { match r { Some(Err(_)) => true, _ => false } }
}

#[requires(pinv(s.p))]
#[ensures((^s).p == s.p)]
#[ensures(result.0@.len() == entries@.len())]
#[ensures(entries@.len() == 0 ==> *(^s).g == *s.g)]
#[ensures(s.p.init && !s.p.has_gpu ==> *(^s).g == *s.g && forall<i: Int> 0 <= i && i < entries@.len() ==> result.0@[i] == Err(DE::NotInit(Ni::Gpu)))]
#[ensures(!s.p.init ==> forall<i: Int> 0 <= i && i < entries@.len() ==> result.0@[i] == Err(DE::NotInit(Ni::Dispatcher)))]
#[ensures(s.p.init && s.p.has_gpu && entries@.len() > 0 ==> result.1.kind.len() == entries@.len()
    && result.1.rel.len() == entries@.len() && result.1.reg.len() == entries@.len()
    && (forall<i: Int> 0 <= i && i < entries@.len() ==>
        result.1.kind[i] != Kind::Unseen
        && ((entries@[i].h.size@ == 0) == (result.1.kind[i] == Kind::Zero))
        && (result.1.kind[i] == Kind::Zero ==> result.0@[i] == Err(DE::InvalidParam))
        && (result.1.kind[i] == Kind::Dropped ==> result.0@[i] == Ok(()) && !result.1.reg[i])
        && (result.1.kind[i] == Kind::CopyErr ==> (match result.0@[i] { Err(_) => true, _ => false }) && result.1.rel[i] && !result.1.reg[i])
        && (result.1.kind[i] == Kind::RsvErr ==> (match result.0@[i] { Err(_) => true, _ => false }) && !result.1.reg[i])
        && (result.1.reg[i] ==> result.1.kind[i] == Kind::Sub && result.1.sync_ok)
        && (result.1.kind[i] == Kind::Sub && !result.1.sync_ok ==> result.0@[i] == Err(DE::Io(IoK::Sync)) && result.1.rel[i] && !result.1.reg[i])
        && (result.1.kind[i] == Kind::Sub && result.1.sync_ok ==> result.1.reg[i])
        && (match result.0@[i] { Err(DE::AlreadyExists(k)) => k == entries@[i].key, _ => true }))
    && ((exists<i: Int> 0 <= i && i < entries@.len() && result.1.kind[i] == Kind::Sub) ==>
          (*s.g).log.len() <= result.1.sync_at && result.1.sync_at < (*(^s).g).log.len()
          && (*(^s).g).log[result.1.sync_at] == Ev::Sync { stream: result.1.store })
    && (forall<q: Int> (*s.g).log.len() <= q && q < (*(^s).g).log.len() ==>
          match (*(^s).g).log[q] { Ev::DmCreate { .. } => q > result.1.sync_at && result.1.sync_at >= 0 && result.1.sync_ok,
                                   Ev::Enq { .. } => q > result.1.sync_at && result.1.sync_at >= 0 && result.1.sync_ok, _ => true }))]
pub fn batch_populate(s: &mut S, entries: &[Pe]) -> (Vec<Result<(), DE>>, Snapshot<BpG>) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let n = entries.len();
    let empty_g = snapshot!(BpG { kind: Seq::empty(), rel: Seq::empty(), reg: Seq::empty(), sync_at: -1, sync_ok: false, store: 0u64 });
    if n == 0 {
        return (Vec::new(), empty_g);
    }
    match ensure_initialized(s) {
        Err(e) => return (fill_err(n, e), empty_g),
        Ok(()) => {}
    }
    if !s.p.has_gpu {
        return (fill_err(n, DE::NotInit(Ni::Gpu)), empty_g);
    }
    // Resolve the batch device from the first non-null address, and its store stream.
    let mut fi: usize = 0;
    #[invariant(fi@ <= n@)]
    while fi < n && entries[fi].h.addr == 0 {
        fi += 1;
    }
    let batch_device = if fi < n { set_batch_device(s, entries[fi].h.addr) } else { -1 };
    let store_raw = match device_streams_for(s, batch_device) {
        Some(d) => d.store,
        None => 0,
    };
    let g1 = snapshot!(*s.g);
    let mut results: Vec<Option<Result<(), DE>>> = Vec::new();
    let mut kind: Snapshot<Seq<Kind>> = snapshot!(Seq::empty());
    let mut rel: Snapshot<Seq<bool>> = snapshot!(Seq::empty());
    let mut submitted: Vec<usize> = Vec::new();
    let mut pos: Snapshot<Mapping<Int, Int>> = snapshot!(|j: Int| 0);
    let mut i: usize = 0;
    // Interleaved reserve + async D2H.
    #[invariant(s.p == *p0 && i@ <= n@ && results@.len() == i@ && kind.len() == i@ && rel.len() == i@)]
    #[invariant(log_ext(*g1, *s.g) && no_reg(*g1, *s.g))]
    #[invariant(forall<j: Int> 0 <= j && j < submitted@.len() ==> submitted@[j]@ < i@ && kind[submitted@[j]@] == Kind::Sub)]
    #[invariant(forall<j: Int, k: Int> 0 <= j && j < k && k < submitted@.len() ==> submitted@[j]@ < submitted@[k]@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> kind[j] == Kind::Sub ==> 0 <= pos[j] && pos[j] < submitted@.len() && submitted@[pos[j]]@ == j)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==>
        kind[j] != Kind::Unseen
        && ((entries@[j].h.size@ == 0) == (kind[j] == Kind::Zero))
        && (kind[j] == Kind::Zero ==> results@[j] == Some(Err(DE::InvalidParam)))
        && (kind[j] == Kind::Dropped ==> results@[j] == Some(Ok(())))
        && (kind[j] == Kind::CopyErr ==> is_err(results@[j]) && rel[j])
        && (kind[j] == Kind::RsvErr ==> is_err(results@[j]))
        && (kind[j] == Kind::Sub ==> results@[j] == None)
        && (match results@[j] { Some(Err(DE::AlreadyExists(k))) => k == entries@[j].key, _ => true }))]
    while i < n {
        let e = entries[i];
        if e.h.size == 0 {
            results.push(Some(Err(DE::InvalidParam)));
            kind = snapshot!(kind.push_back(Kind::Zero));
            rel = snapshot!(rel.push_back(false));
            i += 1;
            continue;
        }
        let (r, _rg) = reserve_memory(s, e.key, e.h.size, None);
        match r {
            Ok(_) => {
                let (cr, _cg) = copy_gpu_to_memory_async(s, e.key, &[e.h], store_raw);
                match cr {
                    Ok(()) => {
                        pos = snapshot!(pos.set(i@, submitted@.len()));
                        submitted.push(i);
                        results.push(None);
                        kind = snapshot!(kind.push_back(Kind::Sub));
                        rel = snapshot!(rel.push_back(false));
                    }
                    Err(err) => {
                        let rr = release_memory(s, e.key);
                        results.push(Some(Err(err)));
                        kind = snapshot!(kind.push_back(Kind::CopyErr));
                        rel = snapshot!(rel.push_back(rr == Ok(())));
                    }
                }
            }
            Err(DE::Alloc) => {
                rec_store_drop_on_full(s);
                results.push(Some(Ok(())));
                kind = snapshot!(kind.push_back(Kind::Dropped));
                rel = snapshot!(rel.push_back(false));
            }
            Err(err) => {
                results.push(Some(Err(err)));
                kind = snapshot!(kind.push_back(Kind::RsvErr));
                rel = snapshot!(rel.push_back(false));
            }
        }
        i += 1;
    }
    let none_reg = snapshot!(Seq::create(n@, |j: Int| false));
    if submitted.len() == 0 {
        let out = unwrap_all(results);
        return (out, snapshot!(BpG { kind: *kind, rel: *rel, reg: *none_reg, sync_at: -1, sync_ok: false, store: store_raw }));
    }
    // ONE stream_synchronize for the whole batch.
    let sync_at = snapshot!(s.g.log.len());
    if !gpu_sync(s, store_raw) {
        let g2 = snapshot!(*s.g);
        let res0 = snapshot!(results@);
        let rel0 = snapshot!(*rel);
        let mut j: usize = 0;
        #[invariant(s.p == *p0 && j@ <= submitted@.len() && results@.len() == n@ && rel.len() == n@)]
        #[invariant(log_ext(*g2, *s.g) && no_reg(*g2, *s.g))]
        #[invariant(s.g.log.len() > *sync_at && s.g.log[*sync_at] == Ev::Sync { stream: store_raw })]
        #[invariant(forall<q: Int> 0 <= q && q < j@ ==> results@[submitted@[q]@] == Some(Err(DE::Io(IoK::Sync))) && rel[submitted@[q]@])]
        #[invariant(forall<q: Int> j@ <= q && q < submitted@.len() ==> results@[submitted@[q]@] == None)]
        #[invariant(forall<k: Int> 0 <= k && k < n@ ==> kind[k] == Kind::Sub ==> 0 <= pos[k] && pos[k] < submitted@.len() && submitted@[pos[k]]@ == k)]
        #[invariant(forall<k: Int> 0 <= k && k < n@ ==> kind[k] != Kind::Sub ==> results@[k] == res0[k] && rel[k] == rel0[k])]
        while j < submitted.len() {
            let idx = submitted[j];
            let rr = release_memory(s, entries[idx].key);
            results[idx] = Some(Err(DE::Io(IoK::Sync)));
            rel = snapshot!(rel.set(idx@, rr == Ok(())));
            j += 1;
        }
        let out = unwrap_all(results);
        return (out, snapshot!(BpG { kind: *kind, rel: *rel, reg: *none_reg, sync_at: *sync_at, sync_ok: false, store: store_raw }));
    }
    // Register every submitted entry and enqueue its write-through.
    let mut reg: Snapshot<Seq<bool>> = none_reg;
    let g3 = snapshot!(*s.g);
    let res1 = snapshot!(results@);
    let mut j: usize = 0;
    #[invariant(s.p == *p0 && j@ <= submitted@.len() && results@.len() == n@ && reg.len() == n@)]
    #[invariant(log_ext(*g3, *s.g) && s.g.log.len() > *sync_at && s.g.log[*sync_at] == Ev::Sync { stream: store_raw })]
    #[invariant(forall<q: Int> 0 <= q && q < j@ ==> reg[submitted@[q]@] && results@[submitted@[q]@] != None
                  && match results@[submitted@[q]@] { Some(Err(DE::AlreadyExists(k))) => k == entries@[submitted@[q]@].key, _ => true })]
    #[invariant(forall<q: Int> j@ <= q && q < submitted@.len() ==> !reg[submitted@[q]@] && results@[submitted@[q]@] == None)]
    #[invariant(forall<k: Int> 0 <= k && k < n@ ==> kind[k] == Kind::Sub ==> 0 <= pos[k] && pos[k] < submitted@.len() && submitted@[pos[k]]@ == k)]
    #[invariant(forall<k: Int> 0 <= k && k < n@ ==> kind[k] != Kind::Sub ==> results@[k] == res1[k] && !reg[k])]
    while j < submitted.len() {
        let idx = submitted[j];
        let r = copy_gpu_to_memory_completed(s, entries[idx].key, entries[idx].h.size);
        results[idx] = Some(r);
        reg = snapshot!(reg.set(idx@, true));
        j += 1;
    }
    let out = unwrap_all(results);
    (out, snapshot!(BpG { kind: *kind, rel: *rel, reg: *reg, sync_at: *sync_at, sync_ok: true, store: store_raw }))
}


/// `entries.iter().map(|_| Err(e.clone())).collect()`
#[ensures(result@.len() == n@ && forall<i: Int> 0 <= i && i < n@ ==> result@[i] == Err(e))]
pub fn fill_err(n: usize, e: DE) -> Vec<Result<(), DE>> {
    let mut v: Vec<Result<(), DE>> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= n@ && v@.len() == i@ && forall<j: Int> 0 <= j && j < i@ ==> v@[j] == Err(e))]
    while i < n {
        v.push(Err(e));
        i += 1;
    }
    v
}

/// `results.into_iter().map(|r| r.unwrap()).collect()` — a None would panic.
#[ensures(result@.len() == r@.len() && forall<i: Int> 0 <= i && i < r@.len() ==> r@[i] == Some(result@[i]))]
pub fn unwrap_all(r: Vec<Option<Result<(), DE>>>) -> Vec<Result<(), DE>> {
    let mut v: Vec<Result<(), DE>> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= r@.len() && v@.len() == i@ && forall<j: Int> 0 <= j && j < i@ ==> r@[j] == Some(v@[j]))]
    while i < r.len() {
        v.push(unwrap_or_panic(r[i]));
        i += 1;
    }
    v
}
