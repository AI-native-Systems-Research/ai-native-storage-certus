//! Per-op translation: decode a wire request blob → `IDispatcher` call(s) →
//! encode a response blob.
//!
//! This is the shm-queue counterpart of `apps/certus-server/src/service.rs`.
//! The CUDA-IPC open/close cache, the multi-region handle resolution, the
//! `pending_stores` reservation bookkeeping, and the `check_duplicate_keys`
//! guard are **lifted from that file** (marked below). The only differences:
//!   * input is the compact little-endian wire framing (`wire.rs`), not protobuf;
//!   * output is a response blob, not a tonic `Response`;
//!   * there is no tokio — the caller runs each op on a blocking worker thread,
//!     so the logic here is plain synchronous code;
//!   * `PendingStoreEntry` additionally records `reserved_at` so a reaper thread
//!     can reclaim Reserve-without-Commit leaks.
//!
//! TODO(unify): once the prototype proves out, factor the shared translation
//! (`ipc_cache_open`/`close`, handle-table → `Vec<IpcHandle>`, the per-op
//! dispatcher calls) into a transport-agnostic module used by both servers.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpu_services::cuda_ffi;
use interfaces::{GpuStream, IDispatcher, IpcHandle};

use crate::wire::{self, op, Reader, Writer};

/// Eviction-reason encoding in the TakeEvents response (mirrors `ring.py`).
const REASON_DEMOTED: u32 = 0;
const REASON_REMOVED: u32 = 1;

/// Optional per-op metrics hook for a transport host.
///
/// The plain `certus-server` binary passes no observer (zero overhead); the
/// YAML-composed server implements this to keep its Prometheus/OTel counters
/// (`certus_populates_total`, `certus_lookup_hits_total`, …) that the former
/// gRPC service maintained. All methods default to no-ops, so a host overrides
/// only the ops it cares about.
pub trait TranslatorObserver: Send + Sync {
    /// A Populate op finalized `succeeded` new cache entries.
    fn on_populate(&self, _succeeded: u64) {}
    /// A Lookup op resolved `hits` entries (moving `gpu_bytes` total to the GPU)
    /// and `misses` entries that were not present.
    fn on_lookup(&self, _hits: u64, _misses: u64, _gpu_bytes: u64) {}
    /// A TakeEvents op drained `count` eviction events.
    fn on_evictions(&self, _count: u64) {}
}

// ---- CUDA-IPC open/close cache (lifted from service.rs:35-154) -------------

struct IpcCacheEntry {
    dev_ptr: *mut std::ffi::c_void,
    #[allow(dead_code)]
    gpu_device_id: i32,
    refcount: usize,
}

// SAFETY: dev_ptr is a CUDA device pointer only used from blocking worker
// threads; the map itself is guarded by a Mutex.
unsafe impl Send for IpcCacheEntry {}
unsafe impl Sync for IpcCacheEntry {}

type IpcCache = Arc<Mutex<HashMap<[u8; 64], IpcCacheEntry>>>;

fn ipc_cache_open(
    cache: &IpcCache,
    handle_bytes: &[u8; 64],
    gpu_device_id: i32,
) -> Result<*mut std::ffi::c_void, String> {
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = map.get_mut(handle_bytes) {
        entry.refcount += 1;
        return Ok(entry.dev_ptr);
    }

    // Set the correct GPU device before opening the IPC handle.
    if gpu_device_id >= 0 {
        let err = unsafe { cuda_ffi::cudaSetDevice(gpu_device_id) };
        if err != cuda_ffi::CUDA_SUCCESS {
            return Err(format!(
                "cudaSetDevice({}) failed: {}",
                gpu_device_id,
                cuda_ffi::cuda_error_string(err)
            ));
        }
    }

    let cuda_handle = cuda_ffi::cudaIpcMemHandle_t {
        reserved: *handle_bytes,
    };
    let mut dev_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let err = unsafe {
        cuda_ffi::cudaIpcOpenMemHandle(
            &mut dev_ptr,
            cuda_handle,
            cuda_ffi::CUDA_IPC_MEM_LAZY_ENABLE_PEER_ACCESS,
        )
    };
    if err != cuda_ffi::CUDA_SUCCESS {
        return Err(format!(
            "cudaIpcOpenMemHandle failed: {}",
            cuda_ffi::cuda_error_string(err)
        ));
    }
    if dev_ptr.is_null() {
        return Err("cudaIpcOpenMemHandle returned null".to_string());
    }

    map.insert(
        *handle_bytes,
        IpcCacheEntry {
            dev_ptr,
            gpu_device_id,
            refcount: 1,
        },
    );
    Ok(dev_ptr)
}

