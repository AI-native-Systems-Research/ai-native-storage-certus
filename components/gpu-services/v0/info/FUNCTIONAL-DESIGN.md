This component provides services to interact with a GPU.  It is written in Rust but uses the CUDA 'C' libraries.

The component build is guarded by a feature gate, --features gpu

The IGpuServices interface provides the following:
1) Ability to initialize CUDA libraries
2) Scan system for GPU hardware and provide information about GPU model, memory capacity and supported architecture level.
3) Deserialize a CUDA ipc-handle and size from a Python base-64 encoded serialized handle and size.
4) Check that the GPU memory associated with an ipc-handle is contiguous and pinned.
5) A function to pin and unpin GPU memory.
6) Create a DmaBuffer (defined in object spdk_types.rs) from an ipc-handle that can be used to perform DMA from SSD (via block-device-spdk-nvme) or from CPU-memory allocated DmaBuffer.  DMA from CPU-memory to GPU-memory and vice versa, is implemented using the CUDA libraries.
7) DMA transfer operations, both synchronous and asynchronous, between GPU memory and CPU memory.

The code should include a test application, apps/gpu-handle-test-client (Python) and apps/gpu-handle-test-server (Rust) that demonstrates a Python process handing off a GPU ipc-handle to a Rust process that is using this component, and then perform DMA operations from CPU memory allocated by SPDK.

Write unit tests and benchmarks available when feature --gpu is enabled.
