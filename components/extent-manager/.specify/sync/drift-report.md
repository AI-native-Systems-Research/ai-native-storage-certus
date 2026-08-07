Generated: 2026-08-07T15:31:39Z

# Spec-vs-Implementation Drift Report: extent-manager

**Component**: `components/extent-manager`
**Spec**: `components/extent-manager/specs/001-extent-manager-v2/spec.md`
**Also read**: `plan.md`, `tasks.md` (alongside spec)

## Summary

| Status | Count |
|--------|-------|
| Aligned | 39 |
| Drifted | 4 |
| Not Implemented | 0 |
| Unspecced | 4 |

Requirements analyzed: 37 FR + 6 SC = 43. The implementation is a very
close match to the spec; drift is concentrated in one real defect
(FR-030, `volatile_write_cache`) plus three low-severity doc/semantic
mismatches.

## Per-Requirement Table

| ID | Status | Location | Notes |
|----|--------|----------|-------|
| FR-001 | Aligned | lib.rs:81-103 | `define_component!` ExtentManager, provides IExtentManager, receptacles metadata_device + logger |
| FR-002 | Aligned | lib.rs:384-438 | validates sector_size>0, slab_size%sector_size, max_extent_size<=slab_size, region_count pow2, checkpoint region fit |
| FR-003 | Aligned | lib.rs:483-498; superblock.rs:58-97 | superblock at LBA 0 with params + CRC32 |
| FR-004 | Aligned | lib.rs:514-582; recovery.rs:11-71 | reads/validates superblock, recovers active checkpoint, rebuilds state |
| FR-005 | Aligned | lib.rs:584-630 | sector-aligned slot, WriteHandle w/ offset, not visible pre-publish |
| FR-006 | Aligned | lib.rs:597-616 | publish writes key; FREE_KEY frees slot + returns Ok silently |
| FR-007 | Aligned | lib.rs:618-621; iextent_manager.rs:141-155 | abort/drop frees slot |
| FR-008 | Aligned | region.rs:121-160 | remove sets FREE_KEY, OffsetNotFound, deferred free via pending_frees |
| FR-009 | Aligned | lib.rs:632-678 | get_extents/for_each_extent skip FREE_KEY slots |
| FR-010 | Aligned | slab.rs:7-14 | dense `keys: Vec<u64>` per slab, no separate index |
| FR-011 | Aligned | slab.rs:5 | `FREE_KEY = u64::MAX` |
| FR-012 | Aligned | region.rs:11, 121-127 | `BTreeMap<u64, Slab>`, range(..=offset).next_back() |
| FR-013 | Aligned | checkpoint.rs:15-115 | serialize slabs+key vectors, CRC32, write inactive, flip superblock |
| FR-014 | Aligned | lib.rs:276-286 | skip when no region dirty |
| FR-015 | Aligned | lib.rs:729-761 | Condvar coalescing, single in-flight writer |
| FR-016 | Drifted (Low) | lib.rs:112 (code); iextent_manager.rs:244 + README.md:13 (docs) | code default is 30s (correct); interface doc comment and README still say "five minutes" — FR-016 itself calls for these to be corrected, but they remain stale |
| FR-017 | Aligned | recovery.rs:29-66 | active copy first, inactive fallback |
| FR-018 | Aligned | recovery.rs:76-85; lib.rs:552-568 | rebuild bitmap from key vectors (non-FREE_KEY → allocated) |
| FR-019 | Aligned | buddy.rs | per-region buddy allocator |
| FR-020 | Aligned | slab.rs:29-35; bitmap.rs:42-51 | bitmap allocator with rover |
| FR-021 | Aligned | slab.rs:91-121 | SizeClassManager HashMap<element_size,Vec<start_offset>> |
| FR-022 | Aligned | lib.rs:219 | `key & (regions.len()-1)` |
| FR-023 | Aligned | lib.rs:23,90 | `parking_lot::RwLock` per region |
| FR-024 | Aligned | (type composition) | Send+Sync by composition, not enforced (matches FR wording) |
| FR-025 | Aligned | region.rs:16,147,152-160 | deferred free until post-checkpoint |
| FR-026 | Aligned | lib.rs:686-692 | get_instance_id from superblock |
| FR-027 | Aligned | lib.rs:694-696; 132-141 | set_checkpoint_interval(Option<Duration>); None → wait indefinitely |
| FR-028 | Aligned | lib.rs:181-183 | set_metadata_ns_id on concrete struct, not in trait |
| FR-029 | Aligned | lib.rs:169-171 | set_dma_alloc |
| FR-030 | Drifted (High) | lib.rs:309-310; checkpoint.rs:102-103; block_io.rs | inverted feature semantics AND `BlockDeviceClient::flush()` does not exist (compile error when feature enabled) — see Findings |
| FR-031 | Aligned | lib.rs:698-708 | used_bytes = buddy total_usable - total_free (slab granularity) |
| FR-032 | Drifted (Low) | lib.rs:710-715 | returns sum of buddy usable size (= data_disk_size - data_start_offset), not total data_disk_size; differs under shared-device (metadata_region_size>0) |
| FR-033 | Aligned | iextent_manager.rs:95-167 | WriteHandle in interfaces crate, RAII two-phase |
| FR-034 | Aligned | test_support.rs:15-63; lib.rs:16-17 | FaultConfig (fail_after_n_writes, fail_all_writes) behind `testing`; no read faults (matches spec) |
| FR-035 | Aligned | iextent_manager.rs:62-90; lib.rs:425-446 | metadata_region_size default 128 MiB; caps metadata area, computes data_start_offset |
| FR-036 | Drifted (Low) | lib.rs:717-727,200 | set/get on trait present; metadata_base_lba IS applied (block_io base_lba); data_base_lba is stored+returned but never applied to any I/O (component performs no data-device I/O — caller adds it) |
| FR-037 | Aligned | lib.rs:173-175, 365-367 | set_post_checkpoint_hook; fired once, synchronously, after superblock update before return |
| SC-001 | Aligned | tests/lifecycle.rs, tests/edge_cases.rs | lifecycle correctness covered by integration tests |
| SC-002 | Aligned | tests/checkpoint.rs; recovery.rs | round-trip reconstructed from key vectors |
| SC-003 | Aligned | tests/checkpoint.rs; recovery.rs:47-66 | dual-copy fallback |
| SC-004 | Aligned | tests/concurrent.rs | per-region locks, 8+ thread tests |
| SC-005 | Aligned (unverified) | (design) | 100M-extent scale is an explicit "not yet verified by benchmark" architectural target; tasks.md Open item confirms unverified — consistent with spec wording |
| SC-006 | Aligned | lib.rs:729-761 | at most 2 sequential checkpoint I/O, never parallel |

