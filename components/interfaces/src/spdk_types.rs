//! Data types used by SPDK-related interface traits.
//!
//! These types are gated behind the `spdk` Cargo feature.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::{Deref, DerefMut};

// ---------------------------------------------------------------------------
// SpdkEnvError
// ---------------------------------------------------------------------------

/// Error conditions reported by the SPDK environment component.
///
/// Each variant carries a descriptive message with actionable guidance
/// to help the user resolve the issue.
#[derive(Debug, Clone)]
pub enum SpdkEnvError {
    /// VFIO is not available: `/dev/vfio` not found or `vfio-pci` module not loaded.
    VfioNotAvailable(String),
    /// Insufficient permissions on a specific VFIO path.
    PermissionDenied(String),
    /// No hugepages configured for DPDK.
    HugepagesNotConfigured(String),
    /// Logging receptacle not connected before `init()`.
    LoggerNotConnected(String),
    /// Another SPDK environment instance is already active in this process.
    AlreadyInitialized(String),
    /// SPDK/DPDK environment initialization failed.
    InitFailed(String),
    /// PCI device enumeration failed after environment was initialized.
    DeviceProbeFailed(String),
    /// DMA buffer allocation failed (hugepage memory exhausted or env not initialized).
    DmaAllocationFailed(String),
}

impl fmt::Display for SpdkEnvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpdkEnvError::VfioNotAvailable(msg) => write!(f, "VFIO not available: {msg}"),
            SpdkEnvError::PermissionDenied(msg) => write!(f, "Permission denied: {msg}"),
            SpdkEnvError::HugepagesNotConfigured(msg) => {
                write!(f, "Hugepages not configured: {msg}")
            }
            SpdkEnvError::LoggerNotConnected(msg) => write!(f, "Logger not connected: {msg}"),
            SpdkEnvError::AlreadyInitialized(msg) => write!(f, "Already initialized: {msg}"),
            SpdkEnvError::InitFailed(msg) => write!(f, "SPDK init failed: {msg}"),
            SpdkEnvError::DeviceProbeFailed(msg) => write!(f, "Device probe failed: {msg}"),
            SpdkEnvError::DmaAllocationFailed(msg) => {
                write!(f, "DMA allocation failed: {msg}")
            }
        }
    }
}

impl std::error::Error for SpdkEnvError {}

// ---------------------------------------------------------------------------
// BlockDeviceError
// ---------------------------------------------------------------------------

/// Error conditions reported by the simple block device component.
///
/// Each variant carries a descriptive message with actionable guidance.
#[derive(Debug, Clone)]
pub enum BlockDeviceError {
    /// The block device has not been opened yet.
    NotOpen(String),
    /// The block device is already open.
    AlreadyOpen(String),
    /// NVMe probe/attach failed — no controller found.
    ProbeFailure(String),
    /// No active NVMe namespace found on the controller.
    NamespaceNotFound(String),
    /// Failed to allocate an I/O queue pair.
    QpairAllocationFailed(String),
    /// A read I/O operation failed.
    ReadFailed(String),
    /// A write I/O operation failed.
    WriteFailed(String),
    /// The supplied buffer size does not match the required sector-aligned size.
    BufferSizeMismatch(String),
    /// Failed to allocate DMA-safe memory for I/O buffers.
    DmaAllocationFailed(String),
    /// The SPDK environment receptacle is not connected or not initialized.
    EnvNotInitialized(String),
    /// The logger receptacle is not connected.
    LoggerNotConnected(String),
}

impl fmt::Display for BlockDeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockDeviceError::NotOpen(msg) => write!(f, "Block device not open: {msg}"),
            BlockDeviceError::AlreadyOpen(msg) => write!(f, "Block device already open: {msg}"),
            BlockDeviceError::ProbeFailure(msg) => write!(f, "NVMe probe failed: {msg}"),
            BlockDeviceError::NamespaceNotFound(msg) => {
                write!(f, "NVMe namespace not found: {msg}")
            }
            BlockDeviceError::QpairAllocationFailed(msg) => {
                write!(f, "I/O queue pair allocation failed: {msg}")
            }
            BlockDeviceError::ReadFailed(msg) => write!(f, "Read failed: {msg}"),
            BlockDeviceError::WriteFailed(msg) => write!(f, "Write failed: {msg}"),
            BlockDeviceError::BufferSizeMismatch(msg) => {
                write!(f, "Buffer size mismatch: {msg}")
            }
            BlockDeviceError::DmaAllocationFailed(msg) => {
                write!(f, "DMA allocation failed: {msg}")
            }
            BlockDeviceError::EnvNotInitialized(msg) => {
                write!(f, "SPDK environment not initialized: {msg}")
            }
            BlockDeviceError::LoggerNotConnected(msg) => {
                write!(f, "Logger not connected: {msg}")
            }
        }
    }
}

