//! Multi-device NVMe IOPS benchmark using the block-device-spdk-nvme component.
//!
//! Supports multiple block devices with client threads distributed across them.
//! Each actor thread and each client thread is pinned to a distinct CPU core.
//! Run with `--help` for usage information.

#![allow(clippy::arc_with_non_send_sync)]

mod config;
mod lba;
mod report;
mod stats;
mod worker;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use clap::Parser;

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponent;
use component_core::binding::bind;
use component_core::iunknown::query;
use component_core::numa::{get_thread_affinity, set_thread_affinity, CpuSet, NumaTopology};
use interfaces::{Command, Completion, IBlockDevice, IBlockDeviceAdmin, NamespaceInfo};
use spdk_env::SPDKEnvComponent;

use config::BenchConfig;
use stats::FinalReport;

/// Per-device state after initialization.
pub struct DeviceContext {
    pub pci_addr_str: String,
    pub ibd: Arc<dyn IBlockDevice + Send + Sync>,
    pub admin: Arc<dyn IBlockDeviceAdmin + Send + Sync>,
    pub ns_info: NamespaceInfo,
    pub numa_node: i32,
    pub actor_cpu: usize,
}

/// Allocate CPUs for actors and workers from available NUMA-local cores.
///
/// Returns `(actor_cpus, worker_cpus)` where each actor and worker gets a
/// distinct core. If there are not enough cores for one-per-thread, workers
/// wrap around the available pool (with a warning).
fn plan_cpu_assignment(
    num_devices: usize,
    num_threads: u32,
    numa_nodes: &[i32],
    topo: &Option<NumaTopology>,
) -> (Vec<usize>, Vec<usize>) {
    // Collect all unique CPUs across the relevant NUMA nodes.
    let mut all_cpus = BTreeSet::new();
    for &node in numa_nodes {
        if node >= 0 {
            if let Some(ref t) = topo {
                if let Some(n) = t.node(node as usize) {
                    for cpu in n.cpus().iter() {
                        all_cpus.insert(cpu);
                    }
                }
            }
        }
    }

    let mut available: Vec<usize> = all_cpus.into_iter().collect();
    if available.is_empty() {
        return (vec![], vec![]);
    }

    // Reserve one CPU per device for its actor thread.
    let mut actor_cpus = Vec::with_capacity(num_devices);
    for _ in 0..num_devices {
        if available.is_empty() {
            break;
        }
        actor_cpus.push(available.remove(0));
    }

    // Assign remaining CPUs to worker threads.
    let mut worker_cpus = Vec::with_capacity(num_threads as usize);
    if available.is_empty() {
        eprintln!("warning: no CPUs left for worker pinning after reserving actor cores");
    } else {
        let needed = num_threads as usize;
        if needed > available.len() {
            eprintln!(
                "warning: {} worker threads but only {} CPUs available — some cores will be shared",
                needed,
                available.len()
            );
        }
        for i in 0..needed {
            worker_cpus.push(available[i % available.len()]);
        }
    }

    (actor_cpus, worker_cpus)
}

