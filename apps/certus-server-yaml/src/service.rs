//! gRPC service implementation for the Certus Dispatcher.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tonic::{Request, Response, Status};

use gpu_services::cuda_ffi;
use interfaces::{DispatcherError, GpuStream, IDispatcher, IpcHandle};

pub mod proto {
    tonic::include_proto!("certus.dispatcher.v1");
}

use proto::dispatcher_server::{Dispatcher, DispatcherServer};
use proto::{
    BatchAbortStoreRequest, BatchAbortStoreResponse, BatchCheckRequest, BatchCheckResponse,
    BatchCommitStoreRequest, BatchCommitStoreResponse, BatchCopyToStoreRequest,
    BatchCopyToStoreResponse, BatchLookupRequest, BatchLookupResponse, BatchPopulateRequest,
    BatchPopulateResponse, BatchRemoveRequest, BatchRemoveResponse, BatchReserveRequest,
    BatchReserveResponse, BatchTouchRequest, BatchTouchResponse, CheckResult,
    BatchPinRequest, BatchPinResponse, BatchUnpinRequest, BatchUnpinResponse,
    ClearMemoryTierRequest, ClearMemoryTierResponse, EntryResult, ErrorCode, FlushToSsdRequest,
    FlushToSsdResponse, GetIoStatsRequest, IoStatsResponse, TakeEventsRequest, TakeEventsResponse,
};

#[cfg(feature = "p2p-native")]
use dispatcher_p2p::{EvictionEvent, EvictionReason};
#[cfg(not(feature = "p2p-native"))]
use dispatcher::{EvictionEvent, EvictionReason};

pub fn dispatcher_server(svc: DispatcherService) -> DispatcherServer<DispatcherService> {
    DispatcherServer::new(svc)
}

struct IpcCacheEntry {
    dev_ptr: *mut std::ffi::c_void,
    #[allow(dead_code)]
    gpu_device_id: i32,
    refcount: usize,
}

// SAFETY: dev_ptr is a CUDA device pointer only used from blocking threads.
unsafe impl Send for IpcCacheEntry {}
unsafe impl Sync for IpcCacheEntry {}

type IpcCache = Arc<Mutex<HashMap<[u8; 64], IpcCacheEntry>>>;

struct PendingStoreEntry {
    size: u32,
}

type PendingStores = Arc<Mutex<HashMap<u64, PendingStoreEntry>>>;

/// Cumulative counters exposed to the metrics/telemetry layer.
/// All fields are monotonic; take deltas across two reads to compute rates.
#[derive(Clone)]
pub struct ServiceCounters {
    pub populates: Arc<AtomicU64>,
    pub lookup_hits: Arc<AtomicU64>,
    pub lookup_misses: Arc<AtomicU64>,
    pub evictions: Arc<AtomicU64>,
    pub gpu_bytes_transferred: Arc<AtomicU64>,
}

impl ServiceCounters {
    pub fn new() -> Self {
        Self {
            populates: Arc::new(AtomicU64::new(0)),
            lookup_hits: Arc::new(AtomicU64::new(0)),
            lookup_misses: Arc::new(AtomicU64::new(0)),
            evictions: Arc::new(AtomicU64::new(0)),
            gpu_bytes_transferred: Arc::new(AtomicU64::new(0)),
        }
    }
}

pub struct DispatcherService {
    dispatcher: Arc<dyn IDispatcher + Send + Sync>,
    ipc_cache: IpcCache,
    pending_stores: PendingStores,
    eviction_rx: crossbeam_channel::Receiver<EvictionEvent>,
    eviction_dropped: Arc<AtomicU64>,
    counters: ServiceCounters,
}

impl DispatcherService {
    pub fn new(
        dispatcher: Arc<dyn IDispatcher + Send + Sync>,
        eviction_rx: crossbeam_channel::Receiver<EvictionEvent>,
        eviction_dropped: Arc<AtomicU64>,
        counters: ServiceCounters,
    ) -> Self {
        Self {
            dispatcher,
            ipc_cache: Arc::new(Mutex::new(HashMap::new())),
            pending_stores: Arc::new(Mutex::new(HashMap::new())),
            eviction_rx,
            eviction_dropped,
            counters,
        }
    }
}

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

    map.insert(*handle_bytes, IpcCacheEntry {
        dev_ptr,
        gpu_device_id,
        refcount: 1,
    });
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

