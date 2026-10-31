---
spec_sync_component: eviction-policy-session-lists
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-29T23:34:02Z
spec_sync_git_commit: 1de9052f9
spec_sync_inputs_sha256: 47596bef9d7e076422c07618b5f59e4f3b005e1ba907416535243ecc0cf62edc
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Sync 2026-10-30 (branch `specsync-c`, piece-copy-bench merge) — clippy commit `55a4ddd4`, behaviour unchanged.** Base `a0292be6` (`origin/unstable`, where the last stamp was fresh), HEAD `0a71990e`. This component's only change on the branch is the clippy commit `55a4ddd4`; its digest also moves because `components/interfaces` changed (analysed by the interfaces sync, not here). `cargo clippy --all-targets -D warnings` and `cargo test` pass for logger, eviction-policy-lru, eviction-policy-optimized and eviction-policy-session-lists at HEAD. `55a4ddd4` replaces every `self.state.read()/write().unwrap()` and `pool.lock().unwrap()` with private `#[inline] #[track_caller]` helpers `read_state`/`write_state`/`lock_pool` that call the same `unwrap()` (`src/lib.rs:89-124`), so a poisoned lock still panics, now reported at the caller's location. It adds `#[expect(clippy::cast_possible_truncation)]` on the `len() as u32` casts in `create_pool` and in `Pool::alloc` (`src/session_list.rs:68-73`) (no cast changed), digit separators on hex constants, and test-only changes. `EvictionHandle::new/pool_id/index` gain `#[must_use]` in interfaces. No signature, return value, error variant or ordering changed. No `# Panics`/`# Errors` section was added to any public item. Every spec statement the commit could affect was checked (12 items: FR-009, FR-010, FR-011, FR-013, FR-014, FR-015, FR-016, FR-017, SC-002, SC-004, SC-006, Observability (startup announcement)): all aligned. No spec states lock-poison behaviour, quotes `from_env_str` arms or `Debug` output, or bounds pool ids or node indices, so the `#[expect]` reasons and the private helpers' panic docs add no requirement. Recorded only, not proposed: the specs do not say that a poisoned lock panics; this is pre-existing, the same before and after the commit, and the `IEvictionPolicy`/`ILogger` methods have no error channel for it. The Observability note cited `src/lib.rs:107-120`, now `:149-162`. `tasks.md` T006-T008 cite lines in other components' files as a record of completed work; they were stale before this branch and are left as written. Applied 1 BACKFILL (line citations only; no requirement text changed). Backup in `backups/20261031T045440Z/`. No source files changed.
>
> **Status: clean** (to be stamped by the final pass; frontmatter left as it was).

> **Re-stamp 2026-10-29 (branch `fix/dispatcher-remove-ssd-extent`, merged with unstable `e35618b2`).** Transitive only: this component's own `src/` and `specs/` are as unstable has them; its digest moves because `components/interfaces` changed (the `IDispatcher::check` doc gains an `# Errors` section, B-17 of the dispatcher sync). No analysis below changes.
> **Re-stamp 2026-10-25 (branch `001-rdma-cache-transfer`, rebased onto unstable `a905f89b`).** Transitive only: this component's own `src/` and `specs/` are as unstable has them; its digest moves because `components/interfaces` changed (`IDispatcher::clear_all`) and the rebase brought unstable's changes. No analysis below changes.

> **Re-stamp 2026-10-23 (branch `unstable`, commit `d7a24d58` "Fix workspace build warnings" plus the spec-sync follow-up).** That commit only fixed warnings under the workspace lint posture: it added `Debug` impls, narrowed unreachable `pub` items to `pub(crate)`, wrapped `unsafe fn` bodies in explicit `unsafe {}` blocks, and the follow-up ran `rustfmt`. It also touched `components/interfaces` (a `Debug` impl for `Command`), which changes every component's digest. This component's own `src/` was in that commit. Every changed item was checked: none is part of a provided interface, none is named as public by the spec, and no behaviour changed. No spec citation into this component's source moved. All citations were re-verified by diff-mapping each base line to its new position. Status stays `clean`.

