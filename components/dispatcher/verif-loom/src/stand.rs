//! Deterministic stand-ins for the non-concurrency dependencies: crossbeam
//! channels, the memory tier, the extent manager + NVMe drive, and an
//! observation log. None of these are under verification; they only have to
//! keep the synchronization the real ones give (one lock per object).

use crate::CacheKey;
use loom::cell::UnsafeCell;
use loom::sync::{Condvar, Mutex};
use std::collections::{HashMap, VecDeque};

/// Slot size of the memory-tier stand-in (one slot = one 4 KiB block).
pub const SLOT: u32 = 4096;

// ---------------------------------------------------------------------------
// Channels (crossbeam_channel stand-ins)
// ---------------------------------------------------------------------------

/// Unbounded MPSC channel with disconnect, standing in for
/// `crossbeam_channel::unbounded` (loom's mpsc does not model hang-up).
pub struct Chan<T> {
    q: Mutex<(VecDeque<T>, bool)>,
    cv: Condvar,
    cap: Option<usize>,
}

impl<T> Chan<T> {
    pub fn unbounded() -> Self {
        Self { q: Mutex::new((VecDeque::new(), false)), cv: Condvar::new(), cap: None }
    }

    /// `crossbeam_channel::bounded(cap)`; only `try_send` is used on it.
    pub fn bounded(cap: usize) -> Self {
        Self { q: Mutex::new((VecDeque::new(), false)), cv: Condvar::new(), cap: Some(cap) }
    }

    pub fn send(&self, v: T) -> Result<(), T> {
        let mut g = self.q.lock().unwrap();
        if g.1 {
            return Err(v);
        }
        g.0.push_back(v);
        drop(g);
        self.cv.notify_all();
        Ok(())
    }

    /// Never blocks: fails when the bounded queue is full.
    pub fn try_send(&self, v: T) -> Result<(), T> {
        let mut g = self.q.lock().unwrap();
        if g.1 || self.cap.is_some_and(|c| g.0.len() >= c) {
            return Err(v);
        }
        g.0.push_back(v);
        drop(g);
        self.cv.notify_all();
        Ok(())
    }

    /// Blocking receive; `None` once the channel is closed and empty.
    pub fn recv(&self) -> Option<T> {
        let mut g = self.q.lock().unwrap();
        loop {
            if let Some(v) = g.0.pop_front() {
                return Some(v);
            }
            if g.1 {
                return None;
            }
            g = self.cv.wait(g).unwrap();
        }
    }

    pub fn try_recv(&self) -> Option<T> {
        self.q.lock().unwrap().0.pop_front()
    }

