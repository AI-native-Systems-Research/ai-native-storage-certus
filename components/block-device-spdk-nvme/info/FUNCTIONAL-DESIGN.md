# "/speckit-specify @info/FUNCTIONAL-DESIGN.md"

Component is called BlockDeviceSpdkNvmeComponentV1.  

Component must use the components/component-framework as the basis for defining interfaces and receptacles etc.

Component should be built as an actor with a thread to service requests. 

The component should provide IBlockDevice interface for creating and connecting channels

Each client instance should have two shared memory channels, one for ingress command messages, and one for call-back messages (e.g., for asynchronous command completions). 

The actor thread polls all attached channels.

Component should use a ILogger receptacle to provide for debug logging services. LoggerComponent can be used for testing.

SPDK should be used to directly access NVMe controller devices. 

spdk-env component should be used to initialize SPDK

Each component instance is associated with a single NVMe controller device, which is attached to, and initialized at instantiation

Component should be aware of NVMe namespaces.

Client provides memory for read/write operations in the form of one or more DmaBuffer structs (defined in spdk_types.rs)

Component provides public messaging APIs for:
+ Probing, creating, formatting and deleting NVMe namespaces 
+ Both synchronous and asynchronous read/write operations with parameters for NVMe namespace id, DmaBuffer, LBA (logical block offset), time-out 
+ IO operations should include a time-out value and return errors if not completed before time-out
+ Writing zeros
+ Submitting batches of operations 
+ Aborting an asynchronous operations 
+ Completion of asynchronous operations can be signaled as a call back
+ Controller h/w reset 
+ Retrieving information about the device, capacity, including max queue depth, number of NVMe IO queues, max data transfer size, block/sector size, NUMA id for controller, NVMe version (provided as part of IBlockDevice interface as opposed to messaging)
+ The component should include an API for telemetry (provided as part of IBlockDevice interface as opposed to messaging). If compiled with 'telemetry' feature (with cargo build --feature telemetry), then statistics are collected for min,max,mean IO latencies, total operation count, mean throughput etc. The telemetry data should be accessible though public API. If the feature is not include, then the API should return an error.

Unit tests should be included.

Benchmarks should be defined to test latency and throughput at different queue-depths.

Actor thread should be pinned to cores in the same NUMA zone as the controller device

You can assume that clients for this component are in the same process and therefore references (e.g. Arc) can be included in messages to avoid expensive copies.

The component should exploit different NVMe IO queues with different queue depths to provide the lowest latency for requests for a given batch size.

A fast SPSC channel should be used for testing and benchmarking, e.g., crossbeam bounded to 64 slots.