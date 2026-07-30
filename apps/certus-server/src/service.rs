//! gRPC service implementation for the Certus Dispatcher.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tonic::{Request, Response, Status};

use gpu_services::cuda_ffi;
use interfaces::{DispatcherError, GpuStream, IDispatcher, IpcHandle};

#[cfg(feature = "otel")]
use crate::telemetry::Metrics;

pub mod proto {
    tonic::include_proto!("certus.dispatcher.v1");
}

use proto::dispatcher_server::{Dispatcher, DispatcherServer};
use proto::{
    BatchAbortStoreRequest, BatchAbortStoreResponse, BatchCheckRequest, BatchCheckResponse,
    BatchCommitStoreRequest, BatchCommitStoreResponse, BatchCopyToStoreRequest,
    BatchCopyToStoreResponse, BatchLookupRequest, BatchLookupResponse, BatchPinRequest,
    BatchPinResponse, BatchPopulateRequest, BatchPopulateResponse, BatchRemoveRequest,
    BatchRemoveResponse, BatchReserveRequest, BatchReserveResponse, BatchTouchRequest,
    BatchTouchResponse, BatchUnpinRequest, BatchUnpinResponse, CheckResult, ClearMemoryTierRequest,
    ClearMemoryTierResponse, EntryResult, ErrorCode, FlushToSsdRequest, FlushToSsdResponse,
    GetIoStatsRequest, IoStatsResponse, TakeEventsRequest, TakeEventsResponse,
};

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

pub struct DispatcherService {
    dispatcher: Arc<dyn IDispatcher + Send + Sync>,
    ipc_cache: IpcCache,
    pending_stores: PendingStores,
    eviction_rx: crossbeam_channel::Receiver<dispatcher::EvictionEvent>,
    eviction_dropped: Arc<AtomicU64>,
    #[cfg(feature = "otel")]
    metrics: Option<Metrics>,
}

impl DispatcherService {
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
            #[cfg(feature = "otel")]
            metrics: None,
        }
    }

    #[cfg(feature = "otel")]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = Some(metrics);
        self
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
        DispatcherError::KeyNotFound(k) => (ErrorCode::KeyNotFound, format!("key not found: {k}")),
        DispatcherError::AlreadyExists(k) => {
            (ErrorCode::AlreadyExists, format!("key already exists: {k}"))
        }
        DispatcherError::AllocationFailed(msg) => (ErrorCode::AllocationFailed, msg.clone()),
        DispatcherError::IoError(msg) => (ErrorCode::IoError, msg.clone()),
        DispatcherError::Timeout(msg) => (ErrorCode::Timeout, msg.clone()),
        DispatcherError::InvalidParameter(msg) => (ErrorCode::InvalidParameter, msg.clone()),
    }
}

/// Throttled diagnostic for store copies that fail server-side. A whole run's
/// worth of per-key failures is returned to the client (in `error_message`) but
/// was otherwise invisible in the server log — a silent-offload regression (e.g.
/// a Reserve slot smaller than the copy size) could run to completion looking
/// healthy. Log the first `COPY_FAIL_LOG_MAX` failures with their real reason,
/// then a throttled tail, so the server self-documents.
static COPY_FAIL_LOGGED: AtomicU64 = AtomicU64::new(0);
const COPY_FAIL_LOG_MAX: u64 = 20;

