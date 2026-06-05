//! In-memory LRU cache tier for hot-path lookups.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

struct CacheEntry {
    data: Vec<u8>,
}

pub struct MemoryTier {
    inner: Mutex<MemoryTierInner>,
}

struct MemoryTierInner {
    entries: HashMap<u64, CacheEntry>,
    lru_order: VecDeque<u64>,
    current_bytes: usize,
    capacity_bytes: usize,
}

impl MemoryTier {
    pub fn new(capacity_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(MemoryTierInner {
                entries: HashMap::new(),
                lru_order: VecDeque::new(),
                current_bytes: 0,
                capacity_bytes,
            }),
        }
    }

    pub fn get(&self, key: u64) -> Option<Vec<u8>> {
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            inner.lru_order.retain(|&k| k != key);
            inner.lru_order.push_back(key);
            Some(inner.entries[&key].data.clone())
        } else {
            None
        }
    }

    pub fn insert(&self, key: u64, data: Vec<u8>) {
        let mut inner = self.inner.lock().unwrap();
        let data_size = data.len();

        if inner.entries.contains_key(&key) {
            let old_size = inner.entries[&key].data.len();
            inner.current_bytes -= old_size;
            inner.lru_order.retain(|&k| k != key);
        }

        while inner.current_bytes + data_size > inner.capacity_bytes && !inner.lru_order.is_empty()
        {
            if let Some(evict_key) = inner.lru_order.pop_front() {
                if let Some(evicted) = inner.entries.remove(&evict_key) {
                    inner.current_bytes -= evicted.data.len();
                }
            }
        }

        inner.current_bytes += data_size;
        inner.entries.insert(key, CacheEntry { data });
        inner.lru_order.push_back(key);
    }

    pub fn contains(&self, key: u64) -> bool {
        self.inner.lock().unwrap().entries.contains_key(&key)
    }

    pub fn remove(&self, key: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.entries.remove(&key) {
            inner.current_bytes -= entry.data.len();
            inner.lru_order.retain(|&k| k != key);
            true
        } else {
            false
        }
    }

    pub fn touch(&self, key: u64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            inner.lru_order.retain(|&k| k != key);
            inner.lru_order.push_back(key);
            true
        } else {
            false
        }
    }

    pub fn clear(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let count = inner.entries.len();
        inner.entries.clear();
        inner.lru_order.clear();
        inner.current_bytes = 0;
        count
    }
}
