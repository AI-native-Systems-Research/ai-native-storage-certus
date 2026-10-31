# Drift Resolution Proposals — interfaces

> **Proposals 2026-10-30 (branch `specsync-a`; delta over base `a0292be6`).** No human in the loop: BACKFILL is approved and applied; ALIGN and judgment calls are not. Source: `drift-report.md` top note, `drift-report.json` key `sync_2026_10_30`.
>
> | # | Requirement | Direction | Confidence | Approved |
> |---|---|---|---|---|
> | P-1 | FR-011: add `IGpuServices::memcpy_batch_async` (signature, `spdk` gate, no order among entries, entries must not overlap, pointers valid until sync, host pointers pinned, legacy NULL stream not accepted, errors, empty batch `Ok`) | BACKFILL | HIGH | yes, applied |
> | P-2 | `plan.md` counts: total 178 → 180; `IGpuServices` 24 → 25 (spdk-gated 10 → 11); `IDispatcher` 24 → 25 (`clear_all`, missed on unstable) | BACKFILL | HIGH | yes, applied |
> | P-3 | Implementation Notes: `src/igpu_services.rs:231-246` → `:250-265` | BACKFILL | HIGH | yes, applied |
> | P-4 | SC-4: fix 3 pre-existing rustdoc unresolved links in `src/` (`idispatcher.rs:339` `IDispatcher::tier_event_stats`, the trait being spdk-gated; `igpu_services.rs:587,607` `create_stream`/`stream_synchronize`, need `Self::`) | ALIGN (doc, `src/`) | HIGH | **no**: needs a `src/` edit; pre-existing at `a0292be6`. Recorded in `align-tasks.md` |


Generated: 2026-10-21
Based on: drift-report from 2026-10-21 (branch `fix/dispatcher-remove-clear-mt`, full re-analysis)
Mode: batch. **Every proposal is `approved: false`. Nothing has been applied.**

The approved working-tree edits (FR-018a `store_already_resident` note at spec.md:331, and the `flush_to_ssd` `# Errors` doc at src/idispatcher.rs:832-837) were verified as ALIGNED. No proposal undoes or rewords them.

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 10 |
| Align (Spec → Code) | 4 (2 interface doc comments in `src/idispatcher.rs`, 1 code fix in `dispatcher-p2p`, 1 doc-examples task) |
| Human Decision | 0 (one choice is flagged inside Proposal 14) |
| New Specs | 0 |
| Remove from Spec | 0 |

| # | Requirement | Type | Severity | Confidence |
|---|---|---|---|---|
| 1 | FR-017 `Command`/`Completion` counts, `ReadWriteStats` histograms | BACKFILL | minor | HIGH |
| 2 | FR-018 `DispatcherConfig` 19 fields + `TierEventStats` | BACKFILL | minor | HIGH |
| 3 | FR-008 / US2 method inventory + `copy_gpu_to_memory_async` signature | BACKFILL | moderate | HIGH |
| 4 | FR-008 `clear_memory_tier` / `flush_to_ssd` one-liners | BACKFILL | moderate | HIGH |
| 5 | FR-008 `clear_memory_tier` interface doc comment | ALIGN (doc) | moderate | HIGH |
| 6 | FR-008 `batch_lookup` interface doc comment | ALIGN (doc) | minor | HIGH |
| 7 | FR-008 `IDispatcher` contract in `dispatcher-p2p` | ALIGN (code, other component) | major | HIGH |
| 8 | FR-023 `LookupConfig` 12 fields | BACKFILL | minor | HIGH |
| 9 | FR-021 `FormatParams` 9 fields | BACKFILL | minor | HIGH |
| 10 | Overview + FR-009 `IMemoryTier` gating, feature count | BACKFILL | minor | HIGH |
| 11 | NFR-002 `gpu` feature | BACKFILL | minor | HIGH |
| 12 | Implementation Notes: verified properties | BACKFILL | minor | HIGH |
| 13 | Backfill banner: FR-014 deferred | BACKFILL | minor | HIGH |
| 14 | NFR-004 runnable examples | ALIGN (doc task) | moderate | MEDIUM |

---

### Proposal 1: 001-interfaces/FR-017 — `Command` / `Completion` / `ReadWriteStats`

**Direction**: BACKFILL

