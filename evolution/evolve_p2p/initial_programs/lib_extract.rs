// --- Section: imports and struct ---
//! Dispatcher component for the Certus storage system.
//!
//! Orchestrates cache operations (populate, lookup, check, remove) using
//! a DRAM memory-tier with LRU eviction and write-through to SSD.
//! Coordinates N data block devices with N extent managers for persistent storage.
//!
//! Provides the [`IDispatcher`] interface with receptacles for
//! [`ILogger`], [`IDispatchMap`], and [`IMemoryTier`].

mod background;
pub mod io_segmenter;
pub mod pipeline;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use component_framework::define_component;
use interfaces::{
    CacheKey, ClientChannels, Command, Completion, DispatcherConfig, DispatcherError, DmaAllocFn,
    DmaBuffer, FormatParams, GpuStream, IBlockDevice, IBlockDeviceAdmin, IDispatchMap, IDispatcher,
    IExtentManager, IGpuServices, ILogger, IMemoryTier, IpcHandle, LookupResult, PciAddress,
    WriteHandle,
};

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::query_interface;
use extent_manager::ExtentManager;
use spdk_env::ISPDKEnv;

use crate::background::{BackgroundEvictor, BackgroundWriter, EvictorConfig, WriteJob};

/// A pending store awaiting commit or cancel.
///
/// Created by `prepare_store` and consumed by either `commit_store` (writes
/// the buffer to SSD and publishes the extent) or `cancel_store` (drops the
/// handle, which auto-aborts the reservation).
struct PendingWrite {
    /// Extent reservation handle; calling `publish()` commits, dropping aborts.
    write_handle: WriteHandle,
    /// DMA buffer the caller writes data into between prepare and commit.
    buffer: Arc<DmaBuffer>,
    /// Original (unaligned) data size in bytes.
    size: u32,
    /// Index into `data_drives` identifying the target SSD.
    drive_idx: usize,
}

/// Holds one (block-device, extent-manager) pair for a data drive.
#[allow(dead_code)]
struct DataDrive {
    _block_dev: Arc<dyn component_core::IUnknown + Send + Sync>,
    block_dev_admin: Arc<dyn IBlockDeviceAdmin + Send + Sync>,
    block_dev_iface: Arc<dyn IBlockDevice + Send + Sync>,
    extent_mgr: Arc<ExtentManager>,
    cached_channels: Option<ClientChannels>,
}

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
            data_drives: RwLock<Vec<DataDrive>>,
            pending_writes: Mutex<HashMap<CacheKey, PendingWrite>>,
            pipeline_ring: RwLock<Option<pipeline::PipelineRing>>,
            warm_stream: AtomicU64,
        },
    }
}

unsafe extern "C" fn libc_free(ptr: *mut std::ffi::c_void) {
    unsafe { libc::free(ptr) };
}

/// No-op free function for temporary DmaBuffer wrappers around memory-tier pointers.
/// The memory-tier component owns the memory; this wrapper must not free it.
unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}

impl DispatcherComponent {
    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    #[allow(dead_code)]

// --- Section: promote_and_serve (single-object path) ---
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
        let drives = self.data_drives.read();
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
        let ring_guard = self.pipeline_ring.read();
        let ring_ref = ring_guard
            .as_ref()
            .ok_or_else(|| DispatcherError::NotInitialized("pipeline ring not allocated".into()))?;
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
                16,
            )?;
        }
        drop(ring_guard);
        drop(drives);

        // Update dispatch-map: remove old BlockDevice entry and create fresh MemoryTier.
        // Since we released the read ref before calling this method, we can remove
        // and re-register.
        let _ = dm.remove(key);

