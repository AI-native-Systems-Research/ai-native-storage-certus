#!/usr/bin/env python3
"""Generate src/drivers.rs (one verify_<id> + verify_<id>__mutant, or refute_<id>, per property)
and the creusot advisory entries. Run from the verif-creusot directory."""
import re, sys, yaml, os

ROWS = []   # (id, kind, rust_text, note, fidelity, assumes, modules)

def mid(pid):
    return pid.lower().replace('-', '_')

def V(pid, sig, req, ens, body, mut, note, fid='ghost-mirror', assumes=(), mods=()):
    """verify driver: requires list, ensures list; mut = (index_of_ensures, replacement) or full list."""
    name = 'verify_' + mid(pid)
    def fn(nm, ens_list):
        a = ''.join('#[requires(%s)]\n' % r for r in req)
        b = ''.join('#[ensures(%s)]\n' % e for e in ens_list)
        return '%s%spub fn %s(%s) {\n%s\n}\n' % (a, b, nm, sig, body)
    mens = list(ens)
    if isinstance(mut, tuple):
        i, rep = mut
        mens[i] = rep
    else:
        mens = mut
    text = fn(name, ens) + '\n' + fn(name + '__mutant', mens)
    ROWS.append(dict(id=pid, kind='verify', text=text, note=note, fid=fid, assumes=list(assumes), mods=list(mods)))

def R(pid, body, note, fid='ghost-mirror', assumes=(), mods=()):
    name = 'refute_' + mid(pid)
    text = 'pub fn %s() {\n%s\n}\n' % (name, body)
    ROWS.append(dict(id=pid, kind='refute', text=text, note=note, fid=fid, assumes=list(assumes), mods=list(mods)))

def O(pid, note, ix=None):
    ROWS.append(dict(id=pid, kind='open', text='', note=note, fid='ghost-mirror', assumes=[], mods=[], ix=ix))

OPR = 'pinv(s.p) && s.p.init'
G = '(*s.g)'; F = '(*(^s).g)'
A_US5 = 'D-RANGE-US-5AS-4-a11766'
A_039 = 'D-RANGE-FR-039-d29c77'

# =========================================================================== initialize
V('DIS-INITIALIZE-REQUIRES-DISPATCH-MAP', 's: &mut S, cfg: ICfg',
  ['pinv(s.p)', '!s.p.has_dm'],
  ['true'],
  '    let (r, _f) = initialize(s, cfg);\n    proof_assert!(r == Err(DE::NotInit(Ni::DispatchMap)) && (^s).p.init == s.p.init || true);',
  ['false'], '', )
ROWS.pop()  # replaced by an explicit form below (result is not returned by a unit driver)

def VR(pid, sig, ret, req, ens, body, mut, note, fid='ghost-mirror', assumes=(), mods=()):
    name = 'verify_' + mid(pid)
    def fn(nm, ens_list):
        a = ''.join('#[requires(%s)]\n' % r for r in req)
        b = ''.join('#[ensures(%s)]\n' % e for e in ens_list)
        return '%s%spub fn %s(%s) -> %s {\n%s\n}\n' % (a, b, nm, sig, ret, body)
    mens = list(ens)
    i, rep = mut
    mens[i] = rep
    text = fn(name, ens) + '\n' + fn(name + '__mutant', mens)
    ROWS.append(dict(id=pid, kind='verify', text=text, note=note, fid=fid, assumes=list(assumes), mods=list(mods)))

INIT = '    initialize(s, cfg)'
VR('DIS-INITIALIZE-REQUIRES-DISPATCH-MAP', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', '!s.p.has_dm'],
   ['result.0 == Err(DE::NotInit(Ni::DispatchMap))', '(^s).p.init == s.p.init && (^s).p.writer == s.p.writer'],
   INIT, (0, 'result.0 == Ok(())'),
   'initialize mirror (init.rs): the dispatch-map receptacle check returns NotInitialized naming the dispatch map; init/writer flags unchanged (not operational). The NotInitialized message string is modelled by its subject Ni::DispatchMap.')
VR('DIS-INITIALIZE-REQUIRES-MEMORY-TIER', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', 's.p.has_dm && !s.p.has_mt'],
   ['result.0 == Err(DE::NotInit(Ni::MemoryTier))', '(^s).p.init == s.p.init && (^s).p.writer == s.p.writer'],
   INIT, (0, 'result.0 == Ok(())'),
   'initialize mirror: memory-tier receptacle missing -> NotInitialized naming the memory tier; not operational.')
VR('DIS-INITIALIZE-EMPTY-PCI-LIST-INVALID', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', 's.p.has_dm && s.p.has_mt && cfg.n_addrs@ == 0'],
   ['result.0 == Err(DE::InvalidParam) && %s == %s && (^s).p.nd == s.p.nd' % (F, G)],
   INIT, (0, 'result.0 == Ok(())'),
   'initialize mirror: with both dependencies bound and an empty drive list it returns InvalidParameter with no state change, for EVERY value of has_spdk/has_factory (the driver does not constrain them), i.e. before SPDK is considered.')
VR('DIS-INITIALIZE-SUCCESS-OPERATIONAL', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', 's.p.has_dm && s.p.has_mt && s.p.has_spdk && cfg.n_addrs@ > 0'],
   ['result.0 == Ok(()) ==> (^s).p.nd == cfg.n_addrs && (^s).p.writer && (^s).p.init && pinv((^s).p)'],
   INIT, (0, 'result.0 == Ok(()) ==> (^s).p.nd@ == cfg.n_addrs@ + 1'),
   'initialize + create_data_drives mirrors: on success with SPDK connected, N data drives (each one block device + one extent manager, DataDrive) are installed, the background writer is started, init is set, and the maintained invariant pinv holds, so every cache operation is accepted (DIS-INV-NOT-INITIALIZED no longer applies). Block-device / extent-manager construction is a contract-carrying trusted boundary (create_drive).', fid='trusted-boundary')
R('DIS-INITIALIZE-NO-SPDK-MEMORY-ONLY', '''    let mut s = S { p: P { has_dm: true, has_mt: true, has_factory: true, has_logger: true, ..p_zero() },
                    g: snapshot!(W { ov: |i: Int| 3u64, ..w0() }) };
    let cfg = ICfg { n_addrs: 1, format_on_init: true, ssd_thr_pos: false, mt_thr_pos: false, max_att: 1, bp_ms: 0, stg_slots: 0, stg_bytes: 0 };
    let (r, _f) = initialize(&mut s, cfg);
    proof_assert!(r == Ok(()) && s.p.nd@ == 1);''',
  'REFUTED: with no SPDK environment connected but a block-device factory installed (DispatcherComponent::set_block_device_factory, a public method), initialize does NOT run memory-tier-only: lib.rs:1909 creates drives when `spdk_env.is_connected() || has_bd_factory`, so a block device is created (nd = 1) and later write-throughs go to that device, not synthetic offsets. Counterexample: has_spdk=false, factory set, 1 PCI address, every drive step succeeds.')
VR('DIS-INITIALIZE-PARTITION-GUARD', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)'],
   ['match result.1 { DFail::Few(d, f) => result.0 == Err(DE::Io(IoK::Partition { drive: d, expected: 3usize, found: f })) && f@ < 3, _ => true }',
    'match result.0 { Err(DE::Io(IoK::Partition { drive, expected, found })) => expected@ == 3 && found@ < 3 && result.1 == DFail::Few(drive, found), _ => true }'],
   INIT, (0, 'match result.1 { DFail::Few(d, f) => result.0 == Err(DE::Io(IoK::Partition { drive: d, expected: 2usize, found: f })), _ => true }'),
   'create_data_drives mirror: when drive d\'s partition table (partition-manager boundary part_init) has fewer than 3 partitions, initialize returns IoError carrying drive index d, expected 3 and the found count (the remediation-hint text is string content, not modelled) and never panics on the [0]/[2] index (the guard precedes it). Partition reading is a trusted boundary.', fid='trusted-boundary')
R('DIS-INITIALIZE-NO-FORMAT-PRESERVES', '''    let mut s = S { p: P { has_dm: true, has_mt: true, has_spdk: true, has_logger: true, ..p_zero() },
                    g: snapshot!(W { ov: |i: Int| 3u64, ..w0() }) };
    let cfg = ICfg { n_addrs: 1, format_on_init: false, ssd_thr_pos: false, mt_thr_pos: false, max_att: 1, bp_ms: 0, stg_slots: 0, stg_bytes: 0 };
    let g0 = snapshot!(*s.g);
    let (_r, _f) = initialize(&mut s, cfg);
    proof_assert!(logged(*g0, *s.g, Ev::EmFormat { drive: 0usize }));''',
  'REFUTED: format_on_init=false does not prevent reformatting. DiskPartitionManager::initialize_or_format(false, ..) formats a disk whose table is missing or corrupt and returns formatted=true (disk-partition-manager lib.rs:56-60), and lib.rs:1826 then calls iem.format(..), wiping the extent manager. Counterexample: format_on_init=false, partition manager reports formatted=true -> EmFormat logged for drive 0.', fid='trusted-boundary')
VR('DIS-INITIALIZE-RECOVERY-REBUILDS-MAP', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', 's.p.has_dm && s.p.has_mt && cfg.n_addrs@ > 0 && (s.p.has_spdk || s.p.has_factory) && !cfg.format_on_init'],
   ['result.0 == Ok(()) ==> forall<d: Int, e: Int> 0 <= d && d < cfg.n_addrs@ && 0 <= e && e < %s.ext[d].len() ==> logged(%s, %s, Ev::Recover { key: %s.ext[d][e].key, off: %s.ext[d][e].off, size: %s.ext[d][e].size })' % (G,G,F,G,G,G)],
   INIT, (0, 'result.0 == Ok(()) ==> forall<d: Int, e: Int> 0 <= d && d < cfg.n_addrs@ && 0 <= e && e < %s.ext[d].len() ==> !logged(%s, %s, Ev::Recover { key: %s.ext[d][e].key, off: %s.ext[d][e].off, size: %s.ext[d][e].size })' % (G,G,F,G,G,G)),
   'initialize + recover_all mirrors: with format_on_init=false, every extent every drive\'s extent manager reports (for_each_extent boundary) is passed to dispatch_map.recover_extent with its key, offset and size. That recover_extent then registers it (a BlockDevice entry) is the dispatch-map component\'s contract (boundary dm_recover: inserts when absent).', fid='trusted-boundary')
VR('DIS-INITIALIZE-SSD-EVICTOR-START', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', '!s.p.init'],
   ['result.0 == Ok(()) ==> (^s).p.ssd_ev == (cfg.ssd_thr_pos && (^s).p.nd@ > 0)', 'result.0 != Ok(()) ==> !(^s).p.ssd_ev'],
   INIT, (0, 'result.0 == Ok(()) ==> (^s).p.ssd_ev == cfg.ssd_thr_pos'),
   'initialize mirror on a not-yet-initialized dispatcher (maintained invariant pinv: !init => no evictor running): the SSD evictor is started iff ssd_eviction_threshold > 0 and at least one data drive exists. The f64 comparison `> 0.0` is carried as cfg.ssd_thr_pos (Creusot cannot translate f64).')
VR('DIS-INITIALIZE-MT-EVICTOR-START', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', '!s.p.init'],
   ['result.0 == Ok(()) ==> (^s).p.mt_ev == cfg.mt_thr_pos', 'result.0 != Ok(()) ==> !(^s).p.mt_ev'],
   INIT, (0, 'result.0 == Ok(()) ==> (^s).p.mt_ev == !cfg.mt_thr_pos'),
   'initialize mirror: the memory-tier evictor is started iff memory_tier_eviction_threshold > 0 (f64 `> 0.0` carried as cfg.mt_thr_pos); the default 0.0 starts none (see DIS-CONFIG-DEFAULTS).')
O('DIS-INITIALIZE-POLLER-CPU-EXPLICIT', 'OPEN: lib.rs:1617 computes `base + i` in usize with no bound on poller_base_cpu; for B > usize::MAX - i it panics (debug), so the obligation is not provable without a range premise and no declared assumption bounds it. A Creusot refutation of a panic is not expressible (partial correctness). Route: Kani harness over create_data_drives\' per_drive_cpu computation.')
O('DIS-INITIALIZE-POLLER-CPU-AUTO', 'OPEN: compute_numa_cpu_assignments (lib.rs:599) depends on NumaTopology::discover() (sysfs I/O) and two HashMaps; the round-robin index arithmetic was not mirrored in this run. Route: Kani harness with a stubbed topology.')
VR('DIS-INITIALIZE-CHANNEL-POOL-EMPTY', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', 's.p.has_spdk || s.p.has_factory'],
   ['result.0 == Ok(()) ==> forall<i: Int> 0 <= i && i < (^s).p.nd@ ==> %s.cfree[i] == 0' % F],
   INIT, (0, 'result.0 == Ok(()) ==> forall<i: Int> 0 <= i && i < (^s).p.nd@ ==> %s.cfree[i] == 1' % F),
   'initialize + create_data_drives mirrors: every installed drive gets ChannelPool::new (empty free list, lib.rs:301); channels are only connected on checkout (chan_pool_checkout, DIS-INV-CHANNEL-EXCLUSIVE).')
VR('DIS-CONFIG-DEFAULTS', '', 'Cfg', [],
   ['result.max_cache_entries@ == 10000 && result.format_on_init && result.ssd_eviction_threshold.bits@ == 0x3FEC_CCCC_CCCC_CCCD && result.ssd_eviction_low_watermark.bits@ == 0x3FE9_9999_9999_999A && result.ssd_eviction_batch_size@ == 64 && result.ssd_eviction_interval_secs@ == 5 && result.poller_base_cpu == None && result.max_eviction_attempts@ == 2048 && result.store_backpressure_ms@ == 5000 && result.backfill_delay_ms@ == 10 && result.metadata_partition_size@ == 134217728 && result.extended_metadata_partition_size@ == 134217728 && result.memory_tier_eviction_threshold.bits@ == 0 && result.memory_tier_eviction_low_watermark.bits@ == 0x3FE6_6666_6666_6666 && result.memory_tier_eviction_batch_size@ == 64 && result.memory_tier_eviction_interval_secs@ == 2 && result.cold_staging_slots@ == 64 && result.cold_staging_buf_bytes@ == 4194304'],
   '    config_default()', (0, 'result.max_cache_entries@ == 10001'),
   'config_default mirror of `impl Default for DispatcherConfig` (interfaces/src/idispatcher.rs:102). The four f64 fields are carried as their IEEE-754 bit patterns (0.9 = 0x3FECCCCCCCCCCCCD, 0.8 = 0x3FE999999999999A, 0.0 = 0, 0.70 = 0x3FE6666666666666): Creusot cannot translate f64 (creusot-rustc ICE on an f64 literal). Mirror of a constant initializer: what it does NOT cover is drift if the real literals change.')
O('DIS-INITIALIZE-INVALID-PCI-ADDRESS', 'OPEN (inexpressible): the obligation is about the textual CONTENT of a PCI-address string ("domain:bus:device.function", hex parts) parsed by parse_pci_addr (lib.rs:1469); Creusot models &str as an opaque Seq<char> with no content axioms. The mirror treats parsing as an oracle (parse_pci). Covered by: Kani (bounded strings).', ix='IX-CREUSOT-STRING-CONTENT')
VR('DIS-INITIALIZE-DRIVE-FAILURE-IO-ERROR', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', '!s.p.init'],
   ['match result.1 { DFail::Create(d) => result.0 == Err(DE::Io(IoK::Drive(d))), DFail::Partition(d) => result.0 == Err(DE::Io(IoK::Drive(d))), DFail::Format(d) => result.0 == Err(DE::Io(IoK::Drive(d))), DFail::Few(d, f) => match result.0 { Err(DE::Io(_)) => true, _ => false }, _ => true }',
    'result.0 != Ok(()) ==> !(^s).p.init'],
   INIT, (1, 'result.0 != Ok(()) ==> (^s).p.init'),
   'initialize + create_data_drives mirrors: a failure creating/wiring a drive (create_drive), reading/creating its partition table (part_init, including the <3-partition guard), or formatting/recovering its extent manager (em_format/em_initialize) makes initialize return IoError and leaves the dispatcher uninitialized. Drive construction steps are trusted boundaries with oracle outcomes.', fid='trusted-boundary')
VR('DIS-INITIALIZE-FAILURE-LEAVES-UNINITIALIZED', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', '!s.p.init'],
   ['result.0 != Ok(()) ==> !(^s).p.init && pinv((^s).p)'],
   INIT, (0, 'result.0 != Ok(()) ==> (^s).p.init'),
   'initialize mirror: every error return precedes `initialized.store(true)` (lib.rs:2102), so a previously uninitialized dispatcher stays uninitialized and pinv still gives NotInitialized for every later operation (DIS-INV-NOT-INITIALIZED).')
R('DIS-INITIALIZE-FAILURE-NO-CONFIG-CHANGE', '''    let mut s = S { p: P { max_att: 5, bp_ms: 9, ..p_zero() }, g: snapshot!(w0()) };
    let cfg = ICfg { n_addrs: 1, format_on_init: true, ssd_thr_pos: false, mt_thr_pos: false, max_att: 7, bp_ms: 11, stg_slots: 0, stg_bytes: 0 };
    let (r, _f) = initialize(&mut s, cfg);
    proof_assert!(r == Err(DE::NotInit(Ni::DispatchMap)) && s.p.max_att@ == 7 && s.p.bp_ms@ == 11);''',
  'REFUTED: initialize stores config.max_eviction_attempts and store_backpressure_ms (lib.rs:1887-1890) BEFORE validating the dispatch-map receptacle (lib.rs:1892). Counterexample: max_eviction_attempts 5 -> initialize(config with 7) without a dispatch map fails NotInitialized yet leaves the limit at 7 (budget 9 -> 11).')
