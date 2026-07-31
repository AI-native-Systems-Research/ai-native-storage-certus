//! Test seams: in-process mock implementations of the receptacle interfaces
//! (`IZyre`/`IZyreNode`, RDMA initiator/responder) that let a two-instance,
//! back-to-back harness exercise the full protocol over channels — no NIC
//! (research Decision 8).
//!
//! Compiled unconditionally and `pub` so both in-crate unit tests and the
//! `tests/mesh.rs` integration harness can construct a multi-node mesh.
//! (Mirrors the sibling RDMA crates, which likewise ship their mock transport in
//! the library.)
//!
//! # Structure
//!
//! The centre of gravity is `NodeWorld` — a cloneable, shared, scriptable
//! model of one node's local cache (memory-tier + block-device state) plus a
//! byte-backing pool that hands out real, stable pointers. Each mocked
//! interface (`MockDispatchMap`, `MockMemoryTier`, `MockDispatcher`,
//! `MockInitiator`, `MockResponder`/`MockResponderAdmin`) wraps a
//! `NodeWorld` clone and answers only the methods the protocol actually
//! exercises; every other trait method is `unimplemented!()`.
//!
//! Zyre itself is **not** mocked — the mesh harness drives the real `zyre`
//! component.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use component_core::channel::spsc::SpscChannel;

use interfaces::{
    CacheKey, ControlChannel, DispatchMapError, DispatcherConfig, DispatcherError, Endpoint,
    GpuStream, IDispatchMap, IDispatcher, IMemoryTier, IRemoteLookupRdmaInitiator,
    IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin, IpcHandle, LocalRegion,
    LookupResult, MemoryTierError, MemoryTierTelemetrySnapshot, PeerId, PushStatus, ReadWriteStats,
    RemoteLookupRdmaInitiatorError, RemoteLookupRdmaResponderError, RemoteRegion, ResponderCommand,
    ResponderEvent,
};

/// Default byte-backing pool size for a [`NodeWorld`] (64 MiB).
pub const DEFAULT_POOL_BYTES: usize = 64 * 1024 * 1024;

/// A fixed pool-wide `rkey` reported by the mock responder's region.
const MOCK_RKEY: u32 = 0x4242;

/// Where a mock cache entry currently lives.
enum MockLoc {
    /// Resident in the DRAM memory-tier at `ptr` (into the [`NodeWorld`] pool).
    Memory { ptr: *mut u8 },
    /// Committed to the (mock) block device at `offset`.
    Disk { offset: u64 },
}

impl MockLoc {
    /// Whether this entry is currently memory-resident.
    fn is_memory(&self) -> bool {
        matches!(self, MockLoc::Memory { .. })
    }
}

/// A single scripted cache entry.
struct MockEntry {
    location: MockLoc,
    size: u32,
    read_ref: u32,
    write_ref: u32,
}

/// Interior state of a [`NodeWorld`].
struct NodeWorldInner {
    /// Dispatch-map routing entries keyed by [`CacheKey`] — the node's
    /// published cache contents (what `IDispatchMap::lookup`/`entry_size`
    /// classify, what the initiator serves from, what `with_memory`/`with_disk`
    /// stage). Distinct from `reservations`.
    entries: HashMap<CacheKey, MockEntry>,
    /// Memory-tier landing-slot reservations keyed by [`CacheKey`] — what
    /// `IMemoryTier::insert`/`get`/`remove` track. On the requester these are
    /// private slots reserved for an in-flight RDMA fetch, published into
    /// `entries` (via `create_memory_tier_entry`) only on success. Modelled as a
    /// separate map because the real memory-tier and dispatch-map are separate
    /// components, so an `insert` reservation must not collide with a later
    /// publish of the same key.
    reservations: HashMap<CacheKey, (*mut u8, u32)>,
    /// Byte-backing pool so memory entries expose real, stable pointers.
    pool: Box<[u8]>,
    /// Bump cursor into `pool` (bytes consumed so far).
    cursor: usize,
    /// Forced per-key push results (overrides the natural outcome).
    push_outcomes: HashMap<CacheKey, PushStatus>,
    /// Keys whose dispatcher promote is scripted to leave them non-resident.
    promote_failures: HashSet<CacheKey>,
    /// Keys dropped (and reported not-found) the moment a push/serve is tried.
    evict_on_serve: HashSet<CacheKey>,
    /// This node's advertised RDMA endpoint.
    endpoint: Endpoint,
    /// Fixed pool-wide remote key.
    rkey: u32,
    /// Every key this node's initiator was asked to `push`, in order — lets
    /// tests assert single-flight (a key fetched exactly once).
    push_log: Vec<CacheKey>,
    /// Every endpoint this node's initiator was asked to warm via `connect`, in
    /// order — lets tests assert warm-at-discovery (connect-hardening).
    warm_log: Vec<String>,
    /// Artificial delay applied inside `push` before it returns, so a serve (and
    /// thus the RDMA_STATUS) can be held while other events are processed
    /// (research Decision 8 app-level delays; used by the single-flight test).
    serve_delay: Duration,
    /// Artificial delay applied inside `IDispatchMap::lookup`, delaying this
    /// node's KEY_RESPONSE (classification) — used to stage reply-ordering
    /// scenarios (research Decision 8 app-level delays; retry / canonical mesh).
    lookup_delay: Duration,
}

