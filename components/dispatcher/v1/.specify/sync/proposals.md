# Drift Resolution Proposals

Generated: 2026-05-12
Based on: drift-report from 2026-05-12

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 1 |
| Align (Code -> Spec) | 1 |
| Human Decision | 0 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: Unspecced - Background SSD Evictor

**Direction**: BACKFILL (Code -> Spec)

**Feature**: BackgroundEvictor thread that evicts oldest BlockDevice entries from SSD when utilization exceeds a threshold.
**Location**: `src/background.rs:108-282`, `src/lib.rs:658-695`

**Proposed Addition to Spec** (User Story 10 + FR-029..FR-033):

#### User Story 10 - SSD Capacity Eviction (Priority: P3)

When the SSD data drives approach capacity, a background evictor removes the oldest (LRU by TSC timestamp) BlockDevice entries to prevent extent allocation failures. The evictor periodically checks combined SSD utilization across all data drives. When utilization exceeds the high-water mark, it evicts entries in batches until utilization drops below the low-water mark.

**Acceptance Scenarios**:

1. **Given** SSD utilization is above `ssd_eviction_threshold` (default 0.9), **When** the evictor wakes, **Then** it calls `oldest_keys(batch_size)` and evicts entries in BlockDevice state until utilization drops below `ssd_eviction_low_watermark` (default 0.8).
2. **Given** an entry is in MemoryTier state, **When** the evictor evaluates it, **Then** it is skipped (still hot in DRAM).
3. **Given** an entry has active read/write references, **When** the evictor attempts to remove it, **Then** the removal fails and the entry is skipped.
4. **Given** `ssd_eviction_threshold` is set to 0.0, **When** the dispatcher initializes, **Then** the evictor is NOT started.
5. **Given** shutdown is called, **When** the evictor is running, **Then** it finishes the current batch (if any) and exits cleanly.

#### Functional Requirements:

- **FR-029**: The dispatcher MUST start a background SSD evictor thread during `initialize()` if `ssd_eviction_threshold > 0.0` and at least one data drive is configured. The evictor MUST be shut down during `shutdown()`.
- **FR-030**: The evictor MUST periodically check combined SSD utilization (sum of `used_bytes()` / sum of `capacity_bytes()` across all extent managers). The check interval MUST be configurable via `ssd_eviction_interval_secs` (default: 5).
- **FR-031**: When utilization exceeds `ssd_eviction_threshold` (default: 0.9), the evictor MUST evict BlockDevice-only entries using `oldest_keys(batch_size)` for LRU ordering, stopping when utilization drops below `ssd_eviction_low_watermark` (default: 0.8) or the batch is exhausted.
- **FR-032**: The evictor MUST skip entries in MemoryTier state (non-null pointer in dispatch map, indicating the entry is still hot in DRAM). Entries with active read or write references MUST be skipped (dm.remove fails gracefully).
- **FR-033**: The `DispatcherConfig` MUST include `ssd_eviction_threshold`, `ssd_eviction_low_watermark`, `ssd_eviction_batch_size`, and `ssd_eviction_interval_secs` fields with the specified defaults.

**Rationale**: Without SSD eviction, the SSD fills up and all background write-throughs fail silently — new entries have no SSD backing and are permanently lost on memory-tier eviction. This is a critical data-path issue for long-running workloads.

**Confidence**: HIGH

---

### Proposal 2: 001-dispatcher-cache-interface/FR-024

**Direction**: ALIGN (Spec + Code update)

**Current State**:
- Spec says: "Count-based TSC eviction (from v0) is NOT used in v1"
- Code has: `max_cache_entries` (default: 10000) and `eviction_threshold` (default: 0.8) in DispatcherConfig, unused by v1 eviction logic

**Proposed Resolution**:

Mark the fields as deprecated in the code documentation:

```rust
/// **Deprecated in v1**: Memory-tier eviction is capacity-based.
/// Retained for backward compatibility with v0 config consumers.
pub max_cache_entries: usize,
/// **Deprecated in v1**: Unused. See `ssd_eviction_threshold` for SSD eviction.
pub eviction_threshold: f64,
```

Add a note to the spec clarification section:
> The `max_cache_entries` and `eviction_threshold` fields in `DispatcherConfig` are retained for API compatibility but unused by the v1 eviction logic.

**Rationale**: Removing the fields would be a breaking change for existing consumers using struct literals. Deprecation with documentation is the minimal-risk approach.

**Confidence**: HIGH