// --- Section: PipelineRing initialization ---
                self.log_info(&format!(
                    "dispatcher: dispatch-map recovered {recovered} extents from disk ({elapsed:.2?})"
                ));
            }

            // Pre-allocate pipeline ring for promote_and_serve (CUDA-pinned + SPDK-registered).
            if let Ok(gpu) = self.gpu_services.get() {
                let chunk_size = {
                    let dd = self.data_drives.read();
                    dd.first()
                        .map(|d| d.block_dev_iface.max_transfer_size() as usize)
                        .unwrap_or(131072)
                };
                match pipeline::PipelineRing::new(&*gpu, chunk_size) {
                    Ok(ring) => {
                        *self.pipeline_ring.write() = Some(ring);
                    }
                    Err(e) => {
                        self.log_info(&format!(
                            "pipeline ring allocation failed (non-fatal): {e:?}"
                        ));

// --- Section: batch_lookup (hot path — queue allocation + pipeline call) ---
    fn batch_lookup(
        &self,
        entries: &[(CacheKey, IpcHandle)],
    ) -> Vec<Result<(), DispatcherError>> {
        if entries.is_empty() {
            return Vec::new();
        }

        let init_check = self.ensure_initialized();
        if let Err(e) = init_check {
            return entries.iter().map(|_| Err(e.clone())).collect();
        }

        let dm = match self.dispatch_map.get() {
            Ok(dm) => dm,
            Err(_) => {
                let e = DispatcherError::NotInitialized("dispatch_map not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };
        let mt = match self.memory_tier.get() {
            Ok(mt) => mt,
            Err(_) => {
                let e = DispatcherError::NotInitialized("memory_tier not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };
        let gpu = match self.gpu_services.get() {
            Ok(gpu) => gpu,
            Err(_) => {
                let e = DispatcherError::NotInitialized("gpu_services not bound".into());
                return entries.iter().map(|_| Err(e.clone())).collect();
            }
        };

        let mut results: Vec<Option<Result<(), DispatcherError>>> = vec![None; entries.len()];

        // Classify entries and handle fast paths inline.
        struct ColdEntry {
            idx: usize,
            key: CacheKey,
            offset: u64,
            ipc_handle_addr: *mut u8,
            ipc_handle_size: u32,
        }
        // SAFETY: ColdEntry contains a raw pointer from IpcHandle (GPU device pointer).
        // These pointers are valid across threads — CUDA IPC handles are designed for
        // cross-process/thread use. We only read the pointer value to pass to CUDA APIs.
        unsafe impl Send for ColdEntry {}
        unsafe impl Sync for ColdEntry {}

        let mut cold_entries: Vec<ColdEntry> = Vec::new();

        for (i, (key, ipc_handle)) in entries.iter().enumerate() {
            let key = *key;
            match dm.lookup(key) {
                Ok(lookup_result) => match lookup_result {
                    LookupResult::NotExist => {
                        results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                    }
                    LookupResult::MismatchSize => {
                        let _ = dm.release_read(key);
                        results[i] = Some(Err(DispatcherError::InvalidParameter(
                            "size mismatch on lookup".into(),
                        )));
                    }
                    LookupResult::MemoryTier { pointer, size } => {
                        let copy_size = (ipc_handle.size as usize).min(size as usize);
                        let raw = self.warm_stream.load(Ordering::Acquire);
                        let res = if raw != 0 {
                            let s = GpuStream(raw as *mut std::ffi::c_void);
                            gpu.memcpy_h2d_async(
                                pointer as *const std::ffi::c_void,
                                ipc_handle.address as *mut std::ffi::c_void,
                                copy_size,
                                s,
                            )
                            .map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (memory-tier→device) failed: {e}"
                                ))
                            })
                            .and_then(|_| {
                                gpu.stream_synchronize(s).map_err(|e| {
                                    DispatcherError::IoError(format!(
                                        "stream_synchronize failed: {e}"
                                    ))
                                })
                            })
                        } else {
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
                                DispatcherError::IoError(format!("DmaBuffer wrap failed: {e}"))
                            });
                            match temp_buf {
                                Ok(buf) => {
                                    let r = gpu.dma_copy_to_device(
                                        &buf,
                                        ipc_handle.address as *mut std::ffi::c_void,
                                        copy_size,
                                    )
                                    .map_err(|e| {
                                        DispatcherError::IoError(format!(
                                            "GPU DMA copy (memory-tier→device) failed: {e}"
                                        ))
                                    });
                                    std::mem::forget(buf);
                                    r
                                }
                                Err(e) => Err(e),
                            }
                        };
                        let _ = dm.release_read(key);
                        mt.touch(key);
                        results[i] = Some(res);
                    }
                    LookupResult::Staging { buffer } => {
                        let res = gpu
                            .dma_copy_to_device(
                                &buffer,
                                ipc_handle.address as *mut std::ffi::c_void,
                                ipc_handle.size as usize,
                            )
                            .map_err(|e| {
                                DispatcherError::IoError(format!(
                                    "GPU DMA copy (staging→device) failed: {e}"
                                ))
                            });
                        let _ = dm.release_read(key);
                        results[i] = Some(res);
                    }
                    LookupResult::BlockDevice { offset } => {
                        let _ = dm.release_read(key);
                        cold_entries.push(ColdEntry {
                            idx: i,
                            key,
                            offset,
                            ipc_handle_addr: ipc_handle.address,
                            ipc_handle_size: ipc_handle.size,
                        });
                    }
                },
                Err(_) => {
                    results[i] = Some(Err(DispatcherError::KeyNotFound(key)));
                }
            }
        }

        // Promote cold entries in parallel — multiple queue threads per drive.
        // Each thread gets its own NVMe queue pair and CUDA streams, enabling
        // concurrent reads on the same physical drive.
        if !cold_entries.is_empty() {
            const MAX_QUEUES_PER_DRIVE: usize = 2;

            let chunk_size = {
                let ring_guard = self.pipeline_ring.read();
                ring_guard.as_ref().map_or(131072, |r| r.chunk_size)
            };

            let drives = self.data_drives.read();
            let num_drives = drives.len();

            if num_drives == 0 {
                for entry in &cold_entries {
                    Self::evict_for_space(&dm, &mt, entry.ipc_handle_size).ok();
                    let res = mt.insert(entry.key, entry.ipc_handle_size).map(|mem_ptr| {
                        let _ = dm.create_memory_tier_entry(entry.key, mem_ptr, entry.ipc_handle_size);
                        let _ = dm.release_write(entry.key);
                    }).map_err(|e| {
                        DispatcherError::AllocationFailed(format!("promote insert failed: {e}"))
                    });
                    results[entry.idx] = Some(res);
                }
            } else {
                // Group cold entries by target drive.
                let mut per_drive: Vec<Vec<usize>> = vec![Vec::new(); num_drives];
                for (ci, entry) in cold_entries.iter().enumerate() {
                    let drive_idx = Self::drive_index(entry.key, num_drives);
                    per_drive[drive_idx].push(ci);
                }

                std::thread::scope(|s| {
                    let mut thread_handles: Vec<
                        std::thread::ScopedJoinHandle<Vec<(usize, Result<(), DispatcherError>)>>,
                    > = Vec::new();

                    for (drive_idx, entry_indices) in per_drive.iter().enumerate() {
                        if entry_indices.is_empty() {
                            continue;
                        }

                        // Split this drive's entries across multiple queue threads.
                        let num_queues = MAX_QUEUES_PER_DRIVE.min(entry_indices.len());
                        let chunks: Vec<&[usize]> = entry_indices
                            .chunks((entry_indices.len() + num_queues - 1) / num_queues)
                            .collect();

                        let queue_depth = 16 / num_queues;

                        for chunk in chunks {
                            let dm_ref = &dm;
                            let mt_ref = &mt;
                            let gpu_ref = &gpu;
                            let drives_ref = &drives;
                            let cold_ref = &cold_entries;
                            let indices = chunk.to_vec();

                            let handle = s.spawn(move || {
                                let drive = &drives_ref[drive_idx];
                                let block_size = drive.block_dev_iface.block_size();

                                let channels =
                                    drive.block_dev_iface.connect_client().map_err(|e| {
                                        DispatcherError::IoError(format!(
                                            "connect_client failed: {e}"
                                        ))
                                    });
                                let streams_result = gpu_ref.create_stream().and_then(|a| {
                                    gpu_ref.create_stream().map(|b| [a, b]).map_err(|e| {
                                        let _ = gpu_ref.destroy_stream(a);
                                        e
                                    })
                                });

                                let mut batch_results: Vec<(usize, Result<(), DispatcherError>)> =
                                    Vec::with_capacity(indices.len());

                                let (channels, streams) = match (channels, streams_result) {
                                    (Ok(ch), Ok(st)) => (ch, st),
                                    (Err(e), _) => {
                                        for &ci in &indices {
                                            batch_results.push((ci, Err(e.clone())));
                                        }
                                        return batch_results;
                                    }
                                    (_, Err(e)) => {
                                        let err = DispatcherError::IoError(format!(
                                            "create_stream failed: {e}"
                                        ));
                                        for &ci in &indices {
                                            batch_results.push((ci, Err(err.clone())));
                                        }
                                        return batch_results;
                                    }
                                };

                                for &ci in &indices {
                                    let entry = &cold_ref[ci];
                                    let ipc = IpcHandle {
                                        address: entry.ipc_handle_addr,
                                        size: entry.ipc_handle_size,
                                    };
                                    let total_bytes = ipc.size as usize;

                                    let res = (|| -> Result<(), DispatcherError> {
                                        Self::evict_for_space(dm_ref, mt_ref, ipc.size)?;

                                        let mem_ptr =
                                            mt_ref.insert(entry.key, ipc.size).map_err(|e| {
                                                DispatcherError::AllocationFailed(format!(
                                                    "promote insert failed: {e}"
                                                ))
                                            })?;

                                        let start_lba = entry.offset / block_size as u64;

                                        let pipeline_result = unsafe {
                                            pipeline::pipelined_ssd_to_gpu_zero_copy(
                                                &*drive.block_dev_iface,
                                                &**gpu_ref,
                                                &streams,
                                                &channels,
                                                mem_ptr,
                                                ipc.address as *mut std::ffi::c_void,
                                                start_lba,
                                                total_bytes,
                                                chunk_size,
                                                queue_depth,
                                            )
                                        };

                                        pipeline_result?;

                                        let _ = dm_ref.remove(entry.key);
                                        dm_ref
                                            .create_memory_tier_entry(entry.key, mem_ptr, ipc.size)
                                            .map_err(|e| {
                                                DispatcherError::IoError(format!(
                                                    "promote re-register failed: {e}"
                                                ))
                                            })?;
                                        let _ =
                                            dm_ref.convert_to_storage(entry.key, entry.offset);
                                        let _ = dm_ref.release_write(entry.key);

                                        Ok(())
                                    })();

                                    batch_results.push((ci, res));
                                }

                                let _ = gpu_ref.destroy_stream(streams[0]);
                                let _ = gpu_ref.destroy_stream(streams[1]);

                                batch_results
                            });


// --- Section: fallback path ---
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

    fn check(&self, key: CacheKey) -> Result<bool, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;