// SAFETY: The only non-`Send`/`Sync` state is the `*mut u8` stored in
// `MockLoc::Memory`, which always points into `pool` (a `Box<[u8]>` owned by
// this same struct, so it outlives every entry). All access is serialised by
// the `Mutex` wrapping this struct in `NodeWorld`. This mirrors the
// `unsafe impl Send/Sync for LookupResult` pattern in `idispatch_map.rs`.
unsafe impl Send for NodeWorldInner {}
// SAFETY: see the `Send` impl above.
unsafe impl Sync for NodeWorldInner {}

impl NodeWorldInner {
    /// Base pointer of the backing pool.
    fn pool_base(&self) -> *mut u8 {
        self.pool.as_ptr() as *mut u8
    }

    /// Bump-allocate `size` bytes from the pool (8-byte aligned) and return a
    /// stable pointer into it.
    fn alloc(&mut self, size: u32) -> *mut u8 {
        let aligned = (self.cursor + 7) & !7;
        let base = self.pool_base();
        // SAFETY: `aligned` stays within the pool as long as tests do not
        // over-subscribe it; the pointer is only ever read/written by the
        // harness within the region it staged.
        let ptr = unsafe { base.add(aligned) };
        self.cursor = aligned + size as usize;
        ptr
    }
}

/// A cloneable, shared handle to one node's scriptable local cache.
///
/// `NodeWorld` is `Arc<Mutex<..>>` under the hood: cloning yields another
/// handle onto the *same* state, so every mock built from a given world sees a
/// consistent view. Tests stage a scenario with the chainable builder methods
/// (e.g. hold `K1`,`K2` on disk and `K3` in memory, then evict `K3` before it
/// is served) and hand clones to the interface mocks.
#[derive(Clone)]
pub struct NodeWorld(Arc<Mutex<NodeWorldInner>>);

impl NodeWorld {
    /// Create a world with a `pool_bytes`-byte backing pool and a default
    /// endpoint (`127.0.0.1:0`).
    pub fn new(pool_bytes: usize) -> Self {
        let inner = NodeWorldInner {
            entries: HashMap::new(),
            reservations: HashMap::new(),
            pool: vec![0u8; pool_bytes].into_boxed_slice(),
            cursor: 0,
            push_outcomes: HashMap::new(),
            promote_failures: HashSet::new(),
            evict_on_serve: HashSet::new(),
            endpoint: Endpoint {
                ip: "127.0.0.1".to_string(),
                port: 0,
            },
            rkey: MOCK_RKEY,
            push_log: Vec::new(),
            warm_log: Vec::new(),
            serve_delay: Duration::ZERO,
            lookup_delay: Duration::ZERO,
        };
        Self(Arc::new(Mutex::new(inner)))
    }

    /// Create a world with the [`DEFAULT_POOL_BYTES`] backing pool.
    pub fn with_default_pool() -> Self {
        Self::new(DEFAULT_POOL_BYTES)
    }

