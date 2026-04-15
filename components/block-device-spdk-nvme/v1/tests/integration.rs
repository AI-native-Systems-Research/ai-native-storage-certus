// DmaBuffer is Send but not Sync; Arc<DmaBuffer> is required by Command::WriteSync API.
#![allow(clippy::arc_with_non_send_sync)]

//! Integration tests for the SPDK NVMe block device component.
//!
//! Tests that do not require hardware verify component wiring, receptacle
//! binding, and interface queries.
//!
//! Tests that require a live NVMe device use [`try_init_spdk`] for runtime
//! detection — they pass but do nothing when SPDK hardware is unavailable.

use std::sync::{Arc, Mutex, OnceLock};

use block_device_spdk_nvme::{BlockDeviceSpdkNvmeComponentV1, IBlockDevice};
use component_core::binding::bind;
use component_core::iunknown::{query, IUnknown};
use example_logger::LoggerComponent;
use spdk_env::SPDKEnvComponent;

/// Create a fully-wired component set without initializing SPDK.
///
/// Returns (block_device_component, logger_component, spdk_env_component).
fn wire_components() -> (
    Arc<BlockDeviceSpdkNvmeComponentV1>,
    Arc<LoggerComponent>,
    Arc<SPDKEnvComponent>,
) {
    let logger = LoggerComponent::new();
    let spdk_env = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();

    // Wire logger receptacle.
    bind(&*logger, "ILogger", &*block_dev, "logger").expect("failed to bind logger receptacle");

    // Wire spdk_env receptacle.
    bind(&*spdk_env, "ISPDKEnv", &*block_dev, "spdk_env")
        .expect("failed to bind spdk_env receptacle");

    // Wire logger into spdk_env as well.
    bind(&*logger, "ILogger", &*spdk_env, "logger").expect("failed to bind logger to spdk_env");

    (block_dev, logger, spdk_env)
}

/// Result of a successful SPDK hardware initialization.
///
/// Stored in a process-global `OnceLock` because SPDK is a singleton —
/// `SPDKEnvComponent` can only be initialized once per process, and the
/// component framework's `define_component!` macro creates `Arc<Self>`
/// reference cycles that prevent `Drop` (and thus re-initialization).
struct SpdkHardwareContext {
    block_dev: Arc<BlockDeviceSpdkNvmeComponentV1>,
    #[allow(dead_code)]
    logger: Arc<LoggerComponent>,
    #[allow(dead_code)]
    spdk_env: Arc<SPDKEnvComponent>,
}

// SAFETY: The component Arcs are Send+Sync (they use internal Mutex/RwLock).
// OnceLock requires Sync for shared access across test threads.
unsafe impl Sync for SpdkHardwareContext {}

/// Process-global shared SPDK context.
///
/// Initialized exactly once by whichever hardware test runs first.
/// All subsequent hardware tests reuse the same context.  This is
/// necessary because SPDK is a process-global singleton and the
/// component framework's Arc self-references prevent cleanup between
/// tests.
static SPDK_CONTEXT: OnceLock<Option<SpdkHardwareContext>> = OnceLock::new();

/// Get or initialize the shared SPDK hardware context.
///
/// Returns `None` (with an explanatory eprintln) when:
/// - VFIO is not available (kernel module not loaded)
/// - Hugepages are not configured
/// - SPDK environment initialization fails
/// - No NVMe devices are discovered
///
/// When hardware is available, returns a reference to the shared
/// initialized component set ready for IO operations.
fn get_spdk_context() -> Option<&'static SpdkHardwareContext> {
    SPDK_CONTEXT
        .get_or_init(|| {
            // Pre-flight: check VFIO and hugepages without side effects.
            if let Err(e) = spdk_env::checks::check_vfio_available() {
                eprintln!("SPDK hardware not available (VFIO): {e}");
                return None;
            }
            if let Err(e) = spdk_env::checks::check_hugepages() {
                eprintln!("SPDK hardware not available (hugepages): {e}");
                return None;
            }

            let (block_dev, logger, spdk_env) = wire_components();

            // Initialize the SPDK environment.
            let ienv = query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env)
                .expect("ISPDKEnv interface not found");
            if let Err(e) = ienv.init() {
                eprintln!("SPDK init failed: {e}");
                return None;
            }

            // Check for discovered NVMe devices.
            let devices = ienv.devices();
            if devices.is_empty() {
                eprintln!("SPDK initialized but no NVMe devices found");
                return None;
            }

            // Configure the block device with the first discovered device address.
            // Convert spdk_env::PciAddress → interfaces::PciAddress (same layout, different types).
            let spdk_addr = devices[0].address;
            let addr = interfaces::PciAddress {
                domain: spdk_addr.domain,
                bus: spdk_addr.bus,
                dev: spdk_addr.dev,
                func: spdk_addr.func,
            };
            block_dev.set_pci_address(addr);

            // Initialize the block device (probe controller, start actor).
            if let Err(e) = block_dev.initialize() {
                eprintln!("Block device initialize failed: {e}");
                return None;
            }

            Some(SpdkHardwareContext {
                block_dev,
                logger,
                spdk_env,
            })
        })
        .as_ref()
}

