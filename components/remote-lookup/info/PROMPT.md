specify init . --ai claude

Model: Claude Opus 4.6

## Add spec-kit-sync

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

## Skeleton

Created component skeleton with /component-make-new skill

## Constitution

/add-dir ../component-framework/ ../interfaces ../dispatcher

/speckit-constitution Create principles focused on code quality, extensive testing,
established good engineering practice, maintainability and meeting performance requirements. All code must run on the Linux operating system. All public APIs must have unit tests for correctness and performance, and must be well documented. Rust documentation tests should exist for all public APIs. All Rust performance tests should be based on Criterion and must be available for all performance sensitive code. Assurance of code correctness is of high importance. Component should conform to the components/component-framework methodology. Component must only expose functions through interfaces, public functions outside the component are not allowed. All interfaces should be defined in the components/interfaces crate.

/speckit-specify The component is a placeholder for performing remote lookups to other nodes running Certus on the same network. The component has an ILogger receptacle. The IRemoteLookup interface should support the batch_lookup function with the same parameters as IDispatch export. For the moment, the lookup implementation should only output some logging statement--this is a placeholder.

/speckit-clarify

/speckit-plan

/speckit-tasks

/speckit-implement
