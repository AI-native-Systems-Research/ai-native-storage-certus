//! gRPC service implementation for the filesystem-backed Certus server.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tonic::{Request, Response, Status};

use gpu_services::cuda_ffi;

use crate::memory_tier::MemoryTier;
use crate::storage::FsStorage;

pub mod proto {
    tonic::include_proto!("certus.dispatcher.v1");
}

use proto::dispatcher_server::{Dispatcher, DispatcherServer};
use proto::{
    BatchCheckRequest, BatchCheckResponse, BatchLookupRequest, BatchLookupResponse,
    BatchPopulateRequest, BatchPopulateResponse, BatchRemoveRequest, BatchRemoveResponse,
    BatchTouchRequest, BatchTouchResponse, CheckResult, ClearMemoryTierRequest,
    ClearMemoryTierResponse, EntryResult, ErrorCode,
};

pub fn dispatcher_server(svc: FsDispatcherService) -> DispatcherServer<FsDispatcherService> {
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

pub struct FsDispatcherService {
    storage: Arc<FsStorage>,
    memory_tier: Arc<MemoryTier>,
    ipc_cache: IpcCache,
    staging_buf: Arc<StagingBuffer>,
}

struct StagingBuffer {
    ptr: *mut std::ffi::c_void,
    size: usize,
    lock: Mutex<()>,
}

// SAFETY: staging buffer is CUDA pinned host memory, access serialized by lock.
unsafe impl Send for StagingBuffer {}
unsafe impl Sync for StagingBuffer {}

impl StagingBuffer {
    fn new(size: usize) -> Self {
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let err = unsafe { cuda_ffi::cudaHostAlloc(&mut ptr, size, cuda_ffi::CUDA_HOST_ALLOC_DEFAULT) };
        if err != cuda_ffi::CUDA_SUCCESS || ptr.is_null() {
            panic!(
                "cudaHostAlloc({} bytes) failed: {}",
                size,
                cuda_ffi::cuda_error_string(err)
            );
        }
        Self {
            ptr,
            size,
            lock: Mutex::new(()),
        }
    }
}

impl Drop for StagingBuffer {
    fn drop(&mut self) {
        unsafe {
            cuda_ffi::cudaFreeHost(self.ptr);
        }
    }
}

impl FsDispatcherService {
    pub fn new(storage: Arc<FsStorage>, memory_tier: Arc<MemoryTier>, staging_size: usize) -> Self {
        Self {
            storage,
            memory_tier,
            ipc_cache: Arc::new(Mutex::new(HashMap::new())),
            staging_buf: Arc::new(StagingBuffer::new(staging_size)),
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

fn success_result(key: u64) -> EntryResult {
    EntryResult {
        key,
        success: true,
        error_code: ErrorCode::Unspecified.into(),
        error_message: String::new(),
    }
}

fn error_result(key: u64, code: ErrorCode, msg: String) -> EntryResult {
    EntryResult {
        key,
        success: false,
        error_code: code.into(),
        error_message: msg,
    }
}

#[tonic::async_trait]
impl Dispatcher for FsDispatcherService {
    async fn populate(
        &self,
        request: Request<BatchPopulateRequest>,
    ) -> Result<Response<BatchPopulateResponse>, Status> {
        let req = request.into_inner();
        let keys: Vec<u64> = req.entries.iter().map(|e| e.key).collect();
        check_duplicate_keys(&keys)?;

        let cache = Arc::clone(&self.ipc_cache);
        let storage = Arc::clone(&self.storage);
        let memory_tier = Arc::clone(&self.memory_tier);
        let staging = Arc::clone(&self.staging_buf);

        let results = tokio::task::spawn_blocking(move || {
            let mut opened_keys: Vec<[u8; 64]> = Vec::new();
            let mut results: Vec<EntryResult> = Vec::with_capacity(req.entries.len());

            for entry in &req.entries {
                let handle = match entry.ipc_handle.as_ref() {
                    Some(h) => h,
                    None => {
                        results.push(error_result(
                            entry.key,
                            ErrorCode::InvalidParameter,
                            "missing ipc_handle".into(),
                        ));
                        continue;
                    }
                };
                let handle_key: [u8; 64] = match handle.cuda_ipc_handle.as_slice().try_into() {
                    Ok(k) => k,
                    Err(_) => {
                        results.push(error_result(
                            entry.key,
                            ErrorCode::InvalidParameter,
                            format!(
                                "cuda_ipc_handle must be 64 bytes, got {}",
                                handle.cuda_ipc_handle.len()
                            ),
                        ));
                        continue;
                    }
                };

                if storage.exists(entry.key) {
                    results.push(error_result(
                        entry.key,
                        ErrorCode::AlreadyExists,
                        format!("key already exists: {}", entry.key),
                    ));
                    continue;
                }

                let dev_ptr = match ipc_cache_open(&cache, &handle_key, handle.gpu_device_id) {
                    Ok(ptr) => {
                        opened_keys.push(handle_key);
                        ptr
                    }
                    Err(e) => {
                        results.push(error_result(
                            entry.key,
                            ErrorCode::IoError,
                            format!("IPC open failed: {e}"),
                        ));
                        continue;
                    }
                };

                let size = handle.size as usize;
                let _lock = staging.lock.lock().unwrap();

                if size > staging.size {
                    results.push(error_result(
                        entry.key,
                        ErrorCode::InvalidParameter,
                        format!("data size {} exceeds staging buffer {}", size, staging.size),
                    ));
                    continue;
                }

                // cudaMemcpy: GPU device → host staging buffer
                let err = unsafe {
                    cuda_ffi::cudaMemcpy(
                        staging.ptr,
                        dev_ptr as *const std::ffi::c_void,
                        size,
                        cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
                    )
                };
                if err != cuda_ffi::CUDA_SUCCESS {
                    results.push(error_result(
                        entry.key,
                        ErrorCode::IoError,
                        format!("cudaMemcpy D2H failed: {}", cuda_ffi::cuda_error_string(err)),
                    ));
                    continue;
                }

                let data =
                    unsafe { std::slice::from_raw_parts(staging.ptr as *const u8, size) }.to_vec();

                // Write to filesystem
                match storage.write(entry.key, &data) {
                    Ok(()) => {}
                    Err(e) => {
                        results.push(error_result(
                            entry.key,
                            ErrorCode::IoError,
                            format!("filesystem write failed: {e}"),
                        ));
                        continue;
                    }
                }

                // Also insert into memory tier
                memory_tier.insert(entry.key, data);

                results.push(success_result(entry.key));
            }

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

        let cache = Arc::clone(&self.ipc_cache);
        let storage = Arc::clone(&self.storage);
        let memory_tier = Arc::clone(&self.memory_tier);
        let staging = Arc::clone(&self.staging_buf);

        let results = tokio::task::spawn_blocking(move || {
            let mut opened_keys: Vec<[u8; 64]> = Vec::new();
            let mut results: Vec<EntryResult> = Vec::with_capacity(req.entries.len());

            for entry in &req.entries {
                let handle = match entry.ipc_handle.as_ref() {
                    Some(h) => h,
                    None => {
                        results.push(error_result(
                            entry.key,
                            ErrorCode::InvalidParameter,
                            "missing ipc_handle".into(),
                        ));
                        continue;
                    }
                };
                let handle_key: [u8; 64] = match handle.cuda_ipc_handle.as_slice().try_into() {
                    Ok(k) => k,
                    Err(_) => {
                        results.push(error_result(
                            entry.key,
                            ErrorCode::InvalidParameter,
                            format!(
                                "cuda_ipc_handle must be 64 bytes, got {}",
                                handle.cuda_ipc_handle.len()
                            ),
                        ));
                        continue;
                    }
                };

                let dev_ptr = match ipc_cache_open(&cache, &handle_key, handle.gpu_device_id) {
                    Ok(ptr) => {
                        opened_keys.push(handle_key);
                        ptr
                    }
                    Err(e) => {
                        results.push(error_result(
                            entry.key,
                            ErrorCode::IoError,
                            format!("IPC open failed: {e}"),
                        ));
                        continue;
                    }
                };

                let size = handle.size as usize;

                // Try memory tier first (hot path)
                let data = if let Some(cached) = memory_tier.get(entry.key) {
                    cached
                } else {
                    // Cold path: read from filesystem
                    match storage.read(entry.key) {
                        Ok(data) => {
                            // Promote to memory tier
                            memory_tier.insert(entry.key, data.clone());
                            data
                        }
                        Err(e) => {
                            results.push(error_result(
                                entry.key,
                                ErrorCode::KeyNotFound,
                                format!("key not found: {}", e),
                            ));
                            continue;
                        }
                    }
                };

                let copy_size = size.min(data.len());
                let _lock = staging.lock.lock().unwrap();

                // Copy data to staging buffer
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        data.as_ptr(),
                        staging.ptr as *mut u8,
                        copy_size,
                    );
                }

                // cudaMemcpy: host staging buffer → GPU device
                let err = unsafe {
                    cuda_ffi::cudaMemcpy(
                        dev_ptr,
                        staging.ptr as *const std::ffi::c_void,
                        copy_size,
                        cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
                    )
                };
                if err != cuda_ffi::CUDA_SUCCESS {
                    results.push(error_result(
                        entry.key,
                        ErrorCode::IoError,
                        format!("cudaMemcpy H2D failed: {}", cuda_ffi::cuda_error_string(err)),
                    ));
                    continue;
                }

                results.push(success_result(entry.key));
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

        let storage = Arc::clone(&self.storage);
        let memory_tier = Arc::clone(&self.memory_tier);

        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| CheckResult {
                    key,
                    exists: memory_tier.contains(key) || storage.exists(key),
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

        let storage = Arc::clone(&self.storage);
        let memory_tier = Arc::clone(&self.memory_tier);

        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| {
                    memory_tier.remove(key);
                    match storage.remove(key) {
                        Ok(()) => success_result(key),
                        Err(_) => error_result(
                            key,
                            ErrorCode::KeyNotFound,
                            format!("key not found: {key}"),
                        ),
                    }
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

        let storage = Arc::clone(&self.storage);
        let memory_tier = Arc::clone(&self.memory_tier);

        let results = tokio::task::spawn_blocking(move || {
            req.keys
                .iter()
                .map(|&key| {
                    if memory_tier.touch(key) || storage.exists(key) {
                        success_result(key)
                    } else {
                        error_result(key, ErrorCode::KeyNotFound, format!("key not found: {key}"))
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchTouchResponse { results }))
    }

    async fn clear_memory_tier(
        &self,
        _request: Request<ClearMemoryTierRequest>,
    ) -> Result<Response<ClearMemoryTierResponse>, Status> {
        let memory_tier = Arc::clone(&self.memory_tier);

        let entries_cleared = tokio::task::spawn_blocking(move || memory_tier.clear())
            .await
            .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(ClearMemoryTierResponse {
            entries_cleared: entries_cleared as u64,
        }))
    }
}
