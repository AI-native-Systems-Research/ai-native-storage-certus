//! Repair test for finding `ssd-evictor-releases-foreign-pin`
//! (DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE).
//!
//! The SSD evictor's candidate probe (`get_evictable_offset`) releases a read
//! reference in its catch-all `Ok(_)` arm — the arm that answers
//! `LookupResult::NotExist`. But `lookup` takes a read reference only for an
//! entry it *finds*: a candidate that answers NotExist acquired none, so the
//! release is foreign. In the race where a concurrent `populate` registers the
//! candidate between the probe's lookup and its release — reserving the slot,
//! creating the entry (write ref) and downgrading that ref to the write-through
//! pin (read ref = 1) — the probe's release drops *that* pin. A later `remove`
//! then no longer sees active references and frees the memory-tier slot while
//! the entry's write-through job is still reading it.
//!
//! Self-contained: it drives `BackgroundEvictor::start` through its own minimal
//! mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use interfaces::{
    CacheKey, DispatchMapError, Extent, ExtentKey, ExtentManagerError, FormatParams, IDispatchMap,
    IExtentManager, IMemoryTier, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
    WriteHandle,
};

use crate::background::{BackgroundEvictor, EvictorConfig};
use crate::TierEventCounters;

const KEY: CacheKey = 1;
const BLOCK: u32 = 4096;
const SLOT_ADDR: usize = 0x1000;

// --- memory tier: the set of allocated slots ---

#[derive(Default)]
struct Mt {
    slots: Mutex<HashMap<CacheKey, (usize, u32)>>,
}

impl IMemoryTier for Mt {
    fn initialize(&self, _: usize, _: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
        let mut slots = self.slots.lock().unwrap();
        match slots.insert(key, (SLOT_ADDR, size)) {
            Some(_) => {
                slots.insert(key, (SLOT_ADDR, size));
                Err(MemoryTierError::AlreadyExists(key))
            }
            None => Ok(SLOT_ADDR as *mut u8),
        }
    }
    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        self.peek(key)
    }
    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let slots = self.slots.lock().unwrap();
        slots.get(&key).map(|&(p, s)| (p as *mut u8, s))
    }
    fn evict_next(&self) -> Option<CacheKey> {
        None
    }
    fn evict_next_for_key(&self, _: CacheKey) -> Option<CacheKey> {
        None
    }
    fn oldest_keys(&self, _: usize) -> Vec<CacheKey> {
        Vec::new()
    }
    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        match self.slots.lock().unwrap().remove(&key) {
            Some(_) => Ok(()),
            None => Err(MemoryTierError::KeyNotFound(key)),
        }
    }
    fn touch(&self, _: CacheKey) {}
    fn batch_touch(&self, _: &[CacheKey]) {}
    fn contains(&self, key: CacheKey) -> bool {
        self.slots.lock().unwrap().contains_key(&key)
    }
    fn capacity(&self) -> usize {
        1 << 20
    }
    fn used(&self) -> usize {
        0
    }
    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        None
    }
    fn is_dma_capable(&self) -> bool {
        false
    }
    fn clear(&self) -> Result<usize, MemoryTierError> {
        let mut slots = self.slots.lock().unwrap();
        let n = slots.len();
        slots.clear();
        Ok(n)
    }
    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        MemoryTierTelemetrySnapshot::default()
    }
}

// --- dispatch map: entries with reference counts, as the real one enforces them ---

#[derive(Clone, Copy, Debug, PartialEq)]
enum Loc {
    #[allow(dead_code)] // the block-device state exists in the map this mock stands in for
    Block(u64),
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory(usize, u32),
}

struct Ent {
    loc: Loc,
    read_refs: u32,
    write_ref: bool,
}

/// The mock's rendezvous state for the registration race.
#[derive(Default)]
struct Race {
    /// Arm the raced lookup: the next `lookup` of an absent key marks the race
    /// as fired and then blocks until the key is registered and pinned, then
    /// answers `NotExist` — the absence observed *before* a concurrent
    /// populate won the registration race.
    armed: bool,
    /// The raced lookup fired; the concurrent populate may register now.
    fired: bool,
}

struct Dm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
    race: Mutex<Race>,
    /// Signals a change to an entry's references, for the rendezvous wait.
    pins_changed: Condvar,
}

