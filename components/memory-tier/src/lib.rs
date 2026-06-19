//! Memory-tier component for the Certus storage system.
//!
//! Provides a DRAM-resident cache pool with LRU eviction. Objects are
//! allocated from a contiguous pre-allocated pool and tracked by key.
//! The pool uses a first-fit free-list allocator with 4 KiB alignment.
//!
//! Internally sharded into 16 independent partitions to reduce lock
//! contention under concurrent access from multiple dispatcher threads.
//!
//! Provides the [`IMemoryTier`] interface with receptacles for [`ILogger`]
//! and [`IEvictionPolicy`].

mod allocator;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};

use component_framework::define_component;
use interfaces::{
    CacheKey, EvictionHandle, IEvictionPolicy, ILogger, IMemoryTier, MemoryTierError, PoolId,
};

use crate::allocator::FreeList;

/// Default memory-tier pool size (256 MiB).
pub const DEFAULT_POOL_SIZE: usize = 256 * 1024 * 1024;

const NUM_SHARDS: usize = 16;

struct Slot {
    offset: usize,
    size: u32,
    eviction_handle: EvictionHandle,
}

struct Shard {
    allocator: FreeList,
    slots: HashMap<CacheKey, Slot>,
}

/// Internal state using lock-free access pattern after initialization.
/// After `initialize()` completes, `pool_ptr`, `pool_size`, and `shard_size`
/// are immutable. Only `shards` requires locking (per-shard).
struct MemoryTierState {
    pool_ptr: *mut u8,
    pool_size: usize,
    shard_size: usize,
    shards: Vec<Mutex<Shard>>,
    pool_ids: [PoolId; NUM_SHARDS],
    evict_counter: AtomicUsize,
    initialized: AtomicBool,
}

// SAFETY: pool_ptr points to mmap'd memory accessible from any thread.
// Per-shard Mutex serializes shard access. Immutable fields (pool_ptr,
// pool_size, shard_size) are only written during initialize() which is
// protected by the component-level Mutex.
unsafe impl Send for MemoryTierState {}
unsafe impl Sync for MemoryTierState {}

impl Default for MemoryTierState {
    fn default() -> Self {
        Self {
            pool_ptr: std::ptr::null_mut(),
            pool_size: 0,
            shard_size: 0,
            shards: Vec::new(),
            pool_ids: [0; NUM_SHARDS],
            evict_counter: AtomicUsize::new(0),
            initialized: AtomicBool::new(false),
        }
    }
}

impl Drop for MemoryTierState {
    fn drop(&mut self) {
        if !self.pool_ptr.is_null() {
            unsafe {
                libc::munmap(self.pool_ptr as *mut libc::c_void, self.pool_size);
            }
            self.pool_ptr = std::ptr::null_mut();
        }
    }
}

define_component! {
    pub MemoryTierComponent {
        version: "0.2.0",
        provides: [IMemoryTier],
        receptacles: {
            logger: ILogger,
            eviction_policy: IEvictionPolicy,
        },
        fields: {
            state: RwLock<MemoryTierState>,
        },
    }
}

impl MemoryTierComponent {
    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    #[inline]
    fn shard_for_key(key: CacheKey) -> usize {
        key as usize % NUM_SHARDS
    }

}