    /// Lock the interior state.
    fn lock(&self) -> MutexGuard<'_, NodeWorldInner> {
        self.0.lock().expect("NodeWorld mutex poisoned")
    }

    /// Stage `key` as memory-resident with `size` bytes (allocates a real
    /// pointer from the pool). Chainable.
    pub fn with_memory(&self, key: CacheKey, size: u32) -> &Self {
        let mut inner = self.lock();
        let ptr = inner.alloc(size);
        inner.entries.insert(
            key,
            MockEntry {
                location: MockLoc::Memory { ptr },
                size,
                read_ref: 0,
                write_ref: 0,
            },
        );
        self
    }

    /// Stage `key` as block-device-resident with `size` bytes at `offset`.
    /// Chainable.
    pub fn with_disk(&self, key: CacheKey, size: u32, offset: u64) -> &Self {
        let mut inner = self.lock();
        inner.entries.insert(
            key,
            MockEntry {
                location: MockLoc::Disk { offset },
                size,
                read_ref: 0,
                write_ref: 0,
            },
        );
        self
    }

    /// Script `key` so that the next push/serve attempt drops it first and
    /// reports it not-found (models an eviction racing the serve). Chainable.
    pub fn evict_before_serve(&self, key: CacheKey) -> &Self {
        self.lock().evict_on_serve.insert(key);
        self
    }

    /// Force the initiator's push result for `key` to `status`, overriding the
    /// natural outcome. Chainable.
    pub fn force_push(&self, key: CacheKey, status: PushStatus) -> &Self {
        self.lock().push_outcomes.insert(key, status);
        self
    }

    /// Script `key` so a dispatcher promote leaves it non-resident (stays on
    /// disk). Chainable.
    pub fn fail_promote(&self, key: CacheKey) -> &Self {
        self.lock().promote_failures.insert(key);
        self
    }

    /// Set this node's advertised RDMA endpoint. Chainable.
    pub fn set_endpoint(&self, endpoint: Endpoint) -> &Self {
        self.lock().endpoint = endpoint;
        self
    }

    /// Delay this node's `push` (serve) by `d` before it returns, holding the
    /// RDMA_STATUS while the requester processes other events. Chainable.
    pub fn set_serve_delay(&self, d: Duration) -> &Self {
        self.lock().serve_delay = d;
        self
    }

    /// Delay this node's `IDispatchMap::lookup` by `d`, delaying its
    /// KEY_RESPONSE so reply ordering can be staged in tests. Chainable.
    pub fn set_reply_delay(&self, d: Duration) -> &Self {
        self.lock().lookup_delay = d;
        self
    }

    /// Whether `key` is currently present in the local cache (any tier).
    pub fn contains(&self, key: CacheKey) -> bool {
        self.lock().entries.contains_key(&key)
    }

    /// How many times this node's initiator was asked to `push` `key` (one per
    /// RDMA_REQUEST slot served) — used to assert single-flight.
    pub fn push_count(&self, key: CacheKey) -> usize {
        self.lock().push_log.iter().filter(|k| **k == key).count()
    }

    /// Endpoints this node's initiator was asked to warm via `connect`, in call
    /// order — used to assert warm-at-discovery (connect-hardening).
    pub fn warms(&self) -> Vec<String> {
        self.lock().warm_log.clone()
    }

    /// Whether a memory-tier landing-slot reservation for `key` is still held
    /// (i.e. not yet reclaimed) — used to assert teardown/reclaim behavior.
    pub fn has_reservation(&self, key: CacheKey) -> bool {
        self.lock().reservations.contains_key(&key)
    }

    /// Whether `key` is currently memory-resident.
    pub fn is_memory_resident(&self, key: CacheKey) -> bool {
        self.lock()
            .entries
            .get(&key)
            .map(|e| e.location.is_memory())
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// MockDispatchMap
// ---------------------------------------------------------------------------

/// Mock [`IDispatchMap`] backed by a [`NodeWorld`].
pub struct MockDispatchMap(NodeWorld);

impl MockDispatchMap {
    /// Wrap `world`.
    pub fn new(world: NodeWorld) -> Self {
        Self(world)
    }
}

impl IDispatchMap for MockDispatchMap {
    fn initialize(&self) -> Result<(), DispatchMapError> {
        Ok(())
    }

    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        // Optionally delay the classification/serve lookup without holding the
        // world lock, to stage KEY_RESPONSE ordering.
        let delay = self.0.lock().lookup_delay;
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        let mut inner = self.0.lock();
        match inner.entries.get_mut(&key) {
            None => Ok(LookupResult::NotExist),
            Some(entry) => {
                // A hit pins a read reference, mirroring the real dispatch-map;
                // callers balance it with `release_read`.
                entry.read_ref += 1;
                match entry.location {
                    MockLoc::Memory { ptr } => Ok(LookupResult::MemoryTier {
                        pointer: ptr,
                        size: entry.size,
                    }),
                    MockLoc::Disk { offset } => Ok(LookupResult::BlockDevice { offset }),
                }
            }
        }
    }

    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        self.0
            .lock()
            .entries
            .get(&key)
            .map(|e| e.size)
            .ok_or(DispatchMapError::KeyNotFound(key))
    }

    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.0.lock();
        match inner.entries.get_mut(&key) {
            Some(entry) => {
                entry.read_ref += 1;
                Ok(())
            }
            None => Err(DispatchMapError::KeyNotFound(key)),
        }
    }

    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.0.lock();
        match inner.entries.get_mut(&key) {
            Some(entry) if entry.read_ref == 0 => Err(DispatchMapError::RefCountUnderflow(key)),
            Some(entry) => {
                entry.read_ref -= 1;
                Ok(())
            }
            None => Err(DispatchMapError::KeyNotFound(key)),
        }
    }

    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.0.lock();
        match inner.entries.get_mut(&key) {
            Some(entry) => {
                entry.write_ref = 0;
                Ok(())
            }
            None => Err(DispatchMapError::KeyNotFound(key)),
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.0.lock();
        if inner.entries.remove(&key).is_some() {
            Ok(())
        } else {
            Err(DispatchMapError::KeyNotFound(key))
        }
    }

    fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut inner = self.0.lock();
        if inner.entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        inner.entries.insert(
            key,
            MockEntry {
                location: MockLoc::Memory { ptr: pointer },
                size,
                read_ref: 0,
                write_ref: 1,
            },
        );
        Ok(())
    }

    fn promote_block_to_memory_tier(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        let mut inner = self.0.lock();
        match inner.entries.get_mut(&key) {
            None => Err(DispatchMapError::KeyNotFound(key)),
            Some(entry) if !matches!(entry.location, MockLoc::Disk { .. }) => Err(
                DispatchMapError::InvalidState(format!("key {key} not in block-device state")),
            ),
            Some(entry) => {
                entry.location = MockLoc::Memory { ptr: pointer };
                entry.size = size;
                Ok(())
            }
        }
    }

    fn oldest_keys(&self, _n: usize) -> Vec<CacheKey> {
        // Not exercised by the remote-lookup protocol; harmless empty result.
        Vec::new()
    }

    fn take_write(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!("mock: IDispatchMap::take_write not needed by remote-lookup tests")
    }

    fn convert_to_storage(&self, _key: CacheKey, _offset: u64) -> Result<(), DispatchMapError> {
        unimplemented!("mock: IDispatchMap::convert_to_storage not needed by remote-lookup tests")
    }

    fn downgrade_reference(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!("mock: IDispatchMap::downgrade_reference not needed by remote-lookup tests")
    }

    fn convert_memory_tier_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!(
            "mock: IDispatchMap::convert_memory_tier_to_block not needed by remote-lookup tests"
        )
    }

    fn touch(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!("mock: IDispatchMap::touch not needed by remote-lookup tests")
    }

    fn is_evictable(&self, _key: CacheKey) -> bool {
        unimplemented!("mock: IDispatchMap::is_evictable not needed by remote-lookup tests")
    }

    fn recover_extent(
        &self,
        _key: CacheKey,
        _offset: u64,
        _size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
        unimplemented!("mock: IDispatchMap::recover_extent not needed by remote-lookup tests")
    }

    fn try_evict_to_block(&self, _key: CacheKey) -> Result<(), DispatchMapError> {
        unimplemented!("mock: IDispatchMap::try_evict_to_block not needed by remote-lookup tests")
    }
}

