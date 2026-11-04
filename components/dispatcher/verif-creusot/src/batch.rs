// batch.rs — batch_lookup (lib.rs:2228-2992) and its helpers serve_cold_staged (lib.rs:919),
// serve_concurrently_promoted (lib.rs:1349). The real function is one 760-line body; the mirror
// splits it at its phase boundaries (classify, warm touch, cold path, warm sync + pin release,
// remote pass, concurrent-promotion recovery, attribution) with the same statements in order.

/// One `(CacheKey, Vec<IpcHandle>)` batch entry.
pub struct Be {
    pub key: u64,
    pub regions: Vec<Ipc>,
}

/// The level-2 assumption D-RANGE-FR-039-d29c77 at the call of batch_lookup: every entry's
/// region sizes add up to at most u32::MAX.
#[logic(open)]
pub fn a_fr039(e: Seq<Be>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < e.len() ==> sum_sz(e[i].regions@) <= u32::MAX@ }
}

/// `regions.iter().map(|r| r.size).sum::<u32>()`.
#[requires(sum_sz(r@) <= u32::MAX@)]
#[ensures(result@ == sum_sz(r@))]
pub fn total_u32(r: &Vec<Ipc>) -> u32 {
    let mut t: u32 = 0;
    let mut i: usize = 0;
    proof_assert!(pre_zero(r@) == ());
    proof_assert!(pre_full(r@) == ());
    #[invariant(i@ <= r@.len() && t@ == pre(r@, i@))]
    while i < r.len() {
        proof_assert!(pre_step(r@, i@) == ());
        proof_assert!(pre_mono(r@, i@ + 1, r@.len()) == ());
        t = t + r[i].size;
        i += 1;
    }
    t
}

/// How the classification pass disposed of entry i (ghost).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Cls {
    /// NotExist: KeyNotFound (a miss, eligible for remote forwarding)
    Miss,
    /// A dispatch-map lookup error that survived the retries: the entry is live
    /// (a writer holds it), so the map's own error is reported — Timeout maps
    /// to DE::Timeout, anything else to Io(Dm) — never KeyNotFound, and the
    /// entry is never forwarded to remote lookup (FR-039 step 1).
    Retry { e: DmE },
    /// MemoryTier hit: H2D queued (not synchronized), pin adopted
    Warm { ptr: u64, size: u32 },
    /// BlockDevice: pin released, queued for the cold path
    Cold { off: u64 },
}

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct ColdE {
    pub idx: usize,
    pub key: u64,
    pub off: u64,
    pub total: u32,
}

pub type Res = Option<Result<(), DE>>;

/// The batch's working state (the locals of batch_lookup).
pub struct Bl {
    pub results: Vec<Res>,
    pub tier: Vec<Option<ServedBy>>,
    pub cold: Vec<ColdE>,
    pub warm_keys: Vec<u64>,
}

/// No result became KeyNotFound or None that was not already.
#[logic(open)]
pub fn no_new_kn(r0: Seq<Res>, r: Seq<Res>) -> bool {
    pearlite! { r0.len() == r.len() && forall<i: Int> 0 <= i && i < r.len() ==> (is_kn(r[i]) ==> is_kn(r0[i])) && (r[i] == None ==> r0[i] == None) }
}

#[logic(open)]
pub fn is_kn(r: Res) -> bool {
    pearlite! { match r { Some(Err(DE::KeyNotFound(_))) => true, _ => false } }
}

/// Retry wrapper (lib.rs:2377-2386): up to LOOKUP_RETRIES = 5 extra lookups while Err, then the
/// map's own error is returned — never a synthesized KeyNotFound. In a single-threaded run the
/// dispatch map answers the same way each time. The second component is the retry count (ghost:
/// how many extra lookups the retries issued, at most LOOKUP_RETRIES).
#[ensures((^s).p == s.p)]
#[ensures(result.1@ <= 5)]
#[ensures(match result.0 {
    Ok(Lr::NotExist) => !has(*s.g, k) && *(^s).g == *s.g,
    Ok(Lr::Mismatch) => false,
    Ok(r) => has(*s.g, k) && ent(*s.g, k).wr@ == 0 && ent(*s.g, k).rr@ < u32::MAX@ && r == lr_of(ent(*s.g, k).loc)
             && ent_rr(*s.g, *(^s).g, k, 1) && only_dm(*s.g, *(^s).g),
    Err(_) => has(*s.g, k) && (ent(*s.g, k).wr@ > 0 || ent(*s.g, k).rr@ == u32::MAX@) && *(^s).g == *s.g,
})]
pub fn lookup_retry(s: &mut S, k: u64) -> (Result<Lr, DmE>, Snapshot<u32>) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut lk = dm_lookup(s, k);
    let mut retries: u32 = 0;
    #[invariant(s.p == *p0 && retries@ <= 5)]
    #[invariant(match lk { Err(_) => *s.g == *g0 && has(*g0, k) && (ent(*g0, k).wr@ > 0 || ent(*g0, k).rr@ == u32::MAX@),
                           Ok(Lr::NotExist) => !has(*g0, k) && *s.g == *g0,
                           Ok(Lr::Mismatch) => false,
                           Ok(r) => has(*g0, k) && ent(*g0, k).wr@ == 0 && ent(*g0, k).rr@ < u32::MAX@ && r == lr_of(ent(*g0, k).loc)
                                    && ent_rr(*g0, *s.g, k, 1) && only_dm(*g0, *s.g) })]
    while (match lk { Err(_) => true, Ok(_) => false }) && retries < 5 {
        retries += 1;
        lk = dm_lookup(s, k);
    }
    (lk, snapshot!(retries))
}

// ---------------------------------------------------------------------------
// Phase 1: classification (lib.rs:2353-2449)
// ---------------------------------------------------------------------------

pub struct ClsG {
    pub cls: Seq<Cls>,
    /// log position of entry i's serve (warm hits)
    pub sv: Seq<ServeG>,
    /// position of a cold entry in `cold`
    pub cpos: Seq<Int>,
}

#[requires(a_fr039(entries@))]
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq && (*(^s).g).tick >= (*s.g).tick)]
#[ensures((*(^s).g).mt == (*s.g).mt && same_ctr((*s.g).c, (*(^s).g).c) && no_reg(*s.g, *(^s).g) && no_ssd(*s.g, *(^s).g) && (*(^s).g).used == (*s.g).used)]
#[ensures(result.0.results@.len() == entries@.len() && result.0.tier@.len() == entries@.len() && result.1.cls.len() == entries@.len()
    && result.1.sv.len() == entries@.len())]
#[ensures(forall<k: u64> has(*s.g, k) == has(*(^s).g, k)
    && (has(*s.g, k) ==> ent(*(^s).g, k).loc == ent(*s.g, k).loc && ent(*(^s).g, k).wr == ent(*s.g, k).wr && ent(*(^s).g, k).es == ent(*s.g, k).es
        && ent(*(^s).g, k).rr@ == ent(*s.g, k).rr@ + count_k(result.0.warm_keys@, k)))]
#[ensures(forall<i: Int> 0 <= i && i < entries@.len() ==> match result.1.cls[i] {
    Cls::Miss => result.0.results@[i] == Some(Err(DE::KeyNotFound(entries@[i].key))) && result.0.tier@[i] == None,
    Cls::Retry { e } => has(*s.g, entries@[i].key)
                 && result.0.results@[i] == Some(Err(match e { DmE::Timeout => DE::Timeout, _ => DE::Io(IoK::Dm) }))
                 && result.0.tier@[i] == None,
    Cls::Warm { ptr, size } => has(*s.g, entries@[i].key) && ent(*s.g, entries@[i].key).loc == Loc::Mem { ptr, size, ssd: ssd_of(ent(*s.g, entries@[i].key).loc) }
                 && result.0.tier@[i] == Some(ServedBy::Dram)
                 && (match result.0.results@[i] { Some(Ok(())) => result.1.sv[i].gpu_ok, Some(Err(DE::Io(_))) => !result.1.sv[i].gpu_ok, _ => false })
                 && h2d_scatter_ok(*(^s).g, result.1.sv[i], entries@[i].regions@, ptr, size),
    Cls::Cold { off } => has(*s.g, entries@[i].key) && ent(*s.g, entries@[i].key).loc == Loc::Blk { off }
                 && result.0.results@[i] == None && result.0.tier@[i] == Some(ServedBy::Ssd),
})]
#[ensures(forall<i: Int> 0 <= i && i < entries@.len() && !has(*s.g, entries@[i].key) ==> result.1.cls[i] == Cls::Miss)]
#[ensures(forall<q: Int> 0 <= q && q < result.0.cold@.len() ==> 0 <= result.0.cold@[q].idx@ && result.0.cold@[q].idx@ < entries@.len()
    && result.1.cls[result.0.cold@[q].idx@] == Cls::Cold { off: result.0.cold@[q].off } && result.0.cold@[q].key == entries@[result.0.cold@[q].idx@].key
    && result.0.cold@[q].total@ == sum_sz(entries@[result.0.cold@[q].idx@].regions@))]
#[ensures(forall<q: Int, r: Int> 0 <= q && q < r && r < result.0.cold@.len() ==> result.0.cold@[q].idx@ < result.0.cold@[r].idx@)]
#[ensures(forall<i: Int> 0 <= i && i < entries@.len() && (match result.1.cls[i] { Cls::Cold { .. } => true, _ => false }) ==>
    0 <= result.1.cpos[i] && result.1.cpos[i] < result.0.cold@.len() && result.0.cold@[result.1.cpos[i]].idx@ == i)]
#[ensures((result.0.warm_keys@.len() == 0) == (forall<i: Int> 0 <= i && i < entries@.len() ==> !is_warm(result.1.cls[i])))]
#[ensures(forall<i: Int> (*s.g).log.len() <= i && i < (*(^s).g).log.len() ==>
    match (*(^s).g).log[i] { Ev::H2D { stream, .. } => stream == warm_raw, Ev::RelRead { own, .. } => own > 0, Ev::RelWrite { .. } => false, _ => true })]
pub fn bl_classify(s: &mut S, entries: &[Be], warm_raw: u64) -> (Bl, Snapshot<ClsG>) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let n = entries.len();
    let mut results: Vec<Res> = Vec::new();
    let mut tier: Vec<Option<ServedBy>> = Vec::new();
    let mut cold: Vec<ColdE> = Vec::new();
    let mut warm_keys: Vec<u64> = Vec::new();
    let mut cls: Snapshot<Seq<Cls>> = snapshot!(Seq::empty());
    let mut svs: Snapshot<Seq<ServeG>> = snapshot!(Seq::empty());
    let mut cpos: Snapshot<Seq<Int>> = snapshot!(Seq::empty());
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= n@ && log_ext(*g0, *s.g) && same_env(*g0, *s.g) && s.g.wq == g0.wq && s.g.tick >= g0.tick)]
    #[invariant(s.g.mt == g0.mt && same_ctr(g0.c, s.g.c) && no_reg(*g0, *s.g) && no_ssd(*g0, *s.g) && s.g.used == g0.used)]
    #[invariant(results@.len() == i@ && tier@.len() == i@ && cls.len() == i@ && svs.len() == i@ && cpos.len() == i@)]
    #[invariant(forall<k: u64> has(*g0, k) == has(*s.g, k)
        && (has(*g0, k) ==> ent(*s.g, k).loc == ent(*g0, k).loc && ent(*s.g, k).wr == ent(*g0, k).wr && ent(*s.g, k).es == ent(*g0, k).es
            && ent(*s.g, k).rr@ == ent(*g0, k).rr@ + count_k(warm_keys@, k)))]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> match cls[j] {
        Cls::Miss => results@[j] == Some(Err(DE::KeyNotFound(entries@[j].key))) && tier@[j] == None,
        Cls::Retry { e } => has(*g0, entries@[j].key)
                     && results@[j] == Some(Err(match e { DmE::Timeout => DE::Timeout, _ => DE::Io(IoK::Dm) })) && tier@[j] == None,
        Cls::Warm { ptr, size } => has(*g0, entries@[j].key) && ent(*g0, entries@[j].key).loc == Loc::Mem { ptr, size, ssd: ssd_of(ent(*g0, entries@[j].key).loc) }
                     && tier@[j] == Some(ServedBy::Dram)
                     && (match results@[j] { Some(Ok(())) => svs[j].gpu_ok, Some(Err(DE::Io(_))) => !svs[j].gpu_ok, _ => false })
                     && h2d_scatter_ok(*s.g, svs[j], entries@[j].regions@, ptr, size),
        Cls::Cold { off } => has(*g0, entries@[j].key) && ent(*g0, entries@[j].key).loc == Loc::Blk { off }
                     && results@[j] == None && tier@[j] == Some(ServedBy::Ssd)
                     && 0 <= cpos[j] && cpos[j] < cold@.len() && cold@[cpos[j]].idx@ == j,
    })]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && !has(*g0, entries@[j].key) ==> cls[j] == Cls::Miss)]
    #[invariant(forall<q: Int> 0 <= q && q < cold@.len() ==> 0 <= cold@[q].idx@ && cold@[q].idx@ < i@
        && cls[cold@[q].idx@] == Cls::Cold { off: cold@[q].off } && cold@[q].key == entries@[cold@[q].idx@].key
        && cold@[q].total@ == sum_sz(entries@[cold@[q].idx@].regions@))]
    #[invariant(forall<q: Int, r: Int> 0 <= q && q < r && r < cold@.len() ==> cold@[q].idx@ < cold@[r].idx@)]
    #[invariant((warm_keys@.len() == 0) == (forall<j: Int> 0 <= j && j < i@ ==> !is_warm(cls[j])))]
    #[invariant(forall<q: Int> g0.log.len() <= q && q < s.g.log.len() ==>
        match s.g.log[q] { Ev::H2D { stream, .. } => stream == warm_raw, Ev::RelRead { own, .. } => own > 0, Ev::RelWrite { .. } => false, _ => true })]
    while i < n {
        let key = entries[i].key;
        let total_size = total_u32(&entries[i].regions);
        let gi = snapshot!(*s.g);
        let dsv = snapshot!(ServeG { base: 0, m: 0, gpu_ok: false });
        match lookup_retry(s, key).0 {
            Ok(Lr::NotExist) => {
                results.push(Some(Err(DE::KeyNotFound(key))));
                tier.push(None);
                cls = snapshot!(cls.push_back(Cls::Miss));
                svs = snapshot!(svs.push_back(*dsv));
                cpos = snapshot!(cpos.push_back(0));
            }
            Ok(Lr::Mismatch) => {
                let _ = dm_release_read(s, key, snapshot!(1));
                results.push(Some(Err(DE::InvalidParam)));
                tier.push(None);
                cls = snapshot!(cls.push_back(Cls::Miss));
                svs = snapshot!(svs.push_back(*dsv));
                cpos = snapshot!(cpos.push_back(0));
            }
            Ok(Lr::Mem { ptr, size }) => {
                // Warm hit: queue the H2D copies; the stream sync is deferred (synchronize = false).
                let (res, sv) = serve_memory_tier_to_gpu(s, ptr, size, &entries[i].regions, warm_raw, false);
                // The copy is only submitted: the pin is adopted by `warm_pins`.
                proof_assert!(forall<k: u64> count_push(warm_keys@, key, k) == ());
                warm_keys.push(key);
                results.push(Some(res));
                tier.push(Some(ServedBy::Dram));
                cls = snapshot!(cls.push_back(Cls::Warm { ptr, size }));
                svs = snapshot!(svs.push_back(*sv));
                cpos = snapshot!(cpos.push_back(0));
            }
            Ok(Lr::Blk { off }) => {
                let _ = dm_release_read(s, key, snapshot!(1));
                tier.push(Some(ServedBy::Ssd));
                results.push(None);
                cpos = snapshot!(cpos.push_back(cold@.len()));
                cold.push(ColdE { idx: i, key, off, total: total_size });
                cls = snapshot!(cls.push_back(Cls::Cold { off }));
                svs = snapshot!(svs.push_back(*dsv));
            }
            Err(e) => {
                // Every retry timed out behind a writer: the key is live, so the
                // map's own error is what this entry reports — never KeyNotFound
                // (FR-039 step 1), which would attribute it a Miss and forward
                // it to remote lookup.
                let de = match e { DmE::Timeout => DE::Timeout, _ => DE::Io(IoK::Dm) };
                results.push(Some(Err(de)));
                tier.push(None);
                cls = snapshot!(cls.push_back(Cls::Retry { e }));
                svs = snapshot!(svs.push_back(*dsv));
                cpos = snapshot!(cpos.push_back(0));
            }
        }
        proof_assert!(forall<q: Int> 0 <= q && q < gi.log.len() ==> s.g.log[q] == gi.log[q]);
        proof_assert!(forall<j: Int> 0 <= j && j < i@ ==> match cls[j] {
            Cls::Warm { ptr, size } => scatter_ok_ext(*gi, *s.g, svs[j], entries@[j].regions@, ptr, size) == (), _ => true });
        i += 1;
    }
    (Bl { results, tier, cold, warm_keys }, snapshot!(ClsG { cls: *cls, sv: *svs, cpos: *cpos }))
}

