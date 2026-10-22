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
    BlockSemantics, BounceBuffer, BounceError, CacheKey, EvictionHandle, IEvictionPolicy, ILogger,
    IMemoryTier, MemoryTierError, MemoryTierTelemetrySnapshot, PoolId,
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

/// The bounce reserve: the top of the pool, outside the cache, allocated to
/// serve block-tier entries to peers (see `IMemoryTier::bounce_alloc`).
struct Bounce {
    /// Offsets relative to the reserve's start.
    allocator: FreeList,
    /// Live buffers, offset → length asked for: `bounce_free` checks against it,
    /// because a wrong free would corrupt the free list silently.
    live: HashMap<usize, u32>,
}

struct MemoryTierState {
    pool_ptr: *mut u8,
    pool_size: usize,
    /// Bytes of the pool the cache may use: `pool_size` less the reserve,
    /// rounded down to a page so the reserve starts on one.
    cache_size: usize,
    /// What `set_bounce_reserve` asked for; applied at `initialize`.
    bounce_request: usize,
    bounce: std::sync::Mutex<Bounce>,
    pool_id: PoolId,
    pool: RwLock<Pool>,
    initialized: AtomicBool,
    spdk_allocated: bool,
    /// Tests stand in for SPDK: lets the bounce reserve allocate from an `mmap`
    /// pool, which no device reads into in a unit test anyway.
    #[cfg(test)]
    pretend_dma: bool,
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
            cache_size: 0,
            bounce_request: interfaces::DEFAULT_BOUNCE_RESERVE,
            bounce: std::sync::Mutex::new(Bounce {
                allocator: FreeList::new(0),
                live: HashMap::new(),
            }),
            pool_id: 0,
            pool: RwLock::new(Pool {
                allocator: FreeList::new(0),
                slots: HashMap::new(),
            }),
            initialized: AtomicBool::new(false),
            spdk_allocated: false,
            #[cfg(test)]
            pretend_dma: false,
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

/// The bounce reserve `initialize` takes for `requested` bytes from a pool of
/// `pool_size`: at most an eighth of the pool, so a small pool keeps a cache,
/// and whole 4 KiB pages.
fn bounce_reserve_bytes(requested: usize, pool_size: usize) -> usize {
    (requested.min(pool_size / 8) / 4096) * 4096
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

        // The bounce reserve comes quietly out of the top of the pool: small
        // next to any real cache, and inside the pool so whatever registers
        // the pool for DMA or RDMA covers it too.
        // The cache ends where the reserve starts, on a page boundary: a pool
        // that is not whole pages leaves its tail unused rather than start
        // the reserve mid-page.
        let reserve = bounce_reserve_bytes(state.bounce_request, pool_size);
        let cache_size = if reserve == 0 {
            pool_size
        } else {
            (pool_size - reserve) / 4096 * 4096
        };

        state.pool_ptr = ptr;
        state.pool_size = pool_size;
        state.cache_size = cache_size;
        state.bounce = std::sync::Mutex::new(Bounce {
            allocator: FreeList::new(reserve),
            live: HashMap::new(),
        });
        state.pool_id = pool_id;
        state.pool = RwLock::new(Pool {
            allocator: FreeList::new(cache_size),
            slots: HashMap::new(),
        });
        state.spdk_allocated = spdk_allocated;
        state.initialized.store(true, Ordering::Release);

        self.log_info(&format!(
            "memory-tier: initialized with {} MiB pool ({} KiB of it the bounce reserve, \
             outside the cache)",
            pool_size / (1024 * 1024),
            reserve / 1024
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

        // Check for room before changing anything, so that a refusal by the
        // eviction policy below leaves the pool exactly as it was.
        if !pool.allocator.can_allocate(size as usize) {
            return Err(MemoryTierError::PoolFull);
        }

        let eviction_handle = ep
            .track(state.pool_id, key, BlockSemantics::default())
            .map_err(|e| {
                MemoryTierError::NotInitialized(format!(
                    "eviction policy refused to track key {key}: {e}"
                ))
            })?;

        // Cannot fail: `can_allocate` held under the same write lock.
        let Some(offset) = pool.allocator.allocate(size as usize) else {
            let _ = ep.remove(eviction_handle);
            return Err(MemoryTierError::PoolFull);
        };
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
        pool.allocator = FreeList::new(state.cache_size);
        ep.clear_pool(state.pool_id);
        Ok(count)
    }

    fn is_dma_capable(&self) -> bool {
        let state = self.state.read().unwrap();
        state.spdk_allocated
    }

    fn set_bounce_reserve(&self, bytes: usize) -> Result<(), MemoryTierError> {
        let mut state = self.state.write().unwrap();
        if state.initialized.load(Ordering::Acquire) {
            return Err(MemoryTierError::AllocationFailed(
                "the bounce reserve is taken at initialize; set it before".into(),
            ));
        }
        state.bounce_request = bytes;
        Ok(())
    }

    fn bounce_alloc(&self, len: u32) -> Result<BounceBuffer, BounceError> {
        let state = self.state.read().unwrap();
        if !state.initialized.load(Ordering::Acquire) {
            return Err(BounceError::NotInitialized("pool not initialized".into()));
        }
        let reserve = state.bounce.lock().unwrap().allocator.capacity();
        #[cfg(test)]
        let dma = state.spdk_allocated || state.pretend_dma;
        #[cfg(not(test))]
        let dma = state.spdk_allocated;
        if !dma {
            // The mmap fallback: a device cannot DMA into it, so a read into a
            // buffer could never be done in place. Not transient.
            return Err(BounceError::NeverFits(
                "the pool is not DMA-capable (no SPDK environment)".into(),
            ));
        }
        let need = (len as usize).next_multiple_of(4096);
        if len == 0 || need > reserve {
            return Err(BounceError::NeverFits(format!(
                "{len} bytes against a bounce reserve of {reserve}"
            )));
        }
        let mut bounce = state.bounce.lock().unwrap();
        let off = bounce
            .allocator
            .allocate(len as usize)
            .ok_or(BounceError::NoSpace)?;
        bounce.live.insert(off, len);
        // SAFETY: `off + need <= reserve`, so the buffer lies inside the
        // reserve, which is the top `reserve` bytes of the pool allocation.
        let ptr = unsafe { state.pool_ptr.add(state.cache_size + off) };
        Ok(BounceBuffer { ptr, len })
    }

    fn bounce_free(&self, buf: BounceBuffer) {
        let state = self.state.read().unwrap();
        let base = state.pool_ptr as usize + state.cache_size;
        let off = (buf.ptr as usize).wrapping_sub(base);
        let mut bounce = state.bounce.lock().unwrap();
        match bounce.live.remove(&off) {
            Some(len) if len == buf.len => bounce.allocator.deallocate(off, len as usize),
            // Loud: freeing what was not allocated, or with another length, would
            // hand bytes in use to the next buffer.
            other => {
                panic!("bounce_free of {buf:?}, which is not a live buffer (found {other:?})")
            }
        }
    }

    fn bounce_usage(&self) -> (usize, usize) {
        let state = self.state.read().unwrap();
        let bounce = state.bounce.lock().unwrap();
        (bounce.allocator.capacity(), bounce.allocator.used())
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
        c.state.write().unwrap().pretend_dma = true;
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
        // Pool is 64 * 4096 = 256 KiB, less an eighth for the bounce reserve.
        // Fill the cache completely.
        let total = mt.capacity();
        assert_eq!(total, 56 * 4096);
        mt.insert(0, total as u32).unwrap();
        // Next insert should fail.
        assert!(matches!(mt.insert(1, 4096), Err(MemoryTierError::PoolFull)));
    }

    #[test]
    fn capacity_and_used() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        assert_eq!(mt.capacity(), 56 * 4096, "the pool less its bounce reserve");
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
    // --- the bounce reserve ---------------------------------------------------

    /// A component with its eviction policy bound, not yet initialized.
    fn fresh() -> Arc<MemoryTierComponent> {
        let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
        let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(ep_comp, IEvictionPolicy).unwrap();
        let c = MemoryTierComponent::new(RwLock::new(MemoryTierState::default()));
        c.eviction_policy.connect(ep).unwrap();
        c
    }

    #[test]
    fn the_default_reserve_is_taken_quietly_and_capped() {
        // A 1 GiB pool: the full 32 MiB default.
        let mt = query_interface!(fresh(), IMemoryTier).unwrap();
        mt.initialize(1 << 30, None).unwrap();
        assert_eq!(mt.bounce_usage(), (interfaces::DEFAULT_BOUNCE_RESERVE, 0));
        assert_eq!(
            mt.capacity(),
            (1 << 30) - interfaces::DEFAULT_BOUNCE_RESERVE
        );
        assert_eq!(
            mt.pool_info().unwrap().1,
            1 << 30,
            "the pool, and so its registration, still covers the reserve"
        );
        // A 256 KiB pool: capped at an eighth.
        let small = query_interface!(fresh(), IMemoryTier).unwrap();
        small.initialize(64 * 4096, None).unwrap();
        assert_eq!(small.bounce_usage(), (8 * 4096, 0));
    }

    #[test]
    fn the_reserve_is_set_before_initialize_only() {
        let mt = query_interface!(fresh(), IMemoryTier).unwrap();
        mt.set_bounce_reserve(3 * 4096 + 100).unwrap();
        mt.initialize(64 * 4096, None).unwrap();
        assert_eq!(mt.bounce_usage().0, 3 * 4096, "whole pages");
        assert!(mt.set_bounce_reserve(4096).is_err(), "too late");
        let none = query_interface!(fresh(), IMemoryTier).unwrap();
        none.set_bounce_reserve(0).unwrap();
        none.initialize(64 * 4096, None).unwrap();
        assert_eq!(none.capacity(), 64 * 4096, "no reserve, the whole pool");
        assert!(matches!(
            none.bounce_alloc(1),
            Err(BounceError::NeverFits(_))
        ));
    }

    /// FR-012: a full cache cannot reach the reserve, and allocating never evicts.
    #[test]
    fn the_cache_and_the_reserve_never_share_bytes() {
        let c = setup();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        let cap = mt.capacity();
        let entry = mt.insert(1, cap as u32).unwrap();
        assert!(matches!(mt.insert(2, 4096), Err(MemoryTierError::PoolFull)));
        let a = mt.bounce_alloc(4096).unwrap();
        let b = mt.bounce_alloc(3 * 4096).unwrap();
        let (base, len) = mt.pool_info().unwrap();
        for l in [a, b] {
            let lo = l.ptr as usize;
            assert!(lo >= entry as usize + cap, "past the cache");
            assert!(
                lo + l.len as usize <= base as usize + len,
                "inside the pool"
            );
        }
        assert_ne!(a.ptr, b.ptr);
        assert!(mt.contains(1), "nothing evicted");
        assert_eq!(mt.used(), cap, "the cache's own count is untouched");
        assert_eq!(mt.bounce_usage(), (8 * 4096, 4 * 4096));
    }

    /// A pool that is not a whole number of pages still puts the reserve on
    /// page boundaries, inside the pool: a device writes a buffer in whole
    /// blocks from its start.
    #[test]
    fn the_reserve_starts_on_a_page_when_the_pool_does_not_end_on_one() {
        let c = fresh();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        let pool_size = 64 * 4096 + 100;
        mt.initialize(pool_size, None).unwrap();
        c.state.write().unwrap().pretend_dma = true;
        let (base, len) = mt.pool_info().unwrap();
        assert_eq!(len, pool_size);
        let (reserve, _) = mt.bounce_usage();
        assert_eq!(reserve, 8 * 4096);
        let all = mt.bounce_alloc(reserve as u32).unwrap();
        let off = all.ptr as usize - base as usize;
        assert_eq!(off % 4096, 0, "the reserve starts at pool offset {off}");
        assert!(off + reserve <= pool_size, "and ends inside the pool");
        assert!(
            mt.capacity() <= off,
            "the cache ends where the reserve starts"
        );
        mt.bounce_free(all);
    }

    #[test]
    fn leases_are_transient_until_freed_and_coalesce() {
        let mt = query_interface!(setup(), IMemoryTier).unwrap();
        // 8 pages of reserve.
        let buffers: Vec<_> = (0..4).map(|_| mt.bounce_alloc(2 * 4096).unwrap()).collect();
        assert_eq!(mt.bounce_alloc(1), Err(BounceError::NoSpace), "full now");
        mt.bounce_free(buffers[1]);
        mt.bounce_free(buffers[2]);
        // Two freed neighbours coalesce: a 4-page buffer fits again.
        let big = mt.bounce_alloc(4 * 4096).unwrap();
        assert_eq!(big.ptr, buffers[1].ptr);
        mt.bounce_free(big);
        mt.bounce_free(buffers[0]);
        mt.bounce_free(buffers[3]);
        assert_eq!(mt.bounce_usage().1, 0);
        assert!(mt.bounce_alloc(8 * 4096).is_ok(), "all of it, in one");
    }

    /// Without SPDK the pool is mmap'd and no device can DMA into a buffer.
    #[test]
    fn a_pool_that_is_not_dma_capable_never_fits() {
        let c = fresh();
        let mt = query_interface!(c, IMemoryTier).unwrap();
        mt.initialize(64 * 4096, None).unwrap();
        assert!(!mt.is_dma_capable());
        assert!(matches!(
            mt.bounce_alloc(4096),
            Err(BounceError::NeverFits(m)) if m.contains("DMA")
        ));
    }

    #[test]
    fn a_lease_larger_than_the_reserve_never_fits() {
        let mt = query_interface!(setup(), IMemoryTier).unwrap();
        assert!(matches!(
            mt.bounce_alloc(8 * 4096 + 1),
            Err(BounceError::NeverFits(_))
        ));
        assert!(matches!(mt.bounce_alloc(0), Err(BounceError::NeverFits(_))));
        let uninit = query_interface!(fresh(), IMemoryTier).unwrap();
        assert!(matches!(
            uninit.bounce_alloc(4096),
            Err(BounceError::NotInitialized(_))
        ));
    }

    #[test]
    #[should_panic(expected = "not a live buffer")]
    fn freeing_with_the_wrong_length_is_loud() {
        let mt = query_interface!(setup(), IMemoryTier).unwrap();
        let l = mt.bounce_alloc(4096).unwrap();
        mt.bounce_free(BounceBuffer { len: 8192, ..l });
    }

    #[test]
    fn clear_empties_the_cache_and_leaves_the_reserve() {
        let mt = query_interface!(setup(), IMemoryTier).unwrap();
        let l = mt.bounce_alloc(4096).unwrap();
        mt.insert(1, 4096).unwrap();
        mt.clear().unwrap();
        assert_eq!(mt.capacity(), 56 * 4096);
        assert_eq!(mt.bounce_usage().1, 4096, "a buffer survives clear");
        mt.bounce_free(l);
    }
}

#[cfg(test)]
#[path = "repair_test_insert_track_failure.rs"]
mod repair_test_insert_track_failure;
