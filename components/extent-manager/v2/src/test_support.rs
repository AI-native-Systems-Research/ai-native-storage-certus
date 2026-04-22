use component_core::channel::SpscChannel;
use interfaces::{
    ClientChannels, Command, Completion, DmaAllocFn, DmaBuffer, IBlockDevice, IExtentManager,
    ILogger, NvmeBlockError, OpHandle, TelemetrySnapshot,
};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;

const BLOCK_SIZE: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct FaultConfig {
    pub fail_after_n_writes: Option<u32>,
    pub fail_all_writes: bool,
}

pub struct MockState {
    pub blocks: HashMap<u64, Vec<u8>>,
    fault_config: FaultConfig,
    write_count: u32,
    disk_size: u64,
}

pub struct MockBlockDevice {
    state: Arc<Mutex<MockState>>,
}

impl MockBlockDevice {
    pub fn new(disk_size: u64) -> Self {
        MockBlockDevice {
            state: Arc::new(Mutex::new(MockState {
                blocks: HashMap::new(),
                fault_config: FaultConfig::default(),
                write_count: 0,
                disk_size,
            })),
        }
    }

    pub fn with_fault_config(disk_size: u64, fault_config: FaultConfig) -> Self {
        MockBlockDevice {
            state: Arc::new(Mutex::new(MockState {
                blocks: HashMap::new(),
                fault_config,
                write_count: 0,
                disk_size,
            })),
        }
    }

    pub fn set_fault_config(&self, config: FaultConfig) {
        let mut state = self.state.lock().unwrap();
        state.fault_config = config;
        state.write_count = 0;
    }

    pub fn clear_faults(&self) {
        self.set_fault_config(FaultConfig::default());
    }

    pub fn shared_state(&self) -> Arc<Mutex<MockState>> {
        Arc::clone(&self.state)
    }

    pub fn reboot_from(shared_state: &Arc<Mutex<MockState>>) -> Self {
        {
            let mut state = shared_state.lock().unwrap();
            state.fault_config = FaultConfig::default();
            state.write_count = 0;
        }
        MockBlockDevice {
            state: Arc::clone(shared_state),
        }
    }

    fn make_channels(&self) -> ClientChannels {
        let cmd_ch = SpscChannel::<Command>::new(64);
        let cmd_tx = cmd_ch.sender().expect("cmd sender");
        let cmd_rx = cmd_ch.receiver().expect("cmd receiver");

        let comp_ch = SpscChannel::<Completion>::new(64);
        let comp_tx = comp_ch.sender().expect("comp sender");
        let comp_rx = comp_ch.receiver().expect("comp receiver");

        let state = Arc::clone(&self.state);
        let handle_counter = Arc::new(AtomicU64::new(1));

        thread::spawn(move || {
            while let Ok(cmd) = cmd_rx.recv() {
                let handle = OpHandle(handle_counter.fetch_add(1, Ordering::Relaxed));
                let completion = process_command(cmd, &state, handle);
                if comp_tx.send(completion).is_err() {
                    break;
                }
            }
        });

        ClientChannels {
            command_tx: cmd_tx,
            completion_rx: comp_rx,
        }
    }
}

impl IBlockDevice for MockBlockDevice {
    fn connect_client(&self) -> Result<ClientChannels, NvmeBlockError> {
        Ok(self.make_channels())
    }

    fn sector_size(&self, _ns_id: u32) -> Result<u32, NvmeBlockError> {
        Ok(BLOCK_SIZE as u32)
    }

    fn num_sectors(&self, _ns_id: u32) -> Result<u64, NvmeBlockError> {
        let state = self.state.lock().unwrap();
        Ok(state.disk_size / BLOCK_SIZE as u64)
    }

    fn max_queue_depth(&self) -> u32 {
        64
    }

    fn num_io_queues(&self) -> u32 {
        1
    }

    fn max_transfer_size(&self) -> u32 {
        u32::MAX
    }

    fn block_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    fn numa_node(&self) -> i32 {
        -1
    }

    fn nvme_version(&self) -> String {
        "mock".to_string()
    }

    fn telemetry(&self) -> Result<TelemetrySnapshot, NvmeBlockError> {
        Err(NvmeBlockError::NotSupported(
            "mock does not support telemetry".into(),
        ))
    }
}