// ---------------------------------------------------------------------------
// MockMemoryTier
// ---------------------------------------------------------------------------

/// Mock [`IMemoryTier`] backed by a [`NodeWorld`].
pub struct MockMemoryTier(NodeWorld);

impl MockMemoryTier {
    /// Wrap `world`.
    pub fn new(world: NodeWorld) -> Self {
        Self(world)
    }
}

impl IMemoryTier for MockMemoryTier {
    fn initialize(
        &self,
        _pool_size: usize,
        _numa_node: Option<i32>,
    ) -> Result<(), MemoryTierError> {
        Ok(())
    }

    fn insert(&self, key: CacheKey, size: u32) -> Result<*mut u8, MemoryTierError> {
        let mut inner = self.0.lock();
        if inner.reservations.contains_key(&key) {
            return Err(MemoryTierError::AlreadyExists(key));
        }
        let ptr = inner.alloc(size);
        inner.reservations.insert(key, (ptr, size));
        Ok(ptr)
    }

    fn get(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        self.peek(key)
    }

    fn peek(&self, key: CacheKey) -> Option<(*mut u8, u32)> {
        let inner = self.0.lock();
        // A landing-slot reservation (`insert`, requester side) takes precedence.
        if let Some(&slot) = inner.reservations.get(&key) {
            return Some(slot);
        }
        // Otherwise fall back to memory-resident source data staged via
        // `with_memory` (server side): the *real* RDMA initiator reads the bytes
        // it one-sided-writes through `IMemoryTier::get`/`peek`, so a
        // memory-resident entry must expose its real pool pointer here. (The mock
        // initiator reads `entries` directly, so mock-path tests never exercised
        // this; the hardware loopback test — T034 — does.)
        match inner.entries.get(&key).map(|e| (&e.location, e.size)) {
            Some((MockLoc::Memory { ptr }, size)) => Some((*ptr, size)),
            _ => None,
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), MemoryTierError> {
        let mut inner = self.0.lock();
        if inner.reservations.remove(&key).is_some() {
            Ok(())
        } else {
            Err(MemoryTierError::KeyNotFound(key))
        }
    }

    fn contains(&self, key: CacheKey) -> bool {
        self.0.lock().reservations.contains_key(&key)
    }

    fn capacity(&self) -> usize {
        self.0.lock().pool.len()
    }

    fn used(&self) -> usize {
        self.0.lock().cursor
    }

    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        let inner = self.0.lock();
        Some((inner.pool_base(), inner.pool.len()))
    }

