// methods.rs — mirrors of the small IDispatcher methods and their helpers (lib.rs:651-3900).
// One mirror per real function, statement-for-statement; `let _ = x` results are discarded as
// in the source. Ghost ownership counts (`snapshot!(n)`) instrument release calls only.

/// `ensure_initialized` (lib.rs:651).
#[check(terminates)]
#[ensures(match result { Ok(()) => s.p.init, Err(e) => !s.p.init && e == DE::NotInit(Ni::Dispatcher) })]
pub fn ensure_initialized(s: &S) -> Result<(), DE> {
    if !s.p.init {
        return Err(DE::NotInit(Ni::Dispatcher));
    }
    Ok(())
}

/// `drive_index` (lib.rs:581): splitmix64 finalizer modulo the drive count. Trusted only for
/// naming its value `di(key, n)` (an opaque logic name for this very function); `% n` < n.
#[trusted]
#[check(terminates)]
#[requires(n@ > 0)]
#[ensures(result == di(key, n) && result@ < n@)]
pub fn drive_index(key: u64, n: usize) -> usize {
    let mut h = key;
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    h as usize % n
}

#[check(terminates)]
#[ensures(result@ == if n@ == 0 { 1 } else { n@ })]
pub fn max1(n: usize) -> usize {
    if n == 0 { 1 } else { n }
}

