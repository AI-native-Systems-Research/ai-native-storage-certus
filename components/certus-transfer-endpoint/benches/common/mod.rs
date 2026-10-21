//! What both benchmarks share: endpoints built and driven exactly as a caller
//! would, through `ITransferEndpointAdmin` and `ITransferEndpoint` only, over
//! the build's own transport — the in-process mock without `rdma`, verbs with
//! it. Nothing here reaches inside the crate.
//!
//! Over verbs a benchmark needs `CERTUS_RDMA_TEST_IP`, a local RoCE address;
//! without it the verbs cases are skipped, said so on stderr, and the mock is
//! not substituted (Principle XI: an `rdma` build never falls back).

#![allow(dead_code)] // each bench uses part of this

use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use certus_transfer_endpoint::CertusTransferEndpoint;
use interfaces::{
    CacheKey, DispatchMapError, EndpointConfig, EntryOutcome, IDispatchMap, IMemoryTier,
    ITransferEndpoint, ITransferEndpointAdmin, LandingRegionHandle, LookupResult, MemoryTierError,
    MemoryTierTelemetrySnapshot, RemoteRegion,
};

/// Where an endpoint listens: the RoCE address with `rdma`, a loopback-shaped
/// name over the mock (which never resolves it). `None` if an `rdma` build has
/// no address to use.
pub fn host() -> Option<String> {
    if cfg!(feature = "rdma") {
        match std::env::var("CERTUS_RDMA_TEST_IP") {
            Ok(ip) => Some(ip),
            Err(_) => {
                eprintln!(
                    "skipping: an `rdma` build benchmarks over verbs and needs \
                     CERTUS_RDMA_TEST_IP (it never falls back to the mock)"
                );
                None
            }
        }
    } else {
        Some("10.9.9.9".to_string())
    }
}

/// The transport the numbers describe, for the benchmark's name.
pub fn transport() -> &'static str {
    if cfg!(feature = "rdma") {
        "verbs"
    } else {
        "mock"
    }
}

fn next_addr(host: &str) -> String {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let port =
        44000 + (std::process::id() % 400) as u16 * 16 + NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{host}:{port}")
}

/// A dispatch map answering from a fixed table, pinning as the real one does:
/// every `MemoryTier` answer takes a read reference, released by `release_read`.
/// Kept minimal so the benchmark measures the endpoint, not a map.
#[derive(Default)]
pub struct TableMap {
    entries: Mutex<HashMap<CacheKey, (u64, u32)>>,
    read_ref: Mutex<HashMap<CacheKey, i64>>,
}

impl TableMap {
    pub fn insert(&self, key: CacheKey, addr: u64, size: u32) {
        self.entries.lock().unwrap().insert(key, (addr, size));
    }

    /// Pins held now, across all keys.
    pub fn held(&self) -> i64 {
        self.read_ref.lock().unwrap().values().sum()
    }
}

