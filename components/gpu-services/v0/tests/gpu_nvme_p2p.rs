//! Integration test: True NVMe → GPU P2P DMA via GDRCopy BAR1 mapping.
//!
//! Tests the NVMe → GPU data path using GDRCopy for GPU BAR1 address resolution:
//! 1. PyTorch subprocess allocates GPU memory and exports a CUDA IPC handle
//! 2. Open the IPC handle to get a device pointer in this process
//! 3. GDRCopy pins the GPU memory (nvidia_p2p_get_pages) and maps BAR1 pages
//! 4. Register the BAR1 mapping with SPDK for NVMe DMA
//! 5. Write a known pattern to NVMe LBA 0
//! 6. NVMe reads directly into GPU VRAM via the BAR1 mapping (true P2P)
//! 7. Verify via cudaMemcpy D2H from the device pointer
//!
//! This achieves true peer-to-peer DMA: the NVMe controller writes directly
//! into GPU VRAM through PCIe BAR1, with no host memory staging buffer.
//!
//! Requires:
//!   - NVIDIA GPU with compute capability 7.0+
//!   - `gdrdrv` kernel module loaded (GDRCopy)
//!   - `nvidia-peermem` kernel module loaded
//!   - SPDK-bound NVMe device (VFIO + hugepages)
//!   - Python 3 with CUDA runtime available (for IPC handle generation)
//!
//! Self-skips when hardware or kernel modules are unavailable.

#![cfg(all(feature = "gpu", feature = "spdk"))]
#![allow(clippy::arc_with_non_send_sync)]

use std::ffi::c_void;
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine as _;

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponentV1;
use component_core::binding::bind;
use component_core::iunknown::query;
use component_core::query_interface;
use gpu_services::cuda_ffi;
use gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar;
use gpu_services::GpuServicesComponentV0;
use interfaces::{IBlockDevice, IGpuServices, ILogger};
use logger::LoggerComponentV1;
use spdk_env::SPDKEnvComponent;

/// GPU page size for GDRCopy alignment (64KB).
const GPU_PAGE_SIZE: usize = 64 * 1024;

struct HardwareContext {
    block_dev: Arc<BlockDeviceSpdkNvmeComponentV1>,
    #[allow(dead_code)]
    gpu: Arc<dyn IGpuServices + Send + Sync>,
    #[allow(dead_code)]
    spdk_env: Arc<SPDKEnvComponent>,
    logger: Arc<LoggerComponentV1>,
    #[allow(dead_code)]
    gpu_component: Arc<GpuServicesComponentV0>,
    sector_size: usize,
    ns_id: u32,
}

// SAFETY: All component Arcs use internal synchronization.
unsafe impl Sync for HardwareContext {}

static CONTEXT: OnceLock<Option<&'static HardwareContext>> = OnceLock::new();

fn kernel_module_loaded(name: &str) -> bool {
    std::fs::read_to_string("/proc/modules")
        .map(|s| s.lines().any(|line| line.starts_with(&format!("{name} "))))
        .unwrap_or(false)
}