// ---------------------------------------------------------------------------
// check (lib.rs:3075)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && s.p.has_dm ==> match result {
    Ok(true) => has(*s.g, k) && ent(*s.g, k).wr@ == 0,
    Ok(false) => !has(*s.g, k) || ent(*s.g, k).wr@ > 0 || ent(*s.g, k).rr@ == u32::MAX@,
    Err(_) => false,
})]
#[ensures(forall<j: u64> (*(^s).g).dm.get(j) == (*s.g).dm.get(j))]
#[ensures((*(^s).g).mt == (*s.g).mt && (*(^s).g).wq == (*s.g).wq && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(log_ext(*s.g, *(^s).g) && no_dma(*s.g, *(^s).g))]
pub fn check(s: &mut S, k: u64) -> Result<bool, DE> {
    match ensure_initialized(s) {
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    if !s.p.has_dm {
        return Err(DE::NotInit(Ni::DispatchMap));
    }
    match dm_lookup(s, k) {
        Ok(r) => {
            let exists = match r { Lr::NotExist => false, _ => true };
            if exists {
                let _ = dm_release_read(s, k, snapshot!(1));
            }
            Ok(exists)
        }
        Err(_) => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// pin / unpin / touch (lib.rs:3586-3634)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && s.p.has_dm ==> match result {
    Ok(()) => ent_rr(*s.g, *(^s).g, k, 1) && only_dm(*s.g, *(^s).g),
    Err(DE::KeyNotFound(j)) => j == k && !has(*s.g, k) && *(^s).g == *s.g,
    Err(DE::Timeout) => has(*s.g, k) && ent(*s.g, k).wr@ > 0 && *(^s).g == *s.g,
    Err(DE::Io(_)) => *(^s).g == *s.g,
    Err(_) => false,
})]
pub fn pin(s: &mut S, k: u64) -> Result<(), DE> {
    match ensure_initialized(s) {
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    if !s.p.has_dm {
        return Err(DE::NotInit(Ni::DispatchMap));
    }
    match dm_take_read(s, k) {
        Ok(()) => Ok(()),
        Err(DmE::KeyNotFound) => Err(DE::KeyNotFound(k)),
        Err(DmE::Timeout) => Err(DE::Timeout),
        Err(_) => Err(DE::Io(IoK::Dm)),
    }
}

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && s.p.has_dm ==> match result {
    Ok(()) => ent_rr(*s.g, *(^s).g, k, -1) && only_dm_log(*s.g, *(^s).g, Ev::RelRead { key: k, own: 1 }),
    Err(DE::KeyNotFound(j)) => j == k && (!has(*s.g, k) || ent(*s.g, k).rr@ == 0)
                               && (*(^s).g).dm == (*s.g).dm && (*(^s).g).mt == (*s.g).mt,
    Err(_) => false,
})]
pub fn unpin(s: &mut S, k: u64) -> Result<(), DE> {
    match ensure_initialized(s) {
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    if !s.p.has_dm {
        return Err(DE::NotInit(Ni::DispatchMap));
    }
    match dm_release_read(s, k, snapshot!(1)) {
        Ok(()) => Ok(()),
        Err(DmE::KeyNotFound) => Err(DE::KeyNotFound(k)),
        Err(DmE::Underflow) => Err(DE::KeyNotFound(k)),
        Err(_) => Err(DE::Io(IoK::Dm)),
    }
}

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && s.p.has_dm ==> match result {
    Ok(()) => has(*s.g, k) && (*(^s).g).dm == (*s.g).dm && (*(^s).g).mt == (*s.g).mt
              && (*(^s).g).log == (if s.p.has_mt { (*s.g).log.push_back(Ev::DmTouch { key: k }).push_back(Ev::MtTouch { key: k }) }
                                   else { (*s.g).log.push_back(Ev::DmTouch { key: k }) }),
    Err(DE::KeyNotFound(j)) => j == k && !has(*s.g, k) && *(^s).g == *s.g,
    Err(_) => false,
})]
#[ensures((*(^s).g).dm == (*s.g).dm && (*(^s).g).mt == (*s.g).mt && log_ext(*s.g, *(^s).g) && no_dma(*s.g, *(^s).g))]
pub fn touch(s: &mut S, k: u64) -> Result<(), DE> {
    match ensure_initialized(s) {
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    if !s.p.has_dm {
        return Err(DE::NotInit(Ni::DispatchMap));
    }
    match dm_touch(s, k) {
        Ok(()) => {}
        Err(_) => return Err(DE::KeyNotFound(k)),
    }
    if s.p.has_mt {
        mt_touch(s, k);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// remove (lib.rs:3186)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && s.p.has_dm && !has(*s.g, k) ==> result == Err(DE::KeyNotFound(k)) && *(^s).g == *s.g)]
#[ensures(s.p.init && s.p.has_dm && has(*s.g, k) && ent(*s.g, k).rr@ == 0 && ent(*s.g, k).wr@ == 0 ==> result == Ok(()))]
#[ensures(s.p.init && s.p.has_dm && s.p.has_mt ==> match result {
    Ok(()) => has(*s.g, k) && !has(*(^s).g, k) && !mt_has(*(^s).g, k)
              && (s.p.nd@ == 0 || match ent(*s.g, k).loc {
                    Loc::Blk { off } => logged(*s.g, *(^s).g, Ev::ExtFree { drive: di(k, s.p.nd), off }),
                    Loc::Mem { ssd: Some(off), .. } => logged(*s.g, *(^s).g, Ev::ExtFree { drive: di(k, s.p.nd), off }),
                    _ => true }),
    _ => true,
})]
#[ensures(forall<j: u64> j != k ==> (*(^s).g).dm.get(j) == (*s.g).dm.get(j) && (*(^s).g).mt.get(j) == (*s.g).mt.get(j))]
#[ensures(log_ext(*s.g, *(^s).g) && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(s.p.init && s.p.has_dm ==> own_reads(*s.g, *(^s).g))]
pub fn remove(s: &mut S, k: u64) -> Result<(), DE> {
    match ensure_initialized(s) {
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    if !s.p.has_dm {
        return Err(DE::NotInit(Ni::DispatchMap));
    }
    // The memory-tier state this call's pre-pass flips away, to restore if the
    // removal below is refused (lib.rs:3205): a refused remove (`KeyNotFound`)
    // frees nothing and changes nothing.
    let mut flipped_memory_tier: Option<(u64, u32)> = None;
    let block_offset = match dm_lookup(s, k) {
        Ok(Lr::Blk { off }) => {
            let _ = dm_release_read(s, k, snapshot!(1));
            Some(off)
        }
        Ok(Lr::Mem { ptr, size }) => {
            // A resident entry may already carry its write-through SSD copy (`ssd_offset`),
            // which this lookup result does not expose: flip the entry to its block-device
            // state and look it up once more to learn which extent to free (lib.rs:3206-3241).
            let converted = dm_convert_memory_tier_to_block(s, k).is_ok();
            let _ = dm_release_read(s, k, snapshot!(1));
            if converted {
                match dm_lookup(s, k) {
                    Ok(Lr::Blk { off }) => {
                        let _ = dm_release_read(s, k, snapshot!(1));
                        // This call flipped the entry away: remember how to put it
                        // back if the removal is refused.
                        flipped_memory_tier = Some((ptr, size));
                        Some(off)
                    }
                    Ok(Lr::Mem { .. }) => {
                        let _ = dm_release_read(s, k, snapshot!(1));
                        None
                    }
                    Ok(_) => None,
                    Err(_) => None,
                }
            } else {
                None
            }
        }
        // NotExist (and MismatchSize, never answered from lookup): this lookup took no
        // read reference, so there is none of this caller's to release.
        Ok(_) => None,
        Err(_) => return Err(DE::KeyNotFound(k)),
    };
    match dm_remove(s, k) {
        Ok(()) => {}
        Err(DmE::ActiveRefs) => {
            // A reader, a client pin or a concurrent lookup holds the entry: the
            // remove is refused (the write-through-pin takeover of FR-018 is not
            // modelled here — no modelled state holds a pending write-through pin
            // across remove). `KeyNotFound` promises that nothing was freed and
            // nothing changed, so put the entry back the way this call found it:
            // still MemoryTier, with its memory-tier slot and its ssd_offset (the
            // in-place promote is the convert's inverse and keeps every reference).
            if let Some((ptr, size)) = flipped_memory_tier {
                let _ = dm_promote(s, k, ptr, size);
            }
            return Err(DE::KeyNotFound(k));
        }
        Err(_) => return Err(DE::KeyNotFound(k)),
    }
    if s.p.has_mt {
        let _ = mt_remove(s, k);
    }
    match block_offset {
        Some(off) => {
            let n = max1(s.p.nd);
            let idx = drive_index(k, n);
            if idx < s.p.nd {
                em_remove_extent(s, idx, off);
            }
        }
        None => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// release_memory (lib.rs:3571)
// ---------------------------------------------------------------------------

#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init && s.p.has_mt ==> result == Ok(()) && !mt_has(*(^s).g, k)
          && (mt_has(*s.g, k) ==> *(^s).g == lg(W { mt: (*s.g).mt.remove(k), used: if (*s.g).used >= (*s.g).mt.lookup(k).size@ { (*s.g).used - (*s.g).mt.lookup(k).size@ } else { 0 }, ..*s.g }, Ev::MtRemove { key: k }))
          && (!mt_has(*s.g, k) ==> *(^s).g == *s.g))]
#[ensures((*(^s).g).dm == (*s.g).dm && forall<j: u64> j != k ==> (*(^s).g).mt.get(j) == (*s.g).mt.get(j))]
pub fn release_memory(s: &mut S, k: u64) -> Result<(), DE> {
    match ensure_initialized(s) {
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    if !s.p.has_mt {
        return Err(DE::NotInit(Ni::MemoryTier));
    }
    match mt_remove(s, k) {
        Ok(()) => Ok(()),
        Err(MtE::KeyNotFound) => Ok(()),
        Err(_) => Err(DE::Io(IoK::Other)),
    }
}

// ---------------------------------------------------------------------------
// Background writer: enqueue (background.rs:187), process_write_job (lib.rs:1397), the
// untracked path of run_write_job (lib.rs:1412) with its record_write (background.rs:365)
// ---------------------------------------------------------------------------

/// `ParallelBackgroundWriter::enqueue`: routed to the writer thread of the job's drive; the
/// channel is unbounded and its receiver lives as long as the writer, so the send succeeds.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == lg(W { wq: (*s.g).wq.push_back(job), ..*s.g }, Ev::Enq { job }))]
pub fn writer_enqueue(s: &mut S, job: Job) {
    unimplemented!()
}

/// What a processed write job did, for the drivers (ghost).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum WjOut {
    /// no memory-tier slot: the job's read ref was released.
    NoSlot,
    /// the data landed (or memory-tier-only mode): `convert_to_storage(key, off)` was called.
    Stored { off: u64 },
    /// extent reservation or SSD write failed: the job's read ref was released.
    Failed,
}

#[requires(a_us5(s.p, job.key))]
#[ensures((^s).p == s.p)]
#[ensures(log_ext(*s.g, *(^s).g) && no_lru(*s.g, *(^s).g, job.key) && no_cpdev(*s.g, *(^s).g) && same_env(*s.g, *(^s).g))]
#[ensures((*(^s).g).mt == (*s.g).mt && (*(^s).g).wq == (*s.g).wq && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(others(*s.g, *(^s).g, job.key))]
#[ensures(match result {
    WjOut::Stored { off } => mt_has(*s.g, job.key)
        && (s.p.nd@ == 0 ==> off@ == job.key@ * 4096)
        && (has(*s.g, job.key) && is_mem(ent(*s.g, job.key).loc) ==>
              has(*(^s).g, job.key) && ent(*(^s).g, job.key).loc == (match ent(*s.g, job.key).loc { Loc::Mem { ptr, size, .. } => Loc::Mem { ptr, size, ssd: Some(off) }, l => l })
              && ent(*(^s).g, job.key).wr == ent(*s.g, job.key).wr
              && ent(*(^s).g, job.key).rr@ == (if ent(*s.g, job.key).rr@ > 0 { ent(*s.g, job.key).rr@ - 1 } else { 0 }))
        && (!(has(*s.g, job.key) && is_mem(ent(*s.g, job.key).loc)) ==> (*(^s).g).dm == (*s.g).dm),
    _ => (has(*s.g, job.key) && ent(*s.g, job.key).rr@ > 0 ==> ent_rr(*s.g, *(^s).g, job.key, -1))
        && (!(has(*s.g, job.key) && ent(*s.g, job.key).rr@ > 0) ==> (*(^s).g).dm == (*s.g).dm),
})]
#[ensures(match result { WjOut::NoSlot => !mt_has(*s.g, job.key), _ => true })]
#[ensures(forall<k: u64> has(*s.g, k) ==> has(*(^s).g, k))]
#[ensures(forall<k: u64> has(*s.g, k) && persisted(ent(*s.g, k).loc) ==> persisted(ent(*(^s).g, k).loc))]
pub fn process_write_job(s: &mut S, job: Job) -> WjOut {
    let _mem = match mt_peek(s, job.key) {
        Some(v) => v,
        None => {
            let _ = dm_release_read(s, job.key, snapshot!(1));
            return WjOut::NoSlot;
        }
    };
    if s.p.nd == 0 {
        let block_offset = job.key * 4096;
        let _ = dm_convert_to_storage(s, job.key, block_offset);
        return WjOut::Stored { off: block_offset };
    }
    let drive_idx = job.dev % s.p.nd;
    // DmaBuffer::from_raw over a valid memory-tier slot (non-null, size > 0) cannot fail here;
    // its block-size alignment arithmetic lives inside the extent / SSD boundaries below.
    let block_offset = match em_reserve(s, drive_idx, job.key) {
        Some(o) => o,
        None => {
            let _ = dm_release_read(s, job.key, snapshot!(1));
            return WjOut::Failed;
        }
    };
    if !ssd_write(s, drive_idx, job.key) {
        let _ = dm_release_read(s, job.key, snapshot!(1));
        return WjOut::Failed;
    }
    let _ = dm_convert_to_storage(s, job.key, block_offset);
    WjOut::Stored { off: block_offset }
}

