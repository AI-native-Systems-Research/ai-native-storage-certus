//! Memory-tier component for the Certus storage system.
//!
//! Provides a DRAM-resident cache pool with LRU eviction. Objects are
//! allocated from a contiguous pre-allocated pool and tracked by key.
//! The pool uses a first-fit free-list allocator with 4 KiB alignment.
//!
//! Provides the [`IMemoryTier`] interface with a receptacle for [`ILogger`].

mod allocator;
mod lru;

use std::collections::HashMap;
use std::sync::Mutex;

use component_framework::define_component;
use interfaces::{CacheKey, ILogger, IMemoryTier, MemoryTierError};

use crate::allocator::FreeList;
use crate::lru::LruList;

/// Default memory-tier pool size (256 MiB).
pub const DEFAULT_POOL_SIZE: usize = 256 * 1024 * 1024;

struct Slot {
    offset: usize,
    size: u32,
    lru_index: usize,
}

struct MemoryTierState {
    pool_ptr: *mut u8,
    pool_size: usize,
    allocator: FreeList,
    lru: LruList,
    slots: HashMap<CacheKey, Slot>,
    initialized: bool,
}

// SAFETY: The pool pointer refers to memory that is accessible from any thread.
// All access is serialized through the Mutex<MemoryTierState>.
unsafe impl Send for MemoryTierState {}

impl Default for MemoryTierState {
    fn default() -> Self {
        Self {
            pool_ptr: std::ptr::null_mut(),
            pool_size: 0,
            allocator: FreeList::new(0),
            lru: LruList::new(),
            slots: HashMap::new(),
            initialized: false,
        }
    }
}

impl Drop for MemoryTierState {
    fn drop(&mut self) {
        if !self.pool_ptr.is_null() {
            // SAFETY: pool_ptr was allocated with mmap in initialize().
            unsafe {
                libc::munmap(self.pool_ptr as *mut libc::c_void, self.pool_size);
            }
            self.pool_ptr = std::ptr::null_mut();
        }
    }
}

define_component! {
    pub MemoryTierComponent {
        version: "0.1.0",
        provides: [IMemoryTier],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            state: Mutex<MemoryTierState>,
        },
    }
}

impl MemoryTierComponent {
    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }
}

