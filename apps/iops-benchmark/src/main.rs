//! NVMe IOPS benchmark using the block-device-spdk-nvme component.
//!
//! Measures read/write IOPS, throughput (MB/s), and latency percentiles
//! (min, mean, p50, p99, max) for NVMe devices via SPDK userspace drivers.
//!
//! Run with `--help` for usage information.

// DmaBuffer is Send but not Sync; Arc<DmaBuffer> is required by Command::WriteAsync API.
#![allow(clippy::arc_with_non_send_sync)]

mod config;
mod lba;
mod report;
mod stats;
mod worker;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use clap::Parser;

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::iunknown::query;
use component_core::numa::{set_thread_affinity, CpuSet, NumaTopology};
use interfaces::{Command, Completion, IBlockDevice, NamespaceInfo, PciAddress};
use spdk_env::{SPDKEnvComponent, VfioDevice};

use config::BenchConfig;
use stats::FinalReport;

/// Per-device state for multi-device benchmarking.
struct DeviceState {
    /// Admin interface to this device.
    admin: Arc<dyn interfaces::IBlockDeviceAdmin + Send + Sync>,
    /// Block device interface.
    ibd: Arc<dyn IBlockDevice + Send + Sync>,
    /// PCI address of this device.
    pci_addr: PciAddress,
    /// PCI address as formatted string.
    pci_addr_str: String,
    /// Namespace information for this device.
    namespaces: Vec<NamespaceInfo>,
}