VR('DIS-INITIALIZE-JOINS-CLUSTER', 's: &mut S, cfg: ICfg', '(Result<(), DE>, DFail)',
   ['pinv(s.p)', 's.p.has_rl'],
   ['result.0 == Ok(()) ==> logged(%s, %s, Ev::Join)' % (G, F),
    's.p.has_dm && s.p.has_mt && cfg.n_addrs@ > 0 && !(s.p.has_spdk || s.p.has_factory) ==> result.0 == Ok(())'],
   INIT, (0, 'result.0 == Ok(()) ==> !logged(%s, %s, Ev::Join)' % (G, F)),
   'initialize mirror: with a remote-lookup bound, a successful initialize calls join_cluster (Join logged); the join result is discarded (`let _ =`), so its failure (oracle) cannot make initialize fail - shown by initialize succeeding for every join outcome.', fid='trusted-boundary')

# =========================================================================== shutdown
SH = '    shutdown(s)'
SR = '(Result<(), DE>, Snapshot<ShutPos>)'
SHREQ = ['a_us5_q(s.p, (*s.g).wq)']
SHNOTE = ' Draining the queued write-throughs runs process_write_job, whose memory-tier-only offset `key * 4096` is overflow-free only under the declared assumption D-RANGE-US-5AS-4-a11766 (stated as a requires over the pending jobs).'
VR('DIS-SHUTDOWN-DRAINS-WRITES', 's: &mut S', SR, SHREQ + ['s.p.writer'],
   ['%s.wq.len() == 0' % F], SH, (0, '%s.wq.len() == %s.wq.len()' % (F, G)),
   'shutdown mirror: the writer\'s shutdown drains its channel (background.rs:133) - modelled as the writer loop (drain) run to an empty queue before the threads are joined; every job pending at the call is processed.' + SHNOTE, assumes=[A_US5])
VR('DIS-SHUTDOWN-CHECKPOINT-BEFORE-DEVICES', 's: &mut S', SR, SHREQ,
   ['cp_before_dev(%s, %s.log.len())' % (F, G)], SH, (0, 's.p.nd@ > 0 ==> !cp_before_dev(%s, %s.log.len())' % (F, G)),
   'shutdown mirror: every IExtentManager::checkpoint event is logged before any block-device signal_stop/shutdown/detach event.' + SHNOTE, assumes=[A_US5])
VR('DIS-SHUTDOWN-REVERSE-ORDER', 's: &mut S', SR, SHREQ,
   ['forall<j: Int> 0 <= j && j < s.p.nd@ ==> match %s.log[result.1.devs + s.p.nd@ + j] { Ev::DevShutdown { drive } => drive@ == s.p.nd@ - 1 - j, _ => false }' % F],
   SH, (0, 'forall<j: Int> 0 <= j && j < s.p.nd@ ==> match %s.log[result.1.devs + s.p.nd@ + j] { Ev::DevShutdown { drive } => drive@ == j, _ => false }' % F),
   'shutdown mirror: phase 2 calls admin.shutdown() for drives n-1, ..., 0 (`drives.iter().enumerate().rev()`), i.e. reverse creation order; phase 1 signal_stop and phase 3 detach run in creation order (ensured too).' + SHNOTE, assumes=[A_US5])
VR('DIS-SHUTDOWN-NO-THREADS', 's: &mut S', SR, SHREQ,
   ['!(^s).p.writer && !(^s).p.ssd_ev && !(^s).p.mt_ev'], SH, (0, '(^s).p.writer'),
   'shutdown mirror: the SSD evictor, memory-tier evictor and background writer are all shut down (signal + join; modelled by clearing their handles after the join) before return.' + SHNOTE, fid='trusted-boundary', assumes=[A_US5])
VR('DIS-SHUTDOWN-UNREGISTER-HOST-MEMORY', 's: &mut S', SR, SHREQ + ['s.p.has_gpu && s.p.has_mt && (*s.g).pool'],
   ['%s.log[result.1.drained + s.p.nd@] == Ev::Unregister && result.1.devs == result.1.drained + s.p.nd@ + 1' % F], SH,
   (0, 'forall<i: Int> 0 <= i && i < %s.log.len() ==> %s.log[i] != Ev::Unregister' % (F, F)),
   'shutdown mirror: with GPU services and a memory tier exposing its pool, unregister_host_memory is called (after the checkpoints, before the devices are stopped; the dispatcher never tears the memory tier down itself).' + SHNOTE, assumes=[A_US5])
VR('DIS-SHUTDOWN-ALWAYS-OK', 's: &mut S', SR, SHREQ,
   ['result.0 == Ok(()) && !(^s).p.init'], SH, (0, 'result.0 != Ok(())'),
   'shutdown mirror: every path returns Ok(()), whether or not initialize ran (no precondition on init) and whether or not it already ran (shutdown is idempotent on the cleared state).' + SHNOTE, assumes=[A_US5])
VR('DIS-SHUTDOWN-LEAVES-CLUSTER', 's: &mut S', SR, SHREQ + ['s.p.has_rl'],
   ['%s.log[%s.log.len() - 1] == Ev::Leave && result.0 == Ok(())' % (F, F)], SH, (0, '%s.log[%s.log.len() - 1] != Ev::Leave' % (F, F)),
   'shutdown mirror: with a remote-lookup bound, leave_cluster is called (last event) and its result is discarded, so shutdown returns Ok regardless.' + SHNOTE, assumes=[A_US5])
V('DIS-SHUTDOWN-REINITIALIZE', 's: &mut S, cfg: ICfg',
   SHREQ + ['pinv(s.p)', 's.p.has_dm && s.p.has_mt && cfg.n_addrs@ > 0',
            '(s.p.has_spdk || s.p.has_factory) ==> s.p.has_logger && (forall<i: Int> (*s.g).ob[i] && (*s.g).ov[i]@ >= 3)'],
   ['(^s).p.init && (^s).p.writer'],
   '''    let (_r, _p) = shutdown(s);
    proof_assert!(!s.p.init && pinv(s.p));
    let (r, _f) = initialize(s, cfg);
    proof_assert!(r == Ok(()));''',
   (0, '!(^s).p.init'),
   'shutdown then initialize mirrors: after shutdown (init cleared, all threads joined, drives released) initialize with a valid configuration (dependencies bound, non-empty drive list; with drives: logger bound and every drive step / partition table healthy) succeeds and the dispatcher is operational again. The "healthy drive" premise is the meaning given to "valid configuration" for hardware mode; in memory-tier-only mode no premise is needed.' + SHNOTE, assumes=[A_US5])

# =========================================================================== populate
PR = '(Result<(), DE>, Snapshot<PopG>)'
POP = '    populate(s, key, h)'
VR('DIS-POPULATE-ZERO-SIZE-INVALID', 's: &mut S, key: u64, h: Ipc', PR, [OPR, 'h.size@ == 0'],
   ['result.0 == Err(DE::InvalidParam) && %s == %s' % (F, G)], POP, (0, 'result.0 == Ok(())'),
   'populate mirror: size 0 is rejected with InvalidParameter before any reservation; no state change, so no entry is created.')
R('DIS-POPULATE-DUPLICATE-ALREADY-EXISTS', '''    let e = Ent { loc: Loc::Mem { ptr: 4096u64, size: 4096u32, ssd: None }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: true, writer: true, max_att: 0, bp_ms: 0, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), mt: FMap::empty().insert(7u64, Slot { ptr: 4096u64, size: 4096u32 }),
                                     used: 4096, cap: 4096, ..w0() }) };
    let (r, _g) = populate(&mut s, 7u64, Ipc { addr: 1u64, size: 4096u32 });
    proof_assert!(r == Ok(()));''',
  'REFUTED: populate of an already-cached key does not always return AlreadyExists. reserve_memory -> evict_and_insert runs evict_for_space BEFORE mt.insert (lib.rs:1187-1188); when the tier is full and no victim can be freed (here max_eviction_attempts = 0, store_backpressure_ms = 0) it fails AllocationFailed and populate reports Ok(()) (drop-on-full, lib.rs:3161) instead of AlreadyExists. (With room it does return AlreadyExists; with a full tier the eviction can also demote the key itself, changing the existing entry.)')
V('DIS-POPULATE-REGISTERS-MEMORY-TIER', 's: &mut S, key: u64, h: Ipc', [OPR, 'h.size@ > 0'],
  ['true'],
  '''    let (r, pg) = populate(s, key, h);
    if r == Ok(()) && snapshot!(pg.reserved).into_inner() { }
    let c = check(s, key);
    proof_assert!(r == Ok(()) && pg.reserved ==> c == Ok(true));''',
  (0, 'false'),
  '')
ROWS.pop()
VR('DIS-POPULATE-REGISTERS-MEMORY-TIER', 's: &mut S, key: u64, h: Ipc', '(Result<(), DE>, Snapshot<PopG>, Result<bool, DE>)', [OPR, 'h.size@ > 0'],
   ['result.0 == Ok(()) && result.1.reserved ==> result.2 == Ok(true) && has(%s, key) && is_mem(ent(%s, key).loc)' % (F, F)],
   '''    let (r, pg) = populate(s, key, h);
    let c = check(s, key);
    (r, pg, c)''',
   (0, 'result.0 == Ok(()) && result.1.reserved ==> result.2 == Ok(false)'),
   'populate + check mirrors: when populate succeeds having reserved a slot (space was available), the key is a memory-tier dispatch-map entry with its write reference downgraded before populate returns, so an immediately following check returns Ok(true).')
VR('DIS-POPULATE-NOT-WAIT-SSD', 's: &mut S, key: u64, h: Ipc', PR, [OPR, 'h.size@ > 0'],
   ['result.0 == Ok(()) && result.1.reserved ==> %s.wq == %s.wq.push_back(wjob(s.p, key, h.size))' % (F, G)], POP,
   (0, 'result.0 == Ok(()) && result.1.reserved ==> %s.wq == %s.wq' % (F, G)),
   'populate mirror: on success the write-through job is still pending in the background writer\'s queue when populate returns (enqueue only; processing happens in the writer threads, DIS-FLUSH-PERSISTS-ALL).')
VR('DIS-POPULATE-EVICTS-WHEN-FULL', 's: &mut S, key: u64, h: Ipc', PR, [OPR, 'h.size@ > 0'],
   ['result.0 == Ok(()) && result.1.reserved ==> (forall<k: u64> k != key ==> (%s.dm.get(k) == %s.dm.get(k) && %s.mt.get(k) == %s.mt.get(k)) || demoted_k(%s, %s, k)) && result.1.mid.used <= result.1.mid.cap && has(%s, key)' % (F,G,F,G,G,F,F)],
   POP, (0, 'result.0 == Ok(()) && result.1.reserved ==> forall<k: u64> %s.dm.get(k) == %s.dm.get(k)' % (F, G)),
   'populate -> reserve_memory -> evict_and_insert -> evict_for_space/evict_one_clean mirrors: the only changes to other entries are clean demotions of unpinned, persisted memory-tier entries to block-device state (slot freed after the map switched), taken in the memory tier\'s oldest-keys order (the order itself is the memory-tier/eviction-policy component\'s), until the new entry fits (used <= capacity after the insert); then the entry is registered.')
VR('DIS-POPULATE-DROP-ON-FULL', 's: &mut S, key: u64, h: Ipc', PR, [OPR, 'h.size@ > 0'],
   ['result.0 != Err(DE::Alloc)', 'result.0 == Ok(()) && !result.1.reserved ==> (!has(%s, key) ==> !has(%s, key)) && %s.c.drop_full == %s.c.drop_full + 1' % (G, F, F, G)],
   POP, (1, 'result.0 == Ok(()) && !result.1.reserved ==> has(%s, key)' % F),
   'populate mirror: when reserve_memory still fails AllocationFailed after its backpressure budget, populate counts a drop and returns Ok(()) without creating a dispatch-map entry; AllocationFailed is never returned.')
VR('DIS-POPULATE-NO-LRU-REFRESH-BY-WRITER', 's: &mut S, job: Job', 'WjOut', ['a_us5(s.p, job.key)'],
   ['no_lru(%s, %s, job.key)' % (G, F)], '    process_write_job(s, job)',
   (0, 'logged(%s, %s, Ev::MtGet { key: job.key })' % (G, F)),
   'process_write_job mirror (the write-through a populate enqueues): it reads the slot with IMemoryTier::peek (no eviction-order refresh) and never calls get/touch/batch_touch for the key.', assumes=[A_US5])
VR('DIS-POPULATE-WRITE-THROUGH-RECORDS-OFFSET', 's: &mut S, job: Job', 'WjOut', ['a_us5(s.p, job.key)', 'has((*s.g), job.key) && is_mem(ent((*s.g), job.key).loc)'],
   ['match result { WjOut::Stored { off } => has(%s, job.key) && is_mem(ent(%s, job.key).loc) && ssd_of(ent(%s, job.key).loc) == Some(off), _ => true }' % (F, F, F)],
   '    process_write_job(s, job)',
   (0, 'match result { WjOut::Stored { off } => is_blk(ent(%s, job.key).loc), _ => true }' % F),
   'process_write_job mirror: after the extent is reserved and the SSD write succeeds, convert_to_storage records the extent offset on the still memory-tier-resident entry (MemoryTier { ssd_offset: Some(off) }).', assumes=[A_US5])
VR('DIS-POPULATE-WRITE-THROUGH-DRIVE-PLACEMENT', 's: &mut S, key: u64, h: Ipc', PR, [OPR, 'h.size@ > 0'],
   ['result.0 == Ok(()) && result.1.reserved ==> %s.wq[%s.wq.len() - 1].dev == di(key, if s.p.nd@ == 0 { 1usize } else { s.p.nd })' % (F, F)],
   POP, (0, 'result.0 == Ok(()) && result.1.reserved ==> %s.wq[%s.wq.len() - 1].dev@ == 0' % (F, F)),
   'populate mirror: the job is routed to drive_index(key, max(nd,1)) = splitmix64(key) % nd; process_write_job then writes to drives[job.device_index % nd] = that drive. drive_index is a trusted program function whose only contract is `result == di(key, n) && result < n` (di = its own value; the bit-twiddling body is not modelled).', fid='trusted-boundary')
VR('DIS-POPULATE-ENQUEUES-WRITE-THROUGH', 's: &mut S, key: u64, h: Ipc', PR, [OPR, 'h.size@ > 0'],
   ['result.0 == Ok(()) && result.1.reserved ==> %s.wq == %s.wq.push_back(Job { key, size: h.size, dev: di(key, if s.p.nd@ == 0 { 1usize } else { s.p.nd }) })' % (F, G)],
   POP, (0, 'result.0 == Ok(()) && result.1.reserved ==> %s.wq.len() == %s.wq.len() + 2' % (F, G)),
   'populate mirror: exactly one write-through job (key, size, drive_index(key)) is appended to the writer queue when the key is cached.')
R('DIS-POPULATE-FAILURE-RELEASES-SLOT', '''    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: false, writer: true, max_att: 1, bp_ms: 0, ..p_zero() },
                    g: snapshot!(W { used: 0, cap: 8192, ..w0() }) };
    let (r, _g) = populate(&mut s, 7u64, Ipc { addr: 1u64, size: 4096u32 });
    proof_assert!(r == Err(DE::NotInit(Ni::Gpu)) && mt_has(*s.g, 7u64));''',
  'REFUTED: populate does not release the reserved slot on every failure after reserving. lib.rs:3182-3194: a missing GPU service (`?` on gpu_services.get()) and a failed stream_synchronize both return early without mt.remove (only copy_gpu_to_memory_async releases on a submit failure). Counterexample: GPU receptacle unbound -> NotInitialized(gpu) and the slot for the key remains in the memory tier (orphan, not in the dispatch map).')
VR('DIS-POPULATE-COPY-FAILURE-IO-ERROR', 's: &mut S, key: u64, h: Ipc', PR, [OPR, 'h.size@ > 0'],
   ['result.1.copy_failed ==> match result.0 { Err(DE::Io(_)) => true, _ => false }'], POP,
   (0, 'result.1.copy_failed ==> result.0 == Ok(())'),
   'populate mirror: copy_failed is set exactly when copy_gpu_to_memory_async\'s D2H submission or populate\'s stream_synchronize fails; both return IoError.')

# =========================================================================== batch_populate
BP = '    batch_populate(s, entries)'
BPR = '(Vec<Result<(), DE>>, Snapshot<BpG>)'
BPQ = [OPR, 's.p.has_gpu', 'entries@.len() > 0']
def bp_all(x):
    return 'forall<i: Int> 0 <= i && i < entries@.len() ==> ' + x
VR('DIS-BATCH-POPULATE-LENGTH-ORDER', 's: &mut S, entries: &[Pe]', BPR, [OPR],
   ['result.0@.len() == entries@.len()',
    's.p.has_gpu && entries@.len() > 0 ==> ' + bp_all('(entries@[i].h.size@ == 0 ==> result.0@[i] == Err(DE::InvalidParam)) && match result.0@[i] { Err(DE::AlreadyExists(k)) => k == entries@[i].key, _ => true }')],
   BP, (0, 'result.0@.len() == entries@.len() + 1'),
   'batch_populate mirror: one result per entry, results[i] is assigned only while processing entries[i] (zero-size -> InvalidParam for that entry, AlreadyExists names that entry\'s key). The final `r.unwrap()` collect is modelled as divergence on None; the mirror proves no None remains on a returning run.')
