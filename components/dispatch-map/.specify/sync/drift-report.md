---
spec_sync_component: dispatch-map
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-21T23:03:30Z
spec_sync_git_commit: efd502331
spec_sync_inputs_sha256: 37a89b4b2eb2579122605aea228bc8029a2dfb4197586858901a15c7858a53a8
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Re-stamp 2026-10-21 (transitive, `components/interfaces` changed on branch `fix/dispatcher-remove-clear-mt`).** Only `components/interfaces` changed for this component: `IDispatcher` doc comments (`flush_to_ssd` errors, `clear_memory_tier`, `batch_lookup`) and the interfaces spec (FR-008, FR-009, FR-017, FR-018, FR-018a, FR-021, FR-023, Overview, NFR-002, Implementation Notes; text only, no type or signature changed). This component uses none of the changed items (grep over its src/ and specs/: 0 hits). This component's last full sync below stands; no re-analysis was needed.

> **Re-stamp 2026-10-14 (interfaces FR-027 fold-in, branch `fix/cold-staging-buf-16mib`).** `components/interfaces/{src,specs}` folds into this component's spec-sync digest, so correcting interfaces FR-027 (`DispatcherConfig::cold_staging_buf_bytes` default 4 MiB → 16 MiB, matching commit `8e12e49f`) moves this component's hash. dispatch-map's own reconciliation is current — its `src/specs` were fully synced and stamped clean by PR #542 (merged into this branch; see the note below) — and it does not reference `cold_staging_buf_bytes` (consumed only by the dispatcher's host-DRAM cold-staging pool), so no re-analysis was needed for the interfaces change. Digest refreshed to match the committed merged tree.

