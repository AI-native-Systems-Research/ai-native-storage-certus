specify init . --ai claude

Model: Claude Opus 4.6
Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

/add-dir ../component-framework/ ../interfaces ../extent-manager ../block-device-spdk-nvme

/speckit-constitution Create principles focused on code quality,
extensive testing, established good engineering practice,
maintainability and meeting performance requirements. All code must
run on the Linux operating system. All public APIs must have unit
tests for correctness and performance, and must be well
documented. Rust documentation tests should exist for all public
APIs. All Rust performance tests should be based on Criterion and must
be available for all performance sensitive code. Assurance of code
correctness is of high importance. Component should conform to the
components/component-framework methodology. Component must only expose
functions through interfaces, public functions outside the component
are not allowed. All interfaces should be defined in the
components/interfaces crate.

/speckit-specify Create a new block-device component, block-device-filesys,
that exports the IBlockDevice interface and has a receptacle for
ILogger.  The component is a substitute for the block-device-spdk-nvme
that does not use SPDK NVMe driver, but instead uses a file on a
kernel-based filesystem to simulate a block device.  Create unit tests
for the component and benchmarks like those available for
block-device-spdk-nvme.

/speckit-clarify