fn ipc_cache_close(cache: &IpcCache, handle_bytes: &[u8; 64]) {
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = map.get_mut(handle_bytes) {
        entry.refcount -= 1;
        if entry.refcount == 0 {
            unsafe {
                cuda_ffi::cudaIpcCloseMemHandle(entry.dev_ptr);
            }
            map.remove(handle_bytes);
        }
    }
}

// ---- Pending reservations (lifted from service.rs:48-52, + reserved_at) ----

struct PendingStoreEntry {
    size: u32,
    reserved_at: Instant,
}

type PendingStores = Arc<Mutex<HashMap<u64, PendingStoreEntry>>>;

// ---- Op error (wire decode failure or a dispatch-level guard) --------------

/// An op that could not be decoded or dispatched; surfaced to the client as a
/// `STATUS_ERROR` response whose payload is this message as UTF-8.
#[derive(Debug)]
pub enum OpError {
    Wire(wire::WireError),
    Msg(String),
}

impl From<wire::WireError> for OpError {
    fn from(e: wire::WireError) -> Self {
        OpError::Wire(e)
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpError::Wire(e) => write!(f, "{e}"),
            OpError::Msg(m) => write!(f, "{m}"),
        }
    }
}

/// Lifted from service.rs:156-167 (returns a plain message, not a tonic Status).
fn check_duplicate_keys(keys: &[u64]) -> Result<(), OpError> {
    let mut seen = HashSet::with_capacity(keys.len());
    for &key in keys {
        if !seen.insert(key) {
            return Err(OpError::Msg(format!("duplicate key in batch: {key}")));
        }
    }
    Ok(())
}

/// One distinct CUDA IPC handle in the table: `(handle_bytes, gpu_device_id)`.
type TableHandle = ([u8; 64], i32);
/// One per-block region reference: `(handle_idx, offset, size)`.
type RegionRef = (u32, u64, u32);
/// One batch entry: `(key, regions)`.
type BatchEntry = (u64, Vec<RegionRef>);

/// A decoded handle table + per-entry region references (CopyToStore / Lookup).
struct HandleBatch {
    /// Distinct CUDA IPC handles, sent once.
    handles: Vec<TableHandle>,
    entries: Vec<BatchEntry>,
}

fn decode_handle_batch(r: &mut Reader) -> Result<HandleBatch, OpError> {
    let n_handles = r.u32()? as usize;
    let mut handles = Vec::with_capacity(n_handles);
    for _ in 0..n_handles {
        let h = r.handle()?;
        let dev = r.i32()?;
        handles.push((h, dev));
    }
    let n_entries = r.u32()? as usize;
    let mut entries = Vec::with_capacity(n_entries);
    for _ in 0..n_entries {
        let key = r.u64()?;
        let nreg = r.u16()? as usize;
        let mut regions = Vec::with_capacity(nreg);
        for _ in 0..nreg {
            let idx = r.u32()?;
            let offset = r.u64()?;
            let size = r.u32()?;
            if (idx as usize) >= n_handles {
                return Err(OpError::Msg(format!(
                    "region handle_idx {idx} out of range (n_handles={n_handles})"
                )));
            }
            regions.push((idx, offset, size));
        }
        entries.push((key, regions));
    }
    Ok(HandleBatch { handles, entries })
}

/// Shared, server-global translation state. Cloneable (all fields are `Arc`),
/// so each worker thread holds its own handle; `ipc_cache`/`pending_stores` are
/// shared under mutexes exactly as in the gRPC server — a Reserve on one channel
/// and a Commit on another must see the same map.
#[derive(Clone)]
pub struct Translator {
    dispatcher: Arc<dyn IDispatcher + Send + Sync>,
    ipc_cache: IpcCache,
    pending_stores: PendingStores,
    eviction_rx: crossbeam_channel::Receiver<dispatcher::EvictionEvent>,
    eviction_dropped: Arc<AtomicU64>,
    observer: Option<Arc<dyn TranslatorObserver>>,
}