/// One background-writer step: the oldest pending job is taken off the queue and processed.
#[requires(a_us5_q(s.p, (*s.g).wq))]
#[requires((*s.g).wq.len() > 0)]
#[ensures((^s).p == s.p)]
#[ensures((*(^s).g).wq == (*s.g).wq.subsequence(1, (*s.g).wq.len()))]
#[ensures(log_ext(*s.g, *(^s).g) && no_cpdev(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).mt == (*s.g).mt && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(forall<k: u64> has(*s.g, k) ==> has(*(^s).g, k))]
#[ensures(forall<k: u64> has(*s.g, k) && persisted(ent(*s.g, k).loc) ==> persisted(ent(*(^s).g, k).loc))]
#[ensures(match result {
    WjOut::Stored { .. } => has(*s.g, (*s.g).wq[0].key) && is_mem(ent(*s.g, (*s.g).wq[0].key).loc) ==> persisted(ent(*(^s).g, (*s.g).wq[0].key).loc),
    _ => true })]
pub fn writer_step(s: &mut S) -> WjOut {
    let job = writer_pop(s);
    let key = job.key;
    let out = process_write_job(s, job);
    pins_record_write(s, key, match out { WjOut::Stored { .. } => true, _ => false });
    out
}

/// The writer thread's `recv`: the oldest pending job.
#[trusted]
#[requires((*s.g).wq.len() > 0)]
#[ensures((^s).p == s.p && result == (*s.g).wq[0] && *(^s).g == W { wq: (*s.g).wq.subsequence(1, (*s.g).wq.len()), ..*s.g })]
pub fn writer_pop(s: &mut S) -> Job {
    unimplemented!()
}

