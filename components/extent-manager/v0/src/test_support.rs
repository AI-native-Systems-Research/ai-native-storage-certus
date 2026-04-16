//! Test infrastructure: MockBlockDevice, heap DMA allocation, fault injection.
//!
//! This module is gated behind `#[cfg(test)]` and not compiled in production.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use component_core::channel::spsc::SpscChannel;
use interfaces::{
    BlockDeviceError, ClientChannels, Command, Completion, DmaBuffer, IBlockDevice, NvmeBlockError,
    OpHandle, TelemetrySnapshot,
};

use crate::block_device::DmaAllocFn;
use crate::metadata::BLOCK_SIZE;
use crate::ExtentManagerComponentV1;
use component_core::iunknown::query;

// ---------------------------------------------------------------------------
// Heap DMA allocation (T003)
// ---------------------------------------------------------------------------

/// C-ABI deallocator matching the 4096/4096 layout used by [`heap_dma_alloc`].
///
/// # Safety
///
/// `ptr` must have been allocated with `std::alloc::alloc_zeroed` using
/// `Layout::from_size_align(4096, 4096)`.
unsafe extern "C" fn heap_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        let layout = std::alloc::Layout::from_size_align_unchecked(BLOCK_SIZE, BLOCK_SIZE);
        std::alloc::dealloc(ptr as *mut u8, layout);
    }
}

/// Allocate a zeroed, 4KiB-aligned heap buffer wrapped as a [`DmaBuffer`].
///
/// This replaces `DmaBuffer::new()` in tests so that no SPDK runtime is needed.
pub fn heap_dma_alloc(
    _size: usize,
    _align: usize,
    _numa_node: Option<i32>,
) -> Result<DmaBuffer, String> {
    let layout = std::alloc::Layout::from_size_align(BLOCK_SIZE, BLOCK_SIZE)
        .map_err(|e| format!("layout error: {e}"))?;

    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            return Err("heap allocation failed".into());
        }
        DmaBuffer::from_raw(ptr as *mut std::ffi::c_void, BLOCK_SIZE, heap_free, -1)
            .map_err(|e| format!("from_raw failed: {e}"))
    }
}

/// Return a [`DmaAllocFn`] backed by heap memory.
pub fn heap_dma_alloc_fn() -> DmaAllocFn {
    Arc::new(heap_dma_alloc)
}

// ---------------------------------------------------------------------------
// FaultConfig (T004)
// ---------------------------------------------------------------------------

/// Controls fault injection in [`MockBlockDevice`].
pub struct FaultConfig {
    /// Fail after N successful writes (None = disabled).
    pub fail_after_n_writes: Option<u32>,
    /// Fail writes targeting LBAs in `[start, end)` (None = disabled).
    pub fail_lba_range: Option<(u64, u64)>,
    /// Fail all subsequent writes unconditionally.
    pub fail_all_writes: bool,
    /// Internal write counter for `fail_after_n_writes`.
    write_count: u32,
}

impl FaultConfig {
    pub fn new() -> Self {
        Self {
            fail_after_n_writes: None,
            fail_lba_range: None,
            fail_all_writes: false,
            write_count: 0,
        }
    }

    /// Check whether this write should be failed according to the config.
    /// Updates internal counters as a side-effect.
    fn should_fail_write(&mut self, lba: u64) -> bool {
        if self.fail_all_writes {
            return true;
        }

        if let Some((start, end)) = self.fail_lba_range {
            if lba >= start && lba < end {
                return true;
            }
        }

        if let Some(n) = self.fail_after_n_writes {
            self.write_count += 1;
            if self.write_count > n {
                return true;
            }
        }

        false
    }
}

// ---------------------------------------------------------------------------
// MockBlockDevice (T005 + T006)
// ---------------------------------------------------------------------------

/// In-memory block device implementing [`IBlockDevice`] for testing.
///
/// Block storage is a `HashMap<u64, [u8; 4096]>` protected by a `Mutex`.
/// An actor thread is spawned per `connect_client()` call to process
/// `ReadSync`/`WriteSync` commands.
pub struct MockBlockDevice {
    blocks: Arc<Mutex<HashMap<u64, [u8; BLOCK_SIZE]>>>,
    sector_size: u32,
    num_sectors: u64,
    fault_config: Arc<Mutex<FaultConfig>>,
}

impl MockBlockDevice {
    /// Create a new mock block device with `num_blocks` 4KiB blocks.
    pub fn new(num_blocks: u64) -> Self {
        Self {
            blocks: Arc::new(Mutex::new(HashMap::new())),
            sector_size: BLOCK_SIZE as u32,
            num_sectors: num_blocks,
            fault_config: Arc::new(Mutex::new(FaultConfig::new())),
        }
    }

    /// Return a handle to the fault config for test-time mutation.
    pub fn fault_config(&self) -> Arc<Mutex<FaultConfig>> {
        Arc::clone(&self.fault_config)
    }

    /// Return a handle to the raw block storage for assertions or
    /// for creating a "rebooted" device from the same backing store.
    pub fn blocks(&self) -> Arc<Mutex<HashMap<u64, [u8; BLOCK_SIZE]>>> {
        Arc::clone(&self.blocks)
    }

    /// Create a new `MockBlockDevice` sharing the same block storage
    /// but with a fresh `FaultConfig`. Simulates rebooting.
    pub fn reboot_from(
        existing: &Arc<Mutex<HashMap<u64, [u8; BLOCK_SIZE]>>>,
        num_blocks: u64,
    ) -> Self {
        Self {
            blocks: Arc::clone(existing),
            sector_size: BLOCK_SIZE as u32,
            num_sectors: num_blocks,
            fault_config: Arc::new(Mutex::new(FaultConfig::new())),
        }
    }
}

