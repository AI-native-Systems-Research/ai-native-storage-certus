# Align Tasks — dispatcher (code-side follow-ups)
Generated: 2026-08-31 (sync on branch `sync-tmp`)

This sync produced **no ALIGN items** (all six findings were code-authoritative
BACKFILL/BACKFILL-UNSPECCED, resolved by `specs/**` edits). The item below was a
documentation-only source cleanup that was outside this sync's editable scope
(`.specify/sync/**` + `specs/**`), recorded here for a follow-up source pass.

## T1 — ✅ RESOLVED (2026-09-21) — stale transport-example source comments removed

- **Status**: Resolved. No follow-up remains. The stale transport-specific
  comments in `copy_gpu_to_memory_async`'s null-stream branch were removed in
  commit `6d7ba234` ("revert dispatcher GPU experiments to specified stream
  model"). Verified 2026-09-21: `grep -rin grpc components/dispatcher/src`
  returns nothing, so the dispatcher source no longer references the removed
  control transport anywhere. The canonical spec's FR-040 / FR-042 already
  describe the shm-queue control transport (`2026-08-31` sweep).
- **Original item** (now closed): two comments described the null-stream caller
  with a transport-specific example that named the pre-`97e26738` control
  transport; the guidance was to reword to the shm-queue control handler or drop
  the example. Superseded by the `6d7ba234` deletion above.

## Task DISP-RESERVE-REASON (OPEN, Low) — "already cached" is reported as a failure

`op_reserve` writes a flat `0` for any `Err`, so a reserve refused because the key is
**already resident** is indistinguishable on the wire from one refused for want of space.
Measured: 7 224 of 121 430 reserves refused, **100% of them already-resident**, with
capacity-driven refusals at exactly **0** (`store_backpressure_events` and
`store_drops_on_full` both 0, `eviction_scans_exhausted` 0).

The same `mt.insert` → `AlreadyExists` is treated as a **hit** on the load path
(`serve_concurrently_promoted`, whose contract is that observing `MemoryTier` means the
data is resident) and as a **failure** on the store path. A declined block stays in GPU
HBM, so ~6% of stores currently waste device memory for data that is already safely
cached.

Options: carry a reason code on the wire, or report already-resident as reserved. Either
changes the client contract, hence deferred. Specified in **interfaces FR-018a**;
`store_already_resident` measures it.

---

# Apply 2026-10-14 — branch `fix/bundle-dispatcher-ems` (approved by Cornel, 2026-10-18)

Source: `.specify/sync/proposals.json` / `proposals.md` (analysis run 2026-10-18, HEAD
`4f8b5554b`). Each approved ALIGN proposal below carries the approval note *"approved as
an ALIGN task only: record in align-tasks.md, no code change in this PR"*. **No code was
changed.** Line numbers refer to HEAD `4f8b5554b`.

## Task: Align 002-served-by-tier-attribution/US1 AS3 + FR-009 (A-01, OPEN, major)

**Spec Requirement**: 002 US1 AS3 / FR-009. When the batched GPU sync fails, every key whose copy was in flight is attributed `ERROR`, and none is reported as a hit (002 spec.md:303-305, 430-434).
**Current Code**: a failed warm `stream_synchronize` is only logged. The warm results stay `Ok` with tier `Dram`, so they are counted as hits and sent as wire byte 1 (`src/lib.rs:2750-2755`).
**Required Change**: record the warm-hit indices as their copies are submitted (`src/lib.rs:2420-2431`). On a sync failure, set each of them to `Err(IoError("batch stream_synchronize failed: …"))`, mirroring the remote-delivery block (`src/lib.rs:2907-2917`). Leave `tier[i]` unchanged; the final mapping (`:2982`) then yields `Error`.
**Files to Modify**: `components/dispatcher/src/lib.rs` plus a test module
**Estimated Effort**: small

### Acceptance Criteria
- [ ] A test using `PinProbe::failing_sync` on warm keys asserts `ServedBy::Error` and `!result.is_ok()`.
- [ ] Warm keys whose sync failed are not counted in `promotions_to_gpu` / `lookup_hits_dram`.

## Task: Align 001-dispatcher-cache-interface/FR-051 + FR-036: `batch_lookup` cold pin (A-02, OPEN, major)

**Spec Requirement**: 001 FR-051 ("The caller's load pin keeps the entry from being evicted while waiting"), and FR-036 as amended by B-01 (the pin is kept through promotion).
**Current Code**: `let _ = dm.release_read(key);` runs right after BlockDevice classification (`src/lib.rs:2433-2434`). `evict_and_insert`, the NVMe read and `promote_block_to_memory_tier` then run unpinned (`src/lib.rs:2562-2717`), and the staging and no-drives paths do the same. `promote_to_memory_tier` repeats the pattern (`src/lib.rs:3726-3729`).
**Required Change**: adopt each cold key's classification pin into a `pins::PinnedKeys` guard, as warm hits do (`src/lib.rs:2356`, `:2428`). Drop the guard after the cold block and the staging post-pass, before the recovery pass. *Alternative:* document "the caller holds a pin for every key passed to `batch_lookup`" as a precondition in 001 FR-039 and in the interfaces trait docs.
**Files to Modify**: `components/dispatcher/src/lib.rs`, `src/repair_test_cold_pin.rs` (or a new test module); for the alternative, `specs/001-…/spec.md` and `components/interfaces/src/idispatcher.rs`
**Estimated Effort**: medium

### Acceptance Criteria
- [ ] `repair_test_cold_pin`'s concurrent-remove test is ported to `batch_lookup` and passes. It must fail before the fix.
- [ ] The shm-queue `LOOKUP` path (`lib/shmq-dispatcher/src/translate.rs:756`) is covered.

## Task: Align 001-dispatcher-cache-interface/FR-038 `clear_memory_tier` (A-03, OPEN, major)

**Spec Requirement**: 001 FR-038. Clearing must not leave a dispatch-map entry that points at a freed slot.
**Current Code**: on *any* `try_evict_to_block` failure, `ActiveReferences` included, the code calls `mt.remove(key)` unconditionally and then `dm.remove(key)`. The map refuses that removal while the entry is pinned, which leaves a dangling `MemoryTier` pointer (`src/lib.rs:3875-3890`). The path is reachable over shm-queue `CLEAR_MEMORY_TIER`.
**Required Change**: match on the error. For `ActiveReferences`, skip the key and leave it resident. For anything else, call `dm.remove` first and `mt.remove` only if it succeeds. Bound the outer loop. In the same change, update the FR-038 wording (`evict_next` → `oldest_keys`; `convert_memory_tier_to_block` → `try_evict_to_block`; "pinned entries are skipped and remain resident").
**Files to Modify**: `components/dispatcher/src/lib.rs`, `specs/001-dispatcher-cache-interface/spec.md` (FR-038)
**Estimated Effort**: small

### Acceptance Criteria
- [ ] A test that pins a key and then calls `clear_memory_tier` sees the key still resident and readable.
- [ ] `clear_memory_tier` terminates when every resident key is pinned.

## Task: Align 001-dispatcher-cache-interface/FR-028 + US-9 AS3: no-drives promote (A-04, OPEN, moderate)

**Spec Requirement**: 001 FR-028 / US-9 AS3. The reader MUST re-insert the entry into the memory tier and re-register it as a MemoryTier entry (001 spec.md:305, :166).
**Current Code**: `dm.create_memory_tier_entry` on an existing BlockDevice entry returns `AlreadyExists`, which is ignored. `dm.release_write` is then called without a write ref. The slot is orphaned and the entry stays BlockDevice (`src/lib.rs:814-820`).
**Required Change**: use `dm.promote_block_to_memory_tier(key, mem_ptr, ipc_handle.size)`, as `batch_lookup`'s no-drives branch does (`src/lib.rs:2495-2500`). Drop the stray `release_write`. Make `MockDispatchMap::create_memory_tier_entry` (`src/lib.rs:4428-4451`) refuse to overwrite.
**Files to Modify**: `components/dispatcher/src/lib.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] After a no-drives single-key cold lookup, the entry is `MemoryTier` and the slot is referenced.
- [ ] The mock refuses an overwrite, and the existing tests still pass.

## Task: Align 001-dispatcher-cache-interface/FR-003 + FR-056: phase-3 release in `copy_gpu_to_memory_completed` (A-05, OPEN, moderate)

**Spec Requirement**: 001 FR-003 as amended by B-03. Every failure before the entry is registered MUST release the reserved slot, and no failure may leave a dangling entry.
**Current Code**: under `integrity-check`, a `stream_synchronize` or `set_checksum` failure after `create_memory_tier_entry` calls `mt.remove(key)` but leaves the new dispatch-map entry write-locked and pointing at the freed slot. A `downgrade_reference` failure leaves it write-locked (`src/lib.rs:3599-3620`).
**Required Change**: on these paths, undo the registration (`release_write` and then `dm.remove`) **before** `mt.remove`.
**Files to Modify**: `components/dispatcher/src/lib.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] A test under `--features integrity-check` with a failing sync leaves neither a dispatch-map entry nor a slot.

