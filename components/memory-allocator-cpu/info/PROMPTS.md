Model: Claude Opus 4.6

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

## Constitution

/speckit.constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system.  All public APIs must have unit tests for correctness and performance, and must be well documented.  Rust documentation tests should exist for all public APIs.  All Rust performance tests should be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  Components should conform to the components/component-framework methodology.

## Features

/speckit-specify Build a component for SPDK-based CPU memory allocation. The component should bind to the ISPDKEnv interface provided by an instantiation of spdk-env component. A receptical should be included. This component uses the DmaBuffer type from components/interfaces crate as the basis for memory handles.  The component exposes an interface IMemoryManagement that provides APIs for allocating, reallocating, zmalloc and freeing memory.  The component should include stats detailed how much memory, in what zones, has been allocated and how much memory remains. The implementation should use SPDK functions, spdk_dma_xx  and APIs should use optional NUMA affinity parameters.  Include unit tests and Criterion performance benchmarks.

/speckit-specify The component should be thread safe (re-entrant) and support lock-based protection of stats state.