    fn is_dma_capable(&self) -> bool {
        true
    }

    fn telemetry_snapshot(&self) -> MemoryTierTelemetrySnapshot {
        MemoryTierTelemetrySnapshot::default()
    }

    fn evict_next(&self) -> Option<CacheKey> {
        unimplemented!("mock: IMemoryTier::evict_next not needed by remote-lookup tests")
    }

    fn evict_next_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
        unimplemented!("mock: IMemoryTier::evict_next_for_key not needed by remote-lookup tests")
    }

    fn oldest_keys(&self, _n: usize) -> Vec<CacheKey> {
        unimplemented!("mock: IMemoryTier::oldest_keys not needed by remote-lookup tests")
    }

    fn touch(&self, _key: CacheKey) {
        unimplemented!("mock: IMemoryTier::touch not needed by remote-lookup tests")
    }

    fn batch_touch(&self, _keys: &[CacheKey]) {
        unimplemented!("mock: IMemoryTier::batch_touch not needed by remote-lookup tests")
    }

    fn clear(&self) -> Result<usize, MemoryTierError> {
        unimplemented!("mock: IMemoryTier::clear not needed by remote-lookup tests")
    }
}

// ---------------------------------------------------------------------------
// MockDispatcher
// ---------------------------------------------------------------------------

/// Mock [`IDispatcher`] backed by a [`NodeWorld`].
pub struct MockDispatcher(NodeWorld);

impl MockDispatcher {
    /// Wrap `world`.
    pub fn new(world: NodeWorld) -> Self {
        Self(world)
    }
}

impl IDispatcher for MockDispatcher {
    fn initialize(&self, _config: DispatcherConfig) -> Result<(), DispatcherError> {
        Ok(())
    }

