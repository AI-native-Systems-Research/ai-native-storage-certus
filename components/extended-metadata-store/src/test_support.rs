//! Test infrastructure: MockBlockDevice, heap_dma_alloc, fault injection.

use interfaces::iblock_device::{
    ClientChannels, Command, Completion, IBlockDevice, NvmeBlockError, OpHandle,
    TelemetrySnapshot,
};
use interfaces::{DmaAllocFn, DmaBuffer};

use component_core::channel::SpscChannel;
use component_core::channel::{Receiver, Sender};

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

/// Fault injection configuration for simulating crashes.
#[derive(Debug, Clone, Default)]
pub struct FaultConfig {
    /// Fail all writes after this many have succeeded. None = no limit.
    pub fail_after_n_writes: Option<u64>,
}

/// Shared state for MockBlockDevice — persists across "reboots".
#[derive(Debug, Clone)]
pub struct MockState {
    pub blocks: HashMap<u64, Vec<u8>>,
    pub sector_size: u32,
    pub num_sectors: u64,
    pub write_count: u64,
}

impl MockState {
    pub fn new(disk_size: u64, sector_size: u32) -> Self {
        Self {
            blocks: HashMap::new(),
            sector_size,
            num_sectors: disk_size / sector_size as u64,
            write_count: 0,
        }
    }
}

/// In-memory block device for testing persistence logic without SPDK.
pub struct MockBlockDevice {
    state: Arc<Mutex<MockState>>,
    #[allow(dead_code)]
    fault_config: FaultConfig,
    channels: Mutex<Option<ClientChannels>>,
    #[allow(dead_code)]
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl MockBlockDevice {
    pub fn new(disk_size: u64) -> Arc<Self> {
        Self::with_fault_config(disk_size, FaultConfig::default())
    }

    pub fn with_fault_config(disk_size: u64, fault_config: FaultConfig) -> Arc<Self> {
        let state = Arc::new(Mutex::new(MockState::new(disk_size, 4096)));
        Self::from_state(state, fault_config)
    }

    /// Create a MockBlockDevice from existing shared state (for simulating reboot).
    pub fn reboot_from(shared_state: Arc<Mutex<MockState>>) -> Arc<Self> {
        {
            let mut s = shared_state.lock().unwrap();
            s.write_count = 0;
        }
        Self::from_state(shared_state, FaultConfig::default())
    }

    fn from_state(state: Arc<Mutex<MockState>>, fault_config: FaultConfig) -> Arc<Self> {
        let cmd_channel = SpscChannel::<Command>::new(64);
        let comp_channel = SpscChannel::<Completion>::new(64);

        let cmd_rx = cmd_channel.receiver().unwrap();
        let cmd_tx = cmd_channel.sender().unwrap();
        let comp_rx = comp_channel.receiver().unwrap();
        let comp_tx = comp_channel.sender().unwrap();

        let channels = ClientChannels {
            command_tx: cmd_tx,
            completion_rx: comp_rx,
        };

        let worker_state = state.clone();
        let worker_faults = fault_config.clone();
        let worker = thread::spawn(move || {
            Self::worker_loop(cmd_rx, comp_tx, worker_state, worker_faults);
        });

        Arc::new(Self {
            state,
            fault_config,
            channels: Mutex::new(Some(channels)),
            worker: Mutex::new(Some(worker)),
        })
    }

    fn worker_loop(
        cmd_rx: Receiver<Command>,
        comp_tx: Sender<Completion>,
        state: Arc<Mutex<MockState>>,
        faults: FaultConfig,
    ) {
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                Command::ReadSync { lba, buf, .. } => {
                    let s = state.lock().unwrap();
                    let sector_size = s.sector_size as usize;
                    let data = s.blocks.get(&lba).cloned();
                    drop(s);

                    let mut locked = buf.lock().unwrap();
                    if let Some(block_data) = data {
                        let len = block_data.len().min(sector_size);
                        locked.as_mut_slice()[..len].copy_from_slice(&block_data[..len]);
                    } else {
                        locked.as_mut_slice()[..sector_size].fill(0);
                    }
                    drop(locked);

                    let _ = comp_tx.send(Completion::ReadDone {
                        handle: OpHandle(0),
                        tag: 0,
                        result: Ok(()),
                    });
                }
                Command::WriteSync { lba, buf, .. } => {
                    let mut s = state.lock().unwrap();

                    if let Some(limit) = faults.fail_after_n_writes {
                        if s.write_count >= limit {
                            drop(s);
                            let _ = comp_tx.send(Completion::WriteDone {
                                handle: OpHandle(0),
                                tag: 0,
                                result: Err(NvmeBlockError::LbaOutOfRange(
                                    "fault injected".into(),
                                )),
                            });
                            continue;
                        }
                    }

                    let sector_size = s.sector_size as usize;
                    let data = buf.as_slice()[..sector_size].to_vec();

                    s.blocks.insert(lba, data);
                    s.write_count += 1;
                    drop(s);

                    let _ = comp_tx.send(Completion::WriteDone {
                        handle: OpHandle(0),
                        tag: 0,
                        result: Ok(()),
                    });
                }
                _ => {}
            }
        }
    }

    /// Get the shared state for inspection or reboot simulation.
    pub fn shared_state(&self) -> Arc<Mutex<MockState>> {
        self.state.clone()
    }
}

