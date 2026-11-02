//! NUMA-local memory allocation via `mmap` + `mbind`.

use std::alloc::Layout;
use std::ptr::NonNull;

use super::NumaError;

/// A NUMA-local memory allocator that binds allocations to a specific NUMA node.
///
/// Uses `mmap(MAP_ANONYMOUS)` + `mbind(MPOL_BIND)` to allocate memory on the
/// target node. Falls back to default allocation if `mbind` fails.
///
/// # Examples
///
/// ```no_run
/// use component_core::numa::NumaAllocator;
/// use std::alloc::Layout;
///
/// let alloc = NumaAllocator::new(0);
/// let layout = Layout::from_size_align(4096, 8).unwrap();
/// let ptr = alloc.alloc(layout).unwrap();
/// // Use the memory...
/// unsafe { alloc.dealloc(ptr, layout) };
/// ```
#[derive(Debug, Clone, Copy)]
pub struct NumaAllocator {
    node_id: usize,
}

impl NumaAllocator {
    /// Create a new allocator targeting the given NUMA node.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::NumaAllocator;
    ///
    /// let alloc = NumaAllocator::new(0);
    /// assert_eq!(alloc.node_id(), 0);
    /// ```
    #[must_use]
    pub fn new(node_id: usize) -> Self {
        Self { node_id }
    }

    /// The target NUMA node ID.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::numa::NumaAllocator;
    ///
    /// let alloc = NumaAllocator::new(1);
    /// assert_eq!(alloc.node_id(), 1);
    /// ```
    #[must_use]
    pub fn node_id(&self) -> usize {
        self.node_id
    }

    /// Allocate memory on the target NUMA node.
    ///
    /// The returned pointer is page-aligned. If `mbind` fails, the allocation
    /// still succeeds with default memory policy (fallback per FR-019).
    ///
    /// # Errors
    ///
    /// Returns [`NumaError::AllocationFailed`] if `mmap` itself fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use component_core::numa::NumaAllocator;
    /// use std::alloc::Layout;
    ///
    /// let alloc = NumaAllocator::new(0);
    /// let layout = Layout::from_size_align(4096, 8).unwrap();
    /// let ptr = alloc.alloc(layout).unwrap();
    /// assert!(!ptr.as_ptr().is_null());
    /// unsafe { alloc.dealloc(ptr, layout) };
    /// ```
    pub fn alloc(&self, layout: Layout) -> Result<NonNull<u8>, NumaError> {
        let size = layout.size();
        if size == 0 {
            return Err(NumaError::AllocationFailed(
                "zero-size allocation".to_string(),
            ));
        }

        // Round up to page size for mmap.
        let page_size = page_size();
        let alloc_size = (size + page_size - 1) & !(page_size - 1);

        // SAFETY: mmap with MAP_ANONYMOUS | MAP_PRIVATE allocates fresh memory.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                alloc_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            return Err(NumaError::AllocationFailed(format!(
                "mmap failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        // Attempt to bind to the target NUMA node.
        // Build nodemask: one bit per node.
        let mut nodemask: libc::c_ulong = 0;
        if self.node_id < (std::mem::size_of::<libc::c_ulong>() * 8) {
            nodemask = 1 << self.node_id;
        }

        // SAFETY: ptr is a valid mmap'd region. mbind binds the pages to a node.
        // We ignore mbind failure — FR-019 says fallback to default policy.
        unsafe {
            libc::syscall(
                libc::SYS_mbind,
                ptr,
                alloc_size,
                libc::MPOL_BIND,
                std::ptr::addr_of!(nodemask),
                self.node_id + 2, // maxnode: must be > highest node bit + 1
                0u32,             // flags
            );
        }

        // Touch first page to fault it onto the target node.
        // SAFETY: ptr is valid, alloc_size >= page_size.
        unsafe {
            std::ptr::write_volatile(ptr.cast::<u8>(), 0);
        }

        // SAFETY: mmap succeeded, so ptr is non-null.
        Ok(unsafe { NonNull::new_unchecked(ptr.cast::<u8>()) })
    }

    /// Deallocate memory previously allocated by [`alloc`](Self::alloc).
    ///
    /// # Safety
    ///
    /// - `ptr` must have been returned by a prior call to `alloc` on this
    ///   allocator (or any `NumaAllocator` — the deallocation is size-based).
    /// - `layout` must match the layout used for the original allocation.
    /// - The pointer must not have been deallocated already.
    pub unsafe fn dealloc(&self, ptr: NonNull<u8>, layout: Layout) {
        let page_size = page_size();
        let alloc_size = (layout.size() + page_size - 1) & !(page_size - 1);
        // SAFETY: the caller guarantees `ptr` came from `alloc` with this
        // `layout` and was not freed yet, so `[ptr, ptr + alloc_size)` is
        // exactly the region that `alloc` mapped (same page rounding).
        unsafe {
            libc::munmap(ptr.as_ptr().cast::<libc::c_void>(), alloc_size);
        }
    }
}

/// The system page size.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sysconf(_SC_PAGESIZE) cannot fail on Linux and returns a small positive \
              power of two (4 KiB-64 KiB)"
)]
fn page_size() -> usize {
    // SAFETY: sysconf takes an integer name and reads no caller memory.
    (unsafe { libc::sysconf(libc::_SC_PAGESIZE) }) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_node_id() {
        let alloc = NumaAllocator::new(1);
        assert_eq!(alloc.node_id(), 1);
    }

    #[test]
    fn alloc_and_dealloc() {
        let alloc = NumaAllocator::new(0);
        let layout = Layout::from_size_align(4096, 8).unwrap();
        let ptr = alloc.alloc(layout).unwrap();
        // ptr is NonNull so it's guaranteed non-null; verify we can write to it.
        // SAFETY: `ptr` is a fresh, writable allocation of at least 4096 bytes.
        unsafe { std::ptr::write_volatile(ptr.as_ptr(), 0xAA) };
        // SAFETY: `ptr` came from `alloc` with this `layout` and is freed once.
        unsafe { alloc.dealloc(ptr, layout) };
    }

    #[test]
    fn alloc_large_region() {
        let alloc = NumaAllocator::new(0);
        let layout = Layout::from_size_align(1024 * 1024, 8).unwrap(); // 1 MB
        let ptr = alloc.alloc(layout).unwrap();
        // Write and read back to verify usability
        // SAFETY: `ptr` is a fresh, writable, zero-filled allocation of at
        // least 1 MiB, and nothing else references it.
        unsafe {
            let slice = std::slice::from_raw_parts_mut(ptr.as_ptr(), 1024 * 1024);
            slice[0] = 42;
            slice[1024 * 1024 - 1] = 99;
            assert_eq!(slice[0], 42);
            assert_eq!(slice[1024 * 1024 - 1], 99);
        }
        // SAFETY: `ptr` came from `alloc` with this `layout` and is freed once;
        // the slice above is out of scope.
        unsafe { alloc.dealloc(ptr, layout) };
    }

    #[test]
    fn alloc_zero_size_fails() {
        let alloc = NumaAllocator::new(0);
        let layout = Layout::from_size_align(0, 1).unwrap();
        assert!(alloc.alloc(layout).is_err());
    }
}