## Detailed Findings

### FR-030 (High) — `volatile_write_cache` semantics inverted and `flush()` missing
Spec FR-030 states: *"When enabled, flush calls in checkpoint and format
paths are conditionally compiled **out**, improving performance..."*.

The code does the **opposite**: flush is gated to be compiled **in** only
when the feature is enabled:
- `lib.rs:309-310`: `#[cfg(feature = "volatile_write_cache")] metadata_client.flush()?;`
- `checkpoint.rs:102-103`: `#[cfg(feature = "volatile_write_cache")] metadata_client.flush()?;`

The Cargo.toml comment (lines 24-27) and README.md:66 confirm the code's
intended semantics ("Enable NVMe flush commands after writes") — i.e. the
spec text is the item that is inverted, not the code.

More seriously: `BlockDeviceClient` (block_io.rs) exposes only
`alloc_buffer`, `write_blocks`, `read_blocks`, `sector_size` — there is
**no `flush()` method**. Enabling the `volatile_write_cache` feature makes
the crate fail to compile at lib.rs:310 and checkpoint.rs:103. The mock does
handle `Command::FlushSync` (test_support.rs:230-234), but nothing sends it.
Net: the feature is non-functional as written.

Also, FR-030 mentions a flush in the "format path"; `format()`
(lib.rs:383-512) contains no flush call under any cfg.

### FR-016 (Low) — stale "five minutes" doc strings not corrected
True default is 30 seconds (lib.rs:112); code is correct. FR-016 explicitly
instructs that the interface doc comment (iextent_manager.rs:244,
"The default is five minutes.") and README.md:13 ("default 5 minutes") be
corrected to 30 seconds. Both remain stale, so the FR's remediation intent
is unmet.

