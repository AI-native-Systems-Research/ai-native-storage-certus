mod config;
mod report;
mod stats;
mod worker;

use std::sync::{Arc, Barrier};
use std::time::Instant;

use clap::Parser;

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponentV1;
use component_core::binding::bind;
use component_core::iunknown::query;
use extent_manager_v2::test_support::{heap_dma_alloc, MockBlockDevice, MockLogger};
use extent_manager_v2::ExtentManagerV2;
use interfaces::{
    DmaAllocFn, DmaBuffer, FormatParams, IBlockDevice, IExtentManager, ILogger,
};
use spdk_env::SPDKEnvComponent;

use config::BenchmarkConfig;

const METADATA_ALIGNMENT: u64 = 1_048_576; // 1 MiB

fn main() {
    let config = BenchmarkConfig::parse();

    if let Err(msg) = validate_config(&config) {
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    let count = config.effective_count();
    let params = make_format_params(&config, count);

    report::print_header(&config, count, params.data_disk_size);

    match config.metadata_device.clone() {
        None => run_mock_mode(config, count, params),
        Some(addr) => run_hardware_mode(config, count, params, addr),
    }
}

// --- Mock mode ---------------------------------------------------------------

fn run_mock_mode(config: BenchmarkConfig, count: u64, params: FormatParams) {
    let metadata_disk_size =
        compute_metadata_disk_size(count, config.size_class, config.slab_size, config.region_count);

    let metadata_mock = Arc::new(MockBlockDevice::new(metadata_disk_size));
    let shared_state = metadata_mock.shared_state();

    let component = make_mock_component(metadata_mock);

    let recover: Box<dyn Fn() -> Arc<ExtentManagerV2>> = Box::new(move || {
        let new_mock = Arc::new(MockBlockDevice::reboot_from(&shared_state));
        make_mock_component(new_mock)
    });

    run_phases(component, recover, params, &config, count);
}

fn make_mock_component(mock: Arc<MockBlockDevice>) -> Arc<ExtentManagerV2> {
    let component = ExtentManagerV2::new_inner();
    component
        .metadata_device
        .connect(mock as Arc<dyn IBlockDevice + Send + Sync>)
        .unwrap_or_else(|e| {
            eprintln!("error: connect mock metadata device: {e}");
            std::process::exit(2);
        });
    component
        .logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap_or_else(|e| {
            eprintln!("error: connect logger: {e}");
            std::process::exit(2);
        });
    component.set_dma_alloc(heap_dma_alloc());
    component
}

// --- Hardware mode -----------------------------------------------------------

fn run_hardware_mode(config: BenchmarkConfig, count: u64, params: FormatParams, pci_addr: String) {
    let spdk_env_comp = SPDKEnvComponent::new_default();
    let metadata_block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();

    bind(&*spdk_env_comp, "ISPDKEnv", &*metadata_block_dev, "spdk_env").unwrap_or_else(|e| {
        eprintln!("error: bind spdk_env→metadata_block_dev: {e}");
        std::process::exit(2);
    });

    let ienv =
        query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp).unwrap_or_else(|| {
            eprintln!("error: failed to query ISPDKEnv");
            std::process::exit(2);
        });
    if let Err(e) = ienv.init() {
        eprintln!("error: SPDK init failed: {e}");
        std::process::exit(2);
    }

    let metadata_target = parse_pci_addr(&pci_addr).unwrap_or_else(|| {
        eprintln!("error: invalid metadata device PCI address: {pci_addr}");
        std::process::exit(1);
    });

    let devices = ienv.devices();
    if devices.is_empty() {
        eprintln!("error: no NVMe devices found");
        std::process::exit(2);
    }
    devices
        .iter()
        .find(|d| {
            d.address.domain == metadata_target.domain
                && d.address.bus == metadata_target.bus
                && d.address.dev == metadata_target.dev
                && d.address.func == metadata_target.func
        })
        .unwrap_or_else(|| {
            eprintln!("error: no NVMe device found at {pci_addr}");
            std::process::exit(1);
        });

    let metadata_admin =
        query::<dyn interfaces::IBlockDeviceAdmin + Send + Sync>(&*metadata_block_dev)
            .unwrap_or_else(|| {
                eprintln!("error: failed to query IBlockDeviceAdmin for metadata device");
                std::process::exit(2);
            });
    metadata_admin.set_pci_address(metadata_target);
    if let Err(e) = metadata_admin.initialize() {
        eprintln!("error: metadata block device init failed: {e}");
        std::process::exit(2);
    }

    let metadata_ibd = query::<dyn IBlockDevice + Send + Sync>(&*metadata_block_dev)
        .unwrap_or_else(|| {
            eprintln!("error: failed to query IBlockDevice for metadata device");
            std::process::exit(2);
        });
    let numa_node = metadata_ibd.numa_node();
    let dma_alloc: DmaAllocFn = Arc::new(move |size, align, _numa| {
        DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
    });

    let meta_ns_id = config.metadata_ns_id;
    let component = make_hardware_component(&metadata_block_dev, Arc::clone(&dma_alloc), meta_ns_id);

    let meta_dev_clone = Arc::clone(&metadata_block_dev);
    let dma_alloc_clone = Arc::clone(&dma_alloc);
    let recover: Box<dyn Fn() -> Arc<ExtentManagerV2>> = Box::new(move || {
        make_hardware_component(&meta_dev_clone, Arc::clone(&dma_alloc_clone), meta_ns_id)
    });

    run_phases(component, recover, params, &config, count);
}