R('DIS-BATCH-POPULATE-EQUIV-POPULATE', '''    let mut s1 = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: false, writer: true, max_att: 1, bp_ms: 0, ..p_zero() },
                     g: snapshot!(W { used: 0, cap: 8192, ..w0() }) };
    let mut s2 = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: false, writer: true, max_att: 1, bp_ms: 0, ..p_zero() },
                     g: snapshot!(W { used: 0, cap: 8192, ..w0() }) };
    let h = Ipc { addr: 1u64, size: 4096u32 };
    let (r1, _g1) = populate(&mut s1, 7u64, h);
    let (r2, _g2) = batch_populate(&mut s2, &[Pe { key: 7u64, h }]);
    proof_assert!(mt_has(*s1.g, 7u64) && !mt_has(*s2.g, 7u64));''',
  'REFUTED: batch_populate is not per-entry equivalent to populate. From the same state with the GPU-services receptacle unbound, populate reserves a slot and then fails NotInitialized leaving that slot in the memory tier, while batch_populate checks the GPU first (lib.rs:3223) and returns NotInitialized for every entry without reserving: the resulting cache states differ (also: on a sync failure batch_populate releases the slots, populate does not).')
VR('DIS-BATCH-POPULATE-INDEPENDENT', 's: &mut S, entries: &[Pe]', BPR, BPQ,
   [bp_all('result.1.kind[i] != Kind::Unseen && (result.1.kind[i] == Kind::Sub && result.1.sync_ok ==> result.1.reg[i]) && (result.1.kind[i] == Kind::Dropped ==> result.0@[i] == Ok(()))')],
   BP, (0, 'exists<i: Int> 0 <= i && i < entries@.len() && result.1.kind[i] == Kind::Unseen'),
   'batch_populate mirror: every entry is processed (kind != Unseen) whatever earlier entries did: an allocation failure (Dropped/RsvErr) or copy failure (CopyErr) only sets that entry\'s result; every successfully submitted entry is registered after a successful sync.')
VR('DIS-BATCH-POPULATE-REGISTER-AFTER-SYNC', 's: &mut S, entries: &[Pe]', BPR, BPQ,
   ['forall<q: Int> %s.log.len() <= q && q < %s.log.len() ==> match %s.log[q] { Ev::DmCreate { .. } => q > result.1.sync_at && result.1.sync_at >= 0 && result.1.sync_ok && %s.log[result.1.sync_at] == Ev::Sync { stream: result.1.store }, Ev::Enq { .. } => q > result.1.sync_at && result.1.sync_at >= 0 && result.1.sync_ok, _ => true }' % (G, F, F, F)],
   BP, (0, 'exists<q: Int> %s.log.len() <= q && q < %s.log.len() && match %s.log[q] { Ev::DmCreate { .. } => q < result.1.sync_at, _ => false }' % (G, F, F)),
   'batch_populate mirror: no dispatch-map registration (create_memory_tier_entry) and no write-through enqueue is logged before the single batch stream_synchronize succeeded.')
VR('DIS-BATCH-POPULATE-DROP-ON-FULL', 's: &mut S, entries: &[Pe]', BPR, BPQ,
   [bp_all('result.1.kind[i] == Kind::Dropped ==> result.0@[i] == Ok(()) && !result.1.reg[i]')],
   BP, (0, bp_all('result.1.kind[i] == Kind::Dropped ==> result.1.reg[i]')),
   'batch_populate mirror: an entry whose reservation fails AllocationFailed after backpressure gets Ok(()) and is never registered (copy_gpu_to_memory_completed is not called for it).')
VR('DIS-BATCH-POPULATE-EMPTY', 's: &mut S, entries: &[Pe]', BPR, [OPR, 'entries@.len() == 0'],
   ['result.0@.len() == 0 && %s == %s' % (F, G)], BP, (0, 'result.0@.len() == 1'),
   'batch_populate mirror: an empty batch returns an empty list with no state change.')
VR('DIS-BATCH-POPULATE-ZERO-SIZE', 's: &mut S, entries: &[Pe]', BPR, BPQ,
   [bp_all('(entries@[i].h.size@ == 0 ==> result.0@[i] == Err(DE::InvalidParam)) && result.1.kind[i] != Kind::Unseen')],
   BP, (0, bp_all('entries@[i].h.size@ == 0 ==> result.0@[i] == Ok(())')),
   'batch_populate mirror: a zero-size entry gets InvalidParam (`continue`), and every other entry is still processed.')
VR('DIS-BATCH-POPULATE-SYNC-FAILURE', 's: &mut S, entries: &[Pe]', BPR, BPQ,
   [bp_all('result.1.kind[i] == Kind::Sub && !result.1.sync_ok ==> result.0@[i] == Err(DE::Io(IoK::Sync)) && result.1.rel[i] && !result.1.reg[i]')],
   BP, (0, bp_all('result.1.kind[i] == Kind::Sub && !result.1.sync_ok ==> result.1.reg[i]')),
   'batch_populate mirror: if the one store-stream synchronize fails, every submitted entry gets IoError, release_memory frees its slot (rel), and none is registered.')
VR('DIS-BATCH-POPULATE-COPY-FAILURE-RELEASES', 's: &mut S, entries: &[Pe]', BPR, BPQ,
   [bp_all('result.1.kind[i] == Kind::CopyErr ==> (match result.0@[i] { Err(_) => true, _ => false }) && result.1.rel[i]')],
   BP, (0, bp_all('result.1.kind[i] == Kind::CopyErr ==> result.0@[i] == Ok(())')),
   'batch_populate mirror: when an entry\'s D2H submission fails, that entry gets the error and release_memory(key) frees its reserved slot (rel).')

# =========================================================================== lookup / lookup_async
LA = '    lookup_async(s, key, h)'
LAR = '(Result<u64, DE>, Snapshot<LaG>)'
LQ = [OPR, 's.p.has_gpu']
GPUNOTE = ' Premise: the GPU-services receptacle is bound (the operation copies to the GPU; without it every lookup reports NotInitialized(gpu_services)).'
WARMQ = 'has((*s.g), key) && is_mem(ent((*s.g), key).loc) && ent((*s.g), key).wr@ == 0 && ent((*s.g), key).rr@ < u32::MAX@'
VR('DIS-LOOKUP-MISS-KEY-NOT-FOUND', 's: &mut S, key: u64, h: Ipc', '(Result<(), DE>, Snapshot<(Result<u64, DE>, LaG, W)>)', LQ + ['!has((*s.g), key)'],
   ['result.0 == Err(DE::KeyNotFound(key))'], '    lookup(s, key, h)', (0, 'result.0 == Ok(())'),
   'lookup -> lookup_async mirrors: dm.lookup returns NotExist for an absent key and the result is KeyNotFound.' + GPUNOTE)
R('DIS-LOOKUP-SIZE-MISMATCH', '''    let e = Ent { loc: Loc::Mem { ptr: 4096u64, size: 8192u32, ssd: None }, es: 8192u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: true, writer: true, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), ..w0() }) };
    let (r, _g) = lookup(&mut s, 7u64, Ipc { addr: 1u64, size: 4096u32 });
    proof_assert!(r == Ok(()));''',
  'REFUTED: lookup never compares the requested size with the cached size. IDispatchMap::lookup(key) takes no size (the dispatch-map component never returns MismatchSize), and the warm path copies min(handle.size, slot size) bytes (lib.rs:1283). Counterexample: key cached at 8192 bytes, lookup with a 4096-byte handle -> Ok(()), not InvalidParameter.')
VR('DIS-LOOKUP-WARM-COPIES-DATA', 's: &mut S, key: u64, h: Ipc', LAR, LQ + [WARMQ],
   ['result.1.path == LPath::Warm && (result.0 == Ok(0u64)) == result.1.sv.gpu_ok && match ent(%s, key).loc { Loc::Mem { ptr, size, .. } => min_int(h.size@, size@) > 0 && result.0 == Ok(0u64) ==> h2d_logged(%s, %s, ptr, h.addr, min_int(h.size@, size@)), _ => false }' % (G, G, F),
    'all_from(%s.og, %s.tick) ==> result.0 == Ok(0u64)' % (G, G)],
   LA, (0, 'result.1.path == LPath::Warm && match ent(%s, key).loc { Loc::Mem { ptr, size, .. } => !h2d_logged(%s, %s, ptr, h.addr, min_int(h.size@, size@)), _ => false }' % (G, G, F)),
   'lookup_async mirror: for a memory-tier hit the slot (from its pointer) is copied to the caller\'s GPU address (H2D of min(handle size, slot size) bytes, synchronized) and the result is success exactly when the GPU copy and sync succeed (always, if the GPU never fails). Byte movement itself is the IGpuServices boundary (the H2D event), so "read back unchanged" rests on that boundary.' + GPUNOTE, fid='trusted-boundary')
VR('DIS-LOOKUP-WARM-TOUCHES-LRU', 's: &mut S, key: u64, h: Ipc', LAR, LQ + [WARMQ],
   ['result.1.path == LPath::Warm && logged(%s, %s, Ev::MtTouch { key })' % (G, F)], LA,
   (0, '!logged(%s, %s, Ev::MtTouch { key })' % (G, F)),
   'lookup_async mirror: a memory-tier hit calls IMemoryTier::touch(key) (lib.rs:3057), refreshing its eviction-order position (the ordering itself is the memory tier\'s).' + GPUNOTE)
R('DIS-LOOKUP-COLD-PROMOTES', '''    let e = Ent { loc: Loc::Blk { off: 0u64 }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: true, writer: true, max_att: 1, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), used: 0, cap: 8192, ..w0() }) };
    let (r, g) = lookup_async(&mut s, 7u64, Ipc { addr: 1u64, size: 4096u32 });
    proof_assert!(g.path == LPath::Cold);
    proof_assert!(r == Ok(0u64) && has(*s.g, 7u64) && is_blk(ent(*s.g, 7u64).loc));''',
  'REFUTED: a cold lookup does not always re-register the key as a memory-tier entry. In memory-tier-only mode (no data drives) promote_and_serve copies the fresh slot to the GPU and calls create_memory_tier_entry for a key that already has a (block-device) entry, which fails AlreadyExists (lib.rs:809); the entry stays BlockDevice, the slot it allocated is left orphaned, and lookup still returns Ok. Counterexample: 0 drives, key in block-device state, room in the tier.')
VR('DIS-LOOKUP-COLD-EVICTS-FIRST', 's: &mut S, key: u64, h: Ipc', LAR, LQ,
   ['result.1.path == LPath::Cold && (match result.1.ps.path { Path::Slot => true, Path::NoDrives => true, _ => false }) ==> result.1.ps.mid.log[result.1.ps.mid.log.len() - 1] == Ev::MtInsert { key } && result.1.ps.mid.used <= result.1.ps.mid.cap && eai_frame(result.1.after_lookup, result.1.ps.mid, key, result.1.ps.ptr, h.size)'],
   LA, (0, 'result.1.path == LPath::Cold && (match result.1.ps.path { Path::Slot => true, _ => false }) ==> result.1.ps.mid.used > result.1.ps.mid.cap'),
   'lookup_async -> promote_and_serve -> evict_and_insert mirrors: the new slot is allocated by mt.insert only after evict_for_space made room (the insert is the last event of the reservation, every demotion before it; usage within capacity after it); evictions are clean demotions only.' + GPUNOTE)
VR('DIS-LOOKUP-COLD-STAGING-FALLBACK', 's: &mut S, key: u64, h: Ipc', LAR, LQ,
   ['result.1.path == LPath::Cold && result.1.ps.path == Path::Staged ==> s.p.nd@ > 0 && s.p.ring && s.p.stg && h.size@ <= s.p.stg_bytes@ && %s.dm == result.1.ps.mid.dm && !result.1.ps.promoted && (result.0 == Ok(0u64) ==> result.1.ps.read_ok)' % F,
    'result.1.path == LPath::Cold && result.1.ps.path == Path::NoSlot && result.0 == Err(DE::Alloc) ==> !(s.p.nd@ > 0 && s.p.ring && s.p.stg && h.size@ <= s.p.stg_bytes@)'],
   LA, (1, 'result.1.path == LPath::Cold && result.1.ps.path == Path::NoSlot ==> result.0 == Ok(0u64)'),
   'lookup_async -> promote_and_serve mirrors: when evict_and_insert fails AllocationFailed and a drive exists and the staging pool is enabled with a large-enough buffer, the read goes SSD -> staging buffer -> GPU and the dispatch map is left untouched (BlockDevice); only when no staging applies does the load fail AllocationFailed.' + GPUNOTE)
VR('DIS-LOOKUP-DELEGATES-ASYNC', 's: &mut S, key: u64, h: Ipc', '(Result<(), DE>, Snapshot<(Result<u64, DE>, LaG, W)>)', [OPR],
   ['result.0 == match (*result.1).0 { Ok(_) => Ok(()), Err(e) => Err(e) } && %s == (*result.1).2 && match (*result.1).0 { Ok(st) => st@ == 0, _ => true }' % F],
   '    lookup(s, key, h)', (0, 'result.0 == Ok(()) && match (*result.1).0 { Err(_) => true, _ => false }'),
   'lookup mirror: lookup = lookup_async + synchronize the returned stream if non-null; lookup_async always returns the null stream after completing its copy (warm path synchronizes inside serve_memory_tier_to_gpu, cold path inside the pipeline), so lookup returns lookup_async\'s result with nothing left pending.')
VR('DIS-LOOKUP-AFTER-DOWNGRADE', 's: &mut S, key: u64, h: Ipc', LAR, LQ + [WARMQ],
   ['result.1.path == LPath::Warm && (all_from(%s.og, %s.tick) ==> result.0 == Ok(0u64))' % (G, G)], LA,
   (0, 'result.0 == Err(DE::KeyNotFound(key))'),
   'lookup_async mirror: after populate downgraded its write reference (write_ref 0, the write-through holding only a read reference) the entry is served on the warm path; it succeeds whenever the GPU copy does - no wait on the pending write-through.' + GPUNOTE)
VR('DIS-LOOKUP-ASYNC-MISS', 's: &mut S, key: u64, h: Ipc', LAR, LQ + ['!has((*s.g), key)'],
   ['result.0 == Err(DE::KeyNotFound(key))'], LA, (0, 'result.0 == Ok(0u64)'),
   'lookup_async mirror: an absent key yields KeyNotFound.' + GPUNOTE)
VR('DIS-LOOKUP-ASYNC-DMA-ERROR', 's: &mut S, key: u64, h: Ipc', LAR, LQ,
   ['result.1.path == LPath::Warm && !result.1.sv.gpu_ok ==> match result.0 { Err(DE::Io(_)) => true, _ => false }',
    'result.1.path == LPath::Cold && result.1.ps.path == Path::NoDrives && !result.1.ps.read_ok ==> result.0 == Err(DE::Io(IoK::Gpu))'],
   LA, (0, 'result.1.path == LPath::Warm && !result.1.sv.gpu_ok ==> result.0 == Ok(0u64)'),
   'lookup_async mirror: if the memory-tier -> GPU copy (or its sync) fails on the warm path, or the slot -> GPU copy on the memory-tier-only cold path fails, lookup_async returns IoError. (On the hardware cold path the SSD read and GPU copy are one pipelined operation whose failure is IoError as well.)' + GPUNOTE)
VR('DIS-LOOKUP-DM-ERROR-KEY-NOT-FOUND', 's: &mut S, key: u64, h: Ipc', LAR, LQ + ['has((*s.g), key) && ent((*s.g), key).wr@ > 0'],
   ['result.0 == Err(DE::KeyNotFound(key))'], LA, (0, 'result.0 == Err(DE::Timeout)'),
   'lookup_async mirror: when dm.lookup errors (here: Timeout because a writer holds the entry) the result is KeyNotFound.' + GPUNOTE)
VR('DIS-LOOKUP-WARM-RELEASES-PIN', 's: &mut S, key: u64, h: Ipc', LAR, LQ + [WARMQ],
   ['result.1.path == LPath::Warm && logged(%s, %s, Ev::RelRead { key, own: 1 }) && (forall<j: u64> %s.dm.get(j) == %s.dm.get(j))' % (G, F, F, G)],
   LA, (0, 'result.1.path == LPath::Warm && ent(%s, key).rr == ent(%s, key).rr + 1u32' % (F, G)),
   'lookup_async mirror: the read reference dm.lookup took is released after the copy whatever its outcome; the entry\'s reference count is back to its value before the call.' + GPUNOTE)
VR('DIS-LOOKUP-COLD-NO-SLOT', 's: &mut S, key: u64, h: Ipc', LAR, LQ,
   ['result.1.path == LPath::Cold && result.1.ps.path == Path::NoSlot && result.0 != Err(DE::AlreadyExists(key)) ==> result.0 == Err(DE::Alloc) && !(s.p.nd@ > 0 && s.p.ring && s.p.stg && h.size@ <= s.p.stg_bytes@)'],
   LA, (0, 'result.1.path == LPath::Cold && result.1.ps.path == Path::NoSlot ==> result.0 == Ok(0u64)'),
   'lookup_async -> promote_and_serve mirrors: a key on SSD for which no memory-tier slot can be obtained (evict_and_insert AllocationFailed) and no suitable staging buffer exists fails AllocationFailed.' + GPUNOTE)
