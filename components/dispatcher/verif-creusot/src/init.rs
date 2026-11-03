// init.rs — create_data_drives (lib.rs:1586) and initialize (lib.rs:1884).

/// The `DispatcherConfig` fields initialize reads. The two eviction thresholds are `f64`; only
/// the comparison `> 0.0` is used, which is carried as `*_pos` (Creusot cannot translate f64).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct ICfg {
    pub n_addrs: usize,
    pub format_on_init: bool,
    pub ssd_thr_pos: bool,
    pub mt_thr_pos: bool,
    pub max_att: usize,
    pub bp_ms: u64,
    pub stg_slots: usize,
    pub stg_bytes: usize,
}

/// Why create_data_drives failed (ghost): at which drive and step.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum DFail {
    None,
    Parse(usize),
    Create(usize),
    Partition(usize),
    Few(usize, usize),
    Format(usize),
}

/// Every drive step answer from tick `t` on is "success" and every partition table has the
/// three Certus partitions.
#[logic(open)]
pub fn drives_ok(w: W, t: Int) -> bool {
    pearlite! { all_from(w.ob, t) && forall<i: Int> i >= t ==> w.ov[i]@ >= 3 }
}

#[requires(s.p.has_spdk || s.p.has_factory)]
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures((*(^s).g).dm == (*s.g).dm && (*(^s).g).mt == (*s.g).mt && (*(^s).g).wq == (*s.g).wq && (*(^s).g).pool == (*s.g).pool
    && (*(^s).g).cluster == (*s.g).cluster && (*(^s).g).ob == (*s.g).ob && (*(^s).g).ob2 == (*s.g).ob2 && (*(^s).g).ov == (*s.g).ov
    && (*(^s).g).ext == (*s.g).ext && (*(^s).g).tick >= (*s.g).tick)]
#[ensures(match result.0 {
    Ok(()) => s.p.has_logger && result.1 == DFail::None && forall<i: Int> 0 <= i && i < cfg.n_addrs@ ==> (*(^s).g).cfree[i] == 0,
    Err(DE::NotInit(Ni::Logger)) => !s.p.has_logger && result.1 == DFail::None,
    Err(DE::InvalidParam) => match result.1 { DFail::Parse(_) => true, _ => false },
    Err(DE::Io(IoK::Partition { drive, expected, found })) => expected@ == 3 && found@ < 3 && result.1 == DFail::Few(drive, found),
    Err(DE::Io(IoK::Drive(i))) => result.1 == DFail::Create(i) || result.1 == DFail::Partition(i) || result.1 == DFail::Format(i),
    Err(_) => false,
})]
#[ensures(s.p.has_logger && drives_ok(*s.g, (*s.g).tick) ==> result.0 == Ok(()))]
#[ensures(match result.1 {
    DFail::Few(d, f) => result.0 == Err(DE::Io(IoK::Partition { drive: d, expected: 3usize, found: f })) && f@ < 3,
    DFail::Create(d) => result.0 == Err(DE::Io(IoK::Drive(d))),
    DFail::Partition(d) => result.0 == Err(DE::Io(IoK::Drive(d))),
    DFail::Format(d) => result.0 == Err(DE::Io(IoK::Drive(d))),
    DFail::Parse(_) => result.0 == Err(DE::InvalidParam),
    DFail::None => true })]
#[ensures(forall<i: Int> (*s.g).log.len() <= i && i < (*(^s).g).log.len() ==> match (*(^s).g).log[i] {
    Ev::EmFormat { .. } => true, Ev::EmInit { .. } => true, _ => false })]
#[ensures(!cfg.format_on_init && s.p.has_logger && cfg.n_addrs@ >= 1 && all_from((*s.g).ob, (*s.g).tick) && all_from((*s.g).ob2, (*s.g).tick)
    && (forall<i: Int> i >= (*s.g).tick ==> (*s.g).ov[i]@ >= 3) ==> logged(*s.g, *(^s).g, Ev::EmFormat { drive: 0usize }))]
