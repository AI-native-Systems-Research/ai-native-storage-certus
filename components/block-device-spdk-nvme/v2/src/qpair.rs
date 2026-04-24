//! NVMe IO queue pair pool with depth-based selection.
//!
//! The [`QueuePairPool`] maintains multiple SPDK NVMe IO queue pairs at
//! different depths. The [`select_qpair`](QueuePairPool::select_qpair)
//! method chooses the best queue pair for a given batch size: shallow
//! queues for small batches (lower latency), deep queues for large
//! batches (higher throughput).

use interfaces::NvmeBlockError;

/// An SPDK NVMe IO queue pair.
///
/// Each queue pair has a configured depth and tracks the number of
/// currently in-flight operations.
///
/// # Examples
///
/// ```
/// use block_device_spdk_nvme_v2::qpair::QueuePair;
///
/// let qp = QueuePair::new_detached(32);
/// assert_eq!(qp.depth(), 32);
/// assert_eq!(qp.in_flight(), 0);
/// assert_eq!(qp.available(), 32);
/// ```
pub struct QueuePair {
    /// Raw SPDK queue pair pointer (null for detached/test instances).
    qpair_ptr: *mut spdk_sys::spdk_nvme_qpair,
    /// Configured queue depth.
    depth: u32,
    /// Number of currently in-flight operations.
    in_flight: u32,
}

// SAFETY: QueuePair is only accessed from the actor thread.
unsafe impl Send for QueuePair {}

impl QueuePair {
    /// Create a detached queue pair for testing (no SPDK pointer).
    ///
    /// # Examples
    ///
    /// ```
    /// use block_device_spdk_nvme_v2::qpair::QueuePair;
    ///
    /// let qp = QueuePair::new_detached(64);
    /// assert_eq!(qp.depth(), 64);
    /// ```
    pub fn new_detached(depth: u32) -> Self {
        Self {
            qpair_ptr: std::ptr::null_mut(),
            depth,
            in_flight: 0,
        }
    }

    #[allow(dead_code)]
    pub fn depth(&self) -> u32 {
        self.depth
    }

    #[allow(dead_code)]
    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    /// Number of available slots.
    pub fn available(&self) -> u32 {
        self.depth.saturating_sub(self.in_flight)
    }

    /// Return the raw SPDK queue pair pointer.
    ///
    /// # Safety
    ///
    /// The pointer is valid only while the parent controller is alive.
    pub fn as_ptr(&self) -> *mut spdk_sys::spdk_nvme_qpair {
        self.qpair_ptr
    }

    /// Record that an operation was submitted.
    pub fn submit(&mut self) {
        self.in_flight += 1;
    }

    #[allow(dead_code)]
    pub fn complete(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// Process completions for this queue pair.
    ///
    /// Returns the number of completions processed.
    ///
    /// # Safety
    ///
    /// The queue pair pointer must be valid. This calls SPDK FFI.
    pub unsafe fn process_completions(&mut self, max_completions: u32) -> i32 {
        if self.qpair_ptr.is_null() {
            return 0;
        }
        // SAFETY: Caller guarantees qpair_ptr is valid.
        let n = unsafe {
            spdk_sys::spdk_nvme_qpair_process_completions(self.qpair_ptr, max_completions)
        };
        if n > 0 {
            self.in_flight = self.in_flight.saturating_sub(n as u32);
        }
        n
    }
}

/// A pool of IO queue pairs with varying depths.
///
/// Provides a selection heuristic that chooses the optimal queue pair
/// based on the batch size of pending operations.
///
/// # Examples
///
/// ```
/// use block_device_spdk_nvme_v2::qpair::QueuePairPool;
///
/// let pool = QueuePairPool::from_detached(&[4, 16, 64]);
/// assert_eq!(pool.len(), 3);
///
/// let idx = pool.select_index(1);
/// assert_eq!(pool.get(idx).unwrap().depth(), 4); // small batch -> shallow queue
///
/// let idx = pool.select_index(32);
/// assert_eq!(pool.get(idx).unwrap().depth(), 64); // large batch -> deep queue
/// ```
pub struct QueuePairPool {
    qpairs: Vec<QueuePair>,
}

impl QueuePairPool {
    /// Standard depths used for queue pair allocation.
    ///
    /// Shallow queues (4, 16) for low-latency small IO; deep queues
    /// (64, 256) for high-throughput batches.
    const STANDARD_DEPTHS: &'static [u32] = &[4, 16, 64, 256];

