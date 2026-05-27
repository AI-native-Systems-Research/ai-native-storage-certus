// === FILE: service.rs (full file, 329 lines) ===
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

// === FILE: lib.rs (EVOLVE-BLOCK sections — lines 60-83, 190-289, 605-661, 801-912) ===
// The following blocks are the ONLY parts of lib.rs you can modify.
// Each block is replaced into lib.rs at its original location.
// Code between blocks is unchanged and not shown.

// --- lib.rs lines 60-83 ---
// ===== EVOLVE-BLOCK: COMPONENT_FIELDS =====
define_component! {
    pub DispatcherComponent {
        version: "0.1.0",
        provides: [IDispatcher],
        receptacles: {
            logger: ILogger,
            dispatch_map: IDispatchMap,
            gpu_services: IGpuServices,
            spdk_env: ISPDKEnv,
            memory_tier: IMemoryTier,
        },
        fields: {
            initialized: AtomicBool,
            bg_writer: Mutex<Option<BackgroundWriter>>,
            bg_evictor: Mutex<Option<BackgroundEvictor>>,
            data_drives: Mutex<Vec<DataDrive>>,
            pending_writes: Mutex<HashMap<CacheKey, PendingWrite>>,
            pipeline_ring: Mutex<Option<pipeline::PipelineRing>>,
            warm_stream: AtomicU64,
        },
    }
}
// ===== END EVOLVE-BLOCK: COMPONENT_FIELDS =====

// --- lib.rs lines 190-289 ---
    // ===== EVOLVE-BLOCK: PROMOTE_AND_SERVE =====
    /// Promote an SSD-resident entry back into the memory-tier and serve to GPU.
    ///
    /// Uses pipelined chunked reads: SSD→DRAM (memory-tier) while streaming
    /// chunks from DRAM→GPU.
    fn promote_and_serve(
        &self,
        key: CacheKey,
        offset: u64,
        ipc_handle: &IpcHandle,
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        mt: &Arc<dyn IMemoryTier + Send + Sync>,
    ) -> Result<(), DispatcherError> {
        let total_bytes = ipc_handle.size as usize;

        // Evict if needed to make space.
        Self::evict_for_space(dm, mt, ipc_handle.size)?;

        // Insert into memory-tier.
        let mem_ptr = mt.insert(key, ipc_handle.size).map_err(|e| {
            DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
        })?;

        // Read from SSD into memory-tier using pipelined reader.
        let drives = self.data_drives.lock().unwrap();
        if drives.is_empty() {
            // No hardware: just copy zeros to GPU (test/staging-only mode).
            let aligned = total_bytes.next_multiple_of(4096).max(4096);
            let temp_buf = unsafe {
                DmaBuffer::from_raw(mem_ptr as *mut std::ffi::c_void, aligned, noop_free, -1)
            }
            .map_err(|e| DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}")))?;
            let result = gpu.dma_copy_to_device(
                &temp_buf,
                ipc_handle.address as *mut std::ffi::c_void,
                total_bytes,
            );
            std::mem::forget(temp_buf);
            // Register promoted entry in dispatch-map.
            let _ = dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size);
            let _ = dm.release_write(key);
            return result.map_err(|e| {
                DispatcherError::IoError(format!("GPU DMA copy (promote) failed: {e}"))
            });
        }

        let idx = Self::drive_index(key, drives.len());
        let drive = &drives[idx];
        let block_size = drive.block_dev_iface.block_size();
        let start_lba = offset / block_size as u64;
        let block_dev = Arc::clone(&drive.block_dev_iface);

        // Use cached channels if available, otherwise create new ones.
        let channels = match &drive.cached_channels {
            Some(ch) => ch,
            None => {
                drop(drives);
                return Err(DispatcherError::IoError(
                    "no cached channels for drive".into(),
                ));
            }
        };

        // Zero-copy pipelined reader: NVMe → memory-tier slot → GPU (no intermediate ring copy).
        // SAFETY: mem_ptr is a valid, CUDA-pinned, SPDK-registered memory-tier slot.
        // ipc_handle.address is a valid GPU destination pointer.
        let ring_guard = self.pipeline_ring.lock().unwrap();
        let ring_ref = ring_guard.as_ref().ok_or_else(|| {
            DispatcherError::NotInitialized("pipeline ring not allocated".into())
        })?;
        unsafe {
            pipeline::pipelined_ssd_to_gpu_zero_copy(
                &*block_dev,
                &**gpu,
                &ring_ref.streams,
                channels,
                mem_ptr,
                ipc_handle.address as *mut std::ffi::c_void,
                start_lba,
                total_bytes,
                ring_ref.chunk_size,
            )?;
        }
        drop(ring_guard);
        drop(drives);

        // Update dispatch-map: remove old BlockDevice entry and create fresh MemoryTier.
        // Since we released the read ref before calling this method, we can remove
        // and re-register.
        let _ = dm.remove(key);
        dm.create_memory_tier_entry(key, mem_ptr, ipc_handle.size)
            .map_err(|e| DispatcherError::IoError(format!("promote re-register failed: {e}")))?;
        // Set the ssd_offset since data is still on SSD.
        let _ = dm.convert_to_storage(key, offset);
        let _ = dm.release_write(key);

        Ok(())
    }
    // ===== END EVOLVE-BLOCK: PROMOTE_AND_SERVE =====