impl IDispatchMap for TableMap {
    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let Some(&(addr, size)) = self.entries.lock().unwrap().get(&key) else {
            return Ok(LookupResult::NotExist);
        };
        *self.read_ref.lock().unwrap().entry(key).or_default() += 1;
        Ok(LookupResult::MemoryTier {
            pointer: addr as *mut u8,
            size,
        })
    }
    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        *self.read_ref.lock().unwrap().entry(key).or_default() -= 1;
        Ok(())
    }
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }
    fn convert_to_storage(&self, _: CacheKey, _: u64) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn take_read(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn take_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn release_write(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn downgrade_reference(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn remove(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn touch(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn entry_size(&self, _: CacheKey) -> Result<u32, DispatchMapError> {
        unimplemented!()
    }
    fn oldest_keys(&self, _: usize) -> Vec<CacheKey> {
        unimplemented!()
    }
    fn create_memory_tier_entry(
        &self,
        _: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn convert_memory_tier_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn promote_block_to_memory_tier(
        &self,
        _: CacheKey,
        _: *mut u8,
        _: u32,
    ) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn is_evictable(&self, _: CacheKey) -> bool {
        unimplemented!()
    }
    fn try_evict_to_block(&self, _: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
    fn recover_extent(&self, _: CacheKey, _: u64, _: u32) -> Result<(), DispatchMapError> {
        unimplemented!()
    }
}

/// A memory tier that only reports its pool.
pub struct PoolOnly {
    base: *mut u8,
    len: usize,
}

// SAFETY: only `pool_info` is called, which copies the two fields out; the
// pointer is never dereferenced through this type.
unsafe impl Send for PoolOnly {}
// SAFETY: as above.
unsafe impl Sync for PoolOnly {}

impl IMemoryTier for PoolOnly {
    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        Some((self.base, self.len))
    }
    fn capacity(&self) -> usize {
        self.len
    }
    fn initialize(&self, _: usize, _: Option<i32>) -> Result<(), MemoryTierError> {
        unimplemented!()
    }
    fn insert(&self, _: CacheKey, _: u32) -> Result<*mut u8, MemoryTierError> {
        unimplemented!()
    }
    fn get(&self, _: CacheKey) -> Option<(*mut u8, u32)> {
        unimplemented!()
    }
    fn peek(&self, _: CacheKey) -> Option<(*mut u8, u32)> {
        unimplemented!()
    }
    fn evict_next(&self) -> Option<CacheKey> {
        unimplemented!()
    }
    fn evict_next_for_key(&self, _: CacheKey) -> Option<CacheKey> {
        unimplemented!()
    }
    fn oldest_keys(&self, _: usize) -> Vec<CacheKey> {
        unimplemented!()
    }
    fn remove(&self, _: CacheKey) -> Result<(), MemoryTierError> {
        unimplemented!()
    }
    fn touch(&self, _: CacheKey) {
        unimplemented!()
    }
    fn batch_touch(&self, _: &[CacheKey]) {
        unimplemented!()
    }
    fn contains(&self, _: CacheKey) -> bool {
        unimplemented!()
    }
    fn used(&self) -> usize {
        unimplemented!()
    }
    fn is_dma_capable(&self) -> bool {
        unimplemented!()
    }
    fn clear(&self) -> Result<usize, MemoryTierError> {
        unimplemented!()
    }
    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        unimplemented!()
    }
    fn set_bounce_reserve(&self, _: usize) -> Result<(), MemoryTierError> {
        unimplemented!()
    }
    fn bounce_alloc(&self, _: u32) -> Result<interfaces::BounceLease, interfaces::BounceError> {
        unimplemented!()
    }
    fn bounce_free(&self, _: interfaces::BounceLease) {
        unimplemented!()
    }
    fn bounce_usage(&self) -> (usize, usize) {
        unimplemented!()
    }
}

/// One endpoint over the build's transport, serving `pool` through `map`.
pub struct Node {
    pub ep: Arc<CertusTransferEndpoint>,
    pub map: Arc<TableMap>,
    pub pool: Box<[u8]>,
    pub addr: String,
}

impl Node {
    pub fn new(host: &str, pool_len: usize) -> Node {
        let mut pool = vec![0u8; pool_len].into_boxed_slice();
        for (i, b) in pool.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let ep = CertusTransferEndpoint::create();
        let map = Arc::new(TableMap::default());
        ep.dispatch_map
            .connect(Arc::clone(&map) as Arc<dyn IDispatchMap + Send + Sync>)
            .unwrap();
        ep.memory_tier
            .connect(Arc::new(PoolOnly {
                base: pool.as_mut_ptr(),
                len: pool.len(),
            }) as Arc<dyn IMemoryTier + Send + Sync>)
            .unwrap();
        let addr = next_addr(host);
        let mut config = EndpointConfig::new(&addr);
        // CERTUS_BENCH_SEND_CREDITS overrides the default, for exploring it.
        if let Some(n) = std::env::var("CERTUS_BENCH_SEND_CREDITS")
            .ok()
            .and_then(|s| s.parse().ok())
        {
            config.send_credits = n;
        }
        ep.configure(config).unwrap();
        ep.initialize().expect("initialize");
        Node {
            ep,
            map,
            pool,
            addr,
        }
    }

    /// Make `count` entries of `size` bytes resident, keys `0..count`, laid end
    /// to end in the pool.
    pub fn hold(&self, count: usize, size: usize) {
        assert!(count * size <= self.pool.len(), "the pool holds them");
        for k in 0..count {
            self.map.insert(
                k as CacheKey,
                self.pool.as_ptr() as u64 + (k * size) as u64,
                size as u32,
            );
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        let _ = self.ep.shutdown();
    }
}

/// Landing slots for `count` entries of `size` bytes, registered with `ep`.
pub struct Slots {
    pub mem: Box<[u8]>,
    pub handle: LandingRegionHandle,
}

impl Slots {
    pub fn new(ep: &CertusTransferEndpoint, count: usize, size: usize) -> Slots {
        let mut mem = vec![0u8; count * size].into_boxed_slice();
        let handle = ep
            .register_landing_region(mem.as_mut_ptr(), mem.len())
            .unwrap();
        Slots { mem, handle }
    }

    /// Key `k` into slot `k`, for keys `0..count`.
    pub fn items(&self, count: usize, size: usize) -> Vec<(CacheKey, RemoteRegion)> {
        (0..count)
            .map(|k| {
                (
                    k as CacheKey,
                    self.handle.region_at(k * size, size as u32).unwrap(),
                )
            })
            .collect()
    }
}

/// Fetch `items` from `peer` and wait; panics on anything but a reply.
pub fn fetch(
    dest: &CertusTransferEndpoint,
    peer: &str,
    items: &[(CacheKey, RemoteRegion)],
) -> Vec<EntryOutcome> {
    let (tx, rx) = mpsc::channel();
    dest.fetch(peer, items, Box::new(move |r| tx.send(r).unwrap()))
        .unwrap();
    rx.recv_timeout(Duration::from_secs(60))
        .expect("fetch completed")
        .expect("fetch answered")
}