## Task: Align 001-dispatcher-cache-interface/FR-042: SSD evictor sender wiring (A-06, OPEN, moderate)

**Spec Requirement**: 001 FR-042 as amended by B-05. The SSD evictor emits `Removed`, and its dropped events are counted.
**Current Code**: `BackgroundEvictor::start` receives a one-time clone of `eviction_tx` at `initialize()` (`src/lib.rs:2059-2071`). Both servers create the channel after `initialize` (`apps/certus-server-yaml/build.rs:447-471`; `apps/certus-server/src/main.rs:469`, `:529`), so the evictor holds `None` forever.
**Required Change**: pass `Arc::clone(&self.eviction_tx)`, as `MemoryTierEvictor` does (`src/lib.rs:2101`, `src/background.rs:462`), and take the lock on each emit.
**Files to Modify**: `components/dispatcher/src/lib.rs`, `components/dispatcher/src/background.rs`
**Estimated Effort**: small

### Acceptance Criteria
- [ ] A test that creates the channel after `initialize` sees a `Removed` event arrive, or sees it counted in `eviction_dropped_count()`.

## Task: Align 002-served-by-tier-attribution/FR-027 + SC-009: per-value attribution tests (A-08, OPEN, moderate)

**Spec Requirement**: 002 FR-027 / SC-009. Each attribution value MUST have a test that fails if that value is mis-assigned (002 spec.md:554-555).
**Current Code**: value-specific assertions exist only for `Dram` (`src/repair_test_served_by.rs:484`) and `Miss` (`src/lib.rs:5927`).
**Required Change**: add one test for each remaining value: `Ssd` (a no-drives cold key), `Remote` (`setup_initialized_with_remote`), `SizeMismatch` (`MockDispatchMap::set_mismatch_key`, `src/lib.rs:4200`) and `Error` (the A-01 warm-sync failure, plus the not-initialized path, `src/lib.rs:2238-2251`).
**Files to Modify**: dispatcher test modules
**Estimated Effort**: small