/// `writer.in_flight()` = jobs enqueued but not yet processed.
#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g && result@ == (*s.g).wq.len())]
pub fn writer_in_flight(s: &mut S) -> usize {
    unimplemented!()
}

#[trusted]
#[ensures((^s).p == s.p && *(^s).g == *s.g && result == ((*s.g).wq.len() == 0))]
pub fn writer_idle(s: &mut S) -> bool {
    unimplemented!()
}

/// The background writer threads processing every pending job while the caller waits
/// (`ParallelBackgroundWriter::flush`, background.rs:198, and the shutdown drain,
/// background.rs:133): in a single-threaded run this is the writer loop run to an empty queue.
/// Returns (ghost) the keys whose write-through landed on a memory-tier entry.
#[requires(a_us5_q(s.p, (*s.g).wq))]
#[ensures((^s).p == s.p && (*(^s).g).wq.len() == 0)]
#[ensures(log_ext(*s.g, *(^s).g) && no_cpdev(*s.g, *(^s).g) && same_env(*s.g, *(^s).g) && (*(^s).g).mt == (*s.g).mt && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures(forall<k: u64> has(*s.g, k) ==> has(*(^s).g, k))]
#[ensures(forall<k: u64> has(*s.g, k) && persisted(ent(*s.g, k).loc) ==> persisted(ent(*(^s).g, k).loc))]
#[ensures(forall<i: Int> 0 <= i && i < result.len() ==> has(*(^s).g, result[i]) && persisted(ent(*(^s).g, result[i]).loc))]
pub fn drain(s: &mut S) -> Snapshot<Seq<u64>> {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut done: Snapshot<Seq<u64>> = snapshot!(Seq::empty());
    #[invariant(s.p == *p0 && a_us5_q(s.p, (*s.g).wq))]
    #[invariant(log_ext(*g0, *s.g) && no_cpdev(*g0, *s.g) && same_env(*g0, *s.g) && (*s.g).mt == g0.mt && same_ctr(g0.c, (*s.g).c))]
    #[invariant(forall<k: u64> has(*g0, k) ==> has(*s.g, k))]
    #[invariant(forall<k: u64> has(*g0, k) && persisted(ent(*g0, k).loc) ==> persisted(ent(*s.g, k).loc))]
    #[invariant(forall<i: Int> 0 <= i && i < done.len() ==> has(*s.g, done[i]) && persisted(ent(*s.g, done[i]).loc))]
    while !writer_idle(s) {
        let g1 = snapshot!(*s.g);
        let r = writer_step(s);
        done = snapshot!(if (match r { WjOut::Stored { .. } => true, _ => false })
                             && has(*g1, g1.wq[0].key) && is_mem(ent(*g1, g1.wq[0].key).loc)
                         { done.push_back(g1.wq[0].key) } else { *done });
    }
    done
}