fn main() {
    let mut config = BenchConfig::parse();

    // --- Component wiring & SPDK init ---
    let spdk_env_comp = SPDKEnvComponent::new_default();

    let ienv = initialize_spdk(&spdk_env_comp);

    // --- Enumerate devices ---
    let available_devices = ienv.devices();
    if available_devices.is_empty() {
        eprintln!("error: no NVMe devices found");
        std::process::exit(2);
    }

    // --- Select devices ---
    let selected_devices: Vec<&_> = if let Some(ref addr_str) = config.pci_addr {
        // When a specific PCI address is given, use only that device
        let target = match parse_pci_addr(addr_str) {
            Some(t) => t,
            None => {
                eprintln!("error: invalid PCI address format: {addr_str}");
                std::process::exit(1);
            }
        };
        let dev = match available_devices.iter().find(|d| {
            d.address.domain == target.domain
                && d.address.bus == target.bus
                && d.address.dev == target.dev
                && d.address.func == target.func
        }) {
            Some(d) => d,
            None => {
                eprintln!("error: no NVMe device found at PCI address {addr_str}");
                std::process::exit(2);
            }
        };
        vec![dev]
    } else {
        // Use first N devices
        let count = (config.device_count as usize).min(available_devices.len());
        available_devices.iter().take(count).collect()
    };

    if selected_devices.is_empty() {
        eprintln!("error: no devices selected");
        std::process::exit(2);
    }

    // --- Initialize devices ---
    let mut device_states: Vec<DeviceState> = Vec::new();
    for (dev_idx, device) in selected_devices.iter().enumerate() {
        match initialize_device(&spdk_env_comp, device) {
            Ok(state) => {
                if dev_idx == 0 {
                    // Print config only for first device
                    report::print_config(
                        &config,
                        &state.pci_addr_str,
                        &state
                            .namespaces
                            .iter()
                            .find(|ns| ns.ns_id == config.ns_id)
                            .cloned()
                            .unwrap(),
                    );
                    println!();
                }
                device_states.push(state);
            }
            Err(e) => {
                eprintln!("error: failed to initialize device {}: {}", dev_idx, e);
                std::process::exit(2);
            }
        }
    }

    // --- Validate config against first device ---
    let first_state = &device_states[0];
    let ns_info = first_state
        .namespaces
        .iter()
        .find(|ns| ns.ns_id == config.ns_id)
        .cloned()
        .unwrap_or_else(|| {
            let available: Vec<u32> = first_state.namespaces.iter().map(|ns| ns.ns_id).collect();
            eprintln!(
                "error: namespace {} not found (available: {:?})",
                config.ns_id, available
            );
            std::process::exit(1);
        });

    if let Err(msg) = config.validate(
        ns_info.sector_size,
        first_state.ibd.max_queue_depth(),
        &first_state.namespaces,
    ) {
        eprintln!("error: {msg}");
        std::process::exit(1);
    }
    config.clamp_queue_depth(first_state.ibd.max_queue_depth());

    // --- Discover NUMA topology for CPU pinning ---
    let numa_node = first_state.ibd.numa_node();
    let (_, worker_cpus) = if numa_node >= 0 {
        NumaTopology::discover()
            .ok()
            .and_then(|topo| {
                topo.node(numa_node as usize)
                    .map(|n| n.cpus().iter().collect::<Vec<_>>())
            })
            .map(|cpus| {
                let actor = cpus.first().copied();
                let workers = if cpus.len() > 1 {
                    cpus[1..].to_vec()
                } else {
                    vec![]
                };
                (actor, workers)
            })
            .unwrap_or((None, vec![]))
    } else {
        (None, vec![])
    };

    if !worker_cpus.is_empty() && device_states.len() == 1 {
        eprintln!(
            "info: pinning {} worker(s) to NUMA-{} CPUs {:?}",
            config.threads, numa_node, &worker_cpus,
        );
    }

    // --- Launch workers ---
    let stop_flag = Arc::new(AtomicBool::new(false));
    let config_arc = Arc::new(config.clone());
    let mut worker_handles = Vec::with_capacity(config.threads as usize);
    let mut op_counters = Vec::with_capacity(config.threads as usize);
    let mut byte_counters = Vec::with_capacity(config.threads as usize);

    for thread_idx in 0..config.threads {
        // Distribute workers round-robin across devices
        let device_idx = (thread_idx as usize) % device_states.len();
        let ibd = Arc::clone(&device_states[device_idx].ibd);

        let channels = ibd.connect_client().unwrap_or_else(|e| {
            eprintln!("error: failed to connect worker client {thread_idx}: {e}");
            std::process::exit(2);
        });

        let op_counter = Arc::new(AtomicU64::new(0));
        op_counters.push(Arc::clone(&op_counter));

        let byte_counter = Arc::new(AtomicU64::new(0));
        byte_counters.push(Arc::clone(&byte_counter));

        let worker_config = Arc::clone(&config_arc);
        let worker_stop = Arc::clone(&stop_flag);
        let worker_ns_info = ns_info.clone();

        let worker_cpus_clone = worker_cpus.clone();
        let handle = std::thread::spawn(move || {
            // Pin this worker to a NUMA-local core (round-robin).
            if !worker_cpus_clone.is_empty() {
                let cpu = worker_cpus_clone[thread_idx as usize % worker_cpus_clone.len()];
                if let Ok(cs) = CpuSet::from_cpu(cpu) {
                    let _ = set_thread_affinity(&cs);
                }
            }

            let mut w = worker::Worker::new(
                worker_config,
                channels,
                worker_ns_info,
                op_counter,
                byte_counter,
                worker_stop,
                thread_idx,
            )
            .unwrap_or_else(|e| {
                eprintln!("error: worker {thread_idx} init failed: {e}");
                std::process::exit(2);
            });

            w.run()
        });

        worker_handles.push(handle);
    }

    // --- Timer + progress reporter ---
    let bench_start = Instant::now();

    let timer_stop = Arc::clone(&stop_flag);
    let duration_secs = config.duration;
    let timer_start = bench_start;
    let timer_handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(duration_secs));
        let elapsed = timer_start.elapsed().as_secs_f64();
        timer_stop.store(true, Ordering::Relaxed);
        elapsed
    });

    // Progress reporting on main thread.
    if !config.quiet {
        let mut prev_op_counts: Vec<u64> = vec![0; op_counters.len()];
        let mut prev_byte_count: u64 = 0;
        let mut elapsed = 0u64;

        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            elapsed += 1;

            let per_thread_iops: Vec<u64> = op_counters
                .iter()
                .zip(prev_op_counts.iter_mut())
                .map(|(counter, prev)| {
                    let current = counter.load(Ordering::Relaxed);
                    let delta = current - *prev;
                    *prev = current;
                    delta
                })
                .collect();
            let total_iops: u64 = per_thread_iops.iter().sum();

            let current_bytes: u64 = byte_counters
                .iter()
                .map(|c| c.load(Ordering::Relaxed))
                .sum();
            let delta_bytes = current_bytes - prev_byte_count;
            prev_byte_count = current_bytes;
            let mbps = delta_bytes as f64 / 1_048_576.0;

            report::print_progress(elapsed, total_iops, &per_thread_iops, mbps);
        }
    }

    // --- Join all threads ---
    let actual_duration = timer_handle.join().expect("timer thread panicked");

    let mut results = Vec::with_capacity(worker_handles.len());
    for handle in worker_handles {
        match handle.join() {
            Ok(thread_result) => results.push(thread_result),
            Err(_) => {
                eprintln!("error: worker thread panicked");
                std::process::exit(2);
            }
        }
    }

    // --- Report ---
    println!();
    let report = FinalReport::from_results(&results, actual_duration);
    report::print_final(&report, config.op, &results);

    if device_states.len() > 1 {
        println!("\n=== Per-Device Summary ===");
        for (idx, _state) in device_states.iter().enumerate() {
            let device_results: Vec<_> = results
                .iter()
                .enumerate()
                .filter(|(t, _)| (t % device_states.len()) == idx)
                .map(|(_, r)| r)
                .collect();
            if !device_results.is_empty() {
                let total_ops: u64 = device_results
                    .iter()
                    .map(|r| r.read_ops + r.write_ops)
                    .sum();
                let total_iops = if actual_duration > 0.0 {
                    total_ops as f64 / actual_duration
                } else {
                    0.0
                };
                let total_bytes: u64 = device_results.iter().map(|r| r.total_bytes).sum();
                let throughput_mbps = if actual_duration > 0.0 {
                    total_bytes as f64 / actual_duration / 1_048_576.0
                } else {
                    0.0
                };
                println!(
                    "\nDevice {} ({}: {:.0} IOPS, {:.1} MB/s",
                    idx, &device_states[idx].pci_addr_str, total_iops, throughput_mbps
                );
            }
        }
    }

    // Shutdown all devices
    for state in device_states {
        let _ = state.admin.shutdown();
    }
}

