/init

/add-dir ../component-framework/ ../interfaces ../spdk-env ../spdk-sys

/speckit-constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system.  All public APIs must have unit tests for correctness and performance, and must be well documented.  Rust documentation tests should exist for all public APIs.  All Rust performance tests should be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  Component should conform to the components/component-framework methodology. Component must only expose functions through interfaces, public functions outside the component are not allowed. All interfaces should be defined in the components/interfaces crate.

# H/w was not enabled.
+ Make sure tests and benchmarks run with or without SPDK hardware. If no hardware is present the tests pass but do nothing.  Hardware is now available, please run the tests.

+ Make sure IBlockDevice interface is added to the components/interfaces crate. Move the component implementation from components/block-device-spdk-nvme to components/block-device-spdk-nvme/v1. This will allow us to support different versions of the component.

+ Remove components/spdk-simple-block-device component and any associated interfaces defined in the interface create.
  
/speckit-specify Write an example application for this component that measures IOPS throughput for read and write operations. The example should take operation type (read,write,rw), block size, IO queue depth, number of client threads and test duration in seconds.  Default values for these should be available.

+ Amend app for per-thread breakdown
+ Amend unit tests to check IO operations and multiple client threads

After logger component was made available:
+ Modify the block-device-spdk-nvme component to include an ILogger receptacle.  Modify the component to out info and debug messages through the log component.

/speckit-specify @info/FUNCTIONAL-DESIGN.md

/speckit-clarify

Build README.md to summarize the component.

/speckit-plan

/speckit-tasks

/speckit-implement

