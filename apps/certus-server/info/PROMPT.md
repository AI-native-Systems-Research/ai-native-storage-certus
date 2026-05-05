specify init . --ai claude

Model: Claude Opus 4.6

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip


## Constitution

/add-dir ../component-framework/ ../interfaces ../extent-manager ../block-device-spdk-nvme

/speckit-constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system.  All public APIs must have unit tests for correctness and performance, and must be well documented.  Rust documentation tests should exist for all public APIs.  All Rust performance tests should be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  

/speckit-specify Build an application that exposes an instance of the dispatcher component, and its interface IDispatcher, to a Python client via gRPC.  The gRPC protocol should expose the methods on the IDispatcher interface.  Configuration parameters, such as PCI address for metadata and data NVMe devices, should be command line configurable.  The implementation must include a Python test-client that provides basic testing.  The protocol should support multi-instance list-based parameters. For example fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) should be exposed to the client as populate([(key0,ipc0),[key1,ipc1]]) and soforth.  Mapping to the singular fn populate(), should be done on the server side to avoid excessive round-trips across the gRPC protocol.

/speckit-clarify

Build README.md to summarize the component.

/speckit-plan

/speckit-tasks

/speckit-implement

Update certus/deps/install_deps.sh and requirements.txt to install tools neccessary to build the certus-server.

In certus/design directory, create a UML deplyoment diagram showing what components are used in the certus-server implementation, including inner components.