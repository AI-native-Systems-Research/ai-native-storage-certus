Model: Claude Opus 4.6

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

## Constitution

/speckit.constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system.  All public APIs must have unit tests for correctness and performance, and must be well documented.  Rust documentation tests should exist for all public APIs.  All Rust performance tests should be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  Components should conform to the components/component-framework methodology.

## Features

/speckit.specify Build a component, as a lib-based crate, that initializes the SPDK and DPDK environments and iterates over available VFIO attached devices.  The component should use the framework provided in ../component-framework. The component interface, ISPDKEnv, should provide methods for iterating over available devices.  The component must verify the availability of VFIO and raise an error if the system is not configured correctly.  The componet should use the logging APIs provided by the framework.  Add a test example main.rs that instantiates the component.  The component should run without root permissions providing that /dev/vfio directories are user accessible.  The component should check for permission and report an error as needed. This component is not an actor, but a plain procedural component.

+ Make sure there are no references to /home/dwaddington. An environment variable should be used instead.
  
+ Add unit tests for spdk-env and spdk-sys components.

+ Write a script in the scripts dir, that allows the user to detach SSD devices from the kernel-driver and attach the vfio-driver

+ The --example sdpk-env-example is not showing devices that our bound to vfio-pci. The SPDK tool spdk_nvme_identify, shows the available device correctly. 