#[logic]
#[ensures(count_k(s.push_back(x), k) == count_k(s, k) + if x == k { 1 } else { 0 })]
pub fn count_push(s: Seq<u64>, x: u64, k: u64) {
    proof_assert! { s.push_back(x).subsequence(0, s.len()) == s };
}

#[logic]
#[requires(h2d_scatter_ok(a, sv, r, ptr, size))]
#[requires(a.log.len() <= b.log.len() && forall<q: Int> 0 <= q && q < a.log.len() ==> b.log[q] == a.log[q])]
#[ensures(h2d_scatter_ok(b, sv, r, ptr, size))]
pub fn scatter_ok_ext(a: W, b: W, sv: ServeG, r: Seq<Ipc>, ptr: u64, size: u32) {}

#[logic(open)]
pub fn is_warm(c: Cls) -> bool {
    pearlite! { match c { Cls::Warm { .. } => true, _ => false } }
}

/// Occurrences of `k` in `s`.
#[logic]
#[variant(s.len())]
pub fn count_k(s: Seq<u64>, k: u64) -> Int {
    pearlite! { if s.len() == 0 { 0 } else { count_k(s.subsequence(0, s.len() - 1), k) + if s[s.len() - 1] == k { 1 } else { 0 } } }
}

/// The serve record `sv` of a warm hit: its H2D copies read inside the slot and region l receives
/// `chunk_of` bytes from `ptr + pre(l)` (up to the first region with nothing left).
#[logic(open)]
pub fn h2d_scatter_ok(w: W, sv: ServeG, r: Seq<Ipc>, ptr: u64, size: u32) -> bool {
    pearlite! { 0 <= sv.base && 0 <= sv.m && sv.m <= r.len() && sv.base + sv.m <= w.log.len()
        && forall<l: Int> 0 <= l && l < sv.m ==>
            match w.log[sv.base + l] { Ev::H2D { src, dst, len, .. } =>
                src@ == ptr@ + pre(r, l) && dst == r[l].addr && len@ == chunk_of(r, l, size@) && len@ > 0
                && ptr@ <= src@ && src@ + len@ <= ptr@ + size@, _ => false } }
}

// ---------------------------------------------------------------------------
// serve_cold_staged (lib.rs:919)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq)]
#[ensures((*(^s).g).dm == (*s.g).dm && (*(^s).g).mt == (*s.g).mt && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(no_reg(*s.g, *(^s).g) && own_reads(*s.g, *(^s).g) && own_writes(*s.g, *(^s).g))]
#[ensures(match result { Ok(()) => s.p.ring && s.p.stg && total@ <= s.p.stg_bytes@ && s.p.nd@ > 0, Err(DE::KeyNotFound(_)) => false, Err(_) => true })]
pub fn serve_cold_staged(s: &mut S, key: u64, offset: u64, regions: &Vec<Ipc>, total: u32, device: i32, warm_raw: u64) -> Result<(), DE> {
    if !(s.p.ring && s.p.stg && (total as usize) <= s.p.stg_bytes) {
        return Err(DE::Alloc);
    }
    let lease = stg_checkout(s);
    if s.p.nd == 0 {
        return Err(DE::Io(IoK::Other));
    }
    let idx = drive_index(key, s.p.nd);
    if !chan_checkout(s, idx) {
        return Err(DE::Io(IoK::Channel));
    }
    match device_streams_for(s, device) {
        Some(_) => {}
        None => {
            if !s.p.ring {
                return Err(DE::NotInit(Ni::Ring));
            }
        }
    }
    let single = regions.len() == 1;
    let ok = if single {
        pipe_read_gpu(s, idx, key, lease, regions[0].addr, total as usize)
    } else {
        ssd_read_dram(s, idx, key)
    };
    if !ok {
        return Err(DE::Io(IoK::Ssd));
    }
    if regions.len() > 1 {
        let (r, _sv) = serve_memory_tier_to_gpu(s, lease, total, regions, warm_raw, true);
        return r;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Phase 3: the cold path (lib.rs:2472-2740)
// ---------------------------------------------------------------------------

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum CK {
    /// memory-tier-only mode (no data drives)
    NoDrive,
    /// got a slot, read through the pooled scatter-gather
    Pooled,
    /// tier saturated: served by the staging post-pass
    Staged,
    /// evict_and_insert failed otherwise
    PrepFail,
}

pub struct ColdG {
    pub kind: Seq<CK>,
    /// dm.promote_block_to_memory_tier succeeded for this cold entry
    pub prom: Seq<bool>,
    /// the no-drives cold path's per-entry record (empty seqs unless kind[q] == NoDrive)
    pub nd: NdG,
    /// nd record q belongs to entries@[nd_idx[q]] (= cold@[q].idx)
    pub nd_idx: Seq<Int>,
    /// batch index i -> entry i's cold entry was served by the pooled path at
    /// the read/promote level: its scatter-gather read completed AND its
    /// dispatch-map promotion landed (DIS-BATCH-LOOKUP-ERROR-ISOLATION: a
    /// read failure on a sibling entry must not turn this entry's result into
    /// an error). False for entries that are not cold, never reached the
    /// scatter-gather (no drives, staged, prep-failed), or whose drive's
    /// channel checkout failed.
    pub ok: Seq<bool>,
}

/// The no-drives cold path's per-entry record: whether the entry was promoted, and the log
/// positions of its slot reservation (MtInsert), its GPU delivery and its in-place
/// re-registration (DmPromote).
pub struct NdG {
    pub prom: Seq<bool>,
    /// log position of the entry's MtInsert (-1 when no slot was reserved)
    pub ins_at: Seq<Int>,
    /// the serve record of its slot -> GPU delivery
    pub sv: Seq<ServeG>,
    /// the slot the delivery read from (0 when none)
    pub ptr: Seq<u64>,
    /// log position of its DmPromote (-1 when not promoted)
    pub pr_at: Seq<Int>,
}

/// `h2d_scatter_ok` with the slot size as an `Int` (callers that only have `sum_sz`).
#[logic(open)]
pub fn h2d_scatter_ok_i(w: W, sv: ServeG, r: Seq<Ipc>, ptr: u64, size: Int) -> bool {
    pearlite! { 0 <= sv.base && 0 <= sv.m && sv.m <= r.len() && sv.base + sv.m <= w.log.len()
        && forall<l: Int> 0 <= l && l < sv.m ==>
            match w.log[sv.base + l] { Ev::H2D { src, dst, len, .. } =>
                src@ == ptr@ + pre(r, l) && dst == r[l].addr && len@ == chunk_of(r, l, size) && len@ > 0
                && ptr@ <= src@ && src@ + len@ <= ptr@ + size, _ => false } }
}

/// `scatter_ok_ext` for the Int-size variant.
#[logic]
#[requires(h2d_scatter_ok_i(a, sv, r, ptr, size))]
#[requires(a.log.len() <= b.log.len() && forall<q: Int> 0 <= q && q < a.log.len() ==> b.log[q] == a.log[q])]
#[ensures(h2d_scatter_ok_i(b, sv, r, ptr, size))]
pub fn scatter_ok_i_ext(a: W, b: W, sv: ServeG, r: Seq<Ipc>, ptr: u64, size: Int) {}

/// `nd_served` only reads the log at fixed positions, so it transfers to any world
/// whose log extends `a`'s log.
#[logic]
#[requires(nd_served(a, from, r, key, total, nd, q))]
#[requires(a.log.len() <= b.log.len() && forall<t: Int> 0 <= t && t < a.log.len() ==> b.log[t] == a.log[t])]
#[ensures(nd_served(b, from, r, key, total, nd, q))]
pub fn nd_served_ext(a: W, b: W, from: Int, r: Seq<Ipc>, key: u64, total: Int, nd: NdG, q: Int) {}

/// Cold entry q of the no-drives path was served and promoted, as log positions in `w`:
/// a slot was reserved for its key (MtInsert at ins_at[q]), the slot was scattered over the
/// entry's GPU regions (the H2D events of sv[q]), and only afterwards was the entry
/// re-registered in place (DmPromote at pr_at[q]) — the serve-then-promote order.
#[logic(open)]
pub fn nd_served(w: W, from: Int, r: Seq<Ipc>, key: u64, total: Int, nd: NdG, q: Int) -> bool {
    pearlite! {
        from <= nd.ins_at[q] && 0 <= nd.ins_at[q] && nd.ins_at[q] < w.log.len() && w.log[nd.ins_at[q]] == Ev::MtInsert { key }
        && nd.ins_at[q] < nd.sv[q].base
        && h2d_scatter_ok_i(w, nd.sv[q], r, nd.ptr[q], total)
        && nd.sv[q].base + nd.sv[q].m <= nd.pr_at[q]
        && 0 <= nd.pr_at[q] && nd.pr_at[q] < w.log.len() && w.log[nd.pr_at[q]] == Ev::DmPromote { key }
        && (nd.sv[q].m < r.len() ==> chunk_of(r, nd.sv[q].m, total) == 0)
    }
}

#[logic]
#[variant(n)]
pub fn count_true(s: Seq<bool>, n: Int) -> Int {
    pearlite! { if n <= 0 { 0 } else { count_true(s, n - 1) + if s[n - 1] { 1 } else { 0 } } }
}

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Job2 {
    pub d: usize,
    pub q: usize,
    pub ptr: u64,
}

/// `bl_cold_pooled`'s ghost outputs: per-cold-entry promotion flags (`prom`,
/// q-indexed) and the batch-indexed per-entry serve record (`ok`, indexed by
/// the entry's position in the batch — see `ColdG.ok`).
pub struct PgG {
    pub prom: Seq<bool>,
    pub ok: Seq<bool>,
}

/// Per-drive cold prep (lib.rs:2540-2598): evict_and_insert per entry, drive by drive.
#[requires(forall<q: Int> 0 <= q && q < cold@.len() ==> cold@[q].idx@ < results@.len())]
#[requires(s.p.nd@ > 0)]
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq)]
#[ensures((^results)@.len() == results@.len())]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && (forall<q: Int> 0 <= q && q < cold@.len() ==> cold@[q].idx@ != i) ==> (^results)@[i] == results@[i])]
#[ensures(rd_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).c.promo_mem == (*s.g).c.promo_mem && (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu)]
#[ensures(no_reg(*s.g, *(^s).g) && own_reads(*s.g, *(^s).g) && own_writes(*s.g, *(^s).g))]
#[ensures(result.2.len() == cold@.len())]
#[ensures(forall<q: Int> 0 <= q && q < cold@.len() ==> result.2[q] != CK::NoDrive && (result.2[q] == CK::Staged ==> s.p.ring && s.p.stg))]
#[ensures(forall<j: Int> 0 <= j && j < result.0@.len() ==> result.0@[j].q@ < cold@.len() && result.2[result.0@[j].q@] == CK::Pooled
    && result.0@[j].d@ < s.p.nd@)]
