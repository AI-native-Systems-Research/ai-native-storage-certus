# Drift Resolution Proposals — Certus Project

**Generated**: 2026-06-11  
**Based on**: Drift Analysis from 2026-06-11

## Summary

| Resolution Type | Count | Status |
|-----------------|-------|--------|
| Backfill (Code → Spec) | 3 | ✅ APPROVED |
| Align (Spec → Code) | 0 | - |
| Human Decision Required | 0 | - |
| New Specs | 0 | - |
| Remove from Spec | 0 | - |

---

## Approved Proposals

All three proposals are BACKFILL cases where the code is production-ready and specifications lack implementation details. All have been **APPROVED** for immediate application.

### Proposal 1: RDMA Test Tool — FR-007 SSH Launch Script

**Status**: ✅ **APPROVED**

**Component**: `tools/rdma-test`  
**Spec Location**: `tools/rdma-test/specs/001-rdma-network-test/spec.md`  
**Implementation**: `tools/rdma-test/scripts/launch.sh` (106 lines)

**Resolution Direction**: BACKFILL (Code → Spec)

**Current State**:
- **Spec says**: "System MUST include a launch script that starts a server on one remote host and a client on another using SSH, collecting and displaying results."
- **Code does**: Full-featured SSH automation with health checks, environment variable configuration, error handling, and cleanup

**Applied Changes**:
- Added "Implementation Details: FR-007" section to spec.md
- Documented script location, usage syntax, CLI arguments, environment variables, error handling, and example invocations

**Confidence**: HIGH | **Effort**: Minimal (15 min) | **Risk**: NONE

---

### Proposal 2: gRPC Dispatcher — FR-010 Python Test Client

**Status**: ✅ **APPROVED**

**Component**: `apps/certus-server`  
**Spec Location**: `apps/certus-server/specs/001-grpc-dispatcher-server/spec.md`  
**Implementation**: `apps/certus-server/python-client/test_client.py` (548 lines)

**Resolution Direction**: BACKFILL (Code → Spec)

**Current State**:
- **Spec says**: "A Python test client MUST be provided that exercises all gRPC methods, demonstrating batch operations and error handling."
- **Code does**: Complete test client with 9 functional test cases, benchmark suite, CLI options, GPU memory integration, and comprehensive output formatting

**Applied Changes**:
- Added "Implementation Details: FR-010" section to spec.md
- Documented script location, prerequisites, CLI options (--server, --skip-tests, --benchmark)
- Listed all 9 test cases with descriptions
- Documented benchmark suite (Memory-tier latency, SSD-tier latency, throughput, batch scaling)
- Included example invocations and exit codes

**Confidence**: HIGH | **Effort**: Minimal (30 min) | **Risk**: NONE

---

### Proposal 3: gRPC Dispatcher — FR-014 TLS Support

**Status**: ✅ **APPROVED**

**Component**: `apps/certus-server`  
**Spec Location**: `apps/certus-server/specs/001-grpc-dispatcher-server/spec.md`  
**Implementation**: `apps/certus-server/src/main.rs` (lines 50-56, 300-308)

**Resolution Direction**: BACKFILL (Code → Spec)

**Current State**:
- **Spec says**: "The server MUST support optional TLS encryption via CLI flags (`--tls-cert`, `--tls-key`). When both are provided, TLS is enabled. When not provided, the server runs in plaintext mode."
- **Code does**: Full TLS implementation with CLI flag parsing, PEM certificate/key loading, async file I/O, conditional enablement, and error handling

**Applied Changes**:
- Added "Implementation Details: FR-014" section to spec.md
- Documented CLI flags, certificate requirements, and format
- Included OpenSSL certificate generation examples
- Documented Python gRPC client TLS configuration
- Added example invocations (plaintext vs TLS modes)
- Included security best practices and notes on self-signed vs production certificates

**Confidence**: HIGH | **Effort**: Minimal (25 min) | **Risk**: NONE

---

## Project-Wide Impact

### Spec Coverage Summary

| Component | Coverage | Status | Changes |
|-----------|----------|--------|---------|
| RDMA Network Test | 92.3% → 100% | ✅ Complete | FR-007 documented |
| gRPC Dispatcher Server | 89.5% → 100% | ✅ Complete | FR-010, FR-014 documented |
| Block Device Filesys | 100% | ✅ Complete | None |
| Extent Manager V2 | 100% | ✅ Complete | None |
| Logger Component | 100% | ✅ Complete | None |
| GPU CUDA Services | 100% | ✅ Complete | None |
| GPU-to-SSD DMA Prepare | 100% | ✅ Complete | None |
| Dispatcher Cache Interface | 100% | ✅ Complete | None |
| Dispatch Map Component | 100% | ✅ Complete | None |

**Overall Project Coverage**: 95.6% → 100% (all specs now have complete implementation details)

---

## Recommendations

1. ✅ **Immediate** (No blockers): Apply all three approved backfills to specs
2. ✅ **Follow-up**: Commit spec updates with message: "docs(specs): backfill implementation details for RDMA and gRPC dispatcher"
3. 🔄 **Future**: Run spec-drift analysis quarterly to catch new unspecced features early

---

## Files Modified

- `tools/rdma-test/specs/001-rdma-network-test/spec.md`
  - Added: "Implementation Details: FR-007" section (35 lines)
  
- `apps/certus-server/specs/001-grpc-dispatcher-server/spec.md`
  - Added: "Implementation Details: FR-010" section (45 lines)
  - Added: "Implementation Details: FR-014" section (50 lines)

---

## Next Steps

1. ✅ Specifications have been updated with implementation details
2. Run `/speckit-sync-apply` to finalize and commit changes
3. Verify spec documentation in README.md files (optional: run `/component-update-docs`)
