# Research: Composable Server with Dynamic Component Loading

## R1: Dynamic Library Loading in Rust

**Decision**: Use `libloading` crate for runtime `.so` loading.

**Rationale**: `libloading` is the de-facto standard Rust crate for dynamic library loading. It provides safe wrappers around `dlopen`/`dlsym` on Linux, handles symbol lookup, and integrates cleanly with Rust's type system. The `create_component()` symbol returns a `ComponentRef` which is an `Arc<dyn IUnknown>` — a fat pointer that is ABI-compatible only within the same Rust toolchain build (acceptable per assumptions).

**Alternatives considered**:
- Raw `libc::dlopen`/`dlsym`: Requires more unsafe code, no ergonomic improvement.
- `dlopen2` crate: Less maintained than `libloading`, smaller community.
- `abi_stable` crate: Provides cross-toolchain FFI safety but adds significant complexity and would require all components to adopt its traits — incompatible with existing `ComponentRef`.

**Key implementation notes**:
- Symbol signature: `extern "Rust" fn create_component() -> ComponentRef`
- `Library` handles must be kept alive for the lifetime of loaded components
- Use `std::panic::catch_unwind` around `create_component()` calls to catch panics at the FFI boundary

## R2: JSON Configuration Schema Design

**Decision**: Flat JSON schema with top-level sections for variables, search_paths, server settings, components array, and bindings array.

**Rationale**: A flat structure with explicit bindings (rather than nested/hierarchical) maps directly to the component framework's receptacle model. Components are listed independently; bindings are a separate concern that references components by name. This mirrors the COM-style separation of instantiation from wiring.

**Alternatives considered**:
- TOML/YAML: JSON chosen per spec; no ambiguity in parsing.
- Hierarchical nesting (components contain their own bindings): Harder to validate for circular deps; bindings cross component boundaries.
- Code-as-config (Lua/Starlark): Overkill for integer variable substitution.

**Schema structure**:
```json
{
  "variables": { "num_ssd_devices": 4 },
  "search_paths": ["/opt/certus/lib", "./target/release"],
  "server": {
    "listen": "0.0.0.0:50051",
    "tls_cert": null,
    "tls_key": null,
    "device_pci": ["0000:41:00.0"],
    "memory_tier_size": "2G",
    "format": false,
    "poller_base_cpu": 2,
    "drive_count": null
  },
  "components": [
    {
      "name": "logger",
      "dylib": "liblogger.so",
      "instances": 1
    },
    {
      "name": "block-device",
      "dylib": "libblock_device_spdk_nvme.so",
      "instances": "$num_ssd_devices"
    }
  ],
  "bindings": [
    {
      "target": "dispatcher",
      "receptacle": "logger",
      "source": "logger"
    }
  ],
  "init_order": ["logger", "spdk-env", "gpu-services", "dispatch-map", "memory-tier", "dispatcher"]
}
```

## R3: Topological Sort for Initialization Order

**Decision**: Kahn's algorithm (BFS-based) for topological sort of the binding dependency graph.

**Rationale**: Kahn's algorithm naturally detects cycles (remaining nodes after exhaustion indicate a cycle). It produces a deterministic order when combined with lexicographic tie-breaking on component names. The binding graph is small (< 20 nodes typically), so performance is not a concern.

**Alternatives considered**:
- DFS-based topo sort: Equally valid; Kahn's chosen for easier cycle detection.
- No sorting (explicit order only): Rejected because it forces operators to manually maintain correct ordering.

**Implementation notes**:
- Each binding `{ target, receptacle, source }` creates an edge: target depends on source.
- After topo sort, if `init_order` is provided in config, validate that it is a valid topological ordering (respects all dependencies), then use it instead.
- If `init_order` is partial or invalid, report error.

## R4: Dylib Path Resolution Strategy

**Decision**: Ordered search path list with absolute path override.

**Rationale**: Mirrors the familiar `PATH`/`LD_LIBRARY_PATH` convention. The resolution order is:
1. If a component entry specifies an absolute path, use it directly.
2. Otherwise, iterate `search_paths` in order, checking for `<dir>/<dylib_filename>`.
3. Also check the `CERTUS_LIB_PATH` environment variable (colon-separated), prepended before JSON-defined search paths.

**Alternatives considered**:
- Absolute paths only: Inflexible for multi-environment deployments.
- Convention-only (single lib_dir): Too restrictive when components come from different build trees.

## R5: Fail-Fast Teardown Strategy

**Decision**: Maintain an ordered `Vec<ComponentRef>` of successfully initialized components; on failure, iterate in reverse calling shutdown (if the component supports it via a known interface pattern), then drop all `ComponentRef` handles, then unload libraries.

**Rationale**: Reverse-order teardown ensures that components which depend on others are shut down before their dependencies. Dropping `ComponentRef` (which wraps `Arc<dyn IUnknown>`) decrements reference counts, triggering component destructors. Libraries must outlive their components — `Library` handles are dropped last.

**Implementation notes**:
- Library handles stored in a `Vec<Library>` parallel to component instances
- Shutdown sequence: shutdown interfaces → drop ComponentRefs → drop Libraries
- Log each teardown step for diagnostics

## R6: Variable Substitution Approach

**Decision**: String-prefix marker (`$`) in JSON string values, replaced with the corresponding integer from the `variables` map during config parsing.

**Rationale**: Simple, no expression parser needed. A field value of `"$num_ssd_devices"` is replaced with the integer value of `variables["num_ssd_devices"]`. Non-string fields (already integers) are used directly. Validation ensures all `$`-prefixed references resolve to defined variables.

**Alternatives considered**:
- Template syntax like `{{var}}`: More complex to parse, not needed for integers.
- Typed variable references in a separate field: Adds schema complexity.

## R7: gRPC Service Layer Reuse

**Decision**: Copy the gRPC service implementation (`service.rs`, `proto/`, `build.rs`) from certus-server with minimal adaptation.

**Rationale**: The gRPC layer is decoupled from component instantiation — it only requires an `Arc<dyn IDispatcher + Send + Sync>` which it obtains after the component stack is assembled. The service handlers, IPC cache logic, and proto definitions are unchanged. Only the "obtain dispatcher" path changes (from hardcoded construction to config-driven assembly).

**Alternatives considered**:
- Shared library for gRPC service: Over-engineering for one binary; adds build complexity.
- Dynamic loading of the service layer too: Spec explicitly excludes this.