    fn promote_to_memory_tier(&self, keys: &[CacheKey]) {
        let mut inner = self.0.lock();
        for &key in keys {
            if inner.promote_failures.contains(&key) {
                continue;
            }
            let is_disk = matches!(
                inner.entries.get(&key).map(|e| &e.location),
                Some(MockLoc::Disk { .. })
            );
            if is_disk {
                let size = inner.entries.get(&key).map(|e| e.size).unwrap_or(0);
                let ptr = inner.alloc(size);
                if let Some(entry) = inner.entries.get_mut(&key) {
                    entry.location = MockLoc::Memory { ptr };
                }
            }
        }
    }

    fn shutdown(&self) -> Result<(), DispatcherError> {
        unimplemented!("mock: IDispatcher::shutdown not needed by remote-lookup tests")
    }

    fn lookup(&self, _key: CacheKey, _ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        unimplemented!("mock: IDispatcher::lookup not needed by remote-lookup tests")
    }

    fn lookup_async(
        &self,
        _key: CacheKey,
        _ipc_handle: IpcHandle,
    ) -> Result<GpuStream, DispatcherError> {
        unimplemented!("mock: IDispatcher::lookup_async not needed by remote-lookup tests")
    }

    fn batch_lookup(
        &self,
        _entries: &[(CacheKey, Vec<IpcHandle>)],
    ) -> Vec<Result<(), DispatcherError>> {
        unimplemented!("mock: IDispatcher::batch_lookup not needed by remote-lookup tests")
    }

    fn check(&self, _key: CacheKey) -> Result<bool, DispatcherError> {
        unimplemented!("mock: IDispatcher::check not needed by remote-lookup tests")
    }

    fn remove(&self, _key: CacheKey) -> Result<(), DispatcherError> {
        unimplemented!("mock: IDispatcher::remove not needed by remote-lookup tests")
    }

    fn populate(&self, _key: CacheKey, _ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        unimplemented!("mock: IDispatcher::populate not needed by remote-lookup tests")
    }

    fn reserve_memory(
        &self,
        _key: CacheKey,
        _size: u32,
        _session_id: u64,
    ) -> Result<*mut u8, DispatcherError> {
        unimplemented!("mock: IDispatcher::reserve_memory not needed by remote-lookup tests")
    }

    fn copy_gpu_to_memory_async(
        &self,
        _key: CacheKey,
        _regions: &[IpcHandle],
        _stream: GpuStream,
    ) -> Result<(), DispatcherError> {
        unimplemented!(
            "mock: IDispatcher::copy_gpu_to_memory_async not needed by remote-lookup tests"
        )
    }

    fn copy_gpu_to_memory_completed(
        &self,
        _key: CacheKey,
        _size: u32,
    ) -> Result<(), DispatcherError> {
        unimplemented!(
            "mock: IDispatcher::copy_gpu_to_memory_completed not needed by remote-lookup tests"
        )
    }

    fn release_memory(&self, _key: CacheKey) -> Result<(), DispatcherError> {
        unimplemented!("mock: IDispatcher::release_memory not needed by remote-lookup tests")
    }

    fn pin(&self, _key: CacheKey) -> Result<(), DispatcherError> {
        unimplemented!("mock: IDispatcher::pin not needed by remote-lookup tests")
    }

    fn unpin(&self, _key: CacheKey) -> Result<(), DispatcherError> {
        unimplemented!("mock: IDispatcher::unpin not needed by remote-lookup tests")
    }

    fn touch(&self, _key: CacheKey) -> Result<(), DispatcherError> {
        unimplemented!("mock: IDispatcher::touch not needed by remote-lookup tests")
    }

    fn clear_memory_tier(&self) -> Result<usize, DispatcherError> {
        unimplemented!("mock: IDispatcher::clear_memory_tier not needed by remote-lookup tests")
    }

    fn flush_to_ssd(&self) -> Result<usize, DispatcherError> {
        unimplemented!("mock: IDispatcher::flush_to_ssd not needed by remote-lookup tests")
    }

    fn read_write_stats(&self) -> ReadWriteStats {
        // Telemetry is not exercised by remote-lookup tests; return zeroed
        // counters, matching the disabled-telemetry contract on the real device.
        ReadWriteStats::default()
    }
}

