# Model Opus 4.6

specify init . --ai claude

specify extension add spec-kit-sync --from https://github.com/bgervin/spec-kit-sync/archive/refs/heads/master.zip

/init

/speckit-constitution Create principles focused on code quality, extensive testing, established good engineering practice, maintainability and meeting performance requirements.  All code must run on the Linux operating system.  All public APIs must have unit tests for correctness and performance, and must be well documented.  All Rust performance tests must be based on Criterion and must be available for all performance sensitive code.  Assurance of code correctness is of high importance.  Components must conform to the component-framework methodology. All public functions should be exposed only as part of an interface.  The macros define_interface! and define_component! must be used to define interfaces and components respectfully. All components need a README.md describing the component and how to build and test it.

/speckit-specify Build a logging component, in directory components/logger, that provides logging to the console or file. The interface should be called ILogger (and added to the interface create).  The component name should be called LoggerComponentV1. The logger must support log levels (debug, info, warn, error) that can be set by using the RUST_LOG environment variable (in the same way as env_logger crate).  The logger must support console output by default, but also allow logs to be written to a file instead.  The log output should include timestamp, log level, and message.  Colorization should be used for console output.