#[ensures(forall<j: Int, k: Int> 0 <= j && j < k && k < result.0@.len() ==> result.0@[j].d@ <= result.0@[k].d@ && result.0@[j].q != result.0@[k].q)]
#[ensures(forall<j: Int> 0 <= j && j < result.1@.len() ==> result.1@[j]@ < cold@.len() && result.2[result.1@[j]@] == CK::Staged)]
#[ensures(cold@.len() == 1 && s.p.ring && s.p.stg && (*s.g).used + cold@[0].total@ > (*s.g).cap && s.p.max_att@ == 0 ==> result.2[0] == CK::Staged)]
#[ensures(cold@.len() == 1 && all_from((*s.g).ob, (*s.g).tick) && (*s.g).used + cold@[0].total@ <= (*s.g).cap && !mt_has(*s.g, cold@[0].key) ==>
    result.0@.len() == 1 && result.0@[0].q@ == 0 && result.0@[0].d == di(cold@[0].key, s.p.nd) && (*(^s).g).dm == (*s.g).dm
    && (*(^s).g).mt.get(cold@[0].key) == Some(Slot { ptr: result.0@[0].ptr, size: cold@[0].total })
    && (*(^s).g).ob == (*s.g).ob && (*(^s).g).os == (*s.g).os && (*(^s).g).tick >= (*s.g).tick)]
#[ensures(no_new_kn(results@, (^results)@))]
pub fn bl_cold_prep(s: &mut S, cold: &Vec<ColdE>, results: &mut Vec<Res>) -> (Vec<Job2>, Vec<usize>, Snapshot<Seq<CK>>) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let r0 = snapshot!(results@);
    let n = s.p.nd;
    let nc = cold.len();
    let max_attempts = s.p.max_att;
    let staging_available = s.p.ring && s.p.stg;
    let mut jobs: Vec<Job2> = Vec::new();
    let mut staged: Vec<usize> = Vec::new();
    let mut kind: Snapshot<Seq<CK>> = snapshot!(Seq::create(nc@, |q: Int| CK::PrepFail));
    let mut done: Snapshot<Seq<bool>> = snapshot!(Seq::create(nc@, |q: Int| false));
    proof_assert!(nc@ >= 1 ==> di_lt(cold@[0].key, n) == ());
    let mut d: usize = 0;
    #[invariant(s.p == *p0 && d@ <= n@ && log_ext(*g0, *s.g) && same_env(*g0, *s.g) && s.g.wq == g0.wq && results@.len() == r0.len())]
    #[invariant(no_new_kn(*r0, results@))]
    #[invariant(forall<i: Int> 0 <= i && i < r0.len() && (forall<q: Int> 0 <= q && q < cold@.len() ==> cold@[q].idx@ != i) ==> results@[i] == r0[i])]
    #[invariant(rd_ctr(g0.c, s.g.c) && s.g.c.promo_mem == g0.c.promo_mem && s.g.c.promo_gpu == g0.c.promo_gpu)]
    #[invariant(no_reg(*g0, *s.g) && own_reads(*g0, *s.g) && own_writes(*g0, *s.g) && kind.len() == nc@ && done.len() == nc@)]
    #[invariant(forall<q: Int> 0 <= q && q < nc@ ==> kind[q] != CK::NoDrive && (kind[q] == CK::Staged ==> s.p.ring && s.p.stg))]
    #[invariant(forall<j: Int> 0 <= j && j < jobs@.len() ==> jobs@[j].q@ < nc@ && kind[jobs@[j].q@] == CK::Pooled && jobs@[j].d@ < d@ && done[jobs@[j].q@])]
    #[invariant(forall<j: Int, k: Int> 0 <= j && j < k && k < jobs@.len() ==> jobs@[j].d@ <= jobs@[k].d@ && jobs@[j].q != jobs@[k].q)]
    #[invariant(forall<j: Int> 0 <= j && j < staged@.len() ==> staged@[j]@ < nc@ && kind[staged@[j]@] == CK::Staged && done[staged@[j]@])]
    #[invariant(forall<q: Int> 0 <= q && q < nc@ && !done[q] ==> kind[q] == CK::PrepFail && di(cold@[q].key, n)@ >= d@)]
    #[invariant(forall<q: Int> 0 <= q && q < nc@ && done[q] ==> di(cold@[q].key, n)@ < d@)]
    #[invariant(nc@ == 1 && d@ <= di(cold@[0].key, n)@ ==> *s.g == *g0 && jobs@.len() == 0 && staged@.len() == 0 && !done[0])]
    #[invariant(nc@ == 1 && d@ > di(cold@[0].key, n)@ && s.p.ring && s.p.stg && g0.used + cold@[0].total@ > g0.cap && s.p.max_att@ == 0 ==> kind[0] == CK::Staged)]
    #[invariant(nc@ == 1 && d@ > di(cold@[0].key, n)@ && all_from(g0.ob, g0.tick) && g0.used + cold@[0].total@ <= g0.cap && !mt_has(*g0, cold@[0].key) ==>
        jobs@.len() == 1 && jobs@[0].q@ == 0 && jobs@[0].d == di(cold@[0].key, n) && s.g.dm == g0.dm
        && s.g.mt.get(cold@[0].key) == Some(Slot { ptr: jobs@[0].ptr, size: cold@[0].total }) && s.g.tick >= g0.tick)]
    while d < n {
        let mut q: usize = 0;
        #[invariant(s.p == *p0 && d@ < n@ && q@ <= nc@ && log_ext(*g0, *s.g) && same_env(*g0, *s.g) && s.g.wq == g0.wq && results@.len() == r0.len())]
        #[invariant(no_new_kn(*r0, results@))]
        #[invariant(forall<i: Int> 0 <= i && i < r0.len() && (forall<q2: Int> 0 <= q2 && q2 < cold@.len() ==> cold@[q2].idx@ != i) ==> results@[i] == r0[i])]
        #[invariant(rd_ctr(g0.c, s.g.c) && s.g.c.promo_mem == g0.c.promo_mem && s.g.c.promo_gpu == g0.c.promo_gpu)]
        #[invariant(no_reg(*g0, *s.g) && own_reads(*g0, *s.g) && own_writes(*g0, *s.g) && kind.len() == nc@ && done.len() == nc@)]
        #[invariant(forall<q2: Int> 0 <= q2 && q2 < nc@ ==> kind[q2] != CK::NoDrive && (kind[q2] == CK::Staged ==> s.p.ring && s.p.stg))]
        #[invariant(forall<j: Int> 0 <= j && j < jobs@.len() ==> jobs@[j].q@ < nc@ && kind[jobs@[j].q@] == CK::Pooled && jobs@[j].d@ <= d@ && done[jobs@[j].q@])]
        #[invariant(forall<j: Int, k: Int> 0 <= j && j < k && k < jobs@.len() ==> jobs@[j].d@ <= jobs@[k].d@ && jobs@[j].q != jobs@[k].q)]
        #[invariant(forall<j: Int> 0 <= j && j < staged@.len() ==> staged@[j]@ < nc@ && kind[staged@[j]@] == CK::Staged && done[staged@[j]@])]
        #[invariant(forall<q2: Int> 0 <= q2 && q2 < nc@ && !done[q2] ==> kind[q2] == CK::PrepFail && (di(cold@[q2].key, n)@ > d@ || (di(cold@[q2].key, n)@ == d@ && q2 >= q@)))]
        #[invariant(forall<q2: Int> 0 <= q2 && q2 < nc@ && done[q2] ==> di(cold@[q2].key, n)@ < d@ || (di(cold@[q2].key, n)@ == d@ && q2 < q@))]
        #[invariant(nc@ == 1 && (d@ < di(cold@[0].key, n)@ || (d@ == di(cold@[0].key, n)@ && q@ == 0)) ==> *s.g == *g0 && jobs@.len() == 0 && staged@.len() == 0 && !done[0])]
        #[invariant(nc@ == 1 && d@ >= di(cold@[0].key, n)@ && done[0] && s.p.ring && s.p.stg && g0.used + cold@[0].total@ > g0.cap && s.p.max_att@ == 0 ==> kind[0] == CK::Staged)]
        #[invariant(nc@ == 1 && d@ >= di(cold@[0].key, n)@ && done[0] && all_from(g0.ob, g0.tick) && g0.used + cold@[0].total@ <= g0.cap && !mt_has(*g0, cold@[0].key) ==>
            jobs@.len() == 1 && jobs@[0].q@ == 0 && jobs@[0].d == di(cold@[0].key, n) && s.g.dm == g0.dm
            && s.g.mt.get(cold@[0].key) == Some(Slot { ptr: jobs@[0].ptr, size: cold@[0].total }) && s.g.tick >= g0.tick)]
        #[invariant(nc@ == 1 && d@ == di(cold@[0].key, n)@ && q@ == 1 ==> done[0])]
        while q < nc {
            let e = cold[q];
            if drive_index(e.key, n) == d {
                let gq = snapshot!(*s.g);
                match evict_and_insert(s, e.key, e.total, max_attempts) {
                    Ok(mem_ptr) => {
                        jobs.push(Job2 { d, q, ptr: mem_ptr });
                        kind = snapshot!(kind.set(q@, CK::Pooled));
                    }
                    Err(DE::Alloc) if staging_available => {
                        staged.push(q);
                        kind = snapshot!(kind.set(q@, CK::Staged));
                    }
                    Err(err) => {
                        results[e.idx] = Some(Err(err));
                        kind = snapshot!(kind.set(q@, CK::PrepFail));
                    }
                }
                done = snapshot!(done.set(q@, true));
            }
            q += 1;
        }
        d += 1;
    }
    (jobs, staged, kind)
}

/// memory-tier-only cold path (lib.rs:2481-2508): reserve a slot, deliver it to the entry's
/// GPU destination, then re-register the entry in place — the same serve-then-promote
/// sequence the with-drives cold path runs.
#[requires(forall<q: Int> 0 <= q && q < cold@.len() ==> cold@[q].idx@ < results@.len() && cold@[q].idx@ < entries@.len())]
#[requires(forall<q: Int, r: Int> 0 <= q && q < r && r < cold@.len() ==> cold@[q].idx@ < cold@[r].idx@)]
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq)]
#[ensures((^results)@.len() == results@.len())]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && (forall<q: Int> 0 <= q && q < cold@.len() ==> cold@[q].idx@ != i) ==> (^results)@[i] == results@[i])]
#[ensures(result.prom.len() == cold@.len() && result.ins_at.len() == cold@.len() && result.sv.len() == cold@.len()
    && result.ptr.len() == cold@.len() && result.pr_at.len() == cold@.len())]
#[ensures((*(^s).g).c.promo_mem == (*s.g).c.promo_mem + count_true(result.prom, cold@.len()))]
#[ensures(rd_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu)]
#[ensures(no_reg(*s.g, *(^s).g) && own_reads(*s.g, *(^s).g) && own_writes(*s.g, *(^s).g) && no_ssd(*s.g, *(^s).g))]
#[ensures(no_new_kn(results@, (^results)@))]
#[ensures(forall<q: Int> 0 <= q && q < cold@.len() ==>
    (match (^results)@[cold@[q].idx@] { Some(Ok(())) => true, _ => false }) == result.prom[q]
    && (!result.prom[q] ==> result.pr_at[q] == -1)
    && (result.prom[q] ==> nd_served(*(^s).g, (*s.g).log.len(), entries@[cold@[q].idx@].regions@, cold@[q].key, cold@[q].total@, *result, q)))]
#[ensures(cold@.len() == 1 && all_from((*s.g).ob, (*s.g).tick) && all_from((*s.g).og, (*s.g).tick)
    && (*s.g).used + cold@[0].total@ <= (*s.g).cap && !mt_has(*s.g, cold@[0].key)
    && has(*s.g, cold@[0].key) && is_blk(ent(*s.g, cold@[0].key).loc) && cold@[0].total@ > 0
    ==> result.prom[0] && match (^results)@[cold@[0].idx@] { Some(Ok(())) => true, _ => false })]