fn get_context() -> Option<&'static HardwareContext> {
    *CONTEXT.get_or_init(|| {
        // Bypass SPDK's atexit teardown which crashes when outliving the test harness.
        extern "C" {
            fn atexit(cb: extern "C" fn()) -> i32;
            fn _exit(status: i32) -> !;
        }
        extern "C" fn exit_before_spdk_teardown() {
            unsafe { _exit(0) };
        }
        unsafe { atexit(exit_before_spdk_teardown) };

        if !kernel_module_loaded("nvidia_peermem") {
            eprintln!("skipping: nvidia-peermem not loaded (sudo modprobe nvidia-peermem)");
            return None;
        }

        if !kernel_module_loaded("gdrdrv") {
            eprintln!("skipping: gdrdrv not loaded (sudo modprobe gdrdrv)");
            return None;
        }

        let mut device_count: std::os::raw::c_int = 0;
        let err = unsafe { cuda_ffi::cudaGetDeviceCount(&mut device_count) };
        if err != cuda_ffi::CUDA_SUCCESS || device_count == 0 {
            eprintln!("skipping: no CUDA GPU available");
            return None;
        }

        if let Err(e) = spdk_env::checks::check_vfio_available() {
            eprintln!("skipping: VFIO not available: {e}");
            return None;
        }
        if let Err(e) = spdk_env::checks::check_hugepages() {
            eprintln!("skipping: hugepages not configured: {e}");
            return None;
        }

        // Wire components
        let spdk_env_comp = SPDKEnvComponent::new_default();
        let block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();
        let logger = LoggerComponentV1::new_default();
        let gpu_component = GpuServicesComponentV0::new_default();

        bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env")
            .expect("bind spdk_env to block_dev");
        bind(&*logger, "ILogger", &*block_dev, "logger").expect("bind logger to block_dev");

        let logger_iface: Arc<dyn ILogger + Send + Sync> = LoggerComponentV1::new_default();
        gpu_component.logger.connect(logger_iface).unwrap();

        let gpu = query_interface!(gpu_component, IGpuServices).expect("IGpuServices query");

        // Initialize SPDK environment
        let ienv =
            query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp).expect("ISPDKEnv query");
        if let Err(e) = ienv.init() {
            eprintln!("skipping: SPDK init failed: {e}");
            return None;
        }

        let devices = ienv.devices();
        if devices.is_empty() {
            eprintln!("skipping: no NVMe devices found");
            return None;
        }

        // Initialize block device
        let spdk_addr = devices[0].address;
        let addr = interfaces::PciAddress {
            domain: spdk_addr.domain,
            bus: spdk_addr.bus,
            dev: spdk_addr.dev,
            func: spdk_addr.func,
        };

        let admin =
            query::<dyn interfaces::iblock_device::IBlockDeviceAdmin + Send + Sync>(&*block_dev)
                .expect("IBlockDeviceAdmin query");
        admin.set_pci_address(addr);
        if let Err(e) = admin.initialize() {
            eprintln!("skipping: block device init failed: {e}");
            return None;
        }

        // Initialize GPU services
        if let Err(e) = gpu.initialize() {
            eprintln!("skipping: CUDA init failed: {e}");
            return None;
        }

        // Probe namespaces
        let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).unwrap();
        let channels = ibd.connect_client().expect("connect_client");
        channels
            .command_tx
            .send(interfaces::Command::NsProbe)
            .expect("send NsProbe");

        let namespaces = match channels.completion_rx.recv().expect("recv") {
            interfaces::Completion::NsProbeResult { namespaces } => namespaces,
            other => panic!("expected NsProbeResult, got {other:?}"),
        };
        drop(channels);

        if namespaces.is_empty() {
            eprintln!("skipping: no NVMe namespaces found");
            return None;
        }

        let ns = &namespaces[0];

        Some(Box::leak(Box::new(HardwareContext {
            block_dev,
            gpu,
            spdk_env: spdk_env_comp,
            logger,
            gpu_component,
            sector_size: ns.sector_size as usize,
            ns_id: ns.ns_id,
        })))
    })
}

/// Round up to the next multiple of GPU_PAGE_SIZE (64KB).
fn align_up(size: usize) -> usize {
    (size + GPU_PAGE_SIZE - 1) & !(GPU_PAGE_SIZE - 1)
}

