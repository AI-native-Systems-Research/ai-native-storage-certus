//! Memory-tier component for the Certus storage system.
//!
//! Provides a DRAM-resident cache pool with pluggable eviction (delegated to a
//! bound `IEvictionPolicy`). Objects are
//! allocated from a contiguous pre-allocated pool and tracked by key.
//! The pool uses a first-fit free-list allocator with 4 KiB alignment.
//!
//! Uses a single `RwLock<Pool>` for concurrency: read operations (`get`,
//! `peek`, `batch_touch`, `contains`) take a shared lock while mutations
//! (`insert`, `remove`, `evict`) take an exclusive lock. Eviction-order touches
//! are performed after releasing the pool lock (the eviction policy has its
//! own internal synchronization).
//!
//! Provides the [`IMemoryTier`] interface with receptacles for [`ILogger`]
//! and [`IEvictionPolicy`].

mod allocator;

use std::collections::HashMap;
#[cfg(feature = "telemetry")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

use component_framework::define_component;
use interfaces::{
    CacheKey, EvictionHandle, IEvictionPolicy, ILogger, IMemoryTier, MemoryTierError,
    MemoryTierTelemetrySnapshot, PoolId,
};

use crate::allocator::FreeList;

/// Default memory-tier pool size (256 MiB).
pub const DEFAULT_POOL_SIZE: usize = 256 * 1024 * 1024;

/// Telemetry counters for the memory-tier (zero-cost when `telemetry` feature is disabled).
#[cfg(feature = "telemetry")]
#[derive(Default)]
pub struct MemoryTierTelemetry {
    pub evictions: AtomicU64,
    pub write_lock_contentions: AtomicU64,
    pub read_lock_contentions: AtomicU64,
}

#[cfg(feature = "telemetry")]
impl MemoryTierTelemetry {
    pub fn snapshot(&self) -> TelemetrySnapshot {
        TelemetrySnapshot {
            evictions: self.evictions.load(Ordering::Relaxed),
            write_lock_contentions: self.write_lock_contentions.load(Ordering::Relaxed),
            read_lock_contentions: self.read_lock_contentions.load(Ordering::Relaxed),
        }
    }

    pub fn reset(&self) {
        self.evictions.store(0, Ordering::Relaxed);
        self.write_lock_contentions.store(0, Ordering::Relaxed);
        self.read_lock_contentions.store(0, Ordering::Relaxed);
    }
}

#[cfg(feature = "telemetry")]
#[derive(Debug, Clone, Copy)]
pub struct TelemetrySnapshot {
    pub evictions: u64,
    pub write_lock_contentions: u64,
    pub read_lock_contentions: u64,
}

struct Slot {
    offset: usize,
    size: u32,
    eviction_handle: EvictionHandle,
}

struct Pool {
    allocator: FreeList,
    slots: HashMap<CacheKey, Slot>,
}

struct MemoryTierState {
    pool_ptr: *mut u8,
    pool_size: usize,
    pool_id: PoolId,
    pool: RwLock<Pool>,
    initialized: AtomicBool,
    spdk_allocated: bool,
    #[cfg(feature = "telemetry")]
    telemetry: MemoryTierTelemetry,
}

// SAFETY: pool_ptr points to mmap'd or SPDK-allocated memory accessible from any thread.
// RwLock<Pool> serializes access. Immutable fields (pool_ptr, pool_size, spdk_allocated)
// are only written during initialize() which is protected by the outer RwLock on the component.
unsafe impl Send for MemoryTierState {}
unsafe impl Sync for MemoryTierState {}

impl Default for MemoryTierState {
    fn default() -> Self {
        Self {
            pool_ptr: std::ptr::null_mut(),
            pool_size: 0,
            pool_id: 0,
            pool: RwLock::new(Pool {
                allocator: FreeList::new(0),
                slots: HashMap::new(),
            }),
            initialized: AtomicBool::new(false),
            spdk_allocated: false,
            #[cfg(feature = "telemetry")]
            telemetry: MemoryTierTelemetry::default(),
        }
    }
}