// ---------------------------------------------------------------------------
// Tests that do NOT require SPDK hardware
// ---------------------------------------------------------------------------

#[test]
fn component_wiring_succeeds() {
    let (block_dev, _logger, _spdk_env) = wire_components();

    // Verify receptacles are connected.
    let receps = block_dev.receptacles();
    assert!(
        receps.iter().any(|r| r.name == "logger"),
        "logger receptacle not found"
    );
    assert!(
        receps.iter().any(|r| r.name == "spdk_env"),
        "spdk_env receptacle not found"
    );
}

#[test]
fn query_iblock_device_interface() {
    let (block_dev, _logger, _spdk_env) = wire_components();

    // Query the IBlockDevice interface from the component.
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev);
    assert!(ibd.is_some(), "IBlockDevice interface not found");
}

#[test]
fn device_info_before_initialize() {
    let (block_dev, _logger, _spdk_env) = wire_components();

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();

    // Before initialize(), device info should return defaults.
    assert_eq!(ibd.max_queue_depth(), 0);
    assert_eq!(ibd.num_io_queues(), 0);
    assert_eq!(ibd.max_transfer_size(), 0);
    assert_eq!(ibd.block_size(), 512);
    assert_eq!(ibd.numa_node(), -1);
    assert_eq!(ibd.nvme_version(), "unknown");
}

#[test]
fn connect_client_before_initialize_returns_error() {
    let (block_dev, _logger, _spdk_env) = wire_components();

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();

    let result = ibd.connect_client();
    assert!(
        result.is_err(),
        "connect_client should fail before initialize"
    );
}

#[test]
fn telemetry_without_feature_returns_error() {
    let (block_dev, _logger, _spdk_env) = wire_components();

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();

    let result = ibd.telemetry();
    assert!(result.is_err(), "telemetry should return error");
}

#[test]
fn sector_size_before_initialize_returns_error() {
    let (block_dev, _logger, _spdk_env) = wire_components();

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();

    let result = ibd.sector_size(1);
    assert!(result.is_err(), "sector_size should fail before initialize");
}

// ---------------------------------------------------------------------------
// Tests that REQUIRE SPDK hardware (self-skipping when unavailable)
// ---------------------------------------------------------------------------

#[test]
fn initialize_with_hardware() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping initialize_with_hardware: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();

    // After initialize(), device info should be populated with real values.
    assert!(ibd.max_queue_depth() > 0, "max_queue_depth should be > 0");
    assert!(ibd.block_size() > 0, "block_size should be > 0");
    assert_ne!(ibd.nvme_version(), "unknown", "nvme_version should be set");
}

#[test]
fn device_info_after_initialize() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping device_info_after_initialize: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();

    let block_size = ibd.block_size();
    assert!(
        block_size == 512 || block_size == 4096,
        "block_size should be 512 or 4096, got {block_size}"
    );

    let max_transfer = ibd.max_transfer_size();
    assert!(
        max_transfer >= 4096,
        "max_transfer_size should be >= 4096, got {max_transfer}"
    );

    let numa = ibd.numa_node();
    assert!(numa >= 0, "numa_node should be >= 0 after init, got {numa}");
}

