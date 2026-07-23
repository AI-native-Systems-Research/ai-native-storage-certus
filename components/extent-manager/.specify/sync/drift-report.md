# Spec Drift Report — `extent-manager`

**Generated**: 2026-07-22T22:37:58Z
**Spec analyzed**: `components/extent-manager/specs/001-extent-manager-v2/spec.md` (Feature 001, "Extent Manager V2", Status: Active)
**Implementation**: `components/extent-manager/src/**`, `Cargo.toml`, `README.md`

## Summary

| Metric | Count |
|---|---|
| Specs analyzed | 1 |
| Requirements checked (FR + SC + on-disk-format entries) | 41 |
| Aligned | 37 |
| Drifted | 4 |
| Not implemented | 0 |
| Unspecced features found | 3 |
| Internal spec conflicts | 1 |

Overall the implementation is a very close match to the spec — all six user stories, all 34 functional requirements, and all six success criteria have corresponding, working code and test coverage (`tests/lifecycle.rs`, `tests/checkpoint.rs`, `tests/concurrent.rs`, `tests/edge_cases.rs`). Drift is confined to (a) documentation strings that disagree with the code they describe, and (b) three real features that ship in production call sites (`dispatcher`, `dispatcher-p2p`, `dispatch-map`) but have no corresponding requirement in the spec.

---

## Spec 001-extent-manager-v2: Extent Manager V2

### Aligned (37)

FR-001 through FR-029, FR-031 through FR-034, and SC-001 through SC-005 were verified against the implementation and match the spec text. Highlights:

- **FR-001** `define_component!` with `provides: [IExtentManager]`, receptacles `metadata_device: IBlockDevice`, `logger: ILogger` — `src/lib.rs:81-103`.
- **FR-002/FR-003** `format()` validation and superblock write — `src/lib.rs:383-512`.
- **FR-005–FR-009** two-phase reserve/publish/abort, FREE_KEY silent discard, `get_extents`/`for_each_extent` skip FREE_KEY — `src/lib.rs:584-684`, `src/region.rs:94-150`.
- **FR-010–FR-012** per-slab `Vec<u64>` key vectors, `FREE_KEY = u64::MAX` sentinel, `BTreeMap<u64, Slab>` keyed by `start_offset` with `range(..=offset).next_back()` lookup — `src/slab.rs:7-27`, `src/region.rs:121-127`.
- **FR-013–FR-015, FR-017, FR-018** checkpoint serialization, dirty-skip, coalescing (single in-flight writer via `in_progress` bool), dual-copy fallback recovery — `src/checkpoint.rs`, `src/region.rs` (`dirty`), `src/lib.rs:729-761`, `src/recovery.rs`.
- **FR-019–FR-022** buddy allocator per region, bitmap+rover slab allocator, `SizeClassManager` HashMap with documented linear-cleanup edge case, `key & (region_count-1)` sharding — `src/buddy.rs`, `src/slab.rs`, `src/region.rs:41-92`, `src/lib.rs:211-221`.
- **FR-023–FR-025** per-region `parking_lot::RwLock`, deferred free via `pending_frees` flushed only after a successful checkpoint — `src/region.rs:16, 121-160`, `src/lib.rs:312-321`.
- **FR-026–FR-029, FR-031–FR-034** `get_instance_id`, `set_checkpoint_interval(None)` fully disabling the timer, `set_metadata_ns_id`/`set_dma_alloc` as inherent (non-trait) methods, `used_bytes`/`capacity_bytes` at slab/buddy granularity, `WriteHandle` defined in `interfaces` crate, `testing`-gated `FaultConfig` fault injection (write-only, read faults explicitly unsupported) — `src/lib.rs:169-183, 686-727`, `components/interfaces/src/iextent_manager.rs:95-167`, `src/test_support.rs:15-19, 158-242`.
- **SC-001–SC-005**: exercised by `tests/lifecycle.rs`, `tests/checkpoint.rs` (dual-copy corruption test `corrupt_active_falls_back_to_previous`), `tests/concurrent.rs` (8 threads × 100 ops matching User Story 4's acceptance scenario exactly), and `benches/benchmarks.rs` (enumerate benchmark tops out at 100,000 extents, consistent with SC-005's "not yet verified by benchmark" caveat for the 100M target).

### Drifted (4)