impl Drop for MemoryTierState {
    fn drop(&mut self) {
        if self.pool_ptr.is_null() {
            return;
        }
        #[cfg(feature = "spdk")]
        if self.spdk_allocated {
            if interfaces::is_spdk_env_active() {
                unsafe {
                    spdk_sys::spdk_free(self.pool_ptr as *mut std::ffi::c_void);
                }
            }
            self.pool_ptr = std::ptr::null_mut();
            return;
        }
        unsafe {
            libc::munmap(self.pool_ptr as *mut libc::c_void, self.pool_size);
        }
        self.pool_ptr = std::ptr::null_mut();
    }
}

define_component! {
    pub MemoryTierComponent {
        version: "0.3.0",
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

    fn log_warn(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.warn(msg);
        }
    }

    /// Returns the telemetry counters (only available with `telemetry` feature).
    #[cfg(feature = "telemetry")]
    pub fn telemetry(&self) -> TelemetrySnapshot {
        let state = self.state.read().unwrap();
        state.telemetry.snapshot()
    }

    /// Resets all telemetry counters to zero.
    #[cfg(feature = "telemetry")]
    pub fn reset_telemetry(&self) {
        let state = self.state.read().unwrap();
        state.telemetry.reset();
    }

    /// Returns free capacity in bytes (capacity - used).
    pub fn free_capacity(&self) -> usize {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return 0;
        }
        let pool = state.pool.read().unwrap();
        pool.allocator.capacity() - pool.allocator.used()
    }

    /// Fallback pool allocation via mmap (used when SPDK is unavailable).
    fn alloc_mmap(
        &self,
        pool_size: usize,
        numa_node: Option<i32>,
    ) -> Result<*mut u8, MemoryTierError> {
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

        if let Some(node) = numa_node {
            if node >= 0 {
                let node_id = node as usize;
                let mut nodemask: libc::c_ulong = 0;
                if node_id < (std::mem::size_of::<libc::c_ulong>() * 8) {
                    nodemask = 1 << node_id;
                }
                // SAFETY: ptr is a valid mmap'd region. mbind binds pages to a NUMA node.
                let rc = unsafe {
                    libc::syscall(
                        libc::SYS_mbind,
                        ptr,
                        pool_size,
                        libc::MPOL_BIND,
                        &nodemask as *const libc::c_ulong,
                        node_id + 2,
                        0u32,
                    )
                };
                if rc == 0 {
                    self.log_info(&format!("memory-tier: pool bound to NUMA node {node_id}"));
                } else {
                    self.log_warn(&format!(
                        "memory-tier: mbind to NUMA node {node_id} failed (errno={}), \
                         using default memory policy",
                        std::io::Error::last_os_error()
                    ));
                }
            }
        }

        Ok(ptr as *mut u8)
    }
}

impl IMemoryTier for MemoryTierComponent {
    fn initialize(&self, pool_size: usize, numa_node: Option<i32>) -> Result<(), MemoryTierError> {
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

        #[cfg(feature = "spdk")]
        let (ptr, spdk_allocated) = if interfaces::is_spdk_env_active() {
            const SPDK_MALLOC_DMA: u32 = 0x01;
            let node_id = numa_node.unwrap_or(-1);
            // SAFETY: spdk_zmalloc returns hugepage-backed, zero-initialized memory or NULL.
            let p = unsafe {
                spdk_sys::spdk_zmalloc(
                    pool_size,
                    4096,
                    std::ptr::null_mut(),
                    node_id,
                    SPDK_MALLOC_DMA,
                )
            };
            if p.is_null() {
                return Err(MemoryTierError::AllocationFailed(
                    "spdk_zmalloc failed (insufficient hugepages?)".into(),
                ));
            }
            self.log_info(&format!(
                "memory-tier: allocated from SPDK hugepages (NUMA node {})",
                node_id
            ));
            (p as *mut u8, true)
        } else {
            (self.alloc_mmap(pool_size, numa_node)?, false)
        };

        #[cfg(not(feature = "spdk"))]
        let (ptr, spdk_allocated) = (self.alloc_mmap(pool_size, numa_node)?, false);

        let pool_id = ep.create_pool();

        state.pool_ptr = ptr;
        state.pool_size = pool_size;
        state.pool_id = pool_id;
        state.pool = RwLock::new(Pool {
            allocator: FreeList::new(pool_size),
            slots: HashMap::new(),
        });
        state.spdk_allocated = spdk_allocated;
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
            return Err(MemoryTierError::NotInitialized(
                "pool not initialized".into(),
            ));
        }

