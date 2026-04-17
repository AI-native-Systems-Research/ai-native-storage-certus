# Model Opus 4.7

specify init . --ai claude

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

/init


/speckit-constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system.  All public APIs must have unit tests for correctness and performance, and must be well documented.  All Rust performance tests must be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  Components must conform to the component-framework methodology. All public functions should be exposed only as part of an interface.  The macros define_interface! and define_component! must be used to define interfaces and components respectfully.

/speckit-specify @info/DESIGN.md

+ Modify the component so that the initialization function on IExtentManagerAdmin is given a size of the space to manage.  The component manages this space and dynamically allocates slabs (e.g. of 1GiB in size) from this space as new sizes are demanded.  The basic architecture is a set of slab allocators which are dynamically allocated.  

/speckit-specify Create a benchmarking application, apps/extent-benchmark, for this component that measures latencies and throughput of extent allocation, lookup and remove/deletion.  The application should allow multiple client threads through a --threads option.  Add a README.md summarizing the app and giving instructions on how to run with real SPDK NVme block device.