impl IBlockDevice for MockBlockDevice {
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        self.channels
            .lock()
            .unwrap()
            .take()
            .ok_or(NvmeBlockError::ClientDisconnected("already connected".into()))
    }

    fn sector_size(&self, _ns_id: u32) -> Result<u32, NvmeBlockError> {
        Ok(self.state.lock().unwrap().sector_size)
    }

    fn num_sectors(&self, _ns_id: u32) -> Result<u64, NvmeBlockError> {
        Ok(self.state.lock().unwrap().num_sectors)
    }

    fn max_queue_depth(&self) -> u32 {
        64
    }

    fn num_io_queues(&self) -> u32 {
        1
    }

    fn max_transfer_size(&self) -> u32 {
        128 * 1024
    }

    fn block_size(&self) -> u32 {
        self.state.lock().unwrap().sector_size
    }

    fn numa_node(&self) -> i32 {
        -1
    }

    fn nvme_version(&self) -> String {
        "mock-1.0".into()
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

    fn io_byte_stats(&self) -> interfaces::IoByteStats {
        interfaces::IoByteStats::default()
    }
}

// --- DMA allocation for tests (heap-based, no hugepages) ---

static ALLOC_REGISTRY: OnceLock<Mutex<HashMap<usize, Layout>>> = OnceLock::new();

fn alloc_registry() -> &'static Mutex<HashMap<usize, Layout>> {
    ALLOC_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

unsafe extern "C" fn heap_free(ptr: *mut std::ffi::c_void) {
    let addr = ptr as usize;
    if let Some(layout) = alloc_registry().lock().unwrap().remove(&addr) {
        unsafe { dealloc(ptr as *mut u8, layout) };
    }
}

/// DMA allocator backed by the standard heap (aligned allocation).
pub fn heap_dma_alloc() -> DmaAllocFn {
    Arc::new(|size: usize, align: usize, _numa: Option<i32>| {
        let layout = Layout::from_size_align(size, align)
            .map_err(|e| format!("invalid layout: {e}"))?;
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("allocation failed".into());
        }
        alloc_registry()
            .lock()
            .unwrap()
            .insert(ptr as usize, layout);
        // SAFETY: ptr is valid, non-null, allocated with alloc_zeroed for `size` bytes.
        unsafe { DmaBuffer::from_raw(ptr as *mut std::ffi::c_void, size, heap_free, -1) }
            .map_err(|e| e.to_string())
    })
}

/// Create a test component wired with a MockBlockDevice (128MiB virtual disk).
pub fn create_test_component(
    disk_size: u64,
) -> (Arc<crate::ExtendedMetadataStoreComponent>, Arc<MockBlockDevice>) {
    let mock = MockBlockDevice::new(disk_size);
    let comp = crate::ExtendedMetadataStoreComponent::new_default();
    (comp, mock)
}

/// Create a test component from existing shared state (simulates reboot).
pub fn create_test_component_from_state(
    shared_state: Arc<Mutex<MockState>>,
) -> (Arc<crate::ExtendedMetadataStoreComponent>, Arc<MockBlockDevice>) {
    let mock = MockBlockDevice::reboot_from(shared_state);
    let comp = crate::ExtendedMetadataStoreComponent::new_default();
    (comp, mock)
}