### Acceptance Criteria
- [ ] Each new test fails against a deliberately wrong mapping (FR-028).

## Open items (not approved — awaiting a decision)

### OPEN A-07 — 001/FR-058 `store_already_resident` counts load-path race losers (minor)
> Approval note: *"waits for Cornel's decision (ALIGN as a task, or BACKFILL); record in align-tasks.md as an open decision"*

- **Option ALIGN**: move `record_store_already_resident()` out of `evict_and_insert` (`src/lib.rs:1213-1222`) into `reserve_memory`'s `Err(AlreadyExists)` handling.
- **Option BACKFILL**: in FR-058, replace "reserves refused because the key was already cached" with "`mt.insert` calls refused because the key was already resident, on the store path (a refused reserve) and on the load path (a lost concurrent promotion, FR-051)". Interfaces FR-018a then needs the matching change.
- Related: DISP-RESERVE-REASON above.

### OPEN N-01 — new 001/FR-062 `integrity-check` verify-before-serve (MEDIUM confidence)
> Approval note: *"MEDIUM confidence new requirement: record as an open item in align-tasks.md (Cornel's rule)"*

- Draft FR-062 text: see `proposals.md` § N-01. CRC-32 is computed after the store D2H (`src/lib.rs:3593-3617`) and checked before every warm, remote and single-key H2D. A mismatch returns `IoError` and is attributed `ERROR`. Cold SSD promotions are not verified.

### OPEN N-04 — 001/FR-009 + `contracts/errors.md`: `remove` on a pinned entry (MEDIUM confidence)
> Approval note: *"MEDIUM confidence: record as an open item; it belongs to the remove-no-wait PR"*

- Draft: a pinned or write-locked entry → `remove` frees nothing and returns `KeyNotFound`, and a dispatch-map timeout is reported as `KeyNotFound` as well. Drop `remove` from the `Timeout` row of `contracts/errors.md:37`. *Alternative (ALIGN)*: return `Timeout` or a distinct error, so callers can tell busy from absent. Deferred to the remove-no-wait PR.

