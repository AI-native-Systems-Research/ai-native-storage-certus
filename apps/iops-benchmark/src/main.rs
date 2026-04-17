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

use block_device_spdk_nvme::{BlockDeviceSpdkNvmeComponentV1, Command, Completion, IBlockDevice};
use component_core::binding::bind;
use component_core::iunknown::query;
use component_core::numa::{set_thread_affinity, CpuSet, NumaTopology};
use console_logger::LoggerComponent;
use spdk_env::SPDKEnvComponent;

use config::BenchConfig;
use stats::FinalReport;

fn main() {
    let mut config = BenchConfig::parse();

    // --- Component wiring (same pattern as benches/latency.rs) ---
    let logger = LoggerComponent::new();
    let spdk_env_comp = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();

    bind(&*logger, "ILogger", &*block_dev, "logger").unwrap_or_else(|e| {
        eprintln!("error: failed to bind logger: {e}");
        std::process::exit(2);
    });
    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env").unwrap_or_else(|e| {
        eprintln!("error: failed to bind spdk_env: {e}");
        std::process::exit(2);
    });
    bind(&*logger, "ILogger", &*spdk_env_comp, "logger").unwrap_or_else(|e| {
        eprintln!("error: failed to bind logger to spdk_env: {e}");
        std::process::exit(2);
    });

    // --- Initialize SPDK environment ---
    let ienv =
        query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp).unwrap_or_else(|| {
            eprintln!("error: failed to query ISPDKEnv");
            std::process::exit(2);
        });
    if let Err(e) = ienv.init() {
        eprintln!("error: SPDK init failed: {e}");
        std::process::exit(2);
    }

    // --- Select device ---
    let devices = ienv.devices();
    if devices.is_empty() {
        eprintln!("error: no NVMe devices found");
        std::process::exit(2);
    }

    let device = if let Some(ref addr_str) = config.pci_addr {
        match parse_pci_addr(addr_str) {
            Some(target) => {
                match devices.iter().find(|d| {
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
                }
            }
            None => {
                eprintln!("error: invalid PCI address format: {addr_str}");
                std::process::exit(1);
            }
        }
    } else {
        &devices[0]
    };

    let pci_addr_str = format!("{}", device.address);

    let admin = query::<dyn interfaces::IBlockDeviceAdmin + Send + Sync>(&*block_dev)
        .unwrap_or_else(|| {
            eprintln!("error: failed to query IBlockDeviceAdmin");
            std::process::exit(2);
        });

    admin.set_pci_address(interfaces::PciAddress {
        domain: device.address.domain,
        bus: device.address.bus,
        dev: device.address.dev,
        func: device.address.func,
    });

    if let Err(e) = admin.initialize() {
        eprintln!("error: block device init failed: {e}");
        std::process::exit(2);
    }

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap_or_else(|| {
        eprintln!("error: failed to query IBlockDevice");
        std::process::exit(2);
    });

    // --- Probe namespaces ---
    let probe_channels = ibd.connect_client().unwrap_or_else(|e| {
        eprintln!("error: failed to connect probe client: {e}");
        std::process::exit(2);
    });

    probe_channels
        .command_tx
        .send(Command::NsProbe)
        .unwrap_or_else(|e| {
            eprintln!("error: failed to send NsProbe: {e}");
            std::process::exit(2);
        });

    let namespaces = match probe_channels.completion_rx.recv() {
        Ok(Completion::NsProbeResult { namespaces }) => namespaces,
        Ok(other) => {
            eprintln!("error: unexpected completion from NsProbe: {other:?}");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("error: failed to recv NsProbe result: {e}");
            std::process::exit(2);
        }
    };

    if namespaces.is_empty() {
        eprintln!("error: no active namespaces on device");
        std::process::exit(2);
    }

    // Drop the probe channels — we no longer need them.
    drop(probe_channels);

    // --- Validate config ---
    let ns_info = namespaces
        .iter()
        .find(|ns| ns.ns_id == config.ns_id)
        .cloned()
        .unwrap_or_else(|| {
            let available: Vec<u32> = namespaces.iter().map(|ns| ns.ns_id).collect();
            eprintln!(
                "error: namespace {} not found (available: {:?})",
                config.ns_id, available
            );
            std::process::exit(1);
        });

    if let Err(msg) = config.validate(ns_info.sector_size, ibd.max_queue_depth(), &namespaces) {
        eprintln!("error: {msg}");
        std::process::exit(1);
    }
    config.clamp_queue_depth(ibd.max_queue_depth());

    // --- Print config ---
    report::print_config(&config, &pci_addr_str, &ns_info);
    println!();

    // --- Discover NUMA-local CPUs for worker thread pinning ---
    // The actor is pinned to the first CPU on the controller's NUMA node.
    // Pin workers to *other* cores on the same node to avoid contention.
    let numa_node = ibd.numa_node();
    let (actor_cpu, worker_cpus): (Option<usize>, Vec<usize>) = if numa_node >= 0 {
        NumaTopology::discover()
            .ok()
            .and_then(|topo| {
                topo.node(numa_node as usize)
                    .map(|n| n.cpus().iter().collect::<Vec<_>>())
            })
            .map(|cpus| {
                // Actor uses cpus[0]; workers get the rest.
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

    if !worker_cpus.is_empty() {
        eprintln!(
            "info: pinning {} worker(s) to NUMA-{} CPUs {:?} (actor on CPU {})",
            config.threads,
            numa_node,
            &worker_cpus,
            actor_cpu.unwrap_or(0),
        );
    }

    // --- Launch workers ---
    let stop_flag = Arc::new(AtomicBool::new(false));
    let config_arc = Arc::new(config.clone());

    let mut worker_handles = Vec::with_capacity(config.threads as usize);
    let mut op_counters = Vec::with_capacity(config.threads as usize);

    for thread_idx in 0..config.threads {
        let channels = ibd.connect_client().unwrap_or_else(|e| {
            eprintln!("error: failed to connect worker client {thread_idx}: {e}");
            std::process::exit(2);
        });

        let op_counter = Arc::new(AtomicU64::new(0));
        op_counters.push(Arc::clone(&op_counter));

        let worker_config = Arc::clone(&config_arc);
        let worker_stop = Arc::clone(&stop_flag);
        let worker_ns_info = ns_info.clone();

        let worker_cpus_clone = worker_cpus.clone();
        let handle = std::thread::spawn(move || {
            // Pin this worker to a NUMA-local core (round-robin, skipping the actor core).
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
    let timer_handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(duration_secs));
        timer_stop.store(true, Ordering::Relaxed);
    });

    // Progress reporting on main thread.
    if !config.quiet {
        let mut prev_counts: Vec<u64> = vec![0; op_counters.len()];
        let mut elapsed = 0u64;

        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_secs(1));
            elapsed += 1;

            let per_thread_iops: Vec<u64> = op_counters
                .iter()
                .zip(prev_counts.iter_mut())
                .map(|(counter, prev)| {
                    let current = counter.load(Ordering::Relaxed);
                    let delta = current - *prev;
                    *prev = current;
                    delta
                })
                .collect();
            let total_iops: u64 = per_thread_iops.iter().sum();

            report::print_progress(elapsed, total_iops, &per_thread_iops);
        }
    }

    // --- Join all threads ---
    timer_handle.join().expect("timer thread panicked");

    let actual_duration = bench_start.elapsed().as_secs_f64();

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
    let report = FinalReport::from_results(&results, actual_duration, config.block_size);
    report::print_final(&report, config.op, &results);

    std::process::exit(0);
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
