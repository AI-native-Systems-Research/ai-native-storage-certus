//! Mirror of the `DispatcherComponent` methods in `src/lib.rs` that the loom
//! targets exercise. The GPU side is a slot copy into a returned `u64`; the
//! dispatcher's statistics counters, metrics and logging are dropped.

use crate::background::{
    EvictionEvent, EvictionReason, EvictionSender, JobStart, ParallelBackgroundWriter, WriteGate,
    WriteJob, WriteThroughPins,
};
use crate::dmap::{DispatchMap, DmError, LookupResult};
use crate::stand::{Chan, ExtentManager, MemoryTier, MtError};
use crate::{CacheKey, Mutation};
use loom::sync::atomic::{AtomicU64, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

/// `interfaces::DispatcherError` (payloads dropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DErr {
    KeyNotFound(CacheKey),
    InvalidParameter,
    AllocationFailed,
    AlreadyExists(CacheKey),
    IoError,
}

/// `interfaces::ServedBy`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServedBy {
    Dram,
    Ssd,
    Remote,
    Miss,
    SizeMismatch,
    Error,
}

/// `interfaces::LookupOutcome`, plus observation fields for the models.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub served_by: ServedBy,
    pub result: Result<(), DErr>,
    /// The result was overwritten by the concurrent-promotion recovery pass.
    pub overwritten: bool,
    /// The recorded tier was rewritten in that same step.
    pub tier_rewritten: bool,
}

/// Points at which a model can observe or rendezvous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// Inside a lookup's read section (read reference held, slot being served).
    ReadSection(CacheKey),
    /// remove released a read reference (src/lib.rs:3208/:3224/:3228/:3239):
    /// `acquired` tells whether remove's own lookup took one for the key,
    /// `decremented` whether the release actually dropped a reference.
    /// Observation only.
    RemoveRelease { key: CacheKey, acquired: bool, decremented: bool },
}

pub type HookFn = Box<dyn Fn(Hook) + Send + Sync>;

// mirrors src/lib.rs:587 (drive_index). Every model has at most one drive, so
// the splitmix64 placement hash is not needed.
pub fn drive_index(key: CacheKey, num_drives: usize) -> usize {
    (key as usize) % num_drives
}

pub struct Dispatcher {
    pub dm: Arc<DispatchMap>,
    pub mt: Arc<MemoryTier>,
    /// `data_drives[i].extent_mgr` (empty = no-hardware mode).
    pub drives: Vec<Arc<ExtentManager>>,
    pub bg_writer: Mutex<Option<ParallelBackgroundWriter>>,
    pub eviction_tx: Arc<Mutex<Option<EvictionSender>>>,
    pub eviction_dropped: Arc<AtomicU64>,
    pub max_eviction_attempts: usize,
    /// `store_backpressure_ms > 0`
    pub store_backpressure: bool,
    pub hook: Option<HookFn>,
    pub write_through_pins: Arc<WriteThroughPins>,
    /// Test rendezvous (DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE): when set, an
    /// owned write-through job parks between taking the slot pointer and
    /// reading the slot until the gate is released. `None` in every model
    /// except the EVICTOR-NO-FOREIGN-PIN-RELEASE harm scenario.
    pub write_gate: Option<WriteGate>,
    pub m: Mutation,
}

impl Dispatcher {
    pub fn new(dm: Arc<DispatchMap>, mt: Arc<MemoryTier>, drives: Vec<Arc<ExtentManager>>, m: Mutation) -> Self {
        Self {
            dm,
            mt,
            drives,
            bg_writer: Mutex::new(None),
            eviction_tx: Arc::new(Mutex::new(None)),
            eviction_dropped: Arc::new(AtomicU64::new(0)),
            max_eviction_attempts: 4,
            store_backpressure: true,
            hook: None,
            write_through_pins: Arc::new(WriteThroughPins::default()),
            write_gate: None,
            m,
        }
    }