pub fn create_data_drives(s: &mut S, cfg: ICfg) -> (Result<(), DE>, DFail) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    if !s.p.has_logger {
        return (Err(DE::NotInit(Ni::Logger)), DFail::None);
    }
    // per-drive poller CPU assignment (poller_base_cpu or NUMA round-robin): not modelled here.
    let mut i: usize = 0;
    let mut fmt0: Snapshot<Int> = snapshot!(-1);
    #[invariant(s.p == *p0 && i@ <= cfg.n_addrs@ && log_ext(*g0, *s.g) && same_ctr(g0.c, s.g.c))]
    #[invariant(s.g.dm == g0.dm && s.g.mt == g0.mt && s.g.wq == g0.wq && s.g.pool == g0.pool && s.g.cluster == g0.cluster
        && s.g.ob == g0.ob && s.g.ob2 == g0.ob2 && s.g.ov == g0.ov && s.g.ext == g0.ext && s.g.tick >= g0.tick)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> s.g.cfree[j] == 0)]
    #[invariant(forall<q: Int> g0.log.len() <= q && q < s.g.log.len() ==> match s.g.log[q] { Ev::EmFormat { .. } => true, Ev::EmInit { .. } => true, _ => false })]
    #[invariant(drives_ok(*g0, g0.tick) ==> drives_ok(*s.g, s.g.tick))]
    #[invariant(!cfg.format_on_init && all_from(g0.ob, g0.tick) && all_from(g0.ob2, g0.tick) && (forall<t: Int> t >= g0.tick ==> g0.ov[t]@ >= 3) && i@ >= 1 ==>
        *fmt0 >= g0.log.len() && *fmt0 < s.g.log.len() && s.g.log[*fmt0] == Ev::EmFormat { drive: 0usize })]
    #[invariant(all_from(g0.ob, g0.tick) && all_from(g0.ob2, g0.tick) ==> all_from(s.g.ob, s.g.tick) && all_from(s.g.ob2, s.g.tick))]
    while i < cfg.n_addrs {
        if !parse_pci(s, i) {
            return (Err(DE::InvalidParam), DFail::Parse(i));
        }
        if !create_drive(s, i) {
            return (Err(DE::Io(IoK::Drive(i))), DFail::Create(i));
        }
        let po = match part_init(s, i, cfg.format_on_init) {
            Some(po) => po,
            None => return (Err(DE::Io(IoK::Drive(i))), DFail::Partition(i)),
        };
        // EXPECTED_PARTITIONS guard (lib.rs:1773).
        if po.nparts < 3 {
            return (Err(DE::Io(IoK::Partition { drive: i, expected: 3, found: po.nparts })), DFail::Few(i, po.nparts));
        }
        if po.formatted {
            if i == 0 {
                fmt0 = snapshot!(s.g.log.len());
            }
            if !em_format(s, i) {
                return (Err(DE::Io(IoK::Drive(i))), DFail::Format(i));
            }
        } else {
            if !em_initialize(s, i) {
                return (Err(DE::Io(IoK::Drive(i))), DFail::Format(i));
            }
        }
        chanpool_new(s, i);
        i += 1;
    }
    (Ok(()), DFail::None)
}

/// Recovery (lib.rs:1914-1932): every allocated extent of every drive, in drive order.
#[ensures((^s).p == s.p && log_ext(*s.g, *(^s).g) && same_ctr((*s.g).c, (*(^s).g).c) && same_env(*s.g, *(^s).g) && (*(^s).g).wq == (*s.g).wq && (*(^s).g).mt == (*s.g).mt)]
#[ensures(forall<d: Int, e: Int> 0 <= d && d < n@ && 0 <= e && e < (*s.g).ext[d].len() ==>
    logged(*s.g, *(^s).g, Ev::Recover { key: (*s.g).ext[d][e].key, off: (*s.g).ext[d][e].off, size: (*s.g).ext[d][e].size }))]
#[ensures(forall<k: u64> has(*s.g, k) ==> has(*(^s).g, k))]
pub fn recover_all(s: &mut S, n: usize) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut d: usize = 0;
    #[invariant(s.p == *p0 && d@ <= n@ && log_ext(*g0, *s.g) && same_ctr(g0.c, s.g.c) && same_env(*g0, *s.g) && s.g.wq == g0.wq && s.g.mt == g0.mt)]
    #[invariant(forall<k: u64> has(*g0, k) ==> has(*s.g, k))]
    #[invariant(forall<d2: Int, e: Int> 0 <= d2 && d2 < d@ && 0 <= e && e < g0.ext[d2].len() ==>
        logged(*g0, *s.g, Ev::Recover { key: g0.ext[d2][e].key, off: g0.ext[d2][e].off, size: g0.ext[d2][e].size }))]
    while d < n {
        let exts = em_extents(s, d);
        let gd = snapshot!(*s.g);
        let mut e: usize = 0;
        #[invariant(s.p == *p0 && e@ <= exts@.len() && log_ext(*gd, *s.g) && same_ctr(g0.c, s.g.c) && same_env(*g0, *s.g) && s.g.wq == g0.wq && s.g.mt == g0.mt)]
        #[invariant(log_ext(*g0, *s.g) && forall<k: u64> has(*g0, k) ==> has(*s.g, k))]
        #[invariant(forall<d2: Int, e2: Int> 0 <= d2 && d2 < d@ && 0 <= e2 && e2 < g0.ext[d2].len() ==>
            logged(*g0, *s.g, Ev::Recover { key: g0.ext[d2][e2].key, off: g0.ext[d2][e2].off, size: g0.ext[d2][e2].size }))]
        #[invariant(forall<e2: Int> 0 <= e2 && e2 < e@ ==> logged(*g0, *s.g, Ev::Recover { key: exts@[e2].key, off: exts@[e2].off, size: exts@[e2].size }))]
        while e < exts.len() {
            let x = exts[e];
            let at = snapshot!(s.g.log.len());
            let _ = dm_recover(s, x.key, x.off, x.size);
            proof_assert!(s.g.log[*at] == Ev::Recover { key: x.key, off: x.off, size: x.size });
            e += 1;
        }
        d += 1;
    }
}