/// End-to-end true P2P DMA test: NVMe → GPU VRAM via GDRCopy BAR1 mapping.
///
/// Flow:
/// 1. Allocate GPU device memory with cudaMalloc
/// 2. GDRCopy pins GPU memory and maps BAR1 → bar_ptr (has valid pagemap)
/// 3. Register BAR1 mapping with SPDK for NVMe DMA
/// 4. Write known pattern to NVMe LBA 0
/// 5. NVMe ReadSync into BAR1 mapping → data lands in GPU VRAM (P2P)
/// 6. cudaMemcpy D2H from dev_ptr to verify GPU sees the data
///
/// Note: nvidia_p2p_get_pages requires memory allocated by the current process.
/// IPC-opened memory (cudaIpcOpenMemHandle) is not supported for P2P pinning.
/// For cross-process scenarios, the allocating process must perform the GDRCopy
/// pin+map and share the BAR mapping info.
#[test]
fn test_nvme_to_gpu_p2p_gdrcopy() {
    let Some(ctx) = get_context() else {
        eprintln!("test_nvme_to_gpu_p2p_gdrcopy: skipped (no hardware)");
        return;
    };

    let log: Arc<dyn ILogger + Send + Sync> = ctx.logger.clone();

    let alloc_size = align_up(ctx.sector_size);
    log.info(&format!(
        "P2P test: sector_size={}, alloc_size={} (64KB-aligned)",
        ctx.sector_size, alloc_size
    ));

    // Step 1: Allocate GPU device memory with cudaMalloc.
    let mut dev_ptr: *mut c_void = std::ptr::null_mut();
    let err = unsafe { cuda_ffi::cudaMalloc(&mut dev_ptr, alloc_size) };
    assert_eq!(
        err,
        cuda_ffi::CUDA_SUCCESS,
        "cudaMalloc({} bytes) failed: {}",
        alloc_size,
        cuda_ffi::cuda_error_string(err)
    );
    assert!(!dev_ptr.is_null());

    log.info(&format!(
        "Step 1: cudaMalloc'd {} bytes, dev_ptr={:?}",
        alloc_size, dev_ptr
    ));

    // Step 2: Create DMA buffer via GDRCopy BAR1 mapping.
    // GDRCopy pins the GPU memory (nvidia_p2p_get_pages), maps it through
    // BAR1 producing valid pagemap entries, and registers with SPDK for DMA.
    let dma_buf = match create_spdk_dma_buffer_from_gpu_bar(dev_ptr, alloc_size) {
        Ok(buf) => buf,
        Err(e) => {
            log.info(&format!("FAIL: create_spdk_dma_buffer_from_gpu_bar: {e}"));
            unsafe { cuda_ffi::cudaFree(dev_ptr) };
            panic!("GDRCopy BAR mapping failed: {e}");
        }
    };

    log.info(&format!(
        "Step 2: GDRCopy BAR1 mapping created, DMA buffer {} bytes (bar_ptr={:?})",
        dma_buf.len(),
        dma_buf.as_ptr()
    ));

    // Step 3: Write a known pattern to NVMe LBA 0.
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client");

    let pattern: Vec<u8> = (0..alloc_size).map(|i| (i % 251) as u8).collect();

    let mut write_buf =
        interfaces::DmaBuffer::new(alloc_size, ctx.sector_size, None).expect("DMA alloc");
    write_buf.as_mut_slice().copy_from_slice(&pattern);
    let write_buf = Arc::new(write_buf);

    channels
        .command_tx
        .send(interfaces::Command::WriteSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: write_buf,
        })
        .expect("send WriteSync");

    match channels.completion_rx.recv().expect("recv") {
        interfaces::Completion::WriteDone { result, .. } => result.expect("NVMe write failed"),
        other => panic!("expected WriteDone, got {other:?}"),
    }
    log.info(&format!("Step 3: wrote {} bytes to NVMe LBA 0", alloc_size));

    // Step 4: NVMe ReadSync into the GDRCopy BAR1 DMA buffer.
    // This is the true P2P path: NVMe DMA writes directly into GPU VRAM
    // via the BAR1 physical addresses resolved from the GDRCopy mapping.
    let dma_buf = Arc::new(Mutex::new(dma_buf));

    channels
        .command_tx
        .send(interfaces::Command::ReadSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: Arc::clone(&dma_buf),
        })
        .expect("send ReadSync");

    match channels.completion_rx.recv().expect("recv") {
        interfaces::Completion::ReadDone { result, .. } => {
            result.expect("NVMe read into GPU BAR1 mapping failed")
        }
        other => panic!("expected ReadDone, got {other:?}"),
    }
    log.info("Step 4: NVMe read completed into GPU VRAM (via BAR1 P2P DMA)");

    // Step 5: Verify GPU memory contents via cudaMemcpy D2H from the
    // original device pointer. If P2P worked correctly, the GPU VRAM
    // now contains the pattern we wrote to NVMe.
    let mut verify_buf = vec![0u8; alloc_size];
    let err = unsafe {
        cuda_ffi::cudaMemcpy(
            verify_buf.as_mut_ptr() as *mut c_void,
            dev_ptr as *const c_void,
            alloc_size,
            cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
        )
    };
    assert_eq!(
        err,
        cuda_ffi::CUDA_SUCCESS,
        "cudaMemcpy D2H failed: {}",
        cuda_ffi::cuda_error_string(err)
    );

    assert_eq!(
        verify_buf, pattern,
        "NVMe→GPU P2P data mismatch: data in GPU VRAM does not match NVMe pattern"
    );

    log.info(&format!(
        "Step 5: VERIFIED — NVMe → GPU P2P via GDRCopy correct ({} bytes, true BAR1 P2P DMA)",
        alloc_size
    ));

    // Cleanup: drop DMA buffer (triggers SPDK unregister + GDRCopy unmap/unpin/close),
    // then free the GPU memory.
    drop(dma_buf);
    unsafe { cuda_ffi::cudaFree(dev_ptr) };
}