impl std::error::Error for BlockDeviceError {}

// ---------------------------------------------------------------------------
// PCI types
// ---------------------------------------------------------------------------

/// PCI Bus-Device-Function address identifying a specific PCI device.
///
/// Displayed in standard notation: `DDDD:BB:DD.F` (e.g., `0000:01:00.0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PciAddress {
    /// PCI domain (segment).
    pub domain: u32,
    /// PCI bus number.
    pub bus: u8,
    /// PCI device number.
    pub dev: u8,
    /// PCI function number.
    pub func: u8,
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04x}:{:02x}:{:02x}.{:x}",
            self.domain, self.bus, self.dev, self.func
        )
    }
}

/// PCI vendor/device/class identification for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciId {
    /// PCI class code.
    pub class_id: u32,
    /// PCI vendor ID.
    pub vendor_id: u16,
    /// PCI device ID.
    pub device_id: u16,
    /// Subsystem vendor ID.
    pub subvendor_id: u16,
    /// Subsystem device ID.
    pub subdevice_id: u16,
}

/// A VFIO-attached device discovered by SPDK during initialization.
///
/// Instances are immutable snapshots created during initialization and
/// do not track runtime state changes.
#[derive(Debug, Clone)]
pub struct VfioDevice {
    /// PCI BDF address uniquely identifying this device.
    pub address: PciAddress,
    /// Vendor/device/class identification.
    pub id: PciId,
    /// NUMA node the device is attached to (-1 = unknown).
    pub numa_node: i32,
    /// SPDK device type string (e.g., "nvme", "virtio").
    pub device_type: String,
}

// ---------------------------------------------------------------------------
// DmaBuffer
// ---------------------------------------------------------------------------

/// A DMA-safe buffer suitable for direct NVMe I/O — no intermediate copies
/// needed.
///
/// The buffer can be backed by different allocators:
/// - **SPDK hugepage memory** — via [`DmaBuffer::new`], using
///   `spdk_dma_zmalloc` / `spdk_dma_free`.
/// - **External memory** (e.g. CUDA device memory) — via
///   [`DmaBuffer::from_raw`], with a caller-supplied deallocation function.
///
/// On [`Drop`] the stored deallocator is called automatically.
pub struct DmaBuffer {
    ptr: *mut std::ffi::c_void,
    len: usize,
    /// Function used to free `ptr` when this buffer is dropped.
    free_fn: unsafe extern "C" fn(*mut std::ffi::c_void),
    /// NUMA node the memory was allocated from, or -1 if unknown.
    numa_node: i32,
    /// Optional key-value metadata (e.g. `"gpu_device" => "0"`).
    metadata: BTreeMap<String, String>,
}

// SAFETY: The underlying hugepage memory has no thread affinity.
// It is valid from any thread once allocated.
unsafe impl Send for DmaBuffer {}