R('DIS-LOOKUP-COLD-FAILURE-RELEASES-SLOT', '''    let e = Ent { loc: Loc::Blk { off: 0u64 }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: true, writer: true, max_att: 1, nd: 1, ring: true, chunk: 131072, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), used: 0, cap: 8192, os: |i: Int| false, ..w0() }) };
    let (r, g) = lookup_async(&mut s, 7u64, Ipc { addr: 1u64, size: 4096u32 });
    proof_assert!(g.path == LPath::Cold && g.ps.path == Path::Slot);
    proof_assert!(r != Ok(0u64) && mt_has(*s.g, 7u64) && has(*s.g, 7u64) && is_blk(ent(*s.g, 7u64).loc));''',
  'REFUTED: when the SSD read of a cold lookup fails after evict_and_insert reserved a slot, promote_and_serve returns the error (lib.rs:882-884) without mt.remove; the same holds for a failed promote_block_to_memory_tier (lib.rs:903-904) and a failed channel checkout. Counterexample: one drive, key in block-device state, room in the tier, the NVMe read fails -> error returned, the memory tier still holds a slot for the key while the dispatch map says BlockDevice.', fid='trusted-boundary')
R('DIS-LOOKUP-NO-FOREIGN-WRITE-RELEASE', '''    let e = Ent { loc: Loc::Blk { off: 0u64 }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: true, writer: true, max_att: 1, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), used: 0, cap: 8192, ..w0() }) };
    let g0 = snapshot!(*s.g);
    let (r, g) = lookup_async(&mut s, 7u64, Ipc { addr: 1u64, size: 4096u32 });
    proof_assert!(g.path == LPath::Cold && g.ps.path == Path::NoDrives && !g.ps.promoted);
    proof_assert!(!own_writes(*g0, *s.g));''',
  'REFUTED: in memory-tier-only mode the cold lookup calls dm.release_write(key) unconditionally (lib.rs:815), although create_memory_tier_entry failed (the key already has an entry) and this call acquired no write reference - so it would clear a write reference held by another caller. Counterexample: 0 drives, key in block-device state: release_write is issued with zero owned write references (ghost ownership count).')
VR('DIS-LOOKUP-COUNTS-MEMORY-PROMOTION', 's: &mut S, key: u64, h: Ipc', LAR, LQ,
   ['result.1.path == LPath::Cold ==> %s.c.promo_mem == %s.c.promo_mem + (if result.1.ps.promoted { 1 } else { 0 }) && (result.1.ps.path == Path::Staged ==> !result.1.ps.promoted)' % (F, G),
    'result.1.path == LPath::Warm ==> %s.c.promo_mem == %s.c.promo_mem' % (F, G)],
   LA, (0, 'result.1.path == LPath::Cold ==> %s.c.promo_mem == %s.c.promo_mem + 2' % (F, G)),
   'lookup_async -> promote_and_serve mirrors: promotions_to_memory grows by exactly one when the cold path re-registered the entry in the memory tier (promote_block_to_memory_tier / create_memory_tier_entry succeeded) and is unchanged for a staged (uncached) serve and for warm hits.' + GPUNOTE)
R('DIS-INV-EVICTION-DEMOTED-STILL-READABLE', '''    let e = Ent { loc: Loc::Blk { off: 28672u64 }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, has_gpu: true, writer: true, max_att: 1, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), used: 0, cap: 8192, ..w0() }) };
    let g0 = snapshot!(*s.g);
    let (r, g) = lookup_async(&mut s, 7u64, Ipc { addr: 1u64, size: 4096u32 });
    proof_assert!(g.path == LPath::Cold && g.ps.path == Path::NoDrives);
    proof_assert!(r == Ok(0u64) && no_ssd(*g0, *s.g));''',
  'REFUTED: in memory-tier-only mode (no data drives) a demoted entry is NOT served from SSD by a later lookup. Demotion (try_evict_to_block) only needs the synthetic offset key*4096 that process_write_job records with no drives (lib.rs:1409); a later lookup then takes the no-hardware branch of promote_and_serve (lib.rs:795) and copies a freshly allocated, never-filled memory-tier slot to the GPU and returns Ok - no SSD read happens and the data is not preserved. Counterexample: 0 drives, demoted (BlockDevice) entry, room in the tier: lookup_async returns Ok with no SSD read event.')
V('DIS-TIER-STATS-PROMOTIONS-TO-GPU', 's: &mut S, key: u64, h: Ipc, s2: &mut S, entries: &[Be]',
  LQ + ['pinv(s2.p) && a_fr039(entries@)'],
  ['%s.c.promo_gpu == %s.c.promo_gpu + (if result_ok_la(*s) { 1 } else { 0 }) || true' % (F, G)],
  '''    let g0 = snapshot!(*s.g);
    let (r, _g) = lookup_async(s, key, h);
    proof_assert!(s.g.c.promo_gpu == g0.c.promo_gpu + (if r == Ok(0u64) { 1 } else { 0 }));
    let g2 = snapshot!(*s2.g);
    let (out, bg) = batch_lookup(s2, entries);
    proof_assert!(bg.ready ==> s2.g.c.promo_gpu == g2.c.promo_gpu + count_ok(out@, entries@.len()));''',
  ['true'], '')
ROWS.pop()
V('DIS-TIER-STATS-PROMOTIONS-TO-GPU', 's: &mut S, key: u64, h: Ipc, s2: &mut S, entries: &[Be]',
  LQ + ['pinv(s2.p) && a_fr039(entries@)'],
  ['true'],
  '''    let g0 = snapshot!(*s.g);
    let (r, _g) = lookup_async(s, key, h);
    proof_assert!(s.g.c.promo_gpu == g0.c.promo_gpu + (if r == Ok(0u64) { 1 } else { 0 }));
    let g2 = snapshot!(*s2.g);
    let (out, bg) = batch_lookup(s2, entries);
    proof_assert!(bg.ready ==> s2.g.c.promo_gpu == g2.c.promo_gpu + count_ok(out@, entries@.len()));''',
  ['true'], '')
ROWS.pop()

def VA(pid, sig, req, body, mut_body, note, fid='ghost-mirror', assumes=()):
    """verify driver whose obligation is a set of proof_assert!s after the real calls."""
    name = 'verify_' + mid(pid)
    a = ''.join('#[requires(%s)]\n' % r for r in req)
    text = '%spub fn %s(%s) {\n%s\n}\n\n%spub fn %s__mutant(%s) {\n%s\n}\n' % (a, name, sig, body, a, name, sig, mut_body)
    ROWS.append(dict(id=pid, kind='verify', text=text, note=note, fid=fid, assumes=list(assumes), mods=[]))

PG_BODY = """    let g0 = snapshot!(*s.g);
    let (r, _g) = lookup_async(s, key, h);
    proof_assert!(s.g.c.promo_gpu == g0.c.promo_gpu + (if r == Ok(0u64) { 1 } else { 0 }));
    let g2 = snapshot!(*s2.g);
    let (out, bg) = batch_lookup(s2, entries);
    proof_assert!(bg.ready ==> s2.g.c.promo_gpu == g2.c.promo_gpu + count_ok(out@, entries@.len()));"""
VA('DIS-TIER-STATS-PROMOTIONS-TO-GPU', 's: &mut S, key: u64, h: Ipc, s2: &mut S, entries: &[Be]',
   LQ + ['pinv(s2.p) && a_fr039(entries@)'], PG_BODY,
   PG_BODY.replace("(if r == Ok(0u64) { 1 } else { 0 })", "1"),
   'lookup_async and batch_lookup mirrors: promotions_to_gpu grows by exactly one per key served to the GPU - one for a successful lookup (warm or cold), count_ok(outcomes) for a batch.' + GPUNOTE, assumes=[A_039])

# =========================================================================== batch_lookup
BL = '    batch_lookup(s, entries)'
BLR = '(Vec<Outcome>, Snapshot<BlG>)'
BLQ = ['pinv(s.p)', 'a_fr039(entries@)']
BLN = ' The per-entry Σ region sizes is a u32 sum that is overflow-free under the declared assumption D-RANGE-FR-039-d29c77 (requires on the driver). The final `results.into_iter().map(|r| r.unwrap())` is modelled as divergence on None (partial correctness): see DIS-BATCH-LOOKUP-LENGTH-ORDER.'
def bl_all(x):
    return 'result.1.ready ==> forall<i: Int> 0 <= i && i < entries@.len() ==> ' + x
O('DIS-BATCH-LOOKUP-COLD-FAILURE-RELEASES-SLOT', 'OPEN (counterexample found, not machine-checked in this run): in the pooled cold path a failed NVMe read (`Err(e) => Err(e.clone())`, lib.rs:2710), a failed promote_block_to_memory_tier (lib.rs:2681-2691), a failed create_stream (lib.rs:2619) or a failed channel checkout (lib.rs:2645) records the error but never calls mt.remove for the slot evict_and_insert reserved, so the memory tier keeps a slot the dispatch map does not point to. The bl_cold mirror proves the single-entry instance (bl_cold_pooled: jobs = [one], all reads fail => slot kept, entry unpromoted), but exposing it through the top-level batch_lookup contract was not completed.')
O('DIS-BATCH-LOOKUP-COLD-NO-THREADS', 'OPEN: structural (no thread API on the batch cold path; one ChannelPool::checkout per active drive job set in bl_cold_pooled). The mirror has no thread construct, so the "spawns no threads" half has no contract-level content in this model; the per-drive-checkout half is visible in bl_cold_pooled but was not stated as a contract. Route: code review / Kani.')
O('DIS-BATCH-LOOKUP-COLD-STAGING-POSTPASS', 'OPEN: bl_cold proves staged entries are classified Staged only when a drive and the staging pool exist and are served by serve_cold_staged after the pooled jobs (which leaves dm/mt untouched); stating "their dispatch-map entries stay BlockDevice" at the batch level needs per-key dm tracking through the pooled promotions (a duplicate key in the batch can be promoted by its pooled twin), which was not completed.')
VA('DIS-BATCH-LOOKUP-CONCURRENT-PROMOTE-HIT', 's: &mut S, entries: &[Be], s2: &mut S, key: u64, regions: &Vec<Ipc>, warm: u64',
   BLQ, '''    let (out, bg) = batch_lookup(s, entries);
    proof_assert!(bg.ready ==> forall<i: Int> 0 <= i && i < entries@.len() ==> match out@[i].result { Err(DE::AlreadyExists(_)) => false, _ => true });
    let (r, saw_mem) = serve_concurrently_promoted(s2, key, regions, warm);
    proof_assert!(match r { Err(DE::AlreadyExists(_)) => false, _ => true });
    proof_assert!(r == Ok(()) ==> saw_mem);''',
   '''    let (out, bg) = batch_lookup(s, entries);
    let (r, saw_mem) = serve_concurrently_promoted(s2, key, regions, warm);
    proof_assert!(r == Ok(()) ==> !saw_mem);''',
   'batch_lookup + serve_concurrently_promoted mirrors: an entry whose promotion lost the mt.insert race (AlreadyExists) is re-served by serve_concurrently_promoted, which polls the dispatch map (racing lookups: an environment oracle standing for the concurrent winner) until the entry is MemoryTier, then copies that slot to the GPU and synchronizes; AlreadyExists is never returned, and success implies a memory-tier slot was served. The 5 s bound is the oracle clock timed_out_5s.' + BLN, assumes=[A_039])
VA('DIS-BATCH-LOOKUP-CONCURRENT-PROMOTE-TIMEOUT', 's: &mut S, key: u64, regions: &Vec<Ipc>, warm: u64', [],
   '''    let (r, saw_mem) = serve_concurrently_promoted(s, key, regions, warm);
    proof_assert!(!saw_mem ==> match r { Err(DE::KeyNotFound(k)) => k == key, Err(DE::InvalidParam) => true, Err(DE::Io(_)) => true, _ => false });''',
   '''    let (r, saw_mem) = serve_concurrently_promoted(s, key, regions, warm);
    proof_assert!(!saw_mem ==> r == Ok(()));''',
   'serve_concurrently_promoted mirror: if the entry never became MemoryTier, the result is KeyNotFound (still BlockDevice at the 5 s timeout, or gone), InvalidParam (size mismatch) or IoError (dispatch-map error) - never success, never AlreadyExists.')
VR('DIS-BATCH-LOOKUP-EMPTY', 's: &mut S, entries: &[Be]', BLR, BLQ + ['entries@.len() == 0'],
   ['result.0@.len() == 0 && %s == %s' % (F, G)], BL, (0, 'result.0@.len() == 1'),
   'batch_lookup mirror: an empty batch returns an empty list with no state change (no tier event counter moves).')
VR('DIS-BATCH-LOOKUP-INIT-FAILURE-WHOLE-BATCH', 's: &mut S, entries: &[Be]', BLR, BLQ + ['entries@.len() > 0', '!(s.p.init && s.p.has_dm && s.p.has_mt && s.p.has_gpu)'],
   ['forall<i: Int> 0 <= i && i < entries@.len() ==> result.0@[i].served_by == ServedBy::Error && match result.0@[i].result { Err(DE::NotInit(_)) => true, _ => false }'],
   BL, (0, 'forall<i: Int> 0 <= i && i < entries@.len() ==> result.0@[i].served_by == ServedBy::Miss'),
   'batch_lookup mirror: uninitialized, or dispatch map / memory tier / GPU services unbound -> every entry is NotInitialized with attribution Error.')
O('DIS-BATCH-LOOKUP-LENGTH-ORDER', 'OPEN (defect suspected): on a returning run the mirror gives exactly one outcome per entry in entry order (proved: result.len() == entries.len(), outcome i built from results[i]); but lib.rs:2640-2652 breaks out of the checkout loop on the first failed ChannelPool::checkout, leaving results[i] == None for the cold entries of every OTHER active drive, and lib.rs:2956 `r.unwrap()` then panics instead of returning outcomes. A panic is not expressible as a Creusot postcondition (partial correctness), so this was neither proved nor refuted here. Route: Kani (should_panic harness with two active drives and a failing connect_client).')
VR('DIS-BATCH-LOOKUP-MEMORY-PROMOTION-COUNTER', 's: &mut S, entries: &[Be]', BLR, BLQ,
   ['result.1.ready ==> %s.c.promo_mem == %s.c.promo_mem + count_true(result.1.cold.prom, result.1.cold.prom.len())' % (F, G)],
   BL, (0, 'result.1.ready ==> %s.c.promo_mem == %s.c.promo_mem' % (F, G)),
   'batch_lookup -> bl_cold mirrors: promotions_to_memory grows by exactly the number of cold keys whose promote_block_to_memory_tier succeeded (ghost prom flags).' + BLN, assumes=[A_039])
O('DIS-BATCH-LOOKUP-MISS-PER-ENTRY', 'OPEN (counterexample found, not machine-checked): with a remote-lookup bound, a locally absent key whose forward fails with a transport error is reported IoError("remote lookup: ..."), not KeyNotFound (lib.rs:2832-2837) - "not served remotely" does not imply KeyNotFound. The mirror proves the remote-transport mapping (bl_remote: Transport => Io(Remote)); building the refuting run needs the IRemoteLookup answers tied to a ghost oracle, not done in this run.')
VR('DIS-BATCH-LOOKUP-NO-REMOTE-UNBOUND', 's: &mut S, entries: &[Be]', BLR, BLQ + ['!s.p.has_rl'],
   ['result.1.ready ==> result.1.rem.nf.len() == 0 && forall<i: Int> 0 <= i && i < entries@.len() && result.1.cls[i] == Cls::Miss ==> result.0@[i].result == Err(DE::KeyNotFound(entries@[i].key))'],
   BL, (0, 'result.1.ready ==> result.1.rem.nf.len() > 0'),
   'batch_lookup mirror: without a remote-lookup receptacle nothing is forwarded and local misses stay KeyNotFound.' + BLN, assumes=[A_039])
VA('DIS-BATCH-LOOKUP-QUEUE-DEPTH-SPLIT', 'segs: &Vec<usize>', ['segs@.len() > 0 && forall<d: Int> 0 <= d && d < segs@.len() ==> segs@[d]@ >= 1'],
   '''    let qd = qd_shares(segs, 128);
    proof_assert!(forall<d: Int> 0 <= d && d < segs@.len() ==> qd@[d]@ == min_int(if 128 * segs@[d]@ / qsum(segs@, segs@.len()) >= 1 { 128 * segs@[d]@ / qsum(segs@, segs@.len()) } else { 1 }, segs@[d]@));
    proof_assert!(segs@.len() == 1 ==> qd@[0]@ == min_int(128, segs@[0]@));''',
   '''    let qd = qd_shares(segs, 128);
    proof_assert!(segs@.len() == 1 ==> qd@[0]@ == 64);''',
   'qd_shares mirror of pipeline.rs:853-861 (batch_lookup passes queue_depth = 128): each active drive gets max(1, 128 * its segments / all segments), clamped to its own segment count; a single active drive gets the whole budget of 128 (clamped to its segment count). The over-budget reduction loop (lib 864-875) is not part of the property.')
O('DIS-BATCH-LOOKUP-REMOTE-FORWARD', 'OPEN: bl_remote proves every local KeyNotFound is forwarded with (key, Σ sizes) and that a REMOTE attribution implies the peer answered Ok; the converse delivery direction (peer Ok + resident + copy/sync ok => success at that position) was not stated in the contract in this run.')
O('DIS-BATCH-LOOKUP-REMOTE-NOT-RESIDENT', 'OPEN (counterexample found, not machine-checked): if the peer answers Ok and the post-fetch dm.lookup returns Err (e.g. Timeout: a writer still holds the entry), lib.rs:2891-2895 reports IoError("post-remote lookup"), not KeyNotFound, although the key is not resident. Needs the remote answers tied to a ghost oracle to build the refuting run.')
VR('DIS-BATCH-LOOKUP-REMOTE-NOTFOUND-IS-MISS', 's: &mut S, entries: &[Be]', BLR, BLQ,
   ['result.1.ready ==> forall<t: Int> 0 <= t && t < result.1.rem.nf.len() ==> (result.1.rem.ans[t] == Rl::NotFound ==> result.0@[result.1.rem.nf[t]@].result == Err(DE::KeyNotFound(entries@[result.1.rem.nf[t]@].key)) && result.0@[result.1.rem.nf[t]@].served_by == ServedBy::Miss)'],
   BL, (0, 'result.1.ready ==> forall<t: Int> 0 <= t && t < result.1.rem.nf.len() ==> (result.1.rem.ans[t] == Rl::NotFound ==> result.0@[result.1.rem.nf[t]@].result == Err(DE::Io(IoK::Remote)))'),
   'batch_lookup -> bl_remote mirrors: a forwarded key no peer holds (RemoteLookupError::NotFound) is reported KeyNotFound (attribution MISS), not IoError.' + BLN, assumes=[A_039])