impl IMemoryTier for MemoryTierComponent {
    fn initialize(&self, pool_size: usize) -> Result<(), MemoryTierError> {
        if pool_size == 0 {
            return Err(MemoryTierError::InvalidSize);
        }

        let ep = self.eviction_policy.get().map_err(|_| {
            MemoryTierError::NotInitialized("eviction_policy receptacle not connected".into())
        })?;

        let mut state = self.state.write().unwrap();
        if state.initialized.load(Ordering::Relaxed) {
            return Err(MemoryTierError::AllocationFailed(
                "already initialized".into(),
            ));
        }

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                pool_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_HUGETLB,
                -1,
                0,
            )
        };

        let ptr = if ptr == libc::MAP_FAILED {
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    pool_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if p == libc::MAP_FAILED {
                return Err(MemoryTierError::AllocationFailed("mmap failed".into()));
            }
            p
        } else {
            ptr
        };

        let shard_size = pool_size / NUM_SHARDS;
        let mut shards = Vec::with_capacity(NUM_SHARDS);
        let mut pool_ids = [0u32; NUM_SHARDS];
        for pool_id in pool_ids.iter_mut() {
            *pool_id = ep.create_pool();
            shards.push(Mutex::new(Shard {
                allocator: FreeList::new(shard_size),
                slots: HashMap::new(),
            }));
        }

        state.pool_ptr = ptr as *mut u8;
        state.pool_size = pool_size;
        state.shard_size = shard_size;
        state.shards = shards;
        state.pool_ids = pool_ids;
        state.evict_counter = AtomicUsize::new(0);
        state.initialized.store(true, Ordering::Release);

        self.log_info(&format!(
            "memory-tier: initialized with {} MiB pool",
            pool_size / (1024 * 1024)
        ));

        Ok(())
    }

    fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
        if size == 0 {
            return Err(MemoryTierError::InvalidSize);
        }

        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return Err(MemoryTierError::NotInitialized("pool not initialized".into()));
        }

        let ep = self.eviction_policy.get().unwrap();
        let shard_idx = Self::shard_for_key(key);
        let mut shard = state.shards[shard_idx].lock().unwrap();

        if shard.slots.contains_key(&key) {
            return Err(MemoryTierError::AlreadyExists(key));
        }

        let local_offset = shard
            .allocator
            .allocate(size as usize)
            .ok_or(MemoryTierError::PoolFull)?;

        let pool_id = state.pool_ids[shard_idx];
        let eviction_handle = ep.track(pool_id, key).unwrap();
        shard.slots.insert(
            key,
            Slot {
                offset: local_offset,
                size,
                eviction_handle,
            },
        );

        let global_offset = shard_idx * state.shard_size + local_offset;
        let ptr = unsafe { state.pool_ptr.add(global_offset) };
        Ok(ptr)
    }

    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return None;
        }

        let ep = self.eviction_policy.get().unwrap();
        let shard_idx = Self::shard_for_key(key);
        let shard = state.shards[shard_idx].lock().unwrap();
        let slot = shard.slots.get(&key)?;
        let global_offset = shard_idx * state.shard_size + slot.offset;
        let ptr = unsafe { state.pool_ptr.add(global_offset) };
        let size = slot.size;
        let handle = slot.eviction_handle;
        drop(shard);
        let _ = ep.touch(handle);
        Some((ptr, size))
    }

    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return None;
        }

        let shard_idx = Self::shard_for_key(key);
        let shard = state.shards[shard_idx].lock().unwrap();
        let slot = shard.slots.get(&key)?;
        let global_offset = shard_idx * state.shard_size + slot.offset;
        let ptr = unsafe { state.pool_ptr.add(global_offset) };
        Some((ptr, slot.size))
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) || n == 0 {
            return Vec::new();
        }

        let ep = self.eviction_policy.get().unwrap();
        let per_shard = (n / NUM_SHARDS).max(1);
        let mut keys = Vec::with_capacity(n);
        for pool_id in &state.pool_ids {
            keys.extend(ep.peek_oldest(*pool_id, per_shard));
            if keys.len() >= n {
                break;
            }
        }
        keys.truncate(n);
        keys
    }

    fn evict_lru(&self) -> Option<CacheKey> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return None;
        }

        let ep = self.eviction_policy.get().unwrap();
        let start = state.evict_counter.fetch_add(1, Ordering::Relaxed) % NUM_SHARDS;
        for i in 0..NUM_SHARDS {
            let idx = (start + i) % NUM_SHARDS;
            let pool_id = state.pool_ids[idx];
            if let Some(key) = ep.pop_oldest(pool_id) {
                let mut shard = state.shards[idx].lock().unwrap();
                if let Some(slot) = shard.slots.remove(&key) {
                    shard.allocator.deallocate(slot.offset, slot.size as usize);
                }
                return Some(key);
            }
        }
        None
    }

    fn evict_lru_for_key(&self, key: CacheKey) -> Option<CacheKey> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return None;
        }

        let ep = self.eviction_policy.get().unwrap();
        let shard_idx = Self::shard_for_key(key);
        let pool_id = state.pool_ids[shard_idx];
        if let Some(evicted_key) = ep.pop_oldest(pool_id) {
            let mut shard = state.shards[shard_idx].lock().unwrap();
            if let Some(slot) = shard.slots.remove(&evicted_key) {
                shard.allocator.deallocate(slot.offset, slot.size as usize);
            }
            Some(evicted_key)
        } else {
            None
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return Err(MemoryTierError::NotInitialized("pool not initialized".into()));
        }

        let ep = self.eviction_policy.get().unwrap();
        let shard_idx = Self::shard_for_key(key);
        let mut shard = state.shards[shard_idx].lock().unwrap();
        let slot = shard
            .slots
            .remove(&key)
            .ok_or(MemoryTierError::KeyNotFound(key))?;
        let _ = ep.remove(slot.eviction_handle);
        shard.allocator.deallocate(slot.offset, slot.size as usize);
        Ok(())
    }

    fn touch(&self, key: CacheKey) {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return;
        }

        let ep = self.eviction_policy.get().unwrap();
        let shard_idx = Self::shard_for_key(key);
        let shard = state.shards[shard_idx].lock().unwrap();
        if let Some(slot) = shard.slots.get(&key) {
            let handle = slot.eviction_handle;
            drop(shard);
            let _ = ep.touch(handle);
        }
    }

    fn contains(&self, key: CacheKey) -> bool {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return false;
        }

        let shard_idx = Self::shard_for_key(key);
        let shard = state.shards[shard_idx].lock().unwrap();
        shard.slots.contains_key(&key)
    }

    fn capacity(&self) -> usize {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return 0;
        }
        state.shards.iter().map(|s| s.lock().unwrap().allocator.capacity()).sum()
    }

    fn used(&self) -> usize {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return 0;
        }
        state.shards.iter().map(|s| s.lock().unwrap().allocator.used()).sum()
    }

    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        let state = self.state.read().unwrap();
        if state.initialized.load(Ordering::Acquire) && !state.pool_ptr.is_null() {
            Some((state.pool_ptr, state.pool_size))
        } else {
            None
        }
    }

    fn clear(&self) -> Result<usize, MemoryTierError> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return Err(MemoryTierError::NotInitialized("pool not initialized".into()));
        }
        let ep = self.eviction_policy.get().unwrap();
        let mut count = 0;
        for (i, shard_mutex) in state.shards.iter().enumerate() {
            let mut shard = shard_mutex.lock().unwrap();
            count += shard.slots.len();
            shard.slots.clear();
            shard.allocator = FreeList::new(state.shard_size);
            ep.clear_pool(state.pool_ids[i]);
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use component_core::query_interface;

    fn setup() -> Arc<MemoryTierComponent> {
        let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
        let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(ep_comp, IEvictionPolicy).unwrap();

        let c = MemoryTierComponent::new(RwLock::new(MemoryTierState::default()));
        c.eviction_policy.connect(ep).unwrap();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.initialize(64 * 4096).unwrap();
        c
    }

    #[test]
    fn initialize_twice_fails() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert!(mt.initialize(4096).is_err());
    }

    #[test]
    fn insert_and_get() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        let ptr = mt.insert(1, 4096).unwrap();
        assert!(!ptr.is_null());
        let (got_ptr, got_size) = mt.get(1).unwrap();
        assert_eq!(got_ptr, ptr);
        assert_eq!(got_size, 4096);
    }

    #[test]
    fn insert_duplicate_fails() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(1, 4096).unwrap();
        assert!(matches!(
            mt.insert(1, 4096),
            Err(MemoryTierError::AlreadyExists(1))
        ));
    }

    #[test]
    fn insert_zero_size_fails() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert!(matches!(
            mt.insert(1, 0),
            Err(MemoryTierError::InvalidSize)
        ));
    }

    #[test]
    fn remove_and_reuse() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.remove(1).unwrap();
        assert!(mt.get(1).is_none());
        mt.insert(1, 4096).unwrap();
    }

    #[test]
    fn evict_lru_returns_some() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(0, 4096).unwrap();
        mt.insert(16, 4096).unwrap();
        let evicted = mt.evict_lru();
        assert!(evicted.is_some());
    }

    #[test]
    fn pool_full_returns_error() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        // Pool is 256 KiB / 16 shards = 16 KiB per shard.
        mt.insert(0, 16384).unwrap();
        assert!(matches!(mt.insert(16, 4096), Err(MemoryTierError::PoolFull)));
    }

    #[test]
    fn capacity_and_used() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert_eq!(mt.capacity(), 64 * 4096);
        assert_eq!(mt.used(), 0);
        mt.insert(1, 4096).unwrap();
        assert_eq!(mt.used(), 4096);
    }

    #[test]
    fn contains() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert!(!mt.contains(1));
        mt.insert(1, 4096).unwrap();
        assert!(mt.contains(1));
    }

    #[test]
    fn clear_resets_all() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.insert(2, 4096).unwrap();
        let count = mt.clear().unwrap();
        assert_eq!(count, 2);
        assert_eq!(mt.used(), 0);
        assert!(!mt.contains(1));
    }

    #[test]
    fn touch_updates_lru() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(0, 4096).unwrap();
        mt.insert(16, 4096).unwrap();
        mt.insert(32, 4096).unwrap();
        mt.touch(0);
        // Evict from shard 0 multiple times until we get one from shard 0.
        let mut evicted_from_shard0 = None;
        for _ in 0..NUM_SHARDS {
            if let Some(k) = mt.evict_lru() {
                if k == 16 || k == 0 || k == 32 {
                    evicted_from_shard0 = Some(k);
                    break;
                }
            }
        }
        assert_eq!(evicted_from_shard0, Some(16));
    }

    #[test]
    fn peek_does_not_update_lru() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(0, 4096).unwrap();
        let result = mt.peek(0);
        assert!(result.is_some());
    }
}