#[requires(pinv(s.p))]
#[ensures(pinv((^s).p))]
#[ensures((^s).p.max_att == cfg.max_att && (^s).p.bp_ms == cfg.bp_ms)]
#[ensures((^s).p.has_dm == s.p.has_dm && (^s).p.has_mt == s.p.has_mt && (^s).p.has_gpu == s.p.has_gpu && (^s).p.has_spdk == s.p.has_spdk
    && (^s).p.has_factory == s.p.has_factory && (^s).p.has_rl == s.p.has_rl && (^s).p.has_logger == s.p.has_logger)]
#[ensures(log_ext(*s.g, *(^s).g) && same_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).wq == (*s.g).wq && (*(^s).g).mt == (*s.g).mt)]
#[ensures(!s.p.has_dm ==> result.0 == Err(DE::NotInit(Ni::DispatchMap)) && *(^s).g == *s.g)]
#[ensures(s.p.has_dm && !s.p.has_mt ==> result.0 == Err(DE::NotInit(Ni::MemoryTier)) && *(^s).g == *s.g)]
#[ensures(s.p.has_dm && s.p.has_mt && cfg.n_addrs@ == 0 ==> result.0 == Err(DE::InvalidParam) && *(^s).g == *s.g && (^s).p.nd == s.p.nd)]
#[ensures(match result.0 { Err(_) => (^s).p.init == s.p.init && (^s).p.writer == s.p.writer && (^s).p.ssd_ev == s.p.ssd_ev && (^s).p.mt_ev == s.p.mt_ev, Ok(()) => true })]
#[ensures(match result.0 {
    Ok(()) => (^s).p.init && (^s).p.writer
        && ((^s).p.ssd_ev == (s.p.ssd_ev || (cfg.ssd_thr_pos && (^s).p.nd@ > 0)))
        && ((^s).p.mt_ev == (s.p.mt_ev || cfg.mt_thr_pos))
        && ((s.p.has_spdk || s.p.has_factory) ==> (^s).p.nd == cfg.n_addrs && forall<i: Int> 0 <= i && i < cfg.n_addrs@ ==> (*(^s).g).cfree[i] == 0)
        && (!(s.p.has_spdk || s.p.has_factory) ==> (^s).p.nd == s.p.nd)
        && (s.p.has_rl ==> logged(*s.g, *(^s).g, Ev::Join)),
    Err(DE::NotInit(_)) => true,
    Err(DE::InvalidParam) => true,
    Err(DE::Io(_)) => (s.p.has_spdk || s.p.has_factory) && match result.1 { DFail::None => false, DFail::Parse(_) => false, _ => true },
    Err(_) => false,
})]
#[ensures(match result.0 { Err(DE::Io(IoK::Partition { drive, expected, found })) => expected@ == 3 && found@ < 3 && result.1 == DFail::Few(drive, found), _ => true })]
#[ensures(s.p.has_dm && s.p.has_mt && cfg.n_addrs@ > 0 && !(s.p.has_spdk || s.p.has_factory) ==> result.0 == Ok(()))]
#[ensures(s.p.has_dm && s.p.has_mt && cfg.n_addrs@ > 0 && (s.p.has_spdk || s.p.has_factory) && s.p.has_logger && drives_ok(*s.g, (*s.g).tick) ==> result.0 == Ok(()))]
#[ensures(s.p.has_dm && s.p.has_mt && cfg.n_addrs@ > 0 && (s.p.has_spdk || s.p.has_factory) && !cfg.format_on_init && result.0 == Ok(()) ==>
    forall<d: Int, e: Int> 0 <= d && d < cfg.n_addrs@ && 0 <= e && e < (*s.g).ext[d].len() ==>
        logged(*s.g, *(^s).g, Ev::Recover { key: (*s.g).ext[d][e].key, off: (*s.g).ext[d][e].off, size: (*s.g).ext[d][e].size }))]
