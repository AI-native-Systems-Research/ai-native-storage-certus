# "/speckit-specify @info/DESIGN.md"

Component must use the components/component-framework as the basis for defining interfaces and receptacles etc.

Component is called ExtentManagerComponentV1.  It exports an interface IExtentManager and receptacles for IBlockDevice and ILogger.

Component must use the components/component-framework as the basis for defining interfaces and receptacles etc.
Extents represent a continuous region of data on disk. 
This Extent Manager component should manage Extents.
The component should provide interface IExtentManager.
Extents have metadata associated with them. The metadata includes:
- Location of data on some SSD, i.e., NVMe device, namespace id and offset in 4KiB blocks.
- 64bit key uniquely identifying extent.
- Optional filename.
- Optional CRC of data.
The NVMe SSD hardware supports power-fail atomic 4KiB (4KiB aligned) writes.
Extents are fixed size. Only a few (1-32) fixed sizes ranging from 128KiB to 5MiB need to be supported. The extent sizes that need to be supported are determined at run time. Sizes are multiples of 4KiB.
A receptacle for IBlockDevice (e.g., component block-device-spdk-nvme) should be provided to store persistent metadata on SSD. Metadata writes should be atomic and consistent in the event of power-off/power-fail events. Data corruption in metadata should be avoided.
Interface IExtentManager should provide APIs for:
- Creating new extents (given key, extent size, and optionally filename, CRC) and marking space as allocated.
- Removing extents (given key) and freeing space.
- Looking up extents given key (so that the client can read the corresponding data from some block device) providing access to extent metadata.
- Iterating through all extents as fast as possible - this is needed for rebuilding other in-memory volatile caching/indexing.
Methods on the interface should be re-entrant and thread safe.
Unit tests should be included that check API operation and data integrity in simulated power-failure. Include tests for thread-safety.
Add benchmarks for basic operations.
Add README.md to describe the component and give instructions how to run unit tests and benchmarks.