// --- lib.rs lines 605-661 ---
            // ===== EVOLVE-BLOCK: PIPELINE_INIT =====
            // Pre-allocate pipeline ring for promote_and_serve (CUDA-pinned + SPDK-registered).
            if let Ok(gpu) = self.gpu_services.get() {
                let chunk_size = self
                    .data_drives
                    .lock()
                    .unwrap()
                    .first()
                    .map(|d| d.block_dev_iface.max_transfer_size() as usize)
                    .unwrap_or(131072);
                match pipeline::PipelineRing::new(&*gpu, chunk_size) {
                    Ok(ring) => {
                        *self.pipeline_ring.lock().unwrap() = Some(ring);
                    }
                    Err(e) => {
                        self.log_info(&format!(
                            "pipeline ring allocation failed (non-fatal): {e:?}"
                        ));
                    }
                }

                // Dedicated CUDA stream for warm-path DMA (avoids pipeline_ring lock).
                match gpu.create_stream() {
                    Ok(stream) => {
                        self.warm_stream.store(stream.0 as u64, Ordering::Release);
                    }
                    Err(e) => {
                        self.log_info(&format!(
                            "warm stream allocation failed (non-fatal): {e}"
                        ));
                    }
                }

                // Register memory-tier pool as CUDA-pinned + SPDK DMA-capable
                // for zero-copy NVMe reads and async GPU transfers.
                if let Ok(mt) = self.memory_tier.get() {
                    if let Some((pool_ptr, pool_size)) = mt.pool_info() {
                        match gpu.register_host_memory(
                            pool_ptr as *mut std::ffi::c_void,
                            pool_size,
                        ) {
                            Ok(()) => {
                                self.log_info(&format!(
                                    "dispatcher: registered memory-tier pool ({} MiB) for zero-copy DMA",
                                    pool_size / (1024 * 1024)
                                ));
                            }
                            Err(e) => {
                                self.log_info(&format!(
                                    "memory-tier pool registration failed (non-fatal): {e}"
                                ));
                            }
                        }
                    }
                }
            }
            // ===== END EVOLVE-BLOCK: PIPELINE_INIT =====

