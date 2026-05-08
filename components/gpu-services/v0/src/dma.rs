//! DMA buffer creation from GPU IPC handles.

#[cfg(feature = "gpu")]
use crate::cuda_ffi;

#[cfg(feature = "gpu")]
use interfaces::{GpuDmaBuffer, GpuIpcHandle};

// SPDK functions used by the general GPU+SPDK DMA path.
#[cfg(all(feature = "gpu", feature = "spdk"))]
extern "C" {
    fn spdk_mem_register(vaddr: *mut std::ffi::c_void, len: usize) -> std::os::raw::c_int;
    fn spdk_mem_unregister(vaddr: *mut std::ffi::c_void, len: usize) -> std::os::raw::c_int;
}

// DPDK/VFIO functions only used by the P2P (GDRCopy) path.
#[cfg(feature = "p2p")]
extern "C" {
    fn rte_extmem_register(
        va_addr: *mut std::ffi::c_void,
        len: usize,
        iova_addrs: *const u64,
        n_pages: std::os::raw::c_uint,
        page_sz: usize,
    ) -> std::os::raw::c_int;
    fn rte_extmem_unregister(va_addr: *mut std::ffi::c_void, len: usize) -> std::os::raw::c_int;
    fn rte_vfio_container_dma_map(
        container_fd: std::os::raw::c_int,
        vaddr: u64,
        iova: u64,
        len: u64,
    ) -> std::os::raw::c_int;
    fn rte_vfio_container_dma_unmap(
        container_fd: std::os::raw::c_int,
        vaddr: u64,
        iova: u64,
        len: u64,
    ) -> std::os::raw::c_int;
    fn spdk_vtophys(buf: *const std::ffi::c_void, size: *mut u64) -> u64;
}

/// Free function for GpuDmaBuffer that closes the CUDA IPC handle.
#[cfg(feature = "gpu")]
unsafe extern "C" fn cuda_ipc_close_mem_handle(ptr: *mut std::ffi::c_void) {
    // SAFETY: ptr was obtained from cudaIpcOpenMemHandle and has not been closed.
    unsafe {
        cuda_ffi::cudaIpcCloseMemHandle(ptr);
    }
}

/// Tracks registered GPU memory regions so free functions can look up sizes.
/// DmaBuffer's free_fn signature is `fn(*mut c_void)` — it doesn't receive
/// the size, so we store it here at registration time.
#[cfg(all(feature = "gpu", feature = "spdk"))]
static REGISTERED_REGIONS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, usize>>,
> = std::sync::OnceLock::new();

#[cfg(all(feature = "gpu", feature = "spdk"))]
fn registered_regions() -> &'static std::sync::Mutex<std::collections::HashMap<usize, usize>> {
    REGISTERED_REGIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Free function that unregisters from SPDK, then closes the CUDA IPC handle.
///
/// Used when the memory was already pinned before `prepare_memory_for_spdk`
/// was called — we must not unpin memory that belongs to the originating process,
/// but we still need to unregister from SPDK.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub unsafe extern "C" fn spdk_unregister_and_ipc_close(ptr: *mut std::ffi::c_void) {
    unsafe {
        let size = registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize))
            .unwrap_or(0);
        if size > 0 {
            spdk_mem_unregister(ptr, size);
        }
        cuda_ffi::cudaIpcCloseMemHandle(ptr);
    }
}

/// Free function that unregisters from SPDK, unpins memory, then closes IPC handle.
///
/// Used when `prepare_memory_for_spdk` itself pinned the memory — on drop
/// we must unregister from SPDK, undo the pin, then close the handle.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub unsafe extern "C" fn spdk_unregister_unpin_and_ipc_close(ptr: *mut std::ffi::c_void) {
    unsafe {
        let size = registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize))
            .unwrap_or(0);
        if size > 0 {
            spdk_mem_unregister(ptr, size);
        }
        cuda_ffi::cudaHostUnregister(ptr);
        cuda_ffi::cudaIpcCloseMemHandle(ptr);
    }
}

