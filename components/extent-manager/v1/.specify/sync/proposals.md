# Drift Resolution Proposals

Generated: 2026-04-17
Based on: drift-report from 2026-04-17

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 7 |
| Align (Spec -> Code) | 1 |
| Remove from Spec | 1 |
| New Spec Requirement | 1 |
| Keep as-is | 2 |

## Decisions

### Proposal 1: 001/FR-003 — APPROVED (Backfill)

**Direction**: BACKFILL

**Spec says**: "Persist metadata including namespace ID"
**Code does**: namespace_id removed from Extent and ExtentMetadata. One ExtentManager per namespace/device pair.

**Resolution**: Update FR-003 to:
> System MUST persist extent metadata (key, offset, size, optional filename, optional CRC) to the block device. Each ExtentManager instance manages a single namespace/device — namespace and device identity are not part of the stored metadata.

---

### Proposal 2: 001/FR-006 — APPROVED (Backfill)

**Direction**: BACKFILL

**Spec says**: "Iteration MUST hold an exclusive lock"
**Code does**: `get_extents()` uses read lock. Writers blocked, concurrent reads allowed.

**Resolution**: Update FR-006 to:
> System MUST support iterating through all stored extents via `get_extents()`, visiting each extent exactly once. Iteration holds a read lock — concurrent create and remove operations are blocked until iteration completes, but concurrent lookups and iterations are allowed.

---

### Proposal 3: 001/FR-007 — APPROVED (Backfill)

**Direction**: BACKFILL

**Spec says**: "Valid size classes range from 128 KiB to 5 MiB and must be multiples of 4 KiB"
**Code does**: No validation of extent_size. Any u32 accepted.

**Resolution**: Remove the size class range constraint from FR-007. Any extent_size is valid. Updated text:
> System MUST support dynamic slab-based allocation. At initialization, the caller specifies total managed space (in bytes) and slab size (in bytes, must be >= 8 KiB and a multiple of 4 KiB). Size classes are NOT pre-declared; the first `create_extent` call for a given size class dynamically allocates a slab. Each slab serves exactly one size class. When a slab is full, a new slab for that class is allocated automatically if space remains. Maximum 256 slabs.

---

### Proposal 4: 001/FR-012 — APPROVED (Backfill)

**Direction**: BACKFILL

**Spec says**: "System MUST support fresh initialization AND reopening an existing volume."
**Code does**: `open()` removed. Only fresh `initialize()` exists.

**Resolution**: Update FR-012 to:
> System MUST support fresh initialization via `initialize(total_size_bytes, slab_size_bytes)`. Reopening an existing volume is handled by crash recovery (see US5/FR-008/FR-009).

---

### Proposal 5: 001/Key Entities — APPROVED (Backfill)

**Direction**: BACKFILL

**Spec says**: "Superblock at block 0 with magic, format version, slab table, CRC-32"
**Code does**: superblock.rs deleted. Initialization is in-memory.

**Resolution**: Remove Superblock from Key Entities. Update on-disk layout: bitmap + extent records only, no superblock.

---

### Proposal 6: 001/FR-008, FR-009, US5 — REJECTED

**Direction**: Proposed removal was REJECTED.

**Resolution**: Keep FR-008, FR-009, and US5 (crash recovery) in spec. These are requirements for future implementation.

---

### Proposal 7: 001/FR-015 — APPROVED (Keep)

**Direction**: KEEP

**Resolution**: Keep FR-015 in spec as a future recovery requirement tied to US5.

---

### Proposal 8: 002/FR-003 — APPROVED (Remove)

**Direction**: REMOVE FROM SPEC

**Spec says**: `--ns-id` used for extent manager initialization.
**Code does**: `--ns-id` used for block device capacity queries only.

**Resolution**: Remove `--ns-id` from spec 002 entirely. The extent manager no longer takes namespace_id.

---

### Proposal 9: 002/FR-009 — ALIGN (Logger binding mandatory)

**Direction**: ALIGN (Spec -> Code)

**Spec says**: "Wire full component stack including Logger to all components."
**User decision**: Logger binding is mandatory.

**Resolution**: The benchmark app must wire the logger to the extent manager. Code change needed in `apps/extent-benchmark/src/main.rs`.

---

### Proposal 10: 001 — APPROVED (New Requirement)

**Direction**: NEW SPEC REQUIREMENT

**Resolution**: Add to spec 001:
> **FR-017**: System MUST accept a DMA allocator via `set_dma_alloc()` before any I/O operations. The allocator is used for all block device buffer allocations.

---

### Proposal 11: 001 — APPROVED (Backfill)

**Direction**: BACKFILL

**Resolution**: Remove all namespace_id references from spec 001:
- Key Entities / Extent: Remove "namespace ID" from metadata fields
- Assumptions: Remove "A namespace ID identifies the NVMe namespace..."
- US1 scenario 1: Change "returns the extent's on-disk location (namespace, offset)" to "returns the extent's on-disk offset"
- US2 scenario 1: Remove "namespace ID" from returned fields
- Add note: "Each ExtentManager instance manages a single block device namespace."

---

### Proposal 12: 001/US7 Scenario 5 — APPROVED (Move)

**Direction**: BACKFILL

**Spec says**: US7 scenario 5: "Given a previously initialized block device, When the manager is opened..."
**Resolution**: Move this scenario from US7 (Initialization) to US5 (Crash Recovery), since `open()` is a recovery operation.
