//! GPU services interface trait definition.

use component_macros::define_interface;

/// Information about a discovered GPU device.
///
/// Returned by [`IGpuServices::get_devices`] after successful
/// initialization.  Only GPUs with compute capability 7.0+ (Volta
/// and newer) are reported.
///
/// # Examples
///
/// ```
/// use interfaces::GpuDeviceInfo;
///
/// let info = GpuDeviceInfo {
///     device_index: 0,
///     name: "NVIDIA A100".to_string(),
///     memory_bytes: 42_949_672_960,
///     compute_major: 8,
///     compute_minor: 0,
///     pci_bus_id: "0000:3b:00.0".to_string(),
/// };
/// assert_eq!(info.compute_major, 8);
/// assert!(info.memory_bytes > 0);
/// ```
#[derive(Debug, Clone)]
pub struct GpuDeviceInfo {
    /// CUDA device ordinal.
    pub device_index: u32,
    /// GPU model name (e.g., "NVIDIA A100").
    pub name: String,
    /// Total global memory in bytes.
    pub memory_bytes: u64,
    /// Compute capability major version (>= 7 guaranteed).
    pub compute_major: u32,
    /// Compute capability minor version.
    pub compute_minor: u32,
    /// PCI Bus-Device-Function address string.
    pub pci_bus_id: String,
}

/// An opened CUDA IPC memory handle.
///
/// Represents a reference to GPU memory obtained by deserializing a
/// base64-encoded IPC handle from a remote process.  Tracks
/// verification and pinning state for safety.
///
/// # Examples
///
/// ```
/// use interfaces::GpuIpcHandle;
///
/// // GpuIpcHandle is obtained from IGpuServices::deserialize_ipc_handle
/// // and should not be constructed manually in production code.
/// ```
#[derive(Debug)]
pub struct GpuIpcHandle {
    /// GPU device memory pointer (from cudaIpcOpenMemHandle).
    pub(crate) ptr: *mut std::ffi::c_void,
    /// Buffer size in bytes.
    pub(crate) size: usize,
    /// Whether verify_memory() has been called successfully.
    pub(crate) verified: bool,
    /// Whether pin_memory() has been called successfully.
    pub(crate) pinned: bool,
}

// SAFETY: The GPU pointer is valid from any thread once opened.
unsafe impl Send for GpuIpcHandle {}
unsafe impl Sync for GpuIpcHandle {}

impl GpuIpcHandle {
    /// Create a new handle (crate-internal constructor).
    pub fn new(ptr: *mut std::ffi::c_void, size: usize) -> Self {
        Self {
            ptr,
            size,
            verified: false,
            pinned: false,
        }
    }

    /// Return the buffer size in bytes.
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Return the raw device pointer.
    #[inline]
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.ptr
    }

    /// Return whether this handle has been verified.
    #[inline]
    pub fn is_verified(&self) -> bool {
        self.verified
    }

    /// Return whether this handle has been pinned.
    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// Mark this handle as verified (or not).
    #[inline]
    pub fn set_verified(&mut self, val: bool) {
        self.verified = val;
    }

    /// Mark this handle as pinned (or not).
    #[inline]
    pub fn set_pinned(&mut self, val: bool) {
        self.pinned = val;
    }
}

/// A buffer backed by GPU device memory obtained via CUDA IPC.
///
/// Owns the GPU memory pointer and will close the IPC handle on drop.
/// Can be converted to a `DmaBuffer` when the `spdk` feature is enabled.
///
/// # Examples
///
/// ```
/// use interfaces::GpuDmaBuffer;
///
/// // GpuDmaBuffer is typically obtained from IGpuServices::create_dma_buffer
/// // and should not be constructed manually in production code.
/// ```
pub struct GpuDmaBuffer {
    /// GPU device memory pointer.
    ptr: *mut std::ffi::c_void,
    /// Buffer size in bytes.
    len: usize,
    /// Deallocation function (calls cudaIpcCloseMemHandle).
    free_fn: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
}

// SAFETY: GPU device memory is accessible from any thread via DMA.
// The pointer remains valid until free_fn is called on drop.
unsafe impl Send for GpuDmaBuffer {}
unsafe impl Sync for GpuDmaBuffer {}

impl GpuDmaBuffer {
    /// Create a new GPU DMA buffer wrapping a device pointer.
    ///
    /// # Safety
    ///
    /// * `ptr` must be a valid GPU device pointer from cudaIpcOpenMemHandle.
    /// * `len` must be the correct size of the allocation.
    /// * `free_fn` must correctly close the IPC handle when called with `ptr`.
    pub unsafe fn new(
        ptr: *mut std::ffi::c_void,
        len: usize,
        free_fn: unsafe extern "C" fn(*mut std::ffi::c_void),
    ) -> Self {
        Self {
            ptr,
            len,
            free_fn: Some(free_fn),
        }
    }

