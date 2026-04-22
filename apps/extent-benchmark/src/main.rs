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
use extent_manager_v2::MetadataManagerV2;
use interfaces::{DmaBuffer, FormatParams};
use spdk_env::SPDKEnvComponent;

use config::BenchmarkConfig;

fn main() {
    let config = BenchmarkConfig::parse();

    if let Err(msg) = validate_config(&config) {
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    let spdk_env_comp = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();
    let extent_mgr = MetadataManagerV2::new_inner();

    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env").unwrap_or_else(|e| {
        eprintln!("error: bind spdk_env→block_dev: {e}");
        std::process::exit(2);
    });
    bind(&*block_dev, "IBlockDevice", &*extent_mgr, "block_device").unwrap_or_else(|e| {
        eprintln!("error: bind block_dev→extent_mgr: {e}");
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

    let devices = ienv.devices();
    if devices.is_empty() {
        eprintln!("error: no NVMe devices found");
        std::process::exit(2);
    }

    let target = parse_pci_addr(&config.device).unwrap_or_else(|| {
        eprintln!("error: invalid PCI address format: {}", config.device);
        std::process::exit(1);
    });

    let _device = devices
        .iter()
        .find(|d| {
            d.address.domain == target.domain
                && d.address.bus == target.bus
                && d.address.dev == target.dev
                && d.address.func == target.func
        })
        .unwrap_or_else(|| {
            eprintln!("error: no NVMe device found at {}", config.device);
            std::process::exit(1);
        });

    let admin = query::<dyn interfaces::IBlockDeviceAdmin + Send + Sync>(&*block_dev)
        .unwrap_or_else(|| {
            eprintln!("error: failed to query IBlockDeviceAdmin");
            std::process::exit(2);
        });

    admin.set_pci_address(target);

    if let Err(e) = admin.initialize() {
        eprintln!("error: block device init failed: {e}");
        std::process::exit(2);
    }

    let ibd =
        query::<dyn interfaces::IBlockDevice + Send + Sync>(&*block_dev).unwrap_or_else(|| {
            eprintln!("error: failed to query IBlockDevice");
            std::process::exit(2);
        });

    let iem =
        query::<dyn interfaces::IExtentManager + Send + Sync>(&*extent_mgr).unwrap_or_else(|| {
            eprintln!("error: failed to query IExtentManager");
            std::process::exit(2);
        });

    let numa_node = ibd.numa_node();
    let dma_alloc: interfaces::DmaAllocFn = Arc::new(move |size, align, _numa| {
        DmaBuffer::new(size, align, Some(numa_node)).map_err(|e| e.to_string())
    });

    iem.set_dma_alloc(dma_alloc);

    let total_size = config.total_size.unwrap_or_else(|| {
        let sectors = ibd.num_sectors(config.ns_id).unwrap_or_else(|e| {
            eprintln!("error: failed to get num_sectors: {e}");
            std::process::exit(2);
        });
        let sector_size = ibd.sector_size(config.ns_id).unwrap_or_else(|e| {
            eprintln!("error: failed to get sector_size: {e}");
            std::process::exit(2);
        });
        sectors * sector_size as u64
    });

    let params = FormatParams {
        slab_size: config.slab_size,
        max_element_size: config.size_class,
        metadata_block_size: config.slab_size.min(131072),
        sector_size: 4096,
        region_count: 32,
    };
    if let Err(e) = iem.format(params) {
        eprintln!("error: extent manager format failed: {e}");
        std::process::exit(2);
    }

    report::print_header(&config, total_size);

    let count = config.count;
    let size_class = config.size_class;
    let threads = config.threads;

    if threads == 1 {
        run_single_threaded(&iem, count, size_class);
    } else {
        run_multi_threaded(&iem, count, size_class, threads);
    }

    report::print_summary(count);
}

fn run_single_threaded(
    iem: &Arc<dyn interfaces::IExtentManager + Send + Sync>,
    count: u64,
    size_class: u32,
) {
    for (phase_name, phase_fn) in [
        (
            "Create",
            run_create
                as fn(&dyn interfaces::IExtentManager, interfaces::ExtentKey, u64, u32) -> Vec<std::time::Duration>,
        ),
        (
            "Lookup",
            run_lookup
                as fn(&dyn interfaces::IExtentManager, interfaces::ExtentKey, u64, u32) -> Vec<std::time::Duration>,
        ),
        (
            "Remove",
            run_remove
                as fn(&dyn interfaces::IExtentManager, interfaces::ExtentKey, u64, u32) -> Vec<std::time::Duration>,
        ),
    ] {
        let start = Instant::now();
        let latencies = phase_fn(&**iem, 0, count, size_class);
        let elapsed = start.elapsed();
        let result = worker::aggregate_results(phase_name, vec![(0, latencies)], elapsed);
        report::print_phase(&result);
    }
}

fn run_multi_threaded(
    iem: &Arc<dyn interfaces::IExtentManager + Send + Sync>,
    count: u64,
    size_class: u32,
    threads: usize,
) {
    let key_ranges = compute_key_ranges(count, threads);

    for (phase_name, phase_id) in [("Create", 0u8), ("Lookup", 1), ("Remove", 2)] {
        let barrier = Arc::new(Barrier::new(threads));

        let start = Instant::now();
        let handles: Vec<_> = key_ranges
            .iter()
            .enumerate()
            .map(|(tid, &(key_start, key_count))| {
                let barrier = Arc::clone(&barrier);
                let iem = Arc::clone(iem);
                std::thread::spawn(move || {
                    barrier.wait();
                    let latencies = match phase_id {
                        0 => run_create(&*iem, key_start, key_count, size_class),
                        1 => run_lookup(&*iem, key_start, key_count, size_class),
                        _ => run_remove(&*iem, key_start, key_count, size_class),
                    };
                    (tid, latencies)
                })
            })
            .collect();

        let mut worker_latencies = Vec::with_capacity(threads);
        for h in handles {
            match h.join() {
                Ok(r) => worker_latencies.push(r),
                Err(_) => {
                    eprintln!("error: worker thread panicked in {phase_name} phase");
                    std::process::exit(2);
                }
            }
        }
        let elapsed = start.elapsed();

        worker_latencies.sort_by_key(|(tid, _)| *tid);
        let result = worker::aggregate_results(phase_name, worker_latencies, elapsed);
        report::print_phase(&result);
    }
}

fn compute_key_ranges(total_count: u64, num_threads: usize) -> Vec<(u64, u64)> {
    let per_thread = total_count / num_threads as u64;
    let remainder = total_count % num_threads as u64;
    let mut ranges = Vec::with_capacity(num_threads);
    let mut offset = 0u64;
    for i in 0..num_threads {
        let count = per_thread + if (i as u64) < remainder { 1 } else { 0 };
        ranges.push((offset, count));
        offset += count;
    }
    ranges
}

fn run_create(
    iem: &dyn interfaces::IExtentManager,
    key_start: interfaces::ExtentKey,
    count: u64,
    size_class: u32,
) -> Vec<std::time::Duration> {
    let mut latencies = Vec::with_capacity(count as usize);
    for i in 0..count {
        let key = key_start + i;
        let start = Instant::now();
        match iem.reserve_extent(key, size_class) {
            Ok(handle) => {
                if let Err(e) = handle.publish() {
                    eprintln!("  publish({key}) failed: {e}");
                }
            }
            Err(e) => {
                eprintln!("  reserve_extent({key}) failed: {e}");
            }
        }
        latencies.push(start.elapsed());
    }
    latencies
}

fn run_lookup(
    iem: &dyn interfaces::IExtentManager,
    key_start: interfaces::ExtentKey,
    count: u64,
    _size_class: u32,
) -> Vec<std::time::Duration> {
    let mut latencies = Vec::with_capacity(count as usize);
    for i in 0..count {
        let key = key_start + i;
        let start = Instant::now();
        match iem.lookup_extent(key) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("  lookup_extent({key}) failed: {e}");
            }
        }
        latencies.push(start.elapsed());
    }
    latencies
}