impl Dm {
    fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            race: Mutex::new(Race::default()),
            pins_changed: Condvar::new(),
        }
    }

    fn arm_registration_race(&self) {
        self.race.lock().unwrap().armed = true;
    }

    fn race_fired(&self) -> bool {
        self.race.lock().unwrap().fired
    }

    fn read_refs(&self, key: CacheKey) -> Option<u32> {
        self.entries.lock().unwrap().get(&key).map(|e| e.read_refs)
    }

    fn loc(&self, key: CacheKey) -> Option<Loc> {
        self.entries.lock().unwrap().get(&key).map(|e| e.loc)
    }
}

impl IDispatchMap for Dm {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    /// As the real dispatch map: `NotExist` takes no read reference (the map
    /// returns before incrementing), a found entry takes one.
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        {
            let mut race = self.race.lock().unwrap();
            if race.armed && !entries.contains_key(&key) {
                race.armed = false;
                race.fired = true;
                drop(race);
                // Hold the answer (absent) while the concurrent populate
                // registers the entry and downgrades its write ref to the
                // write-through pin, then answer the stale miss.
                drop(
                    self.pins_changed
                        .wait_while(entries, |es| {
                            !matches!(es.get(&key), Some(e) if !e.write_ref && e.read_refs >= 1)
                        })
                        .unwrap(),
                );
                return Ok(LookupResult::NotExist);
            }
        }
        let Some(e) = entries.get_mut(&key) else {
            return Ok(LookupResult::NotExist);
        };
        if e.write_ref {
            return Err(DispatchMapError::Timeout(key));
        }
        e.read_refs += 1;
        Ok(match e.loc {
            Loc::Block(offset) => LookupResult::BlockDevice { offset },
            Loc::Memory(p, size) => LookupResult::MemoryTier {
                pointer: p as *mut u8,
                size,
            },
        })
    }
    fn convert_to_storage(&self, _: CacheKey, _: u64) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.read_refs += 1;
        drop(entries);
        self.pins_changed.notify_all();
        Ok(())
    }
    fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.write_ref = true;
        Ok(())
    }
    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if e.read_refs == 0 {
            return Err(DispatchMapError::RefCountUnderflow(key));
        }
        e.read_refs -= 1;
        drop(entries);
        self.pins_changed.notify_all();
        Ok(())
    }
    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        if !e.write_ref {
            return Err(DispatchMapError::RefCountUnderflow(key));
        }
        e.write_ref = false;
        Ok(())
    }
    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::NoWriteReference(key))?;
        if !e.write_ref {
            return Err(DispatchMapError::NoWriteReference(key));
        }
        e.write_ref = false;
        e.read_refs += 1;
        drop(entries);
        self.pins_changed.notify_all();
        Ok(())
    }
    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        match entries.get(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) if e.read_refs > 0 || e.write_ref => {
                Err(DispatchMapError::ActiveReferences(key))
            }
            Some(_) => {
                entries.remove(&key);
                Ok(())
            }
        }
    }
    fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        if self.entries.lock().unwrap().contains_key(&key) {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }
    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        self.touch(key).map(|_| BLOCK)
    }
    /// The pass's candidate list is a snapshot: the entry was present when the
    /// list was built and can be gone (and re-registered) by the probe — so
    /// the candidate is handed out regardless of current presence.
    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        vec![KEY; n.min(1)]
    }
    fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        if entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        entries.insert(
            key,
            Ent {
                loc: Loc::Memory(pointer as usize, size),
                read_refs: 0,
                write_ref: true,
            },
        );
        drop(entries);
        self.pins_changed.notify_all();
        Ok(())
    }
    fn convert_memory_tier_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not modelled".into()))
    }
    fn promote_block_to_memory_tier(
        &self,
        _: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::InvalidState("not modelled".into()))
    }
    fn is_evictable(&self, _: CacheKey) -> bool {
        false
    }
    fn try_evict_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(KEY))
    }
    fn recover_extent(&self, _: CacheKey, _: u64, _: u32) -> Result<(), DispatchMapError> {
        Ok(())
    }
    #[cfg(feature = "integrity-check")]
    fn set_checksum(&self, _: CacheKey, _: u32) -> Result<(), DispatchMapError> {
        Ok(())
    }
    #[cfg(feature = "integrity-check")]
    fn get_checksum(&self, _: CacheKey) -> Option<u32> {
        None
    }
}