    // mirrors src/lib.rs:2083 (initialize: ParallelBackgroundWriter::start)
    pub fn start_bg_writer(&self) {
        let n = self.drives.len().max(1);
        let (dm, mt, drives) = (self.dm.clone(), self.mt.clone(), self.drives.clone());
        let pins = self.write_through_pins.clone();
        let gate = self.write_gate.clone();
        let w = ParallelBackgroundWriter::start(
            n,
            move |_idx| {
                let (pins, dm, mt, drives) = (pins.clone(), dm.clone(), mt.clone(), drives.clone());
                let gate = gate.clone();
                move |job| Self::run_write_job(&pins, &dm, &mt, &drives, job, gate.as_ref())
            },
            self.m,
        );
        *self.bg_writer.lock().unwrap() = Some(w);
    }

    // mirrors src/lib.rs:2172 (shutdown: bg_writer part)
    pub fn shutdown(&self) {
        if let Some(mut w) = self.bg_writer.lock().unwrap().take() {
            w.shutdown();
        }
    }

    // mirrors src/lib.rs:563 (create_eviction_channel)
    pub fn create_eviction_channel(&self, capacity: usize) -> Arc<Chan<EvictionEvent>> {
        let ch = Arc::new(Chan::bounded(capacity));
        *self.eviction_tx.lock().unwrap() =
            Some(EvictionSender { tx: ch.clone(), dropped: Arc::clone(&self.eviction_dropped) });
        ch
    }

    // mirrors src/lib.rs:576 (eviction_dropped_count)
    pub fn eviction_dropped_count(&self) -> u64 {
        self.eviction_dropped.swap(0, Ordering::Relaxed)
    }

    // mirrors src/lib.rs:580 (emit_eviction)
    fn emit_eviction(&self, key: CacheKey, reason: EvictionReason) {
        let guard = self.eviction_tx.lock().unwrap();
        if let Some(ref tx) = *guard {
            tx.emit(EvictionEvent { key, reason });
        }
    }

    // mirrors src/lib.rs:1056 (evict_one_clean)
    fn evict_one_clean(&self, scan: usize) -> bool {
        for cand in self.mt.oldest_keys(scan) {
            match self.dm.try_evict_to_block(cand) {
                Ok(()) => {
                    let _ = self.mt.remove(cand);
                    self.emit_eviction(cand, EvictionReason::Demoted);
                    return true;
                }
                Err(DmError::ActiveReferences(_)) => {}
                Err(_) => {}
            }
        }
        false
    }

    // mirrors src/lib.rs:1129 (evict_for_space)
    fn evict_for_space(&self, needed: u32, max_attempts: usize) -> Result<(), DErr> {
        const MAX_SCAN: usize = 4;
        let mut attempts = 0usize;
        while self.mt.used() + needed as usize > self.mt.capacity() {
            attempts += 1;
            if attempts > max_attempts {
                return Err(DErr::AllocationFailed);
            }
            let scan = (MAX_SCAN * attempts).min(1024);
            if !self.evict_one_clean(scan) {
                return Err(DErr::AllocationFailed);
            }
        }
        Ok(())
    }

