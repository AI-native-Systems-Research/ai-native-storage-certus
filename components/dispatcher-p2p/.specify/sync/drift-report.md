# Spec Drift Report
Generated: 2026-07-21
Project: dispatcher-p2p
Base commit: 833e9f36e01f1df8a0e0fc57d5cd223d823d3199..HEAD (scope: `src/lib.rs`)

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 14 |
| Aligned | 14 (100%) |
| Drifted | 0 (0%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 1 |

## Change Under Analysis

`git diff 833e9f36...HEAD -- components/dispatcher-p2p/src/lib.rs` (35 insertions, 10 deletions):

- **Substantive (1 change)**: `MockGpuServices` test-mock impl gained two new methods to satisfy the expanded `IGpuServices` trait:
  - `fn set_device(&self, device: i32) -> Result<(), String>` (`src/lib.rs:3091-3093`)
  - `fn device_of_ptr(&self, ptr: *const std::ffi::c_void) -> Result<i32, String>` (`src/lib.rs:3094-3096`)
- **Non-substantive**: the remaining ~40 lines are pure `rustfmt` reflow (multi-line argument/struct-literal wrapping, `.ok()`/`.is_err()` method-chain breaks) with no behavioral effect.

The two new methods originate in the `IGpuServices` interface (`components/interfaces/src/igpu_services.rs:555,577`), added as part of the repo-wide multi-GPU work also visible in the standard dispatcher (`components/dispatcher/src/lib.rs:3478,3481` and its tests/benches). In dispatcher-p2p the change is currently confined to the **test mock** — production cold-path code in `src/lib.rs` does not yet call `set_device`/`device_of_ptr` (grep finds them only in the mock at 3091/3094). The component thus now *depends on* an `IGpuServices` receptacle that exposes per-device selection, but does not yet exercise per-device routing on the P2P path.

## Detailed Findings
### Spec: 001-gpudirect-cold-path - GPUDirect Storage Cold Path

#### Aligned
- FR-001 .. FR-014: All functional requirements remain aligned with `src/lib.rs`, `src/pipeline.rs`, `src/p2p_ring.rs`, and `src/background.rs`. The prior FR-007 and FR-009 drifts recorded on 2026-07-15 were resolved in spec.md (FR-007 now documents the single-key `lookup()` DRAM fallback for test/staging; FR-009 now documents the lazy `DramBackfillWorker`; FR-014 documents `backfill_delay_ms`). Nothing in the analyzed diff alters these.

#### Drifted
(none) — the diff introduces no behavior that contradicts an existing FR.

#### Not Implemented
(none)

## Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| `IGpuServices` receptacle now requires multi-GPU device selection (`set_device`, `device_of_ptr`) for cold-path P2P routing; currently satisfied by the test mock, production wiring pending | `src/lib.rs:3091-3096` (mock); interface `interfaces/src/igpu_services.rs:555,577` | ~6 | Add FR-015 |

Note: the 2026-07-15 report's remaining unspecced items (MemoryTierEvictor, background SSD evictor/write-through, prepare/commit/cancel_store, fallback pipelines) are unchanged by this diff and are carried forward as pre-existing recommendations (MemoryTierEvictor previously suggested at FR-016+).

## Inter-Spec Conflicts

- none

## Recommendations

1. **Add FR-015** (BACKFILL, code authoritative): "The component's `IGpuServices` receptacle MUST provide multi-GPU device selection — `set_device(device)` to bind the active CUDA device and `device_of_ptr(ptr)` to resolve the GPU a device pointer resides on — so cold-path staging-ring D2D copies and CUDA streams can target the client destination's GPU in multi-GPU deployments."
   - Confidence: High for the interface dependency (present in code and mock). Medium for the routing intent — production P2P code does not yet call these methods, so the FR documents an available capability whose cold-path wiring is pending.
2. Pre-existing (unchanged by this diff): the MemoryTierEvictor and inherited background-worker features remain candidates for future FRs (MemoryTierEvictor at FR-016+).