**Requirement**: FR-017 Supporting Types – Block Device.

**Current State**:
- Spec says (spec.md:312): "`Command`: 12-variant enum for all NVMe operations (ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp, NsProbe, NsCreate, NsFormat, NsDelete, ControllerReset)." (spec.md:313): "`Completion`: 11-variant enum for operation results (ReadDone, …, ResetDone, Error); …" (spec.md:315): "`ReadWriteStats`: Cumulative per-direction (read/write) byte, op, and latency-sum counters with `total_ops`/`total_bytes`/`mean_read_latency_ns`/`mean_write_latency_ns` helpers; …"
- Code does: `Command` has 13 variants, the 13th being `FlushSync { ns_id }` (`src/iblock_device.rs:408-414`). `Completion` has 12, adding `FlushDone { handle, result }` (`src/iblock_device.rs:500-506`). Both were added in `e56a8b4e6` (2026-08-07); the behaviour is specified in block-device-spdk-nvme FR-031. `ReadWriteStats` also carries `read_size_buckets` / `write_size_buckets: [u64; IO_SIZE_BUCKETS]` (`:159-161`), with `IO_SIZE_BUCKETS = 25` (`:139`, exported `src/lib.rs:99`), `size_bucket` (`:177`), `bucket_lower_bound` (`:193`) and `merge_from` (`:218`).

**Proposed Resolution** — exact spec edits:

Old (spec.md:312):
```
- `Command`: 12-variant enum for all NVMe operations (ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp, NsProbe, NsCreate, NsFormat, NsDelete, ControllerReset).
```
New:
```
- `Command`: 13-variant enum for all NVMe operations (ReadSync, WriteSync, ReadAsync, WriteAsync, WriteZeros, BatchSubmit, AbortOp, NsProbe, NsCreate, NsFormat, NsDelete, ControllerReset, FlushSync). *(Sync 2026-10-21 — backfilled: `FlushSync { ns_id }` forces the namespace's volatile write cache to media, added in `e56a8b4e6` for the extent-manager `volatile_write_cache` feature; behaviour in block-device-spdk-nvme FR-031.)*
```
Old (spec.md:313, prefix only; the rest of the line is unchanged):
```
- `Completion`: 11-variant enum for operation results (ReadDone, WriteDone, WriteZerosDone, AbortAck, Timeout, NsProbeResult, NsCreated, NsFormatted, NsDeleted, ResetDone, Error);
```
New:
```
- `Completion`: 12-variant enum for operation results (ReadDone, WriteDone, WriteZerosDone, AbortAck, Timeout, NsProbeResult, NsCreated, NsFormatted, NsDeleted, ResetDone, FlushDone, Error);
```
Old (spec.md:315):
```
- `ReadWriteStats`: Cumulative per-direction (read/write) byte, op, and latency-sum counters with `total_ops`/`total_bytes`/`mean_read_latency_ns`/`mean_write_latency_ns` helpers; returned by `IBlockDevice::read_write_stats` and `IDispatcher::read_write_stats` (see FR-004/FR-008).
```
New:
```
- `ReadWriteStats`: Cumulative per-direction (read/write) byte, op, and latency-sum counters plus per-direction power-of-two IO-size histograms (`read_size_buckets`/`write_size_buckets`, `IO_SIZE_BUCKETS` = 25 buckets; `size_bucket`/`bucket_lower_bound` map sizes to buckets), with `total_ops`/`total_bytes`/`mean_read_latency_ns`/`mean_write_latency_ns` helpers and `merge_from` for aggregating across drives; returned by `IBlockDevice::read_write_stats` and `IDispatcher::read_write_stats` (see FR-004/FR-008).
```

**Rationale**: The code is shipped and tested, and is specified in the owning component; only the counts here are stale.
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 2: 001-interfaces/FR-018 — `DispatcherConfig` field count, `TierEventStats`

**Direction**: BACKFILL

**Requirement**: FR-018 Supporting Types – Dispatcher.