/// Create an SPDK `DmaBuffer` from a GPU device pointer with the appropriate
/// free function based on whether the memory was already pinned.
///
/// Registers the GPU memory with SPDK (`spdk_mem_register`) so that SPDK's
/// vtophys translation works for DMA operations. Requires nvidia-peermem
/// kernel module for GPU BAR memory to be IOMMU-accessible.
///
/// * `was_already_pinned = true` → uses close-only free (no unpin on drop)
/// * `was_already_pinned = false` → uses unpin + close free (undo pin on drop)
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub fn create_spdk_dma_buffer_from_gpu(
    ptr: *mut std::ffi::c_void,
    size: usize,
    was_already_pinned: bool,
) -> Result<interfaces::DmaBuffer, String> {
    // Register GPU memory with SPDK so vtophys can resolve the physical address.
    // This requires nvidia-peermem to be loaded for GPU device memory.
    // SAFETY: ptr is a valid device pointer of `size` bytes from cudaIpcOpenMemHandle.
    let rc = unsafe { spdk_mem_register(ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed (rc={}). Is nvidia-peermem loaded?",
            rc
        ));
    }

    // Record size so the free function can unregister the correct range.
    registered_regions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(ptr as usize, size);

    let free_fn: unsafe extern "C" fn(*mut std::ffi::c_void) = if was_already_pinned {
        spdk_unregister_and_ipc_close
    } else {
        spdk_unregister_unpin_and_ipc_close
    };

    // SAFETY: ptr is a valid GPU device pointer obtained from cudaIpcOpenMemHandle,
    // now registered with SPDK for DMA. size is the correct allocation size.
    // free_fn handles SPDK unregister + cleanup based on pin state.
    // numa_node = -1 because GPU device memory has no CPU NUMA affinity.
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(ptr, size, free_fn, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));
        unsafe {
            spdk_mem_unregister(ptr, size);
        }
    }

    result
}

/// Free function that unregisters from SPDK then frees via cudaFree.
///
/// Used for directly-allocated GPU memory (cudaMalloc) rather than
/// IPC-opened handles.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub unsafe extern "C" fn spdk_unregister_and_cuda_free(ptr: *mut std::ffi::c_void) {
    unsafe {
        let size = registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize))
            .unwrap_or(0);
        if size > 0 {
            spdk_mem_unregister(ptr, size);
        }
        cuda_ffi::cudaFree(ptr);
    }
}

/// Create an SPDK `DmaBuffer` from a `cudaMalloc`-allocated GPU pointer.
///
/// Registers the GPU memory with SPDK (`spdk_mem_register`) for DMA via
/// nvidia-peermem and returns a `DmaBuffer` that will unregister and
/// `cudaFree` on drop.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub fn create_spdk_dma_buffer_from_cuda_malloc(
    ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    // SAFETY: ptr is a valid device pointer of `size` bytes from cudaMalloc.
    let rc = unsafe { spdk_mem_register(ptr, size) };
    if rc != 0 {
        return Err(format!(
            "spdk_mem_register failed (rc={}). Is nvidia-peermem loaded?",
            rc
        ));
    }

    registered_regions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(ptr as usize, size);

    // SAFETY: ptr is a valid GPU device pointer from cudaMalloc, now registered
    // with SPDK for DMA. free_fn handles SPDK unregister + cudaFree.
    // numa_node = -1 because GPU device memory has no CPU NUMA affinity.
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(ptr, size, spdk_unregister_and_cuda_free, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));
        unsafe {
            spdk_mem_unregister(ptr, size);
        }
    }

    result
}

/// Free function that unregisters from SPDK then frees via cudaFreeHost.
///
/// Used for pinned host memory allocated with `cudaHostAlloc`.
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub unsafe extern "C" fn spdk_unregister_and_cuda_free_host(ptr: *mut std::ffi::c_void) {
    unsafe {
        let size = registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize))
            .unwrap_or(0);
        if size > 0 {
            spdk_mem_unregister(ptr, size);
        }
        cuda_ffi::cudaFreeHost(ptr);
    }
}