#[test]
fn namespace_probe() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping namespace_probe: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client failed");

    // Probe namespaces.
    channels
        .command_tx
        .send(block_device_spdk_nvme::Command::NsProbe)
        .expect("send NsProbe failed");
    ctx.block_dev.flush_io().expect("flush_io");

    let completion = channels.completion_rx.recv().expect("recv failed");
    let namespaces = match completion {
        block_device_spdk_nvme::Completion::NsProbeResult { namespaces } => namespaces,
        other => panic!("expected NsProbeResult, got {other:?}"),
    };

    assert!(!namespaces.is_empty(), "no namespaces found on device");
    let ns = &namespaces[0];
    assert!(ns.sector_size > 0, "sector_size should be > 0");
    assert!(ns.num_sectors > 0, "num_sectors should be > 0");
}

#[test]
fn write_read_roundtrip() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping write_read_roundtrip: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client failed");

    // Probe namespaces to find a valid ns_id.
    channels
        .command_tx
        .send(block_device_spdk_nvme::Command::NsProbe)
        .expect("send NsProbe failed");
    ctx.block_dev.flush_io().expect("flush_io");

    let completion = channels.completion_rx.recv().expect("recv failed");
    let namespaces = match completion {
        block_device_spdk_nvme::Completion::NsProbeResult { namespaces } => namespaces,
        other => panic!("expected NsProbeResult, got {other:?}"),
    };

    assert!(!namespaces.is_empty(), "no namespaces found on device");
    let ns = &namespaces[0];

    // Allocate a DMA buffer for one sector and fill with a known pattern.
    let sector_size = ns.sector_size as usize;
    let mut write_buf = interfaces::DmaBuffer::new(sector_size, sector_size, None)
        .expect("DMA buffer allocation failed");

    let pattern: Vec<u8> = (0..sector_size).map(|i| (i % 256) as u8).collect();
    write_buf.as_mut_slice().copy_from_slice(&pattern);
    let write_buf = std::sync::Arc::new(write_buf);

    channels
        .command_tx
        .send(block_device_spdk_nvme::Command::WriteSync {
            ns_id: ns.ns_id,
            lba: 0,
            buf: write_buf,
        })
        .expect("send WriteSync failed");
    ctx.block_dev.flush_io().expect("flush_io");

    let completion = channels.completion_rx.recv().expect("recv failed");
    match completion {
        block_device_spdk_nvme::Completion::WriteDone { result, .. } => {
            result.expect("write failed")
        }
        other => panic!("expected WriteDone, got {other:?}"),
    }

    // Read back from LBA 0.
    let read_buf = interfaces::DmaBuffer::new(sector_size, sector_size, None)
        .expect("DMA buffer allocation failed");
    let read_buf = std::sync::Arc::new(std::sync::Mutex::new(read_buf));

    channels
        .command_tx
        .send(block_device_spdk_nvme::Command::ReadSync {
            ns_id: ns.ns_id,
            lba: 0,
            buf: std::sync::Arc::clone(&read_buf),
        })
        .expect("send ReadSync failed");
    ctx.block_dev.flush_io().expect("flush_io");

    let completion = channels.completion_rx.recv().expect("recv failed");
    match completion {
        block_device_spdk_nvme::Completion::ReadDone { result, .. } => result.expect("read failed"),
        other => panic!("expected ReadDone, got {other:?}"),
    }

    // Verify data integrity.
    let guard = read_buf.lock().unwrap();
    let read_data = guard.as_slice();
    assert_eq!(
        read_data,
        &pattern[..],
        "read data does not match written pattern"
    );
}

/// Helper: probe namespaces on a connected client and return the namespace list.
///
/// Panics if the probe fails or returns an unexpected completion type.
fn probe_namespaces(
    channels: &interfaces::ClientChannels,
    block_dev: &BlockDeviceSpdkNvmeComponentV1,
) -> Vec<interfaces::NamespaceInfo> {
    channels
        .command_tx
        .send(block_device_spdk_nvme::Command::NsProbe)
        .expect("send NsProbe failed");
    block_dev.flush_io().expect("flush_io");

    match channels.completion_rx.recv().expect("recv failed") {
        block_device_spdk_nvme::Completion::NsProbeResult { namespaces } => namespaces,
        other => panic!("expected NsProbeResult, got {other:?}"),
    }
}

