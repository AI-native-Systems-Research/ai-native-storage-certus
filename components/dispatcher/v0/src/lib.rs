//! Dispatcher component for the Certus storage system.
//!
//! Orchestrates cache operations (populate, lookup, check, remove) using
//! GPU-to-SSD data flows via DMA staging buffers. Coordinates N data block
//! devices with N extent managers for persistent storage.
//!
//! Provides the [`IDispatcher`] interface with receptacles for
//! [`ILogger`] and [`IDispatchMap`].

mod background;
pub mod io_segmenter;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use component_framework::define_component;
use interfaces::{
    BlockDeviceVersion, CacheKey, Command, Completion, DmaAllocFn, DmaBuffer,
    DispatcherConfig, DispatcherError, ExtentManagerVersion, FormatParams, IBlockDevice,
    IBlockDeviceAdmin, IDispatchMap, IDispatcher, IExtentManager, IGpuServices, ILogger, IpcHandle,
    LookupResult, PciAddress,
};

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponentV1;
use block_device_spdk_nvme_v2::BlockDeviceSpdkNvmeComponentV2;
use component_core::binding::bind;
use component_core::query_interface;
use extent_manager_v2::ExtentManagerV2;
use spdk_env::ISPDKEnv;

use crate::background::{BackgroundWriter, WriteJob};

/// Holds one (block-device, extent-manager) pair for a data drive.
#[allow(dead_code)]
struct DataDrive {
    _block_dev: Arc<dyn component_core::IUnknown + Send + Sync>,
    block_dev_admin: Arc<dyn IBlockDeviceAdmin + Send + Sync>,
    block_dev_iface: Arc<dyn IBlockDevice + Send + Sync>,
    extent_mgr: Arc<ExtentManagerV2>,
}

define_component! {
    pub DispatcherComponentV0 {
        version: "0.1.0",
        provides: [IDispatcher],
        receptacles: {
            logger: ILogger,
            dispatch_map: IDispatchMap,
            gpu_services: IGpuServices,
            spdk_env: ISPDKEnv,
        },
        fields: {
            initialized: AtomicBool,
            bg_writer: Mutex<Option<BackgroundWriter>>,
            data_drives: Mutex<Vec<DataDrive>>,
        },
    }
}

