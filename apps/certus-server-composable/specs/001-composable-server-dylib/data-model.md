# Data Model: Composable Server with Dynamic Component Loading

## Entities

### Configuration (top-level)

The root JSON document parsed at startup.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| variables | Map<String, i64> | No | Named integer values for substitution |
| search_paths | Vec<String> | No | Ordered directories for dylib resolution |
| server | ServerConfig | No | Server-level settings (CLI overrides these) |
| components | Vec<ComponentSpec> | Yes | Component declarations |
| bindings | Vec<BindingRule> | Yes | Wiring rules between components |
| init_order | Vec<String> | No | Explicit initialization order override |

### ServerConfig

Optional server-level parameters. All fields can be overridden by CLI arguments.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| listen | String | No | gRPC listen address (default: "0.0.0.0:50051") |
| tls_cert | String | No | Path to TLS certificate file |
| tls_key | String | No | Path to TLS private key file |
| device_pci | Vec<String> | No | PCI addresses of NVMe devices |
| drive_count | u64 | No | Auto-select first N NVMe drives |
| memory_tier_size | String | No | Pool size (e.g., "2G", "256M") |
| format | bool | No | Format extent managers on startup |
| poller_base_cpu | u64 | No | Base CPU core for NVMe poller pinning |

### ComponentSpec

Declares a component type to be loaded and instantiated.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| name | String | Yes | Unique identifier for this component (used in bindings) |
| dylib | String | Yes | Filename of the shared library (e.g., "liblogger.so") |
| path | String | No | Absolute path override (bypasses search_paths) |
| instances | InstanceCount | No | Number of instances to create (default: 1) |

**InstanceCount**: Either an integer literal or a string `"$variable_name"` referencing a variable.

**Invariants**:
- `name` must be unique across all component entries
- `dylib` must name a valid `.so` file resolvable via search_paths or path
- When `instances` > 1, individual instances are named `{name}[0]`, `{name}[1]`, etc.

### BindingRule

Connects one component's provided interface to another component's receptacle.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| target | String | Yes | Name of the component that has the receptacle |
| receptacle | String | Yes | Name of the receptacle slot on the target |
| source | String | Yes | Name of the component that provides the interface |

**Invariants**:
- `target` and `source` must reference defined component names
- `receptacle` must match a receptacle name reported by the target's `IUnknown::receptacles()`
- The source component must provide the interface type required by the receptacle
- Binding target/source pairs must not form cycles

### LoadedComponent (runtime)

In-memory representation of a successfully loaded and instantiated component.

| Field | Type | Description |
|-------|------|-------------|
| name | String | Instance name (e.g., "block-device[0]") |
| component_ref | ComponentRef | Arc<dyn IUnknown> handle |
| library | Arc<Library> | Loaded library handle (must outlive component_ref) |
| init_order_index | usize | Position in initialization sequence |

## State Transitions

### Server Lifecycle

```
[Config Loaded] → [Validated] → [Dylibs Resolved] → [Components Instantiated] → [Bound] → [Running] → [Shutting Down] → [Terminated]
```

Transitions:
- Config Loaded → Validated: Parse JSON, substitute variables, check structural validity
- Validated → Dylibs Resolved: Resolve all dylib paths, verify accessibility
- Dylibs Resolved → Components Instantiated: Load dylibs, call create_component() in topo order
- Components Instantiated → Bound: Execute all binding rules via connect_receptacle_raw
- Bound → Running: gRPC server starts accepting connections
- Running → Shutting Down: SIGTERM/SIGINT received
- Shutting Down → Terminated: Reverse-order teardown complete

**Fail-fast on error**: Any transition failure between Validated and Bound triggers immediate reverse teardown of all completed steps and exits.

## Relationships

```
Configuration 1──* ComponentSpec
Configuration 1──* BindingRule
Configuration 1──1 ServerConfig (optional)
ComponentSpec 1──* LoadedComponent (via instances field)
BindingRule *──1 ComponentSpec (target)
BindingRule *──1 ComponentSpec (source)
```