    /// Allocate queue pairs from an SPDK NVMe controller.
    ///
    /// Creates one queue pair at each standard depth that does not exceed
    /// the controller's maximum queue depth.
    ///
    /// # Errors
    ///
    /// Returns [`NvmeBlockError::BlockDevice`] if any queue pair allocation fails.
    ///
    /// # Safety
    ///
    /// `ctrlr_ptr` must be a valid SPDK NVMe controller pointer.
    pub unsafe fn allocate(
        ctrlr_ptr: *mut spdk_sys::spdk_nvme_ctrlr,
        max_depth: u32,
    ) -> Result<Self, NvmeBlockError> {
        let mut qpairs = Vec::new();

        for &depth in Self::STANDARD_DEPTHS {
            if depth > max_depth {
                continue;
            }

            let mut opts: spdk_sys::spdk_nvme_io_qpair_opts = unsafe { std::mem::zeroed() };
            opts.io_queue_size = depth;
            // io_queue_requests must be >= io_queue_size; SPDK uses this for its
            // internal request tracker pool. Setting it to 2x depth allows for
            // request splitting (large IO → multiple NVMe commands).
            opts.io_queue_requests = depth * 2;
            opts.opts_size = std::mem::size_of::<spdk_sys::spdk_nvme_io_qpair_opts>();

            // SAFETY: ctrlr_ptr is valid, opts is properly initialized.
            let qpair_ptr = unsafe {
                spdk_sys::spdk_nvme_ctrlr_alloc_io_qpair(
                    ctrlr_ptr,
                    &opts as *const _ as *const _,
                    std::mem::size_of::<spdk_sys::spdk_nvme_io_qpair_opts>(),
                )
            };

            if qpair_ptr.is_null() {
                return Err(NvmeBlockError::BlockDevice(
                    interfaces::BlockDeviceError::QpairAllocationFailed(format!(
                        "spdk_nvme_ctrlr_alloc_io_qpair(depth={depth}) returned NULL"
                    )),
                ));
            }

            qpairs.push(QueuePair {
                qpair_ptr,
                depth,
                in_flight: 0,
            });
        }

        if qpairs.is_empty() {
            return Err(NvmeBlockError::BlockDevice(
                interfaces::BlockDeviceError::QpairAllocationFailed(
                    "no queue pairs could be allocated — max_depth too small".into(),
                ),
            ));
        }

        Ok(Self { qpairs })
    }

    pub fn from_detached(depths: &[u32]) -> Self {
        let qpairs = depths.iter().map(|&d| QueuePair::new_detached(d)).collect();
        Self { qpairs }
    }

    /// Number of queue pairs in the pool.
    pub fn len(&self) -> usize {
        self.qpairs.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.qpairs.is_empty()
    }

    #[allow(dead_code)]
    pub fn get(&self, index: usize) -> Option<&QueuePair> {
        self.qpairs.get(index)
    }

    /// Get a mutable queue pair by index.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut QueuePair> {
        self.qpairs.get_mut(index)
    }

    /// Select the best queue pair index for a given batch size.
    ///
    /// Heuristic: choose the shallowest queue that has enough capacity
    /// for the batch. Falls back to the deepest queue if none has enough
    /// room.
    ///
    /// # Examples
    ///
    /// ```
    /// use block_device_spdk_nvme_v2::qpair::QueuePairPool;
    ///
    /// let pool = QueuePairPool::from_detached(&[4, 16, 64]);
    ///
    /// // Single op -> shallowest queue
    /// assert_eq!(pool.select_index(1), 0);
    ///
    /// // Batch of 10 -> depth-16 queue
    /// assert_eq!(pool.select_index(10), 1);
    ///
    /// // Batch of 50 -> depth-64 queue
    /// assert_eq!(pool.select_index(50), 2);
    /// ```
    pub fn select_index(&self, batch_size: usize) -> usize {
        // Find the shallowest queue with enough available capacity.
        for (i, qp) in self.qpairs.iter().enumerate() {
            if qp.available() as usize >= batch_size {
                return i;
            }
        }
        // Fall back to deepest (last) queue pair.
        self.qpairs.len().saturating_sub(1)
    }