// ---------------------------------------------------------------------------
// schedule_write_through (lib.rs:4033), flush_to_ssd (lib.rs:3995)
// ---------------------------------------------------------------------------

#[requires(pinv(s.p))]
#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init ==> result == Ok(())
          && *(^s).g == lg(W { wq: (*s.g).wq.push_back(Job { key, size, dev: di(key, if s.p.nd@ == 0 { 1usize } else { s.p.nd }) }), ..*s.g },
                           Ev::Enq { job: Job { key, size, dev: di(key, if s.p.nd@ == 0 { 1usize } else { s.p.nd }) } }))]
pub fn schedule_write_through(s: &mut S, key: u64, size: u32) -> Result<(), DE> {
    match ensure_initialized(s) {
        Err(e) => return Err(e),
        Ok(()) => {}
    }
    let num_drives = max1(s.p.nd);
    if s.p.writer {
        writer_enqueue(s, Job { key, size, dev: drive_index(key, num_drives) });
    }
    Ok(())
}

#[requires(pinv(s.p) && a_us5_q(s.p, (*s.g).wq))]
#[ensures((^s).p == s.p)]
#[ensures(!s.p.init ==> result.0 == Err(DE::NotInit(Ni::Dispatcher)) && *(^s).g == *s.g)]
#[ensures(s.p.init ==> (*(^s).g).wq.len() == 0)]
#[ensures(s.p.init ==> match result.0 {
    Ok(_) => forall<k: u64> mt_has(*(^s).g, k) ==> !(*(^s).g).wf[k],
    Err(e) => e == DE::Io(IoK::Ssd) && exists<k: u64> mt_has(*(^s).g, k) && (*(^s).g).wf[k],
})]
#[ensures(s.p.init ==> forall<i: Int> 0 <= i && i < result.1.len() ==> has(*(^s).g, result.1[i]) && persisted(ent(*(^s).g, result.1[i]).loc))]
#[ensures(forall<k: u64> has(*s.g, k) && persisted(ent(*s.g, k).loc) ==> persisted(ent(*(^s).g, k).loc))]
#[ensures(forall<k: u64> has(*s.g, k) ==> has(*(^s).g, k))]
#[ensures(log_ext(*s.g, *(^s).g) && same_ctr((*s.g).c, (*(^s).g).c) && (*(^s).g).mt == (*s.g).mt)]
pub fn flush_to_ssd(s: &mut S) -> (Result<usize, DE>, Snapshot<Seq<u64>>) {
    match ensure_initialized(s) {
        Err(e) => return (Err(e), snapshot!(Seq::empty())),
        Ok(()) => {}
    }
    let done = if s.p.writer { drain(s) } else { snapshot!(Seq::empty()) };
    // dispatch_map / memory_tier bound: pinv of an initialized dispatcher.
    if pins_failed_writes(s) > 0 {
        return (Err(DE::Io(IoK::Ssd)), done);
    }
    // the entries that now have a valid SSD offset
    let keys = mt_oldest(s, usize::MAX);
    let g1 = snapshot!(*s.g);
    let p1 = snapshot!(s.p);
    let mut n: usize = 0;
    let mut i: usize = 0;
    #[invariant(s.p == *p1 && *s.g == *g1 && n@ <= i@ && i@ <= keys@.len())]
    while i < keys.len() {
        if dm_is_evictable(s, keys[i]) {
            n += 1;
        }
        i += 1;
    }
    (Ok(n), done)
}