pub fn bl_cold_nodrive(s: &mut S, cold: &Vec<ColdE>, results: &mut Vec<Res>, entries: &[Be]) -> Snapshot<NdG> {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let r0 = snapshot!(results@);
    let max_attempts = s.p.max_att;
    let mut prom: Snapshot<Seq<bool>> = snapshot!(Seq::empty());
    let mut ins: Snapshot<Seq<Int>> = snapshot!(Seq::empty());
    let mut svs: Snapshot<Seq<ServeG>> = snapshot!(Seq::empty());
    let mut ptrs: Snapshot<Seq<u64>> = snapshot!(Seq::empty());
    let mut prs: Snapshot<Seq<Int>> = snapshot!(Seq::empty());
    let mut q: usize = 0;
    #[invariant(s.p == *p0 && q@ <= cold@.len() && log_ext(*g0, *s.g) && same_env(*g0, *s.g) && s.g.wq == g0.wq && results@.len() == r0.len())]
    #[invariant(q@ == 0 ==> *s.g == *g0)]
    #[invariant(no_new_kn(*r0, results@))]
    #[invariant(forall<i: Int> 0 <= i && i < r0.len() && (forall<q2: Int> 0 <= q2 && q2 < cold@.len() ==> cold@[q2].idx@ != i) ==> results@[i] == r0[i])]
    #[invariant(prom.len() == q@ && ins.len() == q@ && svs.len() == q@ && ptrs.len() == q@ && prs.len() == q@
        && s.g.c.promo_mem == g0.c.promo_mem + count_true(*prom, q@))]
    #[invariant(rd_ctr(g0.c, s.g.c) && s.g.c.promo_gpu == g0.c.promo_gpu)]
    #[invariant(no_reg(*g0, *s.g) && own_reads(*g0, *s.g) && own_writes(*g0, *s.g) && no_ssd(*g0, *s.g))]
    #[invariant(forall<j: Int> 0 <= j && j < q@ ==>
        (match results@[cold@[j].idx@] { Some(Ok(())) => true, _ => false }) == prom[j]
        && (!prom[j] ==> prs[j] == -1)
        && (prom[j] ==> nd_served(*s.g, g0.log.len(), entries@[cold@[j].idx@].regions@, cold@[j].key, cold@[j].total@,
              NdG { prom: *prom, ins_at: *ins, sv: *svs, ptr: *ptrs, pr_at: *prs }, j)))]
    #[invariant(cold@.len() == 1 && all_from(g0.ob, g0.tick) && all_from(g0.og, g0.tick) && g0.used + cold@[0].total@ <= g0.cap
        && !mt_has(*g0, cold@[0].key) && has(*g0, cold@[0].key) && is_blk(ent(*g0, cold@[0].key).loc) && cold@[0].total@ > 0
        ==> (q@ >= 1 ==> prom[0]))]
    while q < cold.len() {
        let e = cold[q];
        let gi = snapshot!(*s.g);
        let _ = evict_for_space(s, e.total, max_attempts);
        proof_assert!(own_writes(*gi, *s.g) && own_reads(*gi, *s.g) && no_reg(*gi, *s.g) && no_ssd(*gi, *s.g));
        let mut promoted = false;
        let mut ins_q: Snapshot<Int> = snapshot!(-1);
        let mut sv_q: Snapshot<ServeG> = snapshot!(ServeG { base: 0, m: 0, gpu_ok: false });
        let mut ptr_q: Snapshot<u64> = snapshot!(0u64);
        let mut pr_q: Snapshot<Int> = snapshot!(-1);
        let mut gsve = snapshot!(*s.g);
        let res = match mt_insert(s, e.key, e.total) {
            Ok(mem_ptr) => {
                let ins_pos = snapshot!(s.g.log.len() - 1);
                proof_assert!(s.g.log[*ins_pos] == Ev::MtInsert { key: e.key });
                proof_assert!(g0.log.len() <= *ins_pos);
                ins_q = ins_pos;
                ptr_q = snapshot!(mem_ptr);
                // Deliver the slot to the entry's GPU destination (synchronous copies:
                // completed when the call returns).
                let (r, sv2) = serve_memory_tier_to_gpu(s, mem_ptr, e.total, &entries[e.idx].regions, 0u64, true);
                sv_q = sv2;
                gsve = snapshot!(*s.g);
                proof_assert!(own_writes(*gi, *gsve) && own_reads(*gi, *gsve) && no_reg(*gi, *gsve) && no_ssd(*gi, *gsve));
                match r {
                    Err(e2) => {
                        // The delivery failed: release the reserved slot.
                        let _ = mt_remove(s, e.key);
                        Err(e2)
                    }
                    Ok(()) => {
                        proof_assert!(s.g.log.len() == sv_q.base + sv_q.m);
                        let pr_pos = snapshot!(s.g.log.len());
                        // In-place BlockDevice->MemoryTier re-registration.
                        match dm_promote(s, e.key, mem_ptr, e.total) {
                            Ok(()) => {
                                proof_assert!(s.g.log.len() == *pr_pos + 1 && s.g.log[*pr_pos] == Ev::DmPromote { key: e.key });
                                pr_q = pr_pos;
                                rec_promotion_to_memory(s);
                                promoted = true;
                                Ok(())
                            }
                            Err(_) => {
                                let _ = mt_remove(s, e.key);
                                Err(DE::Io(IoK::Promote))
                            }
                        }
                    }
                }
            }
            Err(MtE::AlreadyExists) => Err(DE::AlreadyExists(e.key)),
            Err(_) => Err(DE::Alloc),
        };
        results[e.idx] = Some(res);
        proof_assert!(count_eq(prom.push_back(promoted), *prom, q@) == ());
        proof_assert!(count_true(prom.push_back(promoted), q@ + 1) == count_true(*prom, q@) + if promoted { 1 } else { 0 });
        proof_assert!(forall<t: Int> 0 <= t && t < gi.log.len() ==> s.g.log[t] == gi.log[t]);
        proof_assert!(forall<j: Int> 0 <= j && j < q@ ==>
            scatter_ok_i_ext(*gi, *s.g, svs[j], entries@[cold@[j].idx@].regions@, ptrs[j], cold@[j].total@) == ());
        proof_assert!(forall<t: Int> 0 <= t && t < gsve.log.len() ==> s.g.log[t] == gsve.log[t]);
        proof_assert!(scatter_ok_i_ext(*gsve, *s.g, *sv_q, entries@[e.idx@].regions@, *ptr_q, e.total@) == ());
        proof_assert!(own_writes(*gi, *s.g) && own_reads(*gi, *s.g) && no_reg(*gi, *s.g) && no_ssd(*gi, *s.g));
        prom = snapshot!(prom.push_back(promoted));
        ins = snapshot!(ins.push_back(*ins_q));
        svs = snapshot!(svs.push_back(*sv_q));
        ptrs = snapshot!(ptrs.push_back(*ptr_q));
        prs = snapshot!(prs.push_back(*pr_q));
        q += 1;
    }
    snapshot!(NdG { prom: *prom, ins_at: *ins, sv: *svs, ptr: *ptrs, pr_at: *prs })
}

/// Pooled cold jobs (lib.rs:2600-2718): streams, one channel checkout per active drive, the
/// multiplexed scatter-gather, then per-job promotion (+ multi-region scatter).
/// Error isolation (DIS-BATCH-LOOKUP-ERROR-ISOLATION, US-11 AS-4): the scatter-gather is
/// modelled per job (`sg_job`), so a read failure — including an NVMe read timeout, which
/// fails exactly the job whose segments did not complete — is reported for that entry only
/// (`Err(DE::Io(IoK::Ssd))` at its own batch index); `ok` records, per batch index, the
/// entries whose own read and promotion landed, and those entries' results are Ok.
#[requires(forall<q: Int> 0 <= q && q < cold@.len() ==> cold@[q].idx@ < results@.len() && cold@[q].idx@ < entries@.len())]
#[requires(forall<q: Int, r: Int> 0 <= q && q < r && r < cold@.len() ==> cold@[q].idx@ < cold@[r].idx@)]
#[requires(forall<j: Int> 0 <= j && j < jobs@.len() ==> jobs@[j].q@ < cold@.len())]
#[requires(forall<j: Int, k: Int> 0 <= j && j < k && k < jobs@.len() ==> jobs@[j].d@ <= jobs@[k].d@ && jobs@[j].q != jobs@[k].q)]
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq)]
#[ensures((^results)@.len() == results@.len())]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && (forall<j: Int> 0 <= j && j < jobs@.len() ==> cold@[jobs@[j].q@].idx@ != i) ==> (^results)@[i] == results@[i])]
#[ensures(result.prom.len() == cold@.len())]
#[ensures(result.ok.len() == results@.len())]
#[ensures(forall<q: Int> 0 <= q && q < cold@.len() && result.prom[q] ==> exists<j: Int> 0 <= j && j < jobs@.len() && jobs@[j].q@ == q)]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && result.ok[i] ==> exists<j: Int> 0 <= j && j < jobs@.len() && cold@[jobs@[j].q@].idx@ == i)]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && i < entries@.len() && result.ok[i] && entries@[i].regions@.len() == 1
    ==> (^results)@[i] == Some(Ok(())))]
#[ensures((*(^s).g).c.promo_mem == (*s.g).c.promo_mem + count_true(result.prom, cold@.len()))]
#[ensures(rd_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu)]
#[ensures(no_reg(*s.g, *(^s).g) && own_reads(*s.g, *(^s).g) && own_writes(*s.g, *(^s).g))]
#[ensures(jobs@.len() == 1 && has_streams && all_from((*s.g).ob, (*s.g).tick) && (forall<t: Int> !(*s.g).os[t]) ==>
    !result.prom[jobs@[0].q@] && (*(^s).g).dm == (*s.g).dm && (*(^s).g).mt == (*s.g).mt
    && match (^results)@[cold@[jobs@[0].q@].idx@] { Some(Err(_)) => true, _ => false })]
#[ensures(no_new_kn(results@, (^results)@))]
pub fn bl_cold_pooled(s: &mut S, cold: &Vec<ColdE>, jobs: &Vec<Job2>, results: &mut Vec<Res>, entries: &[Be], has_streams: bool, warm_raw: u64) -> Snapshot<PgG> {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let r0 = snapshot!(results@);
    let nc = cold.len();
    let none = snapshot!(Seq::create(nc@, |q: Int| false));
    let oknone = snapshot!(Seq::create(r0.len(), |i: Int| false));
    proof_assert!(forall<k: Int> 0 <= k && k <= nc@ ==> count_zero(*none, k) == ());
    if jobs.len() == 0 {
        return snapshot!(PgG { prom: *none, ok: *oknone });
    }
    // Streams: the device's pipeline pair, or two temporary streams.
    if !has_streams {
        let a = gpu_create_stream(s);
        let ok = match a {
            Some(_) => match gpu_create_stream(s) { Some(_) => true, None => false },
            None => false,
        };
        if !ok {
            let mut j: usize = 0;
            #[invariant(s.p == *p0 && j@ <= jobs@.len() && results@.len() == r0.len() && *s.g == W { tick: s.g.tick, ..*g0 })]
            #[invariant(no_new_kn(*r0, results@))]
            #[invariant(forall<i: Int> 0 <= i && i < r0.len() && (forall<k: Int> 0 <= k && k < jobs@.len() ==> cold@[jobs@[k].q@].idx@ != i) ==> results@[i] == r0[i])]
            while j < jobs.len() {
                results[cold[jobs[j].q].idx] = Some(Err(DE::Io(IoK::Stream)));
                j += 1;
            }
            return snapshot!(PgG { prom: *none, ok: *oknone });
        }
    }
    // One channel checkout per active drive (jobs are grouped by drive, in drive order).
    let g1 = snapshot!(*s.g);
    let mut j: usize = 0;
    let mut checkout_ok = true;
    #[invariant(s.p == *p0 && j@ <= jobs@.len() && results@.len() == r0.len() && *s.g == W { tick: s.g.tick, ..*g1 } && s.g.tick >= g1.tick)]
    #[invariant(no_new_kn(*r0, results@))]
    #[invariant(forall<i: Int> 0 <= i && i < r0.len() && (forall<k: Int> 0 <= k && k < jobs@.len() ==> cold@[jobs@[k].q@].idx@ != i) ==> results@[i] == r0[i])]
    #[invariant(all_from(g0.ob, g0.tick) && jobs@.len() == 1 ==> checkout_ok)]
    #[invariant(g1.ob == g0.ob && g1.tick >= g0.tick)]
    while j < jobs.len() {
        let d = jobs[j].d;
        if j == 0 || jobs[j - 1].d != d {
            if !chan_checkout(s, d) {
                let mut k: usize = j;
                #[invariant(s.p == *p0 && k@ <= jobs@.len() && results@.len() == r0.len() && *s.g == W { tick: s.g.tick, ..*g1 })]
                #[invariant(no_new_kn(*r0, results@))]
                #[invariant(forall<i: Int> 0 <= i && i < r0.len() && (forall<k2: Int> 0 <= k2 && k2 < jobs@.len() ==> cold@[jobs@[k2].q@].idx@ != i) ==> results@[i] == r0[i])]
                while k < jobs.len() && jobs[k].d == d {
                    results[cold[jobs[k].q].idx] = Some(Err(DE::Io(IoK::Channel)));
                    k += 1;
                }
                checkout_ok = false;
                break;
            }
        }
        j += 1;
    }
    if !checkout_ok {
        return snapshot!(PgG { prom: *none, ok: *oknone });
    }
    // The scatter-gather: every job's NVMe reads (+ coalesced H2D for single-region objects).
    let g2 = snapshot!(*s.g);
    let mut oks: Vec<bool> = Vec::new();
    let mut j: usize = 0;
    #[invariant(s.p == *p0 && j@ <= jobs@.len() && oks@.len() == j@ && results@.len() == r0.len() && results@ == *r0)]
    #[invariant(no_new_kn(*r0, results@))]
    #[invariant(log_ext(*g2, *s.g) && same_env(*g2, *s.g) && s.g.dm == g2.dm && s.g.mt == g2.mt && s.g.wq == g2.wq && s.g.c == g2.c && s.g.used == g2.used)]
    #[invariant(no_reg(*g2, *s.g) && own_reads(*g2, *s.g) && own_writes(*g2, *s.g))]
    #[invariant((forall<t: Int> !g0.os[t]) ==> forall<k: Int> 0 <= k && k < j@ ==> !oks@[k])]
    while j < jobs.len() {
        let job = jobs[j];
        let e = cold[job.q];
        let regions = &entries[e.idx].regions;
        let dst = if regions.len() == 1 { regions[0].addr } else { 0 };
        let ok = sg_job(s, job.d, e.key, job.ptr, dst, e.total as usize);
        oks.push(ok);
        j += 1;
    }
    // Process results: promote and scatter.
    let g3 = snapshot!(*s.g);
    let mut prom: Snapshot<Seq<bool>> = none;
    let mut okv: Snapshot<Seq<bool>> = oknone;
    let mut j: usize = 0;
    #[invariant(s.p == *p0 && j@ <= jobs@.len() && results@.len() == r0.len() && prom.len() == nc@ && okv.len() == r0.len())]
    #[invariant(no_new_kn(*r0, results@))]
    #[invariant(log_ext(*g0, *s.g) && same_env(*g0, *s.g) && s.g.wq == g0.wq)]
    #[invariant(forall<i: Int> 0 <= i && i < r0.len() && (forall<k: Int> 0 <= k && k < jobs@.len() ==> cold@[jobs@[k].q@].idx@ != i) ==> results@[i] == r0[i])]
    #[invariant(forall<q: Int> 0 <= q && q < nc@ && prom[q] ==> exists<k: Int> 0 <= k && k < j@ && jobs@[k].q@ == q)]
    #[invariant(forall<k: Int> j@ <= k && k < jobs@.len() ==> !prom[jobs@[k].q@] && !okv[cold@[jobs@[k].q@].idx@])]
    #[invariant(forall<i: Int> 0 <= i && i < r0.len() && okv[i] ==> exists<k: Int> 0 <= k && k < j@ && cold@[jobs@[k].q@].idx@ == i)]
    #[invariant(forall<i: Int> 0 <= i && i < r0.len() && i < entries@.len() && okv[i] && entries@[i].regions@.len() == 1 ==> results@[i] == Some(Ok(())))]
    #[invariant(s.g.c.promo_mem == g0.c.promo_mem + count_true(*prom, nc@))]
    #[invariant(rd_ctr(g0.c, s.g.c) && s.g.c.promo_gpu == g0.c.promo_gpu)]
    #[invariant(no_reg(*g0, *s.g) && own_reads(*g0, *s.g) && own_writes(*g0, *s.g))]
    #[invariant((forall<t: Int> !g0.os[t]) ==> *prom == *none && s.g.dm == g3.dm && s.g.mt == g3.mt
        && forall<k: Int> 0 <= k && k < j@ ==> match results@[cold@[jobs@[k].q@].idx@] { Some(Err(_)) => true, _ => false })]
    while j < jobs.len() {
        let job = jobs[j];
        let e = cold[job.q];
        let res = if oks[j] {
            match dm_promote(s, e.key, job.ptr, e.total) {
                Ok(()) => {
                    rec_promotion_to_memory(s);
                    proof_assert!(!prom[job.q@]);
                    proof_assert!(count_set(*prom, job.q@, nc@) == ());
                    prom = snapshot!(prom.set(job.q@, true));
                    okv = snapshot!(okv.set(e.idx@, true));
                    if entries[e.idx].regions.len() > 1 {
                        let (r, _sv) = serve_memory_tier_to_gpu(s, job.ptr, e.total, &entries[e.idx].regions, warm_raw, true);
                        r
                    } else {
                        Ok(())
                    }
                }
                Err(_) => Err(DE::Io(IoK::Promote)),
            }
        } else {
            Err(DE::Io(IoK::Ssd))
        };
        results[e.idx] = Some(res);
        j += 1;
    }
    snapshot!(PgG { prom: *prom, ok: *okv })
}