impl Translator {
    pub fn new(
        dispatcher: Arc<dyn IDispatcher + Send + Sync>,
        eviction_rx: crossbeam_channel::Receiver<dispatcher::EvictionEvent>,
        eviction_dropped: Arc<AtomicU64>,
    ) -> Self {
        Self {
            dispatcher,
            ipc_cache: Arc::new(Mutex::new(HashMap::new())),
            pending_stores: Arc::new(Mutex::new(HashMap::new())),
            eviction_rx,
            eviction_dropped,
            observer: None,
        }
    }

    /// Attach a metrics observer. Used by the YAML server to preserve its
    /// per-op Prometheus counters; the plain server leaves it unset.
    pub fn with_observer(mut self, observer: Arc<dyn TranslatorObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Decode `payload` for `opcode`, dispatch, and return the response blob.
    /// On `Err`, the caller replies with `STATUS_ERROR` + the message bytes.
    pub fn dispatch(&self, opcode: u32, payload: &[u8]) -> Result<Vec<u8>, OpError> {
        let mut r = Reader::new(payload);
        match opcode {
            op::CHECK => self.op_check(&mut r),
            op::TOUCH => self.op_touch(&mut r),
            op::RESERVE => self.op_reserve(&mut r),
            op::COPY_TO_STORE => self.op_copy_to_store(&mut r),
            op::COMMIT_STORE => self.op_commit_store(&mut r),
            op::ABORT_STORE => self.op_abort_store(&mut r),
            op::PIN => self.op_pin(&mut r),
            op::UNPIN => self.op_unpin(&mut r),
            op::LOOKUP => self.op_lookup(&mut r),
            op::TAKE_EVENTS => self.op_take_events(&mut r),
            op::POPULATE => self.op_populate(&mut r),
            op::REMOVE => self.op_remove(&mut r),
            op::CLEAR_MEMORY_TIER => self.op_clear_memory_tier(&mut r),
            op::FLUSH_TO_SSD => self.op_flush_to_ssd(&mut r),
            op::GET_IO_STATS => self.op_get_io_stats(&mut r),
            op::CHECK_AND_PIN => self.op_check_and_pin(&mut r),
            other => Err(OpError::Msg(format!("unknown opcode {other}"))),
        }
    }

    /// Reclaim reservations that were never committed/aborted within `timeout`.
    /// Called periodically by the reaper thread. Returns the number reclaimed.
    pub fn reap_stale_reservations(&self, timeout: Duration) -> usize {
        let now = Instant::now();
        let stale: Vec<u64> = {
            let map = self
                .pending_stores
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            map.iter()
                .filter(|(_, e)| now.duration_since(e.reserved_at) >= timeout)
                .map(|(&k, _)| k)
                .collect()
        };
        for key in &stale {
            // release_memory then drop the pending record. Order matches abort.
            let _ = self.dispatcher.release_memory(*key);
            self.pending_stores
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(key);
        }
        stale.len()
    }

    // ---- key-only batch helper ----

    /// Decode `{ n:u32, [key:u64]*n }`.
    fn read_keys(r: &mut Reader) -> Result<Vec<u64>, OpError> {
        let n = r.u32()? as usize;
        let mut keys = Vec::with_capacity(n);
        for _ in 0..n {
            keys.push(r.u64()?);
        }
        Ok(keys)
    }

    fn op_check(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        use wire::check_state::{MISS, PENDING, RESIDENT};
        let keys = Self::read_keys(r)?;
        check_duplicate_keys(&keys)?;

        // Snapshot the reserved-but-uncommitted set once (one lock, not one per
        // key). A key here has a store in flight — Reserve was seen, Commit/Abort
        // is not — so it is coming but not yet loadable. Answering PENDING from
        // this map, *before* consulting the dispatcher, is deliberate on two
        // counts:
        //   * `dispatcher.check()` -> `dispatch_map.lookup()` blocks waiting for
        //     an active writer, which would stall this Check until the store
        //     commits; surfacing PENDING lets the caller defer instead of block.
        //   * keys are content-addressed and the client only reserves keys that
        //     Check reported absent, so a pending key is being written for the
        //     first time and is not already resident — no HIT is being masked.
        // Abort and the stale-reservation reaper both drop the pending record (and
        // release the slot), so a dropped store re-checks as MISS, never a
        // permanent PENDING.
        let pending: HashSet<u64> = {
            let map = self
                .pending_stores
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            keys.iter()
                .copied()
                .filter(|k| map.contains_key(k))
                .collect()
        };

        let mut w = Writer::with_capacity(keys.len());
        for &key in &keys {
            let state = if pending.contains(&key) {
                PENDING
            } else if self.dispatcher.check(key).unwrap_or_default() {
                RESIDENT
            } else {
                MISS
            };
            w.u8(state);
        }
        Ok(w.into_bytes())
    }

    fn op_touch(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let promote = r.u8()? != 0;
        let keys = Self::read_keys(r)?;
        check_duplicate_keys(&keys)?;
        let mut w = Writer::with_capacity(keys.len());
        for &key in &keys {
            w.u8(self.dispatcher.touch(key).is_ok() as u8);
        }
        if promote {
            self.dispatcher.promote_to_memory_tier(&keys);
        }
        Ok(w.into_bytes())
    }

    fn op_reserve(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let n = r.u32()? as usize;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let key = r.u64()?;
            let size = r.u32()?;
            let session = r.u64()?;
            entries.push((key, size, session));
        }
        let keys: Vec<u64> = entries.iter().map(|(k, _, _)| *k).collect();
        check_duplicate_keys(&keys)?;

        let mut w = Writer::with_capacity(n);
        for (key, size, session) in entries {
            match self.dispatcher.reserve_memory(key, size, session) {
                Ok(_ptr) => {
                    self.pending_stores
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(
                            key,
                            PendingStoreEntry {
                                size,
                                reserved_at: Instant::now(),
                            },
                        );
                    w.u8(1);
                }
                Err(_) => w.u8(0),
            }
        }
        Ok(w.into_bytes())
    }