fn log_copy_failures(results: &[EntryResult], batch_len: usize) {
    let first = match results.iter().find(|r| !r.success) {
        Some(r) => r,
        None => return,
    };
    let failed = results.iter().filter(|r| !r.success).count();
    let n = COPY_FAIL_LOGGED.fetch_add(1, Ordering::Relaxed);
    if n < COPY_FAIL_LOG_MAX || n.is_power_of_two() {
        eprintln!(
            "[certus-server] WARN copy_to_store: {failed}/{batch_len} blocks failed \
             (occurrence #{}); first: key={} error_code={} msg={:?}",
            n + 1,
            first.key,
            first.error_code,
            first.error_message,
        );
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
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

        let dispatcher = Arc::clone(&self.dispatcher);
        let cache = Arc::clone(&self.ipc_cache);
        let results = tokio::task::spawn_blocking(move || {
            let mut opened_keys: Vec<[u8; 64]> = Vec::new();
            let mut pre_errors: Vec<Option<EntryResult>> = vec![None; req.entries.len()];

            // Resolve all unique IPC handles upfront via global cache.
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
                if !local_ptrs.contains_key(&key) {
                    match ipc_cache_open(&cache, &key, handle.gpu_device_id) {
                        Ok(ptr) => {
                            local_ptrs.insert(key, ptr);
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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            let errors = results.iter().filter(|r| !r.success).count() as u64;
            m.record_op(
                "populate",
                results.len() as u64,
                errors,
                _t0.elapsed().as_micros() as f64,
            );
        }

        Ok(Response::new(BatchPopulateResponse { results }))
    }

    async fn lookup(
        &self,
        request: Request<BatchLookupRequest>,
    ) -> Result<Response<BatchLookupResponse>, Status> {
        let req = request.into_inner();
        let keys: Vec<u64> = req.entries.iter().map(|e| e.key).collect();
        check_duplicate_keys(&keys)?;
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

        let dispatcher = Arc::clone(&self.dispatcher);
        let cache = Arc::clone(&self.ipc_cache);
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

            // Filter to valid entries and call batch_lookup.
            let valid_indices: Vec<usize> = (0..batch_entries.len())
                .filter(|&i| pre_errors[i].is_none())
                .collect();
            let valid_batch: Vec<(u64, IpcHandle)> = valid_indices
                .iter()
                .map(|&i| {
                    let (key, ref ipc) = batch_entries[i];
                    (
                        key,
                        IpcHandle {
                            address: ipc.address,
                            size: ipc.size,
                        },
                    )
                })
                .collect();

            let batch_results = dispatcher.batch_lookup(&valid_batch);

            // Merge results back.
            let mut results: Vec<EntryResult> = Vec::with_capacity(req.entries.len());
            let mut batch_iter = batch_results.into_iter();
            for (i, entry) in req.entries.iter().enumerate() {
                if let Some(err_result) = pre_errors[i].take() {
                    results.push(err_result);
                } else {
                    match batch_iter.next().unwrap() {
                        Ok(()) => results.push(success_result(entry.key)),
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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            let errors = results.iter().filter(|r| !r.success).count() as u64;
            m.record_op(
                "lookup",
                results.len() as u64,
                errors,
                _t0.elapsed().as_micros() as f64,
            );
        }

        Ok(Response::new(BatchLookupResponse { results }))
    }

    async fn check(
        &self,
        request: Request<BatchCheckRequest>,
    ) -> Result<Response<BatchCheckResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

        let dispatcher = Arc::clone(&self.dispatcher);
        let batch_len = req.keys.len() as u64;
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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            m.record_op("check", batch_len, 0, _t0.elapsed().as_micros() as f64);
        }
        #[cfg(not(feature = "otel"))]
        let _ = batch_len;

        Ok(Response::new(BatchCheckResponse { results }))
    }

    async fn remove(
        &self,
        request: Request<BatchRemoveRequest>,
    ) -> Result<Response<BatchRemoveResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            let errors = results.iter().filter(|r| !r.success).count() as u64;
            m.record_op(
                "remove",
                results.len() as u64,
                errors,
                _t0.elapsed().as_micros() as f64,
            );
        }

        Ok(Response::new(BatchRemoveResponse { results }))
    }

    async fn touch(
        &self,
        request: Request<BatchTouchRequest>,
    ) -> Result<Response<BatchTouchResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            let errors = results.iter().filter(|r| !r.success).count() as u64;
            m.record_op(
                "touch",
                results.len() as u64,
                errors,
                _t0.elapsed().as_micros() as f64,
            );
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
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

        let dispatcher = Arc::clone(&self.dispatcher);
        let pending = Arc::clone(&self.pending_stores);
        let results = tokio::task::spawn_blocking(move || {
            req.entries
                .iter()
                .map(|entry| {
                    match dispatcher.reserve_memory(entry.key, entry.size, entry.session_id) {
                        Ok(_ptr) => {
                            pending
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(entry.key, PendingStoreEntry { size: entry.size });
                            success_result(entry.key)
                        }
                        Err(e) => error_result(entry.key, &e),
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            let errors = results.iter().filter(|r| !r.success).count() as u64;
            m.record_op(
                "reserve",
                results.len() as u64,
                errors,
                _t0.elapsed().as_micros() as f64,
            );
        }

        Ok(Response::new(BatchReserveResponse { results }))
    }

    async fn copy_to_store(
        &self,
        request: Request<BatchCopyToStoreRequest>,
    ) -> Result<Response<BatchCopyToStoreResponse>, Status> {
        let req = request.into_inner();
        let keys: Vec<u64> = req.entries.iter().map(|e| e.key).collect();
        check_duplicate_keys(&keys)?;
        let req_len = req.entries.len();
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

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
                if !local_ptrs.contains_key(&key) {
                    match ipc_cache_open(&cache, &key, handle.gpu_device_id) {
                        Ok(ptr) => {
                            local_ptrs.insert(key, ptr);
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

        if results.iter().any(|r| !r.success) {
            log_copy_failures(&results, req_len);
        }

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            let errors = results.iter().filter(|r| !r.success).count() as u64;
            m.record_op(
                "copy_to_store",
                results.len() as u64,
                errors,
                _t0.elapsed().as_micros() as f64,
            );
        }

        Ok(Response::new(BatchCopyToStoreResponse { results }))
    }

    async fn commit_store(
        &self,
        request: Request<BatchCommitStoreRequest>,
    ) -> Result<Response<BatchCommitStoreResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

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
                            None => return error_result(key, &DispatcherError::KeyNotFound(key)),
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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            let errors = results.iter().filter(|r| !r.success).count() as u64;
            m.record_op(
                "commit_store",
                results.len() as u64,
                errors,
                _t0.elapsed().as_micros() as f64,
            );
        }

        Ok(Response::new(BatchCommitStoreResponse { results }))
    }

    async fn abort_store(
        &self,
        request: Request<BatchAbortStoreRequest>,
    ) -> Result<Response<BatchAbortStoreResponse>, Status> {
        let req = request.into_inner();
        check_duplicate_keys(&req.keys)?;
        #[cfg(feature = "otel")]
        let _t0 = std::time::Instant::now();

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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            let errors = results.iter().filter(|r| !r.success).count() as u64;
            m.record_op(
                "abort_store",
                results.len() as u64,
                errors,
                _t0.elapsed().as_micros() as f64,
            );
        }

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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            m.entries_cleared.add(entries_cleared as u64, &[]);
        }

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

        #[cfg(feature = "otel")]
        if let Some(ref m) = self.metrics {
            m.jobs_flushed.add(jobs_flushed as u64, &[]);
        }

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
                    events.push(proto::EvictionEvent {
                        key: ev.key,
                        reason: match ev.reason {
                            dispatcher::EvictionReason::Demoted => {
                                proto::EvictionReason::Demoted.into()
                            }
                            dispatcher::EvictionReason::Removed => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use interfaces::{DispatcherConfig, GpuStream};

    fn test_service(dispatcher: Arc<dyn IDispatcher + Send + Sync>) -> DispatcherService {
        let (_tx, rx) = crossbeam_channel::bounded(16);
        let dropped = Arc::new(AtomicU64::new(0));
        DispatcherService::new(dispatcher, rx, dropped)
    }

    struct MockDispatcherState {
        populate_results: HashMap<u64, Result<(), DispatcherError>>,
        batch_lookup_results: Vec<Result<(), DispatcherError>>,
        check_results: HashMap<u64, Result<bool, DispatcherError>>,
        remove_results: HashMap<u64, Result<(), DispatcherError>>,
        touch_results: HashMap<u64, Result<(), DispatcherError>>,
        clear_memory_tier_result: Result<usize, DispatcherError>,
        populate_calls: Vec<(u64, u32)>,
        /// Resolved device address passed to `populate`, per key. Used to verify
        /// the server folds `IpcHandle.offset` into the opened base pointer.
        populate_addrs: Vec<(u64, usize)>,
        batch_lookup_calls: Vec<Vec<u64>>,
        check_calls: Vec<u64>,
    }

    impl Default for MockDispatcherState {
        fn default() -> Self {
            Self {
                populate_results: HashMap::new(),
                batch_lookup_results: Vec::new(),
                check_results: HashMap::new(),
                remove_results: HashMap::new(),
                touch_results: HashMap::new(),
                clear_memory_tier_result: Ok(0),
                populate_calls: Vec::new(),
                populate_addrs: Vec::new(),
                batch_lookup_calls: Vec::new(),
                check_calls: Vec::new(),
            }
        }
    }

    struct MockDispatcher {
        state: Mutex<MockDispatcherState>,
    }

    impl MockDispatcher {
        fn new(state: MockDispatcherState) -> Self {
            Self {
                state: Mutex::new(state),
            }
        }
    }

    impl Default for MockDispatcher {
        fn default() -> Self {
            Self::new(Default::default())
        }
    }

    impl IDispatcher for MockDispatcher {
        fn initialize(&self, _config: DispatcherConfig) -> Result<(), DispatcherError> {
            Ok(())
        }

        fn shutdown(&self) -> Result<(), DispatcherError> {
            Ok(())
        }

        fn lookup(&self, _key: u64, _ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
            Ok(())
        }

        fn lookup_async(
            &self,
            _key: u64,
            _ipc_handle: IpcHandle,
        ) -> Result<GpuStream, DispatcherError> {
            Ok(GpuStream(std::ptr::null_mut()))
        }

        fn batch_lookup(&self, entries: &[(u64, IpcHandle)]) -> Vec<Result<(), DispatcherError>> {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .batch_lookup_calls
                .push(entries.iter().map(|(key, _)| *key).collect());
            if state.batch_lookup_results.is_empty() {
                vec![Ok(()); entries.len()]
            } else {
                state.batch_lookup_results.clone()
            }
        }

        fn check(&self, key: u64) -> Result<bool, DispatcherError> {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.check_calls.push(key);
            state.check_results.get(&key).cloned().unwrap_or(Ok(false))
        }

        fn remove(&self, key: u64) -> Result<(), DispatcherError> {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove_results
                .get(&key)
                .cloned()
                .unwrap_or(Ok(()))
        }

        fn populate(&self, key: u64, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.populate_calls.push((key, ipc_handle.size));
            state
                .populate_addrs
                .push((key, ipc_handle.address as usize));
            state.populate_results.get(&key).cloned().unwrap_or(Ok(()))
        }

        fn reserve_memory(
            &self,
            _key: u64,
            _size: u32,
            _session_id: u64,
        ) -> Result<*mut u8, DispatcherError> {
            Ok(std::ptr::null_mut())
        }

        fn copy_gpu_to_memory_async(
            &self,
            _key: u64,
            _ipc_handle: IpcHandle,
            _stream: GpuStream,
        ) -> Result<(), DispatcherError> {
            Ok(())
        }

        fn copy_gpu_to_memory_completed(
            &self,
            _key: u64,
            _size: u32,
        ) -> Result<(), DispatcherError> {
            Ok(())
        }

        fn release_memory(&self, _key: u64) -> Result<(), DispatcherError> {
            Ok(())
        }

        fn touch(&self, key: u64) -> Result<(), DispatcherError> {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .touch_results
                .get(&key)
                .cloned()
                .unwrap_or(Ok(()))
        }

        fn promote_to_memory_tier(&self, _keys: &[u64]) {}

        fn clear_memory_tier(&self) -> Result<usize, DispatcherError> {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear_memory_tier_result
                .clone()
        }

        fn flush_to_ssd(&self) -> Result<usize, DispatcherError> {
            Ok(0)
        }

        fn pin(&self, _key: u64) -> Result<(), DispatcherError> {
            Ok(())
        }

        fn unpin(&self, _key: u64) -> Result<(), DispatcherError> {
            Ok(())
        }

        fn read_write_stats(&self) -> interfaces::ReadWriteStats {
            interfaces::ReadWriteStats::default()
        }
    }

    fn proto_ipc_handle(seed: u8) -> proto::IpcHandle {
        proto::IpcHandle {
            cuda_ipc_handle: vec![seed; 64],
            size: 4096,
            gpu_device_id: -1,
            offset: 0,
        }
    }

    fn handle_key(handle: &proto::IpcHandle) -> [u8; 64] {
        handle
            .cuda_ipc_handle
            .as_slice()
            .try_into()
            .expect("test handles are always 64 bytes")
    }

    fn cache_entry(refcount: usize) -> IpcCacheEntry {
        IpcCacheEntry {
            dev_ptr: std::ptr::null_mut(),
            gpu_device_id: -1,
            refcount,
        }
    }

    #[tokio::test]
    async fn populate_happy_path_uses_cached_ipc_and_stores() {
        let mock = Arc::new(MockDispatcher::default());
        let service = test_service(mock.clone());
        let ipc_handle = proto_ipc_handle(1);
        let key = handle_key(&ipc_handle);
        service
            .ipc_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, cache_entry(1));

        let request = BatchPopulateRequest {
            entries: vec![proto::PopulateEntry {
                key: 10,
                ipc_handle: Some(ipc_handle),
            }],
        };
        let response = service
            .populate(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.results.len(), 1);
        assert!(response.results[0].success);
        assert_eq!(response.results[0].key, 10);
        assert_eq!(
            mock.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .populate_calls,
            vec![(10, 4096)]
        );
        assert_eq!(
            service
                .ipc_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .unwrap()
                .refcount,
            1
        );
    }

    #[tokio::test]
    async fn populate_folds_offset_into_resolved_address() {
        // Seed the IPC cache with a known non-null base so we can assert the
        // server adds IpcHandle.offset to the opened allocation base.
        const BASE: usize = 0x1000_0000;
        const OFFSET: u64 = 0x4_0000;
        let mock = Arc::new(MockDispatcher::default());
        let service = test_service(mock.clone());
        let mut ipc_handle = proto_ipc_handle(2);
        ipc_handle.offset = OFFSET;
        let key = handle_key(&ipc_handle);
        service
            .ipc_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                key,
                IpcCacheEntry {
                    dev_ptr: BASE as *mut std::ffi::c_void,
                    gpu_device_id: -1,
                    refcount: 1,
                },
            );

        let request = BatchPopulateRequest {
            entries: vec![proto::PopulateEntry {
                key: 20,
                ipc_handle: Some(ipc_handle),
            }],
        };
        let response = service
            .populate(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert!(response.results[0].success);
        assert_eq!(
            mock.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .populate_addrs,
            vec![(20, BASE + OFFSET as usize)]
        );
    }

    #[tokio::test]
    async fn populate_reports_ipc_open_failure() {
        let mock = Arc::new(MockDispatcher::default());
        let service = test_service(mock.clone());

        let request = BatchPopulateRequest {
            entries: vec![proto::PopulateEntry {
                key: 11,
                ipc_handle: Some(proto_ipc_handle(0)),
            }],
        };
        let response = service
            .populate(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.results.len(), 1);
        let result = &response.results[0];
        assert!(!result.success);
        assert_eq!(result.error_code, ErrorCode::IoError as i32);
        assert!(result.error_message.contains("IPC open failed"));
        assert!(mock
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .populate_calls
            .is_empty());
    }

    #[tokio::test]
    async fn populate_maps_dispatcher_error_response() {
        let mut state = MockDispatcherState::default();
        state
            .populate_results
            .insert(12, Err(DispatcherError::Timeout("pending".to_string())));
        let mock = Arc::new(MockDispatcher::new(state));
        let service = test_service(mock);

        let ipc_handle = proto_ipc_handle(2);
        let key = handle_key(&ipc_handle);
        service
            .ipc_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, cache_entry(1));

        let request = BatchPopulateRequest {
            entries: vec![proto::PopulateEntry {
                key: 12,
                ipc_handle: Some(ipc_handle),
            }],
        };
        let response = service
            .populate(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.results.len(), 1);
        assert!(!response.results[0].success);
        assert_eq!(response.results[0].error_code, ErrorCode::Timeout as i32);
    }

    #[tokio::test]
    async fn lookup_handles_mixed_success_and_failures() {
        let mut state = MockDispatcherState::default();
        state.batch_lookup_results = vec![
            Ok(()),
            Err(DispatcherError::KeyNotFound(22)),
            Err(DispatcherError::Timeout("pending".to_string())),
        ];
        let mock = Arc::new(MockDispatcher::new(state));
        let service = test_service(mock.clone());

        let h1 = proto_ipc_handle(3);
        let h2 = proto_ipc_handle(4);
        let h3 = proto_ipc_handle(5);
        for h in [&h1, &h2, &h3] {
            let key = handle_key(h);
            service
                .ipc_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(key, cache_entry(1));
        }

        let request = BatchLookupRequest {
            entries: vec![
                proto::LookupEntry {
                    key: 21,
                    ipc_handle: Some(h1),
                },
                proto::LookupEntry {
                    key: 22,
                    ipc_handle: Some(h2),
                },
                proto::LookupEntry {
                    key: 23,
                    ipc_handle: Some(h3),
                },
            ],
        };
        let response = service
            .lookup(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.results.len(), 3);
        assert!(response.results[0].success);
        assert!(!response.results[1].success);
        assert_eq!(
            response.results[1].error_code,
            ErrorCode::KeyNotFound as i32
        );
        assert!(!response.results[2].success);
        assert_eq!(response.results[2].error_code, ErrorCode::Timeout as i32);
        assert_eq!(
            mock.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .batch_lookup_calls,
            vec![vec![21, 22, 23]]
        );
    }

    #[tokio::test]
    async fn check_returns_per_key_results_in_order() {
        let mut state = MockDispatcherState::default();
        state.check_results.insert(31, Ok(true));
        state.check_results.insert(32, Ok(true));
        state.check_results.insert(33, Ok(false));
        state
            .check_results
            .insert(34, Err(DispatcherError::Timeout("pending".to_string())));
        let mock = Arc::new(MockDispatcher::new(state));
        let service = test_service(mock.clone());

        let request = BatchCheckRequest {
            keys: vec![31, 32, 33, 34],
        };
        let response = service
            .check(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            response
                .results
                .iter()
                .map(|r| (r.key, r.exists))
                .collect::<Vec<_>>(),
            vec![(31, true), (32, true), (33, false), (34, false)]
        );
        assert_eq!(
            mock.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .check_calls,
            vec![31, 32, 33, 34]
        );
    }

    #[tokio::test]
    async fn remove_supports_partial_failure() {
        let mut state = MockDispatcherState::default();
        state
            .remove_results
            .insert(42, Err(DispatcherError::KeyNotFound(42)));
        let mock = Arc::new(MockDispatcher::new(state));
        let service = test_service(mock);

        let request = BatchRemoveRequest {
            keys: vec![41, 42, 43],
        };
        let response = service
            .remove(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.results.len(), 3);
        assert!(response.results[0].success);
        assert!(!response.results[1].success);
        assert_eq!(
            response.results[1].error_code,
            ErrorCode::KeyNotFound as i32
        );
        assert!(response.results[2].success);
    }

    #[tokio::test]
    async fn touch_supports_partial_failure() {
        let mut state = MockDispatcherState::default();
        state
            .touch_results
            .insert(52, Err(DispatcherError::KeyNotFound(52)));
        let mock = Arc::new(MockDispatcher::new(state));
        let service = test_service(mock);

        let request = BatchTouchRequest {
            keys: vec![51, 52],
            promote: false,
        };
        let response = service
            .touch(Request::new(request))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.results.len(), 2);
        assert!(response.results[0].success);
        assert!(!response.results[1].success);
        assert_eq!(
            response.results[1].error_code,
            ErrorCode::KeyNotFound as i32
        );
    }

    #[test]
    fn ipc_cache_refcount_close_keeps_handle_open_when_still_referenced() {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let key = [7u8; 64];
        cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, cache_entry(1));

        let ptr = ipc_cache_open(&cache, &key, -1).unwrap();
        assert!(ptr.is_null());
        assert_eq!(
            cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .unwrap()
                .refcount,
            2
        );

        ipc_cache_close(&cache, &key);
        assert_eq!(
            cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&key)
                .unwrap()
                .refcount,
            1
        );
    }

    #[tokio::test]
    async fn clear_memory_tier_returns_entries_cleared() {
        let state = MockDispatcherState {
            clear_memory_tier_result: Ok(17),
            ..Default::default()
        };
        let mock = Arc::new(MockDispatcher::new(state));
        let service = test_service(mock);

        let response = service
            .clear_memory_tier(Request::new(ClearMemoryTierRequest {}))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(response.entries_cleared, 17);
    }

    #[tokio::test]
    async fn clear_memory_tier_maps_not_initialized_error() {
        let state = MockDispatcherState {
            clear_memory_tier_result: Err(DispatcherError::NotInitialized(
                "dispatcher not initialized".to_string(),
            )),
            ..Default::default()
        };
        let mock = Arc::new(MockDispatcher::new(state));
        let service = test_service(mock);

        let err = service
            .clear_memory_tier(Request::new(ClearMemoryTierRequest {}))
            .await
            .unwrap_err();

        assert_eq!(err.code(), tonic::Code::Internal);
        assert!(err.message().contains("clear_memory_tier failed"));
        assert!(err.message().contains("not initialized"));
    }
}
