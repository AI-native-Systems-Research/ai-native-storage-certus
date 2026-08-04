specify init . --ai claude

Model: Claude Opus 4.8

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

## Skeleton

Created component skeleton with /component-make-new skill

## Constitution

/speckit-constitution Create principles focused on code quality, extensive testing, established good engineering practice, maintainability and meeting performance requirements. All code must run on the Linux operating system. All public APIs must have unit tests for correctness and performance, and must be well documented. Rust documentation tests should exist for all public APIs. All Rust performance tests should be based on Criterion and must be available for all performance sensitive code. Assurance of code correctness is of high importance. Component should conform to the components/component-framework methodology. Component must only expose functions through interfaces, public functions outside the component are not allowed. All interfaces should be defined in the components/interfaces crate.

/speckit-specify This component implements an eviction that is an alternative to LRU. For each new session id, a FILO list - or chain of blocks (stack), is used to track lineage of cache block (i.e., what block is the parent). When a block B for session id S is pushed immediately after block A, then it is known that block A is the parent of block B. Each block maintains a timestamp for the most recent time that it has been accessed. When eviction candidates are being requested, the algorithm selects (pops) the block from the session stack that has the oldest use timestamp (i.e. LRU from top of the stacks) - we are basically choosing from the leaves. This approach attempts to improve on basic LRU by exploiting lineage information to avoid loosing the head or higher up members of the chain. When a block is referenced, its timestamp is refreshed unless it is being evicted.