#[logic]
#[requires(forall<i: Int> 0 <= i && i < s.len() ==> !s[i])]
#[requires(0 <= n && n <= s.len())]
#[ensures(count_true(s, n) == 0)]
#[variant(n)]
pub fn count_zero(s: Seq<bool>, n: Int) {
    pearlite! { if n <= 0 { () } else { count_zero(s, n - 1) } }
}

#[logic]
#[requires(0 <= q && q < n && n <= s.len() && !s[q])]
#[ensures(count_true(s.set(q, true), n) == count_true(s, n) + 1)]
#[variant(n)]
pub fn count_set(s: Seq<bool>, q: Int, n: Int) {
    pearlite! { if n <= q + 1 { count_eq(s, s.set(q, true), q) } else { count_set(s, q, n - 1) } }
}

#[logic]
#[requires(0 <= n && n <= a.len() && n <= b.len() && forall<i: Int> 0 <= i && i < n ==> a[i] == b[i])]
#[ensures(count_true(a, n) == count_true(b, n))]
#[variant(n)]
pub fn count_eq(a: Seq<bool>, b: Seq<bool>, n: Int) {
    pearlite! { if n <= 0 { () } else { count_eq(a, b, n - 1) } }
}

/// The cold path of batch_lookup. `cold` is the classification's cold list (indices < n).
#[requires(forall<q: Int> 0 <= q && q < cold@.len() ==> cold@[q].idx@ < results@.len() && cold@[q].idx@ < entries@.len())]
#[requires(forall<q: Int, r: Int> 0 <= q && q < r && r < cold@.len() ==> cold@[q].idx@ < cold@[r].idx@)]
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq)]
#[ensures((^results)@.len() == results@.len())]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && (forall<q: Int> 0 <= q && q < cold@.len() ==> cold@[q].idx@ != i) ==> (^results)@[i] == results@[i])]
#[ensures(result.kind.len() == cold@.len() && result.prom.len() == cold@.len())]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() ==> (is_kn((^results)@[i]) ==> is_kn(results@[i])) && ((^results)@[i] == None ==> results@[i] == None))]
#[ensures((*(^s).g).c.promo_mem == (*s.g).c.promo_mem + count_true(result.prom, cold@.len()))]
#[ensures(mono_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu && (*(^s).g).c.r_hit == (*s.g).c.r_hit
    && (*(^s).g).c.r_miss == (*s.g).c.r_miss && (*(^s).g).c.hit_dram == (*s.g).c.hit_dram && (*(^s).g).c.hit_ssd == (*s.g).c.hit_ssd)]
#[ensures(no_reg(*s.g, *(^s).g) && own_reads(*s.g, *(^s).g) && own_writes(*s.g, *(^s).g))]
#[ensures(forall<q: Int> 0 <= q && q < cold@.len() ==>
    (s.p.nd@ == 0) == (result.kind[q] == CK::NoDrive)
    && (result.kind[q] == CK::Staged ==> s.p.nd@ > 0 && s.p.ring && s.p.stg)
    && (result.prom[q] ==> result.kind[q] == CK::Pooled || result.kind[q] == CK::NoDrive))]
#[ensures(s.p.nd@ == 0 ==> result.nd.prom.len() == cold@.len() && result.nd.ins_at.len() == cold@.len() && result.nd.sv.len() == cold@.len()
    && result.nd.ptr.len() == cold@.len() && result.nd.pr_at.len() == cold@.len() && result.nd_idx.len() == cold@.len())]
#[ensures(s.p.nd@ == 0 ==> forall<q: Int> 0 <= q && q < cold@.len() ==> result.nd_idx[q] == cold@[q].idx@
    && (match (^results)@[cold@[q].idx@] { Some(Ok(())) => true, _ => false }) == result.nd.prom[q]
    && (!result.nd.prom[q] ==> result.nd.pr_at[q] == -1)
    && (result.nd.prom[q] ==> nd_served(*(^s).g, (*s.g).log.len(), entries@[cold@[q].idx@].regions@, cold@[q].key, cold@[q].total@, result.nd, q)))]
#[ensures(s.p.nd@ == 0 && cold@.len() == 1 && all_from((*s.g).ob, (*s.g).tick) && all_from((*s.g).og, (*s.g).tick)
    && (*s.g).used + cold@[0].total@ <= (*s.g).cap && !mt_has(*s.g, cold@[0].key)
    && has(*s.g, cold@[0].key) && is_blk(ent(*s.g, cold@[0].key).loc) && cold@[0].total@ > 0
    ==> result.nd.prom[0] && match (^results)@[cold@[0].idx@] { Some(Ok(())) => true, _ => false })]
#[ensures(cold@.len() == 1 && s.p.nd@ > 0 && s.p.ring && s.p.stg && (*s.g).used + cold@[0].total@ > (*s.g).cap && s.p.max_att@ == 0
    ==> result.kind[0] == CK::Staged)]
#[ensures(cold@.len() == 1 && s.p.nd@ > 0 && has_streams && all_from((*s.g).ob, (*s.g).tick) && (forall<t: Int> !(*s.g).os[t])
    && (*s.g).used + cold@[0].total@ <= (*s.g).cap && !mt_has(*s.g, cold@[0].key)
    ==> result.kind[0] == CK::Pooled && !result.prom[0] && mt_has(*(^s).g, cold@[0].key)
        && (*(^s).g).dm.get(cold@[0].key) == (*s.g).dm.get(cold@[0].key)
        && match (^results)@[cold@[0].idx@] { Some(Err(_)) => true, _ => false })]
#[ensures(result.ok.len() == results@.len())]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && i < entries@.len() && result.ok[i] && entries@[i].regions@.len() == 1
    ==> (^results)@[i] == Some(Ok(())))]
pub fn bl_cold(s: &mut S, cold: &Vec<ColdE>, results: &mut Vec<Res>, entries: &[Be], has_streams: bool, batch_device: i32, warm_raw: u64) -> Snapshot<ColdG> {
    let mut ndidx: Snapshot<Seq<Int>> = snapshot!(Seq::empty());
    let mut q0: usize = 0;
    #[invariant(q0@ <= cold@.len() && ndidx.len() == q0@ && forall<j: Int> 0 <= j && j < q0@ ==> ndidx[j] == cold@[j].idx@)]
    while q0 < cold.len() {
        ndidx = snapshot!(ndidx.push_back(cold[q0].idx@));
        q0 += 1;
    }
    if s.p.nd == 0 {
        let ndg = bl_cold_nodrive(s, cold, results, entries);
        let promv = snapshot!(ndg.prom);
        let okv = snapshot!(Seq::create(results@.len(), |i: Int| false));
        return snapshot!(ColdG { kind: Seq::create(cold@.len(), |q: Int| CK::NoDrive), prom: *promv, nd: *ndg, nd_idx: *ndidx, ok: *okv });
    }
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let r0 = snapshot!(results@);
    let (jobs, staged, kind) = bl_cold_prep(s, cold, results);
    let g1 = snapshot!(*s.g);
    proof_assert!(all_from(g0.ob, g0.tick) && g1.ob == g0.ob && g1.tick >= g0.tick ==> all_from(g1.ob, g1.tick));
    proof_assert!(forall<j: Int> 0 <= j && j < jobs@.len() ==> cold@[jobs@[j].q@].idx@ < r0.len());
    let pg = bl_cold_pooled(s, cold, &jobs, results, entries, has_streams, warm_raw);
    let g2 = snapshot!(*s.g);
    let r2 = snapshot!(results@);
    // A staged entry never reached the scatter-gather, so its `ok` is false and
    // the staging post-pass below may freely rewrite its result.
    proof_assert!(forall<t: Int> 0 <= t && t < staged@.len() ==> !pg.ok[cold@[staged@[t]@].idx@]);
    // Staging post-pass (channel leases released).
    let mut j: usize = 0;
    #[invariant(s.p == *p0 && j@ <= staged@.len() && results@.len() == r0.len())]
    #[invariant(no_new_kn(*r0, results@))]
    #[invariant(log_ext(*g2, *s.g) && same_env(*g2, *s.g) && s.g.wq == g2.wq && s.g.dm == g2.dm && s.g.mt == g2.mt && same_ctr(g2.c, s.g.c))]
    #[invariant(no_reg(*g2, *s.g) && own_reads(*g2, *s.g) && own_writes(*g2, *s.g))]
    #[invariant(forall<i: Int> 0 <= i && i < r0.len() && (forall<q: Int> 0 <= q && q < staged@.len() ==> cold@[staged@[q]@].idx@ != i) ==> results@[i] == r2[i])]
    while j < staged.len() {
        let e = cold[staged[j]];
        let r = serve_cold_staged(s, e.key, e.off, &entries[e.idx].regions, e.total, batch_device, warm_raw);
        results[e.idx] = Some(r);
        j += 1;
    }
    let promv = snapshot!(pg.prom);
    let okv = snapshot!(pg.ok);
    snapshot!(ColdG { kind: *kind, prom: *promv, nd: NdG { prom: Seq::empty(), ins_at: Seq::empty(), sv: Seq::empty(), ptr: Seq::empty(), pr_at: Seq::empty() }, nd_idx: *ndidx, ok: *okv })
}


// ---------------------------------------------------------------------------
// serve_concurrently_promoted (lib.rs:1349)
// ---------------------------------------------------------------------------

/// What the recovery saw: the entry became memory-tier resident (and was served), or not.
#[ensures((^s).p == s.p)]
#[ensures(match result.0 { Err(DE::AlreadyExists(_)) => false, _ => true })]
#[ensures(result.0 == Ok(()) ==> result.1)]
#[ensures(match result.0 { Err(DE::InvalidParam) => true, Err(DE::KeyNotFound(k)) => k == key, Err(DE::Io(_)) => true, Ok(()) => true, _ => false })]
#[ensures(log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && same_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).wq == (*s.g).wq)]
pub fn serve_concurrently_promoted(s: &mut S, key: u64, regions: &Vec<Ipc>, warm_raw: u64) -> (Result<(), DE>, bool) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    #[invariant(s.p == *p0 && log_ext(*g0, *s.g) && same_env(*g0, *s.g) && same_ctr(g0.c, s.g.c) && s.g.wq == g0.wq)]
    loop {
        match dm_lookup_racing(s, key) {
            Ok(Lr::Mem { ptr, size }) => {
                let (res, _sv) = serve_memory_tier_to_gpu(s, ptr, size, regions, warm_raw, true);
                let _ = dm_release_read(s, key, snapshot!(1));
                let _ = dm_touch(s, key);
                return (res, true);
            }
            Ok(Lr::Blk { .. }) => {
                let _ = dm_release_read(s, key, snapshot!(1));
                if timed_out_5s(s) {
                    return (Err(DE::KeyNotFound(key)), false);
                }
                // std::thread::sleep(50 us)
            }
            Ok(Lr::Mismatch) => {
                let _ = dm_release_read(s, key, snapshot!(1));
                return (Err(DE::InvalidParam), false);
            }
            Ok(Lr::NotExist) => return (Err(DE::KeyNotFound(key)), false),
            Err(_) => return (Err(DE::Io(IoK::Dm)), false),
        }
    }
}

// ---------------------------------------------------------------------------
// Attribution (lib.rs:2952-2977)
// ---------------------------------------------------------------------------

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Outcome {
    pub served_by: ServedBy,
    pub result: Result<(), DE>,
}