**Current State**:
- Spec says (spec.md:318): "`DispatcherConfig`: 18-field configuration — `data_pci_addrs`, `max_cache_entries`, `format_on_init`, SSD eviction params (…), `poller_base_cpu`, `max_eviction_attempts`, `backfill_delay_ms`, …". `TierEventStats` is not in the FR-018 type list (it appears only in FR-018a at spec.md:327).
- Code does: 19 fields (`src/idispatcher.rs:26-101`). `store_backpressure_ms: u64` (`:60-68`, default 5000 at `:116`) sits between `max_eviction_attempts` and `backfill_delay_ms`; FR-008 `reserve_memory` (spec.md:179) already depends on it by name. `TierEventStats` is a 16-field `Copy + Default + PartialEq` monotonic-counter snapshot, apart from the gauges `oldest_sampled` / `oldest_persisted` (`src/idispatcher.rs:319-463`), exported at `src/lib.rs:37`.

**Proposed Resolution** — exact spec edits:

Old (spec.md:318, prefix):
```
- `DispatcherConfig`: 18-field configuration — `data_pci_addrs`, `max_cache_entries`, `format_on_init`, SSD eviction params (`ssd_eviction_threshold`, `ssd_eviction_low_watermark`, `ssd_eviction_batch_size`, `ssd_eviction_interval_secs`), `poller_base_cpu`, `max_eviction_attempts`, `backfill_delay_ms`,
```
New:
```
- `DispatcherConfig`: 19-field configuration — `data_pci_addrs`, `max_cache_entries`, `format_on_init`, SSD eviction params (`ssd_eviction_threshold`, `ssd_eviction_low_watermark`, `ssd_eviction_batch_size`, `ssd_eviction_interval_secs`), `poller_base_cpu`, `max_eviction_attempts`, `store_backpressure_ms` (store-path backpressure budget, default 5000 ms, 0 = fail fast; see `reserve_memory` in FR-008), `backfill_delay_ms`,
```
Insert after spec.md:322 (the `LookupResult` bullet):
```
- `TierEventStats` *(Sync 2026-10-21 — backfilled)*: `Copy + Default` 16-field snapshot returned by `IDispatcher::tier_event_stats` — promotions (`promotions_to_memory`, `promotions_to_gpu`), the route partition (`lookup_hits_dram`, `lookup_hits_ssd`, `remote_lookup_hits`, `remote_lookup_misses`), evictions (`evictions_from_memory`, `evictions_from_ssd`), store refusals (`store_backpressure_events`, `store_drops_on_full`, `store_already_resident`; see FR-018a), clean-eviction diagnostics (`evictions_blocked_by_pin`, `eviction_scans_exhausted`, `evictions_blocked_unpersisted`) and the two **gauges** `oldest_sampled`/`oldest_persisted`. All other fields are monotonic since process start.
```

**Rationale**: The shipped field is relied on by FR-008 already; the type is shared across both dispatchers and the metrics endpoint.
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 3: 001-interfaces/FR-008 + User Story 2 — `IDispatcher` method inventory

**Direction**: BACKFILL

**Requirement**: FR-008 IDispatcher Interface; User Story 2 acceptance criterion 1.

**Current State**:
- Spec says (spec.md:180): "`copy_gpu_to_memory_async(&self, key: CacheKey, ipc_handle: IpcHandle, stream: GpuStream) -> Result<(), DispatcherError>` - DMA copy into reserved slot." FR-008 has no `batch_populate` or `tier_event_stats`. US2 (spec.md:49) lists 16 methods.
- Code does: `copy_gpu_to_memory_async(&self, key: CacheKey, regions: &[IpcHandle], stream: GpuStream)` (`src/idispatcher.rs:732`) gathers N per-layer regions contiguously into one slot (`:717-726`). `batch_populate(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` is at `:679-688`, and `tier_event_stats(&self) -> TierEventStats` at `:877-883`. Both are implemented in `dispatcher/src/lib.rs:3324, 4080` and `dispatcher-p2p/src/lib.rs:2430, 2915`.

**Proposed Resolution** — exact spec edits:

Old (spec.md:180):
```
- **Method**: `copy_gpu_to_memory_async(&self, key: CacheKey, ipc_handle: IpcHandle, stream: GpuStream) -> Result<(), DispatcherError>` - DMA copy into reserved slot.
```
New:
```
- **Method**: `copy_gpu_to_memory_async(&self, key: CacheKey, regions: &[IpcHandle], stream: GpuStream) -> Result<(), DispatcherError>` - DMA copy into reserved slot. The N regions are gathered contiguously into the one slot (region L lands at `slot + sum of preceding region sizes`), so a block split into N per-layer GPU allocations (vLLM 0.23+) is stored as one unit; `regions.len() == 1` is the legacy single-allocation path.
```
Insert after spec.md:178 (`populate`):
```
- **Method**: `batch_populate(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), DispatcherError>>` - Semantically `populate` per entry, but all D2H copies are submitted asynchronously and a single `stream_synchronize` covers the batch. Returns one `Result` per entry, in order.
```
Insert after spec.md:189 (`read_write_stats`):
```
- **Method**: `tier_event_stats(&self) -> TierEventStats` - Cumulative KV-cache tier-movement counters (see FR-018, FR-018a); always populated (not telemetry-gated); implementations that do no tiering return zeroes.
- **Method**: `schedule_write_through(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError>` - Non-blocking, best-effort background write-through; contract in FR-031a.
```
Old (spec.md:49):
```
- `IDispatcher` provides `initialize`, `shutdown`, `lookup`, `lookup_async`, `batch_lookup`, `check`, `remove`, `populate`, `reserve_memory`, `copy_gpu_to_memory_async`, `copy_gpu_to_memory_completed`, `release_memory`, `touch`, `promote_to_memory_tier`, `clear_memory_tier`, and `flush_to_ssd` methods.
```
New:
```
- `IDispatcher` provides `initialize`, `shutdown`, `lookup`, `lookup_async`, `batch_lookup`, `check`, `remove`, `populate`, `batch_populate`, `reserve_memory`, `copy_gpu_to_memory_async`, `copy_gpu_to_memory_completed`, `release_memory`, `pin`, `unpin`, `touch`, `promote_to_memory_tier`, `clear_memory_tier`, `flush_to_ssd`, `schedule_write_through`, `read_write_stats`, and `tier_event_stats` methods.
```

**Rationale**: The trait is compiler-enforced on both implementors; the spec inventory is simply behind.
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 4: 001-interfaces/FR-008 — `clear_memory_tier` / `flush_to_ssd` result contracts

**Direction**: BACKFILL

**Requirement**: FR-008, two method lines. This is the spec-side consequence of this branch's repairs `cccfb328e` and `9bd231d85`.

**Current State**:
- Spec says (spec.md:185): "`clear_memory_tier(&self) -> Result<usize, DispatcherError>` - Evict all memory-tier entries." (spec.md:186): "`flush_to_ssd(&self) -> Result<usize, DispatcherError>` - Flush pending writes to SSD."
- Code does: `clear_memory_tier` (`dispatcher/src/lib.rs:3940-4004`) demotes entries with an SSD copy and removes those without one. A pinned entry, including one whose write-through is pending, keeps its slot and is retried (`:3976`). The call returns `Timeout` if pinned entries remain after 5 s (`:3994-3998`), and `Ok(n)` counts only the entries actually cleared. `flush_to_ssd` (`:4006-4035`) blocks on the writer, returns `IoError` if a failed write-through left an entry resident (`:4026-4034`), and otherwise returns the number of memory-tier entries that are demotable. The approved interface doc says the same (`src/idispatcher.rs:831-837`), as do the approved dispatcher FR-038 and FR-056.

**Proposed Resolution** — exact spec edits:

Old (spec.md:185):
```
- **Method**: `clear_memory_tier(&self) -> Result<usize, DispatcherError>` - Evict all memory-tier entries.
```
New:
```
- **Method**: `clear_memory_tier(&self) -> Result<usize, DispatcherError>` - Empty the memory tier: entries with an SSD copy are demoted to BlockDevice, entries without one are removed. A **pinned** entry (a lookup, a client `pin`, or the pending write-through's own reference) keeps its slot and is retried; if pinned entries remain when the implementation's bounded wait expires, returns `Timeout` and leaves them resident. `Ok(n)` means the tier was emptied, and `n` counts only entries actually demoted or removed. *(Sync 2026-10-21 — CODE CHANGE, repair `cccfb328e`; behaviour in dispatcher spec 001 FR-038.)*
```
Old (spec.md:186):
```
- **Method**: `flush_to_ssd(&self) -> Result<usize, DispatcherError>` - Flush pending writes to SSD.
```
New:
```
- **Method**: `flush_to_ssd(&self) -> Result<usize, DispatcherError>` - Block until every queued write-through has run. Returns `IoError` if a write-through failed for an entry that is still in the memory tier (that entry has no SSD copy, so a later `clear_memory_tier` would drop its only copy); otherwise returns the number of memory-tier entries that now have a valid SSD offset. *(Sync 2026-10-21 — CODE CHANGE, repair `9bd231d85` / DIS-FLUSH-REPORTS-FAILED-WRITE; behaviour in dispatcher spec 001 FR-056.)*
```

**Rationale**: These are the approved repairs. The spec should state the result contract that both implementors must meet (see Proposal 7), not just name the method. The wording "bounded wait" deliberately leaves the 5 s value to the dispatcher spec.
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 5: 001-interfaces/FR-008 — `clear_memory_tier` interface doc comment

**Direction**: ALIGN (doc comment in `src/idispatcher.rs`; no behaviour change)

**Requirement**: FR-008 `clear_memory_tier`; dispatcher FR-038 (approved 2026-10-20).

**Current State**:
- Spec side: the approved dispatcher FR-038 says a pinned entry "MUST keep its slot and is retried … `Timeout` … `n` counts only the entries actually demoted or removed". FR-008 here is updated by Proposal 4.
- Code (interface doc, `src/idispatcher.rs:806-810`): "Evict all entries from the memory-tier, demoting them to block-device-backed. Entries whose write-through has completed are converted to block-device state in the dispatch map. **Entries still being written are removed entirely.** Returns the number of entries cleared." There is no `# Errors` section.
- Code (implementation, `dispatcher/src/lib.rs:3963-3998`): an entry still being written holds the write-through pin, so it is **not** removed; it is retried, and the call times out after 5 s.

**Proposed Resolution** — replace `src/idispatcher.rs:806-810` with:
```rust
        /// Empty the memory tier, demoting entries to block-device-backed where possible.
        ///
        /// Entries whose write-through has completed are converted to block-device
        /// state in the dispatch map. Entries with no SSD copy are removed. An entry
        /// that is pinned — by a lookup, a client `pin`, or its own pending
        /// write-through — keeps its DRAM slot and is retried until the pin drops:
        /// its dispatch-map entry still points at the slot.
        ///
        /// Returns the number of entries actually demoted or removed; `Ok` means
        /// the memory tier was emptied.
        ///
        /// # Errors
        ///
        /// Returns [`DispatcherError::Timeout`] if pinned entries are still
        /// resident when the implementation's bounded wait expires. They stay
        /// resident and cached.
```
Task: doc-only edit to `components/interfaces/src/idispatcher.rs`. Then `cargo doc -p interfaces --no-deps --features spdk`.

**Rationale**: The doc describes the exact pre-repair behaviour, freeing the slot of an in-flight entry. A future implementor following the trait doc would reintroduce the use-after-release bug `cccfb328e` fixed.
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 6: 001-interfaces/FR-008 — `batch_lookup` interface doc comment

**Direction**: ALIGN (doc comment in `src/idispatcher.rs`)

**Requirement**: FR-008 `batch_lookup` (spec.md:175, already correct: "Returns one `LookupOutcome` per requested entry, in order").

**Current State**:
- Spec says (spec.md:175): "Returns one `LookupOutcome` per requested entry, in order, each carrying **how** the key was served as well as the result."
- Code (doc, `src/idispatcher.rs:591`): "Returns one `Result` per input entry, in the same order." The signature at `:599-602` returns `Vec<LookupOutcome>`.

**Proposed Resolution** — replace `src/idispatcher.rs:591` with:
```rust
        /// Returns one [`LookupOutcome`] per input entry, in the same order: the
        /// result together with how the key was served (`served_by`).
```
**Rationale**: Spec is correct; the doc was not updated when the return type was widened on 2026-10-01.
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 7: 001-interfaces/FR-008 — `IDispatcher` contract in `dispatcher-p2p`

**Direction**: ALIGN (code change in **another component**, `components/dispatcher-p2p`; out of scope for an interfaces apply)

**Requirement**: FR-008 `remove`, `clear_memory_tier`, `flush_to_ssd` as documented in `src/idispatcher.rs:625-646, 806-838` (and Proposal 4).

**Current State**:
- Spec/interface says: `remove` "Does NOT block waiting for background write-through to complete. Proceeds immediately with removal." (`src/idispatcher.rs:627-628`). `flush_to_ssd` "Returns [`DispatcherError::IoError`] if a write-through to SSD failed for an entry that is still in the memory tier." (`:835-837`, approved). `clear_memory_tier`: see Proposal 5.
- Code does (`dispatcher-p2p`):
  - `flush_to_ssd` (`dispatcher-p2p/src/lib.rs:2889-2902`) returns `writer.in_flight()` sampled **before** the flush and never returns `IoError`. This is the pre-`9bd231d85` behaviour.
  - `clear_memory_tier` (`:2833-2864`) frees the DRAM slot when `try_evict_to_block` fails for **any** reason, including `ActiveReferences` (`:2855-2858`), and counts every candidate (`:2860`). This is the pre-`cccfb328e` bug: the dispatch map points at a released slot.
  - `remove` (`:2380-2381`) maps every `dm.remove` error to `KeyNotFound`, including `ActiveReferences` from the write-through pin installed by `downgrade_reference` at `:2609`. This is the pre-`0236a3c98` bug: removing a just-stored key fails.

**Proposed Resolution**: open a `dispatcher-p2p` repair (separate PR, its own spec-sync) that ports the three `dispatcher` repairs:
1. the `WriteThroughPins` ledger (`dispatcher/src/background.rs:250-389`) for `remove` takeover and the failed-write record;
2. pin-preserving `clear_memory_tier` with a bounded `Timeout` (`dispatcher/src/lib.rs:3940-4004`);
3. `flush_to_ssd` returning `IoError` on a failed write-through and the demotable count (`dispatcher/src/lib.rs:4006-4035`).

Reuse the Loom tests `repair_test_remove_no_wait.rs`, `repair_test_clear_mt_pinned.rs` and `repair_test_flush_failed_write.rs` as failing twins.

**Rationale**: The interface is the contract, and it was just made precise and approved; weakening it to fit the unrepaired implementor would re-admit data loss. Proposal 4's wording applies to every implementor.
**Confidence**: HIGH (code read directly; not reproduced at runtime in this session)
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 8: 001-interfaces/FR-023 — `LookupConfig` field count

**Direction**: BACKFILL

**Requirement**: FR-023 Supporting Types – Remote.

**Current State**:
- Spec says (spec.md:382): "`LookupConfig`: 10-field configuration for `IRemoteLookup::initialize` — `group`, `quorum_pct`, `phase1_timeout`, `op_deadline`, `max_retry_rounds`, `max_keys_per_query`, `bind_ip`, `actor_cpu`, `discovery` (optional `GossipConfig`, see FR-032), `node_endpoint`. Implements `Default`."
- Code does: 12 fields (`src/iremote_lookup.rs:29-79`), adding `caller_wait: Option<Duration>` (`:44-50`, default `None` = block until `op_deadline`) and `connection_teardown_timeout: Duration` (`:51-57`, default 1000 ms at `:89`). Both are specified in remote-lookup spec 002 (lines 376, 480, 490).

**Proposed Resolution**:

Old (spec.md:382):
```
- `LookupConfig`: 10-field configuration for `IRemoteLookup::initialize` — `group`, `quorum_pct`, `phase1_timeout`, `op_deadline`, `max_retry_rounds`, `max_keys_per_query`, `bind_ip`, `actor_cpu`, `discovery` (optional `GossipConfig`, see FR-032), `node_endpoint`. Implements `Default`.
```
New:
```
- `LookupConfig`: 12-field configuration for `IRemoteLookup::initialize` — `group`, `quorum_pct`, `phase1_timeout`, `op_deadline`, `caller_wait` (optional; how long `batch_lookup` blocks the caller, `None` = until `op_deadline`), `connection_teardown_timeout` (grace before an orphaned landing slot's peer QP is torn down and the slot reclaimed; default 1000 ms), `max_retry_rounds`, `max_keys_per_query`, `bind_ip`, `actor_cpu`, `discovery` (optional `GossipConfig`, see FR-032), `node_endpoint`. Implements `Default`. Semantics of `caller_wait` and `connection_teardown_timeout` are specified in remote-lookup spec 002.
```
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 9: 001-interfaces/FR-021 — `FormatParams` field count

**Direction**: BACKFILL

**Current State**:
- Spec says (spec.md:350): "`FormatParams`: 10-field format configuration with defaults."
- Code does: 9 fields (`src/iextent_manager.rs:42-67`), with defaults at `:79-92`.

**Proposed Resolution**:

Old (spec.md:350):
```
- `FormatParams`: 10-field format configuration with defaults.
```
New:
```
- `FormatParams`: 9-field format configuration with defaults (`data_disk_size`, `slab_size`, `max_extent_size`, `sector_size`, `region_count`, `metadata_alignment`, `instance_id`, `metadata_disk_ns_id`, `metadata_region_size`); `FormatParams::new(data_disk_size, instance_id)` fills the rest from `Default`.
```
**Rationale**: The code is authoritative: the struct is compiler-checked and its `Default` impl (`:79-92`) initialises exactly these 9 fields. Whether 10 was a miscount or a field was later removed was not established. Naming the fields keeps the count checkable.
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 10: 001-interfaces/Overview + FR-009 — `IMemoryTier` is not `spdk`-gated; feature count

**Direction**: BACKFILL

**Current State**:
- Spec says (spec.md:28): "The crate has two Cargo features:" (three are then listed). (spec.md:29): "**`spdk`** (optional): Gates SPDK-dependent interfaces and types (`IBlockDevice`, `IBlockDeviceAdmin`, `ISPDKEnv`, `IDispatcher`, `IDispatchMap`, `IMemoryTier`, `IExtentManager`, `IPartitionTable`) …". (spec.md:191): "#### FR-009: IMemoryTier Interface (feature: spdk)".
- Code does: `IMemoryTier`'s `define_interface!` (`src/imemory_tier.rs:53`) has no `cfg`, and the export at `src/lib.rs:51` is unconditional. `Cargo.toml:13-19` declares `integrity-check`, `spdk` and `gpu`.

**Proposed Resolution**:

Old (spec.md:28):
```
The crate has two Cargo features:
```
New:
```
The crate has three Cargo features:
```
Old (spec.md:29, the parenthesised list):
```
(`IBlockDevice`, `IBlockDeviceAdmin`, `ISPDKEnv`, `IDispatcher`, `IDispatchMap`, `IMemoryTier`, `IExtentManager`, `IPartitionTable`)
```
New:
```
(`IBlockDevice`, `IBlockDeviceAdmin`, `ISPDKEnv`, `IDispatcher`, `IDispatchMap`, `IExtentManager`, `IPartitionTable`; `IMemoryTier` is deliberately **not** gated, since its signatures use only raw pointers and crate-local types)
```
Old (spec.md:191):
```
#### FR-009: IMemoryTier Interface (feature: spdk)
```
New:
```
#### FR-009: IMemoryTier Interface
```
**Rationale**: The ungated form compiles in the default build, which is what the code ships. Gating it would break default-build consumers such as eviction-policy users. (The word "deliberately" is an inference from the signatures; reword it if the intent is unknown.)
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 11: 001-interfaces/NFR-002 — `gpu` feature gates nothing

**Direction**: BACKFILL

**Current State**:
- Spec says (spec.md:483): "GPU-specific code is gated behind `--features gpu`."
- Code does: no `#[cfg(feature = "gpu")]` anywhere in `src/**` (grep: 0 matches); `gpu = []` at `Cargo.toml:19`. `IGpuServices` and its types are ungated, and its SPDK-touching methods are gated on `spdk` (`src/igpu_services.rs:430, 458, 498, 656-770`).

**Proposed Resolution**:

Old (spec.md:483):
```
- GPU-specific code is gated behind `--features gpu`.
```
New:
```
- The `gpu` feature is declared but currently gates nothing (reserved, see Overview). `IGpuServices` and its types compile in every build; its methods that take SPDK `DmaBuffer`s are gated behind `spdk`.
```
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 12: 001-interfaces/Implementation Notes — verified-properties claim

**Direction**: BACKFILL

**Current State**:
- Spec says (spec.md:545): "Several interfaces have formally verified properties documented in comments (IDispatchMap: 10 props, IDispatcher: 10 props, IExtentManager: 10 props, IGpuServices: 10 props, IMemoryTier: 10 props)."
- Code does: only `src/igpu_services.rs:231-246` carries a "Verified Properties" block ("10 properties, 19 verification conditions", referring to `components/gpu-services/verif/`). The others were removed deliberately in `a93b5620f` (2026-08-20, "strip verification overclaims … verification status does not belong in the shared contract").

**Proposed Resolution**:

Old (spec.md:545):
```
- Several interfaces have formally verified properties documented in comments (IDispatchMap: 10 props, IDispatcher: 10 props, IExtentManager: 10 props, IGpuServices: 10 props, IMemoryTier: 10 props).
```
New:
```
- Verification status is not recorded in the interface definitions: per-property proof claims were removed from `IDispatchMap`, `IDispatcher`, `IExtentManager` and `IMemoryTier` in `a93b5620f` (2026-08-20) and live with each component's verification artifacts. `IGpuServices` still carries a comment summarising `components/gpu-services/verif/` (`src/igpu_services.rs:231-246`).
```
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 13: 001-interfaces/Backfill banner — FR-014/FR-025 "deferred"

**Direction**: BACKFILL

**Current State**:
- Spec says (spec.md:17-22): "**Deferred (not applied):** FR-014/FR-025 `IExtendedMetadataStore` remains an orphaned module — it is defined in `src/iextended_metadata_store.rs` but never declared/re-exported from `lib.rs`, so it is not part of the compiled crate. …"
- Code does: `mod iextended_metadata_store;` at `src/lib.rs:83`, and `pub use iextended_metadata_store::{ExtendedMetadataStoreError, IExtendedMetadataStore};` at `src/lib.rs:105`, both ungated. This is recorded as RESOLVED in `.specify/sync/align-tasks.md` (2026-08-20).

**Proposed Resolution**:

Old (spec.md:17-22):
```
> label corrected to 4-variant. **Deferred (not applied):** FR-014/FR-025
> `IExtendedMetadataStore` remains an orphaned module — it is defined in
> `src/iextended_metadata_store.rs` but never declared/re-exported from `lib.rs`,
> so it is not part of the compiled crate. A real consumer
> (`extended-metadata-store`) implements it and would fail to build; this latent
> break is masked only because that crate is excluded from the workspace.
```
New:
```
> label corrected to 4-variant. FR-014/FR-025 `IExtendedMetadataStore` was then
> an orphaned module (deferred). *(Resolved 2026-08-20: now declared and
> re-exported, ungated, at `src/lib.rs:83,105`; see `.specify/sync/align-tasks.md`.)*
```
**Rationale**: The note is historical, but it is written in the present tense and contradicts the current code.
**Confidence**: HIGH
**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal 14: 001-interfaces/NFR-004 — runnable doc examples

**Direction**: ALIGN (documentation task in `src/**`). An alternative is offered for a human decision.

**Current State**:
- Spec says (spec.md:492): "All public types and methods have doc comments with runnable examples." CLAUDE.md says the same.
- Code does: there are no doc examples in `src/idispatch_map.rs`, `src/imemory_tier.rs`, `src/ieviction_policy.rs`, `src/iextent_manager.rs` or `src/ipartition_table.rs`. Undocumented public items: `MemoryTierTelemetrySnapshot` fields (`src/imemory_tier.rs:13-15`), `ExtentManagerError` variants (`src/iextent_manager.rs:20-24`), `Extent` fields (`:12-14`), `FormatParams::new` (`:70`), `WriteHandle` and `key`/`extent_offset`/`extent_size`/`publish`/`abort` (`:95-146`). `missing_docs` is not denied in `src/lib.rs`, so `cargo doc` does not catch this.

**Proposed Resolution** (ALIGN): add doc comments to the items listed, and add an example per trait in the five modules (type-level examples can be runnable; trait-method examples `no_run`, as in `idispatcher.rs`). Optionally add `#![warn(missing_docs)]` to `src/lib.rs` so the rule is enforced.
**Alternative** (BACKFILL, needs a human choice): soften NFR-004 to "All public types have doc comments; examples are provided for value types and for at least one method per trait (`no_run` where hardware is required)."
**Rationale**: This is a project-wide coding convention, so the default is ALIGN. However, a gap this wide suggests the rule was never applied to these modules.
**Confidence**: MEDIUM
**Action**: - [ ] Approve ALIGN - [ ] Approve alternative - [ ] Reject