/// Create an SPDK `DmaBuffer` from a `cudaHostAlloc`-allocated pinned host pointer.
///
/// This memory is accessible by both CPU and GPU (via `cudaHostGetDevicePointer`).
/// NVMe can DMA directly into it since it's pinned host memory with valid
/// physical addresses. The GPU accesses it via P2P mapping managed by the
/// nvidia driver (nvidia-peermem).
#[cfg(all(feature = "gpu", feature = "spdk"))]
pub fn create_spdk_dma_buffer_from_cuda_host_alloc(
    ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    // SAFETY: ptr is a valid pinned host pointer from cudaHostAlloc.
    // It has valid physical addresses resolvable by SPDK vtophys.
    let rc = unsafe { spdk_mem_register(ptr, size) };
    if rc != 0 {
        return Err(format!("spdk_mem_register failed (rc={})", rc));
    }

    registered_regions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(ptr as usize, size);

    // SAFETY: ptr is a valid pinned host pointer, registered with SPDK.
    // free_fn handles SPDK unregister + cudaFreeHost.
    // numa_node = -1 (CUDA manages placement).
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(ptr, size, spdk_unregister_and_cuda_free_host, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        registered_regions()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));
        unsafe {
            spdk_mem_unregister(ptr, size);
        }
    }

    result
}

/// Tracks GDRCopy mapping state so free functions can perform full cleanup.
#[cfg(feature = "p2p")]
struct GdrMappingState {
    gdr: crate::gdrcopy_ffi::gdr_t,
    mh: crate::gdrcopy_ffi::gdr_mh_t,
    bar_ptr: *mut std::ffi::c_void,
    size: usize,
}

// SAFETY: GDRCopy handles are process-global and not thread-bound.
#[cfg(feature = "p2p")]
unsafe impl Send for GdrMappingState {}
#[cfg(feature = "p2p")]
unsafe impl Sync for GdrMappingState {}

#[cfg(feature = "p2p")]
static GDR_MAPPINGS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, GdrMappingState>>,
> = std::sync::OnceLock::new();

#[cfg(feature = "p2p")]
fn gdr_mappings() -> &'static std::sync::Mutex<std::collections::HashMap<usize, GdrMappingState>> {
    GDR_MAPPINGS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Free function: unregisters from SPDK, unmaps GDRCopy BAR mapping,
/// unpins the GPU buffer, and closes the GDRCopy handle.
#[cfg(feature = "p2p")]
pub unsafe extern "C" fn spdk_unregister_gdr_unmap_and_close(ptr: *mut std::ffi::c_void) {
    unsafe {
        let state = gdr_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));

        if let Some(s) = state {
            spdk_mem_unregister(s.bar_ptr, s.size);
            crate::gdrcopy_ffi::gdr_unmap(s.gdr, s.mh, s.bar_ptr, s.size);
            crate::gdrcopy_ffi::gdr_unpin_buffer(s.gdr, s.mh);
            crate::gdrcopy_ffi::gdr_close(s.gdr);
        }
    }
}