// --- extent manager: always full, so every pass scans ---

#[derive(Default)]
struct Em {
    removed: Mutex<Vec<u64>>,
}

impl IExtentManager for Em {
    fn format(&self, _: FormatParams) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn initialize(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn reserve_extent(&self, _: ExtentKey, _: u32) -> Result<WriteHandle, ExtentManagerError> {
        Err(ExtentManagerError::OutOfSpace)
    }
    fn get_extents(&self) -> Vec<Extent> {
        Vec::new()
    }
    fn for_each_extent(&self, _: &mut dyn FnMut(&Extent)) {}
    fn remove_extent(&self, offset: u64) -> Result<(), ExtentManagerError> {
        self.removed.lock().unwrap().push(offset);
        Ok(())
    }
    fn checkpoint(&self) -> Result<(), ExtentManagerError> {
        Ok(())
    }
    fn get_instance_id(&self) -> Result<u64, ExtentManagerError> {
        Ok(0)
    }
    fn set_checkpoint_interval(&self, _: Option<Duration>) {}
    fn used_bytes(&self) -> u64 {
        100
    }
    fn capacity_bytes(&self) -> u64 {
        100
    }
    fn set_metadata_base_lba(&self, _: u64) {}
    fn set_data_base_lba(&self, _: u64) {}
    fn data_base_lba(&self) -> u64 {
        0
    }
}

/// DIS-EVICTOR-NO-FOREIGN-PIN-RELEASE: the evictor's candidate probe missed the
/// key (it ran before the concurrent populate registered the entry), so the
/// release that follows must never happen: it must not drop the write-through
/// pin the populate left for its background write job.
#[test]
fn ssd_evictor_never_releases_a_foreign_read_reference() {
    let mt = Arc::new(Mt::default());
    let dm = Arc::new(Dm::new());
    let em = Arc::new(Em::default());

    // Arm the registration race: the evictor's probe will observe the key
    // absent and only then let the concurrent populate register it.
    dm.arm_registration_race();

    let mut evictor = BackgroundEvictor::start(
        Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>,
        Arc::clone(&mt) as Arc<dyn IMemoryTier + Send + Sync>,
        vec![Arc::clone(&em) as Arc<dyn IExtentManager + Send + Sync>],
        EvictorConfig {
            threshold: 0.5,
            low_watermark: 0.1,
            batch_size: 4,
            interval: Duration::from_millis(5),
        },
        None,
        Arc::new(Mutex::new(None)),
        Arc::new(TierEventCounters::default()),
    );

    // The concurrent populate, as the real one drives the map: reserve the
    // memory-tier slot, register the entry (write ref), downgrade that ref to
    // the write-through pin (read ref = 1) the background job settles under.
    let dm_p = Arc::clone(&dm);
    let mt_p = Arc::clone(&mt);
    let populate = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !dm_p.race_fired() {
            assert!(
                Instant::now() < deadline,
                "the evictor never probed the candidate"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let ptr = mt_p.insert(KEY, BLOCK).unwrap();
        dm_p.create_memory_tier_entry(KEY, ptr, BLOCK).unwrap();
        dm_p.downgrade_reference(KEY).unwrap();
    });
    populate.join().unwrap();

    // A few more passes, so a balanced probe (lookup + release of its own
    // reference) would have run too.
    std::thread::sleep(Duration::from_millis(50));
    evictor.shutdown();

    assert_eq!(
        dm.loc(KEY),
        Some(Loc::Memory(SLOT_ADDR, BLOCK)),
        "the concurrently registered entry is gone"
    );
    assert_eq!(
        dm.read_refs(KEY),
        Some(1),
        "the evictor released a read reference its own lookup did not acquire: \
         the write-through pin of the concurrently registered entry was dropped, \
         so a later remove frees its slot while the write job reads it"
    );
    assert!(
        mt.contains(KEY),
        "the pinned entry's memory-tier slot was freed"
    );
    assert!(
        em.removed.lock().unwrap().is_empty(),
        "the pinned entry's extent was freed"
    );
}