    // mirrors src/lib.rs:1182 (evict_and_insert)
    fn evict_and_insert(&self, key: CacheKey, size: u32, max_attempts: usize) -> Result<usize, DErr> {
        const MAX_SCAN: usize = 4;
        let mut attempts = 0usize;
        loop {
            self.evict_for_space(size, max_attempts.saturating_sub(attempts))?;
            match self.mt.insert(key, size) {
                Ok(ptr) => return Ok(ptr),
                Err(MtError::PoolFull) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(DErr::AllocationFailed);
                    }
                    if !self.evict_one_clean(MAX_SCAN * attempts) {
                        return Err(DErr::AllocationFailed);
                    }
                }
                Err(MtError::AlreadyExists(k)) => return Err(DErr::AlreadyExists(k)),
                Err(_) => return Err(DErr::AllocationFailed),
            }
        }
    }

    // mirrors src/lib.rs:1268 (serve_memory_tier_to_gpu): the H2D copy reads the
    // slot; the value read is what the GPU receives.
    fn serve_memory_tier_to_gpu(&self, key: CacheKey, pointer: usize, _size: u32) -> u64 {
        if let Some(h) = &self.hook {
            h(Hook::ReadSection(key));
        }
        self.mt.read_slot(pointer)
    }

    // mirrors src/lib.rs:1355 (serve_concurrently_promoted); 50 µs sleep ->
    // yield_now, the 5 s timeout never fires.
    fn serve_concurrently_promoted(&self, key: CacheKey) -> Result<(), DErr> {
        loop {
            match self.dm.lookup(key) {
                Ok(LookupResult::MemoryTier { pointer, size }) => {
                    let _ = self.serve_memory_tier_to_gpu(key, pointer, size);
                    let _ = self.dm.release_read(key);
                    let _ = self.dm.touch(key);
                    return Ok(());
                }
                Ok(LookupResult::BlockDevice { .. }) => {
                    let _ = self.dm.release_read(key);
                    thread::yield_now();
                }
                Ok(LookupResult::NotExist) => return Err(DErr::KeyNotFound(key)),
                Err(_) => return Err(DErr::IoError),
            }
        }
    }

    // mirrors src/lib.rs:1397 (process_write_job): the untracked path.
    pub fn process_write_job(
        dm: &DispatchMap,
        mt: &MemoryTier,
        drives: &[Arc<ExtentManager>],
        job: WriteJob,
    ) -> bool {
        // Untracked path: never parked (no harm model drives a job here).
        let written = Self::write_through(mt, drives, &job, None);
        Self::settle_write_through(dm, job.key, written);
        written.is_some()
    }

    // mirrors src/lib.rs:1412 (run_write_job)
    pub fn run_write_job(
        pins: &WriteThroughPins,
        dm: &DispatchMap,
        mt: &MemoryTier,
        drives: &[Arc<ExtentManager>],
        job: WriteJob,
        gate: Option<&WriteGate>,
    ) {
        match pins.start(job.key) {
            JobStart::Stale => return,
            JobStart::Untracked => {
                let key = job.key;
                let landed = Self::process_write_job(dm, mt, drives, job);
                return pins.record_write(key, landed);
            }
            JobStart::Owned => {}
        }
        let written = Self::write_through(mt, drives, &job, gate);
        pins.finish(job.key, |owned| {
            if owned {
                pins.record_write(job.key, written.is_some());
                Self::settle_write_through(dm, job.key, written);
                return;
            }
            let _ = mt.remove(job.key);
            if let (Some(offset), false) = (written, drives.is_empty()) {
                let drive_idx = job.device_index % drives.len();
                drives[drive_idx].remove_extent(offset);
            }
        });
    }

    // mirrors src/lib.rs:1448 (settle_write_through)
    fn settle_write_through(dm: &DispatchMap, key: CacheKey, written: Option<u64>) {
        let written = if dm.mutation() == Mutation::SettleSkipsConvert {
            // MUTANT(SettleSkipsConvert): the SSD copy is never recorded.
            None
        } else {
            written
        };
        match written {
            Some(block_offset) => {
                let _ = dm.convert_to_storage(key, block_offset);
            }
            None => {
                let _ = dm.release_read(key);
            }
        }
    }

    // mirrors src/lib.rs:1466 (write_through). The DmaBuffer wrap cannot fail in
    // the stand-in; reserve_extent fails when the drive is full and
    // write_buffer_to_ssd (src/lib.rs:672) fails when the drive injects an I/O
    // error, in which case the reservation is aborted (write_handle drop).
    // `gate` is the test rendezvous of `Dispatcher::write_gate`: it parks the
    // job between taking the slot pointer and reading the slot (between
    // src/lib.rs:1474 and src/lib.rs:1503-1512) and is `None` outside the
    // DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE harm scenario.
    fn write_through(
        mt: &MemoryTier,
        drives: &[Arc<ExtentManager>],
        job: &WriteJob,
        gate: Option<&WriteGate>,
    ) -> Option<u64> {
        let (mem_ptr, _size) = mt.peek(job.key)?;
        if let Some(g) = gate {
            let (lock, cvar) = &**g;
            let mut s = lock.lock().unwrap();
            s.0 = true;
            cvar.notify_all();
            while !s.1 {
                s = cvar.wait(s).unwrap();
            }
        }
        if drives.is_empty() {
            return Some(job.key * 4096);
        }
        let drive_idx = job.device_index % drives.len();
        let iem = &drives[drive_idx];
        let block_offset = iem.reserve_extent(job.key).ok()?;
        let data = mt.read_slot(mem_ptr);
        if iem.write(block_offset, data).is_err() {
            iem.abort(block_offset);
            return None;
        }
        iem.publish(block_offset);
        Some(block_offset)
    }

    // mirrors src/lib.rs:2257 (lookup) + src/lib.rs:3058 (lookup_async).
    // Returns the value delivered to the caller's GPU buffer.
    pub fn lookup(&self, key: CacheKey) -> Result<u64, DErr> {
        let result = self.dm.lookup(key);
        match result {
            Ok(LookupResult::NotExist) => Err(DErr::KeyNotFound(key)),
            Ok(LookupResult::MemoryTier { pointer, size }) => {
                let r = self.serve_memory_tier_to_gpu(key, pointer, size);
                let _ = self.dm.release_read(key);
                self.mt.touch(key);
                Ok(r)
            }
            Ok(LookupResult::BlockDevice { .. }) => {
                // promote_and_serve (src/lib.rs:756) is not mirrored: no model
                // looks up a block-device-only key through `lookup`.
                unreachable!("BlockDevice path of lookup is not mirrored")
            }
            Err(_) => Err(DErr::KeyNotFound(key)),
        }
    }

    // mirrors src/lib.rs:3242 (populate). `data` is the client GPU buffer.
    pub fn populate(&self, key: CacheKey, data: u64, size: u32) -> Result<(), DErr> {
        if size == 0 {
            return Err(DErr::InvalidParameter);
        }
        match self.reserve_memory(key, size, &|| false) {
            Ok(_) => {}
            Err(DErr::AllocationFailed) => return Ok(()),
            Err(e) => return Err(e),
        }
        if self.m == Mutation::PublishBeforeCopy {
            // MUTANT(PublishBeforeCopy): register + enqueue before the D2H lands.
            self.copy_gpu_to_memory_completed(key, size)?;
            self.copy_gpu_to_memory_async(key, data)?;
        } else {
            // Phase 2: every failure before Phase 3 registers the slot releases it.
            if let Err(e) = self.copy_gpu_to_memory_async(key, data) {
                let _ = self.release_memory(key);
                return Err(e);
            }
            self.copy_gpu_to_memory_completed(key, size)?;
        }
        Ok(())
    }

    // mirrors src/lib.rs:3434 (reserve_memory). `deadline_passed` stands for
    // `Instant::now() >= deadline`; sleep(POLL) -> yield_now.
    pub fn reserve_memory(
        &self,
        key: CacheKey,
        size: u32,
        deadline_passed: &dyn Fn() -> bool,
    ) -> Result<usize, DErr> {
        if size == 0 {
            return Err(DErr::InvalidParameter);
        }
        let max_attempts = self.max_eviction_attempts;
        if !self.store_backpressure {
            return self.evict_and_insert(key, size, max_attempts);
        }
        loop {
            match self.evict_and_insert(key, size, max_attempts) {
                Ok(mem_ptr) => return Ok(mem_ptr),
                Err(DErr::AllocationFailed) if !deadline_passed() => {
                    if self.m == Mutation::ReserveFailFast {
                        // MUTANT(ReserveFailFast): give up on the first refusal.
                        return Err(DErr::AllocationFailed);
                    }
                    thread::yield_now();
                }
                Err(e) => return Err(e),
            }
        }
    }

    // mirrors src/lib.rs:3538 (copy_gpu_to_memory_async), single region, synced.
    pub fn copy_gpu_to_memory_async(&self, key: CacheKey, data: u64) -> Result<(), DErr> {
        let (mem_ptr, _slot_size) = self.mt.get(key).ok_or(DErr::KeyNotFound(key))?;
        self.mt.write_slot(mem_ptr, data);
        Ok(())
    }

    // mirrors src/lib.rs:3706 (release_memory): idempotent slot free.
    pub fn release_memory(&self, key: CacheKey) -> Result<(), DErr> {
        match self.mt.remove(key) {
            Ok(()) | Err(MtError::KeyNotFound(_)) => Ok(()),
            Err(_) => Err(DErr::IoError),
        }
    }

    // mirrors src/lib.rs:3605 (copy_gpu_to_memory_completed)
    pub fn copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DErr> {
        let (mem_ptr, _) = self.mt.get(key).ok_or(DErr::KeyNotFound(key))?;
        self.dm.create_memory_tier_entry(key, mem_ptr, size).map_err(|e| match e {
            DmError::AlreadyExists(k) => {
                // Free the slot only when the existing entry provably lives elsewhere.
                let live = match self.dm.lookup(key) {
                    Ok(LookupResult::NotExist) => false,
                    Ok(r) => {
                        let _ = self.dm.release_read(key);
                        matches!(r, LookupResult::MemoryTier { pointer, .. } if pointer == mem_ptr)
                    }
                    Err(_) => true,
                };
                if !live {
                    let _ = self.mt.remove(key);
                }
                DErr::AlreadyExists(k)
            }
            _ => {
                let _ = self.mt.remove(key);
                DErr::IoError
            }
        })?;
        self.dm.downgrade_reference(key).map_err(|_| DErr::IoError)?;
        let num_drives = self.drives.len().max(1);
        let guard = self.bg_writer.lock().unwrap();
        // Record the pin before its job can run, so `remove` can take it over.
        self.write_through_pins.pin(key, || match *guard {
            Some(ref writer) => writer
                .enqueue(WriteJob { key, size, device_index: drive_index(key, num_drives) })
                .is_ok(),
            None => false,
        });
        Ok(())
    }

    // mirrors src/lib.rs:2288 (batch_lookup): classification (2438), cold
    // promotion in no-drives mode (2532), warm pin release after the cold block
    // (2814), concurrent-promotion recovery (3001) and attribution (3016). The pooled
    // multi-drive cold path and the remote pass are not mirrored.
    pub fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<Outcome> {
        assert!(self.drives.is_empty(), "only the no-drives cold path is mirrored");
        let n = entries.len();
        let mut results: Vec<Option<Result<(), DErr>>> = vec![None; n];
        let mut tier: Vec<Option<ServedBy>> = vec![None; n];
        let mut overwritten = vec![false; n];
        let mut tier_rewritten = vec![false; n];
        let mut deferred_touch_keys = Vec::new();
        let mut warm_pins: Vec<CacheKey> = Vec::new();
        // (idx, key, total_size)
        let mut cold_entries: Vec<(usize, CacheKey, u32)> = Vec::new();

        for (i, &(key, total_size)) in entries.iter().enumerate() {
            const LOOKUP_RETRIES: u32 = 5;
            let lookup = {
                let mut lk = self.dm.lookup(key);
                let mut retries = 0u32;
                while lk.is_err() && retries < LOOKUP_RETRIES {
                    retries += 1;
                    lk = self.dm.lookup(key);
                }
                lk
            };
            match lookup {
                Ok(LookupResult::NotExist) => results[i] = Some(Err(DErr::KeyNotFound(key))),
                Ok(LookupResult::MemoryTier { pointer, size }) => {
                    let _ = self.serve_memory_tier_to_gpu(key, pointer, size);
                    warm_pins.push(key);
                    deferred_touch_keys.push(key);
                    tier[i] = Some(ServedBy::Dram);
                    results[i] = Some(Ok(()));
                }
                Ok(LookupResult::BlockDevice { .. }) => {
                    let _ = self.dm.release_read(key);
                    tier[i] = Some(ServedBy::Ssd);
                    cold_entries.push((i, key, total_size));
                }
                Err(_) => results[i] = Some(Err(DErr::KeyNotFound(key))),
            }
        }
        if !deferred_touch_keys.is_empty() {
            self.mt.batch_touch(&deferred_touch_keys);
        }

        if !cold_entries.is_empty() {
            let max_attempts = self.max_eviction_attempts;
            for &(idx, key, total_size) in &cold_entries {
                self.evict_for_space(total_size, max_attempts).ok();
                let res = self
                    .mt
                    .insert(key, total_size)
                    .map(|mem_ptr| {
                        let _ = self.dm.promote_block_to_memory_tier(key, mem_ptr, total_size);
                    })
                    .map_err(|e| match e {
                        MtError::AlreadyExists(k) => DErr::AlreadyExists(k),
                        _ => DErr::AllocationFailed,
                    });
                results[idx] = Some(res);
            }
        }

        // Warm stream sync, then PinnedKeys drop releases the warm pins.
        for key in warm_pins {
            let _ = self.dm.release_read(key);
        }

        for (i, &(key, _)) in entries.iter().enumerate() {
            if matches!(results[i], Some(Err(DErr::AlreadyExists(_)))) {
                let before = tier[i];
                results[i] = Some(self.serve_concurrently_promoted(key));
                if self.m == Mutation::RecoveryWrongTier {
                    // MUTANT(RecoveryWrongTier): rewrite the tier, to the wrong route.
                    tier[i] = Some(ServedBy::Remote);
                } else {
                    // Served out of DRAM after the winner's promotion.
                    tier[i] = Some(ServedBy::Dram);
                }
                overwritten[i] = true;
                tier_rewritten[i] = tier[i] != before;
            }
        }

        results
            .into_iter()
            .zip(tier)
            .enumerate()
            .map(|(i, (r, t))| {
                let result = r.unwrap();
                let served_by = match (&result, t) {
                    (Ok(()), Some(t)) => t,
                    (Ok(()), None) => ServedBy::Error,
                    (Err(DErr::KeyNotFound(_)), _) => ServedBy::Miss,
                    (Err(DErr::InvalidParameter), _) => ServedBy::SizeMismatch,
                    (Err(_), _) => ServedBy::Error,
                };
                Outcome {
                    served_by,
                    result,
                    overwritten: overwritten[i],
                    tier_rewritten: tier_rewritten[i],
                }
            })
            .collect()
    }

    // mirrors src/lib.rs:3197/:3201 (remove's `let _ = dm.release_read(key)`),
    // observation-instrumented for the NO-FOREIGN-PIN-RELEASE models: whether
    // the release dropped a reference, and whether remove's own lookup
    // acquired one. Behavior is exactly `dm.release_read(key)`.
    fn release_read_observed(&self, key: CacheKey, acquired: bool) {
        let decremented = self.dm.release_read(key).is_ok();
        if let Some(h) = &self.hook {
            h(Hook::RemoveRelease { key, acquired, decremented });
        }
    }

    // mirrors src/lib.rs:3186 (remove)
    pub fn remove(&self, key: CacheKey) -> Result<(), DErr> {
        // mirrors src/lib.rs:3205: the memory-tier state this call's pre-pass
        // flips away, to restore if the removal below is refused: a refused
        // remove (`KeyNotFound`) frees nothing and changes nothing, so the
        // entry must still be MemoryTier, with its memory-tier slot and its
        // ssd_offset.
        let mut flipped_memory_tier: Option<(usize, u32)> = None;
        let block_offset = match self.dm.lookup(key) {
            Ok(LookupResult::BlockDevice { offset }) => {
                self.release_read_observed(key, true);
                if self.m == Mutation::RemoveDoubleRelease {
                    // MUTANT(RemoveDoubleRelease): a second release of a read
                    // reference remove does not own.
                    self.release_read_observed(key, false);
                }
                Some(offset)
            }
            Ok(LookupResult::MemoryTier { pointer, size }) => {
                // A resident entry may already carry its write-through SSD
                // copy (`ssd_offset`), which this lookup result does not
                // expose. Flip the entry to its block-device state — the
                // transition that records that offset — and look it up once
                // more to learn which extent to free, holding this lookup's
                // read reference across the flip so the memory-tier evictor's
                // demotion (`try_evict_to_block`, which refuses a pinned
                // entry) cannot change the state underneath us.
                // `InvalidState` means no SSD copy exists yet: nothing to
                // free — a pending job's extent is freed by the takeover
                // below or by the job itself.
                let converted = self.dm.convert_memory_tier_to_block(key).is_ok();
                self.release_read_observed(key, true);
                if self.m == Mutation::RemoveDoubleRelease {
                    // MUTANT(RemoveDoubleRelease): as above.
                    self.release_read_observed(key, false);
                }
                if converted {
                    match self.dm.lookup(key) {
                        Ok(LookupResult::BlockDevice { offset }) => {
                            self.release_read_observed(key, true);
                            if self.m == Mutation::RemoveDoubleRelease {
                                // MUTANT(RemoveDoubleRelease): as above.
                                self.release_read_observed(key, false);
                            }
                            // This call flipped the entry away: remember how
                            // to put it back if the removal is refused.
                            flipped_memory_tier = Some((pointer, size));
                            Some(offset)
                        }
                        // The entry vanished (a concurrent remove won the
                        // race) or was re-promoted in place (its extent stays
                        // the live entry's): the map removal below settles
                        // the outcome either way.
                        Ok(LookupResult::MemoryTier { .. }) => {
                            self.release_read_observed(key, true);
                            if self.m == Mutation::RemoveDoubleRelease {
                                // MUTANT(RemoveDoubleRelease): as above.
                                self.release_read_observed(key, false);
                            }
                            None
                        }
                        // NotExist takes no read reference: nothing to release.
                        Ok(LookupResult::NotExist) => None,
                        Err(_) => None,
                    }
                } else {
                    None
                }
            }
            // NotExist: the lookup took no read reference, so there is none of
            // this caller's to release.
            Ok(LookupResult::NotExist) => None,
            Err(_) => return Err(DErr::KeyNotFound(key)),
        };
        let job_frees_slot = match self.dm.remove(key) {
            Ok(()) => false,
            Err(DmError::ActiveReferences(_)) => {
                match self.write_through_pins.remove_entry(&self.dm, key) {
                    Some(job_frees_slot) => job_frees_slot,
                    // A reader, a client pin or a concurrent lookup holds the
                    // entry: the remove is refused. `KeyNotFound` promises that
                    // nothing was freed and nothing changed, so put the entry
                    // back the way this call found it — still MemoryTier, with
                    // its memory-tier slot and its ssd_offset (the in-place
                    // promote is the convert's inverse and keeps every
                    // reference) — and a later lookup is served from DRAM
                    // again. Once the hold is released, a later remove
                    // succeeds and frees both.
                    None => {
                        if let Some((pointer, size)) = flipped_memory_tier {
                            let _ = self.dm.promote_block_to_memory_tier(key, pointer, size);
                        }
                        return Err(DErr::KeyNotFound(key));
                    }
                }
            }
            Err(_) => return Err(DErr::KeyNotFound(key)),
        };
        if !job_frees_slot && self.m != Mutation::RemoveSkipsSlotFree {
            // MUTANT(RemoveSkipsSlotFree): slot never freed.
            let _ = self.mt.remove(key);
        }
        if let Some(offset) = block_offset {
            if self.m != Mutation::RemoveSkipsExtentFree {
                // MUTANT(RemoveSkipsExtentFree): the extent is never freed.
                let idx = drive_index(key, self.drives.len().max(1));
                if let Some(em) = self.drives.get(idx) {
                    em.remove_extent(offset);
                }
            }
        }
        Ok(())
    }

    // mirrors src/lib.rs:3929 (clear_memory_tier); 50 µs sleep -> yield_now,
    // the 5 s timeout never fires.
    pub fn clear_memory_tier(&self) -> usize {
        let mut count = 0;
        let mut pinned = 0;
        loop {
            let candidates = self.mt.oldest_keys(64 + pinned);
            if candidates.is_empty() {
                break;
            }
            let mut cleared = 0;
            pinned = 0;
            for key in candidates {
                if self.m == Mutation::ClearFreesFirst {
                    // MUTANT(ClearFreesFirst): slot freed before the demotion attempt.
                    let _ = self.mt.remove(key);
                }
                let done = match self.dm.try_evict_to_block(key) {
                    Ok(()) => true,
                    Err(DmError::ActiveReferences(_)) => false,
                    Err(_) => !matches!(self.dm.remove(key), Err(DmError::ActiveReferences(_))),
                };
                if done {
                    let _ = self.mt.remove(key);
                    count += 1;
                    cleared += 1;
                } else {
                    pinned += 1;
                }
            }
            if cleared == 0 {
                thread::yield_now();
            }
        }
        count
    }

    // mirrors src/lib.rs:3995 (flush_to_ssd); the bg_writer lock is held
    // across the flush, as in the real `if let Some(ref writer) = *lock()`.
    // `oldest_keys(usize::MAX)` is every resident key.
    pub fn flush_to_ssd(&self) -> Result<usize, DErr> {
        if let Some(ref writer) = *self.bg_writer.lock().unwrap() {
            if self.m != Mutation::FlushNoWait {
                // MUTANT(FlushNoWait): return without waiting for the queue.
                writer.flush();
            }
        }
        let failed = self.write_through_pins.failed_writes(|key| self.mt.contains(key));
        if failed > 0 {
            return Err(DErr::IoError);
        }
        Ok(self.mt.oldest_keys(usize::MAX).into_iter().filter(|&key| self.dm.is_evictable(key)).count())
    }
}
