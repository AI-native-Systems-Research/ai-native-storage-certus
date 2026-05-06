# Drift Resolution Proposals

Generated: 2026-05-05
Based on: drift-report from 2026-05-05

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 0 |
| Align (Spec -> Code) | 1 |
| Human Decision | 1 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: 002-spdk-env-vfio-init/FR-007

**Direction**: HUMAN_DECISION

**Current State**:
- Spec says: "MUST use framework's logging actor via receptacle. init() MUST fail if logger not connected."
- Code does: "No logger receptacle defined. All diagnostics via eprintln!."

**Options**:
- A) **ALIGN (strict)**: Add mandatory logger receptacle. init() fails without it. Matches spec exactly.
- B) **ALIGN (relaxed)**: Add optional logger receptacle. Use it if connected, fall back to eprintln! otherwise. Update spec to remove "MUST fail" requirement.
- C) **BACKFILL**: Remove logger requirement from spec entirely. SPDK/DPDK outputs to stderr regardless — a framework logger only captures Rust-side messages, not the C library output.

**Recommendation**: Option B (relaxed). Add receptacle but make it optional. Rationale:
1. SPDK/DPDK C libraries will always output to stderr — we can't redirect that through the framework logger
2. Making it optional allows standalone use (testing without full framework)
3. When connected, Rust-side diagnostics (pre-flight checks, init status) route through the framework logger for consistent log aggregation
4. This matches the pattern used by other Certus components (extent-manager uses `logger.get()` with graceful handling)

**Confidence**: HIGH

---

### Proposal 2: 002-spdk-env-vfio-init/FR-010

**Direction**: ALIGN (Spec -> Code)

**Current State**:
- Spec says: "Test example must instantiate component, wire logger, call init(), query ISPDKEnv"
- Code does: "Example instantiates and calls init() but has no logger to wire"

**Proposed Resolution**: After Proposal 1 is resolved (logger receptacle added), update the example to:
1. Create a `LoggerComponentV1` (ConsoleLogger)
2. Wire it to the SPDKEnvComponent's logger receptacle
3. Proceed with init() and device enumeration

**Blocked by**: Proposal 1

**Confidence**: HIGH
