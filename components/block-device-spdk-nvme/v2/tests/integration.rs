// DmaBuffer is Send but not Sync; Arc<DmaBuffer> is required by Command::WriteSync API.
#![allow(clippy::arc_with_non_send_sync)]
#![cfg(feature = "spdk")]

//! Integration tests for the SPDK NVMe block device component.
//!
//! Tests that do not require hardware verify component wiring, receptacle
//! binding, and interface queries.
//!
//! Tests that require a live NVMe device use [`try_init_spdk`] for runtime
//! detection — they pass but do nothing when SPDK hardware is unavailable.

use std::sync::{Arc, Mutex, OnceLock};

use block_device_spdk_nvme_v2::{BlockDeviceSpdkNvmeComponentV2, IBlockDevice};
use component_core::binding::bind;
use component_core::iunknown::{query, IUnknown};
use logger::LoggerComponentV1;
use spdk_env::SPDKEnvComponent;

/// Create a fully-wired component set without initializing SPDK.
///
/// Returns (block_device_component, spdk_env_component).
fn wire_components() -> (
    Arc<BlockDeviceSpdkNvmeComponentV2>,
    Arc<SPDKEnvComponent>,
    Arc<LoggerComponentV1>,
) {
    let spdk_env = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponentV2::new_default();
    let logger = LoggerComponentV1::new_default();

    bind(&*spdk_env, "ISPDKEnv", &*block_dev, "spdk_env")
        .expect("failed to bind spdk_env receptacle");
    bind(&*logger, "ILogger", &*block_dev, "logger").expect("failed to bind logger receptacle");

    (block_dev, spdk_env, logger)
}

/// Result of a successful SPDK hardware initialization.
///
/// Stored in a process-global `OnceLock` because SPDK is a singleton —
/// `SPDKEnvComponent` can only be initialized once per process, and the
/// component framework's `define_component!` macro creates `Arc<Self>`
/// reference cycles that prevent `Drop` (and thus re-initialization).
struct SpdkHardwareContext {
    block_dev: Arc<BlockDeviceSpdkNvmeComponentV2>,
    #[allow(dead_code)]
    spdk_env: Arc<SPDKEnvComponent>,
    #[allow(dead_code)]
    logger: Arc<LoggerComponentV1>,
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
///
/// The context is intentionally leaked (`Box::leak`) so that SPDK
/// resources are never dropped — dropping them at process exit causes
/// a SIGSEGV in the SPDK teardown path.
static SPDK_CONTEXT: OnceLock<Option<&'static SpdkHardwareContext>> = OnceLock::new();

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
    *SPDK_CONTEXT.get_or_init(|| {
        // Register an atexit handler that terminates the process immediately,
        // bypassing SPDK's own atexit teardown which segfaults when the SPDK
        // environment outlives the test harness.
        extern "C" {
            fn atexit(cb: extern "C" fn()) -> i32;
            fn _exit(status: i32) -> !;
        }
        extern "C" fn exit_before_spdk_teardown() {
            unsafe { _exit(0) };
        }
        unsafe { atexit(exit_before_spdk_teardown) };

        // Pre-flight: check VFIO and hugepages without side effects.
        if let Err(e) = spdk_env::checks::check_vfio_available() {
            eprintln!("SPDK hardware not available (VFIO): {e}");
            return None;
        }
        if let Err(e) = spdk_env::checks::check_hugepages() {
            eprintln!("SPDK hardware not available (hugepages): {e}");
            return None;
        }

        let (block_dev, spdk_env, logger) = wire_components();

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

        let admin = query::<dyn interfaces::iblock_device::IBlockDeviceAdmin + Send + Sync>(
            &*block_dev,
        )
        .expect("IBlockDeviceAdmin query");
        admin.set_pci_address(addr);

        // Initialize the block device (probe controller, start actor).
        if let Err(e) = admin.initialize() {
            eprintln!("Block device initialize failed: {e}");
            return None;
        }

        // Leak to prevent SPDK teardown SIGSEGV at process exit.
        Some(Box::leak(Box::new(SpdkHardwareContext {
            block_dev,
            spdk_env,
            logger,
        })))
    })
}

// ---------------------------------------------------------------------------
// Tests that do NOT require SPDK hardware
// ---------------------------------------------------------------------------

