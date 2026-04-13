mod cuda_helpers;

use cuda_helpers::check_prerequisites;
use nvidia_p2p_pin::NvP2pDevice;

const SIZE_64KB: u64 = 65536;
const SIZE_1MB: u64 = 1024 * 1024;

/// Helper: run prerequisite check and return CudaRuntime, or skip the test.
macro_rules! skip_if_no_prereqs {
    () => {
        match check_prerequisites() {
            Ok(runtime) => runtime,
            Err(reason) => {
                println!("{}", reason);
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// User Story 1: Allocate and Pin GPU Memory End-to-End
// ---------------------------------------------------------------------------

#[test]
fn test_cuda_pin_1mb() {
    let runtime = skip_if_no_prereqs!();

    // Allocate 1 MB of GPU memory
    let cuda_mem = runtime.malloc(SIZE_1MB as usize).unwrap_or_else(|e| {
        println!("{}", e);
        panic!("cudaMalloc(1MB) failed unexpectedly after prerequisites passed");
    });

    // Verify device pointer is valid and 64KB-aligned
    assert_ne!(cuda_mem.devptr(), 0, "cudaMalloc returned null pointer");
    assert_eq!(
        cuda_mem.devptr() % SIZE_64KB,
        0,
        "cudaMalloc pointer not 64KB-aligned"
    );

    // Open P2P device and pin
    let device = NvP2pDevice::open().expect("failed to open /dev/nvidia_p2p");
    let pinned = device
        .pin_gpu_memory(cuda_mem.devptr(), SIZE_1MB)
        .expect("pin_gpu_memory(1MB) failed");

    // Validate: 1MB / 64KB = 16 pages
    assert_eq!(
        pinned.page_count(),
        16,
        "expected 16 pages for 1MB at 64KB page size"
    );

    // Validate all physical addresses are non-zero
    for (i, &addr) in pinned.physical_addresses().iter().enumerate() {
        assert_ne!(addr, 0, "physical_address[{}] is zero", i);
    }

    // Drop order: pinned first, then cuda_mem
    drop(pinned);
    drop(cuda_mem);
}

#[test]
fn test_cuda_pin_64kb_minimum() {
    let runtime = skip_if_no_prereqs!();

    let cuda_mem = runtime.malloc(SIZE_64KB as usize).unwrap_or_else(|e| {
        println!("{}", e);
        panic!("cudaMalloc(64KB) failed unexpectedly");
    });

    let device = NvP2pDevice::open().expect("failed to open /dev/nvidia_p2p");
    let pinned = device
        .pin_gpu_memory(cuda_mem.devptr(), SIZE_64KB)
        .expect("pin_gpu_memory(64KB) failed");

    assert_eq!(pinned.page_count(), 1, "expected 1 page for 64KB");
    assert_ne!(
        pinned.physical_addresses()[0],
        0,
        "single physical address is zero"
    );

    drop(pinned);
    drop(cuda_mem);
}

#[test]
fn test_cuda_pin_unpin_lifecycle() {
    let runtime = skip_if_no_prereqs!();

    let cuda_mem = runtime.malloc(SIZE_1MB as usize).unwrap_or_else(|e| {
        println!("{}", e);
        panic!("cudaMalloc(1MB) failed unexpectedly");
    });

    let device = NvP2pDevice::open().expect("failed to open /dev/nvidia_p2p");

    // First pin
    let pinned = device
        .pin_gpu_memory(cuda_mem.devptr(), SIZE_1MB)
        .expect("first pin_gpu_memory(1MB) failed");

    // Explicit unpin
    pinned.unpin().expect("unpin() returned error");

    // Second pin of same address should succeed (resources fully released)
    let pinned2 = device
        .pin_gpu_memory(cuda_mem.devptr(), SIZE_1MB)
        .expect("second pin_gpu_memory(1MB) failed after unpin");

    drop(pinned2);
    drop(cuda_mem);
}

#[test]
fn test_cuda_skip_no_prerequisites() {
    // This test validates the skip mechanism itself.
    // It always passes: if prerequisites are missing, we print and return;
    // if they are present, we just confirm check_prerequisites() returned Ok.
    match check_prerequisites() {
        Ok(_runtime) => {
            println!("Prerequisites available — skip mechanism not exercised");
        }
        Err(reason) => {
            println!("{}", reason);
            // The test passes — graceful skip confirmed.
        }
    }
}

// ---------------------------------------------------------------------------
// User Story 2: Validate Alignment Requirements
// ---------------------------------------------------------------------------

#[test]
fn test_cuda_alignment() {
    let runtime = skip_if_no_prereqs!();

    for &size in &[SIZE_64KB, SIZE_1MB, 16 * SIZE_1MB] {
        let cuda_mem = runtime.malloc(size as usize).unwrap_or_else(|e| {
            println!("{}", e);
            panic!("cudaMalloc({} bytes) failed unexpectedly", size);
        });
        assert_eq!(
            cuda_mem.devptr() % SIZE_64KB,
            0,
            "cudaMalloc({} bytes) returned non-64KB-aligned pointer: 0x{:x}",
            size,
            cuda_mem.devptr()
        );
    }
}

#[test]
fn test_cuda_multi_size_alignment() {
    let runtime = skip_if_no_prereqs!();

    let sizes: &[u64] = &[
        SIZE_64KB,
        256 * 1024,
        SIZE_1MB,
        4 * SIZE_1MB,
        16 * SIZE_1MB,
    ];

    for &size in sizes {
        let cuda_mem = runtime.malloc(size as usize).unwrap_or_else(|e| {
            println!("{}", e);
            panic!("cudaMalloc({} bytes) failed unexpectedly", size);
        });
        assert_eq!(
            cuda_mem.devptr() % SIZE_64KB,
            0,
            "cudaMalloc({} bytes) returned non-64KB-aligned pointer: 0x{:x}",
            size,
            cuda_mem.devptr()
        );
        // Free each allocation after checking
        drop(cuda_mem);
    }
}