fn main() {
    let mut config = BenchConfig::parse();
    config.resolve_num_devices();

    // Ensure at least one worker thread per device.
    if config.threads < config.num_devices {
        eprintln!(
            "info: scaling --threads from {} to {} (one per device minimum)",
            config.threads, config.num_devices
        );
        config.threads = config.num_devices;
    }

    if !config.pci_addrs.is_empty() && config.pci_addrs.len() != config.num_devices as usize {
        eprintln!(
            "error: --pci-addrs specifies {} addresses but --num-devices is {}",
            config.pci_addrs.len(),
            config.num_devices
        );
        std::process::exit(1);
    }

    // --- Initialize SPDK environment (singleton, shared across all devices) ---
    // Save the main thread's CPU affinity before SPDK/DPDK init, which
    // restricts the calling thread to the master lcore.  Restoring it
    // afterwards ensures spawned threads inherit an unrestricted mask.
    let saved_affinity = get_thread_affinity().ok();

    let spdk_env_comp = SPDKEnvComponent::new_default();

    let ienv =
        query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp).unwrap_or_else(|| {
            eprintln!("error: failed to query ISPDKEnv");
            std::process::exit(2);
        });
    if let Err(e) = ienv.init() {
        eprintln!("error: SPDK init failed: {e}");
        std::process::exit(2);
    }

    // Restore the main thread's CPU affinity after SPDK/DPDK init.
    if let Some(ref affinity) = saved_affinity {
        if let Err(e) = set_thread_affinity(affinity) {
            eprintln!("warning: failed to restore main thread affinity: {e}");
        }
    }

    let devices = ienv.devices();
    if devices.is_empty() {
        eprintln!("error: no NVMe devices found");
        std::process::exit(2);
    }

    if config.pci_addrs.is_empty() && (config.num_devices as usize) > devices.len() {
        eprintln!(
            "error: requested {} devices but only {} available",
            config.num_devices,
            devices.len()
        );
        std::process::exit(2);
    }

    // --- Select devices ---
    let selected_devices: Vec<&spdk_env::VfioDevice> = if config.pci_addrs.is_empty() {
        devices.iter().take(config.num_devices as usize).collect()
    } else {
        config
            .pci_addrs
            .iter()
            .map(|addr_str| {
                let target = parse_pci_addr(addr_str).unwrap_or_else(|| {
                    eprintln!("error: invalid PCI address format: {addr_str}");
                    std::process::exit(1);
                });
                devices
                    .iter()
                    .find(|d| {
                        d.address.domain == target.domain
                            && d.address.bus == target.bus
                            && d.address.dev == target.dev
                            && d.address.func == target.func
                    })
                    .unwrap_or_else(|| {
                        eprintln!("error: no NVMe device found at PCI address {addr_str}");
                        std::process::exit(2);
                    })
            })
            .collect()
    };

    let num_devices = selected_devices.len();

    // --- Plan CPU assignment ---
    // All devices currently report NUMA node 0 (hardcoded in probe_controller),
    // but we still read each device's advertised node for future correctness.
    let numa_nodes: Vec<i32> = vec![0; num_devices]; // pre-init placeholder
    let topo = NumaTopology::discover().ok();
    let (actor_cpus, worker_cpus) =
        plan_cpu_assignment(num_devices, config.threads, &numa_nodes, &topo);

    // --- Create and initialize one block device component per device ---
    let mut device_contexts: Vec<DeviceContext> = Vec::with_capacity(num_devices);

    for (dev_idx, vfio_dev) in selected_devices.iter().enumerate() {
        let block_dev: Arc<dyn component_core::IUnknown> =
            BlockDeviceSpdkNvmeComponent::new_default();

        bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env").unwrap_or_else(|e| {
            eprintln!("error: failed to bind spdk_env for device {dev_idx}: {e}");
            std::process::exit(2);
        });

        let admin = query::<dyn interfaces::IBlockDeviceAdmin + Send + Sync>(&*block_dev)
            .unwrap_or_else(|| {
                eprintln!("error: failed to query IBlockDeviceAdmin for device {dev_idx}");
                std::process::exit(2);
            });

        admin.set_pci_address(interfaces::PciAddress {
            domain: vfio_dev.address.domain,
            bus: vfio_dev.address.bus,
            dev: vfio_dev.address.dev,
            func: vfio_dev.address.func,
        });

        // Pin actor to its dedicated CPU.
        let actor_cpu = if dev_idx < actor_cpus.len() {
            admin.set_actor_cpu(actor_cpus[dev_idx]);
            actor_cpus[dev_idx]
        } else {
            0
        };

        if let Err(e) = admin.initialize() {
            eprintln!("error: block device {dev_idx} init failed: {e}");
            std::process::exit(2);
        }

        let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap_or_else(|| {
            eprintln!("error: failed to query IBlockDevice for device {dev_idx}");
            std::process::exit(2);
        });

        let admin =
            query::<dyn IBlockDeviceAdmin + Send + Sync>(&*block_dev).unwrap_or_else(|| {
                eprintln!("error: failed to query IBlockDeviceAdmin for device {dev_idx}");
                std::process::exit(2);
            });

        // Probe namespaces.
        let probe_channels = ibd.connect_client().unwrap_or_else(|e| {
            eprintln!("error: failed to connect probe client for device {dev_idx}: {e}");
            std::process::exit(2);
        });

        probe_channels
            .command_tx
            .send(Command::NsProbe)
            .unwrap_or_else(|e| {
                eprintln!("error: failed to send NsProbe for device {dev_idx}: {e}");
                std::process::exit(2);
            });

        let namespaces = match probe_channels.completion_rx.recv() {
            Ok(Completion::NsProbeResult { namespaces }) => namespaces,
            Ok(other) => {
                eprintln!(
                    "error: unexpected completion from NsProbe on device {dev_idx}: {other:?}"
                );
                std::process::exit(2);
            }
            Err(e) => {
                eprintln!("error: failed to recv NsProbe result for device {dev_idx}: {e}");
                std::process::exit(2);
            }
        };

        if namespaces.is_empty() {
            eprintln!("error: no active namespaces on device {dev_idx}");
            std::process::exit(2);
        }

        drop(probe_channels);

        let ns_id = config.ns_id_for_device(dev_idx);
        let ns_info = namespaces
            .iter()
            .find(|ns| ns.ns_id == ns_id)
            .cloned()
            .unwrap_or_else(|| {
                let available: Vec<u32> = namespaces.iter().map(|ns| ns.ns_id).collect();
                eprintln!(
                    "error: namespace {} not found on device {dev_idx} (available: {:?})",
                    ns_id, available
                );
                std::process::exit(1);
            });

        if let Err(msg) = config.validate(
            ns_info.sector_size,
            ibd.max_queue_depth(),
            &namespaces,
            ns_id,
        ) {
            eprintln!("error: device {dev_idx}: {msg}");
            std::process::exit(1);
        }
        config.clamp_queue_depth(ibd.max_queue_depth());

        let pci_addr_str = format!("{}", vfio_dev.address);
        let numa_node = ibd.numa_node();

        device_contexts.push(DeviceContext {
            pci_addr_str,
            ibd,
            admin: Arc::clone(&admin),
            ns_info,
            numa_node,
            actor_cpu,
        });
    }

    // Sanity check: all PCI addresses must be distinct.
    {
        let addrs: Vec<&str> = device_contexts
            .iter()
            .map(|c| c.pci_addr_str.as_str())
            .collect();
        let unique: BTreeSet<&str> = addrs.iter().copied().collect();
        if unique.len() != addrs.len() {
            eprintln!("WARNING: duplicate PCI addresses detected — devices may alias the same controller!");
            for a in &addrs {
                eprintln!("  {a}");
            }
        }
    }

    // --- Print config and CPU plan ---
    report::print_config(&config, &device_contexts);
    println!();

    // Print CPU assignment plan.
    eprintln!(
        "info: CPU assignment plan ({} actors + {} workers):",
        num_devices, config.threads
    );
    for (dev_idx, ctx) in device_contexts.iter().enumerate() {
        eprintln!(
            "  device {} ({}) actor -> CPU {}",
            dev_idx, ctx.pci_addr_str, ctx.actor_cpu,
        );
    }
    if !worker_cpus.is_empty() {
        let cpu_strs: Vec<String> = worker_cpus.iter().map(|c| c.to_string()).collect();
        eprintln!("  workers -> CPUs [{}]", cpu_strs.join(", "));
    }

    // Print thread-to-device mapping so imbalances are obvious.
    let mut threads_per_dev = vec![0u32; num_devices];
    for t in 0..config.threads as usize {
        threads_per_dev[t % num_devices] += 1;
    }
    let dist: Vec<String> = threads_per_dev
        .iter()
        .enumerate()
        .map(|(d, &n)| format!("D{}:{}", d, n))
        .collect();
    eprintln!("  thread distribution: {}", dist.join(", "));

    // --- Compute thread-to-device mapping (round-robin) ---
    let thread_device_map: Vec<usize> = (0..config.threads as usize)
        .map(|t| t % num_devices)
        .collect();

    // Count threads per device for sequential LBA partitioning.
    let mut threads_per_device = vec![0u32; num_devices];
    for &dev_idx in &thread_device_map {
        threads_per_device[dev_idx] += 1;
    }

    // Track per-device thread index for sequential LBA region assignment.
    let mut device_thread_counter = vec![0u32; num_devices];

    // --- Launch workers ---
    let stop_flag = Arc::new(AtomicBool::new(false));
    let config_arc = Arc::new(config.clone());

    let mut worker_handles = Vec::with_capacity(config.threads as usize);
    let mut op_counters = Vec::with_capacity(config.threads as usize);
    let mut byte_counters = Vec::with_capacity(config.threads as usize);
    let mut thread_device_assignments = Vec::with_capacity(config.threads as usize);

    for thread_idx in 0..config.threads {
        let dev_idx = thread_device_map[thread_idx as usize];
        let ctx = &device_contexts[dev_idx];
        let thread_index_on_device = device_thread_counter[dev_idx];
        device_thread_counter[dev_idx] += 1;

        let channels = ctx.ibd.connect_client().unwrap_or_else(|e| {
            eprintln!("error: failed to connect worker client {thread_idx}: {e}");
            std::process::exit(2);
        });

        let op_counter = Arc::new(AtomicU64::new(0));
        op_counters.push(Arc::clone(&op_counter));

        let byte_counter = Arc::new(AtomicU64::new(0));
        byte_counters.push(Arc::clone(&byte_counter));

        thread_device_assignments.push(dev_idx);

        let worker_config = Arc::clone(&config_arc);
        let worker_stop = Arc::clone(&stop_flag);
        let worker_ns_info = ctx.ns_info.clone();
        let worker_numa_node = ctx.numa_node;
        let ns_id = config.ns_id_for_device(dev_idx);
        let tpd = threads_per_device[dev_idx];

        // Each worker gets its own dedicated CPU from the pre-computed plan.
        let pin_cpu = if (thread_idx as usize) < worker_cpus.len() {
            Some(worker_cpus[thread_idx as usize])
        } else {
            None
        };

        let handle = std::thread::spawn(move || {
            if let Some(cpu) = pin_cpu {
                if let Ok(cs) = CpuSet::from_cpu(cpu) {
                    if let Err(e) = set_thread_affinity(&cs) {
                        eprintln!(
                            "warning: worker {thread_idx} (dev {dev_idx}) failed to pin to CPU {cpu}: {e}"
                        );
                    }
                }
            }

            let mut w = worker::Worker::new(worker::WorkerParams {
                config: worker_config,
                channels,
                ns_info: worker_ns_info,
                op_counter,
                byte_counter,
                stop_flag: worker_stop,
                thread_index_on_device,
                threads_on_device: tpd,
                device_idx: dev_idx,
                ns_id,
                numa_node: worker_numa_node,
            })
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

            report::print_progress(
                elapsed,
                total_iops,
                &per_thread_iops,
                &thread_device_assignments,
                num_devices,
                mbps,
            );
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
    report::print_final(&report, config.op, &results, &device_contexts, num_devices);

    // Explicit shutdown: instruct each block device to stop its actor
    // and join the actor thread before we tear down the SPDK environment.
    for ctx in &device_contexts {
        if let Err(e) = ctx.admin.shutdown() {
            eprintln!(
                "warning: failed to shutdown device {}: {e}",
                ctx.pci_addr_str
            );
        }
    }

    // Now it's safe to drop device contexts and the SPDK environment.
    drop(device_contexts);
    drop(ienv);
    drop(spdk_env_comp);
}

/// Parse a PCI BDF address string like "0000:03:00.0" into components.
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