#[logic(open)]
pub fn attr(r: Result<(), DE>, t: Option<ServedBy>) -> ServedBy {
    pearlite! { match r {
        Ok(()) => match t { Some(x) => x, None => ServedBy::Error },
        Err(DE::KeyNotFound(_)) => ServedBy::Miss,
        Err(DE::InvalidParam) => ServedBy::SizeMismatch,
        Err(_) => ServedBy::Error,
    } }
}

#[requires(results@.len() == tier@.len())]
#[ensures(result@.len() == results@.len())]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() ==> results@[i] == Some(result@[i].result)
    && result@[i].served_by == attr(result@[i].result, tier@[i]))]
pub fn attribute(results: Vec<Res>, tier: Vec<Option<ServedBy>>) -> Vec<Outcome> {
    let mut out: Vec<Outcome> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= results@.len() && out@.len() == i@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> results@[j] == Some(out@[j].result) && out@[j].served_by == attr(out@[j].result, tier@[j]))]
    while i < results.len() {
        let result = unwrap_or_panic(results[i]);
        let served_by = match result {
            Ok(()) => match tier[i] {
                Some(t) => t,
                // debug_assert!(false, "served a key with no recorded tier")
                None => ServedBy::Error,
            },
            Err(DE::KeyNotFound(_)) => ServedBy::Miss,
            Err(DE::InvalidParam) => ServedBy::SizeMismatch,
            Err(_) => ServedBy::Error,
        };
        out.push(Outcome { served_by, result });
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Phase 5: remote lookup for entries not found locally (lib.rs:2756-2935)
// ---------------------------------------------------------------------------

pub struct RemG {
    /// the forwarded positions, in order
    pub nf: Seq<usize>,
    pub ans: Seq<Rl>,
    /// position of the remote-delivery sync (-1 if none)
    pub sync_at: Int,
    /// log length when the remote pins were released
    pub rel_from: Int,
}

#[logic]
#[variant(n)]
pub fn count_rl(a: Seq<Rl>, n: Int, x: Rl) -> Int {
    pearlite! { if n <= 0 { 0 } else { count_rl(a, n - 1, x) + if a[n - 1] == x { 1 } else { 0 } } }
}

#[requires(results@.len() == entries@.len() && tier@.len() == entries@.len() && a_fr039(entries@))]
#[requires(forall<i: Int> 0 <= i && i < results@.len() && is_kn(results@[i]) ==> tier@[i] == None)]
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq)]
#[ensures((^results)@.len() == results@.len() && (^tier)@.len() == tier@.len())]
#[ensures(!s.p.has_rl ==> (^results)@ == results@ && (^tier)@ == tier@ && *(^s).g == *s.g)]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && !is_kn(results@[i]) ==> (^results)@[i] == results@[i] && (^tier)@[i] == tier@[i])]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() ==> (^tier)@[i] == tier@[i] || ((^tier)@[i] == Some(ServedBy::Remote) && is_kn(results@[i])))]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() && (^results)@[i] == Some(Ok(())) ==> results@[i] == Some(Ok(())) || (^tier)@[i] == Some(ServedBy::Remote))]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() ==> match (^results)@[i] { Some(Err(DE::AlreadyExists(_))) => results@[i] == (^results)@[i], _ => true })]
#[ensures(forall<i: Int> 0 <= i && i < results@.len() ==> (^results)@[i] != None || results@[i] == None)]
#[ensures(result.nf.len() == result.ans.len())]
#[ensures(forall<t: Int> 0 <= t && t < result.nf.len() ==> result.nf[t]@ < results@.len() && is_kn(results@[result.nf[t]@])
    && fwd_logged(*s.g, *(^s).g, entries@[result.nf[t]@].key, sum_sz(entries@[result.nf[t]@].regions@))
    && (result.ans[t] == Rl::NotFound ==> (^results)@[result.nf[t]@] == Some(Err(DE::KeyNotFound(entries@[result.nf[t]@].key))))
    && (result.ans[t] == Rl::Transport ==> (^results)@[result.nf[t]@] == Some(Err(DE::Io(IoK::Remote))))
    && ((^tier)@[result.nf[t]@] == Some(ServedBy::Remote) ==> result.ans[t] == Rl::Ok))]
#[ensures((*(^s).g).c.r_hit == (*s.g).c.r_hit + count_rl(result.ans, result.ans.len(), Rl::Ok)
    && (*(^s).g).c.r_miss == (*s.g).c.r_miss + count_rl(result.ans, result.ans.len(), Rl::NotFound)
    && (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu && (*(^s).g).c.promo_mem == (*s.g).c.promo_mem
    && (*(^s).g).c.hit_dram == (*s.g).c.hit_dram && (*(^s).g).c.hit_ssd == (*s.g).c.hit_ssd
    && (*(^s).g).c.bp == (*s.g).c.bp && (*(^s).g).c.drop_full == (*s.g).c.drop_full && (*(^s).g).c.already == (*s.g).c.already
    && (*(^s).g).c.ev_mem == (*s.g).c.ev_mem && (*(^s).g).c.ev_ssd == (*s.g).c.ev_ssd && (*(^s).g).c.blk_pin == (*s.g).c.blk_pin
    && (*(^s).g).c.exhausted == (*s.g).c.exhausted && (*(^s).g).c.blk_unp == (*s.g).c.blk_unp)]
#[ensures(result.sync_at >= 0 ==> (*s.g).log.len() <= result.sync_at && result.sync_at < result.rel_from && (*(^s).g).log[result.sync_at] == Ev::Sync { stream: warm_raw })]
#[ensures(forall<i: Int> (*s.g).log.len() <= i && i < (*(^s).g).log.len() ==> match (*(^s).g).log[i] {
    Ev::RelRead { own, .. } => own > 0, Ev::RelWrite { .. } => false, Ev::DmCreate { .. } => false, Ev::Enq { .. } => false, _ => true })]
pub fn bl_remote(s: &mut S, entries: &[Be], results: &mut Vec<Res>, tier: &mut Vec<Option<ServedBy>>, warm_raw: u64) -> Snapshot<RemG> {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let r0 = snapshot!(results@);
    let t0 = snapshot!(tier@);
    let empty = snapshot!(RemG { nf: Seq::empty(), ans: Seq::empty(), sync_at: -1, rel_from: 0 });
    if !s.p.has_rl {
        return empty;
    }
    let n = results.len();
    // not_found: positions whose result is KeyNotFound
    let mut not_found: Vec<usize> = Vec::new();
    let mut fpos: Snapshot<Mapping<Int, Int>> = snapshot!(|i: Int| 0);
    let mut i: usize = 0;
    #[invariant(i@ <= n@ && forall<t: Int> 0 <= t && t < not_found@.len() ==> not_found@[t]@ < i@ && is_kn(r0[not_found@[t]@]))]
    #[invariant(forall<t: Int, u: Int> 0 <= t && t < u && u < not_found@.len() ==> not_found@[t]@ < not_found@[u]@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && is_kn(r0[j]) ==> 0 <= fpos[j] && fpos[j] < not_found@.len() && not_found@[fpos[j]]@ == j)]
    while i < n {
        if match results[i] { Some(Err(DE::KeyNotFound(_))) => true, _ => false } {
            fpos = snapshot!(fpos.set(i@, not_found@.len()));
            not_found.push(i);
        }
        i += 1;
    }
    if not_found.len() == 0 {
        return empty;
    }
    // (key, Σ region sizes) per forwarded position
    let mut fwd: Vec<(u64, u32)> = Vec::new();
    let mut t: usize = 0;
    #[invariant(t@ <= not_found@.len() && fwd@.len() == t@)]
    #[invariant(forall<u: Int> 0 <= u && u < t@ ==> fwd@[u].0 == entries@[not_found@[u]@].key && fwd@[u].1@ == sum_sz(entries@[not_found@[u]@].regions@))]
    while t < not_found.len() {
        let pos = not_found[t];
        let tot = total_u32(&entries[pos].regions);
        fwd.push((entries[pos].key, tot));
        t += 1;
    }
    let gf = snapshot!(*s.g);
    let remote_results = rl_batch_lookup(s, &fwd);
    let g1 = snapshot!(*s.g);
    proof_assert!(forall<u: Int> 0 <= u && u < not_found@.len() ==> g1.log[gf.log.len() + u] == Ev::RemoteFwd { key: fwd@[u].0, size: fwd@[u].1 });
    proof_assert!(gf.log == g0.log);
    proof_assert!(g1.log.len() == gf.log.len() + fwd@.len());
    proof_assert!(forall<q: Int> gf.log.len() <= q && q < g1.log.len() ==> g1.log[q] == Ev::RemoteFwd { key: fwd@[q - gf.log.len()].0, size: fwd@[q - gf.log.len()].1 });
    proof_assert!(forall<q: Int> g0.log.len() <= q && q < g1.log.len() ==> match g1.log[q] { Ev::RemoteFwd { .. } => true, _ => false });
    let mut remote_pins: Vec<u64> = Vec::new();
    let mut submitted: Vec<usize> = Vec::new();
    let mut hits: u64 = 0;
    let mut misses: u64 = 0;
    let mut t: usize = 0;
    #[invariant(s.p == *p0 && t@ <= not_found@.len() && results@.len() == r0.len() && tier@.len() == t0.len())]
    #[invariant(log_ext(*g1, *s.g) && same_env(*g0, *s.g) && s.g.wq == g0.wq)]
    #[invariant(forall<q: Int> 0 <= q && q < g1.log.len() ==> s.g.log[q] == g1.log[q])]
    #[invariant(hits@ == count_rl(remote_results@, t@, Rl::Ok) && misses@ == count_rl(remote_results@, t@, Rl::NotFound) && hits@ <= t@ && misses@ <= t@)]
    #[invariant(s.g.c == g0.c)]
    #[invariant(forall<j: Int> 0 <= j && j < r0.len() && !is_kn(r0[j]) ==> results@[j] == r0[j] && tier@[j] == t0[j])]
    #[invariant(forall<u: Int> t@ <= u && u < not_found@.len() ==> results@[not_found@[u]@] == r0[not_found@[u]@] && tier@[not_found@[u]@] == t0[not_found@[u]@])]
    #[invariant(forall<j: Int> 0 <= j && j < r0.len() ==> tier@[j] == t0[j] || (tier@[j] == Some(ServedBy::Remote) && is_kn(r0[j])))]
    #[invariant(forall<j: Int> 0 <= j && j < r0.len() && results@[j] == Some(Ok(())) ==> r0[j] == Some(Ok(())) || tier@[j] == Some(ServedBy::Remote))]
    #[invariant(forall<j: Int> 0 <= j && j < r0.len() ==> match results@[j] { Some(Err(DE::AlreadyExists(_))) => r0[j] == results@[j], _ => true })]
    #[invariant(forall<j: Int> 0 <= j && j < r0.len() ==> results@[j] != None || r0[j] == None)]
    #[invariant(forall<u: Int> 0 <= u && u < t@ ==>
        (remote_results@[u] == Rl::NotFound ==> results@[not_found@[u]@] == Some(Err(DE::KeyNotFound(entries@[not_found@[u]@].key))))
        && (remote_results@[u] == Rl::Transport ==> results@[not_found@[u]@] == Some(Err(DE::Io(IoK::Remote))))
        && (tier@[not_found@[u]@] == Some(ServedBy::Remote) ==> remote_results@[u] == Rl::Ok))]
    #[invariant(forall<q: Int> g0.log.len() <= q && q < s.g.log.len() ==> match s.g.log[q] {
        Ev::RelRead { own, .. } => own > 0, Ev::RelWrite { .. } => false, Ev::DmCreate { .. } => false, Ev::Enq { .. } => false, _ => true })]
    #[invariant(forall<u: Int> 0 <= u && u < submitted@.len() ==> submitted@[u]@ < r0.len() && tier@[submitted@[u]@] == Some(ServedBy::Remote) && is_kn(r0[submitted@[u]@]))]
    while t < not_found.len() {
        let pos = not_found[t];
        let key = entries[pos].key;
        proof_assert!(forall<u: Int> 0 <= u && u < not_found@.len() && u != t@ ==> not_found@[u] != pos);
        proof_assert!(forall<x: Rl> count_rl(remote_results@, t@ + 1, x) == count_rl(remote_results@, t@, x) + if remote_results@[t@] == x { 1 } else { 0 });
        match remote_results[t] {
            Rl::NotFound => {
                misses += 1;
                results[pos] = Some(Err(DE::KeyNotFound(key)));
                t += 1;
                continue;
            }
            Rl::Transport => {
                results[pos] = Some(Err(DE::Io(IoK::Remote)));
                t += 1;
                continue;
            }
            Rl::Ok => {}
        }
        hits += 1;
        match dm_lookup(s, key) {
            Ok(Lr::Mem { ptr, size }) => {
                remote_pins.push(key);
                let (res, _sv) = serve_memory_tier_to_gpu(s, ptr, size, &entries[pos].regions, warm_raw, false);
                match res {
                    Ok(()) => submitted.push(pos),
                    Err(_) => {}
                }
                tier[pos] = Some(ServedBy::Remote);
                results[pos] = Some(res);
            }
            Ok(Lr::Blk { .. }) => {
                let _ = dm_release_read(s, key, snapshot!(1));
                results[pos] = Some(Err(DE::KeyNotFound(key)));
            }
            Ok(_) => {
                results[pos] = Some(Err(DE::KeyNotFound(key)));
            }
            Err(_) => {
                results[pos] = Some(Err(DE::Io(IoK::Dm)));
            }
        }
        t += 1;
    }
    proof_assert!(forall<u: Int> 0 <= u && u < not_found@.len() ==> (tier@[not_found@[u]@] == Some(ServedBy::Remote) ==> remote_results@[u] == Rl::Ok));
    let tier_end = snapshot!(tier@);
    let mut sync_at: Snapshot<Int> = snapshot!(-1);
    if submitted.len() > 0 && warm_raw != 0 {
        sync_at = snapshot!(s.g.log.len());
        if !gpu_sync(s, warm_raw) {
            let mut u: usize = 0;
            let r1 = snapshot!(results@);
            #[invariant(u@ <= submitted@.len() && results@.len() == r1.len() && tier@ == *tier_end)]
            #[invariant(forall<j: Int> 0 <= j && j < r1.len() ==> results@[j] == r1[j] || (results@[j] == Some(Err(DE::Io(IoK::Sync))) && tier@[j] == Some(ServedBy::Remote) && is_kn(r0[j])))]
            while u < submitted.len() {
                results[submitted[u]] = Some(Err(DE::Io(IoK::Sync)));
                u += 1;
            }
        }
    }
    let rel_from = snapshot!(s.g.log.len());
    // drop(remote_pins): every remote copy has completed.
    let mut u: usize = 0;
    let g2 = snapshot!(*s.g);
    #[invariant(tier@ == *tier_end)]
    #[invariant(s.p == *p0 && u@ <= remote_pins@.len() && log_ext(*g2, *s.g) && same_env(*g2, *s.g) && s.g.wq == g2.wq && s.g.c == g2.c)]
    #[invariant(forall<q: Int> 0 <= q && q < g2.log.len() ==> s.g.log[q] == g2.log[q])]
    #[invariant(forall<q: Int> g2.log.len() <= q && q < s.g.log.len() ==> match s.g.log[q] { Ev::RelRead { own, .. } => own > 0, _ => false })]
    #[invariant(forall<q: Int> g0.log.len() <= q && q < g2.log.len() ==> match g2.log[q] {
        Ev::RelRead { own, .. } => own > 0, Ev::RelWrite { .. } => false, Ev::DmCreate { .. } => false, Ev::Enq { .. } => false, _ => true })]
    while u < remote_pins.len() {
        let _ = dm_release_read(s, remote_pins[u], snapshot!(1));
        u += 1;
    }
    rec_remote_lookup(s, hits, misses);
    snapshot!(RemG { nf: not_found@, ans: remote_results@, sync_at: *sync_at, rel_from: *rel_from })
}

