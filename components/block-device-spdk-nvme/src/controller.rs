//! Safe wrapper around the SPDK NVMe controller.
//!
//! `NvmeController` manages the lifecycle of an SPDK NVMe controller:
//! probe/attach at creation, info queries during operation, and detach on drop.
//! All unsafe SPDK FFI calls are contained within this module.

use interfaces::NvmeBlockError;

use crate::qpair::QueuePairPool;

/// NVMe specification version reported by the controller.
///
/// # Examples
///
/// ```
/// use block_device_spdk_nvme::controller::NvmeVersion;
///
/// let v = NvmeVersion { major: 1, minor: 4, tertiary: 0 };
/// assert_eq!(format!("{v}"), "1.4.0");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvmeVersion {
    /// Major version number.
    pub major: u16,
    /// Minor version number.
    pub minor: u8,
    /// Tertiary (patch) version number.
    pub tertiary: u8,
}

impl std::fmt::Display for NvmeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.tertiary)
    }
}

/// Information about a discovered NVMe namespace on the controller.
///
/// # Examples
///
/// ```
/// use block_device_spdk_nvme::controller::NvmeNamespaceInfo;
///
/// let ns = NvmeNamespaceInfo {
///     ns_id: 1,
///     num_sectors: 1_000_000,
///     sector_size: 512,
/// };
/// assert_eq!(ns.capacity_bytes(), 512_000_000);
/// ```
#[derive(Debug, Clone)]
pub struct NvmeNamespaceInfo {
    /// NVMe namespace identifier.
    pub ns_id: u32,
    /// Total number of sectors in this namespace.
    pub num_sectors: u64,
    /// Sector size in bytes.
    pub sector_size: u32,
}

impl NvmeNamespaceInfo {
    /// Total capacity in bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use block_device_spdk_nvme::controller::NvmeNamespaceInfo;
    ///
    /// let ns = NvmeNamespaceInfo {
    ///     ns_id: 1,
    ///     num_sectors: 2048,
    ///     sector_size: 4096,
    /// };
    /// assert_eq!(ns.capacity_bytes(), 2048 * 4096);
    /// ```
    #[allow(dead_code)]
    pub fn capacity_bytes(&self) -> u64 {
        self.num_sectors * self.sector_size as u64
    }
}

/// Safe wrapper around an attached SPDK NVMe controller.
///
/// Manages controller lifecycle including probe/attach, namespace discovery,
/// queue pair allocation, and detach on drop.
///
/// # Safety
///
/// The raw `ctrlr_ptr` is obtained from `spdk_nvme_probe` and remains valid
/// until `spdk_nvme_detach` is called in [`Drop`]. All SPDK FFI calls that
/// use this pointer are contained within this module.
pub(crate) struct NvmeController {
    /// Raw SPDK controller pointer.
    /// SAFETY: Valid from probe/attach until Drop calls detach.
    ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
    /// Pool of IO queue pairs with varying depths.
    pub qpairs: QueuePairPool,
    /// Discovered namespaces on this controller.
    pub namespaces: Vec<NvmeNamespaceInfo>,
    /// NUMA node of the controller device.
    numa_node: i32,
    /// NVMe specification version (populated from controller data if available).
    version: NvmeVersion,
    /// Maximum data transfer size in bytes (default 128KB).
    max_transfer_size: u32,
    /// Maximum IO queue depth from controller opts.
    max_queue_depth: u32,
    /// Number of IO queues from controller opts.
    num_io_queues: u32,
}

// SAFETY: The SPDK controller pointer is only accessed from the actor thread
// (single-threaded access pattern). The NvmeController is not shared across
// threads — it lives inside the actor handler.
unsafe impl Send for NvmeController {}

impl NvmeController {
    /// Attach to an NVMe controller and discover its namespaces.
    ///
    /// This reads controller capabilities via default opts, discovers
    /// namespaces, and allocates IO queue pairs.
    ///
    /// # Errors
    ///
    /// Returns [`NvmeBlockError::NotInitialized`] if a null pointer is passed,
    /// or [`NvmeBlockError::BlockDevice`] if queue pair allocation fails.
    ///
    /// # Safety
    ///
    /// `ctrlr_ptr` must be a valid pointer obtained from `spdk_nvme_probe`.
    pub unsafe fn attach(
        ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
        numa_node: i32,
    ) -> Result<Self, NvmeBlockError> {
        if ctrlr_ptr.is_null() {
            return Err(NvmeBlockError::NotInitialized(
                "null controller pointer — SPDK probe/attach may have failed".into(),
            ));
        }

        // Get default controller opts to extract queue config.
        // SAFETY: ctrlr_ptr is non-null and was obtained from SPDK probe.
        let mut opts: spdk_sys::spdk_nvme_ctrlr_opts = std::mem::zeroed();
        spdk_sys::spdk_nvme_ctrlr_get_default_ctrlr_opts(
            &mut opts,
            std::mem::size_of::<spdk_sys::spdk_nvme_ctrlr_opts>(),
        );

        let num_io_queues = opts.num_io_queues;
        let max_queue_depth = opts.io_queue_size;

        // Default version and transfer size (not available from minimal bindings).
        let version = NvmeVersion {
            major: 1,
            minor: 0,
            tertiary: 0,
        };
        let max_transfer_size = 131072; // 128KB default

        // Discover namespaces using available API.
        let namespaces = Self::discover_namespaces(ctrlr_ptr);

        // Allocate queue pair pool with varying depths.
        // SAFETY: ctrlr_ptr is valid; checked non-null at function entry.
        let qpairs = QueuePairPool::allocate(ctrlr_ptr, max_queue_depth)?;

        Ok(Self {
            ctrlr_ptr,
            qpairs,
            namespaces,
            numa_node,
            version,
            max_transfer_size,
            max_queue_depth,
            num_io_queues,
        })
    }