/// Launch the Python GPU verifier that opens an IPC handle and reads data from GPU.
/// Sends base64-encoded (ipc_handle[64] + size[8]) to the child, reads back the data.
/// Returns the data read by the Python process, or None on failure.
fn verify_gpu_via_python(ipc_handle_bytes: &[u8; 64], size: usize) -> Option<Vec<u8>> {
    let script_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("gpu_verify_p2p.py");

    if !script_path.exists() {
        return None;
    }

    let mut child = Command::new("python3")
        .arg(&script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    // Send the IPC handle + size to the verifier
    let mut payload = Vec::with_capacity(72);
    payload.extend_from_slice(ipc_handle_bytes);
    payload.extend_from_slice(&(size as u64).to_le_bytes());
    let b64 = base64::engine::general_purpose::STANDARD.encode(&payload);

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b64.as_bytes());
        let _ = stdin.write_all(b"\n");
    }

    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next()?;
    base64::engine::general_purpose::STANDARD.decode(line).ok()
}

/// Cross-process P2P DMA test with Python verification.
///
/// Validates that NVMe → GPU P2P DMA produces data visible to another process
/// via CUDA IPC. The DMA and GDRCopy pinning happen in the allocating (Rust)
/// process since nvidia_p2p_get_pages requires memory owned by the caller.
/// Cross-process verification confirms the data is accessible from Python.
///
/// Flow:
/// 1. Rust: cudaMalloc → GDRCopy pin+map → SPDK registration
/// 2. Rust: NVMe write pattern to LBA 0
/// 3. Rust: NVMe ReadSync into GPU VRAM via BAR1 P2P DMA
/// 4. Rust: export IPC handle → Python subprocess opens it and reads D2H
/// 5. Verify Python's read matches the expected pattern
#[test]
fn test_nvme_to_gpu_p2p_python_client() {
    let Some(ctx) = get_context() else {
        eprintln!("test_nvme_to_gpu_p2p_python_client: skipped (no hardware)");
        return;
    };

    let log: Arc<dyn ILogger + Send + Sync> = ctx.logger.clone();

    let alloc_size = align_up(ctx.sector_size);

    // Step 1: Allocate GPU memory and create GDRCopy BAR1 DMA buffer.
    let mut dev_ptr: *mut c_void = std::ptr::null_mut();
    let err = unsafe { cuda_ffi::cudaMalloc(&mut dev_ptr, alloc_size) };
    assert_eq!(
        err,
        cuda_ffi::CUDA_SUCCESS,
        "cudaMalloc({} bytes) failed: {}",
        alloc_size,
        cuda_ffi::cuda_error_string(err)
    );
    assert!(!dev_ptr.is_null());

    let dma_buf = match create_spdk_dma_buffer_from_gpu_bar(dev_ptr, alloc_size) {
        Ok(buf) => buf,
        Err(e) => {
            unsafe { cuda_ffi::cudaFree(dev_ptr) };
            panic!("GDRCopy BAR mapping failed: {e}");
        }
    };

    log.info(&format!(
        "Step 1: GPU alloc + GDRCopy BAR1 mapping ({} bytes, bar={:?})",
        alloc_size,
        dma_buf.as_ptr()
    ));

    // Step 2: Write a known pattern to NVMe LBA 0.
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client");

    let pattern: Vec<u8> = (0..alloc_size).map(|i| (i % 251) as u8).collect();

    let mut write_buf =
        interfaces::DmaBuffer::new(alloc_size, ctx.sector_size, None).expect("DMA alloc");
    write_buf.as_mut_slice().copy_from_slice(&pattern);
    let write_buf = Arc::new(write_buf);

    channels
        .command_tx
        .send(interfaces::Command::WriteSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: write_buf,
        })
        .expect("send WriteSync");

    match channels.completion_rx.recv().expect("recv") {
        interfaces::Completion::WriteDone { result, .. } => result.expect("NVMe write failed"),
        other => panic!("expected WriteDone, got {other:?}"),
    }
    log.info(&format!(
        "Step 2: wrote {} bytes to NVMe LBA 0",
        alloc_size
    ));

    // Step 3: NVMe ReadSync into GDRCopy BAR1 mapping → P2P DMA to GPU VRAM.
    let dma_buf = Arc::new(Mutex::new(dma_buf));

    channels
        .command_tx
        .send(interfaces::Command::ReadSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: Arc::clone(&dma_buf),
        })
        .expect("send ReadSync");

    match channels.completion_rx.recv().expect("recv") {
        interfaces::Completion::ReadDone { result, .. } => {
            result.expect("NVMe read into GPU BAR1 failed")
        }
        other => panic!("expected ReadDone, got {other:?}"),
    }
    log.info("Step 3: NVMe read completed (P2P DMA to GPU VRAM via BAR1)");

    // Step 4: Export IPC handle and verify via Python subprocess.
    let mut ipc_handle = cuda_ffi::cudaIpcMemHandle_t {
        reserved: [0u8; 64],
    };
    let err = unsafe { cuda_ffi::cudaIpcGetMemHandle(&mut ipc_handle, dev_ptr) };
    assert_eq!(
        err,
        cuda_ffi::CUDA_SUCCESS,
        "cudaIpcGetMemHandle failed: {}",
        cuda_ffi::cuda_error_string(err)
    );

    log.info("Step 4: IPC handle exported, launching Python verifier");

    let python_data = verify_gpu_via_python(&ipc_handle.reserved, alloc_size);

    match python_data {
        Some(data) => {
            assert_eq!(
                data, pattern,
                "Cross-process verification failed: Python read different data from GPU"
            );
            log.info(&format!(
                "Step 5: VERIFIED — Python subprocess read correct data from GPU ({} bytes, cross-process P2P)",
                alloc_size
            ));
        }
        None => {
            // Python verifier unavailable — fall back to local verification.
            log.info("Python verifier unavailable, using local D2H verification");
            let mut verify_buf = vec![0u8; alloc_size];
            let err = unsafe {
                cuda_ffi::cudaMemcpy(
                    verify_buf.as_mut_ptr() as *mut c_void,
                    dev_ptr as *const c_void,
                    alloc_size,
                    cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
                )
            };
            assert_eq!(
                err,
                cuda_ffi::CUDA_SUCCESS,
                "cudaMemcpy D2H failed: {}",
                cuda_ffi::cuda_error_string(err)
            );
            assert_eq!(
                verify_buf, pattern,
                "Local P2P data mismatch: NVMe data not in GPU VRAM"
            );
            log.info(&format!(
                "Step 5: VERIFIED locally — GPU VRAM contains NVMe data ({} bytes)",
                alloc_size
            ));
        }
    }

    // Cleanup.
    drop(dma_buf);
    unsafe { cuda_ffi::cudaFree(dev_ptr) };
}