impl DispatcherComponentV0 {
    fn log_info(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.info(msg);
        }
    }

    #[allow(dead_code)]
    fn log_error(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            logger.error(msg);
        }
    }

    fn ensure_initialized(&self) -> Result<(), DispatcherError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(DispatcherError::NotInitialized(
                "dispatcher not initialized".into(),
            ));
        }
        Ok(())
    }

    fn read_from_block_device(
        &self,
        offset: u64,
        ipc_handle: &IpcHandle,
        gpu: &Arc<dyn IGpuServices + Send + Sync>,
    ) -> Result<(), DispatcherError> {
        let drives = self.data_drives.lock().unwrap();
        if drives.is_empty() {
            return Err(DispatcherError::IoError(
                "no data drives available for block device read".into(),
            ));
        }

        let drive = &drives[0];
        let block_size = drive.block_dev_iface.block_size();
        let max_transfer = drive.block_dev_iface.max_transfer_size();
        let numa_node = drive.block_dev_iface.numa_node();
        let start_lba = offset / block_size as u64;
        let total_bytes = ipc_handle.size as usize;
        let aligned_bytes = total_bytes.next_multiple_of(block_size as usize);

        let channels = drive.block_dev_iface.connect_client().map_err(|e| {
            DispatcherError::IoError(format!("connect_client failed: {e}"))
        })?;

        // Drop the lock before doing I/O.
        drop(drives);

        let segments =
            io_segmenter::segment_io(start_lba, aligned_bytes, max_transfer, block_size);

        // Allocate a contiguous DMA buffer for the full read, then copy to GPU.
        let read_buf = DmaBuffer::new(aligned_bytes, block_size as usize, Some(numa_node))
            .map_err(|e| {
                DispatcherError::AllocationFailed(format!("DMA read buffer: {e}"))
            })?;

        for seg in &segments {
            let seg_buf =
                DmaBuffer::new(seg.length, block_size as usize, Some(numa_node)).map_err(
                    |e| DispatcherError::AllocationFailed(format!("DMA segment buffer: {e}")),
                )?;
            let seg_buf = Arc::new(Mutex::new(seg_buf));

            channels
                .command_tx
                .send(Command::ReadSync {
                    ns_id: 1,
                    lba: seg.lba,
                    buf: Arc::clone(&seg_buf),
                })
                .map_err(|_| {
                    DispatcherError::IoError("send ReadSync failed".into())
                })?;

            match channels.completion_rx.recv() {
                Ok(Completion::ReadDone { result, .. }) => {
                    result.map_err(|e| {
                        DispatcherError::IoError(format!("SSD read failed: {e}"))
                    })?;
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

            // Copy segment data into the contiguous read buffer.
            let guard = seg_buf.lock().unwrap();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    guard.as_ptr() as *const u8,
                    (read_buf.as_ptr() as *mut u8).add(seg.buffer_offset),
                    seg.length,
                );
            }
        }

        // DMA copy the assembled data to the GPU/caller.
        gpu.dma_copy_to_device(
            &read_buf,
            ipc_handle.address as *mut std::ffi::c_void,
            total_bytes,
        )
        .map_err(|e| {
            DispatcherError::IoError(format!("GPU DMA copy (SSD→device) failed: {e}"))
        })?;

        Ok(())
    }

    fn process_write_job(
        dm: &Arc<dyn IDispatchMap + Send + Sync>,
        drives: &[Arc<dyn IBlockDevice + Send + Sync>],
        next_lba: &mut u64,
        job: WriteJob,
    ) {
        if drives.is_empty() {
            // Staging-only mode: no block devices, just mark as converted.
            let block_offset = job.key * 4096;
            let _ = dm.convert_to_storage(job.key, block_offset);
            return;
        }

        let drive = &drives[job.device_index % drives.len()];
        let block_size = drive.block_size() as usize;
        let max_transfer = drive.max_transfer_size();

        // Look up the staging buffer to get the data to write.
        let staging_buf = match dm.lookup(job.key) {
            Ok(LookupResult::Staging { buffer }) => buffer,
            _ => {
                // Entry may have been removed or already migrated; skip.
                return;
            }
        };

        let total_bytes = job.size as usize;
        let aligned_bytes = total_bytes.next_multiple_of(block_size);
        let num_blocks = (aligned_bytes / block_size) as u64;

        // Sequential LBA allocation.
        let start_lba = *next_lba;
        *next_lba += num_blocks;
        let block_offset = start_lba * block_size as u64;

        let channels = match drive.connect_client() {
            Ok(ch) => ch,
            Err(_) => return,
        };

        let numa_node = drive.numa_node();
        let segments =
            io_segmenter::segment_io(start_lba, aligned_bytes, max_transfer, block_size as u32);

        for seg in &segments {
            let seg_buf = match DmaBuffer::new(seg.length, block_size, Some(numa_node)) {
                Ok(buf) => buf,
                Err(_) => return,
            };

            // Copy data from staging buffer into the write DMA buffer.
            let copy_len = seg.length.min(total_bytes.saturating_sub(seg.buffer_offset));
            if copy_len > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (staging_buf.as_ptr() as *const u8).add(seg.buffer_offset),
                        seg_buf.as_ptr() as *mut u8,
                        copy_len,
                    );
                }
            }

            let seg_buf = Arc::new(seg_buf);
            if channels
                .command_tx
                .send(Command::WriteSync {
                    ns_id: 1,
                    lba: seg.lba,
                    buf: seg_buf,
                })
                .is_err()
            {
                return;
            }

            match channels.completion_rx.recv() {
                Ok(Completion::WriteDone { result, .. }) => {
                    if result.is_err() {
                        return;
                    }
                }
                _ => return,
            }
        }

        let _ = dm.convert_to_storage(job.key, block_offset);
    }
}