| Requirement | Spec text | Actual | Location | Severity |
|---|---|---|---|---|
| FR-016 | "the `IExtentManager` interface doc comment incorrectly states 'five minutes'" — spec discloses one wrong doc string but implies that's the only one. | The same "five minutes" claim is *also* in `README.md`'s feature list ("Background periodic checkpoint thread (configurable interval, default 5 minutes)"), a second undisclosed stale-doc location. Actual default is 30s (`ExtentManager::new_inner`). | `components/extent-manager/README.md:12` vs `components/extent-manager/src/lib.rs:109-112` | Low |
| FR-030 | "When enabled, flush calls in checkpoint and format paths are conditionally compiled out, improving performance..." | Polarity is inverted and scope is wrong: the `#[cfg(feature = "volatile_write_cache")]` guard *adds* the flush call when the feature is enabled (absent by default) — i.e. enabling the feature makes flushing happen, not stop happening. Also, no flush call of any kind exists in the `format()` path; both feature-gated flush sites are inside the checkpoint path only. | `components/extent-manager/src/checkpoint.rs:99-103`, `components/extent-manager/src/lib.rs:308-310` (checkpoint path); `components/extent-manager/src/lib.rs:383-512` (`format()`, no flush call at all) | Medium |
| SC-006 | "Checkpoint coalescing limits concurrent checkpoint I/O to at most two active operations regardless of caller count." | Only one `run_checkpoint()` call is ever physically in flight — `checkpoint_coalesce.in_progress` is a `bool`, not a counter, so exactly zero or one checkpoint executes at any instant. "Two" describes the max number of *sequential* rounds a burst of waiters may need (via `completed_seq + 2`), not concurrently-active I/O. The wording contradicts FR-015 ("at most one actual checkpoint I/O operation executes at a time... serializes all checkpoint I/O through a single writer") if read literally. `README.md:13` repeats the same ambiguous phrasing ("at most two IO rounds regardless of concurrent callers"). | `components/extent-manager/src/lib.rs:729-761` | Medium |
| Key Entities (Superblock) | Spec states format version is `6` ("Magic: `0x4345_5254_5553_5634` ('CERTUSV4'), version 6") and code agrees (`FORMAT_VERSION: u32 = 6`, `src/superblock.rs:6`). | `README.md` still says "format version (5)" — stale from a prior format revision, disagrees with both spec and code. | `components/extent-manager/README.md:19` vs `components/extent-manager/src/superblock.rs:6` | Low |

### Not Implemented

None. Every FR and SC has corresponding, exercised code.

---

## Unspecced Code

| Feature | Location | Lines | Suggested spec |
|---|---|---|---|
| `FormatParams.metadata_region_size` and the shared metadata/data-device co-location mode (non-zero value shifts `data_start_offset` past the checkpoint regions so data and metadata can share one NVMe namespace) | `components/interfaces/src/iextent_manager.rs:62-66` (field); `components/extent-manager/src/lib.rs:425-446` (`format()` branch), `src/lib.rs:530` (`initialize()` reconstruction) | ~40 | New FR under "Initialization & Format": document `metadata_region_size` semantics, default (128 MiB), and the `data_start_offset` computation when metadata and data coexist on one device. |
| Partition-relative base-LBA addressing: `set_metadata_base_lba(u64)`, `set_data_base_lba(u64)`, `data_base_lba() -> u64` on `IExtentManager`, letting the component operate on a sub-range of an NVMe namespace instead of owning it entirely. Used in production by `components/dispatcher/src/lib.rs`, `components/dispatcher-p2p/src/lib.rs`, and `components/dispatch-map/tests/integration.rs`. | `components/interfaces/src/iextent_manager.rs:253-260`; `components/extent-manager/src/lib.rs:717-727` | ~15 | New FR under "Instance & Configuration": partition offset support for shared-namespace deployments. |
| `set_post_checkpoint_hook(Arc<dyn Fn() + Send + Sync>)` — inherent method registering a callback invoked once after each successful `checkpoint()` completes; used by `components/dispatcher/src/lib.rs:1348` and `components/dispatcher-p2p/src/lib.rs:943` (presumably for cross-node sync/invalidation after durability is established). | `components/extent-manager/src/lib.rs:173-175` (setter), `src/lib.rs:365-367` (invocation) | ~6 | New FR under "Persistence & Recovery": post-checkpoint hook callback mechanism. |

---

## Conflicts

1. **FR-015 vs SC-006 (internal spec inconsistency)** — FR-015 asserts single-writer serialization ("at most one actual checkpoint I/O operation executes at a time... serializes all checkpoint I/O through a single writer"), while SC-006 asserts "at most two active operations" of "concurrent checkpoint I/O". The code (`src/lib.rs:729-761`) implements FR-015's semantics exactly (one `bool` in-flight flag); SC-006's "two" only makes sense as "at most two sequential checkpoint rounds needed to drain a burst of concurrent callers," which is a materially different claim from "two active/concurrent operations." `README.md` inherits SC-006's ambiguous phrasing. Recommend rewording SC-006 to remove "concurrent"/"active operations" and instead say "a burst of concurrent callers is satisfied by at most two sequential checkpoint executions."

---

## Recommendations

1. Fix `README.md`: default checkpoint interval is 30s (not "5 minutes"), format version is 6 (not "5"), and reword the coalescing bullet to avoid implying two checkpoints can run concurrently.
2. Fix the `IExtentManager::set_checkpoint_interval` doc comment in `components/interfaces/src/iextent_manager.rs:244` ("The default is five minutes") to say 30 seconds, or remove the specific claim and point to `ExtentManager::new_inner()`.
3. Reword FR-030 in `spec.md` — the feature flag *adds* flush-after-write behavior when enabled (for volatile write caches), and today it only affects the checkpoint path, not `format()`. Either fix the wording or add a flush call to `format()`'s superblock write if that was the original intent.
4. Add FRs for `metadata_region_size`/shared-device mode, `set_metadata_base_lba`/`set_data_base_lba`/`data_base_lba`, and `set_post_checkpoint_hook` — all three are used by production components (`dispatcher`, `dispatcher-p2p`, `dispatch-map`) and should not be undocumented.
5. Clarify SC-006's wording per the Conflicts section above so it cannot be read as contradicting FR-015.