/// Helper: flush and poll until a completion arrives.
///
/// Async SPDK operations may not complete within a single `flush_io()` cycle,
/// so this loops calling `flush_io()` + `try_recv()` until the actor delivers
/// the completion.
fn flush_until_completion(
    block_dev: &BlockDeviceSpdkNvmeComponentV1,
    channels: &interfaces::ClientChannels,
) -> block_device_spdk_nvme::Completion {
    loop {
        block_dev.flush_io().expect("flush_io");
        if let Ok(c) = channels.completion_rx.try_recv() {
            return c;
        }
        std::thread::yield_now();
    }
}

#[test]
fn sync_write_async_read_roundtrip() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping sync_write_async_read_roundtrip: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client failed");
    let namespaces = probe_namespaces(&channels, &ctx.block_dev);
    assert!(!namespaces.is_empty(), "no namespaces found");
    let ns = &namespaces[0];
    let sector_size = ns.sector_size as usize;

    // Sync write a known pattern to LBA 3.
    let mut write_buf =
        interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc failed");
    let pattern: Vec<u8> = (0..sector_size).map(|i| ((i + 0xCD) % 256) as u8).collect();
    write_buf.as_mut_slice().copy_from_slice(&pattern);
    let write_buf = Arc::new(write_buf);

    channels
        .command_tx
        .send(block_device_spdk_nvme::Command::WriteSync {
            ns_id: ns.ns_id,
            lba: 3,
            buf: write_buf,
        })
        .expect("send WriteSync failed");
    ctx.block_dev.flush_io().expect("flush_io");

    match channels.completion_rx.recv().expect("recv failed") {
        block_device_spdk_nvme::Completion::WriteDone { result, .. } => {
            result.expect("write failed")
        }
        other => panic!("expected WriteDone, got {other:?}"),
    }

    // Async read back from LBA 3 to test async read path.
    let read_buf =
        interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc failed");
    let read_buf = Arc::new(Mutex::new(read_buf));

    channels
        .command_tx
        .send(block_device_spdk_nvme::Command::ReadAsync {
            ns_id: ns.ns_id,
            lba: 3,
            buf: Arc::clone(&read_buf),
            timeout_ms: 5000,
        })
        .expect("send ReadAsync failed");

    match flush_until_completion(&ctx.block_dev, &channels) {
        block_device_spdk_nvme::Completion::ReadDone { result, .. } => {
            result.expect("async read failed")
        }
        other => panic!("expected ReadDone, got {other:?}"),
    }

    let guard = read_buf.lock().unwrap();
    assert_eq!(
        guard.as_slice(),
        &pattern[..],
        "async read data does not match sync-written pattern"
    );
}

#[test]
fn write_on_one_client_read_on_another() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping write_on_one_client_read_on_another: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();

    // Client A: write.
    let client_a = ibd.connect_client().expect("connect_client A failed");
    let namespaces = probe_namespaces(&client_a, &ctx.block_dev);
    assert!(!namespaces.is_empty(), "no namespaces found");
    let ns = &namespaces[0];
    let sector_size = ns.sector_size as usize;

    let mut write_buf =
        interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc failed");
    let pattern: Vec<u8> = (0..sector_size).map(|i| ((i + 0xCD) % 256) as u8).collect();
    write_buf.as_mut_slice().copy_from_slice(&pattern);
    let write_buf = Arc::new(write_buf);

    client_a
        .command_tx
        .send(block_device_spdk_nvme::Command::WriteSync {
            ns_id: ns.ns_id,
            lba: 2,
            buf: write_buf,
        })
        .expect("send WriteSync failed");
    ctx.block_dev.flush_io().expect("flush_io");

    match client_a.completion_rx.recv().expect("recv failed") {
        block_device_spdk_nvme::Completion::WriteDone { result, .. } => {
            result.expect("write failed")
        }
        other => panic!("expected WriteDone, got {other:?}"),
    }

    // Client B: read back the same LBA written by client A.
    let client_b = ibd.connect_client().expect("connect_client B failed");
    let read_buf =
        interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc failed");
    let read_buf = Arc::new(Mutex::new(read_buf));

    client_b
        .command_tx
        .send(block_device_spdk_nvme::Command::ReadSync {
            ns_id: ns.ns_id,
            lba: 2,
            buf: Arc::clone(&read_buf),
        })
        .expect("send ReadSync failed");
    ctx.block_dev.flush_io().expect("flush_io");

    match client_b.completion_rx.recv().expect("recv failed") {
        block_device_spdk_nvme::Completion::ReadDone { result, .. } => {
            result.expect("read failed")
        }
        other => panic!("expected ReadDone, got {other:?}"),
    }

    let guard = read_buf.lock().unwrap();
    assert_eq!(
        guard.as_slice(),
        &pattern[..],
        "client B read does not match client A write"
    );
}