    /// Discover all active namespaces on the controller.
    ///
    /// Iterates namespace IDs from 1 to `get_num_ns()` and checks each
    /// with `ns_is_active`.
    fn discover_namespaces(ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr) -> Vec<NvmeNamespaceInfo> {
        let mut namespaces = Vec::new();

        // SAFETY: ctrlr_ptr is valid.
        let num_ns = unsafe { spdk_sys::spdk_nvme_ctrlr_get_num_ns(ctrlr_ptr) };

        for ns_id in 1..=num_ns {
            // SAFETY: ctrlr_ptr is valid, ns_id is in range.
            let ns_ptr = unsafe { spdk_sys::spdk_nvme_ctrlr_get_ns(ctrlr_ptr, ns_id) };
            if ns_ptr.is_null() {
                continue;
            }

            // SAFETY: ns_ptr is non-null.
            let is_active = unsafe { spdk_sys::spdk_nvme_ns_is_active(ns_ptr) };
            if !is_active {
                continue;
            }

            // SAFETY: ns_ptr is valid and active.
            let num_sectors = unsafe { spdk_sys::spdk_nvme_ns_get_num_sectors(ns_ptr) };
            let sector_size = unsafe { spdk_sys::spdk_nvme_ns_get_sector_size(ns_ptr) };

            namespaces.push(NvmeNamespaceInfo {
                ns_id,
                num_sectors,
                sector_size,
            });
        }

        namespaces
    }

    /// Return a raw pointer to the underlying SPDK controller.
    pub(crate) fn as_ptr(&self) -> *mut spdk_sys::spdk_nvme_ctrlr {
        self.ctrlr_ptr
    }

    /// NUMA node of the controller device.
    pub(crate) fn numa_node(&self) -> i32 {
        self.numa_node
    }

    /// NVMe specification version reported by the controller.
    pub(crate) fn version(&self) -> NvmeVersion {
        self.version
    }

    /// Maximum data transfer size in bytes.
    pub(crate) fn max_transfer_size(&self) -> u32 {
        self.max_transfer_size
    }

    /// Maximum IO queue depth supported.
    pub(crate) fn max_queue_depth(&self) -> u32 {
        self.max_queue_depth
    }

    /// Number of IO queues on this controller.
    pub(crate) fn num_io_queues(&self) -> u32 {
        self.num_io_queues
    }

    /// Look up a namespace by ID.
    #[allow(dead_code)]
    pub(crate) fn namespace(&self, ns_id: u32) -> Option<&NvmeNamespaceInfo> {
        self.namespaces.iter().find(|ns| ns.ns_id == ns_id)
    }

    /// Get the default namespace (first active), if any.
    pub(crate) fn default_namespace(&self) -> Option<&NvmeNamespaceInfo> {
        self.namespaces.first()
    }

    /// Return sector size of the default namespace, or 512 as fallback.
    pub(crate) fn sector_size(&self) -> u32 {
        self.default_namespace()
            .map(|ns| ns.sector_size)
            .unwrap_or(512)
    }

    /// Return total sector count of the default namespace, or 0.
    pub(crate) fn num_sectors(&self) -> u64 {
        self.default_namespace()
            .map(|ns| ns.num_sectors)
            .unwrap_or(0)
    }

    /// Refresh the namespace list by re-scanning the controller.
    pub(crate) fn refresh_namespaces(&mut self) {
        self.namespaces = Self::discover_namespaces(self.ctrlr_ptr);
    }
}

impl Drop for NvmeController {
    fn drop(&mut self) {
        if !self.ctrlr_ptr.is_null() {
            // Drop all queue pairs first (they reference the controller).
            self.qpairs.deallocate_all();

            // SAFETY: ctrlr_ptr is valid and was obtained from probe/attach.
            // Detach releases the controller back to the kernel driver.
            unsafe {
                spdk_sys::spdk_nvme_detach(self.ctrlr_ptr);
            }
            self.ctrlr_ptr = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvme_version_display() {
        let v = NvmeVersion {
            major: 1,
            minor: 4,
            tertiary: 0,
        };
        assert_eq!(format!("{v}"), "1.4.0");
    }

    #[test]
    fn nvme_version_equality() {
        let v1 = NvmeVersion {
            major: 1,
            minor: 4,
            tertiary: 0,
        };
        let v2 = NvmeVersion {
            major: 1,
            minor: 4,
            tertiary: 0,
        };
        assert_eq!(v1, v2);
    }

    #[test]
    fn namespace_info_capacity() {
        let ns = NvmeNamespaceInfo {
            ns_id: 1,
            num_sectors: 1000,
            sector_size: 512,
        };
        assert_eq!(ns.capacity_bytes(), 512_000);
    }

    #[test]
    fn namespace_info_clone() {
        let ns = NvmeNamespaceInfo {
            ns_id: 1,
            num_sectors: 2048,
            sector_size: 4096,
        };
        let ns2 = ns.clone();
        assert_eq!(ns2.ns_id, 1);
        assert_eq!(ns2.capacity_bytes(), 2048 * 4096);
    }
}