#[allow(clippy::result_large_err)]
fn check_duplicate_keys(keys: &[u64]) -> Result<(), Status> {
    let mut seen = HashSet::with_capacity(keys.len());
    for &key in keys {
        if !seen.insert(key) {
            return Err(Status::invalid_argument(format!(
                "duplicate key in batch: {key}"
            )));
        }
    }
    Ok(())
}

fn map_dispatcher_error(err: &DispatcherError) -> (ErrorCode, String) {
    match err {
        DispatcherError::NotInitialized(msg) => (ErrorCode::NotInitialized, msg.clone()),
        DispatcherError::KeyNotFound(k) => {
            (ErrorCode::KeyNotFound, format!("key not found: {k}"))
        }
        DispatcherError::AlreadyExists(k) => {
            (ErrorCode::AlreadyExists, format!("key already exists: {k}"))
        }
        DispatcherError::AllocationFailed(msg) => (ErrorCode::AllocationFailed, msg.clone()),
        DispatcherError::IoError(msg) => (ErrorCode::IoError, msg.clone()),
        DispatcherError::Timeout(msg) => (ErrorCode::Timeout, msg.clone()),
        DispatcherError::InvalidParameter(msg) => (ErrorCode::InvalidParameter, msg.clone()),
    }
}

fn success_result(key: u64) -> EntryResult {
    EntryResult {
        key,
        success: true,
        error_code: ErrorCode::Unspecified.into(),
        error_message: String::new(),
    }
}

fn error_result(key: u64, err: &DispatcherError) -> EntryResult {
    let (code, msg) = map_dispatcher_error(err);
    EntryResult {
        key,
        success: false,
        error_code: code.into(),
        error_message: msg,
    }
}