// --- lib.rs lines 801-912 ---
    // ===== EVOLVE-BLOCK: LOOKUP_ASYNC =====
    fn lookup_async(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<GpuStream, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let mt = self
            .memory_tier
            .get()
            .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

        let result = dm.lookup(key);

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        let null_stream = GpuStream(std::ptr::null_mut());

        match result {
            Ok(lookup_result) => {
                use interfaces::LookupResult;
                match lookup_result {
                    LookupResult::NotExist => Err(DispatcherError::KeyNotFound(key)),
                    LookupResult::MismatchSize => {
                        let _ = dm.release_read(key);
                        Err(DispatcherError::InvalidParameter(
                            "size mismatch on lookup".into(),
                        ))
                    }
                    LookupResult::MemoryTier { pointer, size } => {
                        let copy_size = (ipc_handle.size as usize).min(size as usize);

                        // Use dedicated warm stream (lock-free AtomicU64 load).
                        let raw = self.warm_stream.load(Ordering::Acquire);
                        if raw != 0 {
                            let s = GpuStream(raw as *mut std::ffi::c_void);
                            gpu.memcpy_h2d_async(
                                pointer as *const std::ffi::c_void,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                                s,
                            )
                            .map_err(|e| {
                                let _ = dm.release_read(key);
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })?;
                            let _ = dm.release_read(key);
                            mt.touch(key);
                            Ok(s)
                        } else {
                            // Fallback: sync copy via DmaBuffer wrapper.
                            let aligned = copy_size.next_multiple_of(4096).max(4096);
                            let temp_buf = unsafe {
                                DmaBuffer::from_raw(
                                    pointer as *mut std::ffi::c_void,
                                    aligned,
                                    noop_free,
                                    -1,
                                )
                            }
                            .map_err(|e| {
                                let _ = dm.release_read(key);
                                DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}"))
                            })?;
                            let copy_result = gpu.dma_copy_to_device(
                                &temp_buf,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                            );
                            std::mem::forget(temp_buf);
                            let _ = dm.release_read(key);
                            mt.touch(key);
                            copy_result.map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })?;
                            Ok(null_stream)
                        }
                    }
                    LookupResult::Staging { buffer } => {
                        let copy_result = gpu.dma_copy_to_device(
                            &buffer,
                            ipc_handle.address as *mut std::ffi::c_void,
                            ipc_handle.size as usize,
                        );
                        let _ = dm.release_read(key);
                        copy_result.map_err(|e| {
                            DispatcherError::IoError(format!(
                                "GPU DMA copy (staging→device) failed: {e}"
                            ))
                        })?;
                        Ok(null_stream)
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        self.promote_and_serve(key, offset, &ipc_handle, &gpu, &dm, &mt)?;
                        Ok(null_stream)
                    }
                }
            }
            Err(_) => Err(DispatcherError::KeyNotFound(key)),
        }
    }
    // ===== END EVOLVE-BLOCK: LOOKUP_ASYNC =====

// === FILE: pipeline.rs (full file) ===
// Ring-buffer pipelined reader for SSD->DRAM->GPU transfers.

use std::sync::{Arc, Mutex};

use interfaces::{
    ClientChannels, Command, Completion, DmaBuffer, DispatcherError, GpuStream, IBlockDevice,
    IGpuServices,
};

use crate::io_segmenter;

// ===== EVOLVE-BLOCK: CONSTANTS =====
// These constants control pipeline behavior. Evolution may change values
// or add new constants (e.g., adaptive thresholds).

/// Number of ring buffers for pipelined transfers.
pub const PIPELINE_RING_SIZE: usize = 8;

/// Timeout for async NVMe read operations (ms).
const READ_TIMEOUT_MS: u64 = 5000;

/// Queue depth for zero-copy pipeline (max concurrent NVMe reads).
/// Higher values increase device utilization but consume more memory-tier bandwidth.
/// Current: 16. Raw NVMe at QD=32 gives 41% more throughput than QD=16.
const ZERO_COPY_DEPTH: usize = 32;

/// How often to synchronize CUDA streams (every N completions).
/// Lower values reduce GPU command queue depth but add sync overhead.
/// Current: 16. At 32 chunks (4 MiB), this means 2 syncs per transfer.
const SYNC_FREQUENCY: usize = 16;

// ===== END EVOLVE-BLOCK: CONSTANTS =====

/// Pre-allocated ring of CUDA-pinned + SPDK-registered DMA buffers and CUDA streams.
pub struct PipelineRing {
    pub buffers: Vec<Arc<Mutex<DmaBuffer>>>,
    pub streams: [GpuStream; 2],
    pub chunk_size: usize,
}