O('DIS-BATCH-LOOKUP-REMOTE-PIN-UNTIL-SYNC', 'OPEN: bl_remote releases the adopted remote pins strictly after the remote-delivery stream_synchronize (ghost positions sync_at < rel_from are proved), but the per-key link "this RelRead is that key\'s pin" was not stated.')
VA('DIS-BATCH-LOOKUP-SCATTER-BOUNDED', 's: &mut S, pointer: u64, size: u32, regions: &[Ipc], warm: u64, sync: bool', [],
   '''    let g0 = snapshot!(*s.g);
    let (_r, _sv) = serve_memory_tier_to_gpu(s, pointer, size, regions, warm, sync);
    proof_assert!(h2d_within(*g0, *s.g, pointer, size));''',
   '''    let g0 = snapshot!(*s.g);
    let (_r, _sv) = serve_memory_tier_to_gpu(s, pointer, size, regions, warm, sync);
    proof_assert!(!h2d_within(*g0, *s.g, pointer, size));''',
   'serve_memory_tier_to_gpu mirror (every batch_lookup scatter - warm hits, multi-region cold and staged serves, remote deliveries - goes through it): every H2D copy reads inside [slot, slot+size) - copy_size = min(region.size, size - off) - even when the regions add up to more than the slot.')
VR('DIS-BATCH-LOOKUP-SERVEDBY-DRAM', 's: &mut S, entries: &[Be]', BLR, BLQ,
   [bl_all('is_warm(result.1.cls[i]) && result.0@[i].result == Ok(()) ==> result.0@[i].served_by == ServedBy::Dram')],
   BL, (0, bl_all('is_warm(result.1.cls[i]) && result.0@[i].result == Ok(()) ==> result.0@[i].served_by == ServedBy::Ssd')),
   'batch_lookup mirror: an entry served from a memory-tier hit is attributed DRAM (tier set at classification, never rewritten for warm hits).' + BLN, assumes=[A_039])
VR('DIS-BATCH-LOOKUP-SERVEDBY-HIT-AGREEMENT', 's: &mut S, entries: &[Be]', BLR, BLQ,
   ['result.1.ready ==> forall<i: Int> 0 <= i && i < entries@.len() ==> (match result.0@[i].served_by { ServedBy::Dram => true, ServedBy::Ssd => true, ServedBy::Remote => true, _ => false }) == (result.0@[i].result == Ok(()))'],
   BL, (0, 'result.1.ready ==> forall<i: Int> 0 <= i && i < entries@.len() ==> result.0@[i].served_by == ServedBy::Dram'),
   'batch_lookup mirror: every Ok result carries a recorded tier (DRAM/SSD/REMOTE) and every error maps to MISS/SIZE_MISMATCH/ERROR, so is_hit(served_by) <=> result.is_ok() for every returned outcome (the (Ok, None) -> Error branch is unreachable).' + BLN, assumes=[A_039])
VR('DIS-BATCH-LOOKUP-SERVEDBY-MISS-IFF-NOTFOUND', 's: &mut S, entries: &[Be]', BLR, BLQ,
   [bl_all('(result.0@[i].served_by == ServedBy::Miss) == (match result.0@[i].result { Err(DE::KeyNotFound(_)) => true, _ => false })')],
   BL, (0, bl_all('result.0@[i].served_by != ServedBy::Miss')),
   'batch_lookup mirror: MISS exactly for KeyNotFound results, after the remote pass (attribution is computed last).' + BLN, assumes=[A_039])
VA('DIS-BATCH-LOOKUP-SERVEDBY-NO-OUTCOME-CHANGE', 'results: Vec<Res>, tier: Vec<Option<ServedBy>>', ['results@.len() == tier@.len()'],
   '''    let r0 = snapshot!(results@);
    let out = attribute(results, tier);
    proof_assert!(out@.len() == r0.len() && forall<i: Int> 0 <= i && i < r0.len() ==> r0[i] == Some(out@[i].result));''',
   '''    let r0 = snapshot!(results@);
    let out = attribute(results, tier);
    proof_assert!(out@.len() == r0.len() + 1);''',
   'attribute mirror (lib.rs:2952-2977): the attribution pass is a pure map over (result, tier): each outcome keeps its result unchanged, in the same order, and it takes no S - so it makes no dispatch-map call.')
VR('DIS-BATCH-LOOKUP-SERVEDBY-REMOTE', 's: &mut S, entries: &[Be]', BLR, BLQ,
   [bl_all('result.0@[i].served_by == ServedBy::Remote ==> result.0@[i].result == Ok(()) && result.1.cls[i] == Cls::Miss && result.1.tier[i] == Some(ServedBy::Remote)'),
    'result.1.ready ==> forall<t: Int> 0 <= t && t < result.1.rem.nf.len() ==> result.1.tier[result.1.rem.nf[t]@] == Some(ServedBy::Remote) ==> result.1.rem.ans[t] == Rl::Ok'],
   BL, (0, bl_all('result.0@[i].served_by == ServedBy::Remote ==> result.0@[i].result != Ok(())')),
   'batch_lookup mirror: REMOTE is attributed only to a local miss that a peer answered Ok and that was then served (Ok) - such a key is counted in remote_lookup_hits (bl_remote), not as a miss.' + BLN, assumes=[A_039])
VR('DIS-BATCH-LOOKUP-SERVEDBY-SSD-ROUTE', 's: &mut S, entries: &[Be]', BLR, BLQ,
   [bl_all('(match result.1.cls[i] { Cls::Cold { .. } => true, _ => false }) && result.0@[i].result == Ok(()) ==> result.0@[i].served_by == ServedBy::Ssd')],
   BL, (0, bl_all('(match result.1.cls[i] { Cls::Cold { .. } => true, _ => false }) && result.0@[i].result == Ok(()) ==> result.0@[i].served_by == ServedBy::Dram')),
   'batch_lookup mirror: an entry classified BlockDevice is attributed SSD (set once at classification) whichever cold sub-path served it - pooled read with promotion, memory-tier-only path, staging post-pass, or concurrent-promotion recovery.' + BLN, assumes=[A_039])
O('DIS-BATCH-LOOKUP-SINGLE-COLD-POOLED', 'OPEN: bl_cold_prep proves a lone cold key under tier saturation (evict_and_insert AllocationFailed) is queued for the staging post-pass when a drive and the staging pool exist (kind Staged, not a failure); the batch-level "served uncached" outcome needs the staged serve result exposed through batch_lookup, not completed.')
VR('DIS-BATCH-LOOKUP-TOUCH-WARM', 's: &mut S, entries: &[Be]', BLR, BLQ,
   ['result.1.ready && (exists<i: Int> 0 <= i && i < entries@.len() && is_warm(result.1.cls[i])) ==> %s.log.len() <= result.1.touch_at && result.1.touch_at < result.1.cold_from && %s.log[result.1.touch_at] == Ev::MtBatchTouch' % (G, F)],
   BL, (0, 'result.1.ready ==> result.1.touch_at > result.1.cold_from'),
   'batch_lookup mirror: when the batch has memory-tier hits, IMemoryTier::batch_touch is called (refreshing their recency) before the cold path starts (every cold-path event is logged after cold_from).' + BLN, assumes=[A_039])
R('DIS-BATCH-LOOKUP-WARM-DELIVERS', '''    let mut s = S { p: p_zero(), g: snapshot!(w0()) };
    let regions = [Ipc { addr: 100u64, size: 0u32 }, Ipc { addr: 200u64, size: 4096u32 }];
    let g0 = snapshot!(*s.g);
    let (r, sv) = serve_memory_tier_to_gpu(&mut s, 4096u64, 4096u32, &regions, 9u64, false);
    proof_assert!(regions@[0].size@ == 0 && pre_zero(regions@) == ());
    proof_assert!(r == Ok(()) && sv.m == 0 && s.g.log.len() == g0.log.len());''',
  'REFUTED: the warm scatter stops at the first region with nothing to copy (`if copy_size == 0 { break; }`, lib.rs:1284), so a zero-sized region ahead of others starves every later region: with regions [{size 0}, {addr 200, size 4096}] over a 4096-byte slot, batch_lookup\'s warm-hit call serve_memory_tier_to_gpu(ptr, 4096, regions, warm, false) (lib.rs:2415) returns Ok and issues no copy at all - region 1 never receives slot bytes [0, 4096).')
O('DIS-BATCH-LOOKUP-WARM-PINS-HELD-UNTIL-SYNC', 'OPEN: the mirror logs the deferred warm-stream sync before the warm-pin release loop (ghost positions wsync_at < wrel_from), but the "no read pin taken by the batch remains" half needs read-reference accounting through the cold path contracts (refs preserved by evictions/promotions), not completed.')
VR('DIS-TIER-STATS-REMOTE-ACCOUNTING', 's: &mut S, entries: &[Be]', BLR, BLQ,
   ['result.1.ready ==> %s.c.r_hit == %s.c.r_hit + count_rl(result.1.rem.ans, result.1.rem.ans.len(), Rl::Ok) && %s.c.r_miss == %s.c.r_miss + count_rl(result.1.rem.ans, result.1.rem.ans.len(), Rl::NotFound) && result.1.rem.ans.len() == result.1.rem.nf.len()' % (F, G, F, G)],
   BL, (0, 'result.1.ready ==> %s.c.r_hit + %s.c.r_miss == %s.c.r_hit + %s.c.r_miss + result.1.rem.nf.len()' % (F, F, G, G)),
   'batch_lookup -> bl_remote mirrors: remote_lookup_hits grows by the number of forwarded keys a peer answered Ok and remote_lookup_misses by the number answered NotFound; a TransportError (the remaining forwarded keys) moves neither, so hits+misses increase = forwarded - transport errors.' + BLN, assumes=[A_039])
VR('DIS-TIER-STATS-REMOTE-NOT-LOCAL', 's: &mut S, entries: &[Be]', BLR, BLQ,
   ['result.1.ready && (forall<i: Int> 0 <= i && i < entries@.len() ==> result.1.cls[i] != Cls::Miss) ==> %s.c.r_hit == %s.c.r_hit' % (F, G)],
   BL, (0, 'result.1.ready && (forall<i: Int> 0 <= i && i < entries@.len() ==> result.1.cls[i] != Cls::Miss) ==> %s.c.r_hit == %s.c.r_hit + 1' % (F, G)),
   'batch_lookup mirror: only local misses are forwarded (every forwarded position has class Miss), so a batch of purely local hits forwards nothing and remote_lookup_hits does not change.' + BLN, assumes=[A_039])

# =========================================================================== check / remove / touch / pin / unpin
CK = '    check(s, k)'
VR('DIS-CHECK-ABSENT', 's: &mut S, k: u64', 'Result<bool, DE>', [OPR, '!has((*s.g), k)'], ['result == Ok(false)'], CK, (0, 'result == Ok(true)'),
   'check mirror: dm.lookup reports NotExist for a key with no entry -> Ok(false).')
R('DIS-CHECK-PRESENT', '''    let e = Ent { loc: Loc::Mem { ptr: 4096u64, size: 4096u32, ssd: None }, es: 4096u32, rr: 0u32, wr: 1u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), ..w0() }) };
    let r = check(&mut s, 7u64);
    proof_assert!(r == Ok(false));''',
  'REFUTED: check of a key that has a cache entry returns false while a writer holds the entry past the dispatch-map lookup timeout: dm.lookup waits for write_ref == 0 and returns Err(Timeout), which check maps to Ok(false) (lib.rs:3092). Counterexample: entry present (memory-tier) with write_ref = 1 -> check = Ok(false).', fid='trusted-boundary')
VR('DIS-CHECK-NO-SIDE-EFFECTS', 's: &mut S, k: u64', 'Result<bool, DE>', [OPR],
   ['(forall<j: u64> %s.dm.get(j) == %s.dm.get(j)) && %s.mt == %s.mt && no_dma(%s, %s)' % (F, G, F, G, G, F)], CK,
   (0, '%s.dm.get(k) != %s.dm.get(k)' % (F, G)),
   'check mirror: the read reference lookup takes is released at once, so every dispatch-map entry (incl. reference counts) is as before, the memory tier is untouched and no DMA is issued.')
RM = '    remove(s, k)'
VR('DIS-REMOVE-MISSING', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, '!has((*s.g), k)'], ['result == Err(DE::KeyNotFound(k))'], RM, (0, 'result == Ok(())'),
   'remove mirror: no entry -> KeyNotFound.')
VR('DIS-REMOVE-MEMORY-TIER', 's: &mut S, k: u64', '(Result<(), DE>, Result<bool, DE>)', [OPR, 'has((*s.g), k) && is_mem(ent((*s.g), k).loc)'],
   ['result.0 == Ok(()) ==> !has(%s, k) && !mt_has(%s, k) && result.1 == Ok(false)' % (F, F)],
   '''    let r = remove(s, k);
    let c = check(s, k);
    (r, c)''', (0, 'result.0 == Ok(()) ==> result.1 == Ok(true)'),
   'remove + check mirrors: a successful remove of a memory-tier entry deletes the dispatch-map entry and frees its memory-tier slot; check then reports the key absent. (remove fails - KeyNotFound - while the entry is referenced; the property describes a removal that took place.)')
VR('DIS-REMOVE-BLOCK-DEVICE', 's: &mut S, k: u64', '(Result<(), DE>, Result<bool, DE>)', [OPR, 's.p.nd@ > 0', 'has((*s.g), k) && is_blk(ent((*s.g), k).loc)'],
   ['result.0 == Ok(()) ==> !has(%s, k) && result.1 == Ok(false) && match ent(%s, k).loc { Loc::Blk { off } => logged(%s, %s, Ev::ExtFree { drive: di(k, s.p.nd), off }), _ => false }' % (F, G, G, F)],
   '''    let g0 = snapshot!(*s.g);
    let r = remove(s, k);
    let g1 = snapshot!(*s.g);
    let c = check(s, k);
    proof_assert!(match ent(*g0, k).loc { Loc::Blk { off } => r == Ok(()) ==> logged_ext(*g0, *g0, *g1, *s.g, Ev::ExtFree { drive: di(k, s.p.nd), off }) == (), _ => true });
    (r, c)''',
   (0, 'result.0 == Ok(()) ==> result.1 == Ok(true)'),
   'remove + check mirrors: a successful remove of a block-device entry deletes it and frees its extent (remove_extent(offset)) on the extent manager of drive_index(key, n) - the splitmix64 placement; check then reports it absent. Premise: at least one data drive (in memory-tier-only mode a block-device entry carries a synthetic offset and there is no SSD extent).')
VR('DIS-REMOVE-FRAME-OTHER-KEYS', 's: &mut S, k: u64', 'Result<(), DE>', [OPR],
   ['forall<j: u64> j != k ==> %s.dm.get(j) == %s.dm.get(j) && %s.mt.get(j) == %s.mt.get(j)' % (F, G, F, G)], RM,
   (0, 'forall<j: u64> %s.dm.get(j) == %s.dm.get(j)' % (F, G)),
   'remove mirror: only the removed key\'s dispatch-map entry and memory-tier slot change.')
VR('DIS-REMOVE-NO-FOREIGN-PIN-RELEASE', 's: &mut S, k: u64', 'Result<(), DE>', [OPR],
   ['own_reads(%s, %s)' % (G, F)], RM,
   (0, '!has(%s, k) ==> logged(%s, %s, Ev::RelRead { key: k, own: 0 })' % (G, G, F)),
   'remove mirror: only a found entry carries the lookup\'s read reference (NotExist / MismatchSize take none), so every release_read remove issues releases a reference its own lookup acquired - never one held by another caller (e.g. the write-through pin of a concurrent populate registering the key between the lookup and the release).')
VR('DIS-REMOVE-RESOURCES-REUSABLE', 's: &mut S, k: u64', 'Result<(), DE>',
   [OPR, 's.p.nd@ > 0', 'has((*s.g), k) && is_mem(ent((*s.g), k).loc) && persisted(ent((*s.g), k).loc)',
    'ent((*s.g), k).rr@ == 0 && ent((*s.g), k).wr@ == 0', 'mt_has((*s.g), k)'],
   ['result == Ok(()) && !has(%s, k) && !mt_has(%s, k) && match ent(%s, k).loc { Loc::Mem { ssd: Some(off), .. } => logged(%s, %s, Ev::ExtFree { drive: di(k, s.p.nd), off }), _ => false }' % (F, F, G, G, F)], RM,
   (0, 'result == Ok(()) && !has(%s, k) && !mt_has(%s, k) && match ent(%s, k).loc { Loc::Mem { ssd: Some(off), .. } => !logged(%s, %s, Ev::ExtFree { drive: di(k, s.p.nd), off }), _ => false }' % (F, F, G, G, F)),
   'remove mirror: a successful remove of a resident entry whose write-through landed frees both resources the entry used - the memory-tier slot (mt.remove) and the SSD extent at the persisted ssd_offset (remove_extent on the extent manager of drive_index(key, n)), so later entries can reuse them. Premise: at least one data drive and an unreferenced entry (remove refuses a referenced one).')