    /// Return the buffer length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Return true if the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Return the raw GPU device pointer.
    #[inline]
    pub fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.ptr
    }
}

impl Drop for GpuDmaBuffer {
    fn drop(&mut self) {
        if let Some(free_fn) = self.free_fn.take() {
            if !self.ptr.is_null() {
                // SAFETY: ptr was obtained from cudaIpcOpenMemHandle and has not
                // been freed. free_fn wraps cudaIpcCloseMemHandle.
                unsafe {
                    (free_fn)(self.ptr);
                }
            }
        }
    }
}

impl std::fmt::Debug for GpuDmaBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuDmaBuffer")
            .field("ptr", &self.ptr)
            .field("len", &self.len)
            .finish()
    }
}

/// Opaque handle to a CUDA stream for async GPU operations.
///
/// Wraps a raw `cudaStream_t` pointer. Created via
/// [`IGpuServices::create_stream`] and destroyed via
/// [`IGpuServices::destroy_stream`].
///
/// # Examples
///
/// ```
/// use interfaces::GpuStream;
///
/// let stream = GpuStream(std::ptr::null_mut());
/// assert!(stream.0.is_null());
/// ```
#[derive(Debug, Clone, Copy)]
pub struct GpuStream(pub *mut std::ffi::c_void);

// SAFETY: CUDA streams can be used from any thread.
unsafe impl Send for GpuStream {}
unsafe impl Sync for GpuStream {}

