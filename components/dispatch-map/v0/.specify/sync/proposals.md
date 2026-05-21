# Drift Resolution Proposals

Generated: 2026-05-05
Based on: drift-report from 2026-05-05

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 2 |
| Align (Spec -> Code) | 0 |
| Human Decision | 3 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: 001-dispatch-map/FR-004, FR-006, FR-007

**Direction**: HUMAN_DECISION

**Current State**:
- Spec says: "lookup(key, timeout)", "take_read(key, timeout)", "take_write(key, timeout)" with caller-supplied timeout
- Code does: "Hardcoded DEFAULT_TIMEOUT = 2000ms, no timeout parameter in interface signatures"

**Options**:
- A) **ALIGN**: Add `timeout: Duration` parameter to lookup, take_read, take_write in IDispatchMap interface. This changes the trait and all callers.
- B) **BACKFILL**: Update spec to document fixed internal timeout. Rationale: callers don't need per-call timeout control since the timeout is a safety net against deadlocks, not a tuning knob.
- C) **HYBRID**: Add configurable default timeout at construction time (not per-call). Update spec to match.

**Questions**:
- Are there callers that need different timeouts for different operations?
- Is the 2000ms value appropriate or should it be configurable?
- The spec assumed 100ms in assumptions — was 2000ms chosen based on real-world testing?

**Confidence**: MEDIUM

---

### Proposal 2: 001-dispatch-map/FR-002

**Direction**: HUMAN_DECISION

**Current State**:
- Spec says: "Per-entry metadata includes extent_manager_id and block_device_id"
- Code does: "Missing these fields. Has unspecced tsc field for LRU ordering instead."

**Options**:
- A) **ALIGN**: Add extent_manager_id and block_device_id fields. Needed if recovery must identify which physical drive owns an extent.
- B) **BACKFILL**: Remove these fields from spec. The dispatcher owns the drive list and can derive drive identity from the extent manager reference. The tsc field serves LRU eviction which is more critical.
- C) **HYBRID**: Keep block_device_id (for recovery), drop extent_manager_id (redundant with BD), add tsc.

**Questions**:
- During crash recovery, how does the system identify which physical drive an extent record belongs to?
- Is drive identity derived from the extent manager's position in the initialization order?

**Confidence**: MEDIUM

---

### Proposal 3: 001-dispatch-map/SC-004

**Direction**: HUMAN_DECISION

**Current State**:
- Spec says: "Entry size <= 32 bytes"
- Code does: "Entry is ~36+ bytes due to tsc field (8 bytes) and enum padding"

**Options**:
- A) **ALIGN**: Remove tsc field to hit 32-byte target. But this breaks LRU eviction.
- B) **BACKFILL**: Update SC to "Entry size <= 40 bytes" to accommodate LRU timestamp.
- C) **REDESIGN**: Pack tsc into fewer bits (e.g., 32-bit relative timestamp) and optimize enum layout.

**Questions**: Was the 32-byte target driven by cache-line alignment (64B = 2 entries) or memory budget?

**Confidence**: MEDIUM

---

### Proposal 4: Unspecced - LRU Eviction Support

**Direction**: BACKFILL (Code -> Spec)

**Feature**: oldest_keys method + tsc field for LRU ordering
**Location**: src/lib.rs

**Proposed Addition to Spec**:
- FR-016: The dispatch map MUST track entry access time via a monotonic timestamp (TSC). The `oldest_keys(n)` method MUST return the N least-recently-accessed keys for eviction, excluding entries with active references.

**Rationale**: LRU eviction is used by the dispatcher (see dispatcher FR-021 proposal). The dispatch map provides the ordering primitive.

**Confidence**: HIGH

---

### Proposal 5: Unspecced - set_dma_alloc Initialization

**Direction**: BACKFILL (Code -> Spec)

**Feature**: set_dma_alloc as separate interface method
**Location**: src/lib.rs

**Proposed Addition to Spec**:
- FR-017: The dispatch map MUST accept a DMA allocation function via `set_dma_alloc(alloc_fn)` before initialization. This function is used to allocate NUMA-local DMA-capable staging buffers. Calling any data operation before set_dma_alloc MUST return an error.

**Rationale**: DMA allocation is a required dependency not expressible as a component receptacle (it's a function pointer, not a component interface). It must be documented as a mandatory initialization step.

**Confidence**: HIGH