    fn op_commit_store(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let keys = Self::read_keys(r)?;
        check_duplicate_keys(&keys)?;
        let mut w = Writer::with_capacity(keys.len());
        for &key in &keys {
            let size = {
                let map = self
                    .pending_stores
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                map.get(&key).map(|e| e.size)
            };
            let ok = match size {
                None => false,
                Some(size) => match self.dispatcher.copy_gpu_to_memory_completed(key, size) {
                    Ok(()) => {
                        self.pending_stores
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .remove(&key);
                        true
                    }
                    Err(_) => false,
                },
            };
            w.u8(ok as u8);
        }
        Ok(w.into_bytes())
    }

    fn op_abort_store(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let keys = Self::read_keys(r)?;
        check_duplicate_keys(&keys)?;
        let mut w = Writer::with_capacity(keys.len());
        for &key in &keys {
            self.pending_stores
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&key);
            w.u8(self.dispatcher.release_memory(key).is_ok() as u8);
        }
        Ok(w.into_bytes())
    }

    fn op_pin(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let promote = r.u8()? != 0;
        let keys = Self::read_keys(r)?;
        check_duplicate_keys(&keys)?;
        let mut w = Writer::with_capacity(keys.len());
        for &key in &keys {
            w.u8(self.dispatcher.pin(key).is_ok() as u8);
        }
        if promote {
            self.dispatcher.promote_to_memory_tier(&keys);
        }
        Ok(w.into_bytes())
    }

    fn op_check_and_pin(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let keys = Self::read_keys(r)?;
        check_duplicate_keys(&keys)?;
        let mut w = Writer::with_capacity(keys.len());
        for &key in &keys {
            let ok = self.dispatcher.check_and_pin(key).unwrap_or(false);
            w.u8(ok as u8);
        }
        Ok(w.into_bytes())
    }

    fn op_unpin(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let keys = Self::read_keys(r)?;
        check_duplicate_keys(&keys)?;
        let mut w = Writer::with_capacity(keys.len());
        for &key in &keys {
            w.u8(self.dispatcher.unpin(key).is_ok() as u8);
        }
        Ok(w.into_bytes())
    }

    /// Open every distinct handle in the table once (refcounted global cache).
    /// Returns per-index resolved base pointers (`None` if that handle failed to
    /// open) plus the list of handles actually opened, for later close.
    fn open_handle_table(
        &self,
        handles: &[([u8; 64], i32)],
    ) -> (Vec<Option<*mut std::ffi::c_void>>, Vec<[u8; 64]>) {
        let mut resolved: Vec<Option<*mut std::ffi::c_void>> = Vec::with_capacity(handles.len());
        let mut opened_keys: Vec<[u8; 64]> = Vec::new();
        for (h, dev) in handles {
            match ipc_cache_open(&self.ipc_cache, h, *dev) {
                Ok(ptr) => {
                    resolved.push(Some(ptr));
                    opened_keys.push(*h);
                }
                Err(_) => resolved.push(None),
            }
        }
        (resolved, opened_keys)
    }

    /// Build the `Vec<IpcHandle>` for one entry, folding each region's offset
    /// into its resolved allocation base. Returns `None` if any referenced
    /// handle failed to open (the whole entry is then marked failed).
    fn regions_of(
        entry_regions: &[(u32, u64, u32)],
        resolved: &[Option<*mut std::ffi::c_void>],
    ) -> Option<Vec<IpcHandle>> {
        let mut regions = Vec::with_capacity(entry_regions.len());
        for &(idx, offset, size) in entry_regions {
            let base = (*resolved.get(idx as usize)?)?;
            regions.push(IpcHandle {
                address: (base as usize + offset as usize) as *mut u8,
                size,
            });
        }
        Some(regions)
    }

    fn op_copy_to_store(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let batch = decode_handle_batch(r)?;
        let keys: Vec<u64> = batch.entries.iter().map(|(k, _)| *k).collect();
        check_duplicate_keys(&keys)?;

        let (resolved, opened_keys) = self.open_handle_table(&batch.handles);

        let mut w = Writer::with_capacity(batch.entries.len());
        for (key, entry_regions) in &batch.entries {
            let ok = match Self::regions_of(entry_regions, &resolved) {
                None => false,
                Some(regions) => self
                    .dispatcher
                    .copy_gpu_to_memory_async(*key, &regions, GpuStream(std::ptr::null_mut()))
                    .is_ok(),
            };
            w.u8(ok as u8);
        }

        for key in &opened_keys {
            ipc_cache_close(&self.ipc_cache, key);
        }
        Ok(w.into_bytes())
    }

    fn op_lookup(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let batch = decode_handle_batch(r)?;
        let keys: Vec<u64> = batch.entries.iter().map(|(k, _)| *k).collect();
        check_duplicate_keys(&keys)?;

        let (resolved, opened_keys) = self.open_handle_table(&batch.handles);

        // Resolve regions per entry; entries whose handles failed to open are
        // held back from the batch and reported as misses (ok=0).
        let mut ok_flags = vec![0u8; batch.entries.len()];
        let mut valid_indices = Vec::with_capacity(batch.entries.len());
        let mut valid_batch: Vec<(u64, Vec<IpcHandle>)> = Vec::with_capacity(batch.entries.len());
        for (i, (key, entry_regions)) in batch.entries.iter().enumerate() {
            if let Some(regions) = Self::regions_of(entry_regions, &resolved) {
                valid_indices.push(i);
                valid_batch.push((*key, regions));
            }
        }

        let results = self.dispatcher.batch_lookup(&valid_batch);
        let mut hits = 0u64;
        let mut misses = 0u64;
        let mut gpu_bytes = 0u64;
        for ((slot, res), (_, regions)) in valid_indices
            .iter()
            .zip(results.into_iter())
            .zip(valid_batch.iter())
        {
            match res {
                Ok(()) => {
                    ok_flags[*slot] = 1;
                    hits += 1;
                    // Sum across all per-layer regions (N==1 for coalesced blocks).
                    gpu_bytes += regions.iter().map(|h| h.size as u64).sum::<u64>();
                }
                // Only KeyNotFound counts as a miss; other errors (e.g. transient
                // I/O) mirror the gRPC service, which excludes them from misses.
                Err(interfaces::DispatcherError::KeyNotFound(_)) => misses += 1,
                Err(_) => {}
            }
        }

        for key in &opened_keys {
            ipc_cache_close(&self.ipc_cache, key);
        }

        if let Some(obs) = &self.observer {
            obs.on_lookup(hits, misses, gpu_bytes);
        }

        let mut w = Writer::with_capacity(ok_flags.len());
        for f in ok_flags {
            w.u8(f);
        }
        Ok(w.into_bytes())
    }

    fn op_take_events(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let max = r.u32()? as usize;
        let mut events: Vec<(u64, u32)> = Vec::new();
        while let Ok(ev) = self.eviction_rx.try_recv() {
            let reason = match ev.reason {
                dispatcher::EvictionReason::Demoted => REASON_DEMOTED,
                dispatcher::EvictionReason::Removed => REASON_REMOVED,
            };
            events.push((ev.key, reason));
            if max > 0 && events.len() >= max {
                break;
            }
        }
        let dropped = self.eviction_dropped.swap(0, Ordering::Relaxed);

        if let Some(obs) = &self.observer {
            obs.on_evictions(events.len() as u64);
        }

        let mut w = Writer::with_capacity(4 + events.len() * 12 + 8);
        w.u32(events.len() as u32);
        for (key, reason) in events {
            w.u64(key);
            w.u32(reason);
        }
        w.u64(dropped);
        Ok(w.into_bytes())
    }

    /// Populate cache entries by DMA-copying from GPU. Wire request is a
    /// `HandleBatch` (same framing as CopyToStore), but every entry must carry
    /// exactly one region (`nreg == 1`): `populate` takes a single IPC handle.
    /// resp `{ [ok:u8]*n }`. Mirrors service.rs:232 (BatchPopulate).
    fn op_populate(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let batch = decode_handle_batch(r)?;
        let keys: Vec<u64> = batch.entries.iter().map(|(k, _)| *k).collect();
        check_duplicate_keys(&keys)?;

        let (resolved, opened_keys) = self.open_handle_table(&batch.handles);

        // Build a flat batch of (key, IpcHandle) for batch_populate. Entries
        // with != 1 region are rejected upfront (populate is single-handle).
        let mut batch_entries: Vec<(u64, IpcHandle)> = Vec::with_capacity(batch.entries.len());
        let mut rejected: Vec<usize> = Vec::new();
        for (i, (key, entry_regions)) in batch.entries.iter().enumerate() {
            if entry_regions.len() != 1 {
                rejected.push(i);
                continue;
            }
            match Self::regions_of(entry_regions, &resolved) {
                None => { rejected.push(i); }
                Some(regions) => { batch_entries.push((*key, regions[0])); }
            }
        }

        // Single batched call: all D2H copies issued async, ONE sync, then register all.
        let batch_results = self.dispatcher.batch_populate(&batch_entries);

        // Map results back to the original entry order.
        let mut succeeded = 0u64;
        let mut w = Writer::with_capacity(batch.entries.len());
        let mut batch_idx = 0usize;
        for i in 0..batch.entries.len() {
            let ok = if rejected.contains(&i) {
                false
            } else {
                let result = batch_results.get(batch_idx).map(|r| r.is_ok()).unwrap_or(false);
                batch_idx += 1;
                result
            };
            succeeded += ok as u64;
            w.u8(ok as u8);
        }

        for key in &opened_keys {
            ipc_cache_close(&self.ipc_cache, key);
        }
        if let Some(obs) = &self.observer {
            obs.on_populate(succeeded);
        }
        Ok(w.into_bytes())
    }

    /// Remove entries entirely. req `{ n:u32, [key:u64]*n }`, resp `{ [ok:u8]*n }`.
    /// Mirrors service.rs:513 (BatchRemove).
    fn op_remove(&self, r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let keys = Self::read_keys(r)?;
        check_duplicate_keys(&keys)?;
        let mut w = Writer::with_capacity(keys.len());
        for &key in &keys {
            w.u8(self.dispatcher.remove(key).is_ok() as u8);
        }
        Ok(w.into_bytes())
    }

    /// Evict the whole memory-tier. Empty request; resp `{ entries_cleared:u64 }`.
    /// Mirrors service.rs:874 (ClearMemoryTier).
    fn op_clear_memory_tier(&self, _r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let cleared = self
            .dispatcher
            .clear_memory_tier()
            .map_err(|e| OpError::Msg(format!("clear_memory_tier failed: {e}")))?;
        let mut w = Writer::with_capacity(8);
        w.u64(cleared as u64);
        Ok(w.into_bytes())
    }

    /// Drain pending write-through jobs. Empty request; resp `{ jobs_flushed:u64 }`.
    /// Mirrors service.rs:894 (FlushToSsd).
    fn op_flush_to_ssd(&self, _r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let flushed = self
            .dispatcher
            .flush_to_ssd()
            .map_err(|e| OpError::Msg(format!("flush_to_ssd failed: {e}")))?;
        let mut w = Writer::with_capacity(8);
        w.u64(flushed as u64);
        Ok(w.into_bytes())
    }

    /// Cumulative SSD read/write counters. Empty request; resp is 6×u64 in the
    /// order `read_ops, read_bytes, read_latency_ns_sum, write_ops, write_bytes,
    /// write_latency_ns_sum` (matches the gRPC IoStatsResponse — no histogram
    /// buckets). Mirrors service.rs:1010 (GetIoStats).
    fn op_get_io_stats(&self, _r: &mut Reader) -> Result<Vec<u8>, OpError> {
        let s = self.dispatcher.read_write_stats();
        let mut w = Writer::with_capacity(48);
        w.u64(s.read_ops);
        w.u64(s.read_bytes);
        w.u64(s.read_latency_ns_sum);
        w.u64(s.write_ops);
        w.u64(s.write_bytes);
        w.u64(s.write_latency_ns_sum);
        Ok(w.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interfaces::{
        CacheKey, DispatcherConfig, DispatcherError, GpuStream, IDispatcher, IpcHandle,
        ReadWriteStats, TierEventStats,
    };
    use std::sync::atomic::AtomicU64;

    /// Minimal in-memory dispatcher: a key becomes "resident" only after a
    /// completed store (`copy_gpu_to_memory_completed`) or an explicit
    /// `populate`; `reserve_memory` succeeds unless the key is in `reserve_fail`.
    /// Enough to drive `op_check`'s tri-state across the Reserve→Commit and
    /// Reserve→Abort lifecycles through the public `dispatch()` surface.
    #[derive(Default)]
    struct MockDispatcher {
        resident: Mutex<HashSet<u64>>,
        reserve_fail: Mutex<HashSet<u64>>,
    }

    impl IDispatcher for MockDispatcher {
        fn initialize(&self, _config: DispatcherConfig) -> Result<(), DispatcherError> {
            Ok(())
        }
        fn shutdown(&self) -> Result<(), DispatcherError> {
            Ok(())
        }
        fn lookup(&self, _key: CacheKey, _h: IpcHandle) -> Result<(), DispatcherError> {
            Ok(())
        }
        fn lookup_async(
            &self,
            _key: CacheKey,
            _h: IpcHandle,
        ) -> Result<GpuStream, DispatcherError> {
            Ok(GpuStream(std::ptr::null_mut()))
        }
        fn batch_lookup(
            &self,
            entries: &[(CacheKey, Vec<IpcHandle>)],
        ) -> Vec<Result<(), DispatcherError>> {
            entries.iter().map(|_| Ok(())).collect()
        }
        fn check(&self, key: CacheKey) -> Result<bool, DispatcherError> {
            Ok(self.resident.lock().unwrap().contains(&key))
        }
        fn remove(&self, key: CacheKey) -> Result<(), DispatcherError> {
            self.resident.lock().unwrap().remove(&key);
            Ok(())
        }
        fn populate(&self, key: CacheKey, _h: IpcHandle) -> Result<(), DispatcherError> {
            self.resident.lock().unwrap().insert(key);
            Ok(())
        }
        fn batch_populate(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>> {
            entries.iter().map(|(k, h)| self.populate(*k, h.clone())).collect()
        }
        fn reserve_memory(
            &self,
            key: CacheKey,
            _size: u32,
            _session_id: u64,
        ) -> Result<*mut u8, DispatcherError> {
            if self.reserve_fail.lock().unwrap().contains(&key) {
                Err(DispatcherError::AllocationFailed("test".into()))
            } else {
                Ok(std::ptr::NonNull::<u8>::dangling().as_ptr())
            }
        }
        fn copy_gpu_to_memory_async(
            &self,
            _key: CacheKey,
            _regions: &[IpcHandle],
            _stream: GpuStream,
        ) -> Result<(), DispatcherError> {
            Ok(())
        }
        fn copy_gpu_to_memory_completed(
            &self,
            key: CacheKey,
            _size: u32,
        ) -> Result<(), DispatcherError> {
            self.resident.lock().unwrap().insert(key);
            Ok(())
        }
        fn release_memory(&self, _key: CacheKey) -> Result<(), DispatcherError> {
            Ok(())
        }
        fn pin(&self, _key: CacheKey) -> Result<(), DispatcherError> {
            Ok(())
        }
        fn check_and_pin(&self, key: CacheKey) -> Result<bool, DispatcherError> {
            Ok(self.resident.lock().unwrap().contains(&key))
        }
        fn unpin(&self, _key: CacheKey) -> Result<(), DispatcherError> {
            Ok(())
        }
        fn touch(&self, _key: CacheKey) -> Result<(), DispatcherError> {
            Ok(())
        }
        fn promote_to_memory_tier(&self, _keys: &[CacheKey]) {}
        fn clear_memory_tier(&self) -> Result<usize, DispatcherError> {
            Ok(0)
        }
        fn flush_to_ssd(&self) -> Result<usize, DispatcherError> {
            Ok(0)
        }
        fn read_write_stats(&self) -> ReadWriteStats {
            ReadWriteStats::default()
        }

        fn tier_event_stats(&self) -> TierEventStats {
            TierEventStats::default()
        }
    }

    fn translator(disp: Arc<MockDispatcher>) -> Translator {
        // Eviction channel is unused by op_check; keep the sender alive so the
        // receiver does not report "disconnected".
        let (_tx, rx) = crossbeam_channel::unbounded::<dispatcher::EvictionEvent>();
        std::mem::forget(_tx);
        Translator::new(disp, rx, Arc::new(AtomicU64::new(0)))
    }

    fn enc_keys(keys: &[u64]) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(keys.len() as u32);
        for &k in keys {
            w.u64(k);
        }
        w.into_bytes()
    }

    fn enc_reserve(entries: &[(u64, u32, u64)]) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(entries.len() as u32);
        for &(k, size, session) in entries {
            w.u64(k);
            w.u32(size);
            w.u64(session);
        }
        w.into_bytes()
    }

    #[test]
    fn op_check_reports_tristate_across_store_lifecycle() {
        use wire::check_state::{MISS, PENDING, RESIDENT};
        let disp = Arc::new(MockDispatcher::default());
        let tr = translator(disp);

        // Unknown key -> MISS.
        assert_eq!(tr.dispatch(op::CHECK, &enc_keys(&[7])).unwrap(), vec![MISS]);

        // Reserve populates pending_stores -> Check reports PENDING (without
        // blocking on the dispatcher).
        assert_eq!(
            tr.dispatch(op::RESERVE, &enc_reserve(&[(7, 4096, 0)]))
                .unwrap(),
            vec![1]
        );
        assert_eq!(
            tr.dispatch(op::CHECK, &enc_keys(&[7])).unwrap(),
            vec![PENDING]
        );

        // Commit clears pending and makes the key resident -> RESIDENT.
        assert_eq!(
            tr.dispatch(op::COMMIT_STORE, &enc_keys(&[7])).unwrap(),
            vec![1]
        );
        assert_eq!(
            tr.dispatch(op::CHECK, &enc_keys(&[7])).unwrap(),
            vec![RESIDENT]
        );
    }

    #[test]
    fn op_check_abort_returns_to_miss() {
        use wire::check_state::{MISS, PENDING};
        let disp = Arc::new(MockDispatcher::default());
        let tr = translator(disp);

        tr.dispatch(op::RESERVE, &enc_reserve(&[(9, 4096, 0)]))
            .unwrap();
        assert_eq!(
            tr.dispatch(op::CHECK, &enc_keys(&[9])).unwrap(),
            vec![PENDING]
        );

        // Abort drops the pending record; the key never became resident.
        tr.dispatch(op::ABORT_STORE, &enc_keys(&[9])).unwrap();
        assert_eq!(tr.dispatch(op::CHECK, &enc_keys(&[9])).unwrap(), vec![MISS]);
    }

    #[test]
    fn op_check_mixed_batch_states() {
        use wire::check_state::{MISS, PENDING, RESIDENT};
        let disp = Arc::new(MockDispatcher::default());
        let tr = translator(disp.clone());

        // key 1 resident, key 2 pending, key 3 absent.
        disp.resident.lock().unwrap().insert(1);
        tr.dispatch(op::RESERVE, &enc_reserve(&[(2, 4096, 0)]))
            .unwrap();

        assert_eq!(
            tr.dispatch(op::CHECK, &enc_keys(&[1, 2, 3])).unwrap(),
            vec![RESIDENT, PENDING, MISS]
        );
    }

    #[test]
    fn op_check_stale_reservation_reaped_to_miss() {
        use wire::check_state::{MISS, PENDING};
        let disp = Arc::new(MockDispatcher::default());
        let tr = translator(disp);

        tr.dispatch(op::RESERVE, &enc_reserve(&[(5, 4096, 0)]))
            .unwrap();
        assert_eq!(
            tr.dispatch(op::CHECK, &enc_keys(&[5])).unwrap(),
            vec![PENDING]
        );

        // The reaper reclaims a never-committed reservation; re-check is MISS,
        // never a permanent PENDING.
        assert_eq!(tr.reap_stale_reservations(Duration::from_secs(0)), 1);
        assert_eq!(tr.dispatch(op::CHECK, &enc_keys(&[5])).unwrap(), vec![MISS]);
    }
}