        let ep = self.eviction_policy.get().unwrap();

        #[cfg(feature = "telemetry")]
        let mut pool = match state.pool.try_write() {
            Ok(guard) => guard,
            Err(_) => {
                state
                    .telemetry
                    .write_lock_contentions
                    .fetch_add(1, Ordering::Relaxed);
                state.pool.write().unwrap()
            }
        };
        #[cfg(not(feature = "telemetry"))]
        let mut pool = state.pool.write().unwrap();

        if pool.slots.contains_key(&key) {
            return Err(MemoryTierError::AlreadyExists(key));
        }

        let offset = pool
            .allocator
            .allocate(size as usize)
            .ok_or(MemoryTierError::PoolFull)?;

        let eviction_handle = ep.track(state.pool_id, key).unwrap();
        pool.slots.insert(
            key,
            Slot {
                offset,
                size,
                eviction_handle,
            },
        );

        let ptr = unsafe { state.pool_ptr.add(offset) };
        Ok(ptr)
    }

    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return None;
        }

        let ep = self.eviction_policy.get().unwrap();

        #[cfg(feature = "telemetry")]
        let pool = match state.pool.try_read() {
            Ok(guard) => guard,
            Err(_) => {
                state
                    .telemetry
                    .read_lock_contentions
                    .fetch_add(1, Ordering::Relaxed);
                state.pool.read().unwrap()
            }
        };
        #[cfg(not(feature = "telemetry"))]
        let pool = state.pool.read().unwrap();

        let slot = pool.slots.get(&key)?;
        let ptr = unsafe { state.pool_ptr.add(slot.offset) };
        let size = slot.size;
        let handle = slot.eviction_handle;
        drop(pool);
        let _ = ep.touch(handle);
        Some((ptr, size))
    }

    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return None;
        }

        let pool = state.pool.read().unwrap();
        let slot = pool.slots.get(&key)?;
        let ptr = unsafe { state.pool_ptr.add(slot.offset) };
        Some((ptr, slot.size))
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) || n == 0 {
            return Vec::new();
        }

        let ep = self.eviction_policy.get().unwrap();
        ep.get_eviction_candidates(state.pool_id, n)
    }

    fn evict_next(&self) -> Option<CacheKey> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return None;
        }

        let ep = self.eviction_policy.get().unwrap();
        if let Some(key) = ep.identify_next_to_evict(state.pool_id) {
            #[cfg(feature = "telemetry")]
            let mut pool = match state.pool.try_write() {
                Ok(guard) => guard,
                Err(_) => {
                    state
                        .telemetry
                        .write_lock_contentions
                        .fetch_add(1, Ordering::Relaxed);
                    state.pool.write().unwrap()
                }
            };
            #[cfg(not(feature = "telemetry"))]
            let mut pool = state.pool.write().unwrap();

            if let Some(slot) = pool.slots.remove(&key) {
                pool.allocator.deallocate(slot.offset, slot.size as usize);
            }
            #[cfg(feature = "telemetry")]
            state.telemetry.evictions.fetch_add(1, Ordering::Relaxed);

            Some(key)
        } else {
            None
        }
    }

    fn evict_next_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
        self.evict_next()
    }

    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return Err(MemoryTierError::NotInitialized(
                "pool not initialized".into(),
            ));
        }

        let ep = self.eviction_policy.get().unwrap();

        #[cfg(feature = "telemetry")]
        let mut pool = match state.pool.try_write() {
            Ok(guard) => guard,
            Err(_) => {
                state
                    .telemetry
                    .write_lock_contentions
                    .fetch_add(1, Ordering::Relaxed);
                state.pool.write().unwrap()
            }
        };
        #[cfg(not(feature = "telemetry"))]
        let mut pool = state.pool.write().unwrap();

        let slot = pool
            .slots
            .remove(&key)
            .ok_or(MemoryTierError::KeyNotFound(key))?;
        let _ = ep.remove(slot.eviction_handle);
        pool.allocator.deallocate(slot.offset, slot.size as usize);
        Ok(())
    }

    fn touch(&self, key: CacheKey) {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return;
        }

        let ep = self.eviction_policy.get().unwrap();
        let pool = state.pool.read().unwrap();
        if let Some(slot) = pool.slots.get(&key) {
            let handle = slot.eviction_handle;
            drop(pool);
            let _ = ep.touch(handle);
        }
    }

    fn batch_touch(&self, keys: &[CacheKey]) {
        if keys.is_empty() {
            return;
        }
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return;
        }
        let ep = match self.eviction_policy.get() {
            Ok(ep) => ep,
            Err(_) => return,
        };

        #[cfg(feature = "telemetry")]
        let pool = match state.pool.try_read() {
            Ok(guard) => guard,
            Err(_) => {
                state
                    .telemetry
                    .read_lock_contentions
                    .fetch_add(1, Ordering::Relaxed);
                state.pool.read().unwrap()
            }
        };
        #[cfg(not(feature = "telemetry"))]
        let pool = state.pool.read().unwrap();

        let mut handles = Vec::with_capacity(keys.len());
        for &key in keys {
            if let Some(slot) = pool.slots.get(&key) {
                handles.push(slot.eviction_handle);
            }
        }
        drop(pool);
        let _ = ep.batch_touch(&handles);
    }

    fn contains(&self, key: CacheKey) -> bool {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return false;
        }

        let pool = state.pool.read().unwrap();
        pool.slots.contains_key(&key)
    }

    fn capacity(&self) -> usize {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return 0;
        }
        let pool = state.pool.read().unwrap();
        pool.allocator.capacity()
    }

    fn used(&self) -> usize {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return 0;
        }
        let pool = state.pool.read().unwrap();
        pool.allocator.used()
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
            return Err(MemoryTierError::NotInitialized(
                "pool not initialized".into(),
            ));
        }
        let ep = self.eviction_policy.get().unwrap();
        let mut pool = state.pool.write().unwrap();
        let count = pool.slots.len();
        pool.slots.clear();
        pool.allocator = FreeList::new(state.pool_size);
        ep.clear_pool(state.pool_id);
        Ok(count)
    }

    fn is_dma_capable(&self) -> bool {
        let state = self.state.read().unwrap();
        state.spdk_allocated
    }

    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        #[cfg(feature = "telemetry")]
        {
            let state = self.state.read().unwrap();
            MemoryTierTelemetrySnapshot {
                evictions: state.telemetry.evictions.load(Ordering::Relaxed),
                write_lock_contentions: state
                    .telemetry
                    .write_lock_contentions
                    .load(Ordering::Relaxed),
                read_lock_contentions: state
                    .telemetry
                    .read_lock_contentions
                    .load(Ordering::Relaxed),
            }
        }
        #[cfg(not(feature = "telemetry"))]
        {
            MemoryTierTelemetrySnapshot::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;
    use std::sync::Arc;

    fn setup() -> Arc<MemoryTierComponent> {
        let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
        let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(ep_comp, IEvictionPolicy).unwrap();

        let c = MemoryTierComponent::new(RwLock::new(MemoryTierState::default()));
        c.eviction_policy.connect(ep).unwrap();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.initialize(64 * 4096, None).unwrap();
        c
    }

    #[test]
    fn initialize_twice_fails() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert!(mt.initialize(4096, None).is_err());
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
    fn remove_and_reuse() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.remove(1).unwrap();
        assert!(mt.get(1).is_none());
        mt.insert(1, 4096).unwrap();
    }

    #[test]
    fn evict_next_returns_some() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(0, 4096).unwrap();
        mt.insert(1, 4096).unwrap();
        let evicted = mt.evict_next();
        assert!(evicted.is_some());
    }

    #[test]
    fn pool_full_returns_error() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        // Pool is 64 * 4096 = 256 KiB. Fill it completely.
        let total = 64 * 4096;
        mt.insert(0, total as u32).unwrap();
        // Next insert should fail.
        assert!(matches!(mt.insert(1, 4096), Err(MemoryTierError::PoolFull)));
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
    fn touch_updates_eviction_order() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(0, 4096).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.insert(2, 4096).unwrap();
        // Touch key 0 — makes it most recently used.
        mt.touch(0);
        // Evict should return key 1 (oldest untouched).
        assert_eq!(mt.evict_next(), Some(1));
    }

    #[test]
    fn peek_does_not_update_eviction_order() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.insert(0, 4096).unwrap();
        let result = mt.peek(0);
        assert!(result.is_some());
    }
}