> **Apply 2026-10-14 (branch fix/dispatch-map-get-pool-id-panic, approved by Cornel).** All 10 proposals were `approved: true` and were applied in batch mode; no source code changed. **Applied (spec text, exact old→new from `proposals.md`)**: P1 FR-003 and P2 FR-024 (`spec.md` + contract rows now list `NotInitialized`, no entry recorded on error), P3 FR-017 (`oldest_keys` returns an empty list when no `IEvictionPolicy` is connected), P4 (`data-model.md`: `NotInitialized` is no longer "reserved"), P5 FR-026 (contract + `data-model.md` now say `ActiveReferences` for held refs on `try_evict_to_block`), P10 (new 2026-10-14 **Last Synced** note in `spec.md`; the earlier notes are kept as history). Backups are in `.specify/sync/backups/20261014T181140Z/`. **Became tasks (ALIGN, per approval note "no code change in this PR")**: P6 null pointer (FR-003/US1-AS3), P7 stale cached pool id / `track(...).unwrap()` under the map lock, P8 `initialize()`-unbound test (FR-012), P9 `entry_size` `u32` overflow (FR-023); see `.specify/sync/align-tasks.md`. **Stays open**: no unapproved proposals. The four code-side drifts above (P6–P9) stay open until they are implemented, so the drift status is unchanged. Details: `apply-report.md` / `apply-report.json`.
>
> **Stamp: clean.** Every drift item of this analysis is resolved (spec updates applied with Cornel's approval) or tracked in `align-tasks.md` (ALIGN tasks and open items), as in earlier `clean` stamps of this and other components (e.g. dispatcher-p2p).

> **Sync 2026-10-14 (branch fix/dispatch-map-get-pool-id-panic) — full FR/SC re-analysis after fix `c877b74e`.** Static analysis only: SPDK is not built in this worktree, so the tests could not be run. Inputs: `git diff origin/unstable.. -- src`, all of `src/`, `tests/`, `spec.md`, `contracts/idispatch_map.md`, `data-model.md`. Checked 33 requirements (27 FR + 6 SC): 29 aligned, 4 drifted (FR-003, FR-023, FR-024, FR-026), 0 not implemented. One more drift item isn't tied to an FR (the "no panics" rule).
>
> - **Fix confirmed.** FR-012 (`spec.md:251`) is aligned. `get_pool_id` returns `Result` (`src/lib.rs:51-63`), `initialize` returns `NotInitialized` when unbound (`:77`, `:78-81`), and no `eviction_policy.get().unwrap()` is left in `src/lib.rs`. Gap: no test calls `initialize()` with no policy bound. The four tests in `src/repair_test_get_pool_id.rs` cover only `oldest_keys`, `create_memory_tier_entry` and `recover_extent`.
> - **Docs behind the fix:** `NotInitialized` from `create_memory_tier_entry`/`recover_extent` (`:403-407`, `:588-592`) isn't listed in FR-003/FR-024 (`spec.md:242,263`) or contract rows `:45,:60`. `data-model.md:82` still says `NotInitialized` is "not currently returned". FR-017 doesn't say `oldest_keys` returns empty when unbound (`:383-385`).
> - **New since the 10-13 note:** (1) `entry_size` multiplies `size_blocks * 4096` in `u32` (`src/lib.rs:379`). A size above 4 294 963 200 bytes (or `recover_extent` with `size_blocks >= 2^20`) overflows: it panics in debug builds and returns `0` in release (FR-023, contract `:79`). (2) For `try_evict_to_block`, the contract row (`contracts/idispatch_map.md:49`) and `data-model.md:78,86` still say held references give `InvalidState`. The 2026-10-09 switch to `ActiveReferences` (`src/lib.rs:558-560`, `spec.md:265`) never reached those files. (3) The stale-pool-id panic from the 10-13 note is worse than it said. `ep.track(...).unwrap()` (`:101-103`, `:414`, `:598`) runs *while the map mutex is held* (`:97`, `:409`, `:594`), so the panic poisons the mutex and every later call panics too. If the newly connected policy already has that pool index, entries are silently tracked in another component's pool.
> - **Carried over:** the FR-003/US1-AS3 null-pointer check is still missing (`src/lib.rs:399-401` checks only `size`).
>
> Proposals: 10 (backfill 5, align 4, supersede 1), all `approved: false`, none applied. Verdict: drift.

> **Sync 2026-10-13 (branch fix/dispatch-map-get-pool-id-panic, fix: unbound `IEvictionPolicy` returns `NotInitialized` instead of panicking).** Scope: `src/lib.rs` (`get_pool_id`, `initialize`, `oldest_keys`, `create_memory_tier_entry`, `recover_extent`) and new `src/repair_test_get_pool_id.rs`. Findings:
>
> - **Resolved:** FR-012 (`spec.md:251`) and contract "no panics" (`contracts/idispatch_map.md:79`). `get_pool_id` now returns `Result` (`src/lib.rs:51-63`); `initialize` propagates it (`:77`). `oldest_keys` returns empty (`:383-385`), as FR-017 (`spec.md:256`) allows.
> - **Minor doc gap:** `create_memory_tier_entry`/`recover_extent` can now return `NotInitialized` (`:403`, `:588`), but FR-003/FR-024 (`spec.md:242,263`) and contract rows `:45,:60` don't list it.
> - **Still drift:** `ep.track(...).unwrap()` (`:103`, `:414`, `:598`) panics with `InvalidPool` when a different policy is connected after the pool id was cached, so the contract's "no panics" (`:79`) is still broken on these paths. There is no test that `initialize` errors when unbound (align-tasks FR-012 checklist). The FR-003 null-pointer check (US1-AS3) is still missing.
>
> Verdict: drift.

> **Sync 2026-10-09 (branch `fix/evict-blocked-by-pin`).** Delta analysis on a certified baseline: every stamp on `origin/unstable` was re-verified to equal its recomputed spec-sync hash, so the only new inputs are this branch's changes.
>
> - **FR-026 / `try_evict_to_block` scenario 2 — CODE + SPEC.** A held read or write reference now returns `ActiveReferences(key)` (`src/lib.rs`, the refs check in `try_evict_to_block`), as `remove` (FR-011) already did; it was `InvalidState`. Spec text updated to match. The consumer's contract required this: dispatcher spec 002 FR-033 counts `evictions_blocked_by_pin` on `ActiveReferences`, so with `InvalidState` every pinned candidate was counted as unpersisted.
> - **Tests.** Six new unit tests against the real component cover every `try_evict_to_block` outcome (demote, read pin, write ref, no ssd_offset, already BlockDevice, missing key); there were none.
> - Other `try_evict_to_block` callers only test `is_ok()` / `is_err()` and are unaffected.
>
> No actionable drift remains for this component after apply.

> **Re-stamp 2026-10-05 (transitive: `components/interfaces` changed, branch `fix/poller-cpu-placement-logging`).** The only change to this component's hashed inputs is a doc-comment correction on `DispatcherConfig::poller_base_cpu` in `components/interfaces/src/idispatcher.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and it does not reference `poller_base_cpu` (verified by grep over its src/ and specs/). Its previous stamp was re-verified to match `git archive origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

> **Re-stamp 2026-10-01 (transitive: `components/interfaces` changed).** The spec-sync digest
> folds `components/interfaces/{src,specs}` into **every** component's hash, so an interface
> change invalidates all of them at once -- which is the design, not a defect. The interface
> change is the `served_by` attribution work: new `ServedBy` and `LookupOutcome` types, and
> `IDispatcher::batch_lookup` widened to return `Vec<LookupOutcome>`.
>
> **No re-analysis was performed for this component, and the digest bump asserts only what was
> actually checked**: the interface change is additive except for `batch_lookup`'s return type,
> which the compiler enforces across all implementors, and this component's own `src/**` and
> `specs/**` are unchanged. Components that implement `IDispatcher` (`dispatcher`,
> `dispatcher-p2p`) had their specs updated substantively; this one did not need it. If a later
> sweep finds drift here, this stamp is not evidence against it.

> **Re-stamp 2026-09-15 (interfaces-fold; no drift).** Branch `fix-reserve-batch-deadline` (`bec6c6ec`) added a `deadline: Option<std::time::Instant>` parameter to `IDispatcher::reserve_memory` in `components/interfaces/src/idispatcher.rs` (shared batch backpressure deadline for the shm-queue OP_RESERVE handler). `scripts/spec-sync-hash.sh` folds the whole `components/interfaces` tree into every component's hash, so this component's digest moved even though its own spec/implementation did not change. This component does not describe or call `reserve_memory`; the interface delta cannot affect its spec↔implementation alignment. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

> **Re-stamp 2026-09-09 (merged-branch interfaces-fold; no drift).** Branch
> `fix-dispatcher-store-backpressure` was merged into `unstable` at `3411518a`; that
> branch adds `DispatcherConfig::store_backpressure_ms` plus two `TierEventStats`
> counters (`store_backpressure_events`, `store_drops_on_full`) to
> `components/interfaces/src/idispatcher.rs`. `scripts/spec-sync-hash.sh` folds
> the whole `components/interfaces/` tree into every component's hash, so this
> component's digest moved even though its own `src/`+`specs/` are byte-for-byte
> unchanged and it references none of those new dispatcher symbols (verified by
> grep across `components/` and `lib/`). The merge's conflict resolution had
> reverted this stamp to unstable's `b220a1c8` value; the digest is recomputed
> here at merged HEAD. Drift status remains `clean`; the report body stands
> verbatim.

> **Refresh 2026-09-09 (restamp only — no drift, no spec change):** the stamp was
> refreshed to input hash
> `7f74b8e980be9afe665117e3ba9100092073f8f7960e78ad9620f0656cb810a7` at commit
> `5bb71702`. The previous stamp (`2dbb7666…`) went stale for two reasons, neither
> of which is dispatch-map drift: (1) `components/dispatch-map/specs/001-dispatch-map/spec.md`
> was already backfilled to the global-lock design in commit `6a3d801c` (the sweep
> documented below), and (2) `scripts/spec-sync-hash.sh` folds **all** of
> `components/interfaces/` into every component's hash, so the dispatcher-only
> `idispatcher.rs` store-backpressure change (`39ffee9b`/`c400ced8`, new
> `DispatcherConfig::store_backpressure_ms` + two `TierEventStats` counters)
> re-hashed dispatch-map even though it references none of those types. Re-verified
> this sweep: `src/state.rs` still uses a single global `Mutex<Inner>` + `Condvar`
> (matches FR-002/FR-013); no `IDispatchMap` method or type changed. **Drift status
> remains `clean`; no `spec.md` edit was needed.**

> **Digest refreshed 2026-09-03 (interfaces-only hash change).** This branch
> reworded a doc comment on `IMemoryTier::evict_next_for_key`
> (`components/interfaces/src/imemory_tier.rs`) as part of the memory-tier
> spec-sync. `scripts/spec-sync-hash.sh` folds the whole `components/interfaces`
> tree into every component's hash, so this component's digest moved even though
> no interface signature or behavior changed and dispatch-map was not otherwise
> re-synced. The interface delta is confined to documentation and cannot affect
> dispatch-map's spec↔implementation alignment; the report body below stands
> unchanged and drift status remains `clean`. Digest recomputed against the
> current interface tree.

# Dispatch-Map — Spec Drift Report

**Generated**: 2026-10-14
**Project**: `components/dispatch-map` (spec-kit root `components/dispatch-map/`)
**Branch**: `fix/dispatch-map-get-pool-id-panic` (HEAD `0ca07fca`; fix commit `c877b74e`; base `origin/unstable` = `a052af01`)
**Mode**: read-only, static analysis (batch). Tests were **not** run: `deps/spdk-build/` is missing from this worktree and the crate needs `spdk-sys` through `interfaces/spdk`.
**Supersedes**: the 2026-09-02 report body (the sharding-revert sweep), which is still in git history at `0ca07fca`.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 (`001-dispatch-map`) |
| Requirements Checked | 33 (27 FR, since FR-019/FR-021 were removed, + 6 SC) |
| ✓ Aligned | 29 (88%) |
| ⚠️ Drifted | 4 (12%): FR-003, FR-023, FR-024, FR-026 (FR-026 only in the contract/data-model) |
| ✗ Not Implemented | 0 (0%) |
| ⚠️ Drift not tied to an FR | 1: the "no panics" rule (Clarification 2026-04-27 Q3, `spec.md:295`; contract `:79`) |
| 🆕 Unspecced Code | 1: the pool-id cache lifecycle |

## What the branch changed

`git diff origin/unstable.. -- src` touches only `src/lib.rs` and adds `src/repair_test_get_pool_id.rs`:

- `get_pool_id` now returns `Result<PoolId, DispatchMapError>` and maps an unbound receptacle to `NotInitialized` (`src/lib.rs:51-63`, helper `no_eviction_policy` at `:67-69`).
- `initialize` (`:77-81`), `create_memory_tier_entry` (`:403-407`) and `recover_extent` (`:588-592`) pass that error on. The old `eviction_policy.get().unwrap()` calls at those sites are gone.
- `oldest_keys` returns `Vec::new()` when the pool id can't be obtained (`:383-385`).
- Four new unit tests (`src/repair_test_get_pool_id.rs:13-67`) cover the unbound case and the bind-then-disconnect case for `oldest_keys`, `create_memory_tier_entry` and `recover_extent`.

## Detailed Findings

### Spec: 001-dispatch-map — Dispatch Map Component

#### Aligned ✓

- **FR-001** (`spec.md:240`) `CacheKey = u64` → `components/interfaces/src/idispatch_map.rs:6`.
- **FR-002** (`spec.md:241`) Entries hold location, size_blocks, read_ref/write_ref (`u32`) and an `EvictionHandle` (`src/entry.rs:28-43`). There is one `HashMap` behind one `Mutex<Inner>` plus a `Condvar` (`src/state.rs:12-21`), and a separate `pool_id: Mutex<Option<PoolId>>` (`src/state.rs:20`).
- **FR-004** (`spec.md:243`) `lookup` waits for `write_ref == 0` with `DEFAULT_TIMEOUT = 2000ms` (`src/lib.rs:25`, `:131-140`). It returns `NotExist`/`BlockDevice{offset}`/`MemoryTier{pointer,size}` (`:142-162`), increments read_ref (`:147-150`), and refreshes eviction priority after dropping the lock (`:164-167`). `MismatchSize` is never produced.
- **FR-005** (`spec.md:244`) `convert_to_storage` sets `ssd_offset` (`:183-186`), returns `InvalidState` on a BlockDevice entry (`:187-191`) and `KeyNotFound` for a missing key (`:178-181`), and decrements read_ref only if it is above 0 (`:194-196`).
- **FR-006** (`spec.md:245`) `take_read` waits for `write_ref == 0` and keeps the guard held through the increment (`:213-229`); `Timeout` at `:217-219`.
- **FR-007** (`spec.md:246`) `take_write` waits for both refs to be 0, then sets `write_ref = 1` (`:244-260`). Setting it to 1 is the same as incrementing, because the wait guarantees `write_ref == 0`.
- **FR-008 / FR-009** (`spec.md:247-248`) `RefCountUnderflow` when the count is already 0 (`:274-276`, `:294-296`).
- **FR-010** (`spec.md:249`) `downgrade_reference` runs under one lock hold (`:308-331`); `NoWriteReference` at `:314-316`. Note: `write_ref = 0` (`:318`) is written before `checked_add` (`:319-322`), so an overflow would leave a partial state. That path can't be reached: whenever `write_ref == 1`, `read_ref == 0`. `take_write` requires it (`:248`), `create_memory_tier_entry` sets it (`:422-423`), and every read acquisition waits for `write_ref == 0`.
- **FR-011** (`spec.md:250`) `remove` returns `ActiveReferences` (`:341-343`) or `KeyNotFound` (`:336-339`), and on success removes the handle from the policy (`:348-350`).
- **FR-012** (`spec.md:251`) **— resolved by this branch.** With no `IEvictionPolicy` connected, `initialize` returns `NotInitialized`: from `get_pool_id` (`:56-59`) when no pool is cached, otherwise from `:78-81`. With no extent manager it returns `Ok(())` (`:83-91`); otherwise it recovers through `for_each_extent` (`:100-118`). **Test gap:** no test calls `initialize()` unbound, which is an acceptance item in `align-tasks.md`. The only `initialize()` calls are in `tests/integration.rs:509,540`, both with a policy bound.
- **FR-013** (`spec.md:252`) One global lock (`src/state.rs:18`). `wait_for` returns with the guard still held (`src/state.rs:54-79`). Lock order is consistent: `pool_id` is always taken and released before `inner` (`src/lib.rs:77`→`:97`, `:403`→`:409`, `:588`→`:594`).
- **FR-014** (`spec.md:253`) Only `logger.info` (`:87`, `:94`, `:121`) and `logger.debug` are used; there is no `logger.error`/`warn` in `src/lib.rs`.
- **FR-015** (`spec.md:254`) `define_component!` provides `IDispatchMap` and declares the receptacles `logger`, `extent_manager` and `eviction_policy` (`src/lib.rs:33-46`).
- **FR-016** (`spec.md:255`) `touch` returns `KeyNotFound` for a missing key (`:361-364`), calls `ep.touch(handle)` after dropping the lock (`:366-369`), and leaves the ref counts alone.
- **FR-017** (`spec.md:256`) `oldest_keys` delegates to `get_eviction_candidates(pool_id, n)` (`:386-387`). It now returns an empty list when unbound (`:383-385`). The spec doesn't describe that case; see proposal P3.
- **FR-018** (`spec.md:257`) `convert_memory_tier_to_block` reads `ssd_offset` and returns `InvalidState` when it is `None` or the entry isn't MemoryTier (`:448-468`). The `MemoryTier` fields are at `src/entry.rs:13-18`.
- **FR-020** (`spec.md:259`) `initialize` is an explicit trait method (`src/lib.rs:76`). Nothing in the crate calls it except tests.
- **FR-022** (`spec.md:261`) `is_evictable` (`:530-546`) matches the spec exactly.
- **FR-025** (`spec.md:264`) `promote_block_to_memory_tier` flips the entry in place and keeps its handle and refs (`:498-510`). It sets `ssd_offset = Some(offset)` (`:507`) and returns `InvalidSize` (`:488-490`), `KeyNotFound` (`:493-496`) or `InvalidState` (`:511-515`). *No unit test covers it.*
- **FR-026** (`spec.md:265`) `try_evict_to_block` returns `ActiveReferences` for held refs (`:558-560`), then `InvalidState` for a missing `ssd_offset` or a non-MemoryTier entry (`:571-578`), all under one lock hold (`:549`). Six tests at `src/lib.rs:972-1056`. spec.md matches, but the contract and data-model don't (see Drifted).
- **FR-027 / FR-028** (`spec.md:267-268`) `set_checksum`/`get_checksum` are behind `cfg(feature = "integrity-check")` (`src/lib.rs:613-634`; trait `components/interfaces/src/idispatch_map.rs:207,215`). The field is behind the same feature (`src/entry.rs:41-42`), and the feature is off by default (`Cargo.toml` has no `default` feature). A stored `0` reads as unset (`:629-633`). *No test covers it.*
- **FR-029** (`spec.md:270`) `reuse_count` (`src/entry.rs:37`) starts at 0 (`src/lib.rs:112,425,605`) and increments only in `lookup`/`take_read`/`downgrade_reference` (`:151-153`, `:230-232`, `:323-325`). It is visible only through `Debug` (`src/entry.rs:53-56`). *No test covers it.*
- **SC-001** (`spec.md:282`) Every extent from `for_each_extent` is inserted (`:100-118`); test `tests/integration.rs:477`.
- **SC-002** (`spec.md:283`) Tests `tests/integration.rs:42,563,632`.
- **SC-003** (`spec.md:284`) `downgrade_reference` runs under one lock hold (see FR-010); tests `tests/integration.rs:254,276`.
- **SC-004** (`spec.md:285`) `pub fn entry_size()` (`src/lib.rs:16-18`). The `≤ 56` bytes bound is asserted only in `benches/dispatch_map_benchmark.rs:97`, not in any test.
- **SC-005** (`spec.md:286`) `wait_for` checks the predicate before waiting (`src/state.rs:65-67`).
- **SC-006** (`spec.md:287`) Overflow/underflow checks at `src/lib.rs:147-150,226-229,274-276,294-296,319-322`.

#### Drifted ⚠️

- **FR-003** (`spec.md:242`, US1-AS3 `spec.md:27`, Edge Cases `spec.md:225`). Severity: **moderate** for the code gap, **minor** for the docs.
  - (a) Carried over: the spec requires an error for a null pointer, with no entry recorded. The code checks only `size == 0` (`src/lib.rs:399-401`) and records the entry whatever `pointer` is (`:415-430`).
  - (b) New with the fix: the function can now return `NotInitialized` (`:403-407`). FR-003 lists only `AlreadyExists`, and contract `:45` lists only `InvalidSize`/`AlreadyExists`. FR-003 also leaves out `InvalidSize`, which the code returns (`:399-401`) and the contract/data-model already list.
- **FR-023** (`spec.md:262`). Severity: **minor**. `entry_size` returns `entry.size_blocks * 4096` as a `u32` (`src/lib.rs:379`). `size_blocks` comes from `size.div_ceil(4096)` (`:421`, `:509`) or directly from the caller (`recover_extent` `:601`, `initialize` `:108`). The product overflows when `size > 4_294_963_200` (e.g. `u32::MAX` gives `size_blocks = 1_048_576`) or when `size_blocks >= 2^20`. The workspace `Cargo.toml` has no `[profile]` override, so dev and test builds panic on that overflow, which breaks the "no panics" rule. Release builds wrap it to `0`, and FR-023 says the dispatcher uses this value to size its memory-tier allocation.
- **FR-024** (`spec.md:263`). Severity: **minor** (docs). `recover_extent` now returns `NotInitialized` when unbound (`src/lib.rs:588-592`). FR-024 and contract `:60` list only `AlreadyExists`.
- **FR-026**. Severity: **minor** (docs; spec.md itself matches). `contracts/idispatch_map.md:49` says *"`InvalidState` if not evictable (active refs, no `ssd_offset`, or not `MemoryTier`)"*, and `data-model.md:78` lists `ActiveReferences` only for `remove`. The code returns `ActiveReferences` for held refs (`src/lib.rs:558-560`), and so do `spec.md:265` and the interface doc (`components/interfaces/src/idispatch_map.rs:181-185`). The 2026-10-09 sync updated spec.md but neither of these files.

#### Drift not tied to an FR ⚠️

- **"No panics"** (Clarification Q3 `spec.md:295`; contract `contracts/idispatch_map.md:79`; `plan.md:19`). Severity: **moderate**.
  - `get_pool_id` caches the first pool id forever (`src/lib.rs:52-62`) and never checks which policy it came from. The receptacle allows disconnecting and connecting a *different* policy; `src/repair_test_get_pool_id.rs:49-51` already does the disconnect half. After that the cached id is stale, and `ep.track(pool_id, …).unwrap()` (`:101-103`, `:414`, `:598`) panics with `EvictionPolicyError::InvalidPool` (`components/eviction-policy-lru/src/lib.rs:60-67`; pool ids are vector indices, `:43`).
  - Each of those unwraps runs **while `state.inner` is locked** (`:97`, `:409`, `:594`). The panic therefore poisons the mutex, and every later `lock().unwrap()` in the component panics too.
  - If the new policy already has a pool at that index (the LRU policy is multi-pool and shared, per the root CLAUDE.md), `track` succeeds and the entry is silently tracked in another component's pool. `oldest_keys` would then return that component's keys.
  - The 2026-10-13 note reported the panic. The mutex poisoning and the silent foreign-pool case are new here.

#### Not Implemented ✗

None.

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| Pool-id cache lifecycle: created lazily by whichever of `initialize`/`oldest_keys`/`create_memory_tier_entry`/`recover_extent` runs first, and never invalidated | `src/lib.rs:51-63`, `src/state.rs:20` | 13 | `001-dispatch-map` FR-002/FR-012 (covered by proposal P7, no separate spec) |

### Stale spec-kit status notes

`spec.md:6` and `spec.md:8` still list "FR-012 (unbound `IEvictionPolicy` panics …)" as an open code-side align. That is no longer true (proposal P10).

## Inter-Spec Conflicts

- Inside the spec dir: `contracts/idispatch_map.md:49` and `data-model.md:78,86` contradict `spec.md:265` on the error `try_evict_to_block` returns for held references (see FR-026).
- `data-model.md:82` ("NotInitialized … Reserved; not currently returned") contradicts the code and the new behaviour behind FR-012.
- Outside this component's spec (for information only, no proposal): the `NotInitialized` doc comment at `components/interfaces/src/idispatch_map.rs:52` still says "missing DMA allocator", and the `oldest_keys` doc (`:126`) says "lowest TSC values". The component's `CLAUDE.md` doesn't mention the `eviction_policy` receptacle.

## Recommendations

1. Apply the five BACKFILL proposals (P1–P5) so spec.md, the contract and the data-model document `NotInitialized`, the empty-list fallback in `oldest_keys`, and `ActiveReferences` from `try_evict_to_block`.
2. In a code change, map `track` failures to an error instead of calling `unwrap()`, and decide what rebinding `eviction_policy` should mean (P7). This is the most serious open item: the panic poisons the mutex and takes down the whole map.
3. Add the missing `initialize()`-unbound test (P8) and make `entry_size` overflow-safe (P9).
4. The null-pointer check (P6) hasn't changed since 2026-08-20. Decide whether to implement it or drop US1-AS3.
5. Add a Last Synced line recording that the FR-012 align is resolved (P10).
6. Once SPDK is available, run `cargo test -p dispatch-map` to confirm the four new repair tests pass. This analysis didn't run them.