impl PipelineRing {
    pub fn new(gpu: &dyn IGpuServices, chunk_size: usize) -> Result<Self, DispatcherError> {
        let buffers: Vec<Arc<Mutex<DmaBuffer>>> = (0..PIPELINE_RING_SIZE)
            .map(|_| {
                gpu.allocate_pinned_dma_buffer(chunk_size)
                    .map(|b| Arc::new(Mutex::new(b)))
                    .map_err(|e| {
                        DispatcherError::AllocationFailed(format!("pipeline ring buffer: {e}"))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let stream_a = gpu
            .create_stream()
            .map_err(|e| DispatcherError::IoError(format!("create_stream failed: {e}")))?;
        let stream_b = gpu.create_stream().map_err(|e| {
            let _ = gpu.destroy_stream(stream_a);
            DispatcherError::IoError(format!("create_stream failed: {e}"))
        })?;

        Ok(Self {
            buffers,
            streams: [stream_a, stream_b],
            chunk_size,
        })
    }

    pub fn destroy(self, gpu: &dyn IGpuServices) {
        let _ = gpu.destroy_stream(self.streams[0]);
        let _ = gpu.destroy_stream(self.streams[1]);
    }
}

/// Pipeline-read from SSD into a memory-tier slot while streaming chunks to GPU.
pub unsafe fn pipelined_ssd_to_gpu(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    ring: &PipelineRing,
    mem_tier_ptr: *mut u8,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
) -> Result<(), DispatcherError> {
    let block_size = drive.block_size() as usize;
    let chunk_size = ring.chunk_size;
    let aligned_bytes = total_bytes.next_multiple_of(block_size);

    let channels: ClientChannels = drive
        .connect_client()
        .map_err(|e| DispatcherError::IoError(format!("connect_client failed: {e}")))?;

    let segments = io_segmenter::segment_io(
        start_lba,
        aligned_bytes,
        chunk_size as u32,
        block_size as u32,
    );

    if segments.is_empty() {
        return Ok(());
    }

    let num_chunks = segments.len();
    let ring_size = ring.buffers.len().min(num_chunks);
    let streams = &ring.streams;

    let prime_count = ring_size.min(num_chunks);
    for i in 0..prime_count {
        let slot = i % ring_size;
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[i].lba,
                buf: Arc::clone(&ring.buffers[slot]),
                timeout_ms: READ_TIMEOUT_MS,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync send #{i}: {e}")))?;
    }

    let mut next_to_submit = prime_count;

    for completed in 0..num_chunks {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { result, .. }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read #{completed}: {e}"))
                })?;
            }
            Ok(Completion::Timeout { handle }) => {
                return Err(DispatcherError::IoError(format!(
                    "NVMe read timeout (handle {:?})", handle
                )));
            }
            Ok(other) => {
                return Err(DispatcherError::IoError(format!(
                    "unexpected completion: {other:?}"
                )));
            }
            Err(_) => {
                return Err(DispatcherError::IoError(
                    "completion channel disconnected".into(),
                ));
            }
        }

        let slot = completed % ring_size;
        let seg = &segments[completed];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let current_stream = streams[completed % 2];

        let guard = ring.buffers[slot].lock().unwrap();

        if copy_len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    guard.as_ptr() as *const u8,
                    mem_tier_ptr.add(seg.buffer_offset),
                    copy_len,
                );
            }
        }

        gpu.dma_copy_to_device_async(
            &guard,
            unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
            copy_len,
            current_stream,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU async DMA copy #{completed} failed: {e}"))
        })?;

        if (completed + 1) % ring_size == 0 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }

        drop(guard);

        if next_to_submit < num_chunks {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[next_to_submit].lba,
                    buf: Arc::clone(&ring.buffers[slot]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "ReadAsync resubmit #{next_to_submit}: {e}"
                    ))
                })?;
            next_to_submit += 1;
        }
    }

    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
    }

    Ok(())
}

/// No-op free function for DmaBuffer wrappers over memory-tier regions.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

// ===== EVOLVE-BLOCK: ZERO_COPY_PIPELINE =====
// This is the primary evolution target. The function reads from NVMe directly
// into a memory-tier slot (no intermediate buffer), then streams to GPU.
//
// Key tunables within this function:
// - ZERO_COPY_DEPTH: how many NVMe reads in flight (higher = more device utilization)
// - SYNC_FREQUENCY: how often to sync CUDA streams (tradeoff: queue depth vs overhead)
// - Stream assignment: currently round-robin (completed % 2)
// - Completion processing: currently sequential
//
// What evolution might discover:
// - Higher ZERO_COPY_DEPTH (32-64) to match raw NVMe QD=32 performance
// - Adaptive sync frequency based on num_chunks (sync at end for small transfers)
// - More CUDA streams (3-4) for better GPU DMA overlap
// - Batched completion processing (drain multiple completions before issuing DMAs)
// - Size-adaptive strategy (different logic for 8-chunk vs 128-chunk transfers)

