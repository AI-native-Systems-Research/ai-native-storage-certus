//! Init hooks — typed functions called by the generated composition code.
//!
//! Each hook receives the queried interface Arc and the runtime StackConfig,
//! performing component-specific initialization logic.

use std::sync::Arc;

use interfaces::{
    DmaAllocFn, DmaBuffer, DispatcherConfig, IDispatchMap, IDispatcher, IGpuServices, IMemoryTier,
};

use crate::config::StackConfig;

#[cfg(feature = "spdk")]
const NVME_CLASS_CODE: u32 = 0x010802;

#[cfg(feature = "spdk")]
pub fn init_spdk_env(
    iface: &Arc<dyn spdk_env::ISPDKEnv + Send + Sync>,
    config: &StackConfig,
) -> Result<(), String> {
    iface.init().map_err(|e| format!("SPDK init failed: {e}"))?;

    // Resolve device addresses: explicit list or auto-discover via SPDK.
    let addrs = if !config.device_pci.is_empty() {
        config.device_pci.clone()
    } else {
        let count = config.drive_count.unwrap_or(1);
        let devices = iface.devices();
        let mut nvme_devices: Vec<_> = devices
            .iter()
            .filter(|d| d.id.class_id == NVME_CLASS_CODE)
            .collect();
        nvme_devices.sort_by_key(|d| if d.numa_node == 0 { 0 } else { 1 });
        if nvme_devices.len() < count {
            return Err(format!(
                "--drive-count={count} but only {} NVMe device(s) discovered",
                nvme_devices.len()
            ));
        }
        nvme_devices[..count]
            .iter()
            .map(|d| d.address.to_string())
            .collect()
    };

    *config.resolved_pci_addrs.borrow_mut() = addrs;
    Ok(())
}

pub fn init_gpu(
    iface: &Arc<dyn IGpuServices + Send + Sync>,
    _config: &StackConfig,
) -> Result<(), String> {
    iface.initialize().map_err(|e| format!("GPU init failed: {e}"))
}

pub fn init_dispatch_map(
    iface: &Arc<dyn IDispatchMap + Send + Sync>,
    _config: &StackConfig,
) -> Result<(), String> {
    let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
        DmaBuffer::new(size, align, None).map_err(|e| e.to_string())
    });
    iface.set_dma_alloc(dma_alloc);
    iface
        .initialize()
        .map_err(|e| format!("DispatchMap init failed: {e}"))
}

pub fn init_memory_tier(
    iface: &Arc<dyn IMemoryTier + Send + Sync>,
    config: &StackConfig,
) -> Result<(), String> {
    iface
        .initialize(config.memory_tier_size)
        .map_err(|e| format!("MemoryTier init failed: {e}"))?;

    // Register the memory-tier pool with CUDA for pinned DMA transfers.
    if let Some((pool_ptr, pool_size)) = iface.pool_info() {
        let err = unsafe {
            gpu_services::cuda_ffi::cudaHostRegister(
                pool_ptr as *mut std::ffi::c_void,
                pool_size,
                0,
            )
        };
        if err != gpu_services::cuda_ffi::CUDA_SUCCESS {
            eprintln!(
                "warning: cudaHostRegister failed (err={err}), \
                 memory-tier transfers will use staged path"
            );
        }
    }

    Ok(())
}

pub fn init_dispatcher(
    iface: &Arc<dyn IDispatcher + Send + Sync>,
    config: &StackConfig,
) -> Result<(), String> {
    let mut data_pci_addrs = config.resolved_pci_addrs.borrow().clone();

    // When SPDK is not used, generate placeholder addresses for the requested drive count.
    if data_pci_addrs.is_empty() {
        let count = config.drive_count.unwrap_or(1);
        data_pci_addrs = (0..count)
            .map(|i| format!("0000:00:0{i}.0"))
            .collect();
    }

    iface
        .initialize(DispatcherConfig {
            data_pci_addrs,
            format_on_init: config.format,
            poller_base_cpu: config.poller_base_cpu,
            ..Default::default()
        })
        .map_err(|e| format!("Dispatcher init failed: {e}"))
}
