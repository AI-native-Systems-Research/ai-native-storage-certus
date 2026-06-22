use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};
use interfaces::{
    CacheKey, DispatchMapError, DmaAllocFn, DmaBuffer, IDispatchMap, IMemoryTier, LookupResult,
    MemoryTierError,
};

// ===========================================================================
// Bench mock: EvictorBenchMap — all entries in BlockDevice state
// ===========================================================================

struct EvictorBenchMap {
    inner: Mutex<HashMap<CacheKey, u64>>,
}

impl EvictorBenchMap {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn seed(&self, n: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.clear();
        inner.reserve(n as usize);
        for key in 0..n {
            inner.insert(key, key * 4096);
        }
    }

    fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

impl IDispatchMap for EvictorBenchMap {
    fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn create_staging(
        &self,
        _key: CacheKey,
        _size: u32,
    ) -> Result<Arc<DmaBuffer>, DispatchMapError> {
        unimplemented!("not used in evictor benchmark")
    }

    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let inner = self.inner.lock().unwrap();
        match inner.get(&key) {
            Some(&offset) => Ok(LookupResult::BlockDevice { offset }),
            None => Ok(LookupResult::NotExist),
        }
    }

    fn convert_to_storage(&self, _key: CacheKey, _offset: u64) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn take_read(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn take_write(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn release_read(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn release_write(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn downgrade_reference(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.remove(&key).is_some() {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn touch(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn entry_size(&self, _key: CacheKey) -> Result<u32, DispatchMapError> {
        Ok(4096)
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let inner = self.inner.lock().unwrap();
        inner.keys().copied().take(n).collect()
    }

    fn create_memory_tier_entry(
        &self,
        _key: CacheKey,
        _pointer: *mut u8,
        _size: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn convert_memory_tier_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn is_evictable(&self, _key: CacheKey) -> bool {
        false
    }

    fn recover_extent(
        &self,
        _key: CacheKey,
        _offset: u64,
        _size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
        Ok(())
    }
}

// ===========================================================================
// Bench mock: NoopMemoryTier
// ===========================================================================

struct NoopMemoryTier;

impl IMemoryTier for NoopMemoryTier {
    fn initialize(&self, _pool_size: usize, _numa_node: Option<i32>) -> Result<(), MemoryTierError> {
        Ok(())
    }

    fn insert(&self, _key: CacheKey, _size: u32) -> Result<*mut u8, MemoryTierError> {
        Err(MemoryTierError::PoolFull)
    }

    fn get(&self, _key: CacheKey) -> Option<(*mut u8, u32)> {
        None
    }

    fn evict_lru(&self) -> Option<CacheKey> {
        None
    }

    fn evict_lru_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
        None
    }

    fn remove(&self, _key: CacheKey) -> Result<(), MemoryTierError> {
        Err(MemoryTierError::KeyNotFound(0))
    }

    fn touch(&self, _key: CacheKey) {}

    fn contains(&self, _key: CacheKey) -> bool {
        false
    }

    fn capacity(&self) -> usize {
        0
    }

    fn used(&self) -> usize {
        0
    }

    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        None
    }

    fn peek(&self, _key: CacheKey) -> Option<(*mut u8, u32)> {
        None
    }

    fn oldest_keys(&self, _n: usize) -> Vec<CacheKey> {
        Vec::new()
    }

    fn clear(&self) -> Result<usize, MemoryTierError> {
        Ok(0)
    }
}

// ===========================================================================
// Eviction hot-path (inlined from BackgroundEvictor::evictor_loop)
// ===========================================================================

#[inline(always)]
fn evict_entry(
    dm: &Arc<dyn IDispatchMap + Send + Sync>,
    mt: &Arc<dyn IMemoryTier + Send + Sync>,
    key: CacheKey,
) -> bool {
    match dm.lookup(key) {
        Ok(LookupResult::BlockDevice { .. }) => {
            let _ = dm.release_read(key);
            let _ = mt.remove(key);
            dm.remove(key).is_ok()
        }
        Ok(_) => {
            let _ = dm.release_read(key);
            false
        }
        Err(_) => false,
    }
}

// ===========================================================================
// Benchmarks
// ===========================================================================

fn bench_evict_single_entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("evict_single_entry");
    group.throughput(Throughput::Elements(1));

    group.bench_function("latency", |b| {
        let dm = Arc::new(EvictorBenchMap::new());
        let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;
        let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(NoopMemoryTier);

        let pool_size: u64 = 10_000;
        dm.seed(pool_size);
        let mut next_key: u64 = 0;

        b.iter(|| {
            if dm.len() == 0 {
                dm.seed(pool_size);
                next_key = 0;
            }
            let key = next_key;
            next_key += 1;
            black_box(evict_entry(&dm_iface, &mt, black_box(key)));
        });
    });

    group.finish();
}

fn bench_evict_sweep(c: &mut Criterion) {
    let mut group = c.benchmark_group("evict_sweep");

    let batch_sizes: &[u64] = &[64, 256, 1024, 4096];

    for &batch_size in batch_sizes {
        group.throughput(Throughput::Elements(batch_size));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &bs| {
                let dm = Arc::new(EvictorBenchMap::new());
                let dm_iface: Arc<dyn IDispatchMap + Send + Sync> = Arc::clone(&dm) as _;
                let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(NoopMemoryTier);

                b.iter_custom(|iters| {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        dm.seed(bs);

                        let start = std::time::Instant::now();
                        let candidates = dm_iface.oldest_keys(bs as usize);
                        for key in candidates {
                            evict_entry(&dm_iface, &mt, key);
                        }
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_evict_single_entry, bench_evict_sweep);
criterion_main!(benches);