### FR-032 (Low) — capacity_bytes returns usable, not total device capacity
Spec: *"total data device capacity in bytes as configured during format()"*.
`capacity_bytes()` (lib.rs:710-715) sums `buddy.total_usable_size()`
(= `data_disk_size - data_start_offset`). Equals `data_disk_size` in
separate-device mode, but under-reports by the reserved metadata area in
shared-device mode (`metadata_region_size > 0`). Minor.

### FR-036 (Low) — data_base_lba is a stored no-op internally
`set_metadata_base_lba` correctly shifts metadata I/O (base_lba threaded into
`BlockDeviceClient::with_base_lba`, lib.rs:200-208). `data_base_lba` is only
stored (lib.rs:98,721) and returned (lib.rs:725); it is never applied to any
internal I/O because this component performs no data-device I/O (callers own
the data path). Matches the intended design (dispatcher adds the offset) but
is a semantic mismatch with the FR wording "shifts all data-device I/O
similarly." Informational.

## Unspecced Code

| Item | Location | Notes |
|------|----------|-------|
| Checkpoint telemetry logging (extent count, bytes, % used) | lib.rs:288, 323-363 + format_bytes lib.rs:38-51 | Rich `checkpoint_complete` log line with occupancy stats; no FR/SC covers logging output |
| WriteHandle accessors key()/extent_offset()/extent_size() | iextent_manager.rs:120-130 | Public getters beyond the publish/abort contract in FR-033 |
| MockBlockDevice reboot/fault helpers beyond FaultConfig | test_support.rs:55-78 | set_fault_config, clear_faults, shared_state, reboot_from, with_fault_config — support US2/US3 reboot tests; only FaultConfig is named in FR-034 |
| BuddyAllocator::mark_allocated split path | buddy.rs:117-157 | Non-trivial recovery-time reconstruction helper; implied by FR-018 but not individually specced |

## Conflicts / Nonexistent References

- The `verif/` Creusot proofs referenced by iextent_manager.rs:169-184
  (P1–P10) **do exist**:
  `components/extent-manager/verif/verif/extent_manager_verif_rlib/*.coma`
  (12 .coma files present). This is NOT a dangling reference.
- **plan.md is stale vs code** (informational, not spec.md):
  - plan.md:39,213-228 reference a `block_device` receptacle (data device) and
    a `components/extent-manager/v2/` source path; neither exists — code has only
    `metadata_device` + `logger` receptacles and a flat `src/`.
  - plan.md:48 describes `checkpoint_interval_ms: AtomicU64 (default 5000)`; the
    real mechanism is `CheckpointTimerState` with `Option<Duration>` default 30s.
  - plan.md:219 says superblock "(v5)".
- **tasks.md is stale vs spec/code** (informational): tasks.md:18,26 reference
  "CERTUSV5"/"v5" and crate `extent-manager-v2`. spec.md and code use magic
  "CERTUSV4" with FORMAT_VERSION 6 (superblock.rs:5-6) — spec.md and code agree;
  tasks.md does not.
- **README.md:34** states "format version (5)"; code is version 6
  (superblock.rs:6). Doc drift, not a spec requirement.

## Recommendations

1. **FR-030 (High)** — Reconcile spec, feature, and code. Either (a) add a real
   `flush()` method to `BlockDeviceClient` that issues `Command::FlushSync` so
   the feature compiles and works, and rewrite FR-030 so its semantics match the
   code (enabled = issue flush), or (b) if the spec's "compiled out when enabled"
   wording is authoritative, invert the `#[cfg]` gates. Add a CI job that builds
   with `--features volatile_write_cache` to prevent silent breakage.
2. **FR-016 (Low)** — Update iextent_manager.rs:244 and README.md:13 from
   "five minutes" to "30 seconds" to satisfy the FR's own remediation clause.
3. **FR-032 (Low)** — Either return `data_disk_size` (as the FR states) or amend
   the FR to say "usable capacity (excludes reserved metadata area)".
4. **FR-036 (Low)** — Clarify in the FR that `data_base_lba` is a caller-consumed
   configuration value (no internal data-path I/O in this component).
5. Refresh plan.md / tasks.md / README.md (block_device receptacle, v2 path,
   AtomicU64 interval, CERTUSV5/v5) to match the shipped design.