/// Process a single command against the in-memory block store.
fn process_command(
    blocks: &Mutex<HashMap<u64, [u8; BLOCK_SIZE]>>,
    fault_config: &Mutex<FaultConfig>,
    command: Command,
) -> Completion {
    match command {
        Command::ReadSync { lba, buf, .. } => {
            let blocks_guard = blocks.lock().unwrap();
            let data = blocks_guard.get(&lba).copied().unwrap_or([0u8; BLOCK_SIZE]);
            drop(blocks_guard);

            let mut buf_guard = buf.lock().unwrap();
            buf_guard.as_mut_slice()[..BLOCK_SIZE].copy_from_slice(&data);
            drop(buf_guard);

            Completion::ReadDone {
                handle: OpHandle(0),
                result: Ok(()),
            }
        }
        Command::WriteSync { lba, buf, .. } => {
            // Check fault injection before writing.
            let mut fc = fault_config.lock().unwrap();
            if fc.should_fail_write(lba) {
                return Completion::WriteDone {
                    handle: OpHandle(0),
                    result: Err(NvmeBlockError::BlockDevice(BlockDeviceError::WriteFailed(
                        "fault injected".into(),
                    ))),
                };
            }
            drop(fc);

            let data: [u8; BLOCK_SIZE] = buf.as_slice()[..BLOCK_SIZE].try_into().unwrap();
            let mut blocks_guard = blocks.lock().unwrap();
            blocks_guard.insert(lba, data);

            Completion::WriteDone {
                handle: OpHandle(0),
                result: Ok(()),
            }
        }
        _ => Completion::Error {
            handle: None,
            error: NvmeBlockError::NotSupported("mock only supports ReadSync/WriteSync".into()),
        },
    }
}

impl IBlockDevice for MockBlockDevice {
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        let cmd_ch = SpscChannel::<Command>::new(64);
        let (cmd_tx, cmd_rx) = cmd_ch.split().map_err(|e| {
            NvmeBlockError::ClientDisconnected(format!("command channel split: {e}"))
        })?;

        let comp_ch = SpscChannel::<Completion>::new(64);
        let (comp_tx, comp_rx) = comp_ch.split().map_err(|e| {
            NvmeBlockError::ClientDisconnected(format!("completion channel split: {e}"))
        })?;

        let blocks = Arc::clone(&self.blocks);
        let fault_config = Arc::clone(&self.fault_config);

        std::thread::spawn(move || loop {
            match cmd_rx.recv() {
                Ok(command) => {
                    let completion = process_command(&blocks, &fault_config, command);
                    if comp_tx.send(completion).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        });

        Ok(ClientChannels {
            command_tx: cmd_tx,
            completion_rx: comp_rx,
        })
    }

    fn sector_size(&self, _ns_id: u32) -> Result<u32, NvmeBlockError> {
        Ok(self.sector_size)
    }

    fn num_sectors(&self, _ns_id: u32) -> Result<u64, NvmeBlockError> {
        Ok(self.num_sectors)
    }

    fn max_queue_depth(&self) -> u32 {
        64
    }

    fn num_io_queues(&self) -> u32 {
        1
    }

    fn max_transfer_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    fn block_size(&self) -> u32 {
        self.sector_size
    }

    fn numa_node(&self) -> i32 {
        0
    }

    fn nvme_version(&self) -> String {
        "mock-1.0".to_string()
    }

    fn telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError> {
        Ok(TelemetrySnapshot {
            total_ops: 0,
            min_latency_ns: 0,
            max_latency_ns: 0,
            mean_latency_ns: 0,
            mean_throughput_mbps: 0.0,
            elapsed_secs: 0.0,
        })
    }
}

// ---------------------------------------------------------------------------
// Test component helper (T008)
// ---------------------------------------------------------------------------

/// Create a fully-wired `ExtentManagerComponentV1` backed by a [`MockBlockDevice`].
///
/// Returns `(component, mock)` where `mock` can be used for fault injection
/// or block inspection.
pub fn create_test_component(
    num_blocks: u64,
    sizes: &[u32],
    slots: &[u32],
) -> (Arc<ExtentManagerComponentV1>, Arc<MockBlockDevice>) {
    let mock = Arc::new(MockBlockDevice::new(num_blocks));

    let comp = ExtentManagerComponentV1::new_default();

    // Wire mock as IBlockDevice provider.
    let ibd: Arc<dyn IBlockDevice + Send + Sync> = mock.clone();
    comp.block_device.connect(ibd).unwrap();

    // Set heap DMA allocator (bypasses SPDK) via admin interface.
    let admin = query::<dyn interfaces::IExtentManagerAdmin + Send + Sync>(&*comp)
        .expect("IExtentManagerAdmin query");
    admin.set_dma_alloc(heap_dma_alloc_fn());
    // Initialize the extent manager via admin interface.
    admin.initialize(sizes.to_vec(), slots.to_vec(), 1).unwrap();

    (comp, mock)
}

/// Create a wired but **not initialized** component for testing pre-init errors.
pub fn create_uninit_component(
    num_blocks: u64,
) -> (Arc<ExtentManagerComponentV1>, Arc<MockBlockDevice>) {
    let mock = Arc::new(MockBlockDevice::new(num_blocks));

    let comp = ExtentManagerComponentV1::new_default();

    let ibd: Arc<dyn IBlockDevice + Send + Sync> = mock.clone();
    comp.block_device.connect(ibd).unwrap();
    let admin = query::<dyn interfaces::IExtentManagerAdmin + Send + Sync>(&*comp)
        .expect("IExtentManagerAdmin query");
    admin.set_dma_alloc(heap_dma_alloc_fn());

    (comp, mock)
}
