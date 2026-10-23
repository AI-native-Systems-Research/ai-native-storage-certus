---
spec_sync_component: memory-tier
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-23T18:00:34Z
spec_sync_git_commit: d7a24d58
spec_sync_inputs_sha256: e6f69f2674d903eb97258b3354337286a58461c13b857b60023c089608b0d8b7
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Re-stamp 2026-10-23 (branch `unstable`, commit `d7a24d58` "Fix workspace build warnings" plus the spec-sync follow-up).** That commit only fixed warnings under the workspace lint posture: it added `Debug` impls, narrowed unreachable `pub` items to `pub(crate)`, wrapped `unsafe fn` bodies in explicit `unsafe {}` blocks, and the follow-up ran `rustfmt`. It also touched `components/interfaces` (a `Debug` impl for `Command`), which changes every component's digest. This component's own `src/` was in that commit. Every changed item was checked: none is part of a provided interface, none is named as public by the spec, and no behaviour changed. No spec citation into this component's source moved. All citations were re-verified by diff-mapping each base line to its new position. Status stays `clean`.

> **Re-stamp 2026-10-21 (transitive, `components/interfaces` changed on branch `fix/dispatcher-remove-clear-mt`).** Only `components/interfaces` changed for this component: `IDispatcher` doc comments (`flush_to_ssd` errors, `clear_memory_tier`, `batch_lookup`) and the interfaces spec (FR-008, FR-009, FR-017, FR-018, FR-018a, FR-021, FR-023, Overview, NFR-002, Implementation Notes; text only, no type or signature changed). This component uses some of the named items (4 files), but no type, signature or behaviour of them changed. This component's last full sync below stands; no re-analysis was needed.