/// Create an SPDK `DmaBuffer` backed by a GDRCopy BAR1 mapping of GPU device memory.
///
/// This enables true NVMe→GPU P2P DMA: GDRCopy pins the GPU memory via
/// `nvidia_p2p_get_pages` and maps it through GPU BAR1, producing a CPU-visible
/// virtual address with valid pagemap entries pointing to GPU BAR1 physical
/// addresses. SPDK's vtophys can then resolve these for VFIO IOMMU DMA mapping.
///
/// The returned `DmaBuffer`'s pointer is the BAR1 mapping (for NVMe DMA targeting).
/// The actual GPU device pointer (`dev_ptr`) remains valid for CUDA access to the
/// same physical memory.
///
/// Requires:
///   - `gdrdrv` kernel module loaded
///   - `nvidia-peermem` kernel module loaded
///   - GPU memory allocated with `cudaMalloc` or opened via `cudaIpcOpenMemHandle`
///
/// On drop, the buffer unregisters from SPDK, unmaps BAR1, unpins GPU memory,
/// and closes the GDRCopy handle.
#[cfg(feature = "p2p")]
pub fn create_spdk_dma_buffer_from_gpu_bar(
    dev_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    use crate::gdrcopy_ffi::*;

    // Align size up to GPU page boundary (64KB).
    let aligned_size = (size + GPU_PAGE_SIZE - 1) & !(GPU_PAGE_SIZE - 1);

    // SAFETY: Opens a connection to the gdrdrv kernel module.
    let gdr = unsafe { gdr_open() };
    if gdr.is_null() {
        return Err("gdr_open() failed — is gdrdrv kernel module loaded?".to_string());
    }

    // SAFETY: dev_ptr is a valid CUDA device pointer; size is the allocation size.
    let mut mh = gdr_mh_t::default();
    let rc = unsafe {
        gdr_pin_buffer(
            gdr,
            dev_ptr as std::os::raw::c_ulong,
            aligned_size,
            0,
            0,
            &mut mh,
        )
    };
    if rc != 0 {
        unsafe { gdr_close(gdr) };
        return Err(format!("gdr_pin_buffer failed (rc={})", rc));
    }

    // SAFETY: mh is a valid pinned handle from gdr_pin_buffer.
    let mut bar_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let rc = unsafe { gdr_map(gdr, mh, &mut bar_ptr, aligned_size) };
    if rc != 0 {
        unsafe {
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
        }
        return Err(format!("gdr_map failed (rc={})", rc));
    }

    if bar_ptr.is_null() {
        unsafe {
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
        }
        return Err("gdr_map returned null pointer".to_string());
    }

    // Account for alignment offset: gdr_map may align the returned pointer
    // to the GPU page boundary. The offset between the aligned base and
    // the actual device pointer must be applied to bar_ptr.
    let offset = (dev_ptr as usize) & (GPU_PAGE_SIZE - 1);
    let effective_bar_ptr = unsafe { (bar_ptr as *mut u8).add(offset) as *mut std::ffi::c_void };

    // Register the BAR mapping with SPDK. The BAR pages have valid pagemap
    // entries pointing to GPU BAR1 physical addresses, so vtophys works.
    let rc = unsafe { spdk_mem_register(bar_ptr, aligned_size) };
    if rc != 0 {
        unsafe {
            gdr_unmap(gdr, mh, bar_ptr, aligned_size);
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
        }
        return Err(format!(
            "spdk_mem_register on BAR mapping failed (rc={}). Is nvidia-peermem loaded?",
            rc
        ));
    }

    // Store state for cleanup. Key by the effective pointer (what DmaBuffer holds).
    gdr_mappings()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            effective_bar_ptr as usize,
            GdrMappingState {
                gdr,
                mh,
                bar_ptr,
                size: aligned_size,
            },
        );

    // SAFETY: effective_bar_ptr points into a valid BAR1 mapping registered with SPDK.
    // size is the user-requested size (may be less than aligned_size).
    // free_fn handles full cleanup. numa_node = -1 (GPU memory).
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(
            effective_bar_ptr,
            size,
            spdk_unregister_gdr_unmap_and_close,
            -1,
        )
        .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        gdr_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(effective_bar_ptr as usize));
        unsafe {
            spdk_mem_unregister(bar_ptr, aligned_size);
            gdr_unmap(gdr, mh, bar_ptr, aligned_size);
            gdr_unpin_buffer(gdr, mh);
            gdr_close(gdr);
        }
    }

    result
}

/// Tracks physical DMA mappings (mmap'd VA → phys) for cleanup.
#[cfg(feature = "p2p")]
struct PhysMappingState {
    va: *mut std::ffi::c_void,
    phys_addr: u64,
    size: usize,
}

// SAFETY: The VA is process-global and not thread-bound.
#[cfg(feature = "p2p")]
unsafe impl Send for PhysMappingState {}
#[cfg(feature = "p2p")]
unsafe impl Sync for PhysMappingState {}

#[cfg(feature = "p2p")]
static PHYS_MAPPINGS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<usize, PhysMappingState>>,
> = std::sync::OnceLock::new();

#[cfg(feature = "p2p")]
fn phys_mappings() -> &'static std::sync::Mutex<std::collections::HashMap<usize, PhysMappingState>>
{
    PHYS_MAPPINGS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// # Safety
///
/// `ptr` must be a VA previously created by `create_spdk_dma_buffer_from_phys`.
#[cfg(feature = "p2p")]
pub unsafe extern "C" fn vfio_unmap_extmem_munmap(ptr: *mut std::ffi::c_void) {
    unsafe {
        let state = phys_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));

        if let Some(s) = state {
            rte_vfio_container_dma_unmap(-1, s.va as u64, s.phys_addr, s.size as u64);
            rte_extmem_unregister(s.va, s.size);
            libc::munmap(s.va, s.size);
        }
    }
}