define_interface! {
    pub IGpuServices {
        /// Initialize CUDA libraries and discover qualifying GPUs.
        ///
        /// Loads the CUDA runtime, enumerates all NVIDIA GPUs, and
        /// filters to those with compute capability 7.0+.  Idempotent.
        ///
        /// # Errors
        ///
        /// Returns an error if CUDA drivers are not installed, no
        /// qualifying GPU is detected, or CUDA initialization fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices) {
        /// gpu.initialize().expect("CUDA init failed");
        /// # }
        /// ```
        fn initialize(&self) -> Result<(), String>;

        /// Shut down CUDA context and release all resources.
        ///
        /// Closes any open IPC handles, unpins memory, and clears
        /// device state.  Safe to call even if not initialized.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices) {
        /// gpu.shutdown().expect("shutdown failed");
        /// # }
        /// ```
        fn shutdown(&self) -> Result<(), String>;

        /// Return information about all discovered GPUs.
        ///
        /// Only GPUs with compute capability 7.0+ are included.
        /// Must be called after successful initialization.
        ///
        /// # Errors
        ///
        /// Returns an error if the component is not initialized.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices) {
        /// gpu.initialize().unwrap();
        /// let devices = gpu.get_devices().unwrap();
        /// assert!(!devices.is_empty());
        /// # }
        /// ```
        fn get_devices(&self) -> Result<Vec<GpuDeviceInfo>, String>;

        /// Deserialize a base64-encoded CUDA IPC handle and size.
        ///
        /// Input: base64 string encoding 72 bytes (64-byte
        /// cudaIpcMemHandle_t + 8-byte LE u64 size).  Opens the IPC
        /// handle and returns an opaque handle referencing GPU memory.
        ///
        /// # Errors
        ///
        /// Returns an error if not initialized, base64 is invalid,
        /// payload is not 72 bytes, or CUDA IPC open fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices, payload: &str) {
        /// gpu.initialize().unwrap();
        /// let handle = gpu.deserialize_ipc_handle(payload).unwrap();
        /// # }
        /// ```
        fn deserialize_ipc_handle(
            &self, base64_payload: &str
        ) -> Result<GpuIpcHandle, String>;

        /// Verify that GPU memory is device-type and suitable for DMA.
        ///
        /// Checks that the memory is device-allocated (not managed or
        /// host), confirming it is contiguous and implicitly pinned.
        ///
        /// # Errors
        ///
        /// Returns an error if pointer attributes cannot be queried
        /// or the memory is not device-type.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices, handle: &interfaces::GpuIpcHandle) {
        /// gpu.verify_memory(handle).expect("verification failed");
        /// # }
        /// ```
        fn verify_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>;

        /// Pin GPU memory for DMA operations (idempotent).
        ///
        /// # Errors
        ///
        /// Returns an error if pinning fails due to insufficient
        /// resources or an invalid handle.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices, handle: &interfaces::GpuIpcHandle) {
        /// gpu.pin_memory(handle).expect("pin failed");
        /// # }
        /// ```
        fn pin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>;

        /// Unpin previously pinned GPU memory.
        ///
        /// # Errors
        ///
        /// Returns an error if the handle was not previously pinned.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices, handle: &interfaces::GpuIpcHandle) {
        /// gpu.unpin_memory(handle).expect("unpin failed");
        /// # }
        /// ```
        fn unpin_memory(&self, handle: &GpuIpcHandle) -> Result<(), String>;

        /// Create a DMA buffer backed by GPU memory from an IPC handle.
        ///
        /// The handle must have been verified and pinned prior to this
        /// call.  Consumes the handle; dropping the returned buffer
        /// closes the IPC handle.
        ///
        /// # Errors
        ///
        /// Returns an error if the handle has not been verified/pinned
        /// or if buffer creation fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices, handle: interfaces::GpuIpcHandle) {
        /// let buf = gpu.create_dma_buffer(handle).unwrap();
        /// assert!(buf.len() > 0);
        /// # }
        /// ```
        fn create_dma_buffer(
            &self, handle: GpuIpcHandle
        ) -> Result<GpuDmaBuffer, String>;

        /// Copy data from GPU device memory to a DMA staging buffer.
        ///
        /// Performs a synchronous `cudaMemcpy` with `DeviceToHost` direction.
        /// Copies exactly `size` bytes from the GPU source into `dst`.
        ///
        /// # Safety (caller contract)
        ///
        /// * `src` must be a valid GPU device pointer for at least `size` bytes.
        ///
        /// # Errors
        ///
        /// Returns an error if GPU support is not compiled in, the component
        /// is not initialized, `size` exceeds the destination buffer length,
        /// or the CUDA memcpy operation fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IGpuServices, DmaBuffer};
        /// # fn example(gpu: &dyn IGpuServices, gpu_ptr: *const std::ffi::c_void, buf: &DmaBuffer) {
        /// gpu.dma_copy_to_host(gpu_ptr, buf, 4096).unwrap();
        /// # }
        /// ```
        #[cfg(feature = "spdk")]
        fn dma_copy_to_host(
            &self, src: *const std::ffi::c_void, dst: &crate::spdk_types::DmaBuffer, size: usize
        ) -> Result<(), String>;

        /// Copy data from a DMA staging buffer to GPU device memory.
        ///
        /// Performs a synchronous `cudaMemcpy` with `HostToDevice` direction.
        /// Copies exactly `size` bytes from `src` into the GPU destination.
        ///
        /// # Safety (caller contract)
        ///
        /// * `dst` must be a valid GPU device pointer for at least `size` bytes.
        ///
        /// # Errors
        ///
        /// Returns an error if GPU support is not compiled in, the component
        /// is not initialized, `size` exceeds the source buffer length,
        /// or the CUDA memcpy operation fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IGpuServices, DmaBuffer};
        /// # fn example(gpu: &dyn IGpuServices, buf: &DmaBuffer, gpu_ptr: *mut std::ffi::c_void) {
        /// gpu.dma_copy_to_device(buf, gpu_ptr, 4096).unwrap();
        /// # }
        /// ```
        #[cfg(feature = "spdk")]
        fn dma_copy_to_device(
            &self, src: &crate::spdk_types::DmaBuffer, dst: *mut std::ffi::c_void, size: usize
        ) -> Result<(), String>;

        /// Prepare GPU IPC memory for peer-to-peer SSD DMA in one call.
        ///
        /// Accepts a base64-encoded CUDA IPC handle payload (64-byte handle +
        /// 8-byte LE size = 72 bytes), opens it with lazy peer access, checks
        /// whether the memory is already pinned, conditionally pins it, and
        /// returns an SPDK [`DmaBuffer`](crate::spdk_types::DmaBuffer) suitable
        /// for direct NVMe peer-to-peer transfers.
        ///
        /// The returned buffer's drop behavior depends on the original pin state:
        /// - If this function pinned the memory, drop unpins then closes the IPC handle.
        /// - If the memory was already pinned, drop only closes the IPC handle.
        ///
        /// # Parameters
        ///
        /// * `base64_payload` — base64-encoded 72-byte IPC handle from PyTorch (via gRPC).
        /// * `device_index` — optional CUDA device ordinal; if `Some`, sets the
        ///   device context before opening the handle. If `None`, uses the current
        ///   CUDA device.
        ///
        /// # Errors
        ///
        /// Returns an error if not initialized, the payload is invalid, the IPC
        /// handle cannot be opened, pinning fails, or buffer creation fails. No
        /// GPU resources are leaked on error.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices, payload: &str) {
        /// gpu.initialize().unwrap();
        /// let dma_buf = gpu.prepare_memory_for_spdk(payload, None).unwrap();
        /// assert!(dma_buf.len() > 0);
        /// # }
        /// ```
        #[cfg(feature = "spdk")]
        fn prepare_memory_for_spdk(
            &self, base64_payload: &str, device_index: Option<u32>
        ) -> Result<crate::spdk_types::DmaBuffer, String>;

        /// Create a CUDA stream for async GPU operations.
        ///
        /// # Errors
        ///
        /// Returns an error if GPU support is not compiled in, the component
        /// is not initialized, or CUDA stream creation fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices) {
        /// let stream = gpu.create_stream().unwrap();
        /// gpu.destroy_stream(stream).unwrap();
        /// # }
        /// ```
        fn create_stream(&self) -> Result<GpuStream, String>;

        /// Destroy a CUDA stream previously created with [`create_stream`].
        ///
        /// # Errors
        ///
        /// Returns an error if the stream handle is invalid or destruction fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices) {
        /// let stream = gpu.create_stream().unwrap();
        /// gpu.destroy_stream(stream).unwrap();
        /// # }
        /// ```
        fn destroy_stream(&self, stream: GpuStream) -> Result<(), String>;

        /// Query whether all operations on a CUDA stream have completed.
        ///
        /// Returns `Ok(true)` if all operations are complete, `Ok(false)` if
        /// work is still in-flight. Unlike [`stream_synchronize`], this does
        /// not block.
        ///
        /// # Errors
        ///
        /// Returns an error if the stream handle is invalid.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices) {
        /// let stream = gpu.create_stream().unwrap();
        /// // ... issue async work on stream ...
        /// if gpu.stream_query(stream).unwrap() {
        ///     println!("all work complete");
        /// }
        /// gpu.destroy_stream(stream).unwrap();
        /// # }
        /// ```
        fn stream_query(&self, stream: GpuStream) -> Result<bool, String>;

        /// Synchronize a CUDA stream, blocking until all operations complete.
        ///
        /// # Errors
        ///
        /// Returns an error if synchronization fails or the stream is invalid.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::IGpuServices;
        /// # fn example(gpu: &dyn IGpuServices) {
        /// let stream = gpu.create_stream().unwrap();
        /// gpu.stream_synchronize(stream).unwrap();
        /// gpu.destroy_stream(stream).unwrap();
        /// # }
        /// ```
        fn stream_synchronize(&self, stream: GpuStream) -> Result<(), String>;

        /// Asynchronously copy data from a DMA buffer to GPU device memory.
        ///
        /// Issues a `cudaMemcpyAsync` on the given stream. The copy runs
        /// on the GPU DMA engine concurrently with CPU/NVMe work. Call
        /// [`stream_synchronize`] to wait for completion.
        ///
        /// # Safety (caller contract)
        ///
        /// * `dst` must be a valid GPU device pointer for at least `size` bytes.
        /// * The source buffer must remain valid until the stream is synchronized.
        ///
        /// # Errors
        ///
        /// Returns an error if GPU support is not compiled in, the component
        /// is not initialized, `size` exceeds the source buffer length,
        /// or the CUDA async memcpy operation fails.
        ///
        /// # Examples
        ///
        /// ```no_run
        /// # use interfaces::{IGpuServices, DmaBuffer, GpuStream};
        /// # fn example(gpu: &dyn IGpuServices, buf: &DmaBuffer, gpu_ptr: *mut std::ffi::c_void) {
        /// let stream = gpu.create_stream().unwrap();
        /// gpu.dma_copy_to_device_async(buf, gpu_ptr, 4096, stream).unwrap();
        /// gpu.stream_synchronize(stream).unwrap();
        /// gpu.destroy_stream(stream).unwrap();
        /// # }
        /// ```
        #[cfg(feature = "spdk")]
        fn dma_copy_to_device_async(
            &self, src: &crate::spdk_types::DmaBuffer, dst: *mut std::ffi::c_void,
            size: usize, stream: GpuStream
        ) -> Result<(), String>;

        /// Asynchronously copy from a raw host pointer to GPU device memory.
        ///
        /// Like [`dma_copy_to_device_async`] but takes a raw `src` pointer,
        /// avoiding the need to wrap pre-existing pinned memory in a `DmaBuffer`.
        ///
        /// # Safety (caller contract)
        ///
        /// * `src` must be a valid, CUDA-pinned host pointer for at least `size` bytes.
        /// * `dst` must be a valid GPU device pointer for at least `size` bytes.
        /// * Both pointers must remain valid until the stream is synchronized.
        ///
        /// # Errors
        ///
        /// Returns an error if GPU support is not compiled in, the component
        /// is not initialized, or the CUDA async memcpy operation fails.
        #[cfg(feature = "spdk")]
        fn memcpy_h2d_async(
            &self, src: *const std::ffi::c_void, dst: *mut std::ffi::c_void,
            size: usize, stream: GpuStream
        ) -> Result<(), String>;

        /// Asynchronously copy data from GPU device memory to a DMA buffer.
        ///
        /// Issues a `cudaMemcpyAsync` with `DeviceToHost` direction on the given
        /// stream. The copy runs on the GPU DMA engine concurrently with CPU/NVMe
        /// work. Call [`stream_synchronize`] to wait for completion.
        ///
        /// # Safety (caller contract)
        ///
        /// * `src` must be a valid GPU device pointer for at least `size` bytes.
        /// * The destination buffer must remain valid until the stream is synchronized.
        ///
        /// # Errors
        ///
        /// Returns an error if GPU support is not compiled in, the component
        /// is not initialized, `size` exceeds the destination buffer length,
        /// or the CUDA async memcpy operation fails.
        #[cfg(feature = "spdk")]
        fn dma_copy_to_host_async(
            &self, src: *const std::ffi::c_void, dst: &crate::spdk_types::DmaBuffer,
            size: usize, stream: GpuStream
        ) -> Result<(), String>;

        /// Asynchronously copy from GPU device memory to a raw host pointer.
        ///
        /// Like [`dma_copy_to_host_async`] but takes a raw `dst` pointer,
        /// avoiding the need to wrap pre-existing pinned memory in a `DmaBuffer`.
        ///
        /// # Safety (caller contract)
        ///
        /// * `src` must be a valid GPU device pointer for at least `size` bytes.
        /// * `dst` must be a valid, CUDA-pinned host pointer for at least `size` bytes.
        /// * Both pointers must remain valid until the stream is synchronized.
        ///
        /// # Errors
        ///
        /// Returns an error if GPU support is not compiled in, the component
        /// is not initialized, or the CUDA async memcpy operation fails.
        #[cfg(feature = "spdk")]
        fn memcpy_d2h_async(
            &self, src: *const std::ffi::c_void, dst: *mut std::ffi::c_void,
            size: usize, stream: GpuStream
        ) -> Result<(), String>;

        /// Allocate a CUDA-pinned host buffer registered with SPDK for DMA.
        ///
        /// The returned buffer is:
        /// - Page-locked via `cudaHostAlloc` (enables truly async H2D copies)
        /// - Registered with SPDK (`spdk_mem_register`) so NVMe can DMA into it
        ///
        /// This is the correct allocation for pipeline ring buffers that need
        /// to receive NVMe reads AND be source for async GPU copies.
        ///
        /// # Errors
        ///
        /// Returns an error if GPU support is not compiled, CUDA host allocation
        /// fails, or SPDK memory registration fails.
        #[cfg(feature = "spdk")]
        fn allocate_pinned_dma_buffer(&self, size: usize) -> Result<crate::spdk_types::DmaBuffer, String>;

        /// Register existing host memory as CUDA-pinned and SPDK DMA-capable.
        ///
        /// Calls `cudaHostRegister` to page-lock the memory (enabling async
        /// GPU DMA) then `spdk_mem_register` so NVMe controllers can DMA
        /// directly to/from it.
        ///
        /// Use this to make a pre-allocated pool (e.g., memory-tier mmap)
        /// usable for zero-copy NVMe and GPU transfers without reallocating.
        ///
        /// # Safety (caller contract)
        ///
        /// * `ptr` must be page-aligned and valid for `size` bytes.
        /// * The memory must remain allocated until `unregister_host_memory`.
        ///
        /// # Errors
        ///
        /// Returns an error if CUDA host registration or SPDK registration fails.
        #[cfg(feature = "spdk")]
        fn register_host_memory(&self, ptr: *mut std::ffi::c_void, size: usize) -> Result<(), String>;

        /// Unregister host memory previously registered with `register_host_memory`.
        ///
        /// Calls `spdk_mem_unregister` then `cudaHostUnregister`. Must be
        /// called before freeing the underlying allocation.
        ///
        /// # Errors
        ///
        /// Returns an error if unregistration fails.
        #[cfg(feature = "spdk")]
        fn unregister_host_memory(&self, ptr: *mut std::ffi::c_void, size: usize) -> Result<(), String>;
    }
}