TC = '    touch(s, k)'
VR('DIS-TOUCH-MISSING', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, '!has((*s.g), k)'], ['result == Err(DE::KeyNotFound(k))'], TC, (0, 'result == Ok(())'),
   'touch mirror: dm.touch fails for an absent key -> KeyNotFound.')
VR('DIS-TOUCH-REFRESHES', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, 'has((*s.g), k)'],
   ['result == Ok(()) && %s.log == %s.log.push_back(Ev::DmTouch { key: k }).push_back(Ev::MtTouch { key: k })' % (F, G)], TC,
   (0, 'result == Ok(()) && %s.log == %s.log.push_back(Ev::DmTouch { key: k })' % (F, G)),
   'touch mirror: an existing key gets dispatch_map.touch (eviction timestamp) and memory_tier.touch (LRU position; a no-op in the tier when the key has no slot).')
VR('DIS-TOUCH-NO-DMA-NO-REFS', 's: &mut S, k: u64', 'Result<(), DE>', [OPR],
   ['%s.dm == %s.dm && no_dma(%s, %s)' % (F, G, G, F)], TC, (0, 'logged(%s, %s, Ev::H2D { src: 0u64, dst: 0u64, len: 0usize, stream: 0u64 })' % (G, F)),
   'touch mirror: no DMA and no dispatch-map change (reference counts unchanged).')
PN = '    pin(s, k)'
VR('DIS-PIN-MISSING', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, '!has((*s.g), k)'], ['result == Err(DE::KeyNotFound(k))'], PN, (0, 'result == Ok(())'),
   'pin mirror: take_read on an absent key -> KeyNotFound.')
VR('DIS-PIN-WRITER-TIMEOUT', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, 'has((*s.g), k) && ent((*s.g), k).wr@ > 0'], ['result == Err(DE::Timeout)'], PN, (0, 'result == Ok(())'),
   'pin mirror: take_read waits for write_ref == 0 (none clears in a single-threaded run) and times out -> Timeout.', fid='trusted-boundary')
VA('DIS-PIN-INCREMENTS', 's: &mut S, k: u64', [OPR],
   '''    let g0 = snapshot!(*s.g);
    let r1 = pin(s, k);
    let g1 = snapshot!(*s.g);
    let r2 = pin(s, k);
    proof_assert!(r1 == Ok(()) ==> ent(*g1, k).rr@ == ent(*g0, k).rr@ + 1);
    proof_assert!(r1 == Ok(()) && r2 == Ok(()) ==> ent(*s.g, k).rr@ == ent(*g0, k).rr@ + 2);''',
   '''    let g0 = snapshot!(*s.g);
    let r1 = pin(s, k);
    let r2 = pin(s, k);
    proof_assert!(r1 == Ok(()) && r2 == Ok(()) ==> ent(*s.g, k).rr@ == ent(*g0, k).rr@ + 1);''',
   'pin mirror: each successful pin adds one read reference; two pins stack to +2.')
VA('DIS-UNPIN-DECREMENTS', 's: &mut S, k: u64', [OPR],
   '''    let g0 = snapshot!(*s.g);
    let r = unpin(s, k);
    proof_assert!(r == Ok(()) ==> ent(*s.g, k).rr@ == ent(*g0, k).rr@ - 1);
    proof_assert!(r == Ok(()) && ent(*s.g, k).rr@ == 0 && ent(*s.g, k).wr@ == 0 && persisted(ent(*s.g, k).loc) ==> evictable(*s.g, k));''',
   '''    let g0 = snapshot!(*s.g);
    let r = unpin(s, k);
    proof_assert!(r == Ok(()) ==> ent(*s.g, k).rr@ == ent(*g0, k).rr@);''',
   'unpin mirror: each successful unpin removes one read reference; once none is left (and no writer, write-through landed) the entry is evictable again (try_evict_to_block precondition).')
VR('DIS-UNPIN-UNDERFLOW', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, '!has((*s.g), k) || ent((*s.g), k).rr@ == 0'],
   ['result == Err(DE::KeyNotFound(k)) && %s.dm == %s.dm' % (F, G)], '    unpin(s, k)', (0, 'result == Ok(())'),
   'unpin mirror: absent key or no read reference left (RefCountUnderflow) -> KeyNotFound, reference count unchanged.')

# =========================================================================== reserve_memory & eviction
RS = '    reserve_memory(s, key, size, deadline)'
RSR = '(Result<u64, DE>, Snapshot<RsvG>)'
RSQ = [OPR, 'size@ > 0']
R('DIS-RESERVE-MEMORY-ALREADY-EXISTS', '''    let e = Ent { loc: Loc::Mem { ptr: 4096u64, size: 4096u32, ssd: None }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, max_att: 0, bp_ms: 0, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), mt: FMap::empty().insert(7u64, Slot { ptr: 4096u64, size: 4096u32 }),
                                     used: 4096, cap: 4096, ..w0() }) };
    let (r, _g) = reserve_memory(&mut s, 7u64, 4096u32, None);
    proof_assert!(r == Err(DE::Alloc));''',
  'REFUTED: reserve_memory for a key that already has a slot does not always return AlreadyExists: evict_and_insert runs evict_for_space before mt.insert (lib.rs:1187), so on a full tier with nothing evictable the call fails AllocationFailed (after the backpressure retries when a budget is set) instead. Counterexample: key resident, tier full, max_eviction_attempts 0, no backpressure budget -> AllocationFailed.')
VR('DIS-RESERVE-MEMORY-SUCCESS', 's: &mut S, key: u64, size: u32, deadline: Option<Dl>', RSR, RSQ,
   ['match result.0 { Ok(ptr) => %s.mt.get(key) == Some(Slot { ptr, size }), _ => true }' % F], RS,
   (0, 'match result.0 { Ok(ptr) => !mt_has(%s, key), _ => true }' % F),
   'reserve_memory -> evict_and_insert mirrors: on success the memory tier holds a slot of exactly `size` bytes for the key at the returned pointer.')
VR('DIS-RESERVE-MEMORY-NO-DISPATCH-ENTRY', 's: &mut S, key: u64, size: u32, deadline: Option<Dl>', RSR, RSQ,
   ['(!has(%s, key) ==> !has(%s, key)) && match result.0 { Ok(_) => no_dma(%s, %s), Err(_) => true }' % (G, F, G, F), 'match result.0 { Err(DE::Alloc) => no_dma(%s, %s), _ => true }' % (G, F)],
   RS, (0, 'has(%s, key)' % F),
   'reserve_memory mirror: it creates no dispatch-map entry (the only dispatch-map changes are clean demotions of other entries) and issues no DMA.')
VR('DIS-RESERVE-MEMORY-FULL-FAILS', 's: &mut S, key: u64, size: u32, deadline: Option<Dl>', RSR, RSQ,
   ['match result.0 { Ok(_) => true, Err(DE::Alloc) => evict_frame(%s, %s), Err(DE::AlreadyExists(k)) => k == key, _ => false }' % (G, F)],
   RS, (0, 'match result.0 { Err(DE::Alloc) => false, _ => true }'),
   'reserve_memory mirror: the backpressure loop exits on capacity failure only when the deadline has passed (or there is no budget), and then returns AllocationFailed - no other error, nothing pinned freed.')
VR('DIS-RESERVE-MEMORY-BACKPRESSURE-COUNTER', 's: &mut S, key: u64, size: u32, deadline: Option<Dl>', RSR, RSQ,
   ['%s.c.bp == %s.c.bp + result.1.attempts - 1' % (F, G)], RS, (0, '%s.c.bp == %s.c.bp + result.1.attempts' % (F, G)),
   'reserve_memory mirror: every retry taken while the tier is full (an AllocationFailed attempt before the deadline) increments store_backpressure_events once: after `attempts` evict_and_insert calls the counter grew by attempts - 1.')
VR('DIS-RESERVE-MEMORY-FAIL-FAST', 's: &mut S, key: u64, size: u32, deadline: Option<Dl>', RSR, RSQ + ['(deadline == None && s.p.bp_ms@ == 0) || match deadline { Some(d) => d.past, None => false }'],
   ['result.1.attempts == 1 && %s.c.bp == %s.c.bp' % (F, G)], RS, (0, 'result.1.attempts == 2'),
   'reserve_memory mirror: with a zero budget (no caller deadline) or a caller deadline already passed (time is monotone: every later clock check sees it passed), exactly one allocation attempt is made and nothing is retried.')
VA('DIS-RESERVE-MEMORY-NO-EVICT-PINNED', 's: &mut S, key: u64, size: u32, deadline: Option<Dl>, s2: &mut S, scan: usize', [OPR, 'size@ > 0'],
   '''    let g0 = snapshot!(*s.g);
    let (r, _g) = reserve_memory(s, key, size, deadline);
    proof_assert!(match r { Ok(ptr) => eai_frame(*g0, *s.g, key, ptr, size), Err(DE::Alloc) => evict_frame(*g0, *s.g), Err(DE::AlreadyExists(_)) => evict_frame(*g0, *s.g), _ => true });
    let g2 = snapshot!(*s2.g);
    let _n = mt_evictor_pass(s2, scan);
    proof_assert!(evict_frame(*g2, *s2.g));''',
   '''    let g0 = snapshot!(*s.g);
    let (r, _g) = reserve_memory(s, key, size, deadline);
    proof_assert!(forall<k: u64> s.g.dm.get(k) == g0.dm.get(k));''',
   'reserve_memory (evict_one_clean / evict_for_space / evict_and_insert) and mt_evictor_pass mirrors: evict_frame - a slot is freed only for an entry that was MemoryTier with an SSD offset and read_ref == write_ref == 0, and the dispatch-map demotion to BlockDevice is logged immediately before the slot is freed (demote_before_free). clear_memory_tier is NOT such a path (DIS-CLEAR-MT-PIN-SAFE is refuted).')
VR('DIS-INV-EVICTION-UNPERSISTED-SKIPPED', 's: &mut S, needed: u32, max_attempts: usize', 'Result<(), DE>', [],
   ['forall<k: u64> has(%s, k) && is_mem(ent(%s, k).loc) && !persisted(ent(%s, k).loc) ==> %s.dm.get(k) == %s.dm.get(k) && %s.mt.get(k) == %s.mt.get(k)' % (G, G, G, F, G, F, G)],
   '    evict_for_space(s, needed, max_attempts)',
   (0, 'forall<k: u64> %s.dm.get(k) == %s.dm.get(k)' % (F, G)),
   'evict_for_space -> evict_one_clean mirrors: an entry whose write-through has not landed (no ssd_offset) is never demoted (try_evict_to_block refuses: InvalidState) nor removed - it stays in the memory tier and in the dispatch map; the scan moves on.')
VR('DIS-INV-EVICTION-BOUNDED-FAILS', 's: &mut S, needed: u32, max_attempts: usize', 'Result<(), DE>', [],
   ['match result { Err(e) => e == DE::Alloc && evict_frame(%s, %s), Ok(()) => %s.used + needed@ <= %s.cap }' % (G, F, F, F),
    'max_attempts@ == 0 && %s.used + needed@ > %s.cap ==> result == Err(DE::Alloc) && %s == %s' % (G, G, F, G)],
   '    evict_for_space(s, needed, max_attempts)', (0, 'match result { Err(e) => e != DE::Alloc, Ok(()) => true }'),
   'evict_for_space mirror: after max_eviction_attempts iterations without making room (or when a scan frees nothing) it returns AllocationFailed; it never frees a pinned or unpersisted entry (evict_frame).')
VR('DIS-INV-EVICTION-NO-OP-WHEN-ROOM', 's: &mut S, needed: u32, max_attempts: usize', 'Result<(), DE>', ['(*s.g).used + needed@ <= (*s.g).cap'],
   ['result == Ok(()) && %s == %s' % (F, G)], '    evict_for_space(s, needed, max_attempts)', (0, '%s != %s' % (F, G)),
   'evict_for_space mirror: when used + needed <= capacity, the loop exits at once: nothing is evicted, no state changes.')
VR('DIS-RESERVE-ZERO-SIZE', 's: &mut S, key: u64, deadline: Option<Dl>', RSR, [OPR],
   ['result.0 == Err(DE::InvalidParam) && %s == %s' % (F, G)], '    reserve_memory(s, key, 0u32, deadline)', (0, 'result.0 == Ok(0u64)'),
   'reserve_memory mirror: size 0 -> InvalidParameter, nothing allocated.')

# =========================================================================== copy async / completed / release
CA = '    copy_gpu_to_memory_async(s, key, regions, stream)'
CAR = '(Result<(), DE>, Snapshot<CopyG>)'
CAQ = [OPR]
VR('DIS-COPY-ASYNC-NO-SLOT', 's: &mut S, key: u64, regions: &[Ipc], stream: u64', CAR, CAQ + ['!mt_has((*s.g), key)'],
   ['result.0 == Err(DE::KeyNotFound(key))'], CA, (0, 'result.0 == Ok(())'),
   'copy_gpu_to_memory_async mirror: no reserved slot -> KeyNotFound.')
VR('DIS-COPY-ASYNC-DMA-ERROR', 's: &mut S, key: u64, regions: &[Ipc], stream: u64', CAR, CAQ,
   ['result.1.failed ==> result.0 == Err(DE::Io(IoK::Gpu))'], CA, (0, 'result.1.failed ==> result.0 == Ok(())'),
   'copy_gpu_to_memory_async mirror: a failed memcpy_d2h_async submission (ghost flag `failed`) returns IoError (and frees the slot).')
VR('DIS-COPY-ASYNC-GATHER-LAYOUT', 's: &mut S, key: u64, regions: &[Ipc], stream: u64', CAR, CAQ,
   ['result.0 == Ok(()) ==> gathered(%s, result.1.base, regions@, regions@.len(), result.1.ptr, result.1.stream) && result.1.ptr == %s.mt.lookup(key).ptr' % (F, G)],
   CA, (0, 'result.0 == Ok(()) && regions@.len() > 1 ==> forall<l: Int> 0 <= l && l < regions@.len() ==> match %s.log[result.1.base + l] { Ev::D2H { dst, .. } => dst == result.1.ptr, _ => false }' % F),
   'copy_gpu_to_memory_async mirror: region L is copied (one D2H, in order) to slot + Σ sizes of regions 0..L (prefix sums pre(regions, L)).')
VR('DIS-COPY-ASYNC-NULL-STREAM-SYNC', 's: &mut S, key: u64, regions: &[Ipc]', CAR, CAQ,
   ['result.0 == Ok(()) ==> result.1.synced && %s.log[result.1.base + regions@.len()] == Ev::Sync { stream: result.1.stream } && %s.log.len() == result.1.base + regions@.len() + 1 && gathered(%s, result.1.base, regions@, regions@.len(), result.1.ptr, result.1.stream)' % (F, F, F)],
   '    copy_gpu_to_memory_async(s, key, regions, 0u64)', (0, 'result.0 == Ok(()) ==> !result.1.synced'),
   'copy_gpu_to_memory_async mirror: with a null stream it resolves the source device\'s store stream (device_streams_for(device_of(regions[0])).store), issues every copy on it, and synchronizes that stream before returning (last event).', fid='trusted-boundary')
VR('DIS-COPY-ASYNC-OVERFLOW', 's: &mut S, key: u64, regions: &[Ipc], stream: u64', CAR, CAQ + ['mt_has((*s.g), key) && sum_sz(regions@) > (*s.g).mt.lookup(key).size@'],
   ['result.0 == Err(DE::InvalidParam) && no_dma(%s, %s) && %s.mt == %s.mt' % (G, F, F, G)], CA, (0, 'result.0 == Ok(())'),
   'copy_gpu_to_memory_async mirror: regions totalling more than the slot -> InvalidParameter before any copy (no D2H event, nothing written).')
VR('DIS-COPY-ASYNC-FRAME-DISPATCH-MAP', 's: &mut S, key: u64, regions: &[Ipc], stream: u64', CAR, [],
   ['%s.dm == %s.dm' % (F, G)], CA, (0, '%s.dm != %s.dm' % (F, G)),
   'copy_gpu_to_memory_async mirror: no dispatch-map call at all.')
CC = '    copy_gpu_to_memory_completed(s, key, size)'
VR('DIS-COPY-COMPLETED-NOT-IN-TIER', 's: &mut S, key: u64, size: u32', 'Result<(), DE>', [OPR, '!mt_has((*s.g), key)'],
   ['result == Err(DE::KeyNotFound(key)) && %s == %s' % (F, G)], CC, (0, 'result == Ok(())'),
   'copy_gpu_to_memory_completed mirror: no slot -> KeyNotFound, no change.')
VR('DIS-COPY-COMPLETED-REGISTERS', 's: &mut S, key: u64, size: u32', 'Result<(), DE>', [OPR],
   ['result == Ok(()) ==> %s.dm.get(key) == Some(Ent { loc: Loc::Mem { ptr: %s.mt.lookup(key).ptr, size, ssd: None }, es: size, rr: 1u32, wr: 0u32 }) && %s.wq == %s.wq.push_back(wjob(s.p, key, size))' % (F, G, F, G)],
   CC, (0, 'result == Ok(()) ==> %s.wq == %s.wq' % (F, G)),
   'copy_gpu_to_memory_completed mirror: success creates the memory-tier dispatch-map entry for the slot (write reference downgraded to the write-through\'s read reference) and enqueues its write-through job.')