> **Re-stamp 2026-10-14 (transitive, merge of unstable 6f5743cc into this branch).** Only `components/interfaces` changed for this component (`DispatcherConfig::cold_staging_buf_bytes` default 4 -> 16 MiB, #550); `memory-tier` does not use it (grep over its src/ and specs/: 0 hits). This branch's full sync below stands; no re-analysis was needed.

> **Re-stamp 2026-10-14 (transitive: `components/interfaces` changed, branch `fix/cold-staging-buf-16mib`).** The spec-sync digest folds `components/interfaces/{src,specs}` into every component's hash, so the single interface change on this branch — `DispatcherConfig::cold_staging_buf_bytes` default 4 MiB → 16 MiB (commit `8e12e49f`) and the matching correction to interfaces FR-027 — invalidates every component's stamp at once; this is the design, not a defect. **No code re-analysis was performed for this component, and none was needed:** its own `src/**` and `specs/**` are unchanged on this branch, and it does not reference `cold_staging_buf_bytes` (consumed only by the dispatcher's host-DRAM cold-staging pool). Digest refreshed to the recomputed value.

> **Apply 2026-10-14 (branch fix/memory-tier-insert-track-panic, approved by Cornel).** Applied from `proposals.json` (2026-10-14T16:47:49Z). Before editing, `spec.md`, `plan.md` and `tasks.md` were backed up to `.specify/sync/backups/20261014T172213Z/`. **Applied (spec text only):** P1, which adds FR-030 (a refused `track` makes `insert` return `NotInitialized`, with nothing reserved or recorded; no new variant, per the approval note) and its US1 test-coverage line; P8 (US1 `peek` test-coverage claim corrected; combined with P1 on the same line); P5 (FR-023 exceptions listed); P6 (FR-027 and the Implementation Notes telemetry sentence; it keeps "for which the policy returned a victim", because P3 did not change code); P7 (NFR-005: first-fit is a linear O(k) scan); P10 (the `plan.md` sharding note is marked SUPERSEDED, and `tasks.md:16,24,37,43` are marked `[~]` superseded, with `tasks.md:13` now counting the regression test). `spec.md` Last-Synced was also bumped. **Became tasks (`align-tasks.md`, no code changed):** P3 (FR-013, `evict_next` victim chosen outside the pool lock) and P4 (SC-3, touches after dropping the pool lock race with `clear`); each needs a failing test first. **Still open:** P2 (`IMemoryTier` doc in `components/interfaces`, deferred to a separate PR) and P9 (disconnected `eviction_policy` receptacle; decision between option A, errors, and option B, must stay connected). Both are recorded in `align-tasks.md`. Drift that remains: FR-013 and SC-3 (code), and the interface doc (P2) plus U2 (P9). Status stays drift. Details are in `apply-report.md`.
>
> **Stamp: clean.** Every drift item of this analysis is resolved (P1, P5-P8, P10 applied to the spec) or tracked in `align-tasks.md` (ALIGN tasks P3, P4; open decisions P2, P9), as in earlier `clean` stamps of this and other components (e.g. dispatcher-p2p).

> **Sync 2026-10-14 (branch fix/memory-tier-insert-track-panic, full re-analysis at `78f60ab8`: every FR/NFR/SC re-checked against `src/` with file:line).** Scope: all 29 FR, 11 NFR and 7 SC of `specs/001-memory-tier/spec.md`, plus `plan.md`/`tasks.md` and the `IMemoryTier` doc (`components/interfaces/src/imemory_tier.rs`). Result: 41 aligned, 5 drifted (FR-013, FR-023, FR-027, NFR-005, SC-3), 1 not re-verified (SC-1: `cargo test` could not be run in this session), 2 unspecced behaviours, 1 plan/spec conflict. This sweep confirms the 2026-10-13 finding. The fix holds: no panic, no space reserved, no slot recorded (`lib.rs:360-378`), and the fallback at `lib.rs:375-378` is unreachable. But the refused-`track` case returns `NotInitialized`, and neither the spec nor the `insert` doc describes it (`imemory_tier.rs:33,66`). New this sweep: (1) **concurrency, SC-3/FR-013 (PLAUSIBLE: found by reading the code, not reproduced).** `evict_next` asks the policy for a victim (`lib.rs:452`) before taking the pool write lock (`:465`). LRU node indices are recycled with no generation counter (`eviction-policy-lru/src/lru_list.rs:38-46,143`). So a racing `remove`/`insert` can delete another key's LRU node, and that key can then never be evicted. `get`/`touch`/`batch_touch` call the policy after dropping the pool lock (`lib.rs:418-419,526-527,564-565`). A racing `clear` (`lib.rs:617` → `lru_list.rs:148`) can therefore leave a stale index that points past the end of the node vector, and the touch panics. `evict_next` also returns `Some(key)` and counts an eviction when it freed no slot (`lib.rs:467-473`). (2) Once the `eviction_policy` receptacle is disconnected, every pool operation except `batch_touch` panics on `eviction_policy.get().unwrap()` (`lib.rs:340,398,441,451,491,522,612` vs `:539-542`). (3) Wording drift in FR-023, FR-027, NFR-005 and the US1 `peek` test-coverage claim. `plan.md`/`tasks.md` still describe the 16-shard design. 10 proposals are in `proposals.md`, all `approved: false`; apply was not run. Verdict: drift.

> **Sync 2026-10-13 (branch fix/memory-tier-insert-track-panic, fix: `insert` returns an error instead of panicking when `IEvictionPolicy::track` refuses).** Scope: `src/lib.rs` `insert`, `src/allocator.rs` `can_allocate`, new `src/repair_test_insert_track_failure.rs`; checked against spec FR-008/009/010/023 (`spec.md:157-159,172`) and the `IMemoryTier` doc (`interfaces/src/imemory_tier.rs:33-34,63-67`). Findings: FR-008/009/010 still hold and keep their order. The pre-check `lib.rs:362-364` uses the same predicate as `allocate` (`allocator.rs:37-43` vs `:47-53`), so the rollback at `lib.rs:375-378` is unreachable. **New drift:** a refused `track` now returns `NotInitialized("eviction policy refused…")` (`lib.rs:366-372`) on an initialized pool, with no space reserved and no slot recorded. Neither the spec nor the interface describes this case: `insert` names only `PoolFull` (`imemory_tier.rs:66`), and the variant's doc says "Component not initialized" (`:33`). Fix: add an FR for this case (and, if wanted, a dedicated variant), then update the `insert` doc. Verdict: drift.

> **Re-stamp 2026-10-09 (transitive: `components/interfaces` changed, branch `fix/evict-blocked-by-pin`).** The only change to this component's hashed inputs is a doc-comment correction on `IDispatchMap::try_evict_to_block` in `components/interfaces/src/idispatch_map.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and its previous stamp was re-verified to match `origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

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

> **Digest refreshed 2026-09-03 (spec-sync gate fix).** The earlier
> `spec_sync_inputs_sha256` was computed in a working tree that contained an
> untracked `components/interfaces/src/iipc.rs` — a local file not part of this
> branch — which `scripts/spec-sync-hash.sh` folds into every component's
> interface hash, so CI's clean checkout recomputed a different digest. The
> stray file was removed and the digest recomputed in a clean tree. No `src/`,
> `specs/`, or report content changed; drift status remains `clean`. (This
> component's own sync — including the `imemory_tier.rs` doc-comment fix — is
> unchanged; the interface edit is already reflected in this digest.)

# Spec Drift Report — memory-tier

Generated: 2026-10-14T16:47:49Z
Project: memory-tier (`components/memory-tier`, spec-kit root `components/memory-tier/`)
Tree: branch `fix/memory-tier-insert-track-panic` at `78f60ab8` (fix commit `21dbc787` on top of `origin/unstable`)
Mode: read-only drift analysis (BATCH). No spec, source or other file was edited.

All line numbers below were taken from the files as they are at `78f60ab8`. Unqualified `lib.rs:` / `allocator.rs:` mean `components/memory-tier/src/`; `spec.md:` means `components/memory-tier/specs/001-memory-tier/spec.md`.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 (`001-memory-tier`) |
| Requirements Checked | 47 (29 FR + 11 NFR + 7 SC) |
| ✓ Aligned | 41 (87%) |
| ⚠️ Drifted | 5 (11%) — FR-013 (moderate), SC-3 (major, plausible), FR-023, FR-027, NFR-005 (minor) |
| ? Not re-verified this sweep | 1 (2%) — SC-1 (`cargo test` could not be run here) |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 2 behaviours (U1 refused-`track` error path; U2 disconnected-receptacle panics) |
| Other doc drift | 2 (D1 US1 `peek` test-coverage claim; D2 `IMemoryTier::insert` / `NotInitialized` doc) |
| Inter-spec conflicts | 1 (`plan.md`/`tasks.md` still describe the 16-shard design) |

The fix on this branch (`21dbc787`) is correct for what it targets: `insert` no longer panics when `IEvictionPolicy::track` refuses. But it adds an error path the spec does not describe (U1). Re-checking every requirement also turned up concurrency problems that were not reported before (FR-013, SC-3). They come from `evict_next` and the touch paths using eviction handles outside the pool lock, combined with the LRU policy recycling node indices.

## What the fix changed (`git diff origin/unstable.. -- components/memory-tier/src`)

| Change | Location | Check |
|--------|----------|-------|
| New `FreeList::can_allocate(size)`: same zero-size check and same `>= aligned_size` predicate as `allocate`, but does not modify the free list | `allocator.rs:36-43` vs `allocate` `:47-53` | Identical predicate (`values().any(\|&s\| s >= aligned)` vs `iter().find(\|(_, &s)\| s >= aligned)`). Verified. |
| `insert` checks for room **before** calling `track` and returns `PoolFull` if there is none | `lib.rs:360-364` | FR-010 still holds; PoolFull is now reported before the policy is called. |
| `track` error is mapped to `MemoryTierError::NotInitialized(format!("eviction policy refused to track key {key}: {e}"))` instead of `.unwrap()` | `lib.rs:366-372` | Removes the panic. A panic here would have poisoned `state.pool` (write guard held from `:354`). **Unspecced (U1).** |
| `allocate` after `track`, with a rollback (`ep.remove(handle)` + `PoolFull`) if it fails | `lib.rs:374-378` | Unreachable: `can_allocate` passed under the same `pool` write guard, and `ep.track` cannot reach `pool`. Harmless defensive code. |
| Regression test `insert_fails_cleanly_when_policy_refuses_to_track` | `src/repair_test_insert_track_failure.rs:21-52`, wired at `lib.rs:788-790` | Points `state.pool_id` at a pool the policy never created, so `track` returns `InvalidPool` (`eviction-policy-lru/src/lib.rs:60-67`). Asserts no panic, `is_err`, `used` unchanged, `!contains`, and that the component still works afterwards. Not listed in the spec's Test Coverage. |

When `track` can fail in production: the LRU implementation fails only with `InvalidPool` (`eviction-policy-lru/src/lib.rs:60-67`). The `pool_id` comes from the same policy's `create_pool()` at `initialize` (`lib.rs:308`). So a refusal needs either (a) a different `IEvictionPolicy` implementation that can refuse, or (b) the receptacle being re-bound with `disconnect()` + `connect()` (`component-core/src/receptacle.rs:84-117`) to a policy instance that never created this pool. The error message is accurate; the variant name (`NotInitialized`) is not.

## Detailed Findings

### Spec: 001-memory-tier — Memory Tier (DRAM Cache Pool)

#### Aligned ✓

| Req | Spec (spec.md) | Implementation evidence |
|-----|----------------|-------------------------|
| FR-001 single contiguous mmap region | `:150` | `alloc_mmap` single `libc::mmap(…, pool_size, …)` `lib.rs:190-257` (call `:195-204`) |
| FR-002 hugepages preferred, fallback | `:151` | `MAP_HUGETLB` `lib.rs:200`; on `MAP_FAILED` plain anonymous mmap `:206-220` |
| FR-003 spdk_zmalloc when SPDK active | `:152` | `#[cfg(feature="spdk")]` + `is_spdk_env_active()` → `spdk_zmalloc(pool_size, 4096, …, SPDK_MALLOC_DMA)` `lib.rs:277-303` |
| FR-004 4 KiB alignment | `:153` | `ALIGNMENT = 4096` `allocator.rs:5`; sizes rounded `:51`; offsets start at 0 and grow by aligned sizes (`:57-63`), and the base pointer is page-aligned (mmap / `spdk_zmalloc` align 4096 `lib.rs:285`). Test `allocate_rounds_up` `allocator.rs:107-113` |
| FR-005 single unsharded `RwLock<Pool>` | `:154` | `struct Pool` `lib.rs:76-79`, `pool: RwLock<Pool>` `:85`, rebuilt in `initialize` `:313-316`; no shard symbols anywhere in `src/` |
| FR-006 reads shared / mutations exclusive | `:155` | read: `get` `lib.rs:412` (telemetry `:401/408`), `peek` `:429`, `contains` `:574`, `batch_touch` `:556` (`:545/552`), `capacity` `:583`, `used` `:592`; write: `insert` `:354` (`:343/350`), `remove` `:505` (`:494/501`), `evict_next` `:465` (`:454/461`), `clear` `:613` |
| FR-007 one `FreeList` + one `HashMap` | `:156` | `lib.rs:76-79` |
| FR-008 zero size → InvalidSize | `:157` | `lib.rs:329-331`; test `insert_zero_size_fails` `:696-701` |
| FR-009 duplicate → AlreadyExists | `:158` | `lib.rs:356-358` (checked before the room check and before `track`); test `insert_duplicate_fails` `:685-694` |
| FR-010 PoolFull when allocator cannot satisfy | `:159` | `can_allocate` check `lib.rs:362-364` (predicate `allocator.rs:37-43`); test `pool_full_returns_error` `lib.rs:723-732` |
| FR-011 get → ptr+size, updates order | `:160` | `lib.rs:392-421`; `ep.touch(handle)` `:419`; test `insert_and_get` `:674-683` |
| FR-012 peek without order update | `:161` | `lib.rs:423-433` (no `ep` call). Code aligned; see D1 for the test claim |
| FR-014 evict_next_for_key alias, key ignored | `:163` | `lib.rs:479-481` |
| FR-015 remove frees; KeyNotFound | `:164` | `lib.rs:483-514`; `ok_or(KeyNotFound)` `:507-510`; `ep.remove` `:511`; dealloc `:512`; test `remove_and_reuse` `:703-711` |
| FR-016 touch updates order | `:165` | `lib.rs:516-529`; test `touch_updates_eviction_order` `:765-776` |
| FR-017 batch_touch amortizes lock | `:166` | one pool read guard `lib.rs:544-556`, handles collected `:558-563`, single `ep.batch_touch` `:565`; absent keys skipped `:560`; empty slice no-op `:532-534` (no test, as US6 says `:122`) |
| FR-018 clear removes all, resets, returns count | `:167` | `lib.rs:605-619`: count `:614`, `slots.clear()` `:615`, `FreeList::new(pool_size)` `:616`, `ep.clear_pool` `:617`; test `clear_resets_all` `:753-763`. ("resets allocators" is plural from the sharded era; there is one allocator. Wording only.) |
| FR-019 NUMA mbind with fallback | `:168` | `lib.rs:225-254`: `SYS_mbind(MPOL_BIND)` `:233-243`; on failure `log_warn` and continue `:246-252`. (For node ≥ 64 the mask is 0 `:228-231`, so mbind fails and falls back.) SPDK path passes node to `spdk_zmalloc` `:280-287` |
| FR-020 is_dma_capable only for SPDK | `:169` | `lib.rs:621-624` returns `spdk_allocated`, set true only on the SPDK branch `:300` |
| FR-021 oldest_keys = single `get_eviction_candidates` | `:170` | `lib.rs:435-443` (`n == 0` → empty `:437`) |
| FR-022 pool_info | `:171` | `lib.rs:596-603` |
| FR-024 IEvictionPolicy receptacle | `:173` | `define_component!` receptacles `lib.rs:142-145`; required at `initialize` `:266-268` |
| FR-025 ILogger optional | `:174` | `lib.rs:143`; `log_info`/`log_warn` skip if unbound `:153-163` |
| FR-026 coalescing | `:175` | `deallocate` prev `allocator.rs:75-82`, next `:84-89`; tests `coalesce_adjacent` `:142-153`, `coalesce_with_following` `:155-165` |
| FR-028 telemetry_snapshot / telemetry / reset | `:177` | `telemetry_snapshot` `lib.rs:626-646` (zeros without feature `:642-645`); `telemetry()` `:165-170`, `reset_telemetry()` `:172-177` |
| FR-029 free_capacity | `:178` | `lib.rs:179-187` |
| NFR-001 O(1) LRU via delegation | `:184` | LRU list is index-linked, O(1) push/move/remove/pop (`eviction-policy-lru/src/lru_list.rs:37,70,99,121`) |
| NFR-002 single RwLock; touches outside pool lock | `:185` | `drop(pool)` before `ep.touch`/`ep.batch_touch` `lib.rs:418-419,526-527,564-565`. The design is as stated; its consequences are under SC-3 |
| NFR-003 no syscalls on data path | `:186` | No libc calls in insert/get/peek/remove/touch/evict (only `RwLock`/`Mutex`, which may futex under contention) |
| NFR-004 Send + Sync | `:187` | `unsafe impl Send/Sync for MemoryTierState` `lib.rs:92-96` with SAFETY comment |
| NFR-006 freed on Drop | `:189` | `lib.rs:116-136` (`spdk_free` if env still active, else leaked as documented `spec.md:242`; `munmap` otherwise) |
| NFR-007 default pool size 256 MiB | `:190` | `DEFAULT_POOL_SIZE` `lib.rs:34`; unused, as `spec.md:245` already says |
| NFR-008 version 0.3.0 | `:191` | `Cargo.toml:3` = `0.3.0`; `define_component!` `version: "0.3.0"` `lib.rs:140` |
| NFR-009 SPDK optional | `:192` | `Cargo.toml:9,18`; gated `lib.rs:121,277,305` |
| NFR-010 DMA-suitable pointers | `:193` | Follows from FR-001/FR-004 (page-aligned base, 4 KiB-aligned offsets, one contiguous region) |
| NFR-011 telemetry zero-cost when off | `:194` | All counters and the `telemetry` field are `#[cfg(feature="telemetry")]` (`lib.rs:20-21,37-68,88-89`) |
| SC-2 Drop frees pool | `:230` | `lib.rs:116-136` (by implementation; no leak test) |
| SC-4 4 KiB alignment for all allocations | `:232` | As FR-004 |
| SC-5 eviction frees space, allows re-insertion | `:233` | `evict_next` deallocates `lib.rs:467-469`. Tests only cover `remove`→reinsert (`remove_and_reuse`), not evict→reinsert |
| SC-6 NUMA bind or graceful fallback | `:234` | As FR-019 (not exercised by tests) |
| SC-7 SPDK DMA pointers | `:235` | As FR-003/FR-020 (not exercised by default tests) |

#### Drifted ⚠️

- **FR-013** — spec `spec.md:162`: "evict_next() delegates victim selection to the eviction policy (`identify_next_to_evict(pool_id)`), then removes that slot and frees its allocation".
  Code `lib.rs:445-477`: the victim is chosen and **removed from the policy** (`identify_next_to_evict` pops it, `eviction-policy-lru/src/lib.rs:133-138`) at `lib.rs:452`, holding only the outer `state` read lock. The pool write lock is taken afterwards, at `:465`. Then:
  1. If the slot is gone by then (a concurrent `remove(key)`), nothing is freed, but `Some(key)` is still returned (`:467-473`) and the `evictions` counter is still bumped (`:470-471`, outside the `if let`). The caller is told the key was evicted when nothing was freed.
  2. ABA on eviction handles: the popped node index goes onto the LRU free list (`lru_list.rs:143`), and the next `track` reuses it (`lru_list.rs:38-46`). `EvictionHandle` has no generation counter. In the window between `:452` and `:465`, `insert(J)` can be handed K's old index, and then `remove(K)` (the slot still holds the stale handle) calls `ep.remove(stale)` (`lib.rs:511`). That removes **J's** LRU node. J keeps its slot but is no longer tracked, so it never comes out of `identify_next_to_evict` and never appears in `oldest_keys`. It can only leave the cache through `remove`/`clear`.
  - Severity: **moderate**. Verdict: PLAUSIBLE (found by reading the code; not reproduced in a test).
- **SC-3** — spec `spec.md:231`: "Concurrent access from 16+ threads does not deadlock or corrupt state".
  Code: no test runs more than one thread (no `thread::spawn` in `src/`; `tasks.md:20` still lists it as open). From the code:
  1. The FR-013 ABA path above corrupts the slot↔policy correspondence.
  2. `get`/`touch`/`batch_touch` copy the handle, drop the pool lock and then call the policy (`lib.rs:417-419,525-527,561-565`). If `clear` runs in between (`lib.rs:613-617`), `ep.clear_pool` calls `LruList::clear`, which empties `nodes` (`lru_list.rs:147-153`). The later `move_to_back(idx)` then indexes `self.nodes[idx]` without a bounds check (`lru_list.rs:70-71`) and **panics** while holding the policy's pool mutex (`eviction-policy-lru/src/lib.rs:84-85`). That poisons the mutex, and every later policy call on that pool panics on `lock().unwrap()`. If new inserts refill `nodes` before the touch arrives, it instead moves an unrelated key.
  3. A `get`/`touch` racing `remove`+`insert` can likewise touch another key's recycled node. That only skews recency and is harmless.
  - No deadlock found: the lock order is always outer `state` → `pool` → policy, or `state` → policy (`evict_next`, `oldest_keys`). The policy never calls back into memory-tier.
  - Severity: **major** (panic + poisoned policy), low likelihood (`clear` racing a hot read). Verdict: PLAUSIBLE.
- **FR-023** — spec `spec.md:172`: "All operations check initialized flag before proceeding".
  Code: the flag is checked in `insert :334`, `get :394`, `peek :425`, `oldest_keys :437`, `evict_next :447`, `remove :485`, `touch :518`, `batch_touch :536`, `contains :570`, `capacity :580`, `used :589`, `pool_info :598`, `clear :607`, `free_capacity :182`. Exceptions:
  - `insert` returns `InvalidSize` for `size == 0` before the flag check (`:329-331` vs `:334`).
  - `batch_touch([])` returns before it (`:532`).
  - `is_dma_capable` (`:621-624`) and `telemetry_snapshot` (`:626-646`) never check it.
  - `evict_next_for_key` relies on `evict_next`.
  - `initialize` checks it inverted (`:271`).

  All of these are benign, but the spec's "All operations" is stronger than the code. Severity: **minor**.
- **FR-027** — spec `spec.md:176`: "tracks eviction count, write-lock contention count, and read-lock contention count". `spec.md:239` adds: "a failed `try_read`/`try_write` first attempt bumps the corresponding lock-contention counter before blocking".
  Code: only some lock sites are instrumented.
  - Write contention is counted in `insert :342-352`, `evict_next :453-463` and `remove :493-503`, but not in `clear :613`.
  - Read contention is counted in `get :400-410` and `batch_touch :544-554`, but not in `peek :429`, `touch :523`, `contains :574`, `capacity :583`, `used :592` or `free_capacity :185`.
  - `evictions` is bumped even when no slot was freed (`:470-471`).

  Severity: **minor**.
- **NFR-005** — spec `spec.md:188`: "Allocator uses BTreeMap for O(log n) first-fit search".
  Code: first-fit is a **linear scan** of the free map in offset order, both in `allocate` (`self.free_regions.iter().find(…)`, `allocator.rs:53`) and in the new `can_allocate` (`values().any(…)`, `:42`). That is O(k) in the number of free regions. Only the map updates and the coalescing lookups are O(log k) (`:55,:59,:76,:80,:86-88,:91`). `insert` now runs two linear scans (`lib.rs:362` and `:375`). Severity: **minor** (documentation; performance claim overstated).

#### Not re-verified this sweep ?

- **SC-1** (`spec.md:229`, "All unit tests pass (`cargo test -p memory-tier`)"): `cargo test` could not be run in this session (the command was not approved), so this report makes **no claim** that the tests pass. Tests found in the tree: 12 in `lib.rs` (`:667-785`), 9 in `allocator.rs` (`:99-178`) and 1 in `repair_test_insert_track_failure.rs` (`:21-52`), 22 in total. The previous report's "21 passed" predates the new test. `tasks.md:13` ("all 12 unit tests in lib.rs") does not count the new test.

#### Not Implemented ✗

None.

### Other spec/doc drift (not an FR/NFR/SC row)

- **D1 — US1 Test Coverage claim, `spec.md:41`**: "`peek_does_not_update_eviction_order` - verifies peek does not update eviction order". The test (`lib.rs:778-785`) inserts one key and asserts only that `peek(0).is_some()`. It never compares eviction order, so FR-012's "without eviction-order update" is verified by reading the code (no `ep` call in `peek`, `lib.rs:423-433`), not by the test. Severity: minor.
- **D2 — `IMemoryTier` doc (`components/interfaces/src/imemory_tier.rs`, part of this component's sync hash)**: the `insert` doc `:63-67` names only `PoolFull`. In fact `insert` also returns `InvalidSize` (`lib.rs:329-331`), `NotInitialized("pool not initialized")` (`:334-338`), `AlreadyExists` (`:356-358`) and, since this branch, `NotInitialized("eviction policy refused to track key …")` (`:366-372`). The variant's doc `imemory_tier.rs:33` ("Component not initialized.") does not cover the new use. Severity: minor (doc); see U1.

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| U1: `insert` when the eviction policy refuses `track`: returns `NotInitialized("eviction policy refused to track key {key}: {e}")`, reserves no space, records no slot, component stays usable; PoolFull takes precedence (checked first); unreachable post-`track` rollback | `lib.rs:360-378`; `allocator.rs:36-43`; test `repair_test_insert_track_failure.rs:21-52` | ~19 + 8 + 52 | `001-memory-tier` new **FR-030** + US1 test-coverage line |
| U2: behaviour with the `eviction_policy` receptacle **disconnected** after `initialize`: `insert :340`, `get :398`, `oldest_keys :441`, `evict_next :451`, `remove :491`, `touch :522` and `clear :612` call `self.eviction_policy.get().unwrap()` and **panic**. `insert`, `remove` and `clear` do so while holding the outer `state` read guard, which is not poisoned by a read guard. `batch_touch :539-542` silently returns instead. Reachable via `Receptacle::disconnect` (`component-core/src/receptacle.rs:110-117`). This is the same class of panic the branch fixed in `track`. | `lib.rs:340,398,441,451,491,522,539-542,612` | 8 sites | `001-memory-tier` new FR (needs a decision on the required behaviour) |

## Inter-Spec Conflicts

- **`plan.md` and `tasks.md` vs `spec.md`**: `plan.md:8,24-48,60,65,75,121,142-149,169,175` still describe the 16-way sharded pool (key % 16, per-shard `Mutex`, `evict_counter` round-robin, Creusot P4/P5). Its own note `plan.md:184` says the sharded text was left "pending a decision", and points to `align-tasks.md`. That decision was taken on 2026-08-20 (backfill to reality, `spec.md:247-253`; `align-tasks.md` "sharding-not-implemented … BACKFILL applied"). `tasks.md:16` (Creusot VCs), `:24` (evict_next_for_key shard test), `:37` (shard-layout diagram) and `:43` (configurable shard count) refer to things that do not exist. `spec.md` is the authority (newest, and it matches the code).
- No conflict between `spec.md` and the `IMemoryTier` docs other than D2.

## Recommendations

1. **Add FR-030 for the refused-`track` path (U1)** and fix the `insert`/`NotInitialized` doc (D2). Decide whether a dedicated error variant (e.g. `EvictionPolicyRefused`) is wanted. The current variant name misleads callers: `dispatcher` maps it to `AllocationFailed(e.to_string())` (`components/dispatcher/src/lib.rs:1219-1221`), so the text "not initialized: …" reaches logs from a pool that is initialized.
2. **Fix the eviction-handle races (FR-013, SC-3)**. One option is to call `identify_next_to_evict` while holding the pool write lock, and to touch while still holding the pool read lock (or give `EvictionHandle` a generation counter that the policy checks). Also return `None` and skip the `evictions` counter when no slot was freed. Then add the 16-thread mixed test that `tasks.md:20` already asks for. Both items are PLAUSIBLE, not reproduced: confirm with a test before changing code.
3. **Decide the disconnected-receptacle contract (U2)** and make all operations follow it (`batch_touch` already returns silently).
4. **Backfill the wording drift**: FR-023, FR-027 (+ `spec.md:239`), NFR-005, D1.
5. **Mark `plan.md`'s sharded sections and the shard/Creusot items in `tasks.md` as superseded.**
6. **Re-run `cargo test -p memory-tier`** (default and `--features telemetry`) to settle SC-1.
