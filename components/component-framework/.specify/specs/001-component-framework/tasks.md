# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy — confirm they reflect intended use cases, not just observed test behavior
- [ ] Verify requirements match intended behavior (not just current behavior) — especially FR-033/FR-034 panic recovery semantics
- [ ] Remove implementation notes that don't belong in spec (or promote to requirements if they represent intentional contracts)
- [ ] Add any missing requirements (e.g., Drop ordering guarantees, actor thread naming, channel capacity validation error vs panic)
- [ ] Confirm non-functional requirements are measurable (NFR-008 mentions "10M idle iterations" — is this the intended threshold or an implementation detail?)
- [ ] Mark spec status as "Draft" or "Approved"

## Review Backfilled Plan

- [ ] Verify architecture diagram accurately reflects current module boundaries
- [ ] Confirm dependency list is complete and version-pinned correctly
- [ ] Validate data flow diagrams against actual code paths (especially binding resolution order)
- [ ] Review "Key Design Decisions" for accuracy — ensure rationale captures original intent, not post-hoc rationalization

## Macro Safety and Correctness

- [ ] Audit the post-construction unsafe initialization pattern in `define_component!` — document in spec whether this is an intentional API contract or implementation artifact
- [ ] Verify compile-fail tests cover all documented rejection cases (FR-002, FR-003, FR-006)
- [ ] Consider adding compile-fail tests for: duplicate interface names in provides list, receptacle name collisions with field names
- [ ] Review generated `Send + Sync` impl safety — document conditions under which these impls remain sound

## Channel Subsystem Improvements

- [ ] Evaluate whether the `force_closed` flag pattern should be elevated to a formal requirement or documented as internal mechanism
- [ ] Review progressive backoff thresholds (spin < 64, yield < 256, park 50us) — determine if these should be configurable or remain hardcoded
- [ ] Assess whether SPSC `sender_alive` flag race with `sender_count` can lead to spurious Closed returns under specific drop ordering
- [ ] Document the relationship between `ChannelState.sender_count` and `RingBuffer.sender_alive` — ensure spec captures the dual-flag closure protocol
- [ ] Consider adding a `capacity()` accessor to channels for runtime introspection

## Actor Model Improvements

- [ ] Clarify spec on actor idle behavior — FR-044 says "10M idle iterations" but code may use a different threshold; reconcile
- [ ] Document thread naming convention for actor threads (if any) — useful for profiling and debugging
- [ ] Consider adding actor metrics (messages processed, panics caught, idle time) as an optional feature
- [ ] Review ActorHandle Drop behavior — spec says "best-effort thread join" but implementation may differ
- [ ] Evaluate whether `signal_stop()` + later `deactivate()` has well-defined semantics when called in sequence

## NUMA Subsystem Improvements

- [ ] Verify NumaAllocator deallocation correctly calls munmap with the original length
- [ ] Test behavior when sysfs paths are absent (container environments) — confirm graceful fallback
- [ ] Consider adding NUMA-aware channel allocation (ring buffer memory bound to specific node)
- [ ] Document minimum kernel version requirements for NUMA sysfs paths used

## Registry and Binding

- [ ] Verify bind() error messages provide enough context for debugging type mismatches
- [ ] Consider adding a `try_bind()` variant that returns detailed mismatch information (expected TypeId vs actual)
- [ ] Review thread safety of ComponentRegistry under concurrent register + create — verify RwLock downgrade pattern is correct
- [ ] Evaluate whether `unregister()` should refuse if instances from that factory are still alive

## Documentation and Examples

- [ ] Ensure all 12 example programs compile and run without errors
- [ ] Verify `cargo doc --no-deps` produces zero warnings across all three crates
- [ ] Add architecture diagram to component-framework crate-level documentation
- [ ] Consider adding a "migration guide" for users of declare_interface!/declare_component! aliases

## Benchmark Validation

- [ ] Run all 13 benchmark suites and record baseline numbers for this hardware
- [ ] Verify benchmarks exercise the documented hot paths (not just constructor overhead)
- [ ] Add benchmark annotations documenting expected performance range (order of magnitude)
- [ ] Consider adding criterion comparison groups for built-in vs third-party channel backends