/// Resolve a virtual address to its physical/IOVA address via SPDK's vtophys.
///
/// The buffer must already be registered with SPDK (e.g., via `spdk_mem_register`
/// or allocated via `spdk_dma_zmalloc`). Returns the physical address on success,
/// or `None` if translation fails (SPDK_VTOPHYS_ERROR = 0xFFFFFFFFFFFFFFFF).
///
/// # Safety
///
/// `ptr` must point to SPDK-registered memory of at least `size` bytes.
#[cfg(feature = "p2p")]
pub unsafe fn get_phys_addr(ptr: *const std::ffi::c_void, size: usize) -> Option<u64> {
    const SPDK_VTOPHYS_ERROR: u64 = 0xFFFF_FFFF_FFFF_FFFF;
    let mut sz = size as u64;
    let phys = unsafe { spdk_vtophys(ptr, &mut sz) };
    if phys == SPDK_VTOPHYS_ERROR {
        None
    } else {
        Some(phys)
    }
}

/// Create an SPDK `DmaBuffer` targeting a known physical address via DPDK IOMMU mapping.
///
/// This is used for cross-process GPU P2P DMA: the remote process (Python client)
/// allocates GPU memory, pins it with GDRCopy, and extracts the GPU BAR1 physical
/// address from pagemap. This function then:
/// 1. mmap's anonymous pages as a local VA placeholder
/// 2. Registers the VA with DPDK, associating it with the GPU BAR physical IOVA
/// 3. Programs the VFIO IOMMU to allow NVMe DMA to the physical address
///
/// When the NVMe controller performs DMA to this buffer, SPDK's vtophys resolves
/// the VA to the GPU BAR1 physical address, and the IOMMU permits the access.
///
/// On drop: VFIO DMA unmap → DPDK extmem unregister → munmap.
#[cfg(feature = "p2p")]
pub fn create_spdk_dma_buffer_from_phys(
    phys_addr: u64,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    // SAFETY: mmap anonymous private pages as a VA placeholder.
    let va = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if va == libc::MAP_FAILED {
        return Err("mmap anonymous pages failed".to_string());
    }

    // Tell DPDK that this VA range maps to the given physical/IOVA address.
    let iova_array: [u64; 1] = [phys_addr];
    let rc = unsafe { rte_extmem_register(va, size, iova_array.as_ptr(), 1, size) };
    if rc != 0 {
        unsafe { libc::munmap(va, size) };
        return Err(format!("rte_extmem_register failed (rc={})", rc));
    }

    // Program the VFIO IOMMU: allow NVMe DMA to the GPU BAR physical address.
    let rc = unsafe { rte_vfio_container_dma_map(-1, va as u64, phys_addr, size as u64) };
    if rc != 0 {
        unsafe {
            rte_extmem_unregister(va, size);
            libc::munmap(va, size);
        }
        return Err(format!("rte_vfio_container_dma_map failed (rc={})", rc));
    }

    // Store state for cleanup.
    phys_mappings()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            va as usize,
            PhysMappingState {
                va,
                phys_addr,
                size,
            },
        );

    // SAFETY: va is a valid mmap'd pointer registered with DPDK for the given IOVA.
    // NVMe DMA to this buffer will hit the GPU BAR1 physical address.
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(va, size, vfio_unmap_extmem_munmap, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        phys_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(va as usize));
        unsafe {
            rte_vfio_container_dma_unmap(-1, va as u64, phys_addr, size as u64);
            rte_extmem_unregister(va, size);
            libc::munmap(va, size);
        }
    }

    result
}