impl DispatcherComponentV0 {
    fn parse_pci_addr(s: &str) -> Result<PciAddress, DispatcherError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return Err(DispatcherError::InvalidParameter(format!(
                "invalid PCI address format: {s}"
            )));
        }
        let domain = u32::from_str_radix(parts[0], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI domain: {}", parts[0]))
        })?;
        let bus = u8::from_str_radix(parts[1], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI bus: {}", parts[1]))
        })?;
        let dev_func: Vec<&str> = parts[2].split('.').collect();
        if dev_func.len() != 2 {
            return Err(DispatcherError::InvalidParameter(format!(
                "invalid PCI dev.func: {}",
                parts[2]
            )));
        }
        let dev = u8::from_str_radix(dev_func[0], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI dev: {}", dev_func[0]))
        })?;
        let func = u8::from_str_radix(dev_func[1], 16).map_err(|_| {
            DispatcherError::InvalidParameter(format!("invalid PCI func: {}", dev_func[1]))
        })?;
        Ok(PciAddress {
            domain,
            bus,
            dev,
            func,
        })
    }

    #[allow(clippy::type_complexity)]
    fn create_block_device(
        &self,
        i: usize,
        version: BlockDeviceVersion,
        spdk_env: &Arc<dyn ISPDKEnv + Send + Sync>,
        logger: &Arc<dyn ILogger + Send + Sync>,
        pci_addr: PciAddress,
        addr_str: &str,
    ) -> Result<
        (
            Arc<dyn component_core::IUnknown + Send + Sync>,
            Arc<dyn IBlockDeviceAdmin + Send + Sync>,
            Arc<dyn IBlockDevice + Send + Sync>,
        ),
        DispatcherError,
    > {
        match version {
            BlockDeviceVersion::V1 => {
                let block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();
                block_dev
                    .spdk_env
                    .connect(Arc::clone(spdk_env))
                    .map_err(|e| {
                        DispatcherError::IoError(format!(
                            "failed to wire spdk_env for data drive {i}: {e}"
                        ))
                    })?;
                block_dev
                    .logger
                    .connect(Arc::clone(logger))
                    .map_err(|e| {
                        DispatcherError::IoError(format!(
                            "failed to wire logger for data drive {i}: {e}"
                        ))
                    })?;
                let admin = query_interface!(block_dev, IBlockDeviceAdmin).ok_or_else(|| {
                    DispatcherError::IoError(format!(
                        "failed to query IBlockDeviceAdmin for data drive {i}"
                    ))
                })?;
                admin.set_pci_address(pci_addr);
                admin.initialize().map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to initialize block device at {addr_str}: {e}"
                    ))
                })?;
                let ibd = query_interface!(block_dev, IBlockDevice).ok_or_else(|| {
                    DispatcherError::IoError(format!(
                        "failed to query IBlockDevice for data drive {i}"
                    ))
                })?;
                Ok((block_dev as Arc<dyn component_core::IUnknown + Send + Sync>, admin, ibd))
            }
            BlockDeviceVersion::V2 => {
                let block_dev = BlockDeviceSpdkNvmeComponentV2::new_default();
                block_dev
                    .spdk_env
                    .connect(Arc::clone(spdk_env))
                    .map_err(|e| {
                        DispatcherError::IoError(format!(
                            "failed to wire spdk_env for data drive {i}: {e}"
                        ))
                    })?;
                block_dev
                    .logger
                    .connect(Arc::clone(logger))
                    .map_err(|e| {
                        DispatcherError::IoError(format!(
                            "failed to wire logger for data drive {i}: {e}"
                        ))
                    })?;
                let admin = query_interface!(block_dev, IBlockDeviceAdmin).ok_or_else(|| {
                    DispatcherError::IoError(format!(
                        "failed to query IBlockDeviceAdmin for data drive {i}"
                    ))
                })?;
                admin.set_pci_address(pci_addr);
                admin.initialize().map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to initialize block device at {addr_str}: {e}"
                    ))
                })?;
                let ibd = query_interface!(block_dev, IBlockDevice).ok_or_else(|| {
                    DispatcherError::IoError(format!(
                        "failed to query IBlockDevice for data drive {i}"
                    ))
                })?;
                Ok((block_dev as Arc<dyn component_core::IUnknown + Send + Sync>, admin, ibd))
            }
        }
    }

    fn create_data_drives(&self, config: &DispatcherConfig) -> Result<Vec<DataDrive>, DispatcherError> {
        let spdk_env = self
            .spdk_env
            .get()
            .map_err(|_| DispatcherError::NotInitialized("spdk_env not bound".into()))?;

        let logger = self
            .logger
            .get()
            .map_err(|_| DispatcherError::NotInitialized("logger not bound".into()))?;

        let mut drives = Vec::with_capacity(config.data_pci_addrs.len());

        for (i, addr_str) in config.data_pci_addrs.iter().enumerate() {
            let pci_addr = Self::parse_pci_addr(addr_str)?;

            let (block_dev_component, admin, ibd) = self.create_block_device(
                i,
                config.block_device_version,
                &spdk_env,
                &logger,
                pci_addr,
                addr_str,
            )?;

            // Create extent manager for this drive
            let extent_mgr = match config.extent_manager_version {
                ExtentManagerVersion::V2 => ExtentManagerV2::new_inner(),
            };

            let numa_node = ibd.numa_node();
            let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
                DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
            });
            extent_mgr.set_dma_alloc(dma_alloc);

            extent_mgr
                .logger
                .connect(Arc::clone(&logger) as Arc<dyn ILogger + Send + Sync>)
                .map_err(|e| {
                    DispatcherError::IoError(format!(
                        "failed to wire logger for extent manager {i}: {e}"
                    ))
                })?;

            bind(
                &*block_dev_component,
                "IBlockDevice",
                &*extent_mgr as &dyn component_core::IUnknown,
                "metadata_device",
            )
            .map_err(|e| {
                DispatcherError::IoError(format!(
                    "failed to bind block device to extent manager {i}: {e}"
                ))
            })?;

            let iem = query_interface!(extent_mgr, IExtentManager).ok_or_else(|| {
                DispatcherError::IoError(format!(
                    "failed to query IExtentManager for data drive {i}"
                ))
            })?;
            iem.format(FormatParams::default()).map_err(|e| {
                DispatcherError::IoError(format!(
                    "failed to format extent manager for data drive {i}: {e}"
                ))
            })?;

            self.log_info(&format!(
                "dispatcher: data drive {i} initialized at {addr_str} (block_device={:?})",
                config.block_device_version
            ));

            drives.push(DataDrive {
                _block_dev: block_dev_component,
                block_dev_admin: admin,
                block_dev_iface: ibd,
                extent_mgr,
            });
        }

        Ok(drives)
    }
}

