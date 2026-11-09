//! Mirror of the dispatch map (`components/dispatch-map/src/lib.rs`,
//! `components/dispatch-map/src/state.rs`) as the dispatcher sees it through
//! `IDispatchMap`. The eviction policy (`IEvictionPolicy`, LRU) is a second lock,
//! touched after the map lock is dropped, as in the real `lookup`.

use crate::stand::{Ev, EvLog};
use crate::{CacheKey, Mutation};
use loom::sync::{Condvar, Mutex, MutexGuard};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    BlockDevice { offset: u64 },
    MemoryTier { pointer: usize, size: u32, ssd_offset: Option<u64> },
}

#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub location: Location,
    pub read_ref: u32,
    pub write_ref: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupResult {
    NotExist,
    MemoryTier { pointer: usize, size: u32 },
    BlockDevice { offset: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmError {
    KeyNotFound(CacheKey),
    ActiveReferences(CacheKey),
    AlreadyExists(CacheKey),
    InvalidState,
    RefCountUnderflow(CacheKey),
    NoWriteReference(CacheKey),
}

pub struct DispatchMap {
    inner: Mutex<HashMap<CacheKey, Entry>>,
    condvar: Condvar,
    /// IEvictionPolicy LRU order, oldest first.
    ep: Mutex<Vec<CacheKey>>,
    m: Mutation,
    pub log: Option<std::sync::Arc<EvLog>>,
}

impl DispatchMap {
    pub fn new(m: Mutation) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            condvar: Condvar::new(),
            ep: Mutex::new(Vec::new()),
            m,
            log: None,
        }
    }

    // mirrors components/dispatch-map/src/state.rs:54 (wait_for). The 2 s
    // DEFAULT_TIMEOUT is abstracted away: the wait never times out.
    fn wait_for<F>(&self, mut predicate: F) -> MutexGuard<'_, HashMap<CacheKey, Entry>>
    where
        F: FnMut(&HashMap<CacheKey, Entry>) -> bool,
    {
        let mut guard = self.inner.lock().unwrap();
        loop {
            if predicate(&guard) {
                return guard;
            }
            guard = self.condvar.wait(guard).unwrap();
        }
    }

    fn ep_touch(&self, key: CacheKey) {
        let mut ep = self.ep.lock().unwrap();
        if let Some(p) = ep.iter().position(|k| *k == key) {
            ep.remove(p);
            ep.push(key);
        }
    }

    // mirrors components/dispatch-map/src/lib.rs:128 (lookup)
    pub fn lookup(&self, key: CacheKey) -> Result<LookupResult, DmError> {
        let exclusive = self.m == Mutation::LookupExclusive;
        let mut inner = self.wait_for(|inner| match inner.get(&key) {
            None => true,
            // MUTANT(LookupExclusive): readers also wait for other readers.
            Some(e) if exclusive => e.write_ref == 0 && e.read_ref == 0,
            Some(e) => e.write_ref == 0,
        });
        let entry = match inner.get_mut(&key) {
            None => return Ok(LookupResult::NotExist),
            Some(e) => e,
        };
        entry.read_ref += 1;
        let result = match entry.location {
            Location::BlockDevice { offset } => LookupResult::BlockDevice { offset },
            Location::MemoryTier { pointer, size, .. } => LookupResult::MemoryTier { pointer, size },
        };
        drop(inner);
        self.ep_touch(key);
        Ok(result)
    }

    // mirrors components/dispatch-map/src/lib.rs:176 (convert_to_storage)
    pub fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DmError> {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner.get_mut(&key).ok_or(DmError::KeyNotFound(key))?;
        match &mut entry.location {
            Location::MemoryTier { ssd_offset, .. } => *ssd_offset = Some(offset),
            Location::BlockDevice { .. } => return Err(DmError::InvalidState),
        }
        if entry.read_ref > 0 {
            entry.read_ref -= 1;
        }
        drop(inner);
        self.condvar.notify_all();
        Ok(())
    }

    // mirrors components/dispatch-map/src/lib.rs:267 (release_read)
    pub fn release_read(&self, key: CacheKey) -> Result<(), DmError> {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner.get_mut(&key).ok_or(DmError::KeyNotFound(key))?;
        if entry.read_ref == 0 {
            return Err(DmError::RefCountUnderflow(key));
        }
        entry.read_ref -= 1;
        drop(inner);
        self.condvar.notify_all();
        Ok(())
    }

    // mirrors components/dispatch-map/src/lib.rs:210 (take_read)
    pub fn take_read(&self, key: CacheKey) -> Result<(), DmError> {
        let mut inner = self.wait_for(|inner| inner.get(&key).map_or(true, |e| e.write_ref == 0));
        let entry = inner.get_mut(&key).ok_or(DmError::KeyNotFound(key))?;
        entry.read_ref += 1;
        Ok(())
    }

    // mirrors components/dispatch-map/src/lib.rs:307 (downgrade_reference)
    pub fn downgrade_reference(&self, key: CacheKey) -> Result<(), DmError> {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner.get_mut(&key).ok_or(DmError::KeyNotFound(key))?;
        if entry.write_ref == 0 {
            return Err(DmError::NoWriteReference(key));
        }
        entry.write_ref = 0;
        entry.read_ref += 1;
        drop(inner);
        self.condvar.notify_all();
        Ok(())
    }

    // mirrors components/dispatch-map/src/lib.rs:334 (remove)
    pub fn remove(&self, key: CacheKey) -> Result<(), DmError> {
        let mut inner = if self.m == Mutation::RemoveWaitsForRefs {
            // MUTANT(RemoveWaitsForRefs): block until the references drain.
            self.wait_for(|i| i.get(&key).map_or(true, |e| e.read_ref == 0 && e.write_ref == 0))
        } else {
            self.inner.lock().unwrap()
        };
        let entry = inner.get(&key).ok_or(DmError::KeyNotFound(key))?;
        if (entry.read_ref > 0 || entry.write_ref > 0)
            && self.m != Mutation::RemoveIgnoresRefs
        {
            return Err(DmError::ActiveReferences(key));
        }
        // MUTANT(RemoveIgnoresRefs): the entry is dropped although references
        // are still held.
        inner.remove(&key);
        // ep.remove(handle) runs with the map lock still held.
        self.ep.lock().unwrap().retain(|k| *k != key);
        drop(inner);
        if let Some(l) = &self.log {
            l.push(Ev::DmRemove(key));
        }
        Ok(())
    }

    // mirrors components/dispatch-map/src/lib.rs:359 (touch)
    pub fn touch(&self, key: CacheKey) -> Result<(), DmError> {
        let inner = self.inner.lock().unwrap();
        inner.get(&key).ok_or(DmError::KeyNotFound(key))?;
        drop(inner);
        self.ep_touch(key);
        Ok(())
    }

    // mirrors components/dispatch-map/src/lib.rs:382 (oldest_keys)
    pub fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        self.ep.lock().unwrap().iter().take(n).copied().collect()
    }

    // mirrors components/dispatch-map/src/lib.rs:393 (create_memory_tier_entry)
    pub fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        pointer: usize,
        size: u32,
    ) -> Result<(), DmError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.contains_key(&key) {
            return Err(DmError::AlreadyExists(key));
        }
        self.ep.lock().unwrap().push(key);
        inner.insert(
            key,
            Entry {
                location: Location::MemoryTier { pointer, size, ssd_offset: None },
                read_ref: 0,
                write_ref: 1,
            },
        );
        Ok(())
    }

    // mirrors components/dispatch-map/src/lib.rs:441 (convert_memory_tier_to_block):
    // flips a resident entry that carries its write-through copy to its
    // block-device state, keeping every reference count.
    pub fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DmError> {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner.get_mut(&key).ok_or(DmError::KeyNotFound(key))?;
        match entry.location {
            Location::MemoryTier { ssd_offset: Some(offset), .. } => {
                entry.location = Location::BlockDevice { offset };
                drop(inner);
                self.condvar.notify_all();
                Ok(())
            }
            Location::MemoryTier { ssd_offset: None, .. } => Err(DmError::InvalidState),
            Location::BlockDevice { .. } => Err(DmError::InvalidState),
        }
    }

    // mirrors components/dispatch-map/src/lib.rs:482 (promote_block_to_memory_tier)
    pub fn promote_block_to_memory_tier(
        &self,
        key: CacheKey,
        pointer: usize,
        size: u32,
    ) -> Result<(), DmError> {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner.get_mut(&key).ok_or(DmError::KeyNotFound(key))?;
        match entry.location {
            Location::BlockDevice { offset } => {
                entry.location =
                    Location::MemoryTier { pointer, size, ssd_offset: Some(offset) };
            }
            Location::MemoryTier { .. } => return Err(DmError::InvalidState),
        }
        drop(inner);
        self.condvar.notify_all();
        Ok(())
    }

    // mirrors components/dispatch-map/src/lib.rs:530 (is_evictable)
    pub fn is_evictable(&self, key: CacheKey) -> bool {
        let inner = self.inner.lock().unwrap();
        match inner.get(&key) {
            Some(e) => {
                e.read_ref == 0
                    && e.write_ref == 0
                    && matches!(e.location, Location::MemoryTier { ssd_offset: Some(_), .. })
            }
            None => false,
        }
    }

    // mirrors components/dispatch-map/src/lib.rs:548 (try_evict_to_block)
    pub fn try_evict_to_block(&self, key: CacheKey) -> Result<(), DmError> {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner.get_mut(&key).ok_or(DmError::KeyNotFound(key))?;
        if entry.read_ref != 0 || entry.write_ref != 0 {
            return Err(DmError::ActiveReferences(key));
        }
        match entry.location {
            Location::MemoryTier { ssd_offset: Some(offset), .. } => {
                entry.location = Location::BlockDevice { offset };
                Ok(())
            }
            _ => Err(DmError::InvalidState),
        }
    }

    // mirrors components/dispatch-map/src/lib.rs:582 (recover_extent)
    pub fn recover_extent(&self, key: CacheKey, offset: u64) -> Result<(), DmError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.contains_key(&key) {
            return Err(DmError::AlreadyExists(key));
        }
        self.ep.lock().unwrap().push(key);
        inner.insert(
            key,
            Entry { location: Location::BlockDevice { offset }, read_ref: 0, write_ref: 0 },
        );
        Ok(())
    }

    /// Test seeding: install an entry with explicit state (as left by earlier,
    /// fully completed operations).
    pub fn seed(&self, key: CacheKey, e: Entry) {
        self.ep.lock().unwrap().push(key);
        self.inner.lock().unwrap().insert(key, e);
    }

    /// The mutation this mirror runs under.
    pub fn mutation(&self) -> Mutation {
        self.m
    }

    /// Test observation.
    pub fn entry(&self, key: CacheKey) -> Option<Entry> {
        self.inner.lock().unwrap().get(&key).copied()
    }
}