/// Zero-copy pipeline: read from SSD directly into a memory-tier slot, stream to GPU.
pub unsafe fn pipelined_ssd_to_gpu_zero_copy(
    drive: &dyn IBlockDevice,
    gpu: &dyn IGpuServices,
    streams: &[GpuStream; 2],
    channels: &ClientChannels,
    mem_tier_ptr: *mut u8,
    gpu_dst: *mut std::ffi::c_void,
    start_lba: u64,
    total_bytes: usize,
    chunk_size: usize,
) -> Result<(), DispatcherError> {
    let block_size = drive.block_size() as usize;
    let aligned_bytes = total_bytes.next_multiple_of(block_size);

    let segments = io_segmenter::segment_io(
        start_lba,
        aligned_bytes,
        chunk_size as u32,
        block_size as u32,
    );

    if segments.is_empty() {
        return Ok(());
    }

    let num_chunks = segments.len();

    // Create DmaBuffer wrappers for each chunk of the memory-tier slot.
    let chunk_bufs: Vec<Arc<Mutex<DmaBuffer>>> = segments
        .iter()
        .map(|seg| {
            let ptr = unsafe { mem_tier_ptr.add(seg.buffer_offset) as *mut std::ffi::c_void };
            let buf_size = seg.length.next_multiple_of(block_size);
            let buf = unsafe { DmaBuffer::from_raw(ptr, buf_size, noop_free, -1) }
                .map_err(|e| {
                    DispatcherError::AllocationFailed(format!("DmaBuffer wrap chunk: {e}"))
                })?;
            Ok(Arc::new(Mutex::new(buf)))
        })
        .collect::<Result<Vec<_>, DispatcherError>>()?;

    // Prime: submit initial async reads directly into memory-tier chunk offsets.
    let max_inflight = ZERO_COPY_DEPTH.min(num_chunks);
    for i in 0..max_inflight {
        channels
            .command_tx
            .send(Command::ReadAsync {
                ns_id: 1,
                lba: segments[i].lba,
                buf: Arc::clone(&chunk_bufs[i]),
                timeout_ms: READ_TIMEOUT_MS,
            })
            .map_err(|e| DispatcherError::IoError(format!("ReadAsync send #{i}: {e}")))?;
    }

    let mut next_to_submit = max_inflight;

    // Process completions: after each NVMe read completes into memory-tier,
    // issue async H2D from the same memory-tier offset to GPU.
    for completed in 0..num_chunks {
        match channels.completion_rx.recv() {
            Ok(Completion::ReadDone { result, .. }) => {
                result.map_err(|e| {
                    DispatcherError::IoError(format!("SSD read #{completed}: {e}"))
                })?;
            }
            Ok(Completion::Timeout { handle }) => {
                return Err(DispatcherError::IoError(format!(
                    "NVMe read timeout (handle {:?})", handle
                )));
            }
            Ok(other) => {
                return Err(DispatcherError::IoError(format!(
                    "unexpected completion: {other:?}"
                )));
            }
            Err(_) => {
                return Err(DispatcherError::IoError(
                    "completion channel disconnected".into(),
                ));
            }
        }

        let seg = &segments[completed];
        let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
        let current_stream = streams[completed % 2];

        // Async H2D: memory-tier chunk -> GPU (same memory NVMe just wrote into).
        let guard = chunk_bufs[completed].lock().unwrap();
        gpu.dma_copy_to_device_async(
            &guard,
            unsafe { (gpu_dst as *mut u8).add(seg.buffer_offset) as *mut std::ffi::c_void },
            copy_len,
            current_stream,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU async DMA copy #{completed} failed: {e}"))
        })?;
        drop(guard);

        // Batch-sync both streams periodically to throttle GPU command queue depth.
        if (completed + 1) % SYNC_FREQUENCY == 0 {
            gpu.stream_synchronize(streams[0])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
            gpu.stream_synchronize(streams[1])
                .map_err(|e| DispatcherError::IoError(format!("stream_synchronize failed: {e}")))?;
        }

        // Submit next NVMe read (into the next memory-tier chunk).
        if next_to_submit < num_chunks {
            channels
                .command_tx
                .send(Command::ReadAsync {
                    ns_id: 1,
                    lba: segments[next_to_submit].lba,
                    buf: Arc::clone(&chunk_bufs[next_to_submit]),
                    timeout_ms: READ_TIMEOUT_MS,
                })
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "ReadAsync resubmit #{next_to_submit}: {e}"
                    ))
                })?;
            next_to_submit += 1;
        }
    }

    // Sync both streams to ensure all GPU copies are complete.
    for s in streams {
        gpu.stream_synchronize(*s)
            .map_err(|e| DispatcherError::IoError(format!("final stream_synchronize: {e}")))?;
    }

    // Forget all DmaBuffer wrappers (noop_free, but avoid double-free logic).
    for buf in chunk_bufs {
        std::mem::forget(Arc::try_unwrap(buf).ok());
    }

    Ok(())
}

// ===== END EVOLVE-BLOCK: ZERO_COPY_PIPELINE =====

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_ring_size_is_reasonable() {
        let size = PIPELINE_RING_SIZE;
        assert!(size >= 2);
        assert!(size <= 64);
    }
}