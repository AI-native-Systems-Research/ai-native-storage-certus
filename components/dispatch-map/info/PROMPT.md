specify init . --ai claude

Model: Claude Opus 4.6

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

## Skeleton

Created component skeleton with /component-make-new skill

## Constitution

/add-dir ../component-framework/ ../interfaces ../extent-manager ../block-device-spdk-nvme

/speckit-constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system.  All public APIs must have unit tests for correctness and performance, and must be well documented.  Rust documentation tests should exist for all public APIs.  All Rust performance tests should be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  Component should conform to the components/component-framework methodology. Component must only expose functions through interfaces, public functions outside the component are not allowed. All interfaces should be defined in the components/interfaces crate.

/speckit-specify @info/FUNCTIONAL-DESIGN.md

/speckit-clarify

Build README.md to summarize the component.

/speckit-plan

/speckit-tasks

/speckit-implement

>>> Problem. Passing raw pointers for DMA buffers instead of using DmaBuffer type.

create_staging() and lookup() should use Arc<DmaBuffer> references for DMA buffers.  DmaBuffer is defined in spdk_types.h

>>> Problem. Asking for DMA memory allocator function, instead of using SPDK default.

Can you use DmaBuffer::new to allocate buffers?

>>> Unnecessary complexity (bad spec)

Remove block_device_id since this is known implicitly and need not be specified. 

Remove the timeout parameters and have a statically configured time out of 100ms. We don't need variable timeout and don't want the expense of passing parameter/setting the timeout.

Add tests to make sure the locking is correct. 