#[logic(open)]
pub fn fwd_logged(a: W, b: W, key: u64, size: Int) -> bool {
    pearlite! { exists<i: Int> a.log.len() <= i && i < b.log.len() &&
        match b.log[i] { Ev::RemoteFwd { key: k, size: z } => k == key && z@ == size, _ => false } }
}

// ---------------------------------------------------------------------------
// batch_lookup (lib.rs:2228)
// ---------------------------------------------------------------------------

pub struct BlG {
    pub cls: Seq<Cls>,
    pub tier: Seq<Option<ServedBy>>,
    pub cold: ColdG,
    pub rem: RemG,
    /// position of the warm-hit batch_touch event, and log length when the cold path began
    pub touch_at: Int,
    pub cold_from: Int,
    /// position of the deferred warm-stream sync (-1 if none) and where the warm pins were released
    pub wsync_at: Int,
    pub wrel_from: Int,
    pub wrel_to: Int,
    pub warm: u64,
    pub ready: bool,
}

#[logic]
#[variant(n)]
pub fn count_ok(o: Seq<Outcome>, n: Int) -> Int {
    pearlite! { if n <= 0 { 0 } else { count_ok(o, n - 1) + if o[n - 1].result == Ok(()) { 1 } else { 0 } } }
}

#[logic]
#[variant(n)]
pub fn count_sb(o: Seq<Outcome>, n: Int, x: ServedBy) -> Int {
    pearlite! { if n <= 0 { 0 } else { count_sb(o, n - 1, x) + if o[n - 1].served_by == x { 1 } else { 0 } } }
}

/// `count_sb` of a route that no outcome of `o` carries is zero.
#[logic]
#[requires(0 <= n && n <= o.len())]
#[requires(forall<i: Int> 0 <= i && i < n ==> o[i].served_by != x)]
#[ensures(count_sb(o, n, x) == 0)]
#[variant(n)]
pub fn count_sb_none(o: Seq<Outcome>, n: Int, x: ServedBy) {
    if n > 0 {
        count_sb_none(o, n - 1, x);
        proof_assert!(count_sb(o, n, x) == count_sb(o, n - 1, x) + (if o[n - 1].served_by == x { 1 } else { 0 }));
    }
}

/// DIS-TIER-STATS-ROUTE-PARTITION, counting half: when every served outcome of
/// `o` carries exactly one of the three route attributions and no unserved
/// outcome carries one, the per-route counts sum to the served count — the
/// three route counters partition the served keys.
#[logic]
#[requires(0 <= n && n <= o.len())]
#[requires(forall<i: Int> 0 <= i && i < n ==>
    ((match o[i].result { Ok(()) => true, _ => false }) == (o[i].served_by == ServedBy::Dram || o[i].served_by == ServedBy::Ssd || o[i].served_by == ServedBy::Remote)))]
#[ensures(count_ok(o, n) == count_sb(o, n, ServedBy::Dram) + count_sb(o, n, ServedBy::Ssd) + count_sb(o, n, ServedBy::Remote))]
#[variant(n)]
pub fn count_ok_route_split(o: Seq<Outcome>, n: Int) {
    if n > 0 {
        count_ok_route_split(o, n - 1);
        proof_assert!(count_ok(o, n) == count_ok(o, n - 1) + (if match o[n - 1].result { Ok(()) => true, _ => false } { 1 } else { 0 }));
        proof_assert!(forall<x: ServedBy> count_sb(o, n, x) == count_sb(o, n - 1, x) + (if o[n - 1].served_by == x { 1 } else { 0 }));
    }
}

/// `out.iter().filter(pred).count()` for the three tallies.
#[ensures(result.0@ == count_ok(out@, out@.len()) && result.1@ == count_sb(out@, out@.len(), ServedBy::Dram)
    && result.2@ == count_sb(out@, out@.len(), ServedBy::Ssd))]
pub fn tallies(out: &Vec<Outcome>) -> (u64, u64, u64) {
    let mut ok: u64 = 0;
    let mut dram: u64 = 0;
    let mut ssd: u64 = 0;
    let mut i: usize = 0;
    #[invariant(i@ <= out@.len() && ok@ == count_ok(out@, i@) && dram@ == count_sb(out@, i@, ServedBy::Dram)
        && ssd@ == count_sb(out@, i@, ServedBy::Ssd) && ok@ <= i@ && dram@ <= i@ && ssd@ <= i@)]
    while i < out.len() {
        match out[i].result {
            Ok(()) => ok += 1,
            Err(_) => {}
        }
        match out[i].served_by {
            ServedBy::Dram => dram += 1,
            ServedBy::Ssd => ssd += 1,
            _ => {}
        }
        i += 1;
    }
    (ok, dram, ssd)
}

/// The first non-null region address of the batch (`flat_map(...).find(|a| !a.is_null())`).
pub fn first_addr(entries: &[Be]) -> u64 {
    let mut i: usize = 0;
    while i < entries.len() {
        let mut j: usize = 0;
        while j < entries[i].regions.len() {
            if entries[i].regions[j].addr != 0 {
                return entries[i].regions[j].addr;
            }
            j += 1;
        }
        i += 1;
    }
    0
}

#[requires(pinv(s.p) && a_fr039(entries@))]
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq)]
#[ensures(result.0@.len() == entries@.len())]
#[ensures(entries@.len() == 0 ==> *(^s).g == *s.g)]
#[ensures(entries@.len() > 0 && !(s.p.init && s.p.has_dm && s.p.has_mt && s.p.has_gpu) ==> *(^s).g == *s.g && !result.1.ready
    && forall<i: Int> 0 <= i && i < entries@.len() ==> result.0@[i].served_by == ServedBy::Error
        && match result.0@[i].result { Err(DE::NotInit(_)) => true, _ => false })]
#[ensures(entries@.len() > 0 && s.p.init && s.p.has_dm && s.p.has_mt && s.p.has_gpu ==> result.1.ready)]
#[ensures(result.1.ready ==> result.1.cls.len() == entries@.len() && result.1.tier.len() == entries@.len()
    && (forall<i: Int> 0 <= i && i < entries@.len() ==>
        result.0@[i].served_by == attr(result.0@[i].result, result.1.tier[i])
        && (result.0@[i].result == Ok(()) ==> result.1.tier[i] != None)
        && (result.1.tier[i] == None || result.1.tier[i] == Some(ServedBy::Dram) || result.1.tier[i] == Some(ServedBy::Ssd) || result.1.tier[i] == Some(ServedBy::Remote))
        && (is_warm(result.1.cls[i]) ==> result.1.tier[i] == Some(ServedBy::Dram))
        && ((match result.1.cls[i] { Cls::Cold { .. } => true, _ => false }) ==> result.1.tier[i] == Some(ServedBy::Ssd))
        && (result.1.tier[i] == Some(ServedBy::Remote) ==> result.1.cls[i] == Cls::Miss)
        && ((match result.1.cls[i] { Cls::Retry { .. } => true, _ => false }) ==> result.1.tier[i] == None
            && match result.0@[i].result { Err(DE::Timeout) => true, Err(DE::Io(IoK::Dm)) => true, _ => false })
        && (match result.1.cls[i] { Cls::Retry { e } => match e { DmE::Timeout => result.0@[i].result == Err(DE::Timeout), _ => result.0@[i].result == Err(DE::Io(IoK::Dm)) }, _ => true })
        && (match result.0@[i].result { Err(DE::AlreadyExists(_)) => false, _ => true })))]
#[ensures(result.1.ready ==> (*(^s).g).c.promo_gpu == (*s.g).c.promo_gpu + count_ok(result.0@, entries@.len())
    && (*(^s).g).c.hit_dram == (*s.g).c.hit_dram + count_sb(result.0@, entries@.len(), ServedBy::Dram)
    && (*(^s).g).c.hit_ssd == (*s.g).c.hit_ssd + count_sb(result.0@, entries@.len(), ServedBy::Ssd)
    && (*(^s).g).c.r_hit == (*s.g).c.r_hit + count_rl(result.1.rem.ans, result.1.rem.ans.len(), Rl::Ok)
    && (*(^s).g).c.r_miss == (*s.g).c.r_miss + count_rl(result.1.rem.ans, result.1.rem.ans.len(), Rl::NotFound))]
#[ensures(result.1.ready ==> result.1.rem.nf.len() == result.1.rem.ans.len()
    && (!s.p.has_rl ==> result.1.rem.nf.len() == 0)
    && forall<t: Int> 0 <= t && t < result.1.rem.nf.len() ==> result.1.rem.nf[t]@ < entries@.len()
        && result.1.cls[result.1.rem.nf[t]@] == Cls::Miss
        && fwd_logged(*s.g, *(^s).g, entries@[result.1.rem.nf[t]@].key, sum_sz(entries@[result.1.rem.nf[t]@].regions@))
        && (result.1.rem.ans[t] == Rl::NotFound ==> result.0@[result.1.rem.nf[t]@].result == Err(DE::KeyNotFound(entries@[result.1.rem.nf[t]@].key)))
        && (result.1.rem.ans[t] == Rl::Transport ==> result.0@[result.1.rem.nf[t]@].result == Err(DE::Io(IoK::Remote)))
        && (result.1.tier[result.1.rem.nf[t]@] == Some(ServedBy::Remote) ==> result.1.rem.ans[t] == Rl::Ok))]
#[ensures(result.1.ready ==> forall<i: Int> 0 <= i && i < entries@.len() && result.1.cls[i] == Cls::Miss && !s.p.has_rl ==>
    result.0@[i].result == Err(DE::KeyNotFound(entries@[i].key)))]
#[ensures(result.1.ready ==> (*(^s).g).c.promo_mem == (*s.g).c.promo_mem + count_true(result.1.cold.prom, result.1.cold.prom.len()))]
#[ensures(result.1.ready && (exists<i: Int> 0 <= i && i < entries@.len() && is_warm(result.1.cls[i])) ==>
    (*s.g).log.len() <= result.1.touch_at && result.1.touch_at < result.1.cold_from && (*(^s).g).log[result.1.touch_at] == Ev::MtBatchTouch)]
#[ensures(result.1.ready && s.p.nd@ == 0 && entries@.len() == 1
    && (match result.1.cls[0] { Cls::Cold { .. } => true, _ => false })
    && has(*s.g, entries@[0].key) && is_blk(ent(*s.g, entries@[0].key).loc)
    && !mt_has(*s.g, entries@[0].key) && sum_sz(entries@[0].regions@) > 0
    && (*s.g).used + sum_sz(entries@[0].regions@) <= (*s.g).cap
    && all_from((*s.g).ob, (*s.g).tick) && all_from((*s.g).og, (*s.g).tick) ==>
    result.0@[0].result == Ok(()) && result.0@[0].served_by == ServedBy::Ssd
    && result.1.cold.nd.prom.len() == 1 && result.1.cold.nd.ins_at.len() == 1 && result.1.cold.nd.sv.len() == 1
    && result.1.cold.nd.ptr.len() == 1 && result.1.cold.nd.pr_at.len() == 1
    && result.1.cold.nd_idx.len() == 1 && result.1.cold.nd_idx[0] == 0
    && result.1.cold.nd.prom[0]
    && nd_served(*(^s).g, (*s.g).log.len(), entries@[0].regions@, entries@[0].key, sum_sz(entries@[0].regions@), result.1.cold.nd, 0))]