fn make_hardware_component(
    metadata_block_dev: &Arc<BlockDeviceSpdkNvmeComponentV1>,
    dma_alloc: DmaAllocFn,
    metadata_ns_id: u32,
) -> Arc<ExtentManagerV2> {
    let component = ExtentManagerV2::new_inner();
    component.set_dma_alloc(dma_alloc);
    component.set_metadata_ns_id(metadata_ns_id);
    component
        .logger
        .connect(Arc::new(MockLogger) as Arc<dyn ILogger + Send + Sync>)
        .unwrap_or_else(|e| {
            eprintln!("error: connect logger: {e}");
            std::process::exit(2);
        });
    bind(
        &**metadata_block_dev,
        "IBlockDevice",
        &*component,
        "metadata_device",
    )
    .unwrap_or_else(|e| {
        eprintln!("error: bind metadata_block_dev→extent_mgr: {e}");
        std::process::exit(2);
    });
    component
}

// --- Shared phase runner -----------------------------------------------------

fn run_phases(
    component: Arc<ExtentManagerV2>,
    recover: Box<dyn Fn() -> Arc<ExtentManagerV2>>,
    params: FormatParams,
    config: &BenchmarkConfig,
    count: u64,
) {
    let iem = get_iem(&component);
    iem.set_checkpoint_interval(None);
    if let Err(e) = iem.format(params) {
        eprintln!("error: format failed: {e}");
        std::process::exit(2);
    }

    // Phase: Create
    let size_class = config.size_class;
    let phase_start = Instant::now();
    let create_worker_results = run_parallel(
        Arc::clone(&iem),
        config.threads,
        move |iem, key_start, key_count| run_create(iem, key_start, key_count, size_class),
        count,
    );
    let create_elapsed = phase_start.elapsed();
    let create_result = worker::aggregate_results("Create", create_worker_results, create_elapsed);
    report::print_phase(&create_result);

    // Phase: Checkpoint
    let cp_start = Instant::now();
    if let Err(e) = iem.checkpoint() {
        eprintln!("error: checkpoint failed: {e}");
        std::process::exit(2);
    }
    let cp_elapsed = cp_start.elapsed();
    report::print_single_op("Checkpoint", cp_elapsed);

    // Phase: Recover — drop old component, reinitialize from storage
    drop(iem);
    drop(component);
    let new_component = recover();
    let new_iem = get_iem(&new_component);
    new_iem.set_checkpoint_interval(None);

    let recover_start = Instant::now();
    if let Err(e) = new_iem.initialize() {
        eprintln!("error: initialize (recover) failed: {e}");
        std::process::exit(2);
    }
    let recover_elapsed = recover_start.elapsed();
    report::print_single_op("Recover", recover_elapsed);

    // Phase: Enumerate — collect offsets for the Remove phase
    let enum_start = Instant::now();
    let mut offsets: Vec<u64> = Vec::with_capacity(count as usize);
    new_iem.for_each_extent(&mut |e| offsets.push(e.offset));
    let enum_elapsed = enum_start.elapsed();
    report::print_enumerate(offsets.len() as u64, count, enum_elapsed);

    // Phase: Remove
    let rm_start = Instant::now();
    let remove_worker_results = run_parallel_remove(Arc::clone(&new_iem), config.threads, offsets);
    let rm_elapsed = rm_start.elapsed();
    let remove_result = worker::aggregate_results("Remove", remove_worker_results, rm_elapsed);
    report::print_phase(&remove_result);

    report::print_summary(count, &create_result, &remove_result);
}