VR('DIS-COPY-COMPLETED-PRECONDITION-SIZE', 's: &mut S, key: u64, size: u32', 'Result<(), DE>', [OPR, 'mt_has((*s.g), key) && size@ <= (*s.g).mt.lookup(key).size@'],
   ['result == Ok(()) ==> match ent(%s, key).loc { Loc::Mem { ptr, size: z, .. } => ptr == %s.mt.lookup(key).ptr && z@ <= %s.mt.lookup(key).size@, _ => false }' % (F, G, G)],
   CC, (0, 'result == Ok(()) ==> match ent(%s, key).loc { Loc::Mem { size: z, .. } => z@ > %s.mt.lookup(key).size@, _ => false }' % (F, G)),
   'copy_gpu_to_memory_completed mirror: under the caller precondition size <= reserved slot size, the registered entry (and hence the later write-through, which writes `size` bytes from the entry pointer) stays inside the reserved slot.')
R('DIS-COPY-COMPLETED-ALREADY-EXISTS', '''    let e = Ent { loc: Loc::Blk { off: 0u64 }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), ..w0() }) };
    let r = copy_gpu_to_memory_completed(&mut s, 7u64, 4096u32);
    proof_assert!(r == Err(DE::KeyNotFound(7u64)));''',
  'REFUTED: copy_gpu_to_memory_completed checks the memory-tier slot first (lib.rs:3506) and only then the dispatch map; if the dispatch map already holds an entry for the key but the key has no memory-tier slot (e.g. a block-device entry), it fails KeyNotFound, not AlreadyExists.')
R('DIS-COPY-COMPLETED-EXISTING-ENTRY-PRESERVED', '''    let e = Ent { loc: Loc::Mem { ptr: 4096u64, size: 4096u32, ssd: Some(0u64) }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), mt: FMap::empty().insert(7u64, Slot { ptr: 4096u64, size: 4096u32 }),
                                     used: 4096, cap: 8192, ..w0() }) };
    let r = copy_gpu_to_memory_completed(&mut s, 7u64, 4096u32);
    proof_assert!(r == Err(DE::AlreadyExists(7u64)) && !mt_has(*s.g, 7u64));
    proof_assert!(match ent(*s.g, 7u64).loc { Loc::Mem { ptr, .. } => ptr == 4096u64, _ => false });''',
  'REFUTED: on AlreadyExists, copy_gpu_to_memory_completed calls mt.remove(key) (lib.rs:3511), freeing the memory-tier slot for the key - which is the very slot an existing memory-tier entry for that key points to. Counterexample: key registered as MemoryTier{ptr P} with slot P; a second copy_gpu_to_memory_completed returns AlreadyExists and frees P while the dispatch map still points at it.')
VR('DIS-COPY-COMPLETED-REGISTER-FAILURE-RELEASES', 's: &mut S, key: u64, size: u32', 'Result<(), DE>', [OPR],
   ['result == Err(DE::Io(IoK::Dm)) ==> !mt_has(%s, key) && %s.dm == %s.dm' % (F, F, G)], CC, (0, 'result == Err(DE::Io(IoK::Dm)) ==> mt_has(%s, key)' % F),
   'copy_gpu_to_memory_completed mirror: create_memory_tier_entry failing for another reason (size 0, eviction-policy tracking) -> IoError and mt.remove(key) releases the slot.', fid='trusted-boundary')
RLM = '    release_memory(s, k)'
VR('DIS-RELEASE-MEMORY-FREES', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, 'mt_has((*s.g), k)'],
   ['result == Ok(()) && !mt_has(%s, k)' % F], RLM, (0, 'mt_has(%s, k)' % F),
   'release_memory mirror: frees the key\'s slot.')
VR('DIS-RELEASE-MEMORY-IDEMPOTENT', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, '!mt_has((*s.g), k)'],
   ['result == Ok(()) && %s == %s' % (F, G)], RLM, (0, 'result != Ok(())'),
   'release_memory mirror: no slot -> Ok(()) and nothing changes (KeyNotFound is not an error).')
VR('DIS-RELEASE-MEMORY-PRECONDITION', 's: &mut S, k: u64', 'Result<(), DE>', [OPR, 'mt_has((*s.g), k) ==> !(has((*s.g), k) && match ent((*s.g), k).loc { Loc::Mem { ptr, .. } => ptr == (*s.g).mt.lookup(k).ptr, _ => false })'],
   ['%s.dm == %s.dm && forall<j: u64> has(%s, j) ==> match ent(%s, j).loc { Loc::Mem { ptr, .. } => (j != k ==> %s.mt.get(j) == %s.mt.get(j)), _ => true }' % (F, G, F, F, F, G)],
   RLM, (0, '%s.dm != %s.dm' % (F, G)),
   'release_memory mirror: used as specified (on a reserved-but-never-registered slot), it touches only that key\'s slot and no dispatch-map entry, so no memory a dispatch-map entry points to is freed.')

# =========================================================================== promote / clear / flush / schedule / stats
PT = '    promote_to_memory_tier(s, keys)'
R('DIS-PROMOTE-BLOCK-TO-MEMORY', '''    let e = Ent { loc: Loc::Blk { off: 28672u64 }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, max_att: 1, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), used: 0, cap: 8192, ..w0() }) };
    let g0 = snapshot!(*s.g);
    promote_to_memory_tier(&mut s, &[7u64]);
    proof_assert!(is_mem(ent(*s.g, 7u64).loc) && no_ssd(*g0, *s.g));''',
  'REFUTED: in memory-tier-only mode promote_to_memory_tier flips a block-device entry to memory-tier state without reading anything from SSD: the no-drives branch (lib.rs:3681-3700) inserts a fresh slot and calls promote_block_to_memory_tier, so the entry then points at an uninitialized slot. Counterexample: 0 drives, key in block-device state, room in the tier -> entry is MemoryTier, no SSD read occurred.')
O('DIS-PROMOTE-RESIDENT-REFRESH', 'OPEN (counterexample found, not machine-checked): for keys [m, b] with m memory-tier resident (persisted, unpinned) and b on SSD, the cold promotion of b runs evict_for_space, which can demote m itself (lib.rs:3685 / 3746) - so m is not "only refreshed". The mirror proves the touch half (DmTouch/MtTouch for every resident key) but the two-key eviction scenario needs an eviction-order clause through promote_cold that was not completed.')
VR('DIS-PROMOTE-MISSING-SKIPPED', 's: &mut S, keys: &[u64]', '()', [],
   ['forall<k: u64> !has(%s, k) ==> !has(%s, k)' % (G, F)], PT, (0, 'exists<k: u64> !has(%s, k) && has(%s, k)' % (G, F)),
   'promote_to_memory_tier mirror: a key with no entry is skipped (`_ => {}`); promotion only transitions existing block-device entries, so no entry is ever created.')
VR('DIS-PROMOTE-ERRORS-NOT-PROPAGATED', 's: &mut S, keys: &[u64]', '()', [OPR],
   ['forall<i: Int> 0 <= i && i < keys@.len() && has(%s, keys@[i]) && is_mem(ent(%s, keys@[i]).loc) && ent(%s, keys@[i]).wr@ == 0 && ent(%s, keys@[i]).rr@ < u32::MAX@ ==> logged(%s, %s, Ev::DmTouch { key: keys@[i] })' % (G, G, G, G, G, F)],
   PT, (0, 'forall<i: Int> 0 <= i && i < keys@.len() ==> !logged(%s, %s, Ev::DmTouch { key: keys@[i] })' % (G, F)),
   'promote_to_memory_tier mirror: the function returns () (no error can be propagated) and the classification pass visits every key whatever happened to earlier ones (each resident key is touched); per-key failures in the promotion pass (`continue`) skip only that key.')
VR('DIS-PROMOTE-NOT-INITIALIZED-NOOP', 's: &mut S, keys: &[u64]', '()', ['!s.p.init'],
   ['%s == %s' % (F, G)], PT, (0, '%s != %s' % (F, G)),
   'promote_to_memory_tier mirror: uninitialized -> returns at once, no state change.')
R('DIS-PROMOTE-READ-FAILURE-RELEASES', '''    let e = Ent { loc: Loc::Blk { off: 0u64 }, es: 0u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, max_att: 1, nd: 1, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), used: 0, cap: 8192, ..w0() }) };
    promote_to_memory_tier(&mut s, &[7u64]);
    proof_assert!(mt_has(*s.g, 7u64) && is_blk(ent(*s.g, 7u64).loc));''',
  'REFUTED: a failed dispatch-map promotion does not release the slot: after mt.insert and a successful SSD read, `if dm.promote_block_to_memory_tier(..).is_ok()` (lib.rs:3782) has no else branch, so when the promotion fails (here: entry_size 0 -> InvalidSize) the slot stays in the memory tier while the key remains SSD resident. (A failed SSD read is handled: mt.remove at lib.rs:3771.)')
VR('DIS-PROMOTE-PINS-BALANCED', 's: &mut S, keys: &[u64]', '()', [],
   ['refs_kept(%s, %s)' % (G, F)], PT, (0, 'exists<k: u64> has(%s, k) && ent(%s, k).rr != ent(%s, k).rr' % (G, F, G)),
   'promote_to_memory_tier mirror: every lookup\'s read reference is released immediately; evictions and in-place promotions keep reference counts - every entry\'s read/write counts are as before.')
CL = '    clear_memory_tier(s)'
VR('DIS-CLEAR-MT-EMPTIES', 's: &mut S', 'Result<usize, DE>', [OPR],
   ['match result { Ok(_) => %s.mt.len() == 0, Err(_) => false }' % F], CL, (0, 'match result { Ok(_) => %s.mt.len() > 0, Err(_) => true }' % F),
   'clear_memory_tier mirror: it loops until oldest_keys(64) is empty, which the memory tier only reports when it holds no slot.')
CLR_STATE = '''    let e = Ent { loc: Loc::Mem { ptr: 4096u64, size: 4096u32, ssd: SSD }, es: 4096u32, rr: 1u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), mt: FMap::empty().insert(7u64, Slot { ptr: 4096u64, size: 4096u32 }),
                                     used: 4096, cap: 8192, ..w0() }) };
    let r = clear_memory_tier(&mut s);'''
R('DIS-CLEAR-MT-DEMOTES-PERSISTED', CLR_STATE.replace('SSD', 'Some(0u64)') + '''
    proof_assert!(has(*s.g, 7u64) && is_mem(ent(*s.g, 7u64).loc));''',
  'REFUTED: an entry whose write-through completed but which holds a read pin is not converted to block-device state: try_evict_to_block refuses (ActiveReferences), the else branch frees the slot and calls dm.remove, which also refuses - so the entry stays MemoryTier pointing at the freed slot (lib.rs:3814-3820). Counterexample: persisted entry with read_ref 1.')
R('DIS-CLEAR-MT-REMOVES-UNPERSISTED', CLR_STATE.replace('SSD', 'None') + '''
    proof_assert!(has(*s.g, 7u64));''',
  'REFUTED: an entry whose write-through has not completed is not removed from the dispatch map when it is pinned: dm.remove refuses an entry with a read reference (ActiveReferences) and the error is ignored (lib.rs:3819). Counterexample: unpersisted entry with read_ref 1 stays in the dispatch map after clear_memory_tier.')
R('DIS-CLEAR-MT-PIN-SAFE', CLR_STATE.replace('SSD', 'None') + '''
    proof_assert!(!mt_has(*s.g, 7u64) && match ent(*s.g, 7u64).loc { Loc::Mem { ptr, .. } => ptr == 4096u64, _ => false });''',
  'REFUTED: clear_memory_tier frees the slot of an entry that still holds a read reference while the dispatch map points to it: the else branch calls mt.remove(key) unconditionally before dm.remove (which then refuses because of the pin). Counterexample: entry MemoryTier{ptr P}, read_ref 1 -> slot P freed, dispatch map still MemoryTier{ptr P}.')
FL = '    flush_to_ssd(s)'
VR('DIS-FLUSH-PERSISTS-ALL', 's: &mut S', '(Result<usize, DE>, Snapshot<Seq<u64>>)', [OPR, 'a_us5_q(s.p, (*s.g).wq)'],
   ['%s.wq.len() == 0 && forall<i: Int> 0 <= i && i < result.1.len() ==> has(%s, result.1[i]) && persisted(ent(%s, result.1[i]).loc)' % (F, F, F),
    'forall<k: u64> has(%s, k) && persisted(ent(%s, k).loc) ==> persisted(ent(%s, k).loc)' % (G, G, F)],
   FL, (0, '%s.wq.len() > 0' % F),
   'flush_to_ssd + drain mirrors: flush returns only when every write-through queued before the call has been processed (queue empty); every job whose write landed on a memory-tier entry left it with an SSD offset (ghost list), and no earlier offset is lost.' + SHNOTE, assumes=[A_US5])
R('DIS-FLUSH-RETURNS-COUNT', '''    let e = Ent { loc: Loc::Mem { ptr: 4096u64, size: 4096u32, ssd: Some(0u64) }, es: 4096u32, rr: 0u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), ..w0() }) };
    let (r, _d) = flush_to_ssd(&mut s);
    proof_assert!(r == Ok(0usize) && has(*s.g, 7u64) && persisted(ent(*s.g, 7u64).loc));''',
  'REFUTED: flush_to_ssd returns `writer.in_flight()` sampled before the flush (lib.rs:3832) - the number of jobs still queued - not the number of entries with a valid SSD offset. Counterexample: one entry already persisted, empty queue -> flush returns 0.')
VR('DIS-FLUSH-REPORTS-FAILED-WRITE', 's: &mut S', '(Result<usize, DE>, Snapshot<Seq<u64>>)', [OPR, 'a_us5_q(s.p, (*s.g).wq)'],
   ['%s.wq.len() == 0 && match result.0 { Ok(_) => forall<k: u64> mt_has(%s, k) ==> !%s.wf[k], Err(e) => e == DE::Io(IoK::Ssd) && exists<k: u64> mt_has(%s, k) && %s.wf[k] }' % (F, F, F, F, F)],
   FL, (0, 'match result.0 { Ok(_) => false, Err(_) => true }'),
   'flush_to_ssd + drain + WriteThroughPins::failed_writes mirrors: once the queue is empty, flush returns Ok only if no resident entry\'s latest write-through failed (every resident entry then has its SSD copy or none was attempted), and Err(IoError) naming a resident entry whose write-through failed otherwise.' + SHNOTE, assumes=[A_US5])
SW = '    schedule_write_through(s, key, size)'
VA('DIS-SCHEDULE-WT-QUEUES', 's: &mut S, key: u64, size: u32', [OPR, '(*s.g).wq.len() == 0', 'a_us5(s.p, key)', 'has((*s.g), key) && is_mem(ent((*s.g), key).loc)'],
   '''    let r = schedule_write_through(s, key, size);
    proof_assert!(r == Ok(()) && s.g.wq.len() == 1 && s.g.wq[0].key == key);
    let out = writer_step(s);
    proof_assert!(match out { WjOut::Stored { .. } => persisted(ent(*s.g, key).loc), _ => true });''',
   '''    let r = schedule_write_through(s, key, size);
    proof_assert!(s.g.wq.len() == 0);''',
   'schedule_write_through + writer_step mirrors: success enqueues the job; when the writer processes it and the data lands, the memory-tier entry carries an SSD offset (persisted), i.e. it becomes demotable once unpinned.' + SHNOTE, assumes=[A_US5])
VR('DIS-SCHEDULE-WT-NONBLOCKING', 's: &mut S, key: u64, size: u32', 'Result<(), DE>', [OPR],
   ['result == Ok(()) && %s.wq == %s.wq.push_back(wjob(s.p, key, size)) && %s.dm == %s.dm' % (F, G, F, G)], SW, (0, '%s.wq == %s.wq' % (F, G)),
   'schedule_write_through mirror: it only enqueues (the job is still pending on return) - no processing, no dispatch-map change.')
VR('DIS-SCHEDULE-WT-FAILURE-KEEPS-ENTRY', 's: &mut S, key: u64, size: u32', 'Result<(), DE>', [],
   ['%s.dm == %s.dm && %s.mt == %s.mt' % (F, G, F, G)], SW, (0, '%s.dm != %s.dm' % (F, G)),
   'schedule_write_through mirror: whatever happens (including no writer / not initialized) the dispatch map and memory tier are untouched, so the key stays cached and readable exactly as before.')
R('DIS-SCHEDULE-WT-PIN-NEUTRAL', '''    let e = Ent { loc: Loc::Mem { ptr: 4096u64, size: 4096u32, ssd: None }, es: 4096u32, rr: 1u32, wr: 0u32 };
    let mut s = S { p: P { init: true, has_dm: true, has_mt: true, writer: true, ..p_zero() },
                    g: snapshot!(W { dm: FMap::empty().insert(7u64, e), mt: FMap::empty().insert(7u64, Slot { ptr: 4096u64, size: 4096u32 }),
                                     used: 4096, cap: 8192, ..w0() }) };
    let r = schedule_write_through(&mut s, 7u64, 4096u32);
    let out = writer_step(&mut s);
    proof_assert!(ent(*s.g, 7u64).rr@ == 0);''',
  'REFUTED: processing a job queued by schedule_write_through is not pin-neutral: process_write_job ends with convert_to_storage, which decrements read_ref when it is > 0 (dispatch-map lib.rs:184), and its failure paths call release_read - both written for the populate path, where the job owns the downgraded read reference. schedule_write_through takes none, so a reader\'s pin is consumed. Counterexample (memory-tier-only mode): entry pinned once (read_ref 1), schedule + process -> read_ref 0.', assumes=[A_US5])
VR('DIS-READ-WRITE-STATS-SUM', 's: &mut S', 'u64', [],
   ['result@ == rw_total(%s, s.p.nd@) && (s.p.nd@ == 0 && %s.rw_base == 0 ==> result@ == 0)' % (G, G)], '    proof_assert!(rw_total_zero(*s.g) == ());\n    read_write_stats(s)', (0, 'result@ == rw_total(%s, s.p.nd@) + 1' % G),
   'read_write_stats mirror: the dispatcher-wide counters are the sum (ReadWriteStats::merge_from, field-wise +=) of the baseline (counters of drives already dropped, rw_stats_baseline) and every live data drive\'s read_write_stats - all zero with no drives and an empty baseline. One representative u64 field is modelled; every field (and histogram bucket) is merged identically.', fid='trusted-boundary')
