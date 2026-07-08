# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior
- [ ] Remove implementation notes that don't belong in spec
- [ ] Add any missing requirements
- [ ] Mark spec status as "Draft" or "Approved"

## Consolidate Duplicate Type Definitions

- [ ] Remove local `PciAddress`, `PciId`, `VfioDevice` definitions from `src/device.rs` (they duplicate `interfaces::spdk_types`)
- [ ] Re-export types from `interfaces` crate directly (as already done for `DmaBuffer` and `SpdkEnvError`)
- [ ] Update `src/lib.rs` pub-use to re-export from `interfaces` instead of `device` module
- [ ] Verify `src/env.rs` enumerate callback still compiles against the `interfaces` type definitions
- [ ] Run `cargo test -p spdk-env` to confirm no regressions
- [ ] Run `cargo build --workspace` to confirm downstream consumers unaffected

## Improve Error Handling in Enumeration

- [ ] Return `SpdkEnvError::DeviceProbeFailed` when `spdk_pci_enumerate` returns non-zero (currently only logs a warning)
- [ ] Return `SpdkEnvError::DeviceProbeFailed` when `spdk_pci_get_driver("nvme")` returns NULL (currently only logs a warning)
- [ ] Add unit test verifying that enumeration failures propagate correctly

## Add Integration Test Harness

- [ ] Create `tests/integration.rs` with `#[cfg(feature = "integration")]` gate
- [ ] Add test: singleton enforcement (second init returns `AlreadyInitialized`)
- [ ] Add test: full lifecycle init -> devices -> fini -> re-init
- [ ] Add test: Drop calls fini automatically (verify `is_initialized()` after drop)
- [ ] Document how to run integration tests in CLAUDE.md (requires VFIO hardware)

## Add Configuration Support

- [ ] Define a configuration struct for SPDK env options (core mask, hugepage dir, memory channels)
- [ ] Expose configuration through a builder pattern or receptacle binding
- [ ] Update `init_spdk_env()` to apply configuration to `spdk_env_opts`
- [ ] Add unit tests for configuration validation
- [ ] Update spec.md with new FR for configuration

## Improve Documentation

- [ ] Add `#[doc = ...]` module-level documentation to `env.rs` explaining the initialization sequence
- [ ] Add `// SAFETY:` comments to the `enum_cb` extern function (currently has `unsafe` blocks without justification inside the callback)
- [ ] Ensure `cargo doc -p spdk-env --no-deps` produces zero warnings
- [ ] Add architecture diagram to component CLAUDE.md

## Add Clippy and Formatting Compliance

- [ ] Run `cargo clippy -p spdk-env -- -D warnings` and fix any findings
- [ ] Run `cargo fmt --check -p spdk-env` and fix any formatting issues
- [ ] Ensure the `#[allow(unused_unsafe)]` is not needed in enum_cb (nested unsafe blocks)

## Investigate Loom-Based Concurrency Testing

- [ ] Evaluate whether the `AtomicBool` singleton guard can be tested with Loom
- [ ] If feasible, add Loom test for concurrent `init()` calls racing for the singleton
- [ ] If feasible, add Loom test for `init()` + `fini()` interleaving across threads
- [ ] Document decision in plan.md Future Considerations