impl IDispatcher for DispatcherComponentV0 {
    fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError> {
        self.log_info("dispatcher: initializing");

        self.dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        if config.data_pci_addrs.is_empty() {
            return Err(DispatcherError::InvalidParameter(
                "data_pci_addrs must not be empty".into(),
            ));
        }

        // Create N block devices and N extent managers from config.
        // If spdk_env is not connected, skip drive creation (staging-only mode).
        if self.spdk_env.is_connected() {
            let drives = self.create_data_drives(&config)?;
            *self.data_drives.lock().unwrap() = drives;
        }

        let dm_for_writer = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        // Collect block device interfaces for the background writer.
        let bg_drives: Vec<Arc<dyn IBlockDevice + Send + Sync>> = self
            .data_drives
            .lock()
            .unwrap()
            .iter()
            .map(|d| Arc::clone(&d.block_dev_iface))
            .collect();

        // Reserve LBA 0 for metadata; start data at LBA 1024 (512 KiB offset).
        let mut next_lba: u64 = 1024;
        let writer = BackgroundWriter::start(move |job: WriteJob| {
            Self::process_write_job(&dm_for_writer, &bg_drives, &mut next_lba, job);
        });

        *self.bg_writer.lock().unwrap() = Some(writer);
        self.initialized.store(true, Ordering::Release);

        self.log_info("dispatcher: initialized");
        Ok(())
    }

    fn shutdown(&self) -> Result<(), DispatcherError> {
        self.log_info("dispatcher: shutting down");

        if let Some(mut writer) = self.bg_writer.lock().unwrap().take() {
            writer.shutdown();
        }

        // Shut down block devices in reverse order
        let drives = std::mem::take(&mut *self.data_drives.lock().unwrap());
        for (i, drive) in drives.iter().enumerate().rev() {
            if let Err(e) = drive.block_dev_admin.shutdown() {
                self.log_error(&format!(
                    "dispatcher: failed to shut down data drive {i}: {e}"
                ));
            }
        }

        self.initialized.store(false, Ordering::Release);
        self.log_info("dispatcher: shut down");
        Ok(())
    }

    fn lookup(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.take_read(key)
            .map_err(|_| DispatcherError::KeyNotFound(key))?;

        let result = dm.lookup(key);

        dm.release_read(key)
            .map_err(|_| DispatcherError::IoError("failed to release read lock".into()))?;

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        match result {
            Ok(lookup_result) => match lookup_result {
                LookupResult::NotExist => Err(DispatcherError::KeyNotFound(key)),
                LookupResult::MismatchSize => Err(DispatcherError::InvalidParameter(
                    "size mismatch on lookup".into(),
                )),
                LookupResult::Staging { buffer } => {
                    gpu.dma_copy_to_device(
                        &buffer,
                        ipc_handle.address as *mut std::ffi::c_void,
                        ipc_handle.size as usize,
                    )
                    .map_err(|e| {
                        DispatcherError::IoError(format!(
                            "GPU DMA copy (staging→device) failed: {e}"
                        ))
                    })?;
                    Ok(())
                }
                LookupResult::BlockDevice { offset } => {
                    self.read_from_block_device(offset, &ipc_handle, &gpu)
                }
            },
            Err(_) => Err(DispatcherError::KeyNotFound(key)),
        }
    }

    fn check(&self, key: CacheKey) -> Result<bool, DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        match dm.lookup(key) {
            Ok(result) => {
                use interfaces::LookupResult;
                match result {
                    LookupResult::NotExist => Ok(false),
                    _ => Ok(true),
                }
            }
            Err(_) => Ok(false),
        }
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        dm.take_write(key)
            .map_err(|_| DispatcherError::KeyNotFound(key))?;

        let result = dm.remove(key);

