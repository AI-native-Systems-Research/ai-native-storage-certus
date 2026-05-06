# Drift Resolution Proposals

Generated: 2026-05-05
Based on: drift-report from 2026-05-05

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 3 |
| Align (Spec -> Code) | 1 |
| Human Decision | 2 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: 001-dispatcher-cache-interface/FR-017

**Direction**: ALIGN (Spec -> Code)

**Current State**:
- Spec says: "Background write failure must clean up dispatch map entry and release read reference"
- Code does: "Error path in process_write_job returns without cleanup, leaking the dispatch map entry and read reference"

**Proposed Resolution**:

In the background writer error path, add:
1. Call `dispatch_map.release_read(key)` to drop the read reference
2. Call `dispatch_map.remove(key)` to clean up the leaked entry

**Rationale**: This is a resource leak bug. The spec correctly identifies the required cleanup. Without it, failed background writes permanently leak dispatch map entries, eventually exhausting capacity.

**Confidence**: HIGH

---

### Proposal 2: 001-dispatcher-cache-interface/FR-012

**Direction**: HUMAN_DECISION

**Current State**:
- Spec says: "Validate all receptacles are connected at initialize() time"
- Code does: "Only validates dispatch_map at init; gpu_services is checked lazily at first use"

**Options**:
- A) **ALIGN**: Add gpu_services validation to initialize() — fail fast if not connected
- B) **BACKFILL**: Update spec to say "critical receptacles (dispatch_map) validated at init; optional receptacles may be validated lazily"

**Questions**: Is there a scenario where the dispatcher should start without gpu_services connected (e.g., testing without GPU hardware)?

**Confidence**: MEDIUM

---

### Proposal 3: 001-dispatcher-cache-interface/FR-016

**Direction**: HUMAN_DECISION

**Current State**:
- Spec says: "Pass a unique PCI-derived identifier to each extent manager"
- Code does: "Passes data_disk_size but no PCI-derived unique identifier"

**Options**:
- A) **ALIGN**: Pass the PCI address string as a unique ID to each extent manager
- B) **BACKFILL**: Update spec — drive index or size is sufficient since the dispatcher owns the drive list

**Questions**: Is the unique ID needed for crash recovery (to identify which physical drive an extent belongs to)?

**Confidence**: MEDIUM

---

### Proposal 4: Unspecced - Two-phase Store API

**Direction**: BACKFILL (Code -> Spec)

**Feature**: prepare_store/commit_store/cancel_store methods
**Location**: src/lib.rs

**Proposed Addition to Spec**:
- FR-020: The dispatcher MUST support a two-phase store protocol: `prepare_store(key, size)` allocates staging and returns an IPC handle; `commit_store(key)` finalizes the entry and enqueues background write; `cancel_store(key)` aborts and frees staging.

**Rationale**: This API allows clients to write directly into staging buffers without an intermediate copy, which is critical for performance with large entries.

**Confidence**: HIGH

---

### Proposal 5: Unspecced - Eviction Mechanism

**Direction**: BACKFILL (Code -> Spec)

**Feature**: run_eviction_cycle with watermark-based LRU eviction
**Location**: src/lib.rs

**Proposed Addition to Spec**:
- FR-021: The dispatcher MUST support capacity-based eviction. When staging or storage utilization exceeds a configurable high-watermark, the dispatcher evicts least-recently-used entries until utilization drops below the low-watermark. Entries with active references MUST NOT be evicted.

**Rationale**: Eviction is essential for bounded-memory operation. The implementation is functional and tested.

**Confidence**: HIGH

---

### Proposal 6: Unspecced - Version Enums and format_on_init

**Direction**: BACKFILL (Code -> Spec)

**Feature**: BlockDeviceVersion/ExtentManagerVersion enums, format_on_init config
**Location**: src/lib.rs

**Proposed Addition to Spec**:
- FR-022: DispatcherConfig MUST include a `format_on_init` flag. When true, the dispatcher formats all extent managers during initialization (destructive). When false, existing data is preserved and recovered.

**Rationale**: This is a required operational parameter for first-run vs. restart scenarios.

**Confidence**: HIGH