impl IMemoryTier for MemoryTierComponent {
    fn initialize(&self, pool_size: usize) -> Result<(), MemoryTierError> {
        if pool_size == 0 {
            return Err(MemoryTierError::InvalidSize);
        }

        let mut state = self.state.lock().unwrap();
        if state.initialized {
            return Err(MemoryTierError::AllocationFailed(
                "already initialized".into(),
            ));
        }

        // Allocate pool using mmap with MAP_HUGETLB for hugepage backing.
        // Falls back to regular anonymous mmap if hugepages unavailable.
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
            // Fallback: regular anonymous mmap without hugepages.
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
                return Err(MemoryTierError::AllocationFailed(
                    "mmap failed".into(),
                ));
            }
            p
        } else {
            ptr
        };

        state.pool_ptr = ptr as *mut u8;
        state.pool_size = pool_size;
        state.allocator = FreeList::new(pool_size);
        state.lru = LruList::new();
        state.slots = HashMap::new();
        state.initialized = true;

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

        let mut state = self.state.lock().unwrap();
        if !state.initialized {
            return Err(MemoryTierError::NotInitialized("pool not initialized".into()));
        }
        if state.slots.contains_key(&key) {
            return Err(MemoryTierError::AlreadyExists(key));
        }

        let offset = state
            .allocator
            .allocate(size as usize)
            .ok_or(MemoryTierError::PoolFull)?;

        let lru_index = state.lru.push_back(key);
        state.slots.insert(
            key,
            Slot {
                offset,
                size,
                lru_index,
            },
        );

        // SAFETY: offset is within bounds of the pool allocation.
        let ptr = unsafe { state.pool_ptr.add(offset) };
        Ok(ptr)
    }

    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let mut state = self.state.lock().unwrap();
        let slot = state.slots.get(&key)?;
        let ptr = unsafe { state.pool_ptr.add(slot.offset) };
        let size = slot.size;
        let lru_index = slot.lru_index;
        state.lru.move_to_back(lru_index);
        Some((ptr, size))
    }

    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let state = self.state.lock().unwrap();
        let slot = state.slots.get(&key)?;
        let ptr = unsafe { state.pool_ptr.add(slot.offset) };
        Some((ptr, slot.size))
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let state = self.state.lock().unwrap();
        state.lru.peek_front_n(n)
    }

    fn evict_lru(&self) -> Option<CacheKey> {
        let mut state = self.state.lock().unwrap();
        let key = state.lru.pop_front()?;
        if let Some(slot) = state.slots.remove(&key) {
            state.allocator.deallocate(slot.offset, slot.size as usize);
        }
        Some(key)
    }

    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let mut state = self.state.lock().unwrap();
        let slot = state
            .slots
            .remove(&key)
            .ok_or(MemoryTierError::KeyNotFound(key))?;
        state.lru.remove(slot.lru_index);
        state.allocator.deallocate(slot.offset, slot.size as usize);
        Ok(())
    }

    fn touch(&self, key: CacheKey) {
        let mut state = self.state.lock().unwrap();
        if let Some(slot) = state.slots.get(&key) {
            let idx = slot.lru_index;
            state.lru.move_to_back(idx);
        }
    }

    fn contains(&self, key: CacheKey) -> bool {
        let state = self.state.lock().unwrap();
        state.slots.contains_key(&key)
    }

    fn capacity(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.allocator.capacity()
    }

    fn used(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.allocator.used()
    }

    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        let state = self.state.lock().unwrap();
        if state.initialized && !state.pool_ptr.is_null() {
            Some((state.pool_ptr, state.pool_size))
        } else {
            None
        }
    }

    fn clear(&self) -> Result<usize, MemoryTierError> {
        let mut state = self.state.lock().unwrap();
        if !state.initialized {
            return Err(MemoryTierError::NotInitialized("pool not initialized".into()));
        }
        let count = state.slots.len();
        state.slots.clear();
        state.lru = LruList::new();
        state.allocator = FreeList::new(state.pool_size);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    fn setup() -> std::sync::Arc<MemoryTierComponent> {
        let c = MemoryTierComponent::new(Mutex::new(MemoryTierState::default()));
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.initialize(64 * 4096).unwrap(); // 256 KiB pool
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
        assert!(matches!(mt.insert(1, 0), Err(MemoryTierError::InvalidSize)));
    }

    #[test]
    fn pool_full_returns_error() {
        let c = MemoryTierComponent::new(Mutex::new(MemoryTierState::default()));
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.initialize(8192).unwrap(); // 8 KiB pool
        mt.insert(1, 4096).unwrap();
        mt.insert(2, 4096).unwrap();
        assert!(matches!(mt.insert(3, 4096), Err(MemoryTierError::PoolFull)));
    }

    #[test]
    fn evict_lru_returns_oldest() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.insert(2, 4096).unwrap();
        mt.insert(3, 4096).unwrap();
        assert_eq!(mt.evict_lru(), Some(1));
        assert_eq!(mt.evict_lru(), Some(2));
        assert_eq!(mt.evict_lru(), Some(3));
        assert_eq!(mt.evict_lru(), None);
    }

    #[test]
    fn touch_updates_lru_order() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.insert(2, 4096).unwrap();
        mt.insert(3, 4096).unwrap();
        mt.touch(1);
        assert_eq!(mt.evict_lru(), Some(2));
        assert_eq!(mt.evict_lru(), Some(3));
        assert_eq!(mt.evict_lru(), Some(1));
    }

    #[test]
    fn get_updates_lru_order() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.insert(2, 4096).unwrap();
        mt.insert(3, 4096).unwrap();
        mt.get(1);
        assert_eq!(mt.evict_lru(), Some(2));
    }

    #[test]
    fn remove_frees_space() {
        let c = MemoryTierComponent::new(Mutex::new(MemoryTierState::default()));
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.initialize(8192).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.insert(2, 4096).unwrap();
        assert!(matches!(mt.insert(3, 4096), Err(MemoryTierError::PoolFull)));
        mt.remove(1).unwrap();
        mt.insert(3, 4096).unwrap();
        assert!(mt.contains(3));
    }

    #[test]
    fn remove_nonexistent_fails() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert!(matches!(
            mt.remove(99),
            Err(MemoryTierError::KeyNotFound(99))
        ));
    }

    #[test]
    fn contains_works() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert!(!mt.contains(1));
        mt.insert(1, 4096).unwrap();
        assert!(mt.contains(1));
    }

    #[test]
    fn capacity_and_used() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert_eq!(mt.capacity(), 64 * 4096);
        assert_eq!(mt.used(), 0);
        mt.insert(1, 4096).unwrap();
        assert_eq!(mt.used(), 4096);
        mt.insert(2, 8192).unwrap();
        assert_eq!(mt.used(), 4096 + 8192);
    }

    #[test]
    fn evict_frees_space_for_new_insert() {
        let c = MemoryTierComponent::new(Mutex::new(MemoryTierState::default()));
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.initialize(12288).unwrap(); // 12 KiB
        mt.insert(1, 4096).unwrap();
        mt.insert(2, 4096).unwrap();
        mt.insert(3, 4096).unwrap();
        assert!(matches!(mt.insert(4, 4096), Err(MemoryTierError::PoolFull)));
        mt.evict_lru(); // evicts key 1
        mt.insert(4, 4096).unwrap();
        assert!(mt.contains(4));
        assert!(!mt.contains(1));
    }

    #[test]
    fn data_integrity() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        let ptr = mt.insert(1, 4096).unwrap();
        // Write a pattern
        unsafe {
            std::ptr::write_bytes(ptr, 0xAB, 4096);
        }
        // Read it back
        let (got_ptr, _) = mt.get(1).unwrap();
        let slice = unsafe { std::slice::from_raw_parts(got_ptr, 4096) };
        assert!(slice.iter().all(|&b| b == 0xAB));
    }
}

/// Create a new MemoryTier component instance.
///
/// Exported for dynamic loading. Returns a `ComponentRef` wrapping
/// a default-configured `MemoryTierComponent`.
#[cfg(feature = "dylib")]
#[no_mangle]
pub fn create_component_memory_tier() -> component_core::component_ref::ComponentRef {
    component_core::component_ref::ComponentRef::from(
        MemoryTierComponent::new_default() as std::sync::Arc<_>,
    )
}