fn run_remove(
    iem: &dyn interfaces::IExtentManager,
    key_start: interfaces::ExtentKey,
    count: u64,
    _size_class: u32,
) -> Vec<std::time::Duration> {
    let mut latencies = Vec::with_capacity(count as usize);
    for i in 0..count {
        let key = key_start + i;
        let start = Instant::now();
        match iem.remove_extent(key) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("  remove_extent({key}) failed: {e}");
            }
        }
        latencies.push(start.elapsed());
    }
    latencies
}

fn validate_config(config: &BenchmarkConfig) -> Result<(), String> {
    if config.size_class < 131072 || config.size_class > 5 * 1024 * 1024 {
        return Err(format!(
            "size-class must be between 131072 (128 KiB) and 5242880 (5 MiB), got {}",
            config.size_class
        ));
    }
    if config.size_class % 4096 != 0 {
        return Err(format!(
            "size-class must be a multiple of 4096, got {}",
            config.size_class
        ));
    }
    if config.slab_size < 8192 {
        return Err(format!(
            "slab-size must be at least 8192 (8 KiB), got {}",
            config.slab_size
        ));
    }
    if config.slab_size % 4096 != 0 {
        return Err(format!(
            "slab-size must be a multiple of 4096, got {}",
            config.slab_size
        ));
    }
    if let Some(total) = config.total_size {
        if total <= config.slab_size as u64 {
            return Err(format!(
                "total-size ({total}) must be greater than slab-size ({})",
                config.slab_size
            ));
        }
    }
    if config.threads == 0 {
        return Err("threads must be >= 1".to_string());
    }
    if config.count == 0 {
        return Err("count must be >= 1".to_string());
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