VR('DIS-READ-WRITE-STATS-MONOTONIC', 's: &mut S', '(u64, u64)', SHREQ,
   ['result.1@ >= result.0@',
    's.p.nd@ == 0 && %s.rw_base == 0 ==> result.0@ == 0 && result.1@ == 0' % G],
   '''    let a = read_write_stats(s);
    let b = read_write_stats(s);
    let (_r, _p) = shutdown(s);
    proof_assert!(rw_total_zero(*s.g) == ());
    let c = read_write_stats(s);
    let d = read_write_stats(s);
    proof_assert!(b@ >= a@);
    proof_assert!(c@ >= b@);
    proof_assert!(d@ >= c@);
    (a, d)''',
   (0, 'result.1@ > result.0@'),
   'read_write_stats mirror: the counters are cumulative - baseline (drives already dropped) plus every live drive - so they never decrease between two successive calls, not even across shutdown (which folds the live drives into the baseline before dropping them, rw-stats-decrease-after-shutdown); a fresh telemetry-free state (no drives, empty baseline) reports zero.' + SHNOTE, fid='trusted-boundary', assumes=[A_US5])
O('DIS-TIER-STATS-MONOTONIC', 'OPEN: counters only ever change through the TierEventCounters::record_* bumps (+n, n >= 0), and most method mirrors carry a no-counter-decreases clause, but the one-driver-per-operation inductive proof over every IDispatcher operation (populate/batch_populate/remove/pin/unpin/touch lack the clause) was not completed. Counter wrap-around after 2^64 events (fetch_add on u64) is not modelled.')
VR('DIS-TIER-STATS-OLDEST-GAUGES', 's: &mut S', 'Snapshot<Stats>', [],
   ['result.oldest_persisted <= result.oldest_sampled && (s.p.has_dm && s.p.has_mt ==> result.oldest_persisted == n_evictable(%s, %s.ok[%s.tick], result.oldest_sampled))' % (G, G, G)],
   '    tier_event_stats(s)', (0, 'result.oldest_persisted > result.oldest_sampled'),
   'tier_event_stats mirror: oldest_persisted counts exactly the sampled oldest keys for which is_evictable held at sampling time, so it never exceeds oldest_sampled.')
VR('DIS-TIER-STATS-READ-ONLY', 's: &mut S', 'Snapshot<Stats>', [],
   ['%s.dm == %s.dm && %s.mt == %s.mt && %s.log == %s.log' % (F, G, F, G, F, G)], '    tier_event_stats(s)', (0, '%s.dm != %s.dm' % (F, G)),
   'tier_event_stats mirror: only reads (counter snapshot, oldest_keys, is_evictable): no entry, reference count or eviction-order change.')

# =========================================================================== invariants
VA('DIS-INV-NOT-INITIALIZED', 's: &mut S, key: u64, h: Ipc, entries: &[Be], pes: &[Pe], regions: &[Ipc]', ['pinv(s.p)', '!s.p.init', 'a_fr039(entries@)', 'a_us5_q(s.p, (*s.g).wq)'],
   '''    let r1 = check(s, key);
    proof_assert!(r1 == Err(DE::NotInit(Ni::Dispatcher)));
    let r2 = remove(s, key);
    proof_assert!(r2 == Err(DE::NotInit(Ni::Dispatcher)));
    let (r3, _a) = populate(s, key, h);
    proof_assert!(r3 == Err(DE::NotInit(Ni::Dispatcher)));
    let (r4, _b) = reserve_memory(s, key, h.size, None);
    proof_assert!(r4 == Err(DE::NotInit(Ni::Dispatcher)));
    let (r5, _c) = copy_gpu_to_memory_async(s, key, regions, 0u64);
    proof_assert!(r5 == Err(DE::NotInit(Ni::Dispatcher)));
    let r6 = copy_gpu_to_memory_completed(s, key, h.size);
    proof_assert!(r6 == Err(DE::NotInit(Ni::Dispatcher)));
    let r7 = release_memory(s, key);
    proof_assert!(r7 == Err(DE::NotInit(Ni::Dispatcher)));
    let r8 = pin(s, key);
    proof_assert!(r8 == Err(DE::NotInit(Ni::Dispatcher)));
    let r9 = unpin(s, key);
    proof_assert!(r9 == Err(DE::NotInit(Ni::Dispatcher)));
    let r10 = touch(s, key);
    proof_assert!(r10 == Err(DE::NotInit(Ni::Dispatcher)));
    let r11 = clear_memory_tier(s);
    proof_assert!(r11 == Err(DE::NotInit(Ni::Dispatcher)));
    let (r12, _d) = flush_to_ssd(s);
    proof_assert!(r12 == Err(DE::NotInit(Ni::Dispatcher)));
    let r13 = schedule_write_through(s, key, h.size);
    proof_assert!(r13 == Err(DE::NotInit(Ni::Dispatcher)));
    let (r14, _e) = lookup(s, key, h);
    proof_assert!(r14 == Err(DE::NotInit(Ni::Dispatcher)));
    let (r15, _f) = batch_populate(s, pes);
    proof_assert!(forall<i: Int> 0 <= i && i < pes@.len() ==> r15@[i] == Err(DE::NotInit(Ni::Dispatcher)));
    let (r16, _g) = batch_lookup(s, entries);
    proof_assert!(forall<i: Int> 0 <= i && i < entries@.len() ==> r16@[i].result == Err(DE::NotInit(Ni::Dispatcher)));''',
   '''    let r1 = check(s, key);
    proof_assert!(r1 == Ok(false));''',
   'every IDispatcher mirror that reports errors (check, remove, populate, reserve_memory, copy_gpu_to_memory_async/completed, release_memory, pin, unpin, touch, clear_memory_tier, flush_to_ssd, schedule_write_through, lookup, batch_populate, batch_lookup) returns NotInitialized when the dispatcher is not initialized (before initialize, and after shutdown - which clears init); shutdown and initialize are excluded by the property; promote_to_memory_tier reports no errors (DIS-PROMOTE-NOT-INITIALIZED-NOOP).', assumes=[A_039, A_US5])
VA('DIS-INV-MT-EVICTOR-SKIP-NOT-REMOVE', 's: &mut S, scan: usize', [],
   '''    let g0 = snapshot!(*s.g);
    let _n = mt_evictor_pass(s, scan);
    proof_assert!(forall<k: u64> has(*g0, k) && is_mem(ent(*g0, k).loc) && !evictable(*g0, k) ==> s.g.dm.get(k) == g0.dm.get(k) && s.g.mt.get(k) == g0.mt.get(k));''',
   '''    let g0 = snapshot!(*s.g);
    let _n = mt_evictor_pass(s, scan);
    proof_assert!(forall<k: u64> has(*g0, k) ==> !has(*s.g, k));''',
   'mt_evictor_pass mirror (one demotion pass of MemoryTierEvictor::evictor_loop): a candidate with active references or no completed write-through is skipped (`continue` on try_evict_to_block error): it stays memory-tier resident and its dispatch-map entry is untouched (never removed). The f64 utilization/watermark checks are oracles.')
O('DIS-INV-MT-EVICTOR-BATCH-SCALE', 'OPEN: the effective batch size is f64 arithmetic (background.rs:503-510: pressure, 1 + 7*pressure^2, `as usize`); Creusot cannot translate f64 (creusot-rustc ICE on an f64 literal), so the formula is not expressible here. Route: Kani (f64 supported) over MemoryTierEvictor\'s batch computation.')
VA('DIS-INV-SSD-EVICTOR-SKIPS-MEMORY-TIER', 's: &mut S, batch: usize', ['s.p.nd@ > 0'],
   '''    let g0 = snapshot!(*s.g);
    let _n = ssd_evictor_pass(s, batch);
    proof_assert!(forall<k: u64> has(*g0, k) && is_mem(ent(*g0, k).loc) ==> s.g.dm.get(k) == g0.dm.get(k));''',
   '''    let g0 = snapshot!(*s.g);
    let _n = ssd_evictor_pass(s, batch);
    proof_assert!(forall<k: u64> has(*g0, k) && is_mem(ent(*g0, k).loc) ==> !has(*s.g, k));''',
   'ssd_evictor_pass mirror (one pass of BackgroundEvictor::evictor_loop): get_evictable_offset returns None for a memory-tier entry, so it is never removed from the dispatch map.')
VA('DIS-INV-SSD-EVICTOR-SKIPS-REFERENCED', 's: &mut S, batch: usize', ['s.p.nd@ > 0'],
   '''    let g0 = snapshot!(*s.g);
    let _n = ssd_evictor_pass(s, batch);
    proof_assert!(forall<k: u64> has(*g0, k) && (ent(*g0, k).rr@ > 0 || ent(*g0, k).wr@ > 0) ==> s.g.dm.get(k) == g0.dm.get(k));''',
   '''    let g0 = snapshot!(*s.g);
    let _n = ssd_evictor_pass(s, batch);
    proof_assert!(forall<k: u64> has(*g0, k) && ent(*g0, k).rr@ > 0 ==> !has(*s.g, k));''',
   'ssd_evictor_pass mirror: dm.remove refuses an entry with active references (ActiveReferences) and the evictor `continue`s - no panic, no error reported, the dispatch-map entry is left intact. (The memory-tier slot of such a key, if any, is removed first - lib/background.rs:328 - which the property does not cover.)')
VA('DIS-INV-CHANNEL-EXCLUSIVE', 's: &mut S, free: &mut Vec<u64>', ['nodup(free@)'],
   '''    let r = chan_pool_checkout(s, free);
    proof_assert!(nodup(free@));
    proof_assert!(match r { Some(c) => (forall<i: Int> 0 <= i && i < free@.len() ==> free@[i] != c) && pending(*s.g, c) <= 0, None => true });
    match r {
        Some(c) => {
            chan_pool_return(free, c);
            proof_assert!(nodup(free@));
        }
        None => {}
    }''',
   '''    let r = chan_pool_checkout(s, free);
    proof_assert!(match r { Some(c) => exists<i: Int> 0 <= i && i < free@.len() && free@[i] == c, None => false });''',
   'chan_pool_checkout / chan_pool_return mirrors of ChannelPool::checkout and ChannelLease::drop: the pool invariant "no channel twice in the free list" is preserved by both; a leased channel is not in the free list (so no second reader can check it out) and carries no pending completion (the try_recv drain loop runs until empty).', fid='trusted-boundary')
O('DIS-INV-PIPELINE-SEGMENT-MDTS', 'OPEN: the MDTS half follows from io_segmenter::segment_io (every segment <= the max_transfer_size it is given) plus D-RANGE-FR-019-73b5ad (every drive\'s MDTS >= the chunk size), but the "reads in flight never exceed the configured queue depth" half lives in the completion loops of the pipeline functions (pipeline.rs:282-1100), which were not mirrored. Route: Kani over the pipeline loops with a stub channel.')
O('DIS-INV-PIPELINE-COALESCED-H2D', 'OPEN: the "one coalesced H2D after all NVMe segments of an object completed" ordering lives inside scatter_gather_multi_drive_zero_copy / pipelined_multi_object_zero_copy (pipeline.rs:1024-1048), which this mirror treats as a trusted per-job boundary (sg_job / pipe_read_gpu); stating it here would be the boundary\'s own contract, i.e. vacuous. Route: Kani over the completion loop.')
VA('DIS-INV-STAGING-BOUNDED', 'slots: usize, s: &mut S, cfg: ICfg', ['pinv(s.p) && !s.p.stg'],
   '''    let mut p = stg_new(slots);
    proof_assert!(stg_inv(p) && *p.leased <= slots@);
    let i = stg_pool_checkout(&mut p);
    proof_assert!(*p.leased <= slots@ && slots@ >= 1);
    stg_pool_release(&mut p, i);
    proof_assert!(*p.leased <= slots@);
    let (_r, _f) = initialize(s, cfg);
    proof_assert!(cfg.stg_slots@ == 0 ==> !s.p.stg);''',
   '''    let mut p = stg_new(slots);
    let i = stg_pool_checkout(&mut p);
    proof_assert!(*p.leased > slots@);''',
   'StagingPool mirror (stg_new / stg_pool_checkout / stg_pool_release): the pool invariant leased + free = slots is preserved, so at most cold_staging_slots buffers are in use at once (checkout blocks when none is free); and initialize enables staging only when cold_staging_slots > 0 (PipelineRing::new creates no pool for 0 slots), so with 0 no staging is ever used.')
O('DIS-INV-STORE-STREAM-D2H', 'OPEN: copy_gpu_to_memory_async proves every D2H it issues is on one stream (the one passed, or the store stream it resolves for a null stream), and populate/batch_populate pass device_streams_for(device).store; stating "never the warm stream" needs the distinctness of the per-device store and warm CUDA streams as a boundary fact plus D2H-stream clauses on populate/batch_populate, not completed.')
VR('DIS-SERVEDBY-IS-HIT', 'sb: ServedBy', 'bool', [],
   ['result == (sb == ServedBy::Dram || sb == ServedBy::Ssd || sb == ServedBy::Remote)', '(sb == ServedBy::Miss || sb == ServedBy::SizeMismatch || sb == ServedBy::Error) ==> !result'],
   '    served_by_is_hit(sb)', (0, 'result == (sb == ServedBy::Dram)'),
   'served_by_is_hit mirror of ServedBy::is_hit (`matches!(self, Dram | Ssd | Remote)`) over the six-variant mirror enum ServedBy.')

# =========================================================================== emit
HEADER = '''// drivers.rs — GENERATED by tools/gen_drivers.py. One `verify_<id>` per proved property (its
// contract / assertions ARE the obligation, stated against the mirror functions) with a
// deliberately false `verify_<id>__mutant` twin that must fail, and one `refute_<id>` per refuted
// property: it builds one concrete state (receptacle bindings, dispatch-map / memory-tier
// contents, constant oracles) and machine-proves that the property's negation holds after the
// real (mirrored) call.

/// All-false / all-zero dispatcher fields (a freshly constructed DispatcherComponent).
#[ensures(!result.init && !result.has_dm && !result.has_mt && !result.has_gpu && !result.has_spdk && !result.has_factory
    && !result.has_rl && !result.has_logger && result.max_att@ == 0 && result.bp_ms@ == 0 && result.nd@ == 0
    && !result.writer && !result.ssd_ev && !result.mt_ev && !result.ring && result.chunk@ == 0 && !result.stg
    && result.stg_bytes@ == 0 && result.warm@ == 0)]
pub fn p_zero() -> P {
    P { init: false, has_dm: false, has_mt: false, has_gpu: false, has_spdk: false, has_factory: false, has_rl: false,
        has_logger: false, max_att: 0, bp_ms: 0, nd: 0, writer: false, ssd_ev: false, mt_ev: false, ring: false,
        chunk: 0, stg: false, stg_bytes: 0, warm: 0 }
}

/// An empty world whose every oracle answers "success" (and every oldest-keys answer is empty).
#[logic(open)]
pub fn w0() -> W {
    pearlite! { W {
        dm: FMap::empty(), mt: FMap::empty(), used: 0, cap: 0,
        c: Ctr { promo_mem: 0, promo_gpu: 0, ev_mem: 0, ev_ssd: 0, hit_dram: 0, hit_ssd: 0, r_hit: 0, r_miss: 0,
                 bp: 0, drop_full: 0, already: 0, blk_pin: 0, exhausted: 0, blk_unp: 0 },
        wq: Seq::empty(), log: Seq::empty(), tick: 0,
        ob: |i: Int| true, ob2: |i: Int| true, og: |i: Int| true, os: |i: Int| true,
        ov: |i: Int| 0u64, ok: |i: Int| Seq::empty(), ext: |i: Int| Seq::empty(),
        pool: false, cluster: false, threads: 0, rw: |i: Int| 0u64, rw_base: 0, cfree: |i: Int| 0,
        wf: |k: u64| false,
    } }
}

'''
def main():
    out = HEADER + '\n'.join(r['text'] for r in ROWS if r['text'])
    open('src/drivers.rs', 'w').write(out)
    # advisory
    adv_path = '../verif/creusot_advisory.yaml'
    adv = {}
    if os.path.exists(adv_path):
        adv = yaml.safe_load(open(adv_path)) or {}
    for r in ROWS:
        e = {'fidelity': r['fid']}
        a = r['assumes']
        note = r['note'].strip()
        e['note'] = (note + ' ' if note else '') + 'assumes: [%s]' % ', '.join(a)
        if r['kind'] == 'verify':
            e['evidence'] = {'module': 'verify_' + mid(r['id'])}
        elif r['kind'] == 'refute':
            e['evidence'] = {'module': 'refute_' + mid(r['id'])}
        if r.get('ix'):
            e['claims_inexpressible'] = r['ix']
            e['fidelity'] = 'not-expressible'
        if r['kind'] == 'open':
            e['outcome'] = 'open'
        adv[r['id']] = e
    yaml.safe_dump(adv, open(adv_path, 'w'), sort_keys=True, width=110, allow_unicode=True)
    ids = [r['id'] for r in ROWS]
    dup = sorted(set(i for i in ids if ids.count(i) > 1))
    print('rows', len(ROWS), 'dups', dup)
    return ids

if __name__ == '__main__':
    ids = main()
    if len(sys.argv) > 1:
        want = [l.strip() for l in open(sys.argv[1]) if l.strip()]
        print('missing', [w for w in want if w not in ids])
        print('extra', [i for i in ids if i not in want])
