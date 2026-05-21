# Sync Proposals: extent-manager

**Spec**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`
**Generated**: 2026-05-21

---

## Applied Proposals

### BACKFILL-FR-001: Rename ExtentManagerV2 to ExtentManager in spec

- **Type**: Backfill (spec trails code)
- **Status**: Applied
- **Requirement**: FR-001
- **Rationale**: The V2 suffix was intentionally removed from the component name during development. The spec lagged behind this deliberate naming decision.
- **Change**: Replaced all occurrences of `ExtentManagerV2` with `ExtentManager` throughout the spec (FR-001 text, User Story 1 test description, Assumptions section).

### BACKFILL-FORMAT-VERSION: Update on-disk format version from 4 to 5

- **Type**: Backfill (spec trails code)
- **Status**: Applied
- **Requirement**: On-Disk Format Reference / Key Entities (Superblock)
- **Rationale**: Code uses `FORMAT_VERSION = 5` with magic `CERTUSV5` (`0x4345_5254_5553_5635`). The spec documented the previous version 4. This is a code-correct backfill.
- **Change**: Updated magic constant from `0x4345_5254_5553_5634` ("CERTUSV4") to `0x4345_5254_5553_5635` ("CERTUSV5") and version field from 4 to 5 in both the Key Entities section and the Superblock table.

---

## Deferred Proposals (require human decision)

### DEFERRED-SC-005: 100M extent scalability claim unverified

- **Type**: Deferred
- **Status**: Pending human decision
- **Requirement**: SC-005
- **Summary**: The spec claims ~100 million extent support on a 10 TB data device but no test validates this scale. The largest test uses 800 extents on 256 MiB.
- **Options**:
  1. Add a large-scale benchmark (expensive, may need dedicated CI)
  2. Relax the claim to "design supports" without hard verification
  3. Accept the claim as an architectural design statement

### DEFERRED-UNSPECCED-METHODS: Unspecified implemented features

- **Type**: Deferred
- **Status**: Pending human decision
- **Requirement**: N/A (new requirements needed)
- **Summary**: Several implemented features have no spec coverage:
  - `used_bytes()` and `capacity_bytes()` monitoring methods on IExtentManager
  - Fault injection test infrastructure (`FaultConfig`, `MockBlockDevice` fault modes)
  - `WriteHandle` defined in the `interfaces` crate rather than the component crate
- **Options**:
  1. Add informational requirements for monitoring methods
  2. Document test infrastructure as non-normative
  3. Leave as implementation details outside spec scope