/// DIS-BATCH-LOOKUP-ERROR-ISOLATION (US-11 AS-4): when one cold entry reports a read
/// failure (`Err(DE::Io(IoK::Ssd))` — an NVMe read that did not complete, e.g. a
/// timeout), every OTHER cold entry whose own scatter-gather read and promotion landed
/// (`cold.ok[j]`, single region, so the read delivered inside the scatter-gather) is
/// still served: its result is Ok, not an error inherited from the sibling.
#[ensures(result.1.ready ==> result.1.cold.ok.len() == entries@.len()
    && forall<i: Int, j: Int> 0 <= i && i < entries@.len() && 0 <= j && j < entries@.len() && i != j
        && (match result.0@[i].result { Err(DE::Io(IoK::Ssd)) => true, _ => false })
        && (match result.1.cls[j] { Cls::Cold { .. } => true, _ => false })
        && result.1.cold.ok[j] && entries@[j].regions@.len() == 1
        ==> result.0@[j].result == Ok(()))]
pub fn batch_lookup(s: &mut S, entries: &[Be]) -> (Vec<Outcome>, Snapshot<BlG>) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let n = entries.len();
    let dummy = snapshot!(BlG { cls: Seq::empty(), tier: Seq::empty(), cold: ColdG { kind: Seq::empty(), prom: Seq::empty(),
        nd: NdG { prom: Seq::empty(), ins_at: Seq::empty(), sv: Seq::empty(), ptr: Seq::empty(), pr_at: Seq::empty() }, nd_idx: Seq::empty(), ok: Seq::empty() },
        rem: RemG { nf: Seq::empty(), ans: Seq::empty(), sync_at: -1, rel_from: 0 }, touch_at: 0, cold_from: 0,
        wsync_at: -1, wrel_from: 0, wrel_to: 0, warm: 0u64, ready: false });
    if n == 0 {
        return (Vec::new(), dummy);
    }
    if !s.p.init {
        return (fill_outcome(n, DE::NotInit(Ni::Dispatcher)), dummy);
    }
    if !s.p.has_dm {
        return (fill_outcome(n, DE::NotInit(Ni::DispatchMap)), dummy);
    }
    if !s.p.has_mt {
        return (fill_outcome(n, DE::NotInit(Ni::MemoryTier)), dummy);
    }
    if !s.p.has_gpu {
        return (fill_outcome(n, DE::NotInit(Ni::Gpu)), dummy);
    }
    let addr = first_addr(entries);
    let batch_device = if addr != 0 { set_batch_device(s, addr) } else { -1 };
    let (warm_raw, has_streams) = match device_streams_for(s, batch_device) {
        Some(d) => (d.warm, true),
        None => (0, false),
    };
    let g1 = snapshot!(*s.g);
    let (mut bl, cg) = bl_classify(s, entries, warm_raw);
    let g2 = snapshot!(*s.g);
    proof_assert!(forall<q2: Int, r2: Int> 0 <= q2 && q2 < r2 && r2 < bl.cold@.len() ==> bl.cold@[q2].idx@ < bl.cold@[r2].idx@);
    let t_cls = snapshot!(bl.tier@);
    let r_cls = snapshot!(bl.results@);
    // Refresh warm-hit recency before cold prep.
    let touch_at = snapshot!(s.g.log.len());
    if bl.warm_keys.len() > 0 {
        mt_batch_touch(s, &bl.warm_keys);
    }
    let cold_from = snapshot!(s.g.log.len());
    proof_assert!(bl.warm_keys@.len() > 0 ==> s.g.log[*touch_at] == Ev::MtBatchTouch && *touch_at < *cold_from);
    let g3 = snapshot!(*s.g);
    proof_assert!(forall<k: u64> has(*g3, k) == has(*g0, k) && (has(*g3, k) ==> ent(*g3, k).loc == ent(*g0, k).loc)
        && g3.mt == g0.mt && g3.used == g0.used && g3.ob == g0.ob && g3.og == g0.og && g3.tick >= g0.tick && g0.log.len() <= g3.log.len());
    let cgh = if bl.cold.len() > 0 {
        bl_cold(s, &bl.cold, &mut bl.results, entries, has_streams, batch_device, warm_raw)
    } else {
        snapshot!(ColdG { kind: Seq::empty(), prom: Seq::empty(),
            nd: NdG { prom: Seq::empty(), ins_at: Seq::empty(), sv: Seq::empty(), ptr: Seq::empty(), pr_at: Seq::empty() }, nd_idx: Seq::empty(), ok: Seq::create(n@, |i: Int| false) })
    };
    let g4 = snapshot!(*s.g);
    let r_cold = snapshot!(bl.results@);
    proof_assert!(n@ == 1 && (match cg.cls[0] { Cls::Cold { .. } => true, _ => false })
        ==> bl.cold@.len() >= 1 && bl.warm_keys@.len() == 0
            && (forall<q2: Int> 0 <= q2 && q2 < bl.cold@.len() ==> bl.cold@[q2].idx@ == 0));
    proof_assert!(n@ == 1 && (match cg.cls[0] { Cls::Cold { .. } => true, _ => false }) ==> bl.cold@.len() == 1 && cg.cpos[0] == 0);
    proof_assert!(s.p.nd@ == 0 && n@ == 1 && (match cg.cls[0] { Cls::Cold { .. } => true, _ => false })
        && has(*g0, entries@[0].key) && is_blk(ent(*g0, entries@[0].key).loc) && !mt_has(*g0, entries@[0].key)
        && sum_sz(entries@[0].regions@) > 0 && g0.used + sum_sz(entries@[0].regions@) <= g0.cap
        && all_from(g0.ob, g0.tick) && all_from(g0.og, g0.tick)
        ==> bl.cold@.len() == 1 && bl.cold@[0].key == entries@[0].key && bl.cold@[0].total@ == sum_sz(entries@[0].regions@)
            && cgh.nd.prom.len() == 1 && cgh.nd_idx.len() == 1 && cgh.nd_idx[0] == 0 && cgh.nd.prom[0]
            && nd_served(*g4, g0.log.len(), entries@[0].regions@, entries@[0].key, sum_sz(entries@[0].regions@), cgh.nd, 0)
            && r_cold[0] == Some(Ok(())) && bl.tier@[0] == Some(ServedBy::Ssd));
    proof_assert!(bl.warm_keys@.len() > 0 ==> s.g.log[*touch_at] == Ev::MtBatchTouch);
    // DIS-BATCH-LOOKUP-ERROR-ISOLATION: a cold entry whose own read and promotion
    // landed (cold.ok, single region) is Some(Ok) after the cold path, and the
    // warm sync, pin release, remote pass and recovery pass below never rewrite
    // a Some(Ok) — only KeyNotFound and AlreadyExists results are touched.
    proof_assert!(forall<j: Int> 0 <= j && j < n@ && cgh.ok[j] && entries@[j].regions@.len() == 1 ==> r_cold[j] == Some(Ok(())));
    // Deferred batched warm-stream sync, then release the warm pins.
    let mut wsync_at: Snapshot<Int> = snapshot!(-1);
    if bl.warm_keys.len() > 0 && warm_raw != 0 {
        wsync_at = snapshot!(s.g.log.len());
        let _ = gpu_sync(s, warm_raw);
    }
    let wrel_from = snapshot!(s.g.log.len());
    let g5 = snapshot!(*s.g);
    let mut u: usize = 0;
    #[invariant(s.p == *p0 && u@ <= bl.warm_keys@.len() && log_ext(*g5, *s.g) && same_env(*g5, *s.g) && s.g.wq == g5.wq && s.g.c == g5.c)]
    #[invariant(forall<q: Int> g5.log.len() <= q && q < s.g.log.len() ==> match s.g.log[q] { Ev::RelRead { own, .. } => own > 0, _ => false })]
    while u < bl.warm_keys.len() {
        let _ = dm_release_read(s, bl.warm_keys[u], snapshot!(1));
        u += 1;
    }
    let wrel_to = snapshot!(s.g.log.len());
    let g6 = snapshot!(*s.g);
    proof_assert!(bl.warm_keys@.len() > 0 ==> s.g.log[*touch_at] == Ev::MtBatchTouch);
    // Remote lookup for entries not found locally.
    let rem = bl_remote(s, entries, &mut bl.results, &mut bl.tier, warm_raw);
    let g7 = snapshot!(*s.g);
    let r_rem = snapshot!(bl.results@);
    proof_assert!((match cg.cls[0] { Cls::Cold { .. } => true, _ => false }) && r_cold[0] == Some(Ok(()))
        ==> r_rem[0] == Some(Ok(())) && bl.tier@[0] == Some(ServedBy::Ssd));
    proof_assert!(r_cold[0] == Some(Ok(())) ==> forall<t: Int> 0 <= t && t < g4.log.len() ==> s.g.log[t] == g4.log[t]);
    proof_assert!(bl.warm_keys@.len() > 0 ==> s.g.log[*touch_at] == Ev::MtBatchTouch);
    proof_assert!(forall<j: Int> 0 <= j && j < n@ && cgh.ok[j] && entries@[j].regions@.len() == 1 ==> r_rem[j] == Some(Ok(())));
    // Recover concurrent-promotion losers.
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= n@ && bl.results@.len() == n@ && log_ext(*g7, *s.g) && same_env(*g7, *s.g) && s.g.wq == g7.wq && same_ctr(g7.c, s.g.c))]
    #[invariant(forall<j: Int> 0 <= j && j < n@ && (j >= i@ || match r_rem[j] { Some(Err(DE::AlreadyExists(_))) => false, _ => true }) ==> bl.results@[j] == r_rem[j])]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> match bl.results@[j] { Some(Err(DE::AlreadyExists(_))) => false, _ => true })]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> bl.results@[j] == r_rem[j] || match r_rem[j] { Some(Err(DE::AlreadyExists(_))) => true, _ => false })]
    while i < n {
        match bl.results[i] {
            Some(Err(DE::AlreadyExists(_))) => {
                let (r, _) = serve_concurrently_promoted(s, entries[i].key, &entries[i].regions, warm_raw);
                bl.results[i] = Some(r);
            }
            _ => {}
        }
        i += 1;
    }
    let tier_end = snapshot!(bl.tier@);
    let r_end = snapshot!(bl.results@);
    proof_assert!(forall<j: Int> 0 <= j && j < n@ && cgh.ok[j] && entries@[j].regions@.len() == 1 ==> r_end[j] == Some(Ok(())));
    proof_assert!((match cg.cls[0] { Cls::Cold { .. } => true, _ => false }) && r_cold[0] == Some(Ok(()))
        ==> r_end[0] == Some(Ok(())) && tier_end[0] == Some(ServedBy::Ssd));
    proof_assert!(r_cold[0] == Some(Ok(())) ==> forall<t: Int> 0 <= t && t < g4.log.len() ==> s.g.log[t] == g4.log[t]);
    proof_assert!(s.p.nd@ == 0 && n@ == 1 && (match cg.cls[0] { Cls::Cold { .. } => true, _ => false })
        && has(*g0, entries@[0].key) && is_blk(ent(*g0, entries@[0].key).loc) && !mt_has(*g0, entries@[0].key)
        && sum_sz(entries@[0].regions@) > 0 && g0.used + sum_sz(entries@[0].regions@) <= g0.cap
        && all_from(g0.ob, g0.tick) && all_from(g0.og, g0.tick)
        ==> nd_served_ext(*g4, *s.g, g0.log.len(), entries@[0].regions@, entries@[0].key, sum_sz(entries@[0].regions@), cgh.nd, 0) == ());
    proof_assert!(s.p.nd@ == 0 && n@ == 1 && (match cg.cls[0] { Cls::Cold { .. } => true, _ => false })
        && has(*g0, entries@[0].key) && is_blk(ent(*g0, entries@[0].key).loc) && !mt_has(*g0, entries@[0].key)
        && sum_sz(entries@[0].regions@) > 0 && g0.used + sum_sz(entries@[0].regions@) <= g0.cap
        && all_from(g0.ob, g0.tick) && all_from(g0.og, g0.tick)
        ==> bl.cold@.len() == 1 && cgh.nd.prom.len() == 1 && cgh.nd.ins_at.len() == 1 && cgh.nd.sv.len() == 1
            && cgh.nd.ptr.len() == 1 && cgh.nd.pr_at.len() == 1 && cgh.nd_idx.len() == 1 && cgh.nd_idx[0] == 0
            && cgh.nd.prom[0] && nd_served(*s.g, g0.log.len(), entries@[0].regions@, entries@[0].key, sum_sz(entries@[0].regions@), cgh.nd, 0)
            && r_end[0] == Some(Ok(())) && tier_end[0] == Some(ServedBy::Ssd));
    let out = attribute(bl.results, bl.tier);
    let (served, dram, ssd) = tallies(&out);
    if served > 0 {
        rec_promotions_to_gpu(s, served);
    }
    rec_local_hits(s, dram, ssd);
    (out, snapshot!(BlG { cls: cg.cls, tier: *tier_end, cold: *cgh, rem: *rem, touch_at: *touch_at, cold_from: *cold_from,
        wsync_at: *wsync_at, wrel_from: *wrel_from, wrel_to: *wrel_to, warm: warm_raw, ready: true }))
}

/// `entries.iter().map(|_| LookupOutcome { served_by: Error, result: Err(e.clone()) }).collect()`
#[ensures(result@.len() == n@ && forall<i: Int> 0 <= i && i < n@ ==> result@[i].served_by == ServedBy::Error && result@[i].result == Err(e))]
pub fn fill_outcome(n: usize, e: DE) -> Vec<Outcome> {
    let mut v: Vec<Outcome> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= n@ && v@.len() == i@ && forall<j: Int> 0 <= j && j < i@ ==> v@[j].served_by == ServedBy::Error && v@[j].result == Err(e))]
    while i < n {
        v.push(Outcome { served_by: ServedBy::Error, result: Err(e) });
        i += 1;
    }
    v
}

#[logic]
#[requires(fwd_logged(a, b, key, size) && log_ext(b, c) && g.log.len() <= a.log.len())]
#[ensures(fwd_logged(g, c, key, size))]
pub fn fwd_ext(g: W, a: W, b: W, c: W, key: u64, size: Int) {}
