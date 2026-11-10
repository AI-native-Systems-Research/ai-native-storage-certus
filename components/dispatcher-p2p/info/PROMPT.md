specify init . --integration claude

Model: Claude Opus 4.6

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

## Skeleton

Created component via /component-new-version from dispatcher. Renamed to DispatcherP2pComponent.

## Constitution

/add-dir ../component-framework/ ../interfaces ../gpu-services ../spdk-env ../block-device-spdk-nvme ../extent-manager ../memory-tier

/speckit-constitution Create principles focused on code quality, extensive testing,
established good engineering practice, maintainability and meeting performance requirements.
All code must run on the Linux operating system. All public APIs must have unit tests
for correctness and performance, and must be well documented. Rust documentation tests
should exist for all public APIs. All Rust performance tests should be based on Criterion
and must be available for all performance sensitive code. Assurance of code correctness
is of high importance. Component should conform to the components/component-framework
methodology. Component must only expose functions through interfaces, public functions
outside the component are not allowed. All interfaces should be defined in the
components/interfaces crate. The P2P path must gracefully fall back to DRAM if
GDRCopy/BAR1 is unavailable. Performance should be measurable.

/speckit-specify @info/FUNCTIONAL-DESIGN.md

/speckit-plan

/speckit-tasks

/speckit-implement