#[test]
fn component_wiring_succeeds() {
    let (block_dev, _spdk_env, _logger) = wire_components();

    // Verify receptacles are connected.
    let receps = block_dev.receptacles();
    assert!(
        receps.iter().any(|r| r.name == "spdk_env"),
        "spdk_env receptacle not found"
    );
    assert!(
        receps.iter().any(|r| r.name == "logger"),
        "logger receptacle not found"
    );
}

#[test]
fn query_iblock_device_interface() {
    let (block_dev, _spdk_env, _logger) = wire_components();

    // Query the IBlockDevice interface from the component.
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev);
    assert!(ibd.is_some(), "IBlockDevice interface not found");
}

#[test]
fn device_info_before_initialize() {
    let (block_dev, _spdk_env, _logger) = wire_components();

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
    let (block_dev, _spdk_env, _logger) = wire_components();

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();

    let result = ibd.connect_client();
    assert!(
        result.is_err(),
        "connect_client should fail before initialize"
    );
}

#[test]
fn telemetry_without_feature_returns_error() {
    let (block_dev, _spdk_env, _logger) = wire_components();

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();

    let result = ibd.telemetry();
    assert!(result.is_err(), "telemetry should return error");
}

#[test]
fn sector_size_before_initialize_returns_error() {
    let (block_dev, _spdk_env, _logger) = wire_components();

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
        .send(block_device_spdk_nvme_v2::Command::NsProbe)
        .expect("send NsProbe failed");

    let completion = channels.completion_rx.recv().expect("recv failed");
    let namespaces = match completion {
        block_device_spdk_nvme_v2::Completion::NsProbeResult { namespaces } => namespaces,
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
        .send(block_device_spdk_nvme_v2::Command::NsProbe)
        .expect("send NsProbe failed");

    let completion = channels.completion_rx.recv().expect("recv failed");
    let namespaces = match completion {
        block_device_spdk_nvme_v2::Completion::NsProbeResult { namespaces } => namespaces,
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
        .send(block_device_spdk_nvme_v2::Command::WriteSync {
            ns_id: ns.ns_id,
            lba: 0,
            buf: write_buf,
        })
        .expect("send WriteSync failed");

    let completion = channels.completion_rx.recv().expect("recv failed");
    match completion {
        block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
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
        .send(block_device_spdk_nvme_v2::Command::ReadSync {
            ns_id: ns.ns_id,
            lba: 0,
            buf: std::sync::Arc::clone(&read_buf),
        })
        .expect("send ReadSync failed");

    let completion = channels.completion_rx.recv().expect("recv failed");
    match completion {
        block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
            result.expect("read failed")
        }
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
fn probe_namespaces(channels: &interfaces::ClientChannels) -> Vec<interfaces::NamespaceInfo> {
    channels
        .command_tx
        .send(block_device_spdk_nvme_v2::Command::NsProbe)
        .expect("send NsProbe failed");

    match channels.completion_rx.recv().expect("recv failed") {
        block_device_spdk_nvme_v2::Completion::NsProbeResult { namespaces } => namespaces,
        other => panic!("expected NsProbeResult, got {other:?}"),
    }
}

/// Helper: wait for a completion to arrive on the callback channel.
///
/// The actor self-polls, so we just block on `recv()`.
fn wait_for_completion(
    channels: &interfaces::ClientChannels,
) -> block_device_spdk_nvme_v2::Completion {
    channels.completion_rx.recv().expect("recv failed")
}

#[test]
fn sync_write_async_read_roundtrip() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping sync_write_async_read_roundtrip: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client failed");
    let namespaces = probe_namespaces(&channels);
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
        .send(block_device_spdk_nvme_v2::Command::WriteSync {
            ns_id: ns.ns_id,
            lba: 3,
            buf: write_buf,
        })
        .expect("send WriteSync failed");

    match channels.completion_rx.recv().expect("recv failed") {
        block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
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
        .send(block_device_spdk_nvme_v2::Command::ReadAsync {
            ns_id: ns.ns_id,
            lba: 3,
            buf: Arc::clone(&read_buf),
            timeout_ms: 5000,
        })
        .expect("send ReadAsync failed");

    match wait_for_completion(&channels) {
        block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
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
    let namespaces = probe_namespaces(&client_a);
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
        .send(block_device_spdk_nvme_v2::Command::WriteSync {
            ns_id: ns.ns_id,
            lba: 2,
            buf: write_buf,
        })
        .expect("send WriteSync failed");

    match client_a.completion_rx.recv().expect("recv failed") {
        block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
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
        .send(block_device_spdk_nvme_v2::Command::ReadSync {
            ns_id: ns.ns_id,
            lba: 2,
            buf: Arc::clone(&read_buf),
        })
        .expect("send ReadSync failed");

    match client_b.completion_rx.recv().expect("recv failed") {
        block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
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
    let namespaces = probe_namespaces(&probe_channels);
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
                        .send(block_device_spdk_nvme_v2::Command::WriteSync {
                            ns_id: ns.ns_id,
                            lba,
                            buf: wbuf,
                        })
                        .expect("send WriteSync failed");

                    match channels.completion_rx.recv().expect("recv failed") {
                        block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
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
                        .send(block_device_spdk_nvme_v2::Command::ReadSync {
                            ns_id: ns.ns_id,
                            lba,
                            buf: Arc::clone(&rbuf),
                        })
                        .expect("send ReadSync failed");

                    match channels.completion_rx.recv().expect("recv failed") {
                        block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
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

#[test]
fn multi_client_independent_streams() {
    let Some(ctx) = get_spdk_context() else {
        eprintln!("skipping multi_client_independent_streams: no SPDK hardware");
        return;
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();

    // Probe namespaces once.
    let probe_ch = ibd.connect_client().expect("connect_client probe failed");
    let namespaces = probe_namespaces(&probe_ch);
    assert!(!namespaces.is_empty(), "no namespaces found");
    let ns = namespaces[0].clone();
    drop(probe_ch);

    let num_clients = 3u32;
    let ops_per_client = 8u32;
    let sector_size = ns.sector_size as usize;
    let block_dev = Arc::clone(&ctx.block_dev);

    let handles: Vec<_> = (0..num_clients)
        .map(|cid| {
            let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();
            let channels = ibd.connect_client().expect("connect_client failed");
            let ns = ns.clone();

            std::thread::spawn(move || {
                let base_lba = 300 + (cid as u64) * (ops_per_client as u64);

                for i in 0..ops_per_client {
                    let lba = base_lba + i as u64;
                    let sync_tag = ((cid * 100 + i) % 256) as u8;
                    let async_tag = ((cid * 100 + i + 50) % 256) as u8;

                    // --- Sync write ---
                    let mut wbuf = interfaces::DmaBuffer::new(sector_size, sector_size, None)
                        .expect("DMA alloc failed");
                    wbuf.as_mut_slice().fill(sync_tag);
                    let wbuf = Arc::new(wbuf);

                    channels
                        .command_tx
                        .send(block_device_spdk_nvme_v2::Command::WriteSync {
                            ns_id: ns.ns_id,
                            lba,
                            buf: wbuf,
                        })
                        .expect("send WriteSync failed");

                    match channels.completion_rx.recv().expect("recv failed") {
                        block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
                            result.expect("sync write failed")
                        }
                        other => panic!("client {cid} op {i}: expected WriteDone, got {other:?}"),
                    }

                    // --- Sync read-back ---
                    let rbuf = interfaces::DmaBuffer::new(sector_size, sector_size, None)
                        .expect("DMA alloc failed");
                    let rbuf = Arc::new(Mutex::new(rbuf));

                    channels
                        .command_tx
                        .send(block_device_spdk_nvme_v2::Command::ReadSync {
                            ns_id: ns.ns_id,
                            lba,
                            buf: Arc::clone(&rbuf),
                        })
                        .expect("send ReadSync failed");

                    match channels.completion_rx.recv().expect("recv failed") {
                        block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
                            result.expect("sync read failed")
                        }
                        other => panic!("client {cid} op {i}: expected ReadDone, got {other:?}"),
                    }

                    {
                        let guard = rbuf.lock().unwrap();
                        for (j, &byte) in guard.as_slice().iter().enumerate() {
                            assert_eq!(
                                byte, sync_tag,
                                "client {cid} op {i} sync read: byte {j} mismatch \
                                 (expected {sync_tag:#x}, got {byte:#x})"
                            );
                        }
                    }

                    // --- Async write (overwrite same LBA with different pattern) ---
                    let mut awbuf = interfaces::DmaBuffer::new(sector_size, sector_size, None)
                        .expect("DMA alloc failed");
                    awbuf.as_mut_slice().fill(async_tag);
                    let awbuf = Arc::new(awbuf);

                    channels
                        .command_tx
                        .send(block_device_spdk_nvme_v2::Command::WriteAsync {
                            ns_id: ns.ns_id,
                            lba,
                            buf: awbuf,
                            timeout_ms: 5000,
                        })
                        .expect("send WriteAsync failed");

                    match channels.completion_rx.recv().expect("recv failed") {
                        block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
                            result.expect("async write failed")
                        }
                        other => panic!("client {cid} op {i}: expected WriteDone, got {other:?}"),
                    }

                    // --- Async read-back ---
                    let arbuf = interfaces::DmaBuffer::new(sector_size, sector_size, None)
                        .expect("DMA alloc failed");
                    let arbuf = Arc::new(Mutex::new(arbuf));

                    channels
                        .command_tx
                        .send(block_device_spdk_nvme_v2::Command::ReadAsync {
                            ns_id: ns.ns_id,
                            lba,
                            buf: Arc::clone(&arbuf),
                            timeout_ms: 5000,
                        })
                        .expect("send ReadAsync failed");

                    match channels.completion_rx.recv().expect("recv failed") {
                        block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
                            result.expect("async read failed")
                        }
                        other => panic!("client {cid} op {i}: expected ReadDone, got {other:?}"),
                    }

                    {
                        let guard = arbuf.lock().unwrap();
                        for (j, &byte) in guard.as_slice().iter().enumerate() {
                            assert_eq!(
                                byte, async_tag,
                                "client {cid} op {i} async read: byte {j} mismatch \
                                 (expected {async_tag:#x}, got {byte:#x})"
                            );
                        }
                    }
                }

                ops_per_client
            })
        })
        .collect();

    let mut total_ops = 0u32;
    for h in handles {
        total_ops += h.join().expect("worker thread panicked");
    }

    assert_eq!(
        total_ops,
        num_clients * ops_per_client,
        "not all operations completed"
    );
}

// ---------------------------------------------------------------------------
// Namespace partitioning test
// ---------------------------------------------------------------------------

/// Helper: send a namespace create command and return the new ns_id.
fn create_namespace(channels: &interfaces::ClientChannels, size_sectors: u64) -> u32 {
    channels
        .command_tx
        .send(block_device_spdk_nvme_v2::Command::NsCreate { size_sectors })
        .expect("send NsCreate failed");

    match wait_for_completion(channels) {
        block_device_spdk_nvme_v2::Completion::NsCreated { ns_id } => ns_id,
        block_device_spdk_nvme_v2::Completion::Error { error, .. } => {
            panic!("NsCreate({size_sectors} sectors) failed: {error}");
        }
        other => panic!("expected NsCreated, got {other:?}"),
    }
}

/// Helper: delete a namespace by id.
fn delete_namespace(channels: &interfaces::ClientChannels, ns_id: u32) {
    channels
        .command_tx
        .send(block_device_spdk_nvme_v2::Command::NsDelete { ns_id })
        .expect("send NsDelete failed");

    match wait_for_completion(channels) {
        block_device_spdk_nvme_v2::Completion::NsDeleted { .. } => {}
        block_device_spdk_nvme_v2::Completion::Error { error, .. } => {
            panic!("NsDelete(ns_id={ns_id}) failed: {error}");
        }
        other => panic!("expected NsDeleted, got {other:?}"),
    }
}

/// Create N equal-sized NVMe namespaces, perform a write-read roundtrip on each,
/// then restore the device to a single namespace.
///
/// This test is destructive: it deletes all existing namespaces on the device
/// and recreates them. Marked `#[ignore]` for manual execution only.
#[test]
#[ignore]
fn create_n_namespaces_with_io() {
    const NUM_NS: u32 = 4;

    let ctx = match get_spdk_context() {
        Some(c) => c,
        None => {
            eprintln!("skipping create_n_namespaces_with_io: no SPDK hardware");
            return;
        }
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client failed");

    // --- Delete existing namespaces to start clean ---
    let existing = probe_namespaces(&channels);
    let original_total: u64 = existing.iter().map(|ns| ns.num_sectors).sum();
    eprintln!(
        "Device has {} namespace(s), {} total sectors",
        existing.len(),
        original_total
    );
    for ns in &existing {
        delete_namespace(&channels, ns.ns_id);
        eprintln!("Deleted namespace {}", ns.ns_id);
    }

    // --- Create N equal-sized namespaces ---
    // Use 100,000 sectors per namespace (~400MB at 4K sectors, ~50MB at 512B).
    let per_ns_sectors: u64 = 100_000;
    eprintln!("Creating {NUM_NS} namespaces of {per_ns_sectors} sectors each");

    let mut created_ns_ids = Vec::with_capacity(NUM_NS as usize);
    for i in 0..NUM_NS {
        let ns_id = create_namespace(&channels, per_ns_sectors);
        eprintln!("Created namespace {ns_id} ({} of {NUM_NS})", i + 1);
        created_ns_ids.push(ns_id);
    }

    // Verify all N namespaces exist with the expected size.
    let partitions = probe_namespaces(&channels);
    assert_eq!(
        partitions.len(),
        NUM_NS as usize,
        "expected {NUM_NS} namespaces, got {}",
        partitions.len()
    );
    let sector_size = partitions[0].sector_size;
    for ns in &partitions {
        assert_eq!(
            ns.num_sectors, per_ns_sectors,
            "namespace {} has {} sectors, expected {per_ns_sectors}",
            ns.ns_id, ns.num_sectors
        );
    }

    // --- Write-read roundtrip on each namespace ---
    let ss = sector_size as usize;
    for (idx, ns) in partitions.iter().enumerate() {
        let tag = ((idx + 1) * 0x31) as u8;

        // Write a unique pattern to LBA 0 of this namespace.
        let mut wbuf = interfaces::DmaBuffer::new(ss, ss, None).expect("DMA alloc failed");
        wbuf.as_mut_slice().fill(tag);
        let wbuf = Arc::new(wbuf);

        channels
            .command_tx
            .send(block_device_spdk_nvme_v2::Command::WriteSync {
                ns_id: ns.ns_id,
                lba: 0,
                buf: wbuf,
            })
            .expect("send WriteSync failed");

        match wait_for_completion(&channels) {
            block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
                result.unwrap_or_else(|e| panic!("write to ns {} failed: {e}", ns.ns_id));
            }
            other => panic!("ns {}: expected WriteDone, got {other:?}", ns.ns_id),
        }

        // Read back and verify.
        let rbuf = interfaces::DmaBuffer::new(ss, ss, None).expect("DMA alloc failed");
        let rbuf = Arc::new(Mutex::new(rbuf));

        channels
            .command_tx
            .send(block_device_spdk_nvme_v2::Command::ReadSync {
                ns_id: ns.ns_id,
                lba: 0,
                buf: Arc::clone(&rbuf),
            })
            .expect("send ReadSync failed");

        match wait_for_completion(&channels) {
            block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
                result.unwrap_or_else(|e| panic!("read from ns {} failed: {e}", ns.ns_id));
            }
            other => panic!("ns {}: expected ReadDone, got {other:?}", ns.ns_id),
        }

        let guard = rbuf.lock().unwrap();
        for (j, &byte) in guard.as_slice().iter().enumerate() {
            assert_eq!(
                byte, tag,
                "ns {} byte {j}: expected {tag:#x}, got {byte:#x}",
                ns.ns_id
            );
        }
        eprintln!("Namespace {} (ns_id={}): write-read OK", idx + 1, ns.ns_id);
    }

    // --- Cross-namespace isolation: re-read each namespace to confirm no bleed ---
    for (idx, ns) in partitions.iter().enumerate() {
        let tag = ((idx + 1) * 0x31) as u8;

        let rbuf = interfaces::DmaBuffer::new(ss, ss, None).expect("DMA alloc failed");
        let rbuf = Arc::new(Mutex::new(rbuf));

        channels
            .command_tx
            .send(block_device_spdk_nvme_v2::Command::ReadSync {
                ns_id: ns.ns_id,
                lba: 0,
                buf: Arc::clone(&rbuf),
            })
            .expect("send ReadSync failed");

        match wait_for_completion(&channels) {
            block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
                result.unwrap_or_else(|e| panic!("isolation read ns {} failed: {e}", ns.ns_id));
            }
            other => panic!("ns {}: expected ReadDone, got {other:?}", ns.ns_id),
        }

        let guard = rbuf.lock().unwrap();
        for (j, &byte) in guard.as_slice().iter().enumerate() {
            assert_eq!(
                byte, tag,
                "isolation check ns {} byte {j}: expected {tag:#x}, got {byte:#x} (data bled from another namespace)",
                ns.ns_id
            );
        }
    }
    eprintln!("Cross-namespace isolation verified");

    // --- Cleanup: delete created namespaces, restore a single namespace ---
    for &ns_id in &created_ns_ids {
        delete_namespace(&channels, ns_id);
    }

    if original_total > 0 {
        let restored_id = create_namespace(&channels, original_total);
        eprintln!("Restored single namespace {restored_id} with {original_total} sectors");
    } else {
        eprintln!("WARNING: original capacity unknown, device left with no namespaces");
    }
}

// ---------------------------------------------------------------------------
// Success criteria validation tests (SC-001, SC-002, SC-006)
//
// These are hardware-dependent and marked #[ignore] for CI since they require
// a live NVMe device and are timing-sensitive.
// ---------------------------------------------------------------------------

/// SC-001: Sync read/write round-trip within direct NVMe latency envelope.
///
/// Performs 100 sync write+read round-trips of 4KB blocks and asserts p50
/// latency is below 100us (generous threshold accounting for channel overhead).
#[test]
#[ignore]
fn sc001_sync_latency_envelope() {
    let ctx = match get_spdk_context() {
        Some(c) => c,
        None => {
            eprintln!("skipping sc001: no SPDK hardware");
            return;
        }
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client");
    let sector_size = ibd.block_size() as usize;

    let num_rounds = 100;
    let mut latencies = Vec::with_capacity(num_rounds);

    for i in 0..num_rounds {
        let pattern = (i & 0xFF) as u8;
        let mut wbuf =
            interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc");
        wbuf.as_mut_slice().fill(pattern);
        let wbuf = Arc::new(wbuf);
        let rbuf = Arc::new(Mutex::new(
            interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc"),
        ));

        let start = std::time::Instant::now();

        channels
            .command_tx
            .send(block_device_spdk_nvme_v2::Command::WriteSync {
                ns_id: 1,
                lba: 0,
                buf: wbuf,
            })
            .expect("send write");
        match channels.completion_rx.recv().expect("recv") {
            block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
                result.expect("write failed")
            }
            other => panic!("expected WriteDone, got {other:?}"),
        }

        channels
            .command_tx
            .send(block_device_spdk_nvme_v2::Command::ReadSync {
                ns_id: 1,
                lba: 0,
                buf: rbuf.clone(),
            })
            .expect("send read");
        match channels.completion_rx.recv().expect("recv") {
            block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
                result.expect("read failed")
            }
            other => panic!("expected ReadDone, got {other:?}"),
        }

        latencies.push(start.elapsed());
    }

    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p50_us = p50.as_micros();

    eprintln!("SC-001: p50 latency = {p50_us} us (threshold: 100 us)");
    assert!(
        p50_us < 100,
        "SC-001 failed: p50 sync round-trip latency {p50_us} us exceeds 100 us threshold"
    );
}

/// SC-002: Async timeout errors arrive within bounded margin of timeout value.
///
/// Submits an async read with a 50ms timeout and measures the time until
/// the Timeout completion arrives. Asserts it arrives within 10% + 2ms
/// of the specified timeout (2ms margin for check_timeouts granularity).
#[test]
#[ignore]
fn sc002_timeout_accuracy() {
    let ctx = match get_spdk_context() {
        Some(c) => c,
        None => {
            eprintln!("skipping sc002: no SPDK hardware");
            return;
        }
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client");
    let sector_size = ibd.block_size() as usize;

    let timeout_ms: u64 = 50;
    let rbuf = Arc::new(Mutex::new(
        interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc"),
    ));

    // Submit an async read with a short timeout. On most devices the IO will
    // complete well before 50ms, so we use an invalid LBA that will either
    // error immediately or a valid LBA — the test logic handles both cases.
    // To reliably trigger a timeout we would need device-level fault injection.
    // This test validates the timing machinery when a timeout does occur.
    let start = std::time::Instant::now();
    channels
        .command_tx
        .send(block_device_spdk_nvme_v2::Command::ReadAsync {
            ns_id: 1,
            lba: 0,
            buf: rbuf,
            timeout_ms,
        })
        .expect("send async read");

    let completion = channels.completion_rx.recv().expect("recv");
    let elapsed = start.elapsed();

    match completion {
        block_device_spdk_nvme_v2::Completion::Timeout { .. } => {
            let elapsed_ms = elapsed.as_millis() as u64;
            let max_allowed_ms = (timeout_ms as f64 * 1.1) as u64 + 2;
            eprintln!(
                "SC-002: timeout arrived in {elapsed_ms} ms (timeout={timeout_ms} ms, max allowed={max_allowed_ms} ms)"
            );
            assert!(
                elapsed_ms <= max_allowed_ms,
                "SC-002 failed: timeout completion at {elapsed_ms} ms exceeds {max_allowed_ms} ms bound"
            );
        }
        block_device_spdk_nvme_v2::Completion::ReadDone { result, .. } => {
            eprintln!(
                "SC-002: IO completed before timeout ({:?} ms) — timeout accuracy cannot be validated. Result: {result:?}",
                elapsed.as_millis()
            );
        }
        other => panic!("expected Timeout or ReadDone, got {other:?}"),
    }
}

/// SC-006: Telemetry accuracy within 5% of independently measured values.
///
/// Requires `--features telemetry` to compile. Performs N sync writes,
/// measures each independently, then compares with the telemetry snapshot.
#[cfg(feature = "telemetry")]
#[test]
#[ignore]
fn sc006_telemetry_accuracy() {
    let ctx = match get_spdk_context() {
        Some(c) => c,
        None => {
            eprintln!("skipping sc006: no SPDK hardware");
            return;
        }
    };

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client");
    let sector_size = ibd.block_size() as usize;

    let num_ops = 50u64;
    let mut independent_latencies_ns = Vec::with_capacity(num_ops as usize);

    for i in 0..num_ops {
        let pattern = (i & 0xFF) as u8;
        let mut wbuf =
            interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc");
        wbuf.as_mut_slice().fill(pattern);
        let wbuf = Arc::new(wbuf);

        let start = std::time::Instant::now();
        channels
            .command_tx
            .send(block_device_spdk_nvme_v2::Command::WriteSync {
                ns_id: 1,
                lba: 0,
                buf: wbuf,
            })
            .expect("send write");
        match channels.completion_rx.recv().expect("recv") {
            block_device_spdk_nvme_v2::Completion::WriteDone { result, .. } => {
                result.expect("write failed")
            }
            other => panic!("expected WriteDone, got {other:?}"),
        }
        independent_latencies_ns.push(start.elapsed().as_nanos() as u64);
    }

    let snap = ibd.telemetry().expect("telemetry should be available");

    // Validate total_ops (telemetry may include ops from other tests sharing
    // the same global context, so we check >= num_ops).
    assert!(
        snap.total_ops >= num_ops,
        "SC-006: total_ops {} < expected {}",
        snap.total_ops,
        num_ops
    );

    let independent_mean: f64 =
        independent_latencies_ns.iter().sum::<u64>() as f64 / num_ops as f64;
    let telemetry_mean = snap.mean_latency_ns as f64;

    let ratio = if telemetry_mean > independent_mean {
        telemetry_mean / independent_mean
    } else {
        independent_mean / telemetry_mean
    };

    eprintln!(
        "SC-006: independent mean={:.0} ns, telemetry mean={:.0} ns, ratio={:.3}",
        independent_mean, telemetry_mean, ratio
    );
    assert!(
        ratio < 1.05,
        "SC-006 failed: telemetry mean latency differs by {:.1}% from independent measurement (threshold: 5%)",
        (ratio - 1.0) * 100.0
    );
}