// ---------------------------------------------------------------------------
// MockInitiator
// ---------------------------------------------------------------------------

/// Mock [`IRemoteLookupRdmaInitiator`] backed by a [`NodeWorld`].
///
/// Records `disconnect`/`disconnect_all` calls and the local [`PeerId`] so
/// tests can assert on the teardown handshake.
pub struct MockInitiator {
    world: NodeWorld,
    disconnects: Mutex<Vec<String>>,
    local_peer: Mutex<Option<PeerId>>,
}

impl MockInitiator {
    /// Wrap `world`.
    pub fn new(world: NodeWorld) -> Self {
        Self {
            world,
            disconnects: Mutex::new(Vec::new()),
            local_peer: Mutex::new(None),
        }
    }

    /// Snapshot of recorded disconnect targets. `disconnect_all` records the
    /// sentinel `"<all>"`.
    pub fn disconnects(&self) -> Vec<String> {
        self.disconnects
            .lock()
            .expect("disconnects poisoned")
            .clone()
    }

    /// The last local [`PeerId`] supplied via
    /// [`set_local_peer_id`](IRemoteLookupRdmaInitiator::set_local_peer_id).
    pub fn local_peer_id(&self) -> Option<PeerId> {
        self.local_peer.lock().expect("local_peer poisoned").clone()
    }
}

impl IRemoteLookupRdmaInitiator for MockInitiator {
    fn push(
        &self,
        _endpoint: &str,
        items: &[(CacheKey, RemoteRegion)],
    ) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError> {
        // Optionally hold the serve (and thus the RDMA_STATUS) without keeping
        // the world locked, so the requester can process other events meanwhile.
        let delay = self.world.lock().serve_delay;
        if !delay.is_zero() {
            thread::sleep(delay);
        }

        let mut inner = self.world.lock();
        let mut out = Vec::with_capacity(items.len());
        for (key, region) in items {
            inner.push_log.push(*key);
            let status = if let Some(forced) = inner.push_outcomes.get(key).copied() {
                forced
            } else if inner.evict_on_serve.contains(key) {
                inner.entries.remove(key);
                PushStatus::KeyNotFound
            } else {
                match inner.entries.get(key) {
                    Some(entry) if entry.location.is_memory() && entry.size == region.length => {
                        PushStatus::Success
                    }
                    Some(entry) if entry.size != region.length => PushStatus::SizeMismatch,
                    _ => PushStatus::KeyNotFound,
                }
            };
            out.push(status);
        }
        Ok(out)
    }

    fn connect(&self, endpoint: &str) -> Result<(), RemoteLookupRdmaInitiatorError> {
        self.world.lock().warm_log.push(endpoint.to_string());
        Ok(())
    }

    fn disconnect(&self, endpoint: &str) {
        self.disconnects
            .lock()
            .expect("disconnects poisoned")
            .push(endpoint.to_string());
    }

    fn disconnect_all(&self) {
        self.disconnects
            .lock()
            .expect("disconnects poisoned")
            .push("<all>".to_string());
    }

    fn set_local_peer_id(&self, peer: PeerId) {
        *self.local_peer.lock().expect("local_peer poisoned") = Some(peer);
    }
}

// ---------------------------------------------------------------------------
// MockResponder / MockResponderAdmin
// ---------------------------------------------------------------------------

/// Lifecycle/configuration state shared between the responder and its admin.
#[derive(Default)]
struct ResponderInner {
    initialized: bool,
    stopped: bool,
    bind_ip: Option<String>,
    actor_cpu: Option<usize>,
}

/// State shared by [`MockResponder`] and [`MockResponderAdmin`].
struct ResponderShared {
    world: NodeWorld,
    inner: Mutex<ResponderInner>,
}

impl ResponderShared {
    fn lock(&self) -> MutexGuard<'_, ResponderInner> {
        self.inner.lock().expect("responder state poisoned")
    }
}

/// Mock runtime control surface [`IRemoteLookupRdmaResponder`].
pub struct MockResponder(Arc<ResponderShared>);