    /// Select the best queue pair for a given batch size.
    ///
    /// Returns a mutable reference to the selected queue pair.
    pub fn select_qpair(&mut self, batch_size: usize) -> &mut QueuePair {
        let idx = self.select_index(batch_size);
        &mut self.qpairs[idx]
    }

    /// Deallocate all queue pairs (called before controller detach).
    pub fn deallocate_all(&mut self) {
        for qp in &mut self.qpairs {
            if !qp.qpair_ptr.is_null() {
                // SAFETY: qpair_ptr was allocated by spdk_nvme_ctrlr_alloc_io_qpair.
                unsafe {
                    spdk_sys::spdk_nvme_ctrlr_free_io_qpair(qp.qpair_ptr);
                }
                qp.qpair_ptr = std::ptr::null_mut();
            }
        }
        self.qpairs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_pair_detached_initial_state() {
        let qp = QueuePair::new_detached(32);
        assert_eq!(qp.depth(), 32);
        assert_eq!(qp.in_flight(), 0);
        assert_eq!(qp.available(), 32);
        assert!(qp.as_ptr().is_null());
    }

    #[test]
    fn queue_pair_submit_and_complete() {
        let mut qp = QueuePair::new_detached(4);
        qp.submit();
        assert_eq!(qp.in_flight(), 1);
        assert_eq!(qp.available(), 3);

        qp.submit();
        assert_eq!(qp.in_flight(), 2);

        qp.complete();
        assert_eq!(qp.in_flight(), 1);
        assert_eq!(qp.available(), 3);
    }

    #[test]
    fn queue_pair_complete_saturates_at_zero() {
        let mut qp = QueuePair::new_detached(4);
        qp.complete();
        assert_eq!(qp.in_flight(), 0);
    }

    #[test]
    fn pool_from_detached() {
        let pool = QueuePairPool::from_detached(&[4, 16, 64]);
        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());
        assert_eq!(pool.get(0).unwrap().depth(), 4);
        assert_eq!(pool.get(1).unwrap().depth(), 16);
        assert_eq!(pool.get(2).unwrap().depth(), 64);
    }

    #[test]
    fn pool_select_shallow_for_small_batch() {
        let pool = QueuePairPool::from_detached(&[4, 16, 64]);
        let idx = pool.select_index(1);
        assert_eq!(idx, 0);
        assert_eq!(pool.get(idx).unwrap().depth(), 4);
    }

    #[test]
    fn pool_select_medium_for_medium_batch() {
        let pool = QueuePairPool::from_detached(&[4, 16, 64]);
        let idx = pool.select_index(10);
        assert_eq!(idx, 1);
        assert_eq!(pool.get(idx).unwrap().depth(), 16);
    }

    #[test]
    fn pool_select_deep_for_large_batch() {
        let pool = QueuePairPool::from_detached(&[4, 16, 64]);
        let idx = pool.select_index(50);
        assert_eq!(idx, 2);
        assert_eq!(pool.get(idx).unwrap().depth(), 64);
    }

    #[test]
    fn pool_select_falls_back_to_deepest() {
        let pool = QueuePairPool::from_detached(&[4, 16, 64]);
        let idx = pool.select_index(100);
        assert_eq!(idx, 2);
    }

    #[test]
    fn pool_select_with_in_flight_pressure() {
        let mut pool = QueuePairPool::from_detached(&[4, 16, 64]);
        // Fill up the shallow queue.
        for _ in 0..4 {
            pool.get_mut(0).unwrap().submit();
        }
        // Now batch of 1 should skip the full shallow queue.
        let idx = pool.select_index(1);
        assert_eq!(idx, 1); // Falls to depth-16 queue
    }

    #[test]
    fn pool_select_qpair_returns_mutable_ref() {
        let mut pool = QueuePairPool::from_detached(&[4, 16, 64]);
        let qp = pool.select_qpair(1);
        assert_eq!(qp.depth(), 4);
        qp.submit();
        assert_eq!(qp.in_flight(), 1);
    }

    #[test]
    fn pool_empty() {
        let pool = QueuePairPool::from_detached(&[]);
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }
}
