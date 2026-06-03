# Feature Specification: Composable Server with Dynamic Component Loading

**Feature Branch**: `001-composable-server-dylib`

**Created**: 2026-06-03

**Status**: Draft

**Input**: User description: "Write a new version of apps/certus-server, called apps/certus-server-composable, that replicates the functionality of certus-server but only uses dylib component versions that are loaded at runtime. certus-server-composable should include a JSON specification that defines what component versions are used, and how they are instantiated and bound to other components. Variables such as the number of SSD devices, may be included in the specification, e.g., to derive number of instances of block-device-spdk-nvme to create. Ultimately, certus-server-composable will allow different component configurations to be deployed depending on particular deployment requirements and restrictions, such as the number of available SSD."

## Clarifications

### Session 2026-06-03

- Q: How should component initialization order be determined? → A: Hybrid — automatic topological sort derived from binding dependencies, with optional explicit `init_order` override field in config.
- Q: How should dylib files be located at runtime? → A: Search path list (configurable in JSON or env var) plus optional per-component absolute path override. Each component entry MUST explicitly name its dylib file.
- Q: What operations are supported on configuration variables? → A: Integer-only direct substitution — variables hold literal values, no arithmetic expressions.
- Q: How should loaded dylib versions be verified? → A: No runtime version check — trust is established through strict explicit path specification in the configuration.
- Q: What happens when a component fails to load during startup? → A: Fail-fast — abort startup, teardown all already-initialized components in reverse order, exit with error.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Load and Run Certus via JSON Configuration (Priority: P1)

An operator deploys certus-server-composable with a JSON configuration file that specifies which component dylibs to load, how many instances to create, and how they bind together. The server starts, loads all specified components dynamically, wires them according to the binding rules in the configuration, and exposes the same gRPC interface as the existing certus-server.

**Why this priority**: This is the core value proposition — replacing compile-time component wiring with runtime configuration-driven assembly while preserving identical external behavior.

**Independent Test**: Can be fully tested by providing a valid JSON configuration file and verifying that the server starts, loads all specified components, and responds to gRPC requests identically to the static certus-server.

**Acceptance Scenarios**:

1. **Given** a valid JSON configuration specifying logger, spdk-env, gpu-services, dispatch-map, memory-tier, and dispatcher components with their bindings and explicit dylib paths, **When** certus-server-composable starts with this configuration, **Then** all components are loaded from their respective dylib files (resolved via search path or absolute path), instantiated in topologically-sorted order (or explicit `init_order` if provided), bound together, and the gRPC endpoint becomes available.
2. **Given** a running certus-server-composable instance, **When** a client sends populate/lookup/check/remove/touch/clear_memory_tier requests, **Then** the responses are functionally identical to those from the static certus-server.
3. **Given** a JSON configuration referencing a dylib that does not exist at the specified path or in the search path, **When** certus-server-composable starts, **Then** it reports a clear error identifying the missing library and exits with a non-zero status code.
4. **Given** a component fails to initialize after other components have already started, **When** the failure is detected, **Then** the system tears down all previously-initialized components in reverse order and exits with a non-zero status code and error message.

---

### User Story 2 - Variable-Driven Instance Count (Priority: P2)

An operator uses variables in the JSON configuration to parameterize the deployment — for example, specifying `num_ssd_devices: 4` causes the system to instantiate four block-device-spdk-nvme component instances and bind them appropriately. Variables hold integer literal values only (no arithmetic expressions).

**Why this priority**: Variables enable a single configuration template to adapt to diverse hardware environments without manual editing of component instance lists.

**Independent Test**: Can be tested by providing a configuration with a variable for device count and verifying that the correct number of component instances are created and properly bound.

**Acceptance Scenarios**:

1. **Given** a JSON configuration with `"variables": {"num_ssd_devices": 4}` and a component section that references this variable to determine instance count, **When** the server starts, **Then** exactly 4 instances of the block-device-spdk-nvme component are created.
2. **Given** a JSON configuration with `"variables": {"num_ssd_devices": 0}`, **When** the server starts, **Then** it reports a validation error that at least one storage device is required.
3. **Given** a variable referenced in the configuration that is not defined in the variables section, **When** the server starts, **Then** it reports a clear error identifying the undefined variable and exits.

---

### User Story 3 - Deployment-Specific Configurations (Priority: P3)

An operator maintains multiple JSON configuration files for different deployment scenarios (e.g., single-SSD development, multi-SSD production, GPU-less testing) and selects the appropriate one at launch time.

**Why this priority**: This enables operational flexibility — the same binary supports heterogeneous environments without recompilation.

**Independent Test**: Can be tested by providing different configuration files and verifying that each produces the expected component topology.

**Acceptance Scenarios**:

1. **Given** a "dev" configuration with 1 SSD device and reduced memory-tier size, **When** the server starts with this configuration, **Then** it runs with a minimal component set appropriate for development.
2. **Given** a "production" configuration with 8 SSD devices, full memory-tier, and GPU services, **When** the server starts, **Then** all components are instantiated at production scale.
3. **Given** the mandatory `--config` parameter pointing to a configuration file, **When** the server starts, **Then** it uses that configuration. If `--config` is omitted, the server MUST exit with a usage error.

---

### Edge Cases

- How does the system handle a circular binding dependency in the configuration? (Detected during topological sort — reported as validation error.)
- What happens when two components require binding to the same receptacle name but different interface types? (Binding validation catches type mismatch at bind time.)
- What happens if a component's `create_component()` panics during loading? (Panic is caught at the FFI boundary; treated as a load failure triggering fail-fast teardown.)
- How does the system behave when a dylib is compiled with a different Rust toolchain version? (Undefined behavior — operator responsibility; documented in assumptions.)

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST load component implementations from shared libraries (`.so` files) at runtime using per-crate factory symbols. The symbol name is derived from the dylib filename: `lib<name>.so` → `create_component_<name>()`. This avoids linker symbol conflicts when multiple component dylibs are built in a single workspace.
- **FR-002**: System MUST parse a JSON configuration file that declares component types, instance counts, explicit dylib file names, and binding relationships.
- **FR-003**: System MUST support variable definitions in the configuration as integer literal values that can be substituted directly into instance count fields.
- **FR-004**: System MUST bind components together by connecting interface providers to receptacles as specified in the configuration.
- **FR-005**: System MUST validate the configuration at startup: verify all referenced dylibs exist and are accessible (via search path resolution or absolute path), all bindings reference valid receptacles and interfaces, all variables are defined, and no circular dependencies exist. All dylib file locations MUST be resolved and verified as readable BEFORE any component is instantiated — if any dylib is missing or inaccessible, the system MUST report all missing libraries and exit without loading any components.
- **FR-006**: System MUST expose the same gRPC interface (certus.dispatcher.v1) as the existing certus-server. The gRPC service delegates to the dynamically-loaded dispatcher component which handles all cache operations internally.
- **FR-007**: System MUST accept the JSON configuration file path as a mandatory command-line parameter (e.g., `--config <path>`). The server MUST NOT start without this parameter — there is no default or auto-discovered configuration file. Additional CLI arguments for listen address, TLS configuration, and format flag are supported as with certus-server.
- **FR-014**: System MUST support optional specification of `--device-pci` addresses and other certus-server parameters (listen address, TLS cert/key, memory-tier size, format flag, poller-base-cpu, drive-count) within the JSON configuration file. Command-line arguments MUST take precedence over values defined in the JSON configuration when both are provided.
- **FR-008**: System MUST report clear, actionable errors when configuration validation fails, identifying the specific misconfiguration.
- **FR-009**: System MUST perform graceful shutdown by dropping all component references in reverse initialization order. Component destructors handle cleanup via Rust's Drop semantics.
- **FR-010**: System MUST verify interface compatibility at bind time by name-based matching — the provider must expose an interface with the same name as the receptacle requires. This supports cross-dylib binding where TypeIds differ between compilation units.
- **FR-011**: System MUST determine component initialization order by topological sort of binding dependencies, with an optional explicit `init_order` field that overrides the derived order.
- **FR-012**: System MUST resolve dylib paths using a configurable search path list (defined in JSON config or environment variable), while also supporting absolute paths per component entry.
- **FR-013**: System MUST abort startup on any component load or initialization failure, tearing down all already-initialized components in reverse order (fail-fast).