/// Initialize SPDK environment.
fn initialize_spdk(spdk_env_comp: &SPDKEnvComponent) -> Arc<dyn spdk_env::ISPDKEnv + Send + Sync> {
    let block_dev: Arc<dyn component_core::IUnknown> = BlockDeviceSpdkNvmeComponent::new_default();
    bind(spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env").unwrap_or_else(|e| {
        eprintln!("error: failed to bind spdk_env: {e}");
        std::process::exit(2);
    });

    let ienv = query::<dyn spdk_env::ISPDKEnv + Send + Sync>(spdk_env_comp).unwrap_or_else(|| {
        eprintln!("error: failed to query ISPDKEnv");
        std::process::exit(2);
    });

    if let Err(e) = ienv.init() {
        eprintln!("error: SPDK init failed: {e}");
        std::process::exit(2);
    }

    ienv
}

/// Initialize a single NVMe device.
fn initialize_device(
    spdk_env_comp: &SPDKEnvComponent,
    device: &VfioDevice,
) -> Result<DeviceState, String> {
    // Create a fresh component for this device
    let block_dev: Arc<dyn component_core::IUnknown> = BlockDeviceSpdkNvmeComponent::new_default();

    bind(spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env")
        .map_err(|e| format!("failed to bind spdk_env: {e}"))?;

    let admin = query::<dyn interfaces::IBlockDeviceAdmin + Send + Sync>(&*block_dev)
        .ok_or("failed to query IBlockDeviceAdmin")?;

    admin.set_pci_address(interfaces::PciAddress {
        domain: device.address.domain,
        bus: device.address.bus,
        dev: device.address.dev,
        func: device.address.func,
    });

    admin
        .initialize()
        .map_err(|e| format!("block device init failed: {e}"))?;

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev)
        .ok_or("failed to query IBlockDevice")?;

    // Probe namespaces
    let probe_channels = ibd
        .connect_client()
        .map_err(|e| format!("failed to connect probe client: {e}"))?;

    probe_channels
        .command_tx
        .send(Command::NsProbe)
        .map_err(|e| format!("failed to send NsProbe: {e}"))?;

    let namespaces = match probe_channels.completion_rx.recv() {
        Ok(Completion::NsProbeResult { namespaces }) => namespaces,
        Ok(other) => {
            return Err(format!("unexpected completion from NsProbe: {other:?}"));
        }
        Err(e) => {
            return Err(format!("failed to recv NsProbe result: {e}"));
        }
    };

    if namespaces.is_empty() {
        return Err("no active namespaces on device".to_string());
    }

    drop(probe_channels);

    let pci_addr_str = format!("{}", device.address);

    Ok(DeviceState {
        admin,
        ibd,
        pci_addr: interfaces::PciAddress {
            domain: device.address.domain,
            bus: device.address.bus,
            dev: device.address.dev,
            func: device.address.func,
        },
        pci_addr_str,
        namespaces,
    })
}

/// Parse a PCI BDF address string like "0000:03:00.0" into components.
fn parse_pci_addr(s: &str) -> Option<interfaces::PciAddress> {
    // Format: DDDD:BB:DD.F
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
