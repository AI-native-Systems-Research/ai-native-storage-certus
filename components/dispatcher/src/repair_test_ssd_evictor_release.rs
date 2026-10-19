//! Repair test for finding `ssd-evictor-frees-before-remove`
//! (DIS-SPIN-SSD-EVICTOR-RELEASES-AFTER-REMOVE).
//!
//! The SSD evictor may release a key's memory-tier slot only after the
//! dispatch-map removal of that key succeeds. Here a client cold-promotes the
//! candidate into the memory tier and pins it in the window between the
//! evictor's `get_evictable_offset` (which saw `BlockDevice`) and its removal.
//! `dm.remove` then fails on the pin, and the slot the surviving entry points at
//! must still be allocated.
//!
//! Self-contained: it drives `BackgroundEvictor::start` through its own minimal
//! mocks, so it builds against the unfixed code as well.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use interfaces::{
    CacheKey, DispatchMapError, Extent, ExtentKey, ExtentManagerError, FormatParams, IDispatchMap,
    IExtentManager, IMemoryTier, LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot,
    WriteHandle,
};

use crate::background::{BackgroundEvictor, EvictorConfig};
use crate::TierEventCounters;

const KEY: CacheKey = 1;
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
        self.slots.lock().unwrap().insert(key, (SLOT_ADDR, size));
        Ok(SLOT_ADDR as *mut u8)
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

// --- dispatch map: one entry, with a client that promotes and pins it ---

#[derive(Clone, Copy, Debug, PartialEq)]
enum Loc {
    Block(u64),
    // Pointer stored as usize so the mock stays Send + Sync without an unsafe impl.
    Memory(usize, u32),
}

struct Ent {
    loc: Loc,
    read_refs: u32,
}

struct Dm {
    entries: Mutex<HashMap<CacheKey, Ent>>,
    mt: Arc<Mt>,
    /// Fire the client's promote-and-pin on the next `release_read(KEY)`.
    promote_armed: Mutex<bool>,
    remove_attempts: AtomicUsize,
}

impl Dm {
    /// The client: cold-promote `KEY` into the memory tier in place and pin it
    /// with a reader, as `batch_lookup`'s promote path does.
    fn promote_and_pin(&self, entries: &mut HashMap<CacheKey, Ent>) {
        let ptr = self.mt.insert(KEY, 4096).unwrap() as usize;
        let e = entries.get_mut(&KEY).unwrap();
        e.loc = Loc::Memory(ptr, 4096);
        e.read_refs += 1;
    }
}

impl IDispatchMap for Dm {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        match entries.get_mut(&key) {
            None => Ok(LookupResult::NotExist),
            Some(e) => {
                e.read_refs += 1;
                Ok(match e.loc {
                    Loc::Block(offset) => LookupResult::BlockDevice { offset },
                    Loc::Memory(p, size) => LookupResult::MemoryTier {
                        pointer: p as *mut u8,
                        size,
                    },
                })
            }
        }
    }
    fn convert_to_storage(&self, _: CacheKey, _: u64) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn take_read(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn take_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        e.read_refs = e.read_refs.saturating_sub(1);
        // The evictor has just classified KEY as BlockDevice and dropped its
        // pin; the client now wins the race.
        let mut armed = self.promote_armed.lock().unwrap();
        if key == KEY && *armed {
            *armed = false;
            self.promote_and_pin(&mut entries);
        }
        Ok(())
    }
    fn release_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn downgrade_reference(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        self.remove_attempts.fetch_add(1, Ordering::SeqCst);
        let mut entries = self.entries.lock().unwrap();
        match entries.get(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(e) if e.read_refs > 0 => Err(DispatchMapError::ActiveReferences(key)),
            Some(_) => {
                entries.remove(&key);
                Ok(())
            }
        }
    }
    fn touch(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(key))
    }
    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        self.entries
            .lock()
            .unwrap()
            .keys()
            .copied()
            .take(n)
            .collect()
    }
    fn create_memory_tier_entry(
        &self,
        _: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn convert_memory_tier_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn promote_block_to_memory_tier(
        &self,
        _: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn is_evictable(&self, _: CacheKey) -> bool {
        false
    }
    fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        Err(DispatchMapError::KeyNotFound(key))
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

#[test]
fn ssd_evictor_keeps_slot_when_remove_fails() {
    let mt = Arc::new(Mt::default());
    let dm = Arc::new(Dm {
        entries: Mutex::new(HashMap::from([(
            KEY,
            Ent {
                loc: Loc::Block(0x4000),
                read_refs: 0,
            },
        )])),
        mt: Arc::clone(&mt),
        promote_armed: Mutex::new(true),
        remove_attempts: AtomicUsize::new(0),
    });
    let em = Arc::new(Em::default());

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
        None,
        Arc::new(TierEventCounters::default()),
    );

    // Wait until the evictor's pass has run past the race window (the client
    // promoted KEY), plus a few more passes.
    let deadline = Instant::now() + Duration::from_secs(5);
    while *dm.promote_armed.lock().unwrap() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(50));
    evictor.shutdown();

    assert!(
        !*dm.promote_armed.lock().unwrap(),
        "the evictor never classified KEY as evictable"
    );

    let entries = dm.entries.lock().unwrap();
    let e = entries
        .get(&KEY)
        .expect("pinned entry must survive the failed removal");
    assert_eq!(e.loc, Loc::Memory(SLOT_ADDR, 4096));
    assert_eq!(e.read_refs, 1, "the client's pin is still held");
    assert!(
        mt.contains(KEY),
        "SSD evictor released KEY's memory-tier slot although dm.remove(KEY) failed: \
         the surviving MemoryTier entry (pinned by a reader) points at a freed slot \
         ({} removal attempt(s))",
        dm.remove_attempts.load(Ordering::SeqCst)
    );
    assert!(
        em.removed.lock().unwrap().is_empty(),
        "the surviving entry's extent was freed"
    );
}