/// Mock lifecycle/configuration surface [`IRemoteLookupRdmaResponderAdmin`].
pub struct MockResponderAdmin(Arc<ResponderShared>);

impl MockResponder {
    /// Build a responder + admin pair sharing state, backed by `world`.
    pub fn new(world: NodeWorld) -> (MockResponder, MockResponderAdmin) {
        let shared = Arc::new(ResponderShared {
            world,
            inner: Mutex::new(ResponderInner::default()),
        });
        (
            MockResponder(Arc::clone(&shared)),
            MockResponderAdmin(shared),
        )
    }
}

impl IRemoteLookupRdmaResponder for MockResponder {
    fn open_control_channel(&self) -> Result<ControlChannel, RemoteLookupRdmaResponderError> {
        if !self.0.lock().initialized {
            return Err(RemoteLookupRdmaResponderError::NotInitialized(
                "open_control_channel before initialize".into(),
            ));
        }

        // command channel: remote-lookup -> responder actor
        let cmd_ch = SpscChannel::<ResponderCommand>::new(64);
        let command_tx = cmd_ch.sender().expect("fresh command sender");
        let command_rx = cmd_ch.receiver().expect("fresh command receiver");

        // event channel: responder actor -> remote-lookup
        let evt_ch = SpscChannel::<ResponderEvent>::new(64);
        let event_tx = evt_ch.sender().expect("fresh event sender");
        let event_rx = evt_ch.receiver().expect("fresh event receiver");

        // Small responder "actor": answer each Disconnect with a DisconnectAck.
        // Exits when remote-lookup drops `command_tx` (recv -> Closed).
        thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    ResponderCommand::Disconnect { node } => {
                        // If the event receiver is gone, stop.
                        if event_tx
                            .send(ResponderEvent::DisconnectAck { node })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });

        Ok(ControlChannel {
            command_tx,
            event_rx,
        })
    }

    fn local_endpoint(&self) -> Result<Endpoint, RemoteLookupRdmaResponderError> {
        if !self.0.lock().initialized {
            return Err(RemoteLookupRdmaResponderError::NotInitialized(
                "local_endpoint before initialize".into(),
            ));
        }
        Ok(self.0.world.lock().endpoint.clone())
    }

    fn local_region(&self) -> Result<LocalRegion, RemoteLookupRdmaResponderError> {
        if !self.0.lock().initialized {
            return Err(RemoteLookupRdmaResponderError::NotInitialized(
                "local_region before initialize".into(),
            ));
        }
        let world = self.0.world.lock();
        Ok(LocalRegion {
            addr: world.pool_base() as u64,
            rkey: world.rkey,
            length: world.pool.len(),
        })
    }
}

impl MockResponderAdmin {
    /// Whether [`initialize`](IRemoteLookupRdmaResponderAdmin::initialize) has run.
    pub fn is_initialized(&self) -> bool {
        self.0.lock().initialized
    }

    /// Whether a stop/shutdown has been signalled.
    pub fn is_stopped(&self) -> bool {
        self.0.lock().stopped
    }

    /// The bind IP recorded via
    /// [`set_bind_ip`](IRemoteLookupRdmaResponderAdmin::set_bind_ip).
    pub fn bind_ip(&self) -> Option<String> {
        self.0.lock().bind_ip.clone()
    }

    /// The actor CPU recorded via
    /// [`set_actor_cpu`](IRemoteLookupRdmaResponderAdmin::set_actor_cpu).
    pub fn actor_cpu(&self) -> Option<usize> {
        self.0.lock().actor_cpu
    }
}

impl IRemoteLookupRdmaResponderAdmin for MockResponderAdmin {
    fn set_actor_cpu(&self, cpu: usize) {
        self.0.lock().actor_cpu = Some(cpu);
    }

    fn set_bind_ip(&self, ip: String) {
        self.0.lock().bind_ip = Some(ip);
    }

    fn initialize(&self) -> Result<(), RemoteLookupRdmaResponderError> {
        self.0.lock().initialized = true;
        Ok(())
    }

    fn signal_stop(&self) {
        self.0.lock().stopped = true;
    }

    fn shutdown(&self) -> Result<(), RemoteLookupRdmaResponderError> {
        self.0.lock().stopped = true;
        Ok(())
    }
}