    pub fn close(&self) {
        self.q.lock().unwrap().1 = true;
        self.cv.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Observation log (not part of the model's synchronization)
// ---------------------------------------------------------------------------

/// Side effects recorded by the stand-ins, in program order of the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ev {
    MtFree(CacheKey),
    DmRemove(CacheKey),
    ExtentFree(u64),
}

/// A `std` mutex on purpose: recording must not add loom synchronization.
#[derive(Default)]
pub struct EvLog(pub std::sync::Mutex<Vec<Ev>>);

impl EvLog {
    pub fn push(&self, e: Ev) {
        self.0.lock().unwrap().push(e);
    }
    pub fn take(&self) -> Vec<Ev> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

// ---------------------------------------------------------------------------
// Memory tier stand-in
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtError {
    PoolFull,
    AlreadyExists(CacheKey),
    KeyNotFound(CacheKey),
}

struct MtInner {
    /// key -> (slot index, size)
    slots: HashMap<CacheKey, (usize, u32)>,
    free: Vec<usize>,
    /// LRU order, oldest first.
    lru: Vec<CacheKey>,
    used: usize,
}

/// A fixed pool of 4 KiB slots, one lock, LRU order. Modelled on the
/// `MockMemoryTier` used by the dispatcher's own unit tests
/// (src/lib.rs:4131) but with slot reuse and LRU order, which the evictors
/// depend on. A slot's payload is one `UnsafeCell<u64>` so loom flags any
/// unsynchronized write/read pair on it (the "DMA" into/out of the slot).
pub struct MemoryTier {
    inner: Mutex<MtInner>,
    data: Vec<UnsafeCell<u64>>,
    capacity: usize,
    pub log: Option<std::sync::Arc<EvLog>>,
    /// Observation only (a `std` lock, no loom synchronization): which slots are
    /// currently allocated, and how many slot reads hit a released slot.
    live: std::sync::Mutex<Vec<bool>>,
    stale_reads: std::sync::atomic::AtomicUsize,
}

// SAFETY: slot payloads are only accessed through `write_slot`/`read_slot`,
// whose synchronization is exactly what loom checks.
unsafe impl Sync for MemoryTier {}
unsafe impl Send for MemoryTier {}

impl MemoryTier {
    pub fn new(nslots: usize) -> Self {
        Self {
            inner: Mutex::new(MtInner {
                slots: HashMap::new(),
                free: (0..nslots).rev().collect(),
                lru: Vec::new(),
                used: 0,
            }),
            data: (0..nslots).map(|_| UnsafeCell::new(0)).collect(),
            capacity: nslots * SLOT as usize,
            log: None,
            live: std::sync::Mutex::new(vec![false; nslots]),
            stale_reads: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn insert(&self, key: CacheKey, size: u32) -> Result<usize, MtError> {
        let mut g = self.inner.lock().unwrap();
        if g.slots.contains_key(&key) {
            return Err(MtError::AlreadyExists(key));
        }
        let aligned = (size as usize).next_multiple_of(SLOT as usize);
        if g.used + aligned > self.capacity {
            return Err(MtError::PoolFull);
        }
        let slot = g.free.pop().ok_or(MtError::PoolFull)?;
        g.used += aligned;
        g.slots.insert(key, (slot, size));
        g.lru.push(key);
        self.live.lock().unwrap()[slot] = true;
        Ok(slot)
    }

    /// `get` refreshes LRU position; `peek` does not.
    pub fn get(&self, key: CacheKey) -> Option<(usize, u32)> {
        let mut g = self.inner.lock().unwrap();
        let v = g.slots.get(&key).copied()?;
        g.lru.retain(|k| *k != key);
        g.lru.push(key);
        Some(v)
    }

    pub fn peek(&self, key: CacheKey) -> Option<(usize, u32)> {
        self.inner.lock().unwrap().slots.get(&key).copied()
    }

    pub fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        self.inner.lock().unwrap().lru.iter().take(n).copied().collect()
    }

    pub fn remove(&self, key: CacheKey) -> Result<(), MtError> {
        let mut g = self.inner.lock().unwrap();
        match g.slots.remove(&key) {
            Some((slot, size)) => {
                g.used -= (size as usize).next_multiple_of(SLOT as usize);
                g.free.push(slot);
                g.lru.retain(|k| *k != key);
                self.live.lock().unwrap()[slot] = false;
                drop(g);
                if let Some(l) = &self.log {
                    l.push(Ev::MtFree(key));
                }
                Ok(())
            }
            None => Err(MtError::KeyNotFound(key)),
        }
    }

    pub fn touch(&self, key: CacheKey) {
        let mut g = self.inner.lock().unwrap();
        if g.slots.contains_key(&key) {
            g.lru.retain(|k| *k != key);
            g.lru.push(key);
        }
    }

    pub fn batch_touch(&self, keys: &[CacheKey]) {
        for k in keys {
            self.touch(*k);
        }
    }

    pub fn contains(&self, key: CacheKey) -> bool {
        self.inner.lock().unwrap().slots.contains_key(&key)
    }

    pub fn used(&self) -> usize {
        self.inner.lock().unwrap().used
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// GPU→DRAM DMA into a slot (`memcpy_d2h_async` + sync).
    pub fn write_slot(&self, slot: usize, v: u64) {
        self.data[slot].with_mut(|p| unsafe { *p = v });
    }

    /// DRAM→GPU / DRAM→SSD DMA out of a slot.
    pub fn read_slot(&self, slot: usize) -> u64 {
        if !self.live.lock().unwrap()[slot] {
            self.stale_reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.data[slot].with(|p| unsafe { *p })
    }

    /// Observation: slot reads that hit a released (unallocated) slot.
    pub fn stale_reads(&self) -> usize {
        self.stale_reads.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// Extent manager + NVMe drive stand-in (one data drive)
// ---------------------------------------------------------------------------

struct ExtInner {
    /// offset -> (key, data on SSD)
    published: HashMap<u64, (CacheKey, u64)>,
    reserved: HashMap<u64, CacheKey>,
    /// Reserved extents whose data has landed but which are not yet published.
    written: HashMap<u64, u64>,
    free: Vec<u64>,
}

/// One drive's extent manager and its blocks: fixed 4 KiB extents.
pub struct ExtentManager {
    inner: Mutex<ExtInner>,
    nextents: usize,
    pub log: Option<std::sync::Arc<EvLog>>,
    /// Injected NVMe write error: writes of this key's data fail.
    pub fail_write_key: Option<CacheKey>,
}

impl ExtentManager {
    pub fn new(nextents: usize) -> Self {
        Self {
            inner: Mutex::new(ExtInner {
                published: HashMap::new(),
                reserved: HashMap::new(),
                written: HashMap::new(),
                free: (0..nextents as u64).rev().map(|i| i * SLOT as u64).collect(),
            }),
            nextents,
            log: None,
            fail_write_key: None,
        }
    }

    pub fn reserve_extent(&self, key: CacheKey) -> Result<u64, ()> {
        let mut g = self.inner.lock().unwrap();
        let off = g.free.pop().ok_or(())?;
        g.reserved.insert(off, key);
        Ok(off)
    }

    /// `write_buffer_to_ssd`: lands `data` in the reserved extent, or fails
    /// with the injected I/O error.
    pub fn write(&self, off: u64, data: u64) -> Result<(), ()> {
        let mut g = self.inner.lock().unwrap();
        let key = *g.reserved.get(&off).expect("reserved extent");
        if self.fail_write_key == Some(key) {
            return Err(());
        }
        g.written.insert(off, data);
        Ok(())
    }

    /// `write_handle.publish()`: commit the written extent.
    pub fn publish(&self, off: u64) {
        let mut g = self.inner.lock().unwrap();
        let key = g.reserved.remove(&off).expect("reserved extent");
        let data = g.written.remove(&off).expect("written extent");
        g.published.insert(off, (key, data));
    }

    /// `write_handle` dropped unpublished: the reservation is returned.
    pub fn abort(&self, off: u64) {
        let mut g = self.inner.lock().unwrap();
        g.reserved.remove(&off);
        g.written.remove(&off);
        g.free.push(off);
    }

    /// `write_buffer_to_ssd` + `write_handle.publish()`.
    pub fn write_and_publish(&self, off: u64, data: u64) {
        self.write(off, data).expect("seeded write");
        self.publish(off);
    }

    /// Seed a committed extent (recovery / earlier write-through).
    pub fn seed(&self, key: CacheKey, data: u64) -> u64 {
        let off = self.reserve_extent(key).unwrap();
        self.write_and_publish(off, data);
        off
    }

    pub fn remove_extent(&self, off: u64) {
        let mut g = self.inner.lock().unwrap();
        if g.published.remove(&off).is_some() {
            g.free.push(off);
        }
        drop(g);
        if let Some(l) = &self.log {
            l.push(Ev::ExtentFree(off));
        }
    }

    pub fn used_bytes(&self) -> u64 {
        self.inner.lock().unwrap().published.len() as u64 * SLOT as u64
    }

    pub fn capacity_bytes(&self) -> u64 {
        self.nextents as u64 * SLOT as u64
    }

    pub fn data_of(&self, key: CacheKey) -> Option<u64> {
        let g = self.inner.lock().unwrap();
        g.published.values().find(|(k, _)| *k == key).map(|(_, d)| *d)
    }
}