/// Create an SPDK `DmaBuffer` from an existing BAR VA using direct DPDK IOMMU programming.
///
/// Unlike `create_spdk_dma_buffer_from_gpu_bar` which uses `spdk_mem_register`, this
/// function uses the raw DPDK APIs (`rte_extmem_register` + `rte_vfio_container_dma_map`)
/// to program the IOMMU. This is the mechanism used in cross-process P2P DMA where
/// the storage server needs to program DMA access to GPU BAR pages.
///
/// The BAR VA must already be mapped in the calling process's page table (e.g., via
/// GDRCopy `gdr_map`). The IOVA used is the VA itself (identity mapping).
///
/// On drop: VFIO DMA unmap → DPDK extmem unregister. Does NOT munmap (caller owns the VA).
///
/// # Safety
///
/// `bar_ptr` must be a valid pointer to a GDRCopy BAR mapping of at least `size` bytes.
#[cfg(feature = "p2p")]
pub unsafe fn create_spdk_dma_buffer_from_bar_direct(
    bar_ptr: *mut std::ffi::c_void,
    size: usize,
) -> Result<interfaces::DmaBuffer, String> {
    let iova = bar_ptr as u64;

    // Tell DPDK that vtophys(bar_ptr) = iova (VA-mode identity).
    // Use system page size for alignment (GDRCopy BAR mappings are 4K-aligned).
    let page_sz: usize = 4096;
    let n_pages = size.div_ceil(page_sz);
    let iova_array: Vec<u64> = (0..n_pages).map(|i| iova + (i * page_sz) as u64).collect();
    let rc = unsafe {
        rte_extmem_register(
            bar_ptr,
            size,
            iova_array.as_ptr(),
            n_pages as std::os::raw::c_uint,
            page_sz,
        )
    };
    if rc != 0 {
        let errno = std::io::Error::last_os_error();
        return Err(format!(
            "rte_extmem_register failed (rc={}, errno={}, va={:?}, size={}, page_sz={}, n_pages={})",
            rc, errno, bar_ptr, size, page_sz, n_pages
        ));
    }

    // Program the VFIO IOMMU: allow NVMe DMA to the BAR pages at this VA.
    let rc = unsafe { rte_vfio_container_dma_map(-1, bar_ptr as u64, iova, size as u64) };
    if rc != 0 {
        unsafe { rte_extmem_unregister(bar_ptr, size) };
        return Err(format!("rte_vfio_container_dma_map failed (rc={})", rc));
    }

    // Store state for cleanup (no munmap — caller owns the BAR mapping).
    phys_mappings()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            bar_ptr as usize,
            PhysMappingState {
                va: bar_ptr,
                phys_addr: iova,
                size,
            },
        );

    // SAFETY: bar_ptr is a valid GDRCopy BAR mapping, now IOMMU-registered for DMA.
    // free_fn handles VFIO unmap + extmem unregister (no munmap).
    let result = unsafe {
        interfaces::DmaBuffer::from_raw(bar_ptr, size, vfio_unmap_extmem_only, -1)
            .map_err(|e| format!("DmaBuffer creation failed: {}", e))
    };

    if result.is_err() {
        phys_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(bar_ptr as usize));
        unsafe {
            rte_vfio_container_dma_unmap(-1, bar_ptr as u64, iova, size as u64);
            rte_extmem_unregister(bar_ptr, size);
        }
    }

    result
}

/// # Safety
///
/// `ptr` must be a BAR VA previously registered via `create_spdk_dma_buffer_from_bar_direct`.
#[cfg(feature = "p2p")]
pub unsafe extern "C" fn vfio_unmap_extmem_only(ptr: *mut std::ffi::c_void) {
    unsafe {
        let state = phys_mappings()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(ptr as usize));

        if let Some(s) = state {
            rte_vfio_container_dma_unmap(-1, s.va as u64, s.phys_addr, s.size as u64);
            rte_extmem_unregister(s.va, s.size);
        }
    }
}

/// Create a GpuDmaBuffer from a verified and pinned IPC handle.
///
/// The caller is responsible for ensuring the handle has been verified
/// and pinned (tracked externally by the component state).
#[cfg(feature = "gpu")]
pub fn create_gpu_dma_buffer(handle: GpuIpcHandle) -> Result<GpuDmaBuffer, String> {
    if handle.as_ptr().is_null() {
        return Err("Handle has null pointer".to_string());
    }

    // SAFETY: The handle has been verified (device memory) and pinned
    // (tracked by component state). The pointer is valid for handle.size()
    // bytes. cuda_ipc_close_mem_handle correctly frees via cudaIpcCloseMemHandle.
    let buf =
        unsafe { GpuDmaBuffer::new(handle.as_ptr(), handle.size(), cuda_ipc_close_mem_handle) };

    // GpuIpcHandle has no Drop impl so letting it go out of scope is safe.
    // GpuDmaBuffer now owns the pointer via its free_fn.

    Ok(buf)
}