// ---------------------------------------------------------------------------
// shutdown (lib.rs:2112)
// ---------------------------------------------------------------------------

/// `for (i, drive) in drives.iter().enumerate() { drive.extent_mgr.checkpoint() }`
#[ensures((^s).p == s.p && *(^s).g == W { log: (*(^s).g).log, ..*s.g })]
#[ensures((*(^s).g).log.len() == (*s.g).log.len() + n@)]
#[ensures(forall<i: Int> 0 <= i && i < (*s.g).log.len() ==> (*(^s).g).log[i] == (*s.g).log[i])]
#[ensures(forall<j: Int> 0 <= j && j < n@ ==> match (*(^s).g).log[(*s.g).log.len() + j] { Ev::Checkpoint { drive } => drive@ == j, _ => false })]
pub fn checkpoint_all(s: &mut S, n: usize) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= n@ && *s.g == W { log: s.g.log, ..*g0 } && s.g.log.len() == g0.log.len() + i@)]
    #[invariant(forall<q: Int> 0 <= q && q < g0.log.len() ==> s.g.log[q] == g0.log[q])]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> match s.g.log[g0.log.len() + j] { Ev::Checkpoint { drive } => drive@ == j, _ => false })]
    while i < n {
        em_checkpoint(s, i);
        i += 1;
    }
}

/// Phase 1: `admin.signal_stop()` for every drive, in order.
#[ensures((^s).p == s.p && *(^s).g == W { log: (*(^s).g).log, ..*s.g })]
#[ensures((*(^s).g).log.len() == (*s.g).log.len() + n@)]
#[ensures(forall<i: Int> 0 <= i && i < (*s.g).log.len() ==> (*(^s).g).log[i] == (*s.g).log[i])]
#[ensures(forall<j: Int> 0 <= j && j < n@ ==> match (*(^s).g).log[(*s.g).log.len() + j] { Ev::DevSignal { drive } => drive@ == j, _ => false })]
pub fn signal_all(s: &mut S, n: usize) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= n@ && *s.g == W { log: s.g.log, ..*g0 } && s.g.log.len() == g0.log.len() + i@)]
    #[invariant(forall<q: Int> 0 <= q && q < g0.log.len() ==> s.g.log[q] == g0.log[q])]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> match s.g.log[g0.log.len() + j] { Ev::DevSignal { drive } => drive@ == j, _ => false })]
    while i < n {
        bd_signal_stop(s, i);
        i += 1;
    }
}

