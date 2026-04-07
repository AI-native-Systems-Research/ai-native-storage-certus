Model: Claude Opus 4.6

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

## Constitution

/speckit.constitution Create principles focused on code quality, extensive testing, 
established good engineering practice, maintainability and meeting performance requirements.  
All code must run on the Linux operating system.  All public APIs must have unit tests for 
correctness and performance, and must be well documented.  Rust documentation tests should 
exist for all public APIs.  All Rust performance tests should be based on Criterion and
 must be available for all performance sensitive code.  Assurance of code correctness is of high importance

### Missed
The use of unsafe blocks should be minimized.
Target x86 platform.

## Features

These are applied one at a time, going all the way to implementation - depth first.

### 001
/speckit.specify Build a Microsoft-COM like component framework in Rust. The framework must support interfaces (provided interfaces)
as well as recepticals (required interfaces). The framework must support the definition of interfaces that can be referenced   
by other component implementations without access to the full implementation.  This allows components to be developed in isolation.
Components support a collection of interfaces and recepticals. All components must provide a base interface, IUnknown, that can
be used for introspection on versions, interfaces and recepticals.  Macros should be used to define interfaces that make the code
easier to read and help distinguish interfaces from plain Rust structs.

### 002
/speckit.specify 
 The framework must support first-party and third-party binding.  Atomic reference counting with explicit attach/release should be
 used forsafe destruction of components.  Implement a component registry that uses a factory pattern to instantiate components. 
 
 Additional prompts:
 + Add examples of using first and third-party binding.

### 003 (Compacting conversation occurred)

/speckit.specify  In addition to plain components, the framework should support an Actor-based model 
whereby components own their own threads and can exchange messages via channels. Actors should use the basic component, interfaces and 
recepticals paradigm. Channels must be components themselves, as first-class entities, which are bound accordingly. Default channel
implementations include shared memory with lock-free queues. Atleast SPSC and MPSC channels are provided. Channel binds should be restricted depending
on whether they inherently support single (e.g., SPSC) or multiple (e.g. MPSC) bindings. Provide examples of using actor components.

Additional prompts:
+ Channels should be implemented as components with interfaces and recepticals.
+ Back-sync analyze

### 004

/speckit.specify Implement channels based on crossbeam, kanal, rtrb and tokio lock-free queues. Implement performance benchmarks for
different channels available so that their performance can be compared.

Additional prompts:
+ Add an example of using tokio threads and tokia channel for a ping-pong components scenario.
+ The tokio_ping_pong.rs example should use TokioMpscChannel
+ Back-sync analyze
+ Fix mismatch MpscChannel uses Mutex<VecDeque> instead of lock-free queue (FR-010)
+ Back-sync backfill

### 005

/speckit.specify The framework should be NUMA-aware and allow actor threads to be bound to one or more CPUs. Any performance tests     
 should analyze threads bound to the same NUMA zone and also to different NUMA zones.  You can assume that all systems have at least 2 NUMA      
 zones. Include an example of using NUMA pinning.

Additional prompts:
+ Add example using a factory to create an actor component.
+ Back-sync analyze, propose, apply

### 006

Additional prompts:

+ Build a generic LogHandler component as part of the framework that outputs the log to the 
console and a file.  This "default" handler should be used in the examples.

+ Create a markdown file, component-framework/summary.md, that summarizes the purpose of the component framework and details the different aspects of its         
capabilities.

### 007 

+ Can you suggest improvements that simplify and make the framework easier to use?                                                                                


+ Modify the framework to support dynamic component loading using an definition-only 
    representation of component interfaces that are bound without access to
their implementations. This is to enables a paradigm of independent extensibility.