### Key Entities

- **Component Instance**: A runtime instance created by calling `create_component()` from a loaded dylib, identified by a unique name in the configuration.
- **Component Specification**: A JSON object declaring the explicit dylib filename, instance count (possibly variable-derived), and initialization parameters.
- **Binding Rule**: A JSON object specifying which component instance's interface connects to which other component instance's receptacle.
- **Variable**: A named integer value defined in the configuration that can be directly substituted into instance count fields.
- **Configuration**: The top-level JSON document containing variables, search paths, component specifications, bindings, optional init_order, and server settings.
- **Search Path**: An ordered list of directories where the system looks for dylib files, configurable via the JSON config or an environment variable.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Operator can deploy and run the full Certus component stack using only a JSON configuration file and pre-built dylib files, with zero code changes required for different hardware configurations.
- **SC-002**: gRPC clients cannot distinguish between certus-server and certus-server-composable — all API operations produce identical results.
- **SC-003**: Configuration validation catches 100% of structural errors (missing dylibs, undefined variables, invalid bindings, circular dependencies) before any component is loaded.
- **SC-004**: System starts within 5 seconds of the existing certus-server startup time (excluding SPDK initialization which is hardware-dependent).
- **SC-005**: Adding support for a new component type requires only building a dylib with `create_component()` and adding a JSON configuration entry — no changes to certus-server-composable source code.

## Architectural Constraints

### SPDK Singleton and Dylib Boundaries

SPDK uses process-global state (hugepage allocations, DPDK EAL, NVMe driver registration). This state cannot be shared across separate `.so` files because each dylib statically links its own copy of SPDK. Consequently:

- **block-device-spdk-nvme**, **extent-manager**, and **spdk-env** MUST reside in the same dylib as the **dispatcher** (they share SPDK process state)
- The dispatcher internally creates and manages block-device and extent-manager instances
- The JSON config controls how many drives to use via `server.drive_count` or `server.device_pci`
- Non-SPDK components (logger, gpu-services, dispatch-map, memory-tier) are loaded as separate dylibs

### Implemented Dispatcher Extensions (for future external drives)

The framework supports external drive injection via `IDispatcher::add_data_drive()` and `MultiReceptacle<T>`. This path is architecturally complete but blocked by the SPDK singleton constraint. It will become usable when either:
- SPDK is rebuilt as shared libraries (`.so`), or
- A shared `libspdk_bundle.so` is created containing all SPDK-dependent components

## Assumptions

- All components export a `create_component_<crate_name>() -> ComponentRef` function (behind `#[cfg(feature = "dylib")]`) suitable for dynamic loading.
- Component dylibs are built with the same Rust toolchain and ABI as certus-server-composable (no cross-ABI stability guarantees). Operator is responsible for ensuring ABI compatibility.
- The JSON configuration format is specific to this project and does not need to conform to any external standard.
- The gRPC service implementation (proto definitions, service handlers) remains in certus-server-composable source code and is not dynamically loaded.
- Variables are integer-only with direct substitution (no arithmetic, no expression language).
- Component dylibs are located on the local filesystem (no network/registry-based loading).
- No runtime version checking of loaded dylibs — correctness is ensured by the operator specifying the exact dylib file path.