fn get_iem(component: &Arc<ExtentManagerV2>) -> Arc<dyn IExtentManager + Send + Sync> {
    query::<dyn IExtentManager + Send + Sync>(&**component).unwrap_or_else(|| {
        eprintln!("error: failed to query IExtentManager");
        std::process::exit(2);
    })
}

// --- Phase implementations ---------------------------------------------------

// Cap stored latency samples per thread to avoid multi-GB allocations at 100M scale.
// At 100M ops with 1 thread, we sample every 100th operation (1M samples × 16 B = 16 MB).
const MAX_LATENCY_SAMPLES: u64 = 1_000_000;

// Returns (actual_ops_completed, latency_samples).
fn run_create(
    iem: Arc<dyn IExtentManager + Send + Sync>,
    key_start: u64,
    count: u64,
    size_class: u32,
) -> (u64, Vec<std::time::Duration>) {
    let sample_every = (count / MAX_LATENCY_SAMPLES).max(1);
    let capacity = (count / sample_every + 1) as usize;
    let mut latencies = Vec::with_capacity(capacity);
    for i in 0..count {
        let key = key_start + i;
        let t = Instant::now();
        match iem.reserve_extent(key, size_class) {
            Ok(handle) => {
                if let Err(e) = handle.publish() {
                    eprintln!("  publish({key}) failed: {e}");
                }
            }
            Err(e) => eprintln!("  reserve_extent({key}) failed: {e}"),
        }
        if i % sample_every == 0 {
            latencies.push(t.elapsed());
        }
    }
    (count, latencies)
}

// Returns (actual_ops_completed, latency_samples).
fn run_remove_slice(
    iem: Arc<dyn IExtentManager + Send + Sync>,
    offsets: Vec<u64>,
) -> (u64, Vec<std::time::Duration>) {
    let count = offsets.len() as u64;
    let sample_every = (count / MAX_LATENCY_SAMPLES).max(1);
    let capacity = (count / sample_every + 1) as usize;
    let mut latencies = Vec::with_capacity(capacity);
    for (i, offset) in offsets.into_iter().enumerate() {
        let t = Instant::now();
        if let Err(e) = iem.remove_extent(offset) {
            eprintln!("  remove_extent({offset:#x}) failed: {e}");
        }
        if i as u64 % sample_every == 0 {
            latencies.push(t.elapsed());
        }
    }
    (count, latencies)
}

// Run a key-range operation across `threads` threads with a barrier start.
fn run_parallel<F>(
    iem: Arc<dyn IExtentManager + Send + Sync>,
    threads: usize,
    op: F,
    count: u64,
) -> Vec<(usize, u64, Vec<std::time::Duration>)>
where
    F: Fn(Arc<dyn IExtentManager + Send + Sync>, u64, u64) -> (u64, Vec<std::time::Duration>)
        + Send
        + Sync
        + 'static,
{
    let effective = threads.min(count as usize).max(1);
    let ranges = compute_ranges(count, effective);
    let barrier = Arc::new(Barrier::new(effective));
    let op = Arc::new(op);

    let handles: Vec<_> = ranges
        .into_iter()
        .enumerate()
        .map(|(tid, (key_start, key_count))| {
            let iem = Arc::clone(&iem);
            let barrier = Arc::clone(&barrier);
            let op = Arc::clone(&op);
            std::thread::spawn(move || {
                barrier.wait();
                let (ops, latencies) = op(iem, key_start, key_count);
                (tid, ops, latencies)
            })
        })
        .collect();

    handles
        .into_iter()
        .map(|h| {
            h.join().unwrap_or_else(|_| {
                eprintln!("error: worker thread panicked");
                std::process::exit(2);
            })
        })
        .collect()
}

fn run_parallel_remove(
    iem: Arc<dyn IExtentManager + Send + Sync>,
    threads: usize,
    offsets: Vec<u64>,
) -> Vec<(usize, u64, Vec<std::time::Duration>)> {
    let effective = threads.min(offsets.len()).max(1);
    let barrier = Arc::new(Barrier::new(effective));
    let chunks = split_offsets(offsets, effective);

    let handles: Vec<_> = chunks
        .into_iter()
        .enumerate()
        .map(|(tid, chunk)| {
            let iem = Arc::clone(&iem);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let (ops, latencies) = run_remove_slice(iem, chunk);
                (tid, ops, latencies)
            })
        })
        .collect();

    handles
        .into_iter()
        .map(|h| {
            h.join().unwrap_or_else(|_| {
                eprintln!("error: worker thread panicked");
                std::process::exit(2);
            })
        })
        .collect()
}