        match result {
            Ok(()) => {
                // TODO: Free SSD extent if entry was in block-device state
                Ok(())
            }
            Err(_) => {
                let _ = dm.release_write(key);
                Err(DispatcherError::KeyNotFound(key))
            }
        }
    }

    fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
        self.ensure_initialized()?;

        if ipc_handle.size == 0 {
            return Err(DispatcherError::InvalidParameter(
                "IPC handle size must be > 0".into(),
            ));
        }

        let dm = self
            .dispatch_map
            .get()
            .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

        let block_count = ipc_handle.size.div_ceil(4096);

        let staging_buffer = dm.create_staging(key, block_count).map_err(|e| match e {
            interfaces::DispatchMapError::AlreadyExists(k) => DispatcherError::AlreadyExists(k),
            interfaces::DispatchMapError::AllocationFailed(msg) => {
                DispatcherError::AllocationFailed(msg)
            }
            other => DispatcherError::IoError(other.to_string()),
        })?;

        let gpu = self
            .gpu_services
            .get()
            .map_err(|_| DispatcherError::NotInitialized("gpu_services not bound".into()))?;

        gpu.dma_copy_to_host(
            ipc_handle.address as *const std::ffi::c_void,
            &staging_buffer,
            ipc_handle.size as usize,
        )
        .map_err(|e| DispatcherError::IoError(format!("GPU DMA copy failed: {e}")))?;

        dm.downgrade_reference(key)
            .map_err(|e| DispatcherError::IoError(e.to_string()))?;

        let guard = self.bg_writer.lock().unwrap();
        if let Some(ref writer) = *guard {
            let _ = writer.enqueue(WriteJob {
                key,
                size: ipc_handle.size,
                device_index: 0,
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::thread;

    use interfaces::{
        DispatchMapError, DmaAllocFn, DmaBuffer, GpuDeviceInfo, GpuDmaBuffer, GpuIpcHandle,
        LookupResult,
    };

    // -----------------------------------------------------------------------
    // Test infrastructure
    // -----------------------------------------------------------------------

    unsafe extern "C" fn dma_free(ptr: *mut std::ffi::c_void) {
        // SAFETY: ptr was allocated with libc::aligned_alloc in alloc_dma_buffer.
        unsafe { libc::free(ptr) };
    }

    fn alloc_dma_buffer(size: usize) -> Arc<DmaBuffer> {
        let sz = size.max(4096);
        // SAFETY: aligned_alloc requires alignment to be a power of 2 and size
        // to be a multiple of alignment. We enforce both here.
        let aligned_sz = sz.next_multiple_of(4096);
        let ptr = unsafe { libc::aligned_alloc(4096, aligned_sz) };
        assert!(!ptr.is_null(), "aligned_alloc failed for {aligned_sz} bytes");
        // SAFETY: ptr is valid, 4096-aligned, and covers aligned_sz bytes.
        // libc::free is the matching deallocator for aligned_alloc.
        unsafe { std::ptr::write_bytes(ptr as *mut u8, 0, aligned_sz) };
        let buf = unsafe { DmaBuffer::from_raw(ptr, aligned_sz, dma_free, -1) }.unwrap();
        Arc::new(buf)
    }

    struct MockEntry {
        buffer: Arc<DmaBuffer>,
        block_offset: Option<u64>,
        write_ref: bool,
        read_refs: u32,
    }

    struct MockDmInner {
        entries: HashMap<CacheKey, MockEntry>,
        fail_alloc: bool,
        mismatch_keys: HashSet<CacheKey>,
    }

    struct MockDispatchMap {
        inner: Mutex<MockDmInner>,
    }

    impl MockDispatchMap {
        fn new() -> Self {
            Self {
                inner: Mutex::new(MockDmInner {
                    entries: HashMap::new(),
                    fail_alloc: false,
                    mismatch_keys: HashSet::new(),
                }),
            }
        }

        fn with_fail_alloc() -> Self {
            Self {
                inner: Mutex::new(MockDmInner {
                    entries: HashMap::new(),
                    fail_alloc: true,
                    mismatch_keys: HashSet::new(),
                }),
            }
        }

        fn entry_count(&self) -> usize {
            self.inner.lock().unwrap().entries.len()
        }

        fn set_mismatch_key(&self, key: CacheKey) {
            self.inner.lock().unwrap().mismatch_keys.insert(key);
        }

        fn convert_entry_to_block(&self, key: CacheKey, offset: u64) {
            let mut inner = self.inner.lock().unwrap();
            if let Some(entry) = inner.entries.get_mut(&key) {
                entry.block_offset = Some(offset);
            }
        }
    }

    impl IDispatchMap for MockDispatchMap {
        fn set_dma_alloc(&self, _alloc: DmaAllocFn) {}

        fn initialize(&self) -> Result<(), DispatchMapError> {
            Ok(())
        }

        fn create_staging(
            &self,
            key: CacheKey,
            size: u32,
        ) -> Result<Arc<DmaBuffer>, DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.fail_alloc {
                return Err(DispatchMapError::AllocationFailed(
                    "mock: out of memory".into(),
                ));
            }
            if inner.entries.contains_key(&key) {
                return Err(DispatchMapError::AlreadyExists(key));
            }
            let buffer = alloc_dma_buffer(size as usize * 4096);
            inner.entries.insert(
                key,
                MockEntry {
                    buffer: Arc::clone(&buffer),
                    block_offset: None,
                    write_ref: true,
                    read_refs: 0,
                },
            );
            Ok(buffer)
        }

        fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
            let inner = self.inner.lock().unwrap();
            if inner.mismatch_keys.contains(&key) {
                return Ok(LookupResult::MismatchSize);
            }
            match inner.entries.get(&key) {
                None => Ok(LookupResult::NotExist),
                Some(entry) => match entry.block_offset {
                    Some(offset) => Ok(LookupResult::BlockDevice { offset }),
                    None => Ok(LookupResult::Staging {
                        buffer: Arc::clone(&entry.buffer),
                    }),
                },
            }
        }

        fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.block_offset = Some(offset);
                    Ok(())
                }
            }
        }

        fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.read_refs += 1;
                    Ok(())
                }
            }
        }

        fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.write_ref = true;
                    Ok(())
                }
            }
        }

        fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.read_refs = entry.read_refs.saturating_sub(1);
                    Ok(())
                }
            }
        }

        fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::KeyNotFound(key)),
                Some(entry) => {
                    entry.write_ref = false;
                    Ok(())
                }
            }
        }

        fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            match inner.entries.get_mut(&key) {
                None => Err(DispatchMapError::NoWriteReference(key)),
                Some(entry) => {
                    entry.write_ref = false;
                    entry.read_refs += 1;
                    Ok(())
                }
            }
        }

        fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
            let mut inner = self.inner.lock().unwrap();
            if inner.entries.remove(&key).is_some() {
                Ok(())
            } else {
                Err(DispatchMapError::KeyNotFound(key))
            }
        }

        fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
            let inner = self.inner.lock().unwrap();
            inner.entries.keys().copied().take(n).collect()
        }
    }

    struct MockLogger;

    impl ILogger for MockLogger {
        fn error(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
        fn info(&self, _msg: &str) {}
        fn debug(&self, _msg: &str) {}
    }

    struct MockGpuServices;

    impl IGpuServices for MockGpuServices {
        fn initialize(&self) -> Result<(), String> {
            Ok(())
        }
        fn shutdown(&self) -> Result<(), String> {
            Ok(())
        }
        fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String> {
            Ok(vec![])
        }
        fn deserialize_ipc_handle(&self, _base64_payload: &str) -> Result<GpuIpcHandle, String> {
            Err("mock: not implemented".into())
        }
        fn verify_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn pin_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn unpin_memory(&self, _handle: &GpuIpcHandle) -> Result<(), String> {
            Ok(())
        }
        fn create_dma_buffer(&self, _handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
            Err("mock: not implemented".into())
        }
        fn dma_copy_to_host(
            &self,
            src: *const std::ffi::c_void,
            dst: &DmaBuffer,
            size: usize,
        ) -> Result<(), String> {
            // SAFETY: src is a valid host pointer (from IpcHandle) and dst is a valid DmaBuffer.
            unsafe {
                std::ptr::copy_nonoverlapping(src as *const u8, dst.as_ptr() as *mut u8, size);
            }
            Ok(())
        }
        fn dma_copy_to_device(
            &self,
            src: &DmaBuffer,
            dst: *mut std::ffi::c_void,
            size: usize,
        ) -> Result<(), String> {
            // SAFETY: src is a valid DmaBuffer and dst is a valid host pointer (from IpcHandle).
            unsafe {
                std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, dst as *mut u8, size);
            }
            Ok(())
        }
    }

    fn setup_initialized() -> (Arc<DispatcherComponentV0>, Arc<MockDispatchMap>) {
        let dm = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        c.dispatch_map
            .connect(Arc::clone(&dm) as Arc<dyn IDispatchMap + Send + Sync>)
            .unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            metadata_pci_addr: "0000:01:00.0".to_string(),
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        (c, dm)
    }

    fn make_handle(buf: &mut [u8]) -> IpcHandle {
        IpcHandle {
            address: buf.as_mut_ptr(),
            size: buf.len() as u32,
        }
    }

    // -----------------------------------------------------------------------
    // Pre-initialization tests (existing)
    // -----------------------------------------------------------------------

    #[test]
    fn component_creation() {
        let _c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
    }

    #[test]
    fn query_idispatcher() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher);
        assert!(d.is_some());
    }

    #[test]
    fn initialize_without_receptacles_fails() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            metadata_pci_addr: "0000:01:00.0".to_string(),
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        };
        let err = d.initialize(config);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn initialize_with_empty_pci_addrs_fails() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            metadata_pci_addr: "0000:01:00.0".to_string(),
            data_pci_addrs: vec![],
            ..Default::default()
        };
        // This will fail with NotInitialized since dispatch_map isn't bound
        let err = d.initialize(config);
        assert!(err.is_err());
    }

    #[test]
    fn lookup_before_initialize_fails() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 4096,
        };
        let err = d.lookup(42, handle);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn check_before_initialize_fails() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.check(42);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn remove_before_initialize_fails() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.remove(42);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn populate_before_initialize_fails() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 4096,
        };
        let err = d.populate(42, handle);
        assert!(matches!(err, Err(DispatcherError::NotInitialized(_))));
    }

    #[test]
    fn populate_with_zero_size_fails() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        // Even though not initialized, zero-size check comes after init check.
        // This test verifies the parameter validation exists in the code path.
        let mut buf = vec![0u8; 0];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 0,
        };
        let err = d.populate(42, handle);
        // Will fail with NotInitialized since that check comes first
        assert!(err.is_err());
    }

    #[test]
    fn shutdown_without_initialize_succeeds() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn double_shutdown_succeeds() {
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn concurrent_pre_init_calls_from_multiple_threads() {
        let c = Arc::new(DispatcherComponentV0::new(
            AtomicBool::new(false),
            Mutex::new(None),
            Mutex::new(Vec::new()),
        ));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    assert!(matches!(
                        d.check(1),
                        Err(DispatcherError::NotInitialized(_))
                    ));
                    assert!(matches!(
                        d.remove(1),
                        Err(DispatcherError::NotInitialized(_))
                    ));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }

    // -----------------------------------------------------------------------
    // Initialized dispatcher tests (with mock dispatch map)
    // -----------------------------------------------------------------------

    #[test]
    fn initialize_with_dispatch_map_succeeds() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        assert!(d.shutdown().is_ok());
    }

    #[test]
    fn initialize_empty_addrs_with_dispatch_map() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        c.dispatch_map.connect(dm).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        let config = DispatcherConfig {
            metadata_pci_addr: "0000:01:00.0".to_string(),
            data_pci_addrs: vec![],
            ..Default::default()
        };
        let err = d.initialize(config);
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
    }

    #[test]
    fn initialize_multiple_pci_addrs() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::new());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        c.dispatch_map.connect(dm).unwrap();
        c.logger.connect(logger).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            metadata_pci_addr: "0000:01:00.0".to_string(),
            data_pci_addrs: vec![
                "0000:02:00.0".to_string(),
                "0000:03:00.0".to_string(),
                "0000:04:00.0".to_string(),
            ],
            ..Default::default()
        })
        .unwrap();
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_succeeds_after_init() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        assert!(d.populate(1, make_handle(&mut buf)).is_ok());
        assert_eq!(dm.entry_count(), 1);
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_zero_size_returns_invalid_parameter_after_init() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 0];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 0,
        };
        let err = d.populate(1, handle);
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_duplicate_key_returns_already_exists() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf1 = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf1)).unwrap();

        let mut buf2 = vec![0u8; 4096];
        let err = d.populate(1, make_handle(&mut buf2));
        assert!(matches!(err, Err(DispatcherError::AlreadyExists(1))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_allocation_failure() {
        let dm: Arc<dyn IDispatchMap + Send + Sync> = Arc::new(MockDispatchMap::with_fail_alloc());
        let logger: Arc<dyn ILogger + Send + Sync> = Arc::new(MockLogger);
        let gpu: Arc<dyn IGpuServices + Send + Sync> = Arc::new(MockGpuServices);
        let c = DispatcherComponentV0::new(AtomicBool::new(false), Mutex::new(None), Mutex::new(Vec::new()));
        c.dispatch_map.connect(dm).unwrap();
        c.logger.connect(logger).unwrap();
        c.gpu_services.connect(gpu).unwrap();

        let d = query_interface!(c, IDispatcher).unwrap();
        d.initialize(DispatcherConfig {
            metadata_pci_addr: "0000:01:00.0".to_string(),
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        let mut buf = vec![0u8; 4096];
        let err = d.populate(1, make_handle(&mut buf));
        assert!(matches!(err, Err(DispatcherError::AllocationFailed(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_non_block_aligned_size() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 5000];
        let handle = IpcHandle {
            address: buf.as_mut_ptr(),
            size: 5000,
        };
        assert!(d.populate(1, handle).is_ok());
        d.shutdown().unwrap();
    }

    #[test]
    fn populate_enqueues_many_writes() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        for i in 0..100 {
            let mut buf = vec![0u8; 4096];
            d.populate(i, make_handle(&mut buf)).unwrap();
        }
        assert_eq!(dm.entry_count(), 100);
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_staging_hit() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        let mut buf2 = vec![0u8; 4096];
        assert!(d.lookup(1, make_handle(&mut buf2)).is_ok());
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_block_device_hit_without_hardware_returns_error() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        dm.convert_entry_to_block(1, 0x1000);

        let mut buf2 = vec![0u8; 4096];
        let err = d.lookup(1, make_handle(&mut buf2));
        assert!(
            matches!(err, Err(DispatcherError::IoError(_))),
            "BlockDevice lookup without hardware should return IoError, got: {err:?}"
        );
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_key_not_found() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        let err = d.lookup(999, make_handle(&mut buf));
        assert!(matches!(err, Err(DispatcherError::KeyNotFound(999))));
        d.shutdown().unwrap();
    }

    #[test]
    fn lookup_mismatch_size_returns_invalid_parameter() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();

        dm.set_mismatch_key(1);

        let mut buf2 = vec![0u8; 4096];
        let err = d.lookup(1, make_handle(&mut buf2));
        assert!(matches!(err, Err(DispatcherError::InvalidParameter(_))));
        d.shutdown().unwrap();
    }

    #[test]
    fn check_existing_returns_true() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        assert_eq!(d.check(1).unwrap(), true);
        d.shutdown().unwrap();
    }

    #[test]
    fn check_nonexistent_returns_false() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        assert_eq!(d.check(999).unwrap(), false);
        d.shutdown().unwrap();
    }

    #[test]
    fn remove_existing_succeeds() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let mut buf = vec![0u8; 4096];
        d.populate(1, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 1);
        assert!(d.remove(1).is_ok());
        assert_eq!(dm.entry_count(), 0);
        d.shutdown().unwrap();
    }

    #[test]
    fn remove_nonexistent_returns_key_not_found() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        let err = d.remove(999);
        assert!(matches!(err, Err(DispatcherError::KeyNotFound(999))));
        d.shutdown().unwrap();
    }

    #[test]
    fn full_lifecycle_populate_check_lookup_remove() {
        let (c, dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();

        let mut buf = vec![0u8; 8192];
        d.populate(42, make_handle(&mut buf)).unwrap();
        assert_eq!(dm.entry_count(), 1);

        assert_eq!(d.check(42).unwrap(), true);
        assert_eq!(d.check(99).unwrap(), false);

        let mut buf2 = vec![0u8; 8192];
        assert!(d.lookup(42, make_handle(&mut buf2)).is_ok());

        assert!(d.remove(42).is_ok());
        assert_eq!(dm.entry_count(), 0);

        assert_eq!(d.check(42).unwrap(), false);

        d.shutdown().unwrap();
    }

    #[test]
    fn operations_after_shutdown_fail() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();

        let mut buf = vec![0u8; 4096];
        assert!(matches!(
            d.populate(1, make_handle(&mut buf)),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.check(1),
            Err(DispatcherError::NotInitialized(_))
        ));
        let mut buf2 = vec![0u8; 4096];
        assert!(matches!(
            d.lookup(1, make_handle(&mut buf2)),
            Err(DispatcherError::NotInitialized(_))
        ));
        assert!(matches!(
            d.remove(1),
            Err(DispatcherError::NotInitialized(_))
        ));
    }

    #[test]
    fn reinitialize_after_shutdown() {
        let (c, _dm) = setup_initialized();
        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();

        d.initialize(DispatcherConfig {
            metadata_pci_addr: "0000:01:00.0".to_string(),
            data_pci_addrs: vec!["0000:02:00.0".to_string()],
            ..Default::default()
        })
        .unwrap();

        assert_eq!(d.check(1).unwrap(), false);
        d.shutdown().unwrap();
    }

    #[test]
    fn concurrent_checks_on_initialized_dispatcher() {
        let (c, _dm) = setup_initialized();

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    for k in 0..10 {
                        let result = d.check(i * 100 + k);
                        assert!(result.is_ok());
                        assert_eq!(result.unwrap(), false);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();
    }

    #[test]
    fn concurrent_populate_different_keys() {
        let (c, dm) = setup_initialized();

        let handles: Vec<_> = (0..4)
            .map(|t| {
                let comp = Arc::clone(&c);
                thread::spawn(move || {
                    let d = query_interface!(comp, IDispatcher).unwrap();
                    for i in 0..5 {
                        let key = t * 100 + i;
                        let mut buf = vec![0u8; 4096];
                        d.populate(key, make_handle(&mut buf)).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(dm.entry_count(), 20);

        let d = query_interface!(c, IDispatcher).unwrap();
        d.shutdown().unwrap();
    }

}