> **Re-stamp 2026-10-21 (transitive, `components/interfaces` changed on branch `fix/dispatcher-remove-clear-mt`).** Only `components/interfaces` changed for this component: `IDispatcher` doc comments (`flush_to_ssd` errors, `clear_memory_tier`, `batch_lookup`) and the interfaces spec (FR-008, FR-009, FR-017, FR-018, FR-018a, FR-021, FR-023, Overview, NFR-002, Implementation Notes; text only, no type or signature changed). This component uses none of the changed items (grep over its src/ and specs/: 0 hits). This component's last full sync below stands; no re-analysis was needed.

> **Re-stamp 2026-10-14 (transitive: `components/interfaces` changed, branch `fix/cold-staging-buf-16mib`).** The spec-sync digest folds `components/interfaces/{src,specs}` into every component's hash, so the single interface change on this branch — `DispatcherConfig::cold_staging_buf_bytes` default 4 MiB → 16 MiB (commit `8e12e49f`) and the matching correction to interfaces FR-027 — invalidates every component's stamp at once; this is the design, not a defect. **No code re-analysis was performed for this component, and none was needed:** its own `src/**` and `specs/**` are unchanged on this branch, and it does not reference `cold_staging_buf_bytes` (consumed only by the dispatcher's host-DRAM cold-staging pool). Digest refreshed to the recomputed value.

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

# Spec ↔ Implementation Drift Report — eviction-policy-session-lists

**This sweep (2026-09-03)** independently re-verified all 24 requirements
(18 FR + 6 SC) of `001-session-list-eviction` against `src/lib.rs` and
`src/session_list.rs`, and against the shared `IEvictionPolicy` interface
(`components/interfaces/src/ieviction_policy.rs`). One documentation drift was
found and fixed (FR-015 BACKFILL); no code drift.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 24 (18 FR + 6 SC) |
| Aligned | 23 |
| Drifted → resolved this sweep | 1 (FR-015 BACKFILL, doc) |
| Not Implemented | 0 |
| Unspecced | 0 |

**Verification this sweep** (workspace default member — fully buildable/testable):
- `cargo build -p eviction-policy-session-lists` — clean
- `cargo clippy -p eviction-policy-session-lists --all-targets -- -D warnings` — clean
- `cargo test -p eviction-policy-session-lists` — green (lib unit tests +
  `tests/lineage_properties.rs`)

## Resolved this sweep

**FR-015 — BACKFILL (spec doc).** The prior report marked FR-015 "Aligned" with
a blanket reading, but FR-015's text over-generalized. Original text:

> "Operations on an invalid or already-removed handle, or on a non-existent
> eviction domain, MUST be reported as errors rather than silently succeeding
> or corrupting state."

The shared `IEvictionPolicy` interface splits its methods into two classes:

- **Fallible, `Result`-returning:** `track` (`ieviction_policy.rs:87`), `touch`
  (`:92`), `batch_touch` (`:96`), `remove` (`:99`) — these return
  `Result<_, EvictionPolicyError>`.
- **Read-only queries with no error channel:** `identify_next_to_evict` →
  `Option<CacheKey>` (`:104`), `get_eviction_candidates` → `Vec<CacheKey>`
  (`:108`), `len` → `usize` (`:111`), `clear_pool` → `()` (`:114`).

The query methods **structurally cannot** report a pool-existence error — they
have no `Result`. `IEvictionPolicy` is a shared, multi-implementor trait
(`eviction-policy-lru` also implements it; consumed by dispatch-map/memory-tier),
so adding an error channel would be a breaking cross-cutting change nobody
requested. The implementation correctly makes the query methods **degrade
safely** on a non-existent pool: `identify_next_to_evict` returns `None`
(`src/lib.rs:200-205`, `?`-on-`get`), `get_eviction_candidates` returns an empty
`Vec` (`:207-213`), `len` returns `0` (`:215-221`), `clear_pool` is a no-op
(`:223-228`). The fallible methods correctly return `InvalidPool`/`InvalidHandle`
(`:143-198`); the test `invalid_pool_is_reported` (`:250-260`) pins both
behaviors.

This is a documentation over-reach against a deliberate, correct interface
contract — **not** a code bug — so it is a BACKFILL (fix the spec), never an
ALIGN. FR-015 was reworded to scope the "must report an error" requirement to
the fallible `Result`-returning methods, to state that the query methods carry
no error channel and MUST degrade safely on a non-existent domain, and to retain
the blanket "in no case may any operation corrupt tracking state."

## Spec: 001-session-list-eviction — Session-Lineage Eviction Policy

### Aligned ✓

| Req | Evidence |
|-----|----------|
| FR-001 register returns handle | `src/lib.rs:123-141` (`track` → `EvictionHandle::new`) |
| FR-002 parent = current leaf, new block becomes leaf | `src/session_list.rs:90-119` (`register`) |
| FR-003 independent per-session chains | `sessions: HashMap<SessionId,u32>` `src/session_list.rs:49,97,114` |
| FR-004 touch refreshes recency | `src/session_list.rs:123-137`; `src/lib.rs:143-155` |
| FR-005 batch refresh | `src/lib.rs:157-184` (`batch_touch`) |
| FR-006 no refresh on eviction | `evict_oldest`→`unlink` never ticks; `src/session_list.rs:190-193,144-185` |
| FR-007 only leaves evictable | `leaves: BTreeSet<(stamp,idx)>` `src/session_list.rs:51,196-202` |
| FR-008 select+remove oldest leaf / None when empty | `src/session_list.rs:190-193` |
| FR-009 parent promoted to leaf after eviction | `src/session_list.rs:161-174` |
| FR-010 up to N candidates, no removal | `candidates()` `src/session_list.rs:196-202` |
| FR-011 remove relinks interior child→parent | `unlink()` `src/session_list.rs:144-185` |
| FR-012 deterministic tie-break | monotonic `clock` + `(stamp,index)` total order `src/session_list.rs:62-65` |
| FR-013 len per pool | `src/lib.rs:215-221` |
| FR-014 clear pool | `src/lib.rs:223-228`; `src/session_list.rs:220-228` |
| FR-015 fallible ops report error; query ops degrade safely | fallible: `InvalidHandle`/`InvalidPool` `src/lib.rs:143-198`; queries degrade `src/lib.rs:200-228`; test `invalid_pool_is_reported` `src/lib.rs:250-260` (see "Resolved this sweep") |
| FR-016 interface-only surface | all behavior via `IEvictionPolicy` impl `src/lib.rs:102-229` |
| FR-017 idempotent re-registration | `src/session_list.rs:91-94` |
| FR-018 single linear chain, one leaf | enforced by parent/child single links + `check_invariants` `src/session_list.rs:233-314` |
| SC-004 correctness (never non-leaf; oldest leaf; deterministic) | property tests `tests/lineage_properties.rs`, unit tests `src/session_list.rs:412-463` |
| SC-006 invariant consistency after any op sequence | `check_invariants` + randomized test `src/session_list.rs:514-564` |
| SC-002/003/005 perf targets | O(1)/O(log L) structure by construction (arena + BTreeSet); design-consistent |

Observability announce log (`src/lib.rs:107-120`) is documented in the spec's
Observability section — not unspecced.

### Drifted ⚠️

None actionable after this sweep.

- **SC-001** (lineage retention ≥15% vs LRU) — **not drift**. Spec text (line 116)
  already downgrades this to an aspirational design goal and records that the
  comparative trace-replay harness does not exist; the implementation is
  consistent with the spec's own acknowledgement. Building that harness is
  tracked in `.specify/sync/align-tasks.md` (Task 1). No code or spec change
  implied.

### Not Implemented ✗

None.

## Unspecced Features

None.

## Recommendations

1. No source change required. FR-015's wording is now consistent with the shared
   `IEvictionPolicy` contract; the component and spec are in sync.
2. When resources allow, build the session-lists-vs-LRU trace-replay harness to
   convert SC-001 from a design goal into a measured outcome (align-tasks Task 1).