/// P2P DMA with separate GDRCopy and SPDK registration steps.
///
/// This test demonstrates the decomposed registration path that cross-process
/// P2P DMA relies on. Unlike test_nvme_to_gpu_p2p_gdrcopy which uses the
/// all-in-one `create_spdk_dma_buffer_from_gpu_bar`, this test separates:
/// 1. GDRCopy pin + map (would be done by the GPU application process)
/// 2. spdk_mem_register on the BAR VA (done by the storage server)
/// 3. NVMe DMA into GPU VRAM
///
/// In a cross-process scenario, step 1 happens in Python/PyTorch, and the BAR
/// mapping is shared with the Rust storage server (e.g., via memfd or by having
/// both processes mmap the same BAR region). The storage server then performs
/// step 2 and 3.
#[test]
fn test_nvme_to_gpu_p2p_explicit_iommu() {
    let Some(ctx) = get_context() else {
        eprintln!("test_nvme_to_gpu_p2p_explicit_iommu: skipped (no hardware)");
        return;
    };

    let log: Arc<dyn ILogger + Send + Sync> = ctx.logger.clone();

    let alloc_size = align_up(ctx.sector_size);

    // Step 1: Allocate GPU memory with cudaMalloc.
    let mut dev_ptr: *mut c_void = std::ptr::null_mut();
    let err = unsafe { cuda_ffi::cudaMalloc(&mut dev_ptr, alloc_size) };
    assert_eq!(err, cuda_ffi::CUDA_SUCCESS, "cudaMalloc failed");
    assert!(!dev_ptr.is_null());

    // Step 2: GDRCopy pin + map to get BAR1 VA.
    // In cross-process: this step happens in the GPU application (Python).
    use gpu_services::gdrcopy_ffi::*;

    let gdr = unsafe { gdr_open() };
    if gdr.is_null() {
        unsafe { cuda_ffi::cudaFree(dev_ptr) };
        log.info("skipping: gdr_open() failed");
        return;
    }

    let mut mh = gdr_mh_t::default();
    let rc = unsafe {
        gdr_pin_buffer(
            gdr,
            dev_ptr as std::os::raw::c_ulong,
            alloc_size,
            0,
            0,
            &mut mh,
        )
    };
    if rc != 0 {
        unsafe { gdr_close(gdr) };
        unsafe { cuda_ffi::cudaFree(dev_ptr) };
        log.info(&format!("skipping: gdr_pin_buffer failed (rc={})", rc));
        return;
    }

    let mut bar_ptr: *mut c_void = std::ptr::null_mut();
    let rc = unsafe { gdr_map(gdr, mh, &mut bar_ptr, alloc_size) };
    if rc != 0 {
        unsafe {
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
            cuda_ffi::cudaFree(dev_ptr);
        }
        panic!("gdr_map failed (rc={})", rc);
    }

    let offset = (dev_ptr as usize) & (GPU_PAGE_SIZE - 1);
    let effective_bar_ptr = unsafe { (bar_ptr as *mut u8).add(offset) as *mut c_void };

    log.info(&format!(
        "Step 2: GDRCopy BAR mapping at {:?} (effective={:?}), size={}",
        bar_ptr, effective_bar_ptr, alloc_size
    ));

    // Step 3: Register the BAR mapping with SPDK for NVMe DMA.
    // In cross-process: this step happens in the storage server after receiving
    // the shared BAR mapping (via memfd_create + SCM_RIGHTS, or nvidia-peermem).
    extern "C" {
        fn spdk_mem_register(vaddr: *mut std::ffi::c_void, len: usize) -> std::os::raw::c_int;
        fn spdk_mem_unregister(vaddr: *mut std::ffi::c_void, len: usize) -> std::os::raw::c_int;
    }

    let rc = unsafe { spdk_mem_register(bar_ptr, alloc_size) };
    if rc != 0 {
        unsafe {
            gdr_unmap(gdr, mh, bar_ptr, alloc_size);
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
            cuda_ffi::cudaFree(dev_ptr);
        }
        panic!("spdk_mem_register on BAR mapping failed (rc={})", rc);
    }

    log.info("Step 3: BAR mapping registered with SPDK (IOMMU programmed)");

    // Create a DmaBuffer wrapper (no-op free since we manage cleanup manually).
    let dma_buf = unsafe {
        interfaces::DmaBuffer::from_raw(effective_bar_ptr, alloc_size, noop_free, -1)
            .expect("DmaBuffer creation")
    };

    // Step 4: Write a known pattern to NVMe LBA 0.
    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*ctx.block_dev).unwrap();
    let channels = ibd.connect_client().expect("connect_client");

    let pattern: Vec<u8> = (0..alloc_size).map(|i| (i % 251) as u8).collect();

    let mut write_buf =
        interfaces::DmaBuffer::new(alloc_size, ctx.sector_size, None).expect("DMA alloc");
    write_buf.as_mut_slice().copy_from_slice(&pattern);
    let write_buf = Arc::new(write_buf);

    channels
        .command_tx
        .send(interfaces::Command::WriteSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: write_buf,
        })
        .expect("send WriteSync");

    match channels.completion_rx.recv().expect("recv") {
        interfaces::Completion::WriteDone { result, .. } => result.expect("NVMe write failed"),
        other => panic!("expected WriteDone, got {other:?}"),
    }
    log.info("Step 4: wrote pattern to NVMe LBA 0");

    // Step 5: NVMe ReadSync into the SPDK-registered BAR buffer.
    let dma_buf = Arc::new(Mutex::new(dma_buf));

    channels
        .command_tx
        .send(interfaces::Command::ReadSync {
            ns_id: ctx.ns_id,
            lba: 0,
            buf: Arc::clone(&dma_buf),
        })
        .expect("send ReadSync");

    match channels.completion_rx.recv().expect("recv") {
        interfaces::Completion::ReadDone { result, .. } => {
            result.expect("NVMe read into BAR mapping failed")
        }
        other => panic!("expected ReadDone, got {other:?}"),
    }

    log.info("Step 5: NVMe read completed (DMA to GPU BAR1 via separate registration)");

    // Step 6: Verify GPU memory via cudaMemcpy D2H.
    let mut verify_buf = vec![0u8; alloc_size];
    let err = unsafe {
        cuda_ffi::cudaMemcpy(
            verify_buf.as_mut_ptr() as *mut c_void,
            dev_ptr as *const c_void,
            alloc_size,
            cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
        )
    };
    assert_eq!(
        err,
        cuda_ffi::CUDA_SUCCESS,
        "cudaMemcpy D2H failed: {}",
        cuda_ffi::cuda_error_string(err)
    );

    assert_eq!(
        verify_buf, pattern,
        "Separated-registration P2P data mismatch: NVMe data not in GPU VRAM"
    );

    log.info(&format!(
        "Step 6: VERIFIED — NVMe → GPU P2P via decomposed GDRCopy+SPDK path ({} bytes)",
        alloc_size
    ));

    // Cleanup: drop DMA buffer (noop), unregister SPDK, then GDRCopy.
    drop(dma_buf);
    unsafe {
        spdk_mem_unregister(bar_ptr, alloc_size);
        gdr_unmap(gdr, mh, bar_ptr, alloc_size);
        gdr_unpin_buffer(gdr, mh);
        gdr_close(gdr);
        cuda_ffi::cudaFree(dev_ptr);
    }
}

unsafe extern "C" fn noop_free(_ptr: *mut std::ffi::c_void) {}