#[tonic::async_trait]
impl Dispatcher for DispatcherService {
    async fn populate(
        &self,
        request: Request<BatchPopulateRequest>,
    ) -> Result<Response<BatchPopulateResponse>, Status> {
        let req = request.into_inner();
        let keys: Vec<u64> = req.entries.iter().map(|e| e.key).collect();
        check_duplicate_keys(&keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let cache = Arc::clone(&self.ipc_cache);
        let populates_counter = Arc::clone(&self.counters.populates);
        let results = tokio::task::spawn_blocking(move || {
            let mut opened_keys: Vec<[u8; 64]> = Vec::new();
            let mut pre_errors: Vec<Option<EntryResult>> = vec![None; req.entries.len()];

            let mut local_ptrs: HashMap<[u8; 64], *mut std::ffi::c_void> = HashMap::new();
            for (i, entry) in req.entries.iter().enumerate() {
                let handle = match entry.ipc_handle.as_ref() {
                    Some(h) => h,
                    None => {
                        pre_errors[i] = Some(error_result(
                            entry.key,
                            &DispatcherError::InvalidParameter("missing ipc_handle".into()),
                        ));
                        continue;
                    }
                };
                let key: [u8; 64] = match handle.cuda_ipc_handle.as_slice().try_into() {
                    Ok(k) => k,
                    Err(_) => {
                        pre_errors[i] = Some(error_result(
                            entry.key,
                            &DispatcherError::InvalidParameter(format!(
                                "cuda_ipc_handle must be 64 bytes, got {}",
                                handle.cuda_ipc_handle.len()
                            )),
                        ));
                        continue;
                    }
                };
                if let std::collections::hash_map::Entry::Vacant(slot) = local_ptrs.entry(key) {
                    match ipc_cache_open(&cache, &key, handle.gpu_device_id) {
                        Ok(ptr) => {
                            slot.insert(ptr);
                            opened_keys.push(key);
                        }
                        Err(e) => {
                            pre_errors[i] = Some(error_result(
                                entry.key,
                                &DispatcherError::IoError(format!("IPC open failed: {e}")),
                            ));
                        }
                    }
                }
            }

            let results: Vec<EntryResult> = req
                .entries
                .iter()
                .enumerate()
                .map(|(i, entry)| {
                    if let Some(err) = pre_errors[i].take() {
                        return err;
                    }
                    let handle = entry.ipc_handle.as_ref().unwrap();
                    let key: [u8; 64] = handle.cuda_ipc_handle.as_slice().try_into().unwrap();
                    let dev_ptr = match local_ptrs.get(&key) {
                        Some(&ptr) => ptr,
                        None => {
                            return error_result(
                                entry.key,
                                &DispatcherError::IoError("IPC handle not cached".into()),
                            )
                        }
                    };
                    let ipc = IpcHandle {
                        // dev_ptr is the allocation base; offset addresses this block within it.
                        address: (dev_ptr as usize + handle.offset as usize) as *mut u8,
                        size: handle.size,
                    };
                    match dispatcher.populate(entry.key, ipc) {
                        Ok(()) => {
                            populates_counter.fetch_add(1, Ordering::Relaxed);
                            success_result(entry.key)
                        }
                        Err(e) => error_result(entry.key, &e),
                    }
                })
                .collect();

            for key in &opened_keys {
                ipc_cache_close(&cache, key);
            }
            results
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchPopulateResponse { results }))
    }

    async fn lookup(
        &self,
        request: Request<BatchLookupRequest>,
    ) -> Result<Response<BatchLookupResponse>, Status> {
        let req = request.into_inner();
        let keys: Vec<u64> = req.entries.iter().map(|e| e.key).collect();
        check_duplicate_keys(&keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let cache = Arc::clone(&self.ipc_cache);
        let hits_counter = Arc::clone(&self.counters.lookup_hits);
        let misses_counter = Arc::clone(&self.counters.lookup_misses);
        let gpu_bytes_counter = Arc::clone(&self.counters.gpu_bytes_transferred);
        let results = tokio::task::spawn_blocking(move || {
            let mut opened_keys: Vec<[u8; 64]> = Vec::new();
            let mut batch_entries: Vec<(u64, IpcHandle)> = Vec::with_capacity(req.entries.len());
            let mut pre_errors: Vec<Option<EntryResult>> = vec![None; req.entries.len()];
            let mut local_ptrs: HashMap<[u8; 64], *mut std::ffi::c_void> = HashMap::new();

            for (i, entry) in req.entries.iter().enumerate() {
                let handle = match entry.ipc_handle.as_ref() {
                    Some(h) => h,
                    None => {
                        pre_errors[i] = Some(error_result(
                            entry.key,
                            &DispatcherError::InvalidParameter("missing ipc_handle".into()),
                        ));
                        batch_entries.push((
                            entry.key,
                            IpcHandle {
                                address: std::ptr::null_mut(),
                                size: 0,
                            },
                        ));
                        continue;
                    }
                };
                let handle_key: [u8; 64] = match handle.cuda_ipc_handle.as_slice().try_into() {
                    Ok(k) => k,
                    Err(_) => {
                        pre_errors[i] = Some(error_result(
                            entry.key,
                            &DispatcherError::InvalidParameter(format!(
                                "cuda_ipc_handle must be 64 bytes, got {}",
                                handle.cuda_ipc_handle.len()
                            )),
                        ));
                        batch_entries.push((
                            entry.key,
                            IpcHandle {
                                address: std::ptr::null_mut(),
                                size: 0,
                            },
                        ));
                        continue;
                    }
                };
                let dev_ptr = match local_ptrs.get(&handle_key) {
                    Some(&ptr) => ptr,
                    None => match ipc_cache_open(&cache, &handle_key, handle.gpu_device_id) {
                        Ok(ptr) => {
                            local_ptrs.insert(handle_key, ptr);
                            opened_keys.push(handle_key);
                            ptr
                        }
                        Err(e) => {
                            pre_errors[i] = Some(error_result(
                                entry.key,
                                &DispatcherError::IoError(format!("IPC open failed: {e}")),
                            ));
                            batch_entries.push((
                                entry.key,
                                IpcHandle {
                                    address: std::ptr::null_mut(),
                                    size: 0,
                                },
                            ));
                            continue;
                        }
                    },
                };
                batch_entries.push((
                    entry.key,
                    IpcHandle {
                        // dev_ptr is the allocation base (deduped per handle); offset is
                        // per-entry, so apply it here to address this block within the alloc.
                        address: (dev_ptr as usize + handle.offset as usize) as *mut u8,
                        size: handle.size,
                    },
                ));
            }

            let valid_indices: Vec<usize> = (0..batch_entries.len())
                .filter(|&i| pre_errors[i].is_none())
                .collect();
            let valid_batch: Vec<(u64, IpcHandle)> = valid_indices
                .iter()
                .map(|&i| {
                    let (key, ref ipc) = batch_entries[i];
                    (key, IpcHandle {
                        address: ipc.address,
                        size: ipc.size,
                    })
                })
                .collect();

            let batch_results = dispatcher.batch_lookup(&valid_batch);

            let mut results: Vec<EntryResult> = Vec::with_capacity(req.entries.len());
            let mut batch_iter = batch_results.into_iter();
            for (i, entry) in req.entries.iter().enumerate() {
                if let Some(err_result) = pre_errors[i].take() {
                    results.push(err_result);
                } else {
                    match batch_iter.next().unwrap() {
                        Ok(()) => {
                            hits_counter.fetch_add(1, Ordering::Relaxed);
                            if let Some(h) = entry.ipc_handle.as_ref() {
                                gpu_bytes_counter.fetch_add(h.size as u64, Ordering::Relaxed);
                            }
                            results.push(success_result(entry.key));
                        }
                        Err(ref e) if matches!(e, DispatcherError::KeyNotFound(_)) => {
                            misses_counter.fetch_add(1, Ordering::Relaxed);
                            results.push(error_result(entry.key, e));
                        }
                        Err(e) => results.push(error_result(entry.key, &e)),
                    }
                }
            }

            for key in &opened_keys {
                ipc_cache_close(&cache, key);
            }

            results
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchLookupResponse { results }))
    }

    async fn check(
        &self,
        request: Request<BatchCheckRequest>,
    ) -> Result<Response<BatchCheckResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| {
                    let exists: bool = dispatcher.check(key).unwrap_or_default();
                    CheckResult { key, exists }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchCheckResponse { results }))
    }

