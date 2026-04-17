# Alignment Tasks

Generated: 2026-04-17
Source: drift resolution proposals (2026-04-17)

## Task 1: Wire Logger to ExtentManager in Benchmark App

**Proposal**: 9 (002-extent-benchmark/FR-009)
**Spec Requirement**: FR-009 — "Wire full component stack including Logger to all components."
**Current Code**: Logger binding to extent manager is not visible in `apps/extent-benchmark/src/main.rs`.
**Required Change**: Bind the logger component to the extent manager's `ILogger` receptacle in the benchmark app's component wiring.
**Files to Modify**: `apps/extent-benchmark/src/main.rs`
**Estimated Effort**: small

### Acceptance Criteria

- [ ] Logger component is bound to the extent manager's `ILogger` receptacle
- [ ] Logger is bound to all components that declare an `ILogger` receptacle
- [ ] Benchmark app compiles and runs with logger wired
- [ ] Extent manager log output is visible during benchmark execution