#[test]
fn multi_thread_concurrent_io() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping multi_thread_concurrent_io: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();

    // Probe once to get namespace info.
    let probe_channels = ibd.connect_client().expect("connect_client probe failed");
    let namespaces = probe_namespaces(&probe_channels, &ctx.block_dev);
    assert!(!namespaces.is_empty(), "no namespaces found");
    let ns = namespaces[0].clone();
    drop(probe_channels);

    let num_threads = 4u32;
    let ops_per_thread = 16u32;
    let sector_size = ns.sector_size as usize;
    let block_dev = Arc::clone(&ctx.block_dev);

    let handles: Vec<_> = (0..num_threads)
        .map(|tid| {
            let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();
            let channels = ibd.connect_client().expect("connect_client failed");
            let ns = ns.clone();
            let block_dev = Arc::clone(&block_dev);

            std::thread::spawn(move || {
                // Each thread writes to a unique LBA range to avoid conflicts.
                let base_lba = 100 + (tid as u64) * (ops_per_thread as u64);

                for i in 0..ops_per_thread {
                    let lba = base_lba + i as u64;
                    let tag = ((tid * 64 + i) % 256) as u8;

                    // Write a pattern tagged with (tid, i).
                    let mut wbuf = interfaces::DmaBuffer::new(sector_size, sector_size, None)
                        .expect("DMA alloc failed");
                    for byte in wbuf.as_mut_slice().iter_mut() {
                        *byte = tag;
                    }
                    let wbuf = Arc::new(wbuf);

                    channels
                        .command_tx
                        .send(block_device_spdk_nvme::Command::WriteSync {
                            ns_id: ns.ns_id,
                            lba,
                            buf: wbuf,
                        })
                        .expect("send WriteSync failed");
                    block_dev.flush_io().expect("flush_io");

                    match channels.completion_rx.recv().expect("recv failed") {
                        block_device_spdk_nvme::Completion::WriteDone { result, .. } => {
                            result.expect("write failed")
                        }
                        other => panic!("expected WriteDone, got {other:?}"),
                    }

                    // Read back and verify.
                    let rbuf = interfaces::DmaBuffer::new(sector_size, sector_size, None)
                        .expect("DMA alloc failed");
                    let rbuf = Arc::new(Mutex::new(rbuf));

                    channels
                        .command_tx
                        .send(block_device_spdk_nvme::Command::ReadSync {
                            ns_id: ns.ns_id,
                            lba,
                            buf: Arc::clone(&rbuf),
                        })
                        .expect("send ReadSync failed");
                    block_dev.flush_io().expect("flush_io");

                    match channels.completion_rx.recv().expect("recv failed") {
                        block_device_spdk_nvme::Completion::ReadDone { result, .. } => {
                            result.expect("read failed")
                        }
                        other => panic!("expected ReadDone, got {other:?}"),
                    }

                    let guard = rbuf.lock().unwrap();
                    for (j, &byte) in guard.as_slice().iter().enumerate() {
                        assert_eq!(
                            byte, tag,
                            "thread {tid} op {i}: byte {j} mismatch (expected {tag:#x}, got {byte:#x})"
                        );
                    }
                }

                ops_per_thread
            })
        })
        .collect();

    let mut total_ops = 0u32;
    for h in handles {
        total_ops += h.join().expect("worker thread panicked");
    }

    assert_eq!(
        total_ops,
        num_threads * ops_per_thread,
        "not all operations completed"
    );
}

// NOTE: WriteAsync has a known data-integrity bug: the Arc<DmaBuffer> is dropped
// after SPDK submission but before the NVMe device finishes DMA-reading from it,
// so the device reads freed memory. Async write tests are excluded until the
// component pins the write buffer in PendingOp until completion. ReadAsync works
// correctly because the caller retains an Arc clone of the read buffer.