    async fn remove(
        &self,
        request: Request<BatchRemoveRequest>,
    ) -> Result<Response<BatchRemoveResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| match dispatcher.remove(key) {
                    Ok(()) => success_result(key),
                    Err(e) => error_result(key, &e),
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchRemoveResponse { results }))
    }

    async fn touch(
        &self,
        request: Request<BatchTouchRequest>,
    ) -> Result<Response<BatchTouchResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let promote = req.promote;
        let keys = req.keys;

        let results = tokio::task::spawn_blocking({
            let dispatcher = Arc::clone(&dispatcher);
            let keys = keys.clone();
            move || {
                keys.iter()
                    .map(|&key| match dispatcher.touch(key) {
                        Ok(()) => success_result(key),
                        Err(e) => error_result(key, &e),
                    })
                    .collect::<Vec<_>>()
            }
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        if promote {
            tokio::task::spawn_blocking(move || {
                dispatcher.promote_to_memory_tier(&keys);
            });
        }

        Ok(Response::new(BatchTouchResponse { results }))
    }

    async fn reserve(
        &self,
        request: Request<BatchReserveRequest>,
    ) -> Result<Response<BatchReserveResponse>, Status> {
        let req = request.into_inner();
        let keys: Vec<u64> = req.entries.iter().map(|e| e.key).collect();
        check_duplicate_keys(&keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let pending = Arc::clone(&self.pending_stores);
        let results = tokio::task::spawn_blocking(move || {
            req.entries
                .iter()
                .map(|entry| match dispatcher.reserve_memory(entry.key, entry.size) {
                    Ok(_ptr) => {
                        pending
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(entry.key, PendingStoreEntry { size: entry.size });
                        success_result(entry.key)
                    }
                    Err(e) => error_result(entry.key, &e),
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchReserveResponse { results }))
    }

    async fn copy_to_store(
        &self,
        request: Request<BatchCopyToStoreRequest>,
    ) -> Result<Response<BatchCopyToStoreResponse>, Status> {
        let req = request.into_inner();
        let keys: Vec<u64> = req.entries.iter().map(|e| e.key).collect();
        check_duplicate_keys(&keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let cache = Arc::clone(&self.ipc_cache);
        let results = tokio::task::spawn_blocking(move || {
            let mut opened_keys: Vec<[u8; 64]> = Vec::new();
            let mut pre_errors: Vec<Option<EntryResult>> = vec![None; req.entries.len()];
            let mut local_ptrs: HashMap<[u8; 64], *mut std::ffi::c_void> = HashMap::new();

            for (i, entry) in req.entries.iter().enumerate() {
                let handle = match entry.ipc_handle.as_ref() {
                    Some(h) => h,
                    None => {
                        pre_errors[i] = Some(error_result(
                            entry.key,
                            &DispatcherError::InvalidParameter("missing ipc_handle".into()),
                        ));
                        continue;
                    }
                };
                let key: [u8; 64] = match handle.cuda_ipc_handle.as_slice().try_into() {
                    Ok(k) => k,
                    Err(_) => {
                        pre_errors[i] = Some(error_result(
                            entry.key,
                            &DispatcherError::InvalidParameter(format!(
                                "cuda_ipc_handle must be 64 bytes, got {}",
                                handle.cuda_ipc_handle.len()
                            )),
                        ));
                        continue;
                    }
                };
                if let std::collections::hash_map::Entry::Vacant(slot) = local_ptrs.entry(key) {
                    match ipc_cache_open(&cache, &key, handle.gpu_device_id) {
                        Ok(ptr) => {
                            slot.insert(ptr);
                            opened_keys.push(key);
                        }
                        Err(e) => {
                            pre_errors[i] = Some(error_result(
                                entry.key,
                                &DispatcherError::IoError(format!("IPC open failed: {e}")),
                            ));
                        }
                    }
                }
            }

            let results: Vec<EntryResult> = req
                .entries
                .iter()
                .enumerate()
                .map(|(i, entry)| {
                    if let Some(err) = pre_errors[i].take() {
                        return err;
                    }
                    let handle = entry.ipc_handle.as_ref().unwrap();
                    let ipc_key: [u8; 64] = handle.cuda_ipc_handle.as_slice().try_into().unwrap();
                    let dev_ptr = match local_ptrs.get(&ipc_key) {
                        Some(&ptr) => ptr,
                        None => {
                            return error_result(
                                entry.key,
                                &DispatcherError::IoError("IPC handle not cached".into()),
                            )
                        }
                    };
                    let ipc = IpcHandle {
                        // dev_ptr is the allocation base; offset addresses this block within it.
                        address: (dev_ptr as usize + handle.offset as usize) as *mut u8,
                        size: handle.size,
                    };
                    match dispatcher.copy_gpu_to_memory_async(
                        entry.key,
                        ipc,
                        GpuStream(std::ptr::null_mut()),
                    ) {
                        Ok(()) => success_result(entry.key),
                        Err(e) => error_result(entry.key, &e),
                    }
                })
                .collect();

            for key in &opened_keys {
                ipc_cache_close(&cache, key);
            }
            results
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchCopyToStoreResponse { results }))
    }

    async fn commit_store(
        &self,
        request: Request<BatchCommitStoreRequest>,
    ) -> Result<Response<BatchCommitStoreResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let pending = Arc::clone(&self.pending_stores);
        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| {
                    let size = {
                        let map = pending.lock().unwrap_or_else(|e| e.into_inner());
                        match map.get(&key) {
                            Some(entry) => entry.size,
                            None => {
                                return error_result(
                                    key,
                                    &DispatcherError::KeyNotFound(key),
                                )
                            }
                        }
                    };
                    match dispatcher.copy_gpu_to_memory_completed(key, size) {
                        Ok(()) => {
                            pending
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .remove(&key);
                            success_result(key)
                        }
                        Err(e) => error_result(key, &e),
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchCommitStoreResponse { results }))
    }

    async fn abort_store(
        &self,
        request: Request<BatchAbortStoreRequest>,
    ) -> Result<Response<BatchAbortStoreResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let pending = Arc::clone(&self.pending_stores);
        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| {
                    pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&key);
                    match dispatcher.release_memory(key) {
                        Ok(()) => success_result(key),
                        Err(e) => error_result(key, &e),
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchAbortStoreResponse { results }))
    }

    async fn clear_memory_tier(
        &self,
        _request: Request<ClearMemoryTierRequest>,
    ) -> Result<Response<ClearMemoryTierResponse>, Status> {
        let dispatcher = Arc::clone(&self.dispatcher);
        let entries_cleared = tokio::task::spawn_blocking(move || dispatcher.clear_memory_tier())
            .await
            .map_err(|e| Status::internal(format!("task join error: {e}")))?
            .map_err(|e| Status::internal(format!("clear_memory_tier failed: {e}")))?;

        Ok(Response::new(ClearMemoryTierResponse {
            entries_cleared: entries_cleared as u64,
        }))
    }

    async fn flush_to_ssd(
        &self,
        _request: Request<FlushToSsdRequest>,
    ) -> Result<Response<FlushToSsdResponse>, Status> {
        let dispatcher = Arc::clone(&self.dispatcher);
        let jobs_flushed = tokio::task::spawn_blocking(move || dispatcher.flush_to_ssd())
            .await
            .map_err(|e| Status::internal(format!("task join error: {e}")))?
            .map_err(|e| Status::internal(format!("flush_to_ssd failed: {e}")))?;

        Ok(Response::new(FlushToSsdResponse {
            jobs_flushed: jobs_flushed as u64,
        }))
    }

    async fn pin(
        &self,
        request: Request<BatchPinRequest>,
    ) -> Result<Response<BatchPinResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let promote = req.promote;
        let keys = req.keys;

        let results = tokio::task::spawn_blocking({
            let dispatcher = Arc::clone(&dispatcher);
            let keys = keys.clone();
            move || {
                keys.iter()
                    .map(|&key| match dispatcher.pin(key) {
                        Ok(()) => success_result(key),
                        Err(e) => error_result(key, &e),
                    })
                    .collect::<Vec<_>>()
            }
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        if promote {
            tokio::task::spawn_blocking(move || {
                dispatcher.promote_to_memory_tier(&keys);
            });
        }

        Ok(Response::new(BatchPinResponse { results }))
    }

    async fn unpin(
        &self,
        request: Request<BatchUnpinRequest>,
    ) -> Result<Response<BatchUnpinResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;

        let dispatcher = Arc::clone(&self.dispatcher);
        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| match dispatcher.unpin(key) {
                    Ok(()) => success_result(key),
                    Err(e) => error_result(key, &e),
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchUnpinResponse { results }))
    }

    async fn take_events(
        &self,
        request: Request<TakeEventsRequest>,
    ) -> Result<Response<TakeEventsResponse>, Status> {
        let req = request.into_inner();
        let max = req.max_events as usize;

        let mut events = Vec::new();
        loop {
            match self.eviction_rx.try_recv() {
                Ok(ev) => {
                    self.counters.evictions.fetch_add(1, Ordering::Relaxed);
                    events.push(proto::EvictionEvent {
                        key: ev.key,
                        reason: match ev.reason {
                            EvictionReason::Demoted => {
                                proto::EvictionReason::Demoted.into()
                            }
                            EvictionReason::Removed => {
                                proto::EvictionReason::Removed.into()
                            }
                        },
                    });
                    if max > 0 && events.len() >= max {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        let dropped_count = self.eviction_dropped.swap(0, Ordering::Relaxed);

        Ok(Response::new(TakeEventsResponse {
            events,
            dropped_count,
        }))
    }

    async fn get_io_stats(
        &self,
        _request: Request<GetIoStatsRequest>,
    ) -> Result<Response<IoStatsResponse>, Status> {
        let dispatcher = Arc::clone(&self.dispatcher);
        let s = tokio::task::spawn_blocking(move || dispatcher.read_write_stats())
            .await
            .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(IoStatsResponse {
            read_ops: s.read_ops,
            read_bytes: s.read_bytes,
            read_latency_ns_sum: s.read_latency_ns_sum,
            write_ops: s.write_ops,
            write_bytes: s.write_bytes,
            write_latency_ns_sum: s.write_latency_ns_sum,
        }))
    }
}
