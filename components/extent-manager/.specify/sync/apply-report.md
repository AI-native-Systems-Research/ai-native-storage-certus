# Sync Apply Report: extent-manager

**Date**: 2026-05-21
**Spec**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`
**Drift report**: `components/extent-manager/.specify/sync/drift-report.json`

## Summary

| Metric | Count |
|--------|-------|
| Proposals applied | 4 |
| Proposals deferred | 0 |
| Files modified | 1 |

## Applied Changes

### 1. BACKFILL-FR-001: ExtentManagerV2 -> ExtentManager

**File**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`

Replaced all 3 occurrences of `ExtentManagerV2` with `ExtentManager`:
- FR-001 requirement text (line 259)
- User Story 1 independent test description (line 25)
- Assumptions section DMA allocator paragraph (line 506)

**Result**: FR-001 now reads "The component MUST be named ExtentManager" matching the actual `define_component!` invocation in code.

### 2. BACKFILL-FORMAT-VERSION: Version 4 -> 5

**File**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`

Updated format version references:
- Key Entities / Superblock: magic `0x4345_5254_5553_5634` ("CERTUSV4") -> `0x4345_5254_5553_5635` ("CERTUSV5"), version 4 -> 5
- On-Disk Format / Superblock table row 0: magic value and label updated
- On-Disk Format / Superblock table row 1: version (4) -> version (5)

**Result**: Spec on-disk format documentation now matches `FORMAT_VERSION = 5` as defined in `src/superblock.rs`.

### 3. SOFTEN-SC-005: Scalability claim language

**File**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`

Softened SC-005 wording from "The component supports approximately 100 million extents" to "The component is designed to support approximately 100 million extents ... (architectural target scale, not yet verified by benchmark)". This accurately reflects that the claim is an architectural goal based on data structure analysis, not a tested guarantee.

**Result**: SC-005 no longer implies verified performance at 100M extent scale.

### 4. BACKFILL-UNSPECCED-FEATURES: Add FR-031 through FR-034

**File**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`

Added four new functional requirements for features present in the implementation but previously unspecified:

- **FR-031** (`used_bytes()`): IExtentManager method returning total bytes currently allocated.
- **FR-032** (`capacity_bytes()`): IExtentManager method returning total data device capacity in bytes.
- **FR-033** (WriteHandle location): WriteHandle type defined in the interfaces crate for decoupled dependency.
- **FR-034** (Fault injection): Test-only feature gate for simulating I/O failures in crash-safety tests.

**Result**: All previously unspecced features now have formal requirements in the spec.

## Verification

All deferred items from the previous apply report have been resolved:
- SC-005 scalability claim: softened to "designed to support" (applied as item 3)
- Unspecced features: added as FR-031 through FR-034 (applied as item 4)

The spec is now internally consistent with the codebase for all known items.
