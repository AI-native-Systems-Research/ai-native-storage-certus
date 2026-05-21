//! gRPC service implementation for the Certus Dispatcher.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tonic::{Request, Response, Status};

use gpu_services::cuda_ffi;
use interfaces::{DispatcherError, IDispatcher, IpcHandle};

pub mod proto {
    tonic::include_proto!("certus.dispatcher.v1");
}

use proto::dispatcher_server::{Dispatcher, DispatcherServer};
use proto::{
    BatchCheckRequest, BatchCheckResponse, BatchLookupRequest, BatchLookupResponse,
    BatchPopulateRequest, BatchPopulateResponse, BatchRemoveRequest, BatchRemoveResponse,
    BatchTouchRequest, BatchTouchResponse, CheckResult, EntryResult, ErrorCode,
};

pub fn dispatcher_server(svc: DispatcherService) -> DispatcherServer<DispatcherService> {
    DispatcherServer::new(svc)
}

pub struct DispatcherService {
    dispatcher: Arc<Mutex<Arc<dyn IDispatcher + Send + Sync>>>,
}

impl DispatcherService {
    pub fn new(dispatcher: Arc<Mutex<Arc<dyn IDispatcher + Send + Sync>>>) -> Self {
        Self { dispatcher }
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

fn open_cuda_ipc(handle_bytes: &[u8]) -> Result<*mut std::ffi::c_void, String> {
    if handle_bytes.len() != 64 {
        return Err(format!(
            "cuda_ipc_handle must be 64 bytes, got {}",
            handle_bytes.len()
        ));
    }
    let mut reserved = [0u8; 64];
    reserved.copy_from_slice(handle_bytes);
    let cuda_handle = cuda_ffi::cudaIpcMemHandle_t { reserved };

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
    Ok(dev_ptr)
}

fn close_cuda_ipc(dev_ptr: *mut std::ffi::c_void) {
    unsafe {
        cuda_ffi::cudaIpcCloseMemHandle(dev_ptr);
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
        let results = tokio::task::spawn_blocking(move || {
            let disp = dispatcher.lock().unwrap();
            req.entries
                .iter()
                .map(|entry| {
                    let handle = match entry.ipc_handle.as_ref() {
                        Some(h) => h,
                        None => {
                            return error_result(
                                entry.key,
                                &DispatcherError::InvalidParameter(
                                    "missing ipc_handle".into(),
                                ),
                            );
                        }
                    };
                    let dev_ptr = match open_cuda_ipc(&handle.cuda_ipc_handle) {
                        Ok(ptr) => ptr,
                        Err(e) => {
                            return error_result(
                                entry.key,
                                &DispatcherError::IoError(format!("IPC open failed: {e}")),
                            );
                        }
                    };
                    let ipc = IpcHandle {
                        address: dev_ptr as *mut u8,
                        size: handle.size,
                    };
                    let result = match disp.populate(entry.key, ipc) {
                        Ok(()) => success_result(entry.key),
                        Err(e) => error_result(entry.key, &e),
                    };
                    close_cuda_ipc(dev_ptr);
                    result
                })
                .collect::<Vec<_>>()
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
        let results = tokio::task::spawn_blocking(move || {
            let disp = dispatcher.lock().unwrap();
            // Cache opened IPC handles within the batch to avoid repeated
            // cudaIpcOpenMemHandle/Close for entries sharing the same handle.
            let mut ipc_cache: HashMap<[u8; 64], *mut std::ffi::c_void> = HashMap::new();

            let results: Vec<_> = req.entries
                .iter()
                .map(|entry| {
                    let handle = match entry.ipc_handle.as_ref() {
                        Some(h) => h,
                        None => {
                            return error_result(
                                entry.key,
                                &DispatcherError::InvalidParameter(
                                    "missing ipc_handle".into(),
                                ),
                            );
                        }
                    };
                    let handle_key: [u8; 64] = match handle.cuda_ipc_handle.as_slice().try_into() {
                        Ok(k) => k,
                        Err(_) => {
                            return error_result(
                                entry.key,
                                &DispatcherError::InvalidParameter(
                                    format!("cuda_ipc_handle must be 64 bytes, got {}", handle.cuda_ipc_handle.len()),
                                ),
                            );
                        }
                    };
                    let dev_ptr = match ipc_cache.get(&handle_key) {
                        Some(&ptr) => ptr,
                        None => {
                            match open_cuda_ipc(&handle.cuda_ipc_handle) {
                                Ok(ptr) => {
                                    ipc_cache.insert(handle_key, ptr);
                                    ptr
                                }
                                Err(e) => {
                                    return error_result(
                                        entry.key,
                                        &DispatcherError::IoError(format!("IPC open failed: {e}")),
                                    );
                                }
                            }
                        }
                    };
                    let ipc = IpcHandle {
                        address: dev_ptr as *mut u8,
                        size: handle.size,
                    };
                    match disp.lookup(entry.key, ipc) {
                        Ok(()) => success_result(entry.key),
                        Err(e) => error_result(entry.key, &e),
                    }
                })
                .collect();

            // Close all cached IPC handles once at the end of the batch.
            for &ptr in ipc_cache.values() {
                close_cuda_ipc(ptr);
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
            let disp = dispatcher.lock().unwrap();
            req.keys
                .iter()
                .map(|&key| {
                    let exists: bool = disp.check(key).unwrap_or_default();
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
            let disp = dispatcher.lock().unwrap();
            req.keys
                .iter()
                .map(|&key| match disp.remove(key) {
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
        let results = tokio::task::spawn_blocking(move || {
            let disp = dispatcher.lock().unwrap();
            req.keys
                .iter()
                .map(|&key| match disp.touch(key) {
                    Ok(()) => success_result(key),
                    Err(e) => error_result(key, &e),
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| Status::internal(format!("task join error: {e}")))?;

        Ok(Response::new(BatchTouchResponse { results }))
    }
}