/// Phase 2: `admin.shutdown()` (joins the actor thread) for every drive, in REVERSE order
/// (`drives.iter().enumerate().rev()`).
#[ensures((^s).p == s.p && *(^s).g == W { log: (*(^s).g).log, ..*s.g })]
#[ensures((*(^s).g).log.len() == (*s.g).log.len() + n@)]
#[ensures(forall<i: Int> 0 <= i && i < (*s.g).log.len() ==> (*(^s).g).log[i] == (*s.g).log[i])]
#[ensures(forall<j: Int> 0 <= j && j < n@ ==> match (*(^s).g).log[(*s.g).log.len() + j] { Ev::DevShutdown { drive } => drive@ == n@ - 1 - j, _ => false })]
pub fn shutdown_all_rev(s: &mut S, n: usize) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut t: usize = 0;
    #[invariant(s.p == *p0 && t@ <= n@ && *s.g == W { log: s.g.log, ..*g0 } && s.g.log.len() == g0.log.len() + t@)]
    #[invariant(forall<q: Int> 0 <= q && q < g0.log.len() ==> s.g.log[q] == g0.log[q])]
    #[invariant(forall<j: Int> 0 <= j && j < t@ ==> match s.g.log[g0.log.len() + j] { Ev::DevShutdown { drive } => drive@ == n@ - 1 - j, _ => false })]
    while t < n {
        bd_shutdown(s, n - 1 - t);
        t += 1;
    }
}

/// Phase 3: `admin.detach_controller()` for every drive, in order.
#[ensures((^s).p == s.p && *(^s).g == W { log: (*(^s).g).log, ..*s.g })]
#[ensures((*(^s).g).log.len() == (*s.g).log.len() + n@)]
#[ensures(forall<i: Int> 0 <= i && i < (*s.g).log.len() ==> (*(^s).g).log[i] == (*s.g).log[i])]
#[ensures(forall<j: Int> 0 <= j && j < n@ ==> match (*(^s).g).log[(*s.g).log.len() + j] { Ev::DevDetach { drive } => drive@ == j, _ => false })]
pub fn detach_all(s: &mut S, n: usize) {
    let g0 = snapshot!(*s.g);
    let p0 = snapshot!(s.p);
    let mut i: usize = 0;
    #[invariant(s.p == *p0 && i@ <= n@ && *s.g == W { log: s.g.log, ..*g0 } && s.g.log.len() == g0.log.len() + i@)]
    #[invariant(forall<q: Int> 0 <= q && q < g0.log.len() ==> s.g.log[q] == g0.log[q])]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> match s.g.log[g0.log.len() + j] { Ev::DevDetach { drive } => drive@ == j, _ => false })]
    while i < n {
        bd_detach(s, i);
        i += 1;
    }
}

/// Ghost positions in the log: after the writer drain, after the checkpoints (+unregister).
pub struct ShutPos {
    pub drained: Int,
    pub devs: Int,
}

#[requires(a_us5_q(s.p, (*s.g).wq))]
#[ensures(result.0 == Ok(()))]
#[ensures(!(^s).p.init && !(^s).p.writer && !(^s).p.ssd_ev && !(^s).p.mt_ev && (^s).p.nd@ == 0)]
#[ensures((^s).p.has_dm == s.p.has_dm && (^s).p.has_mt == s.p.has_mt && (^s).p.has_gpu == s.p.has_gpu
          && (^s).p.has_spdk == s.p.has_spdk && (^s).p.has_factory == s.p.has_factory && (^s).p.has_rl == s.p.has_rl
          && (^s).p.has_logger == s.p.has_logger && (^s).p.max_att == s.p.max_att && (^s).p.bp_ms == s.p.bp_ms)]