// --- Utilities ---------------------------------------------------------------

fn compute_ranges(total: u64, threads: usize) -> Vec<(u64, u64)> {
    let per = total / threads as u64;
    let rem = total % threads as u64;
    let mut ranges = Vec::with_capacity(threads);
    let mut offset = 0u64;
    for i in 0..threads as u64 {
        let count = per + if i < rem { 1 } else { 0 };
        ranges.push((offset, count));
        offset += count;
    }
    ranges
}

fn split_offsets(offsets: Vec<u64>, threads: usize) -> Vec<Vec<u64>> {
    let per = (offsets.len() + threads - 1) / threads;
    offsets.chunks(per.max(1)).map(|c| c.to_vec()).collect()
}

/// Compute the minimum logical data-disk size to hold `count` extents.
fn compute_data_disk_size(count: u64, size_class: u32, slab_size: u64, region_count: u32) -> u64 {
    let slots_per_slab = slab_size / size_class as u64;
    let total_slabs = count.div_ceil(slots_per_slab);
    // Round up so each region gets the same number of slabs.
    let aligned_slabs = total_slabs.div_ceil(region_count as u64) * region_count as u64;
    aligned_slabs * slab_size
}

/// Compute the minimum metadata-disk size to hold two checkpoint copies for `count` extents.
fn compute_metadata_disk_size(
    count: u64,
    size_class: u32,
    slab_size: u64,
    region_count: u32,
) -> u64 {
    let slots_per_slab = slab_size / size_class as u64;
    let total_slabs = count.div_ceil(slots_per_slab);
    let slabs_per_region = total_slabs.div_ceil(region_count as u64);
    // Per slab in checkpoint: u64 start + u64 size + u32 element_size + u32 num_slots + keys
    let bytes_per_slab = 24u64 + slots_per_slab * 8;
    let bytes_per_region = 4 + slabs_per_region * bytes_per_slab;
    let payload = region_count as u64 * bytes_per_region;
    // Two copies + superblock + generous alignment and header headroom
    let with_overhead = payload * 3 + METADATA_ALIGNMENT * 16;
    // Round up to 4 MiB
    with_overhead.div_ceil(4 * 1024 * 1024) * (4 * 1024 * 1024)
}

fn make_format_params(config: &BenchmarkConfig, count: u64) -> FormatParams {
    let data_disk_size = config.total_size.unwrap_or_else(|| {
        compute_data_disk_size(count, config.size_class, config.slab_size, config.region_count)
    });
    FormatParams {
        data_disk_size,
        slab_size: config.slab_size,
        max_extent_size: config.size_class,
        sector_size: 4096,
        region_count: config.region_count,
        metadata_alignment: METADATA_ALIGNMENT,
        instance_id: None,
        metadata_disk_ns_id: config.metadata_ns_id,
    }
}

fn validate_config(config: &BenchmarkConfig) -> Result<(), String> {
    if config.size_class == 0 || config.size_class % 4096 != 0 {
        return Err(format!(
            "size-class must be a non-zero multiple of 4096, got {}",
            config.size_class
        ));
    }
    if config.slab_size == 0
        || config.slab_size % config.size_class as u64 != 0
        || config.slab_size < config.size_class as u64
    {
        return Err(format!(
            "slab-size ({}) must be a non-zero multiple of size-class ({})",
            config.slab_size, config.size_class
        ));
    }
    if config.region_count == 0 || !config.region_count.is_power_of_two() {
        return Err(format!(
            "region-count must be a power of two >= 1, got {}",
            config.region_count
        ));
    }
    if config.threads == 0 {
        return Err("threads must be >= 1".to_string());
    }
    if let Some(count) = config.count {
        if count == 0 {
            return Err("count must be >= 1".to_string());
        }
    }
    Ok(())
}

fn parse_pci_addr(s: &str) -> Option<interfaces::PciAddress> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let domain = u32::from_str_radix(parts[0], 16).ok()?;
    let bus = u8::from_str_radix(parts[1], 16).ok()?;
    let dev_func: Vec<&str> = parts[2].split('.').collect();
    if dev_func.len() != 2 {
        return None;
    }
    let dev = u8::from_str_radix(dev_func[0], 16).ok()?;
    let func = u8::from_str_radix(dev_func[1], 16).ok()?;
    Some(interfaces::PciAddress {
        domain,
        bus,
        dev,
        func,
    })
}