impl DmaBuffer {
    /// Allocate a zero-initialized DMA buffer.
    ///
    /// `size` is the buffer length in bytes (must be > 0).
    /// `align` is the required alignment (typically the device sector size).
    /// `numa_node` optionally pins the allocation to a specific NUMA node.
    /// When `None`, SPDK chooses any available node.
    ///
    /// The SPDK environment must be initialized before calling this.
    pub fn new(
        size: usize,
        align: usize,
        numa_node: Option<i32>,
    ) -> Result<Self, SpdkEnvError> {
        if size == 0 {
            return Err(SpdkEnvError::DmaAllocationFailed(
                "DmaBuffer size must be > 0".into(),
            ));
        }

        let (ptr, free_fn, node): (
            *mut std::ffi::c_void,
            unsafe extern "C" fn(*mut std::ffi::c_void),
            i32,
        ) = match numa_node {
            Some(id) => {
                // SAFETY: spdk_zmalloc returns hugepage-backed memory or NULL.
                const SPDK_MALLOC_DMA: u32 = 0x01;
                let p = unsafe {
                    spdk_sys::spdk_zmalloc(
                        size,
                        align,
                        std::ptr::null_mut(),
                        id,
                        SPDK_MALLOC_DMA,
                    )
                };
                (p, spdk_sys::spdk_free as _, id)
            }
            None => {
                // SAFETY: spdk_dma_zmalloc returns hugepage-backed memory or NULL.
                let p = unsafe {
                    spdk_sys::spdk_dma_zmalloc(size, align, std::ptr::null_mut())
                };
                (p, spdk_sys::spdk_dma_free as _, -1)
            }
        };

        if ptr.is_null() {
            return Err(SpdkEnvError::DmaAllocationFailed(format!(
                "SPDK DMA allocation({size}, {align}) returned NULL"
            )));
        }

        Ok(Self {
            ptr,
            len: size,
            free_fn,
            numa_node: node,
            metadata: BTreeMap::new(),
        })
    }

    /// Wrap a pre-allocated buffer with a custom deallocator.
    ///
    /// This allows `DmaBuffer` to manage memory obtained from any allocator
    /// (e.g. `cudaMalloc`, `mmap`, or a custom pool) as long as the caller
    /// supplies the matching deallocation function.
    ///
    /// # Safety
    ///
    /// * `ptr` must be valid for reads and writes of `len` bytes.
    /// * `ptr` must remain valid until `free_fn` is called.
    /// * `free_fn` must correctly release the memory pointed to by `ptr`.
    /// * The caller must not free `ptr` themselves — `DmaBuffer` takes
    ///   ownership and will call `free_fn` on drop.
    pub unsafe fn from_raw(
        ptr: *mut std::ffi::c_void,
        len: usize,
        free_fn: unsafe extern "C" fn(*mut std::ffi::c_void),
        numa_node: i32,
    ) -> Result<Self, SpdkEnvError> {
        if ptr.is_null() {
            return Err(SpdkEnvError::DmaAllocationFailed(
                "DmaBuffer::from_raw called with null pointer".into(),
            ));
        }
        if len == 0 {
            return Err(SpdkEnvError::DmaAllocationFailed(
                "DmaBuffer size must be > 0".into(),
            ));
        }
        Ok(Self {
            ptr,
            len,
            free_fn,
            numa_node,
            metadata: BTreeMap::new(),
        })
    }

    /// Return the buffer length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Return true if the buffer has zero length (never true for a valid buffer).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Return the raw pointer for passing to SPDK NVMe commands.
    #[inline]
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.ptr
    }

    /// View the buffer as a byte slice.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: ptr is valid for len bytes (allocated by spdk_dma_zmalloc).
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }

    /// View the buffer as a mutable byte slice.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: ptr is valid for len bytes, and we have exclusive access (&mut self).
        unsafe { std::slice::from_raw_parts_mut(self.ptr as *mut u8, self.len) }
    }

    /// Return the NUMA node this buffer was allocated from, or -1 if unknown.
    #[inline]
    pub fn numa_node(&self) -> i32 {
        self.numa_node
    }

    /// Set the NUMA node for this buffer.
    #[inline]
    pub fn set_numa_node(&mut self, node: i32) {
        self.numa_node = node;
    }

    /// Return a reference to the key-value metadata map.
    #[inline]
    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }

    /// Return a mutable reference to the key-value metadata map.
    #[inline]
    pub fn metadata_mut(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.metadata
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        // SAFETY: ptr was allocated by the corresponding allocator and has not
        // been freed. free_fn is the matching deallocator supplied at
        // construction time.
        unsafe {
            (self.free_fn)(self.ptr);
        }
    }
}

impl Deref for DmaBuffer {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl DerefMut for DmaBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        self.as_mut_slice()
    }
}

impl fmt::Debug for DmaBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("DmaBuffer");
        s.field("len", &self.len)
            .field("ptr", &self.ptr)
            .field("numa_node", &self.numa_node);
        if !self.metadata.is_empty() {
            s.field("metadata", &self.metadata);
        }
        s.finish()
    }
}