#[ensures(s.p.has_dm && s.p.has_mt && cfg.n_addrs@ >= 1 && (s.p.has_spdk || s.p.has_factory) && !cfg.format_on_init && s.p.has_logger
    && all_from((*s.g).ob, (*s.g).tick) && all_from((*s.g).ob2, (*s.g).tick) && (forall<i: Int> i >= (*s.g).tick ==> (*s.g).ov[i]@ >= 3) ==>
    logged(*s.g, *(^s).g, Ev::EmFormat { drive: 0usize }))]
pub fn initialize(s: &mut S, cfg: ICfg) -> (Result<(), DE>, DFail) {
    let g0 = snapshot!(*s.g);
    let mut gfmt: Snapshot<W> = snapshot!(*s.g);
    let mut grec1: Snapshot<W> = snapshot!(*s.g);
    let mut grec2: Snapshot<W> = snapshot!(*s.g);
    s.p.max_att = cfg.max_att;
    s.p.bp_ms = cfg.bp_ms;
    if !s.p.has_dm {
        return (Err(DE::NotInit(Ni::DispatchMap)), DFail::None);
    }
    if !s.p.has_mt {
        return (Err(DE::NotInit(Ni::MemoryTier)), DFail::None);
    }
    if cfg.n_addrs == 0 {
        return (Err(DE::InvalidParam), DFail::None);
    }
    if s.p.has_spdk || s.p.has_factory {
        let (r, f) = create_data_drives(s, cfg);
        match r {
            Ok(()) => {}
            Err(e) => return (Err(e), f),
        }
        let g1 = snapshot!(*s.g);
        gfmt = g1;
        s.p.nd = cfg.n_addrs;
        if !cfg.format_on_init {
            recover_all(s, cfg.n_addrs);
        }
        let g2 = snapshot!(*s.g);
        grec1 = g1;
        grec2 = g2;
        proof_assert!(forall<i: Int> 0 <= i && i < cfg.n_addrs@ ==> s.g.cfree[i] == g1.cfree[i]);
        if s.p.has_gpu {
            // warm stream (non-fatal)
            let _ = gpu_create_stream(s);
            if s.p.has_spdk {
                if ring_new(s) {
                    s.p.ring = true;
                    s.p.stg = cfg.stg_slots > 0;
                    s.p.stg_bytes = cfg.stg_bytes;
                }
                if mt_pool_info(s) {
                    let _ = gpu_register(s);
                }
            }
        }
        proof_assert!(forall<i: Int> 0 <= i && i < cfg.n_addrs@ ==> s.g.cfree[i] == g1.cfree[i]);
    }
    // ParallelBackgroundWriter::start(max(nd, 1), ..)
    s.p.writer = true;
    if cfg.ssd_thr_pos {
        if s.p.nd > 0 {
            s.p.ssd_ev = true;
        }
    }
    if cfg.mt_thr_pos {
        s.p.mt_ev = true;
    }
    s.p.init = true;
    if s.p.has_rl {
        let at = snapshot!(s.g.log.len());
        let _ = rl_join(s);
        proof_assert!(s.g.log[*at] == Ev::Join);
    }
    proof_assert!((s.p.has_spdk || s.p.has_factory) && logged(*g0, *gfmt, Ev::EmFormat { drive: 0usize }) ==>
        logged_ext(*g0, *g0, *gfmt, *s.g, Ev::EmFormat { drive: 0usize }) == ());
    proof_assert!((s.p.has_spdk || s.p.has_factory) && !cfg.format_on_init ==> grec1.ext == g0.ext && g0.log.len() <= grec1.log.len() && log_ext(*grec2, *s.g));
    proof_assert!((s.p.has_spdk || s.p.has_factory) && !cfg.format_on_init ==>
        forall<d: Int, e: Int> 0 <= d && d < cfg.n_addrs@ && 0 <= e && e < g0.ext[d].len() ==>
            logged_ext(*g0, *grec1, *grec2, *s.g, Ev::Recover { key: g0.ext[d][e].key, off: g0.ext[d][e].off, size: g0.ext[d][e].size }) == ());
    (Ok(()), DFail::None)
}