#[ensures(s.p.writer ==> (*(^s).g).wq.len() == 0)]
#[ensures(!s.p.writer ==> (*(^s).g).wq == (*s.g).wq)]
#[ensures(log_ext(*s.g, *(^s).g) && same_ctr((*s.g).c, (*(^s).g).c))]
#[ensures((*(^s).g).ob == (*s.g).ob && (*(^s).g).ov == (*s.g).ov)]
#[ensures(forall<k: u64> has(*s.g, k) ==> has(*(^s).g, k))]
#[ensures((*s.g).log.len() <= result.1.drained && result.1.drained + s.p.nd@ <= result.1.devs
          && result.1.devs + 3 * s.p.nd@ <= (*(^s).g).log.len())]
#[ensures(forall<i: Int> (*s.g).log.len() <= i && i < result.1.drained ==> !is_cp((*(^s).g).log[i]) && !is_dev((*(^s).g).log[i]))]
#[ensures(forall<j: Int> 0 <= j && j < s.p.nd@ ==> match (*(^s).g).log[result.1.drained + j] { Ev::Checkpoint { drive } => drive@ == j, _ => false })]
#[ensures(forall<i: Int> result.1.drained + s.p.nd@ <= i && i < result.1.devs ==> (*(^s).g).log[i] == Ev::Unregister)]
#[ensures(forall<j: Int> 0 <= j && j < s.p.nd@ ==> match (*(^s).g).log[result.1.devs + j] { Ev::DevSignal { drive } => drive@ == j, _ => false })]
#[ensures(forall<j: Int> 0 <= j && j < s.p.nd@ ==> match (*(^s).g).log[result.1.devs + s.p.nd@ + j] { Ev::DevShutdown { drive } => drive@ == s.p.nd@ - 1 - j, _ => false })]
#[ensures(forall<j: Int> 0 <= j && j < s.p.nd@ ==> match (*(^s).g).log[result.1.devs + 2 * s.p.nd@ + j] { Ev::DevDetach { drive } => drive@ == j, _ => false })]
#[ensures(forall<i: Int> result.1.devs + 3 * s.p.nd@ <= i && i < (*(^s).g).log.len() ==> (*(^s).g).log[i] == Ev::Leave)]
#[ensures(s.p.has_gpu && s.p.has_mt && (*s.g).pool ==> result.1.devs == result.1.drained + s.p.nd@ + 1)]
#[ensures(s.p.has_rl ==> (*(^s).g).log.len() == result.1.devs + 3 * s.p.nd@ + 1)]
pub fn shutdown(s: &mut S) -> (Result<(), DE>, Snapshot<ShutPos>) {
    let g_in = snapshot!(*s.g);
    let n = s.p.nd;
    // bg_evictor / bg_mt_evictor: shutdown() = signal + join (the thread is gone afterwards).
    if s.p.ssd_ev {
        s.p.ssd_ev = false;
    }
    if s.p.mt_ev {
        s.p.mt_ev = false;
    }
    // bg_writer: shutdown() drains every queued job, then joins its threads.
    if s.p.writer {
        let _ = drain(s);
        s.p.writer = false;
    }
    let drained = snapshot!(s.g.log.len());
    checkpoint_all(s, n);
    let cpd = snapshot!(s.g.log.len());
    let lcp = snapshot!(s.g.log);
    proof_assert!(*cpd == *drained + n@);
    if s.p.has_gpu && s.p.has_mt {
        if mt_pool_info(s) {
            gpu_unregister(s);
            proof_assert!(s.g.log.len() == *cpd + 1);
        }
    }
    if s.p.has_gpu {
        // destroy the warm stream and the pipeline ring
        s.p.warm = 0;
        s.p.ring = false;
        s.p.stg = false;
    }
    let devs = snapshot!(s.g.log.len());
    // `std::mem::take(&mut *self.data_drives.write())`
    s.p.nd = 0;
    signal_all(s, n);
    shutdown_all_rev(s, n);
    detach_all(s, n);
    let ldev = snapshot!(s.g.log);
    proof_assert!(forall<i: Int> 0 <= i && i < lcp.len() ==> ldev[i] == lcp[i]);
    if s.p.has_rl {
        let _ = rl_leave(s);
    }
    proof_assert!(forall<i: Int> 0 <= i && i < ldev.len() ==> s.g.log[i] == ldev[i]);
    proof_assert!(forall<j: Int> 0 <= j && j < n@ ==> s.g.log[*drained + j] == lcp[*drained + j]);
    s.p.init = false;
    (Ok(()), snapshot!(ShutPos { drained: *drained, devs: *devs }))
}
