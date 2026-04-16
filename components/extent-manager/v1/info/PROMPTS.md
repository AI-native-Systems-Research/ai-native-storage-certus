specify init . --ai claude

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip


/init

/add-dir ../component-framework/ ../interfaces

/speckit-constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system.  All public APIs must have unit tests for correctness and performance, and must be well documented.  Rust documentation tests should exist for all public APIs.  All Rust performance tests must be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  Components must conform to the components/component-framework methodology.

/speckit-specify @info/DESIGN.md

+ Modify IExtentManager to use Error code enum types instead of error Strings                                                                                     
+ Replace the use of InMemoryBlockIO and BlockIO with the block-device-spdk-nvme component and the IBlockDevice interface.                                                  

User answered Claude's questions:                                                                                                                               
  ?  · IBlockDevice and all its types (DmaBuffer, Command, Completion, ClientChannels) are gated behind the `spdk` Cargo feature in the interfaces crate, because 
     DmaBuffer requires SPDK hugepage allocation. Currently, extent-manager compiles without SPDK and uses InMemoryBlockIO for all 93 tests. How should we handle 
     testing? ? Perform a full replacement and gate the component by spdk feature. Do not create a mock IBlockDevice, only use the block-device-spdk-nvme but have
      the option to use other IBlockDevice providers.                                                                                                             
+ Modify the code to use the define_component! macro to define the extent-manager component. Expose the requirement for IBlockDevice through a component receptacle.

+ Add ILogger as another receptacle and use this dependency for debug logging.

/speckit-specify Build unit tests to check API operation and data integrity in simulated power-failure. Include tests for thread-safety.  Implement benchmarks for basic interface operations.

+ Update the README.md

+ Make sure that the component only exposes public functions on interfaces.  Move any 'leaked' APIs to the IExtentManager or IExtentManagerAdmin interfaces.