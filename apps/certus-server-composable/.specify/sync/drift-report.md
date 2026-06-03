# Spec Drift Report

Generated: 2026-06-03T23:30:00Z
Project: certus-server-composable

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 14 |
| Aligned | 10 (71%) |
| Drifted | 3 (21%) |
| Not Implemented | 1 (7%) |
| Unspecced Code | 2 |

## Detailed Findings

### Spec: 001-composable-server-dylib - Composable Server with Dynamic Component Loading

#### Aligned

- FR-002: JSON config parsing with component types, instance counts, dylib names, bindings → `src/config.rs`
- FR-003: Variable definitions as integer literals with `$` substitution → `src/config.rs:InstanceCount::Variable`
- FR-004: Bind components via `connect_receptacle_raw` → `src/binder.rs`
- FR-005: Validate config at startup (dylib existence, bindings, variables, cycles) → `src/config.rs:validate_config` + `src/resolver.rs:resolve_all_dylibs` + `src/topology.rs`
- FR-007: Mandatory `--config` CLI parameter → `src/main.rs:Cli` (config field is required)
- FR-008: Clear actionable error messages → `src/config.rs:ConfigError`, `src/runtime.rs` error formatting
- FR-011: Topological sort with optional `init_order` override → `src/topology.rs`
- FR-012: Search path list + `CERTUS_LIB_PATH` env var → `src/resolver.rs`
- FR-013: Fail-fast with reverse teardown → `src/runtime.rs:teardown_reverse`
- FR-014: Server parameters in JSON config with CLI precedence → `src/config.rs:merge_cli_overrides`

#### Drifted

- **FR-001**: Spec says "using the `create_component()` entry point convention" but code uses `create_component_<crate_name>()` (unique per-crate symbols to avoid linker conflicts)
  - Location: `src/loader.rs:45-54`
  - Severity: minor (intentional design refinement, spec needs updating)

- **FR-006**: Spec says "expose the same gRPC interface" — service.rs has stub implementations (`todo!`-style empty responses) rather than wiring through to IDispatcher for populate/lookup
  - Location: `src/service.rs:91-120`
  - Severity: moderate (gRPC methods return empty results instead of dispatching to the loaded IDispatcher; works because dispatcher handles it internally via the static certus-server path)

- **FR-010**: Spec says "verify interface compatibility at bind time" — the current name-based binding uses unsafe transmute without runtime type verification
  - Location: `component-macros/src/define_component.rs:258-280`
  - Severity: minor (name-matching provides semantic verification; structural compatibility guaranteed by same-compiler ABI)

#### Not Implemented

- **FR-009**: Graceful shutdown invoking shutdown on components — `ComponentStack::shutdown()` only prints messages, doesn't call component-specific shutdown methods (e.g., `IDispatcher::shutdown()`)
  - Note: Partial — the Drop impl reverses component order, and the gRPC server does graceful SIGTERM handling

### Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `query_by_name` unsafe cross-dylib interface query | `src/runtime.rs:24-38` | 15 | Update spec clarifications |
| `parse_memory_size` helper | `src/runtime.rs:203-216` | 14 | N/A (utility) |

## Inter-Spec Conflicts

- Spec says `create_component()` (generic); implementation uses `create_component_<name>()` (unique). Spec section "Architectural Changes Required" references the old convention.
- Spec describes block-device and extent-manager as external dylib components but current working config keeps them internal to the dispatcher dylib (SPDK singleton constraint documented but not reflected in requirements).

## Recommendations

1. **Update FR-001** to reflect the actual `create_component_<crate_name>()` convention (symbol derived from dylib filename).
2. **Update spec** to document that SPDK-dependent components (block-device, extent-manager, spdk-env) must share a dylib with the dispatcher due to SPDK process-global state.
3. **Wire service.rs gRPC handlers** through to the loaded IDispatcher component (currently stubs — works because dispatcher handles operations internally, but the gRPC layer should delegate properly).
4. **Implement proper shutdown** — call `IDispatcher::shutdown()` during teardown.
5. **Remove or update "Architectural Changes Required" section** — the multi-receptacle/add_data_drive approach was implemented but is blocked by SPDK singleton issue for external drives.
