# Spec Drift Report

Generated: 2026-05-05
Project: SPDK Env
Specs: 001-spdk-vfio-env, 002-spdk-env-vfio-init

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 15 |
| Aligned | 11 (73%) |
| Drifted | 2 (13%) |
| Not Implemented | 2 (13%) |
| Unspecced Code | 0 |

## Detailed Findings

### Spec: 001-spdk-vfio-env

Template spec with placeholder content — no concrete requirements to verify.

### Spec: 002-spdk-env-vfio-init - SPDK/DPDK Environment Component with VFIO Device Iteration

#### Aligned

- FR-001: Rust lib crate using define_component! and define_interface! macros
- FR-002: ISPDKEnv interface with init(), devices(), device_count(), is_initialized()
- FR-003: SPDK/DPDK init happens in init(), not during construction
- FR-004: Verifies /dev/vfio and vfio-pci kernel module before initialization
- FR-005: Checks R/W permissions on /dev/vfio, /dev/vfio/vfio, IOMMU groups with specific error messages
- FR-006: Enumerates SPDK-supported devices bound to VFIO with PCI BDF address
- FR-008: Operates without root when /dev/vfio has appropriate user-level access
- FR-009: Returns empty device list (not error) when no devices are bound
- FR-011: Plain procedural component — no threads, no message queues
- FR-012: Drop impl calls spdk_env_fini() and clears singleton flag
- FR-013: Checks hugepage availability, reports clear error if not configured
- FR-014: Singleton semantics via SPDK_ENV_ACTIVE AtomicBool — second init() returns error
- FR-015: Device enumeration returns non-zero from callback to avoid claiming devices

#### Drifted

- FR-007: Spec requires framework's logging actor via receptacle for all diagnostic output, and init() MUST fail if logger not connected. Code uses eprintln! for all diagnostics with NO logger receptacle defined.
  - Location: src/lib.rs (define_component! call has no receptacles)
  - Severity: high (violates framework logging convention)

- FR-010: Spec requires test example that wires logger before calling init(). Example exists at examples/spdk-env-example.rs but cannot wire a logger because no logger receptacle exists.
  - Location: examples/spdk-env-example.rs
  - Severity: low (blocked by FR-007)

#### Not Implemented

- FR-007 (logger receptacle): No logger receptacle defined in the component
- FR-010 (logger wiring in example): Cannot wire what doesn't exist

### Success Criteria

- SC-001: Aligned (device enumeration discovers VFIO-bound devices)
- SC-002: Aligned (pre-flight checks report specific issues)
- SC-003: Partially aligned (example runs as non-root but doesn't use framework logger)
- SC-004: Aligned (all operations synchronous, no threads)
- SC-005: Drifted (uses eprintln! instead of framework logging)

## Recommendations

1. **FR-007 (high)**: Add an optional logger receptacle. Use framework logger if connected, fall back to eprintln! for cases where SPDK is used standalone. Remove the "MUST fail if not connected" requirement since SPDK/DPDK itself outputs to stderr regardless.
2. **FR-010**: After logger receptacle is added, update example to wire ConsoleLogger.
