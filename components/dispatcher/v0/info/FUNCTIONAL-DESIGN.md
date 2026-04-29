Component must be written in Rust 
Component must use the components/component-framework as the basis for defining interfaces and receptacles etc.
Use types defined in idispatch_map.rs and iblock_device.rs where possible.

Terminology:
'data block device' - block device that only holds the cached data.
'metadata block device' - block device with namespace partitions that holds metadata for an extent-manager instance.

This component requires instances of logger (x1), block-device-spdk-nvme (N + 1), extent-manager (N)

Initialization of the Dispatcher requires wiring to the following other components:
1) Logger for info, debug, warn and error console output
2) Block device for metadata with N partitions. The pci address of the metadata block device is passed as a parameter to the initialization function.
3) N x "data" block devices, one for each partition.  The pci addresses of each data block device is passed as a parameter to the initialization function.
4) N x extent-manager components, which are wired to the logger and metadata block device. The extent-manager uses a partition governed by the passed namespace id. The size of the corresponding data block device and a unique identifier, e.g., converted controller pci addr, is passed to the extent-manager format() function.  The unique identifier is used to ensure that the extent-manager + metadata can be mapped to the data block device on restart.


The dispatcher provides an interface IDispatcher that provides methods to:
- lookup(key,ipc-handle) --> Lookup a cache element: Caller to lookup() provides a key (CacheKey) on the DispatchMap.  If the key exists, i.e. a hit has occurred, then the resulting action is to perform a DMA copy into the GPU memory (address/ipc-handle provided by the client) from either the staging memory (CPU memory) or alternatively from the SSD.  Query on the DispatchTable is used to get the location of the data.
- check(key) --> Check if a cache element is present in the cache using DispatchMap. Do not transfer.
- remove(key) --> Remove an cache element. Free staging buffer if not yet in SSD. Free extent, if data is in SSD. 
- populate(key,ipc-handle) --> Create a new element in cache. Use DispatchMap to register element and allocate a CPU memory staging buffer.  Direct DMA from GPU memory (ipc-handle) into staging buffer.  Return confirmation to client. Meanwhile asynchronously execute DMA of cache element in staging buffer, into SSD.  Take locks on DispatchMap as needed. Free staging before after DMA to SSD is complete.

A description of the data flows is provided in files design/design-spec-hit-flow.md and design/design-spec-put-flow.md

The SSD device has a limited Maximum Data Transfer Size (MDTS), usually 128K.  This should be queried from the device. Make sure that IO operations to the block device are broken into segments.

Build a README.md that explains the component and its interfaces.