fn process_command(cmd: Command, state: &Arc<Mutex<MockState>>, handle: OpHandle) -> Completion {
    match cmd {
        Command::ReadSync { lba, buf, .. } => {
            let s = state.lock().unwrap();
            let data = s
                .blocks
                .get(&lba)
                .cloned()
                .unwrap_or_else(|| vec![0u8; BLOCK_SIZE]);
            drop(s);
            {
                let mut locked = buf.lock().unwrap();
                let len = data.len().min(locked.as_mut_slice().len());
                locked.as_mut_slice()[..len].copy_from_slice(&data[..len]);
            }
            Completion::ReadDone {
                handle,
                result: Ok(()),
            }
        }
        Command::WriteSync { lba, buf, .. } => {
            let mut s = state.lock().unwrap();

            if s.fault_config.fail_all_writes {
                return Completion::WriteDone {
                    handle,
                    result: Err(NvmeBlockError::BlockDevice(
                        interfaces::BlockDeviceError::WriteFailed("fault injected".into()),
                    )),
                };
            }

            if let Some(n) = s.fault_config.fail_after_n_writes {
                if s.write_count >= n {
                    return Completion::WriteDone {
                        handle,
                        result: Err(NvmeBlockError::BlockDevice(
                            interfaces::BlockDeviceError::WriteFailed(format!(
                                "fault after {n} writes",
                            )),
                        )),
                    };
                }
            }

            let mut data = vec![0u8; BLOCK_SIZE];
            let len = BLOCK_SIZE.min(buf.as_slice().len());
            data[..len].copy_from_slice(&buf.as_slice()[..len]);
            s.blocks.insert(lba, data);
            s.write_count += 1;

            Completion::WriteDone {
                handle,
                result: Ok(()),
            }
        }
        Command::WriteZeros {
            lba, num_blocks, ..
        } => {
            let mut s = state.lock().unwrap();
            for i in 0..num_blocks as u64 {
                s.blocks.remove(&(lba + i));
            }
            Completion::WriteZerosDone {
                handle,
                result: Ok(()),
            }
        }
        _ => Completion::Error {
            handle: Some(handle),
            error: NvmeBlockError::NotSupported(
                "mock only supports ReadSync/WriteSync/WriteZeros".into(),
            ),
        },
    }
}

pub struct MockLogger;

impl ILogger for MockLogger {
    fn error(&self, msg: &str) {
        eprintln!("[ERROR] {msg}");
    }
    fn warn(&self, msg: &str) {
        eprintln!("[WARN] {msg}");
    }
    fn info(&self, msg: &str) {
        eprintln!("[INFO] {msg}");
    }
    fn debug(&self, msg: &str) {
        eprintln!("[DEBUG] {msg}");
    }
}

use std::sync::OnceLock;

static ALLOC_REGISTRY: OnceLock<Mutex<HashMap<usize, std::alloc::Layout>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<usize, std::alloc::Layout>> {
    ALLOC_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn heap_dma_alloc() -> DmaAllocFn {
    Arc::new(|size: usize, align: usize, _numa: Option<i32>| {
        let layout = std::alloc::Layout::from_size_align(size, align)
            .map_err(|e| format!("invalid layout: {e}"))?;
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("heap allocation failed".to_string());
        }
        registry()
            .lock()
            .unwrap()
            .insert(ptr as usize, layout);
        unsafe {
            DmaBuffer::from_raw(ptr as *mut std::ffi::c_void, size, heap_free, -1)
                .map_err(|e| e.to_string())
        }
    })
}

unsafe extern "C" fn heap_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        let layout = registry()
            .lock()
            .unwrap()
            .remove(&(ptr as usize))
            .unwrap_or_else(|| {
                std::alloc::Layout::from_size_align(BLOCK_SIZE, BLOCK_SIZE).unwrap()
            });
        unsafe { std::alloc::dealloc(ptr as *mut u8, layout) };
    }
}

pub fn create_test_component(
    disk_size: u64,
) -> (Arc<crate::MetadataManagerV2>, Arc<MockBlockDevice>) {
    let mock = Arc::new(MockBlockDevice::new(disk_size));
    let component = crate::MetadataManagerV2::new_inner();
    component
        .block_device
        .connect(mock.clone() as Arc<dyn IBlockDevice + Send + Sync>)
        .expect("connect mock block device");

    let logger = Arc::new(MockLogger);
    component
        .logger
        .connect(logger as Arc<dyn ILogger + Send + Sync>)
        .expect("connect logger");

    component.set_dma_alloc(heap_dma_alloc());
    (component, mock)
}
