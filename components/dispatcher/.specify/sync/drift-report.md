---
spec_sync_component: dispatcher
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-21T23:03:30Z
spec_sync_git_commit: efd502331
spec_sync_inputs_sha256: fddbf882a22464c075cd5197a08f22e8318a5c6638a1a65ec405b060fbb6dbf4
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Merge 2026-10-21: unstable `460a6f554` (#558, Daniel: last-chance full-depth eviction scan) merged into `fix/dispatcher-remove-clear-mt`.** #558 changes only `evict_for_space` and the fragmentation relief in `evict_and_insert` (scan depth), with its own spec backfill (FR-024, eviction section, scenarios 2-3, Session Q&A; note below). This branch's changes in the same file are elsewhere: `remove`, `clear_memory_tier`, `flush_to_ssd`, and the `store_already_resident` counter, which A-07 moved out of `evict_and_insert` into `reserve_memory`. The only conflicts were the test-module list in `src/lib.rs` (both kept), the 001 `**Last Synced**` header lines (both kept), and this report (both notes kept). The merged code was re-checked: the counter is not in `evict_and_insert`. #558's new test `src/repair_test_eviction_window_tinylfu.rs` was written before this branch's `WriteThroughPins` constructor argument, so its `DispatcherComponent::new` call gets that one extra argument (test only). Dispatcher tests 102/102, 5 runs out of 5. Both syncs stand, and this re-stamp certifies the merged inputs.

> **Apply 2026-10-21 (branch fix/dispatcher-remove-clear-mt, approved by Cornel).** Approved 2026-10-21 ("groups 1, 3, 4"); applied 2026-10-21T22:53:16Z at HEAD `efd502331`. **Applied** with the exact proposal text; backup in `.specify/sync/backups/20261021T225316Z/`: **B-7** (001 FR-056 `flush_to_ssd`: return type `Result<usize, DispatcherError>`; the count is taken after the drain over `is_evictable` entries and is a lower bound; only `Ok` vs `IoError` is the guarantee), **B-8** (001 FR-017: a failed write-through is no longer silent; it is recorded per key and reported by `flush_to_ssd` as `IoError` while the key is resident, commit `9bd231d85`), **B-9** (`contracts/errors.md` `IoError` row lists `flush_to_ssd`). Spec 001 got a `Last Synced 2026-10-21` header. This resolves D-19, D-20, D-21 and U-08. Also recorded as **applied** (approved, applied by hand in the working tree before this run, `src/` not edited by the apply): interfaces proposals **5** (`clear_memory_tier` doc comment, which also clears the stale-doc item in the last bullet above) and **6** (`batch_lookup` doc comment). **Closed** in `align-tasks.md`: **A-07** (resolved in code by `efd502331`). **Became tasks**: none approved this run. **Stays open**, recorded in `align-tasks.md` with its TRACK note: **A-5** (P-06, FR-056 + FR-017: tie the failed-write record to the entry it describes; later repair, test first); and **interfaces 7** (the `dispatcher-p2p` port of the three repairs), recorded in `components/interfaces/.specify/sync/align-tasks.md` as a task for the component `dispatcher-p2p`. Carried over, unchanged: A-1, A-2, A-3 (decision), A-4, A-01, A-02, A-04, A-05, A-06, A-08, N-01. Drift stays **OPEN** until the code tasks land. No source files changed by this apply. The frontmatter is left for the stamping script.

> **Sync 2026-10-21 (branch fix/dispatcher-remove-clear-mt) — two more formal-verification repairs re-verified, drift OPEN.** Base `origin/unstable` `a25e9be29` (merged into the branch by `95cdcb5e5`), HEAD `efd502331`, plus the spec edits Cornel approved, which are in the working tree and not yet committed: spec 001 FR-056 `flush_to_ssd`, interfaces FR-018a, and the interfaces `IDispatcher::flush_to_ssd` doc comment. Since the last sweep (`ec4277c0e`, which analysed `cccfb328e`), the component's inputs changed only through two commits. `9bd231d85` (DIS-FLUSH-REPORTS-FAILED-WRITE, Spin J23) makes `flush_to_ssd` return `IoError` while an entry whose write-through failed is still resident, and makes it count the resident entries that have an SSD copy. `efd502331` (DIS-STORE-ALREADY-RESIDENT-COUNTS-STORES-ONLY, Cornel's decision A-07 = ALIGN) counts `store_already_resident` only in `reserve_memory`. The merge from `origin/unstable` touched nothing under `components/`, `lib/` or `apps/`. Both new hunks were re-read in full, together with every requirement that reads or writes the new state: the write-through ledger, `flush_to_ssd`, `remove`, `clear_memory_tier`, `reserve_memory`, `evict_and_insert` and the load paths that share it. The text applied on 2026-10-20 (FR-018, the remove edge case, FR-038, the writer Assumption, and the two contract rows) was checked line by line against the current code. All of it matches. Every other earlier verdict was re-anchored to HEAD and spot-checked. That puts 111 items in scope: the earlier 109, plus the FR-056 `flush_to_ssd` bullet and the `contracts/errors.md` `IoError` row, both newly counted because this branch changed what they should say. Results: 89 aligned, 8 drifted, 5 partially implemented, 9 not verifiable here, and 2 unspecced features (1 folded into a BACKFILL).
>
> - **Resolved in code:** D-11 / open decision **A-07**. `store_already_resident` is now counted only on the store path (`src/lib.rs:3505-3511`, `:3535-3543`; no count in `evict_and_insert`, `:1214-1221`; test `src/repair_test_store_already_resident.rs:474`). FR-058 and interfaces FR-018a (approved text) are aligned. A-07 can be closed in `align-tasks.md`.
> - **The approved FR-056 `flush_to_ssd` text matches the main behaviour** (`src/lib.rs:4006-4042`; tests `src/repair_test_flush_failed_write.rs:607,638`). Two gaps found adversarially: (D-19, minor) the count uses `IDispatchMap::is_evictable` (`components/dispatch-map/src/lib.rs:530-546`), so a persisted entry that is pinned at that moment is not counted, and the bullet still shows the return type as `-> usize`; (P-06, minor) a failure record is forgotten only once the key is no longer in the memory tier (`src/background.rs:376-379`). A re-reserved new incarnation of the key therefore inherits the old failure, and a landed write whose `convert_to_storage` failed is recorded as landed (`src/lib.rs:1401-1403`, `:1429`, `:1452`). **BACKFILL B-7, ALIGN A-5.**
> - **Text this branch made stale:** FR-017 still says a failed write-through is "silently" dropped. It is now recorded and surfaced by `flush_to_ssd` (D-20, **BACKFILL B-8**). `contracts/errors.md:36` does not list `flush_to_ssd` for `IoError` (D-21, **BACKFILL B-9**).
> - **Still open, re-verified unchanged at HEAD:** A-1 (`schedule_write_through` takes no pin; `src/lib.rs:4044-4066`, `JobStart::Untracked` `src/background.rs:303-305`, `:311-313`), A-2 (`remove` leaks a persisted resident entry's extent; `src/lib.rs:3192-3229`), A-3 (`clear_memory_tier` frees reserved slots; `:3974-3988`), A-4 (ledger give-back window; `src/background.rs:347-351`), A-01 (`:2803-2808`), A-02 (`:2488-2489`), A-04 (`:815-821`), A-05 (`:3668-3695`), A-06 (`:2111`), A-08 (still no value-specific test for `Ssd`/`Remote`/`SizeMismatch`/`Error`), N-01 (integrity-check).
> - **Outside this component's specs, recorded only:** `dispatcher-p2p` implements `flush_to_ssd` the old way: it returns the queue length from before the flush and never returns `IoError` (`components/dispatcher-p2p/src/lib.rs:2889-2902`). That breaks the interface contract as edited (`components/interfaces/src/idispatcher.rs:826-838`). The shm-queue reply field is still named `jobs_flushed` (`lib/shmq-dispatcher/src/wire.rs:46-47`, `src/translate.rs:901-910`). The interfaces `clear_memory_tier` doc comment is still stale (`idispatcher.rs:806-810`).
>
> **4 proposals (3 BACKFILL, 1 ALIGN, 0 other), all `approved: false`.** Nothing was applied. See `proposals.md`. The frontmatter is left exactly as it was: the stamping script owns it, and its `clean` stamp predates `9bd231d85` and `efd502331`.

> **Apply 2026-10-14 (branch fix/dispatcher-remove-clear-mt, approved by Cornel).** Approved 2026-10-20 ("let's finish the PR"); applied 2026-10-20T22:58:11Z. **Applied** with the exact proposal text; backup in `.specify/sync/backups/20261020T225811Z/`: **B-1** (FR-018: `remove` takes over the pending write-through's pin; who frees the slot and the extent afterwards), **B-2** (remove edge case: only a reader's or client's pin, or a write lock past the wait, refuses `remove`), **B-3** (FR-038 `clear_memory_tier`: `oldest_keys` scan, pinned entries keep their slot, exact count, 5 s `Timeout`), **B-4** (Assumption: only `copy_gpu_to_memory_completed` jobs hold a read reference; the `schedule_write_through` gap is stated), **B-5** (`contracts/errors.md` `Timeout` → `pin`/`clear_memory_tier`; `KeyNotFound` covers `remove` refusals), **B-6** (`contracts/idispatcher.md` `remove` takes no write reference; postcondition). Spec 001 got a `Last Synced 2026-10-14` header. This resolves D-06, D-10′, D-18, U-04, U-06 and U-07, and closes the earlier **A-03** and **N-04** in `align-tasks.md`. **Became tasks** in `align-tasks.md` (approved as ALIGN tasks only, no code change in this PR): **A-1** (FR-013: a `schedule_write_through` job must own the pin it settles, major), **A-2** (SC-003: `remove` must free the extent of a persisted resident entry), **A-3** (FR-056 vs FR-038: reserved-but-not-completed slots; the decision is recorded with recommendation (a), skip like a pinned entry), **A-4** (FR-018 residual handshake windows, LOW confidence). **Stays open**: no new item from this run, since all 10 proposals were approved. Carried over, unchanged: A-01, A-02, A-04, A-05, A-06, A-08 (tasks), and A-07 and N-01 (decisions). The interfaces-crate `clear_memory_tier` doc comment (the B-3 companion edit) is outside `specs/**` and was not edited. Drift stays **OPEN** until the code tasks land. No source files changed. The frontmatter is left for the stamping script.
>
> **Stamp: clean.** Every drift item of this analysis is resolved (specification updates applied with Cornel's approval) or tracked in `align-tasks.md` (ALIGN tasks A-1 to A-4; A-3 with the recommended option (a)).

> **Sync 2026-10-14 (branch fix/dispatcher-remove-clear-mt) — two formal-verification repairs re-verified, drift OPEN.** Base `origin/unstable` `af221dd82`, HEAD `cccfb328e`. The branch changes only `src/**`: `lib.rs`, `background.rs`, and two new `#[cfg(test)]` modules (`repair_test_remove_no_wait.rs`, `repair_test_clear_mt_pinned.rs`). `specs/**` and `components/interfaces/**` are unchanged. Apart from these hunks, `src/**` on the base is byte-identical to `4f8b5554b`, the HEAD of the previous full sweep. So every earlier verdict was re-anchored to the current line numbers and spot-checked. Every requirement whose code is in, or calls into, the changed hunks was re-read in full: remove, the populate→complete→write-through chain, the background writer, `clear_memory_tier`, `schedule_write_through`, and the evictors that race with them. That is 109 items in scope (the earlier 108, plus the Assumption on the writer's read reference). Results: 86 aligned, 9 drifted, 5 partially implemented, 9 not verifiable here, plus 4 unspecced features. Three of the unspecced features are folded into BACKFILL proposals; the fourth, integrity-check, is the still-open N-01.
>
> - **`remove` no longer fails while the key's write-through is pending** (`0236a3c98`, `src/lib.rs:3203-3218`, `WriteThroughPins` `src/background.rs:244-366`). The earlier sweep counted FR-018 as aligned. That was wrong: on the base, `dm.remove` refused the write-through's read pin, so `remove` returned `KeyNotFound` and left the entry cached. FR-018 is now met. The ledger and the job outcomes (stale job dropped; a running job frees the slot and the extent it wrote) are not in the spec. **BACKFILL** of FR-018, of the "remove" edge case (now partly stale), of the writer-pin Assumption, and of `contracts/idispatcher.md:45`.
> - **`clear_memory_tier` keeps the slot of a pinned entry** (`cccfb328e`, `src/lib.rs:3936-3986`). This resolves the earlier major D-06 / align task **A-03**. FR-038 still describes `evict_next()` / `convert_memory_tier_to_block` / unconditional removal, and says nothing of the new `Timeout` result or of the "count = entries really cleared" rule. **BACKFILL** FR-038 and `contracts/errors.md:37` (`remove` never returns `Timeout`; `clear_memory_tier` and `pin` do).
> - **New findings from reading the changed paths adversarially** (the earlier sweep missed all four; none is caused by this branch, but #4 is inside its new code):
>   1. **FR-013 / Assumption, major (plausible):** a `schedule_write_through` job (the remote-lookup publish path) takes no pin. Its `convert_to_storage` / `release_read` (`src/lib.rs:1442-1455`) therefore decrements a concurrent reader's pin. The job also reads a slot that `remove` / `clear_memory_tier` may free. The branch codifies this as `JobStart::Untracked` (`src/background.rs:279-280`). **ALIGN.**
>   2. **SC-003, moderate:** `remove` of a resident entry that is already persisted (MemoryTier with `ssd_offset`) never frees its SSD extent. `LookupResult::MemoryTier` carries no offset (`src/lib.rs:3189-3199`). With `format_on_init=false` the removed key is resurrected at the next start (`src/lib.rs:1975-1976`). **ALIGN** (needs an interface addition).
>   3. **FR-056, moderate:** `clear_memory_tier` frees slots that were reserved but not yet completed (`try_evict_to_block` and `dm.remove` both return `KeyNotFound`, and the code treats that as "done") while their D2H copy may still be running (`src/lib.rs:3957-3969`). **ALIGN, decision needed.**
>   4. **FR-018, minor (plausible, reasoning only):** while `WriteThroughPins::remove_entry` gives the pin back (`src/background.rs:342-346`), the running job's pin is briefly not held. A second `remove` or a `clear_memory_tier` in that window can free the slot under the job. **ALIGN, LOW confidence.**
> - **Still open from the 2026-10-14 bundle sync, re-verified unchanged:** A-01 (warm-sync failure counted as `Dram` hits, `src/lib.rs:2799-2804`), A-02 (`batch_lookup` cold pin, `:2482-2483`), A-04 (no-drives promote, `:815-821`), A-05 (phase-3 release, `:3657-3678`), A-06 (SSD-evictor sender snapshot, `:2108`), A-07 decision (`store_already_resident`, `:1222`; this branch adds one more load-unrelated source: re-storing a key while its removed write-through still runs), A-08 (per-value `ServedBy` tests), N-01 (integrity-check). The BACKFILL proposals B-2, B-5 and B-6 resolve **N-04**: the reader-pin and timeout results of `remove` become specified.
>
> **10 proposals (6 BACKFILL, 4 ALIGN, 0 other), all `approved: false`**; nothing applied. See `proposals.md`. The frontmatter is left exactly as it was: its `clean` stamp certifies `4f8b5554b`, not this branch, and the stamping script owns it.
> **Sync 2026-10-19 (branch `fix/dispatcher-eviction-last-chance-scan`) — delta, drift resolved by backfill.** Base `unstable` `50db574c`. One `src/lib.rs` change: `evict_for_space` retries a regular-depth eviction scan that frees nothing once at `EVICT_SCAN_MAX` (1024) before `AllocationFailed`, and `evict_and_insert`'s fragmentation relief scans to `EVICT_SCAN_MAX` (constants `EVICT_SCAN_STEP`/`EVICT_SCAN_MAX` replace the local `MAX_SCAN`). Previously the first miss returned immediately, so the advertised widening (`MAX_SCAN × attempts`) never took effect and four unpersisted entries at the LRU head hid all demotable entries; `eviction-policy-optimized`'s TinyLFU front-admission produces exactly that head under store bursts. CODE authoritative; SPEC BACKFILL in spec 001: FR-024, the eviction section (line 125 area), acceptance scenarios 2–3, Session Q&A, plus a `Last Synced` entry. New test module `src/repair_test_eviction_window_tinylfu.rs` (fails on base, passes here). No other FR/SC in scope; prior open items in `align-tasks.md` unchanged. Digest recomputed over a `git archive` of `f97a114a0` (CI parity verified, no untracked files under hashed roots); status `clean`.

> **Apply 2026-10-14 (branch fix/bundle-dispatcher-ems, approved by Cornel).** **Applied** to `specs/**`, exact proposal text, backup in `.specify/sync/backups/20261019T070414Z/`: **B-01** (FR-036/FR-051 lookup pin through promotion + race recovery), **B-02** (FR-032 + edge case, SSD evictor runs `dm.remove` first), **B-03** (FR-003/FR-059 slot released after post-reserve failure), **B-04** (FR-056 `copy_gpu_to_memory_completed` keeps a live slot), **B-05** (FR-042/FR-050 all eviction paths count drops; stale `Removed` clause), **B-06** (ring-buffer fallback removed, 5 sites), **B-07** (FR-037 stream lifetime), **B-08** (remove order), **B-09** (no `dma_copy_to_host`), **B-10** (002 FR-008 sub-paths), **B-11** (US-11 AS4), plus the notes **N-02** (FR-045 `CERTUS_LOG_REMOTE_DELIVERY`) and **N-03** (FR-056 `CERTUS_LOG_RESERVE`). Both specs got a `Last Synced 2026-10-14` header. This resolves D-01…D-05, D-08, D-09, D-10, D-12, D-14, U-02, U-03, U-05 and the US-11 AS4 note. **Became tasks** in `align-tasks.md` (approved as ALIGN tasks only, no code change in this PR): **A-01** (002 warm-sync failure → `ERROR`, major), **A-02** (`batch_lookup` cold pin, major), **A-03** (`clear_memory_tier` pinned slot, major), **A-04** (no-drives promote re-register), **A-05** (phase-3 release in `copy_gpu_to_memory_completed`), **A-06** (SSD-evictor sender wiring), **A-08** (002 per-value attribution tests). **Stays open** (not approved, recorded in `align-tasks.md` with their notes): **A-07** (FR-058 `store_already_resident`, ALIGN or BACKFILL is Cornel's decision), **N-01** (new FR-062 integrity-check verify-before-serve, MEDIUM), **N-04** (`remove` on a pinned entry, MEDIUM, deferred to the remove-no-wait PR). Drift stays **OPEN** until the code tasks land. No source files changed. The frontmatter is left for the stamping script.
>
> **Stamp: clean.** Every drift item of this analysis is resolved (specification updates applied with Cornel's approval) or tracked in `align-tasks.md` (ALIGN tasks and open items, including the decision A-07 where it applies).

> **Sync 2026-10-14 (branch `fix/bundle-dispatcher-ems`) — full re-verification, drift OPEN.** Base `origin/unstable` `acd240255`, HEAD `4f8b5554b`. This branch changes only `src/**` (lib.rs, background.rs and six `repair_test_*` modules). `specs/**` and `components/interfaces` are unchanged, so the frontmatter digest is stale until a spec edit and re-stamp are made (the frontmatter is left exactly as it was; a script re-stamps it). Unlike the delta sweeps below, **every** active FR/SC of both specs was checked against the code. That is 117 items: 108 analysed and 9 out of this component's scope. The six dispatcher repairs on the branch (found by formal verification) are all real fixes. Five of them change behaviour the spec does not yet describe. The sixth (`a7ccc6e48`) makes the code do what spec 002 already says:
>
> - **Cold `lookup` keeps its read pin through promotion** (`1d24ff27f`, `src/lib.rs:3072-3103`) and recovers a lost `mt.insert` race inline. This is a BACKFILL of FR-036/FR-051. **The same pin drop still exists in `batch_lookup`** (`src/lib.rs:2434`). That is the shm-queue `LOOKUP` path. It is an ALIGN finding (major), safe only when the caller pins first (Check→Pin).
> - **SSD evictor runs `dm.remove` before `mt.remove`** (`1d24ff27f`/`d450c7cc1`, `src/background.rs:349-358`). BACKFILL of FR-032 and of the SSD-evictor edge case.
> - **`populate` releases its slot on every failure after it reserves memory** (`9defa9e1a`, `src/lib.rs:3215-3246`). BACKFILL of FR-003/FR-059. The phase-3 failures inside `copy_gpu_to_memory_completed` (integrity-check path) are still not covered. That is ALIGN.
> - **`copy_gpu_to_memory_completed` keeps the live slot on `AlreadyExists`** (`ad3654207`, `src/lib.rs:3560-3585`). BACKFILL of FR-056.
> - **The background evictors count events that do not fit the channel** (`0d3fcde43`, `EvictionSender`, `src/background.rs:225-241`). BACKFILL of FR-042/FR-050. The SSD evictor takes a copy of the sender at `initialize()` (`src/lib.rs:2059`), and both servers create the channel *after* `initialize()`. So the SSD evictor still sends and counts nothing in production. ALIGN.
> - **The concurrent-promotion recovery attributes `Dram`** (`a7ccc6e48`, `src/lib.rs:2946-2954`). Spec 002 FR-009/FR-010 are now aligned (they were drifted on base).
>
> Pre-existing drift found by the full sweep, not caused by this branch:
> - spec 002 US1-AS3/FR-009: a failed warm batched sync is only logged, and the keys are still reported as `Dram` hits (`src/lib.rs:2750-2755`). **Major.**
> - FR-038 `clear_memory_tier` frees the slot of a pinned entry and leaves a dangling dispatch-map pointer (`src/lib.rs:3880-3888`). **Major.**
> - FR-028 / US-9 AS3: the no-drives `promote_and_serve` never re-registers the entry (`src/lib.rs:814-820`).
> - FR-058: `store_already_resident` is also counted on load paths.
> - FR-019 / US-9 AS1: names a ring-buffer fallback that does not exist.
> - FR-037: says per-device streams are destroyed at shutdown; they are not.
> - Edge case "remove order" is the wrong way round.
> - FR-050: "Removed on transition failure" is stale.
> - FR-003 names `dma_copy_to_host`, which populate no longer calls.
> - spec 002 FR-008: lists "inline fallback", which was removed.
> - spec 002 FR-027: value-specific tests exist only for `Dram`/`Miss`.
> - Five unspecced features (integrity-check serve path, remote-delivery probe, reserve logging, remove-of-pinned-entry result, SSD-evictor `Removed` events).
>
> Totals: 108 requirements in scope. 81 aligned, 15 drifted, 3 partially implemented, 9 not verifiable without hardware or a cluster, plus 5 unspecced features. **23 proposals (11 BACKFILL, 8 ALIGN, 4 NEW_SPEC), all `approved: false`**; nothing applied. See `proposals.md`.

> **Re-stamp 2026-10-14 (interfaces spec corrected, branch `fix/cold-staging-buf-16mib`).** The FR-033 reconciliation below is unchanged; this re-stamp only refreshes the digest after the matching 4 MiB → 16 MiB correction to interfaces FR-027, which folds into this component's hash. No code re-analysis needed.

> **Sync 2026-10-14 (branch `fix/cold-staging-buf-16mib`).** Delta analysis on a certified baseline: the prior clean stamp was for `547107f3`, and the only input that changed between it and HEAD (`8e12e49f`) is `components/interfaces/src/idispatcher.rs` — the `cold_staging_buf_bytes` default. This component's own `src/**` and `specs/**` were otherwise unchanged, so the one in-scope question is every FR/SC that constrains the staging-buffer size. Spec 001 in scope.
>
> - **FR-033 `cold_staging_buf_bytes` default — DRIFTED (moderate).** Spec said the default was `4 MiB = 4 × 1024 × 1024`; commit `8e12e49f` raised the shipped default to `16 * 1024 * 1024` (`components/interfaces/src/idispatcher.rs:125`). **Resolution**: code authoritative. `serve_cold_staged` only serves a tier-full cold load through a staging buffer when it fits the buffer (`total_bytes <= sp.buf_bytes()`, `src/lib.rs:777,934`); a 64-token KV-offload block is 8 MiB per worker, so a 4 MiB buffer made that guard always false, every tier-full cold load returned `AllocationFailed`, and vLLM's `OffloadingConnector` escalated it to a fatal `EngineDeadError`. FR-033 updated to 16 MiB with a dated note recording the guard, the failure it fixes, and the 64 × 16 MiB = 1 GiB pinned-host-DMA footprint (up from 256 MiB).
> - **FR-053 / FR-011 (spec 002) / cold-load-staging definitions — ALIGNED, verified not assumed.** All are parameterized on `cold_staging_buf_bytes` by name and carry no hardcoded byte figure, so the default change does not touch them. A grep of `specs/**` for any numeric staging footprint (`4 MiB`, `256 MiB`, `8 MiB`, `per-block transfer`) found only the FR-033 figure above.
>
> No actionable drift remains for this component after apply.

> **Re-stamp 2026-10-09 (merge of `fix/evict-blocked-by-pin` into `fix/dispatcher-p2p-clean-eviction`).** Both branches stamped this report; the merged inputs are the FR-035 narrowing in spec 002 (p2p branch) plus the `IDispatchMap::try_evict_to_block` doc-comment change in `components/interfaces` (fix branch). This component's `src/**` is unchanged by either. Both notes below remain accurate.

> **Re-stamp 2026-10-09 (doc-only: FR-035 narrowed, branch `fix/dispatcher-p2p-clean-eviction`).**
> The previous clean stamp was re-verified to equal the spec-sync hash of `git archive 3c001478`.
> The only new input is spec 002 FR-035, whose claim that `dispatcher-p2p` reports none of
> FR-033/FR-034 became false when that component started counting its foreground eviction
> paths. FR-035 now says what remains unmeasured there (the background `MemoryTierEvictor`).
> **No code re-analysis was performed for this component, and none was needed:** its `src/**`
> is unchanged and `components/interfaces` is unchanged.

> **Re-stamp 2026-10-09 (branch `fix/evict-blocked-by-pin`; inputs changed transitively via `components/interfaces`).** This component's own `src/**` and `specs/**` are unchanged. Checked FR-033 (spec 002) against the change: it requires `evictions_blocked_by_pin` to count `ActiveReferences` from `try_evict_to_block`, and `evict_one_clean` matches that variant; the real dispatch map returned `InvalidState` for a held pin, so FR-033 was not met in production until this branch. It is now met without any change here.

> **Sync 2026-10-08 (branch `opt/evolve-latency/s20261004-001925_PR507`).** Delta analysis on a certified baseline: the existing clean stamp (`d3209328`) equals the spec-sync hash of unstable `7e53d5b5`, and `components/interfaces` is unchanged on this branch, so the only new inputs are this branch's dispatcher `src/` and `specs/` changes. Spec 001 in scope.
>
> - **FR-039 step (2) — DRIFTED (moderate).** Spec said the single warm `stream_synchronize` runs "after all hot-path copies are issued"; since `ada87967` it runs after the cold scatter-gather (`src/lib.rs:2742-2748`), the warm read pins are released only after it (`src/lib.rs:2754`), and the remote lookup follows (`src/lib.rs:2756`). **Resolution**: code authoritative; step (2) rewritten with the pin-lifetime reason and the benign multi-region warm-stream sync noted.
> - **User Story 11 scenario 2 — DRIFTED (minor).** "Hot entries are served inline without waiting for cold promotions" no longer describes when hot copies are confirmed. **Resolution**: reworded; scenario 6 added matching test `mixed_batch_defers_warm_sync_past_cold_block` (`src/lib.rs:6064`).
> - **FR-019 / FR-061 / Pipelined Reader entity / pipelined-reader Q&A — ALIGNED, verified not assumed.** Updated on this branch for `8825678d`; the scatter-gather poll loop has no periodic sync and makes one final dual pipe-stream sync (`src/pipeline.rs:1124-1125`). The single-drive `pipelined_multi_object_zero_copy` periodic sync is unchanged, as the spec states.
> - **Out of scope (not analyzed here):** the TouchCheck opcode (`a7e0ffbf`) and the touch()/lookup() bulk codec (`2fae06c8`) live in `lib/shmq-dispatcher` and `certus-shmq-connector`, which have no `specs/` tree; they are not inputs to this component's hash.
>
> No actionable drift remains for this component after apply.
> **Sync 2026-10-05 (branch `fix/store-declines-root-cause`).** Delta analysis on a certified baseline (see the interfaces report for the baseline argument). Both specs in scope.
>
> - **FR-058 `TierEventStats` cardinality — DRIFTED (major).** The spec fixed the struct at "**ten** `u64` fields" and enumerated them; the shipped struct has **sixteen** (the base had exactly ten, so the count was correct until this branch). The clause "each monotonic since process start" was also falsified: `oldest_sampled` and `oldest_persisted` are *assigned* the latest sample, not accumulated, and the server publishes them as Prometheus gauges. **Resolution**: code authoritative. Count corrected to sixteen, the monotonicity claim qualified to exclude the two gauges, and all six new fields enumerated with a cross-reference to the spec that specifies each — so this inventory stays the complete list without duplicating normative text.
> - **IDispatcher method inventory (spec 001, "Durability / introspection") — DRIFTED (moderate).** Listed `flush_to_ssd` and `read_write_stats` only, omitting the new `schedule_write_through`. **Resolution**: added, with the non-blocking contrast against `flush_to_ssd` stated and the contract and caller cross-referenced.
> - **`oldest_sampled` / `oldest_persisted` — UNSPECCED (moderate).** Two new public fields and two published gauges appeared in **no spec anywhere**, carrying only interface doc comments. **Resolution**: new **FR-036** in spec 002, beside the eviction-scan requirements it belongs with — gauge semantics, the reason the pair measures something FR-033's counters cannot, and the fact that it is the instrument that located the write-through defect.
> - **FR-033 — ALIGNED, verified not assumed.** The call site matches `DispatchMapError::ActiveReferences(_)` as a distinct arm (`src/lib.rs:1073`) rather than testing `is_ok()`, which FR-033 explicitly rules out.
>
> No actionable drift remains for this component.
> **Sync 2026-10-05 (branch `fix/poller-cpu-placement-logging`).** Delta analysis on a certified baseline: this component's existing clean stamp was re-verified to equal the spec-sync hash of `git archive origin/unstable`, so the only new inputs are this branch's src/specs/interfaces changes. Every FR/SC touching CPU placement, NUMA pinning, threads or SPDK init was located by grep over specs/** and re-checked against the changed code.
>
> - **FR-011 — BACKFILL (moderate).** `None` selects NUMA-local automatic placement: round-robin over each drive's NUMA-node cores excluding the node's first two (this branch; previously global CPUs 0/1 were excluded); unresolvable node or node with <=2 cores -> no CPU passed, block device chooses; topology discovery failure -> warning, unpinned. `components/dispatcher/src/lib.rs:550-600, 1530-1539`. *Pre-existing drift (the None path was already automatic on unstable), surfaced because this branch modifies that path.* Resolution: FR-011 rewritten for the None case; Last Synced header added.
>
> No actionable drift remains for this component after apply.

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

> **Re-stamp 2026-09-29 (served_by Phases 2-3).** `ServedBy` + `LookupOutcome` in
> `interfaces`, `batch_lookup` widened, the `LOOKUP` byte carrying the tier, and
> `TierEventStats` from eight fields to ten (the route partition). Digest 127cc2e8f652….

> **Re-stamp 2026-09-29 (feature 002 Phase 1: counters, FR-058 sync, taxonomy
> collapse).** Branch `certus-lookup-observability`, layered on the accounting fixes
> re-stamped above. Adds the two remote-lookup counters to `TierEventStats` (so FR-058
> goes from six fields to eight), the responder-side serve counters, and the collapse
> of the two remote taxonomy values into one. The appended sweep below carries the
> findings and, importantly, the reason its method differs from the workload
> generator's: this component cites 4 requirement ids across 120 defined, so
> citation-counting proves nothing here.

> **Re-stamp 2026-09-29 (lookup-accounting fixes; spec backfilled with the behaviour
> change).** Branch `fix/lookup-accounting`. Two defects in the server's lookup
> accounting: `batch_lookup` collapsed every `RemoteLookupError` into `IoError`, so a
> key no peer holds was reported as an I/O failure and `certus_lookup_misses_total`
> read 0; and the transport host dropped both entries held back before dispatch and
> every non-`KeyNotFound` error, so hits + misses did not account for what the client
> asked for. CODE (authoritative): `NotFound` now surfaces as `KeyNotFound` while
> `TransportError` stays `IoError`; every dispatched entry lands in exactly one of
> hits / misses / errors. SPEC BACKFILL: spec 001 FR-011 gains the unsuccessful-fetch
> case it never stated, and `contracts/errors.md` is corrected for both variants.
> Digest recomputed over this branch's tree (58c590e9c829…); the previous stamp
> (`9c0c88f5…`) predated these `src/**` and `specs/**` edits and was stale.

> **Re-stamp 2026-09-25 (SSD-evictor drive-selection bug fix; drift resolved by backfill).** Branch `fix/ssd-evictor-drive-index-hash`. The background SSD-utilization evictor (`background.rs`, `run_ssd_eviction`) selected the extent manager to free from with a raw `key % num_drives`, while placement/write-through and the inline `remove(key)` path use the splitmix64 `drive_index(key, num_drives)` (FR-039(4), FR-009). The two disagree for almost all keys when `num_drives > 1`, so the evictor freed the wrong extent manager — leaking the intended extent and potentially freeing a live extent for another key on the wrong drive. CODE (authoritative): both call sites now route through `drive_index` (made `pub(crate)`). SPEC BACKFILL: the Session Q&A "How does the SSD evictor determine drive ownership for extent removal?" previously answered `key % num_drives` "matching the write-through path" — a claim that was false pre-fix; it was rewritten to describe `drive_index` and record the prior drift. `src/lib.rs` + `src/background.rs` changed (moved the digest); `spec.md` Q&A changed (moved it again). Digest recomputed over a clean tree matching CI; drift status `clean`.
> **Stamp provenance.** `spec_sync_git_commit` is the HEAD (`e4b97a0b`) the digest
> was computed against; this report is committed together with the `spec.md`
> backfill, the `src/lib.rs` edits, and the `src/cold_pool.rs` deletion it
> certifies, so the tree the next commit lands is exactly the tree the digest
> covers. The CI Spec-Sync Gate re-runs `scripts/spec-sync-hash.sh
> components/dispatcher` over `src/**` + `specs/**` (with `components/interfaces/`
> folded in) and matches `spec_sync_inputs_sha256`; the digest here was recomputed
> over a clean tree (no untracked files under `src/`/`specs/`) after the last edit.

# Spec Drift Report — dispatcher (branch `fix/dispatcher-remove-clear-mt`, second sweep)

Generated: 2026-10-21
Project: `components/dispatcher` (crate `dispatcher`)
Specs: `specs/001-dispatcher-cache-interface/spec.md` (419 lines), `specs/002-served-by-tier-attribution/spec.md` (779 lines)
Branch: `fix/dispatcher-remove-clear-mt`. HEAD `efd502331`, base `origin/unstable` `a25e9be29` (merged into the branch by `95cdcb5e5`). Approved spec edits that are not yet committed are in the working tree (see below).
Mode: read-only drift analysis, batch (no questions). Nothing outside `.specify/sync/` was edited.

## Scope and method

**Inputs that changed since the last sweep** (`ec4277c0e`, the apply of the 2026-10-20 analysis of `cccfb328e`):

| Commit | Finding id | Code change |
|---|---|---|
| `9bd231d85` | DIS-FLUSH-REPORTS-FAILED-WRITE (Spin J23; Loom) | `WriteThroughPins` also keeps a `failed: HashSet<CacheKey>` (`src/background.rs:262-263`). `record_write` (`:363-372`) and `failed_writes` (`:374-380`) maintain it. `process_write_job` returns `written.is_some()` (`src/lib.rs:1394-1404`). `run_write_job` records the outcome of Untracked jobs (`:1419-1423`) and of owned jobs (`:1429`). `flush_to_ssd` drains the queue, returns `IoError` if a resident key's latest write failed, and otherwise counts the resident keys for which `is_evictable` is true (`src/lib.rs:4006-4042`). Tests: `src/repair_test_flush_failed_write.rs:607,638`. |
| `efd502331` | DIS-STORE-ALREADY-RESIDENT-COUNTS-STORES-ONLY (decision A-07 = ALIGN; Loom) | The counter is removed from `evict_and_insert` (`src/lib.rs:1214-1221`) and added to both `AlreadyExists` exits of `reserve_memory` (`:3505-3511`, `:3535-3543`). The unit test now calls `reserve_memory` (`src/lib.rs:6696`). New test `src/repair_test_store_already_resident.rs:474`. |

`git diff --stat ec4277c0e..HEAD -- components lib apps` lists only those four files. The merge commit `95cdcb5e5` brought in only `knowledge/`.

**Approved spec edits in the working tree** (`git diff -- components/dispatcher/specs components/interfaces`). This sweep checks them but does not propose to undo them:
- spec 001 FR-056 `flush_to_ssd` bullet (`spec.md:346`);
- interfaces `001-interfaces` FR-018a, the `store_already_resident` paragraph (`components/interfaces/specs/001-interfaces/spec.md:331`);
- the `# Errors` section of the `IDispatcher::flush_to_ssd` doc comment (`components/interfaces/src/idispatcher.rs:832-837`).

**Method.**
- (1) Both new hunks were re-read in full, with the semantics of the real dependencies they rely on:
  - `DispatchMap::is_evictable` requires `read_ref == 0 && write_ref == 0 && MemoryTier { ssd_offset: Some }` (`components/dispatch-map/src/lib.rs:530-546`).
  - `convert_to_storage` decrements any read reference (`:176-196`).
  - `MemoryTier::oldest_keys` is the eviction policy's `peek_front_n` over every tracked key, reserved-but-not-completed slots included (`components/memory-tier/src/lib.rs:435-443`; `components/eviction-policy-lru/src/lru_list.rs:107-118`).
  - `MemoryTier::contains` is "a slot exists" (`components/memory-tier/src/lib.rs:568-575`).
  - A `WriteHandle` dropped before `publish` aborts its extent (`components/interfaces/src/iextent_manager.rs:149-155`).
- (2) Adversarial question for the failure record: for each actor that creates, frees or re-creates a slot for a key, does `flush_to_ssd` report exactly the resident entries that have no SSD copy because their write failed? For the counter: which callers reach each `AlreadyExists` exit? (`reserve_memory` callers: `src/lib.rs:3255`, `:3368`, `lib/shmq-dispatcher/src/translate.rs:531`. Load paths that call `evict_and_insert`: `promote_and_serve`, `batch_lookup` cold-prep.)
- (3) The text applied on 2026-10-20 (B-1…B-6) was compared clause by clause with the current code.
- (4) Every other verdict was re-anchored to HEAD. Offsets in `lib.rs`: −3 at `1214-1221`, +3 after `1404`, +17 after `3545`, +40 after `4004`. Offsets in `background.rs`: +5 after `263`, +24 after `381`. Each anchor cited below was spot-checked at its new line.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 120. That is 111 in this component's scope: the active FRs and SCs of both specs, the acceptance scenarios and edge cases named below, the writer Assumption, and, newly counted, the FR-056 `flush_to_ssd` bullet and the `contracts/errors.md` `IoError` row. The other 9 items of spec 002 belong to other components and are only listed. |
| ✓ Aligned | 89 (80.2% of 111) |
| ⚠️ Drifted | 8 (7.2%). 3 are new, all BACKFILL: D-19 (FR-056 `flush_to_ssd` bullet), D-20 (FR-017), D-21 (`contracts/errors.md` `IoError`). 5 are carried over as ALIGN tasks: D-16/A-1, D-17/A-2, D-07/A-04, D-13/A-01, D-15/A-08. |
| ◐ Partially implemented | 5 (4.5%), all carried over: P-01/A-05, P-02/A-06, P-03/A-02, P-04/A-4, P-05/A-3. The new P-06 (ALIGN A-5) is a second finding on the FR-056 `flush_to_ssd` item, so it is not counted again. |
| ✗ Not implemented | 0 |
| ⧗ Not verifiable here (hardware / cluster / benchmark) | 9 (8.1%) |
| 🆕 Unspecced Code | 2: U-08 (the failed-write record; folded into **B-8**) and U-01 (integrity-check, open item N-01) |

Changes since the 2026-10-20 sweep (86 / 9 / 5 / 9 of 109):
- D-06, D-10′ and D-18 became aligned once B-1…B-6 were applied (`ec4277c0e`).
- D-11 became aligned through the code (`efd502331`).
- FR-017 moved from aligned to drifted (D-20).
- Two items were added (D-19, D-21).

What this branch's two new commits mean for the spec (one line each):
1. `efd502331`: FR-058 and interfaces FR-018a are now **aligned**, which closes open decision **A-07**. One residual case stays with A-4(b): a re-store while a removed write-through still holds the slot is counted although `check()` says the key is absent (`src/lib.rs:3535-3543`).
2. `9bd231d85`: the approved FR-056 text is met in its main case. Two details are not right yet: the count predicate and the `-> usize` signature (**BACKFILL** B-7), and a failure record that outlives its entry (**ALIGN** A-5).
3. `9bd231d85`: FR-017's "silently drops" is now half true. The store's caller is still not told, but `flush_to_ssd` is (**BACKFILL** B-8). `contracts/errors.md:36` lacks `flush_to_ssd` (**BACKFILL** B-9).

## Detailed Findings

### Spec: 001-dispatcher-cache-interface — Dispatcher Cache Interface (Memory-Tier Architecture)

#### Aligned ✓

*Verified against the text applied on 2026-10-20 (B-1…B-6), clause by clause:*
- **FR-018** (`spec.md:299`). This item was ◐ P-04 last time. Its main case is now specified, and P-04 stays a partial finding on it (see below).
  - The pin is recorded before enqueue: `src/lib.rs:3701-3712`, `WriteThroughPins::pin` `src/background.rs:293-298`.
  - The pin is taken over on `ActiveReferences`: `src/lib.rs:3206-3215` → `remove_entry` `src/background.rs:340-361`. The pin is given back if a reader holds the key: `:348-351`.
  - A queued job is dropped: `start` → `Stale` `:306-310`.
  - A running job frees the slot and extent: `src/lib.rs:1433-1439`, and `remove` skips its own `mt.remove` (`:3219-3221`).
  - Normal settle: `:1445-1458`.
- **Edge case "remove"** (`spec.md:235`): the order is entry → slot → extent (`src/lib.rs:3206-3229`). `KeyNotFound` for a reader's or client's pin (`:3213`) and for a write lock that outlasts the wait (`:3201`).
- **FR-038** (`spec.md:319`) → `clear_memory_tier` `src/lib.rs:3940-4004`:
  - window `oldest_keys(64 + pinned)` `:3967`;
  - `try_evict_to_block` first `:3974`, `ActiveReferences` kept `:3976`;
  - `dm.remove` before `mt.remove`, kept on `ActiveReferences` `:3980-3983`;
  - exact count `:3985-3988`; 50 µs pause `:4000`; 5 s `Timeout` `:3993-3998`.
  - *(The reserved-slot case is still P-05.)*
- **Assumption, write-through read reference** (`spec.md:418`): `downgrade_reference` `src/lib.rs:3694` + ledger `:3703`. `schedule_write_through` takes no pin (`:4044-4066`). The gap is stated in the spec and tracked as A-1.
- **`contracts/errors.md`** `KeyNotFound` and `Timeout` rows (`:33`, `:37`) → `remove` `src/lib.rs:3201`, `:3213-3214`; `pin` `:3742-3744`; `clear_memory_tier` `:3995`. **`contracts/idispatcher.md:171`** (the `remove` postcondition) → `:3206-3232`.

*Resolved in code by `efd502331`:*
- **FR-058 `store_already_resident`** (`spec.md:356`, "reserves refused because the key was already cached rather than for want of space") and **interfaces FR-018a** (approved text):
  - `reserve_memory` counts the refusal on both exits (budget 0: `src/lib.rs:3505-3511`; retry loop: `:3535-3543`).
  - `evict_and_insert`, which the load paths share, no longer counts it (`:1214-1221`).
  - Every caller of `reserve_memory` is on the store path: `populate` `:3255`, `batch_populate` `:3368`, shm-queue `op_reserve` `lib/shmq-dispatcher/src/translate.rs:531`.
  - Tests: `a_reserve_refused_as_already_cached_is_counted_apart_from_one_refused_for_space` (`src/lib.rs:6696`), and `lookup_that_loses_promotion_race_is_not_counted_as_store_already_resident` (`src/repair_test_store_already_resident.rs:474`).

*Re-anchored and spot-checked (verdicts unchanged from the 2026-10-20 sweep):*
- FR-001 → `components/interfaces/src/idispatcher.rs` trait; `create_eviction_channel` / `eviction_dropped_count` inherent `src/lib.rs:563-578`. FR-002 → `idispatcher.rs` `DispatcherError`.
- FR-003 → `populate` `src/lib.rs:3239-3320`: drop-on-full `:3257-3271`; release on GPU-unbound / copy / sync failure `:3281-3307`; register `:3311`.
- FR-004 → `write_through` `src/lib.rs:1463-1521` (`mt.peek` `:1471`, `publish` `:1519`); settle `:1445-1458`. FR-005 → the owned settle never frees the slot (`:1428-1431`).
- FR-006 → `serve_memory_tier_to_gpu` `src/lib.rs:1265-1339`. FR-007 / FR-008 → `lookup_async` `:3055-3160`, `check` `:3162-3181`.
- FR-009 (as worded) → `remove` `src/lib.rs:3183-3232`, extent `:3223-3229` via `drive_index` *(SC-003 consequence: D-17)*.
- FR-010, FR-011 (receptacles, NUMA auto-pin `compute_numa_cpu_assignments` `:605`, remote `NotFound`→`KeyNotFound`).
- FR-012 → `src/lib.rs:1948-1956`.
- FR-014 / SC-006 → `shutdown` `src/lib.rs:2169`: writer drained `:2180` (`src/background.rs:109-145`), checkpoint `:2183-2190`.
- FR-015 / FR-016.
- FR-019, FR-023 → `touch` `src/lib.rs:3764-3780`.
- FR-024 → `evict_one_clean` `src/lib.rs:1056`, `evict_for_space` `:1129`, fragmentation relief `:1203-1212`.
- FR-025 → `recover_extent` `src/lib.rs:1979`.
- FR-029 – FR-031; FR-032 + SSD-evictor edge cases → `dm.remove` first `src/background.rs:498-504`, `mt.remove` `:507`, `Removed` `:509-514`, extent `:516-524`.
- FR-033 – FR-035.
- FR-036 → `src/lib.rs:3124-3150` (pin held through `promote_and_serve` `:3129`; `AlreadyExists` → `serve_concurrently_promoted` `:3142`).
- FR-037, FR-039 (1)–(6) → retry `src/lib.rs:2434-2443`; `warm_pins` `:2408`; deferred warm sync `:2803-2808`, then `drop(warm_pins)` `:2812`; remote after; scatter-gather `:2722`; input order `:3008-3036`.
- FR-040 → `src/lib.rs:3782-3938`. FR-041. FR-042 (as amended; wiring gap P-02). FR-043, FR-045.
- FR-046 – FR-049 → MT evictor `try_evict_to_block` `src/background.rs:705`, then `mt.remove` `:709`. FR-050 → `Demoted` `:713-718`.
- FR-051 → `serve_concurrently_promoted` `src/lib.rs:1352-1391`; batch recovery `:3001`.
- FR-052 – FR-055, FR-057, FR-059 → `batch_populate` `src/lib.rs:3324-3429`. FR-060 → `:3498-3546`. FR-061.
- User Stories 1–5, 7–12 acceptance scenarios as listed in the 2026-10-20 sweep: US-1 AS3 `AlreadyExists` from `populate` → `:3272`; US-4 AS1 `src/repair_test_remove_no_wait.rs:465`.
- Edge cases: populate-full; evict exhaustion; write-ref downgrade (⧗); concurrent lookups; SSD-full write-through; unpersisted skip; SSD-evictor all-pinned; SSD-evictor order; partition guard.
- SC-001, SC-002, SC-004 – SC-007, SC-009 – SC-011, SC-013, SC-015, SC-016, SC-017.

#### Drifted ⚠️

**D-19 · FR-056 `flush_to_ssd` bullet (approved 2026-10-21 text, `spec.md:346`): the count leaves out pinned entries, and the signature is written `-> usize`.** Severity: minor.
- Spec: "`flush_to_ssd() -> usize` … returns the number of memory-tier entries that have an SSD copy, or `IoError` if a write-through to SSD failed for an entry that is still in the memory tier."
- Code (`src/lib.rs:4037-4041`): `mt.oldest_keys(usize::MAX).filter(|&key| dm.is_evictable(key)).count()`.
  - `is_evictable` is true only for `read_ref == 0 && write_ref == 0 && ssd_offset: Some` (`components/dispatch-map/src/lib.rs:530-546`).
  - So a resident, persisted entry that a reader, a client `pin`, or a warm `batch_lookup` H2D holds at that moment has an SSD copy but is **not counted**.
  - The method returns `Result<usize, DispatcherError>` (`src/lib.rs:4006`; trait `idispatcher.rs:838`).
- The interface doc ("Returns the number of entries that now have a valid SSD offset", `idispatcher.rs:830`) has the same imprecision. Its only consumer, shm-queue `FLUSH_TO_SSD` (`lib/shmq-dispatcher/src/translate.rs:903-910`), passes the number through for display (`apps/remote-lookup-bench/src/main.rs:1051-1070`).
- Why not ALIGN by default: no `IDispatchMap` method exposes `ssd_offset` without the pin check (`LookupResult::MemoryTier` carries no offset). That is the interface addition A-2 needs anyway.
- Direction: **BACKFILL** (B-7). The approved sentence is kept as it is; one clarifying sentence and the signature are added. *Alternative:* ALIGN together with A-2's `IDispatchMap::ssd_offset(key)`.

**D-20 · FR-017 (`spec.md:298`): "the background writer silently drops the job" is stale since `9bd231d85`.** Severity: minor.
- Code:
  - The writer records every settled write-through, per key: `record_write(key, written.is_some())`, from `run_write_job` (`src/lib.rs:1419-1423`, `:1429`; `src/background.rs:363-372`).
  - While such a key is still resident, `flush_to_ssd` returns `IoError("flush_to_ssd: {failed} memory-tier entries have no SSD copy …")` (`src/lib.rs:4026-4034`).
  - The store's caller is still not told (FR-003 returns `Ok` before the job runs), and the job is not retried.
- FR-017 does not say this. The edge cases at `spec.md:234` and `:237` and the Assumption at `:417` stay true as worded: they describe the entry, not reporting. FR-056 now says it from the other side, so FR-017 should point to it.
- U-08: the failed-write record (`src/background.rs:257-263`, `:363-380`, ~25 lines) appears in no spec. It is folded into **B-8**.
- Direction: **BACKFILL** (B-8).

**D-21 · `contracts/errors.md:36` (`IoError` row) does not list `flush_to_ssd`.** Severity: minor.
- Code: `flush_to_ssd` returns `DispatcherError::IoError` when a resident key's latest write-through failed (`src/lib.rs:4029-4034`). The interface doc lists it (`idispatcher.rs:832-837`, approved).
- The row's "Raised by" column is `lookup`, `batch_lookup`, `initialize`, `commit_store`.
- Direction: **BACKFILL** (B-9), parallel to the 2026-10-20 B-5 fix of the `Timeout` row.

**D-16 · FR-013 + Assumption: a `schedule_write_through` job takes no pin, yet settles as if it held one.** Severity: major. Unchanged, task **A-1**. Re-anchored:
- enqueue without a pin: `src/lib.rs:4057-4064`;
- `JobStart::Untracked`: `src/background.rs:303-305`, `:311-313`;
- settle with `convert_to_storage` / `release_read`: `src/lib.rs:1421` → `:1452`, `:1455`;
- caller: `components/remote-lookup/src/actor.rs:604-630`.
- *New on this branch:* the job's outcome is now also recorded (`:1422`). If the slot was freed and re-reserved under it, the job may write the new incarnation's bytes and record `landed = true` even though `convert_to_storage` failed (see P-06(b)).

**D-17 · SC-003 vs FR-009: `remove` of a persisted resident entry leaks its extent.** Severity: moderate. Unchanged, task **A-2**: `src/lib.rs:3192-3202`, `:3223-3229`; recovery `:1979`. `flush_to_ssd_counts_entries_with_an_ssd_offset` (`src/repair_test_flush_failed_write.rs:638`) now proves that both entries carry `ssd_offset` after a flush. A `remove` of either one would leak its extent.

**D-07 · FR-028 / US-9 AS3: the no-drives `promote_and_serve` never re-registers.** Unchanged, task **A-04**, `src/lib.rs:815-821`.

#### Partially implemented ◐

**P-06 · FR-056 `flush_to_ssd`: the failure record is not tied to the entry it describes.** Severity: minor. Confidence: MEDIUM for (a), which follows directly from the code; LOW for (b), which is reachable only through the A-1/A-4 windows. Same item as D-19, not counted again.
- (a) **Stale record across incarnations.**
  - `failed_writes` keeps a key while `mt.contains(key)` is true, and drops it only when the key is gone (`src/background.rs:376-379`). `remove` (`src/lib.rs:3183-3232`) and `clear_memory_tier` (`:3940-4004`) do not touch the record.
  - Scenario: key K's write-through fails. `remove(K)`, then a new store of K reserves a fresh slot (`reserve_memory` → `mt.insert`, `:1194`), while its completion is still pending. The shm-queue reserve / CopyToStore / complete steps are separate requests.
  - A `flush_to_ssd` in that interval returns `IoError` for a write of the *previous* entry. The resident slot has had no write-through yet, and the spec says "failed for an entry that is still in the memory tier".
  - It is cleared only once the new job lands (`record_write(K, true)`).
- (b) **"Landed" means "written", not "recorded".**
  - `process_write_job` and the owned branch record `written.is_some()` (`src/lib.rs:1401-1403`, `:1429`). The result of `convert_to_storage` is discarded (`:1452`), although `record_write`'s doc says "landed if its data reached the block device **and the SSD offset was recorded**" (`src/background.rs:363-364`).
  - An Untracked job (D-16) whose entry was removed meanwhile gets `KeyNotFound` from `convert_to_storage`, yet it counts as landed, and its published extent leaks.
  - If the key has been re-created, the old job's `convert_to_storage` stamps its offset onto the new entry, whose bytes it may never have written.
- Direction: **ALIGN** (A-5).

**P-04 · FR-018: two residual windows in the remove/write-through handshake.** Unchanged, task **A-4**: `src/background.rs:347-351`; `src/lib.rs:3206-3215`; re-store `:1214-1221` → `:3535-3543`. *Note:* window (b) now hits the store counter directly: the transient `AlreadyExists` is counted in `store_already_resident` by `reserve_memory` (`:3541`).

**P-05 · FR-056 vs FR-038: `clear_memory_tier` frees reserved-but-not-completed slots.** Unchanged, task **A-3** (recommendation (a)): `src/lib.rs:3974-3988`; entry created only at `:3635`.

**P-01 · FR-003 / FR-056: phase-3 release.** Unchanged, task **A-05**: the integrity-check failure paths call `mt.remove` and leave the dispatch-map entry (`src/lib.rs:3674-3691`); a `downgrade_reference` failure (`:3694-3695`).

**P-02 · FR-042: the SSD evictor holds a sender snapshot.** Unchanged, task **A-06**: `src/lib.rs:2111` (the MT evictor shares the live `Arc`, `:2153`).

**P-03 · FR-051 / FR-036: the `batch_lookup` cold path drops its pin before promotion.** Unchanged, task **A-02**: `src/lib.rs:2488-2489`.

#### Not Implemented ✗
None.

#### Not verifiable here ⧗
- SC-012 (FIFO NVMe correctness, needs hardware).
- SC-014 (≥5× batch-cold throughput, needs a benchmark).
- User Story 11 AS4 under real NVMe errors.
- The edge case "write-ref downgrade lets a concurrent lookup proceed" (`src/lib.rs:3694`).

That is 4 items here; with the 5 in spec 002, 9 in total.

### Spec: 002-served-by-tier-attribution — Serving-Tier Attribution (`served_by`)

This branch does not touch it: no hunk is on an attribution path, and `store_already_resident` / `flush_to_ssd` are not mentioned in it. Every verdict was re-anchored and spot-checked.

#### Aligned ✓
- FR-001 → `idispatcher.rs` `ServedBy`/`LookupOutcome`.
- FR-002 / SC-001 → `src/lib.rs:3008-3036` (`debug_assert` `:3026`).
- FR-003 → `lib/shmq-dispatcher/src/translate.rs`.
- FR-004 – FR-008, FR-010 – FR-013, FR-017, FR-019 – FR-026, FR-033, FR-034, FR-036, SC-004.
- FR-009 / FR-010 recovery → `:3001`.
- US1 AS1, AS2, AS4, AS5; US4 AS1–AS3; edge cases as listed in the 2026-10-20 sweep.

#### Drifted ⚠️
- **D-13 · US1 AS3 / FR-009:** a failed warm batched sync is only logged (`src/lib.rs:2803-2808`). Major. Task **A-01**, unchanged.
- **D-15 · FR-027 / SC-009:** a grep for `assert…ServedBy::{Ssd,Remote,SizeMismatch,Error}` in `src/` still finds nothing. Task **A-08**, unchanged.

#### Not verifiable here ⧗
SC-002, SC-003, SC-006, SC-007, SC-008.

#### Owned by other components (listed, not judged)
- dispatcher-p2p: FR-014, FR-015, FR-035.
- remote-lookup: FR-016 (replacement), FR-018.
- Documentation process: FR-030 – FR-032.
- SC-005; US3, US5.

### Unspecced Code 🆕

| # | Feature | Location | Lines | Suggested Spec |
|---|---------|----------|-------|----------------|
| U-08 | Per-key failed-write record (`record_write`, `failed_writes`), behind `flush_to_ssd`'s `IoError` | `src/background.rs:257-263`, `:363-380`; `src/lib.rs:1419-1423`, `:1429`, `:4026-4034` | ~35 | 001 FR-017 + FR-056 (folded into **B-8**; behaviour in (a) above is ALIGN A-5, not backfilled) |
| U-01 | `integrity-check` verify-before-serve | `src/lib.rs:1236-1257`, `:3668-3692` and the warm/remote/single-key verify sites | ~70 | Open item **N-01** in `align-tasks.md`. Not re-proposed. |

## Inter-Spec Conflicts

1. **001 SC-003 vs 001 FR-009:** the extent of a persisted resident entry (D-17). Still open, A-2.
2. **001 FR-056 vs 001 FR-038:** reserved slots (P-05). Still open, A-3.
3. **001 Assumption / interfaces FR-031a vs code:** the `schedule_write_through` job's pin (D-16). The spec now states the gap; the code is still A-1.
4. **New: 001 FR-017 ("silently drops") vs 001 FR-056 (`flush_to_ssd` reports the failure).** They do not contradict each other, but FR-017 reads as if nothing records the failure (D-20). B-8 resolves it.
5. **New, cross-component (not this component's specs):** the interfaces `IDispatcher::flush_to_ssd` contract (`components/interfaces/src/idispatcher.rs:824-838`, approved) vs the `dispatcher-p2p` implementation (`components/dispatcher-p2p/src/lib.rs:2889-2902`). The p2p version still returns the number of jobs in the queue before the flush, and it never returns `IoError`. The shm-queue reply is still called `jobs_flushed` (`lib/shmq-dispatcher/src/wire.rs:46-47`, `src/translate.rs:901`), which no longer describes the number. Run the `dispatcher-p2p` sync. `lib/shmq-dispatcher` has no `specs/`.
6. **Carried over:** the interfaces `clear_memory_tier` doc comment (`idispatcher.rs:806-810`, "Entries still being written are removed entirely") still predates FR-038 as amended. It is a code-side companion edit, outside `specs/**`.
- **Resolved since the last sweep:** `contracts/errors.md` `Timeout` row; FR-038 vs FR-024/FR-049 text; FR-058 vs the load-path counting (A-07).

## Recommendations

1. **BACKFILL, in this branch's PR**, so the next stamp certifies `9bd231d85`: B-7 (FR-056 `flush_to_ssd` count and signature), B-8 (FR-017), B-9 (`contracts/errors.md` `IoError`). Exact text is in `proposals.md`. Add a spec 001 `Last Synced 2026-10-21` header in the same apply. The working-tree FR-056 edit carries only an inline note.
2. **Close A-07** in `align-tasks.md`. Code `efd502331` took the ALIGN option, and its acceptance is covered by `src/repair_test_store_already_resident.rs:474` and `src/lib.rs:6696`.
3. **ALIGN, minor:** A-5. Tie each failure record to the slot it was written from, and record "landed" only when `convert_to_storage` succeeds. Do this together with A-1, because (b) is reachable only through the A-1/A-4 windows.
4. Still open, priority unchanged: A-1, A-01, A-02 (major); A-2, A-3, A-04, A-05, A-06, A-08 (moderate); A-4 (minor, LOW confidence); N-01 (decision).
5. Cross-component: align `dispatcher-p2p::flush_to_ssd` with the edited interface contract, and rename or redocument the shm-queue `jobs_flushed` reply field.
6. Re-run `scripts/spec-sync-hash.sh components/dispatcher` only after the spec edits are committed. The frontmatter stamp (`spec_sync_git_commit: cccfb328e`) predates `9bd231d85` and `efd502331`, and the approved working-tree edits are not in it.

---

# Earlier sweeps (retained for history)

## Sweep 2026-10-20 — branch `fix/dispatcher-remove-clear-mt` (HEAD `cccfb328e`; line numbers refer to that commit; its BACKFILL items B-1…B-6 were applied in `ec4277c0e`, and its ALIGN items are A-1…A-4 in `align-tasks.md`)

Generated: 2026-10-14 (sync date; analysis run 2026-10-20)
Project: `components/dispatcher` (crate `dispatcher`)
Specs: `specs/001-dispatcher-cache-interface/spec.md` (417 lines), `specs/002-served-by-tier-attribution/spec.md` (779 lines)
Branch: `fix/dispatcher-remove-clear-mt`. HEAD `cccfb328e`, base `origin/unstable` `af221dd82`.
Mode: read-only drift analysis, batch (no questions). Nothing outside `.specify/sync/` was edited.

## Scope and method

**What changed on the branch** (`git diff origin/unstable.. -- components/dispatcher/src`): 4 files, +1448 / −44. `specs/**` and `components/interfaces/**` are unchanged.

| Commit | Finding id | Code change |
|---|---|---|
| `0236a3c98` | DIS-REMOVE-NO-WAIT (FR-018; Kani + Loom) | New `WriteThroughPins` ledger (`src/background.rs:244-366`). `copy_gpu_to_memory_completed` records the write-through pin before enqueueing (`src/lib.rs:3684-3695`). The writer runs `run_write_job` (`src/lib.rs:1409-1438`, wiring `:2077-2086`). `remove` takes over the pin on `ActiveReferences` (`src/lib.rs:3203-3218`). `process_write_job` is split into `write_through` + `settle_write_through` (`src/lib.rs:1396-1518`). Tests: `src/repair_test_remove_no_wait.rs:465,485`. |
| `cccfb328e` | DIS-SPIN-CLEAR-MT-KEEPS-PINNED (Spin model node7 J22; Loom) | `clear_memory_tier` keeps a pinned entry's slot, retries, widens its window, counts only the entries it really cleared, and returns `Timeout` after a 5 s stall (`src/lib.rs:3923-3987`). Tests: `src/repair_test_clear_mt_pinned.rs:599,621,644`. |

**Baseline argument.** `origin/unstable`'s `src/**` equals `4f8b5554b`, the HEAD the 2026-10-14 full sweep analysed. `git diff 4f8b5554b origin/unstable -- components/dispatcher/src` is empty. Between them only `specs/**` and `.specify/sync/**` changed (the approved apply, `a9306c3f6`). The previous full sweep is therefore a valid baseline once the apply is taken into account:
- Its BACKFILL drift items D-01…D-05, D-08, D-09, D-10, D-12, D-14 are now aligned, because the spec text was replaced.
- Every untouched function was re-anchored through the hunk offsets. Offsets in `lib.rs`: +1 after line 433, +48/+49 between 1473 and 3151, +58 to 3624, +62 to 3871, +95 after. Each anchor cited below was spot-checked at its new line.

**What was re-read in full** (not inherited): every requirement whose implementation lies in, or calls into, the changed hunks. That covers `remove`; populate → `copy_gpu_to_memory_completed` → write-through; `schedule_write_through`; the background writer; `clear_memory_tier`; and the paths that race with them (lookup pins, SSD/MT evictors, reserve). The real `IDispatchMap` semantics were checked in `components/dispatch-map/src/lib.rs`:
- `remove` refuses `read_ref>0 || write_ref>0` with `ActiveReferences` (`:341-343`).
- `convert_to_storage` decrements *any* read reference (`:194-196`).
- `take_read` waits for `write_ref==0` (`:213-219`).
- `try_evict_to_block` returns `ActiveReferences` before the state check (`:558-560`).
- `recover_extent` (`:582-607`).

Further evidence: `memory-tier` `insert` tracks the key in the eviction policy at reserve time (`components/memory-tier/src/lib.rs:345-367`), `oldest_keys` (`:435-443`), and the remote-lookup publish path (`components/remote-lookup/src/actor.rs:600-629`).

**Why this sweep is adversarial.** Both repairs come from formal models that each covered one race. This sweep read the new ledger as a protocol and asked, for every actor that touches a write-through key, who holds the pin and who frees the slot. That found four gaps the models did not cover (findings D-16, D-17, P-04, P-05 below). They are reasoning-level findings with file:line evidence. None was reproduced by a test here, and none is caused by the two commits except P-04, which is inside the new ledger.

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 118. That is 109 in this component's scope: the active FRs and SCs of both specs, the acceptance scenarios and edge cases named below, and the Assumption on the background writer's read reference (newly counted, because this branch changes its truth). The other 9 items of spec 002 belong to other components and are only listed. |
| ✓ Aligned | 86 (78.9% of 109) |
| ⚠️ Drifted | 9 (8.3%). 3 are spec text this branch made stale (BACKFILL: D-06, D-10′, D-18). 2 are new ALIGN findings (D-16, D-17). 4 are carried over (D-07/A-04, D-11/A-07, D-13/A-01, D-15/A-08). |
| ◐ Partially implemented | 5 (4.6%). 3 carried over (P-01/A-05, P-02/A-06, P-03/A-02), 2 new (P-04, P-05) |
| ✗ Not implemented | 0 |
| ⧗ Not verifiable here (hardware / cluster / benchmark) | 9 (8.3%) |
| 🆕 Unspecced Code | 4. 3 are folded into BACKFILL proposals; 1 (integrity-check) is the open item N-01 |

*Correction to the earlier full sweep:* it listed **FR-018** as aligned (`src/lib.rs:3131-3171` at `4f8b5554b`: "no flush"). That was wrong. On that code, `dm.remove` refused the pin that `downgrade_reference` leaves for the write-through, so `remove` returned `KeyNotFound` until the job ran. With no running writer, the entry could never be removed. The base test even said so (`src/lib.rs:6212-6219` at base: "Until then the entry is legitimately pinned and `remove` refuses it"). The same blind spot hid the old US-4 AS1 behaviour. US-4 AS1 is aligned now. FR-018's main case is met, but it is counted as partially implemented (P-04) because of two residual windows.

What this branch means for the spec (one line each):
1. FR-018: now met in code. The ledger's outcomes are not specified (**BACKFILL** B-1; unspecced U-06).
2. D-10′ · edge case "remove": "refused because the entry is pinned" is now false for the write-through pin (**BACKFILL** B-2). `contracts/idispatcher.md:45` ("Acquires a write reference") was never true (**BACKFILL** B-6).
3. D-06 · FR-038: the code fix resolves the old major ALIGN task **A-03**. The FR text is still the pre-fix mechanism (**BACKFILL** B-3). `contracts/errors.md:37` lists `Timeout` for `remove`, which never returns it, and omits `clear_memory_tier`/`pin`, which do (**BACKFILL** B-5).
4. D-18 · Assumption "background writer holds a read reference": now conditional (**BACKFILL** B-4).
5. New: D-16 (FR-013, `schedule_write_through` steals a reader's pin; **major**), D-17 (SC-003, extent of a persisted resident entry leaks on `remove`), P-04 (FR-018, ledger give-back window and transient `AlreadyExists` on re-store), P-05 (FR-056 vs FR-038, `clear_memory_tier` frees reserved-not-completed slots). All are **ALIGN**.

## Detailed Findings

### Spec: 001-dispatcher-cache-interface — Dispatcher Cache Interface (Memory-Tier Architecture)

#### Aligned ✓

- FR-001 method inventory → `components/interfaces/src/idispatcher.rs:499-877` (all listed methods present). `create_eviction_channel` / `eviction_dropped_count` are inherent (`src/lib.rs:563-578`).
- FR-002 → `idispatcher.rs:167-181`.
- FR-003 (incl. the 2026-10-14 post-reserve release) → `populate` `src/lib.rs:3236-3317`: drop-on-full `:3252-3268`, release on GPU-unbound / copy / sync failure `:3278-3304`, register `:3308`.
- FR-004 → `write_through` `src/lib.rs:1460-1518` (`mt.peek` `:1468`, `publish` `:1516`), then `settle_write_through` → `convert_to_storage` `:1442-1455`.
- FR-005 → the owned settle path never frees the slot (`src/lib.rs:1426-1428`, `:1447-1450`). The only `mt.remove` in the writer is the "removed meanwhile" branch (`:1432`). That is FR-018, not FR-005.
- FR-006 → `serve_memory_tier_to_gpu` `src/lib.rs:1268-1342`, call `:3106-3113`.
- FR-007 → `src/lib.rs:3087`, `:2443-2444`.
- FR-008 → `check` `src/lib.rs:3159-3178`.
- FR-009 (as worded: frees slot, extent *if BlockDevice*, map entry) → `remove` `src/lib.rs:3180-3229`, extent `:3220-3226` via `drive_index`. *(This is literally met; the SC-003 consequence is D-17.)*
- FR-010 → `src/lib.rs:408-411`.
- FR-011 → receptacles `src/lib.rs:412-419`; NUMA auto-pin `.skip(2)` `:642`; `poller_base_cpu + i` `:1665-1670`; remote `NotFound`→`KeyNotFound` / `TransportError`→`IoError` `:2873-2891`.
- FR-012 → `src/lib.rs:1945-1951`; empty `data_pci_addrs` rejected first `:1953`.
- FR-014 / SC-006 → `shutdown` `src/lib.rs:2166-2190`: evictors, then `writer.shutdown()` (drains the queue, `src/background.rs:109-145`), then `checkpoint` `:2185`.
- FR-015 / FR-016 → `src/lib.rs:1865-1887`.
- FR-017 → `write_through` returns `None` on every failure (`src/lib.rs:1492`, `:1499`, `:1509`). `settle_write_through(None)` releases the pin (`:1451-1453`), and the entry stays MemoryTier.
- *(FR-018 is counted under ◐ P-04, not here. Its main case is now met — `remove` `src/lib.rs:3203-3218` with `WriteThroughPins::remove_entry` `src/background.rs:335-356`, tests `src/repair_test_remove_no_wait.rs:465,485` — but two residual windows remain, and the outcomes are unspecified (B-1).)*
- FR-019 → `promote_and_serve` multi-object pipeline `src/lib.rs:877`; staging `pipelined_ssd_to_gpu_zero_copy` `:1000`; `scatter_gather_multi_drive_zero_copy` `:2719`.
- FR-023 → `touch` `src/lib.rs:3747-3763`.
- FR-024 → `evict_one_clean` `src/lib.rs:1056-1114` (variant match `:1072-1090`); `evict_for_space` `:1129-1180`; fragmentation relief `:1203-1212`.
- FR-025 → `src/lib.rs:1967-1985` (`for_each_extent` + `recover_extent` `:1975-1976`).
- FR-029 / FR-030 / FR-031 → `src/lib.rs:2092-2125`; SSD evictor `src/background.rs:385-531`.
- FR-032 + SSD-evictor edge case → `dm.remove` before `mt.remove` `src/background.rs:474-483`, event `:485-490`, extent `:495-499`.
- FR-033 → `idispatcher.rs:105-127`.
- FR-034 / FR-035 → `ChannelPool` `src/lib.rs:299-357`, `register_host_memory` `:2036`, `unregister_host_memory` `:2196`.
- FR-036 → `lookup_async` `src/lib.rs:3052-3157`; the BlockDevice arm keeps its pin through promotion `:3121-3149`; `AlreadyExists` → `serve_concurrently_promoted` `:3138-3145`.
- FR-037 → `DEVICE_STREAMS` `src/lib.rs:468-507`; `shutdown` destroys only `warm_stream` + ring `:2200-2210`.
- FR-039 (1)–(6) → retry ×5 `src/lib.rs:2431-2440`; warm queued + pin adopted `:2452-2481`; `batch_touch` `:2509-2511`; deferred warm sync then pin drop `:2796-2808`; remote after `:2811`; scatter-gather `:2719`; input order `:3008-3036`.
- FR-040 → `promote_to_memory_tier` `src/lib.rs:3765-3921`.
- FR-041 → `src/pipeline.rs:1166`, `:1279`; used at `src/lib.rs:3888`.
- FR-042 (as amended) → `EvictionSender` `src/background.rs:226-241`; foreground `src/lib.rs:563-584`; MT evictor `src/background.rs:689-694`; SSD evictor `:485-490`. *(The wiring gap is P-02.)*
- FR-043 → `src/metrics.rs:12-47`, `src/lib.rs:557`.
- FR-045 → `src/lib.rs:2811-2990`.
- FR-046 – FR-049 → start `src/lib.rs:2128-2154`; evictor `src/background.rs:582-720`: `try_evict_to_block` `:681` before `mt.remove` `:685`.
- FR-050 (as amended) → `Demoted` only `src/background.rs:689-694`.
- FR-051 → `serve_concurrently_promoted` `src/lib.rs:1355-1394`; batch recovery `:2998-3003`; single-key `:3138-3145`.
- FR-052 → `src/lib.rs:468-507`; batch device `:2370-2380`.
- FR-053 → `src/pipeline.rs:96-151`; `serve_cold_staged` `src/lib.rs:925-1036`; staging post-pass `:2775-2792`.
- FR-054 → `src/pipeline.rs:310-406`, `:569-682`, `:882-1101`.
- FR-055 → `src/lib.rs:1826-1836`.
- FR-057 → `src/lib.rs:545-558`.
- FR-059 → `batch_populate` `src/lib.rs:3321-3427` (per-entry release `:3375`, `:3407`).
- FR-060 → backpressure `src/lib.rs:3470-3531`; drop-on-full `:3261`, `:3384`.
- FR-061 → `src/pipeline.rs:771`; proportional split `:853`; final dual sync `:1124-1125`.
- User Story 1 AS1–AS5; User Story 2 AS1–AS4; User Story 3 AS1–AS2; **User Story 4 AS1–AS3** (AS1 now also holds while the write-through is pending: `remove_proceeds_while_write_through_pending`, `src/repair_test_remove_no_wait.rs:465`); User Story 5 AS1–AS4; User Story 7 AS1–AS5; User Story 8; User Story 9 AS1–AS2; User Story 10 AS1–AS5; User Story 11 AS1–AS6 (AS6 `mixed_batch_defers_warm_sync_past_cold_block` `src/lib.rs:6252`); User Story 12 AS1–AS5.
- Edge cases: populate-full drop; `evict_for_space` exhaustion; concurrent lookups; SSD-full write-through; unpersisted skip; SSD-evictor all-pinned; SSD-evictor removal order; partition guard.
- SC-001, SC-002, SC-004 – SC-007, SC-009 – SC-011, SC-013, SC-015, SC-016, SC-017 (`the_route_counters_partition_the_served_keys` `src/lib.rs:6070`).

#### Drifted ⚠️

**D-06 · FR-038 `clear_memory_tier` — the code is fixed, but the spec still describes the old mechanism (branch `cccfb328e`)** — severity: moderate (spec stale; the major code defect is resolved)
- Spec (`spec.md:317`): "evicts ALL entries … by calling `IMemoryTier::evict_next()` in a loop until empty. For each evicted key, it transitions the dispatch-map entry to BlockDevice state via `convert_memory_tier_to_block`. If the transition fails (write-through not complete), the entry is removed from the dispatch map entirely. Returns the number of entries cleared."
- Code (`src/lib.rs:3923-3987`):
  - It loops on `mt.oldest_keys(64 + pinned)` (`:3950`): the window widens by the number of entries left pinned on the last pass.
  - For each key it calls `try_evict_to_block`. `ActiveReferences` → **keep the slot** (`:3959`). Any other error → `dm.remove` first, and the slot is kept if that returns `ActiveReferences` (`:3963-3966`).
  - `mt.remove` and `count += 1` run only for keys actually cleared (`:3968-3971`).
  - A pass that clears nothing sleeps 50 µs (`:3983`). After 5 s without progress it returns `DispatcherError::Timeout("… pinned entries still resident …")` (`:3943`, `:3976-3982`). So `Ok(n)` means the tier was emptied.
- None of this is in FR-038: not the pin rule, not `Timeout`, not the count rule. The interface doc comment (`components/interfaces/src/idispatcher.rs:802-806`) still says "Entries still being written are removed entirely". `contracts/errors.md:37` does not list `clear_memory_tier` for `Timeout`.
- Tests: `clear_memory_tier_keeps_slot_of_lookup_pinned_entry`, `…_of_writer_pinned_entry`, `…_removes_unpersisted_entry_only_after_pin_drops` (`src/repair_test_clear_mt_pinned.rs:599,621,644`). The mock records any slot freed while a `MemoryTier` entry still points at it.
- Resolves: earlier D-06 / align task **A-03**. Its acceptance criteria are met: the pinned key stays resident, and the call terminates (via `Timeout`) when everything is pinned.
- Direction: BACKFILL (B-3, B-5). The interface doc comment is a code-side companion (noted in B-3).

**D-10′ · Edge case "When remove is called for a key …" (`spec.md:233`) — the write-through pin no longer refuses `remove`** — severity: minor
- Spec: "it is refused while the entry is pinned or write-locked … A remove that is refused because the entry is pinned returns `KeyNotFound` and frees nothing."
- Code: on `ActiveReferences`, `remove` asks the ledger (`src/lib.rs:3207-3210`). If the only reference is the pending write-through's pin, the pin is released and the entry removed (`src/background.rs:335-356`). When the job is already copying the slot, `remove` leaves the slot to the job (`src/lib.rs:3214-3218`; job side `:1430-1436`). A reader's or client's pin still refuses (`src/background.rs:343-346` → `None` → `KeyNotFound`). A dispatch-map timeout on the initial `dm.lookup` is also `KeyNotFound` (`src/lib.rs:3198`).
- Direction: BACKFILL (B-2). Also resolves open item **N-04** (`remove` on a pinned entry). The approved option is "BACKFILL the `KeyNotFound` result".

**D-18 · Assumption "The background writer holds a read reference on each entry while write-through is in progress. The reference is released after write completes or fails." (`spec.md:416`)** — severity: minor (newly counted)
- Code: true only for jobs queued by `copy_gpu_to_memory_completed`: the `downgrade_reference` pin (`src/lib.rs:3677`), recorded in the ledger at `:3686`. Since `0236a3c98`, `remove` may take that reference over first. The job is then dropped (`JobStart::Stale`, `src/background.rs:301-305`), or it finishes and frees the slot and extent (`src/lib.rs:1430-1436`). Jobs from `schedule_write_through` (`src/lib.rs:4004-4026`) hold **no** reference at all (see D-16).
- Direction: BACKFILL (B-4). The stated behaviour for the unpinned case is a defect, tracked as D-16.

**D-16 · FR-013 + Assumption — a `schedule_write_through` job takes no pin, yet settles as if it held one** — severity: **major** (pre-existing since `schedule_write_through` was added; this branch codifies it as `JobStart::Untracked`). Confidence: plausible (reasoning; not reproduced).
- Spec FR-013 (`spec.md:292`): "MUST use appropriate read/write locking on the dispatch map to ensure thread safety during concurrent operations."
- Code:
  - The remote-lookup publish path calls `create_memory_tier_entry`, then `release_write`, then `schedule_write_through`. That leaves the entry with `read_ref == 0` (`components/remote-lookup/src/actor.rs:600-629`).
  - `schedule_write_through` enqueues the job without a pin and without a ledger record (`src/lib.rs:4017-4024`). `WriteThroughPins::start` returns `Untracked` (`src/background.rs:297-300`, `:306-307`), and `process_write_job` runs.
  - `process_write_job` settles with `convert_to_storage` (`src/lib.rs:1449`), which decrements **any** read reference (`components/dispatch-map/src/lib.rs:194-196`). On failure it settles with `release_read` (`src/lib.rs:1452`).
- Consequences:
  1. **Pin theft.** If a reader holds the key when the job settles (a warm `batch_lookup` hit whose H2D is still queued, `src/lib.rs:2477`; a cold `lookup_async` promotion, `:3121-3148`; a client `pin`, `:3715-3730`), the job consumes that reader's pin. The entry then looks unpinned while the reader is still copying. The MT evictor's `try_evict_to_block` + `mt.remove` (`src/background.rs:681-685`), `evict_one_clean` (`src/lib.rs:1072-1074`) or `remove` can free the slot under the in-flight DMA. When the reader releases, its `release_read` underflows (`RefCountUnderflow`, dispatch-map `:274-276`) or steals a third party's pin. This is exactly the use-after-free class FR-024/FR-049 forbid.
  2. **Unpinned slot read.** The job reads the slot (`mt.peek` `src/lib.rs:1468`, then the NVMe write) holding no reference. A concurrent `remove` (`dm.remove` succeeds at `read_ref==0`, then `mt.remove` `:3217`) or `clear_memory_tier` frees it under the write. The extent then gets the bytes of whatever reuses the slot. `convert_to_storage` returns `KeyNotFound`, and the extent just published is never freed.
- Direction: ALIGN (A-1).

**D-17 · SC-003 / User Story 4 — `remove` of a resident entry that is already persisted never frees its SSD extent** — severity: moderate (pre-existing; not caused by this branch, but now the common case: `remove` after the write-through has landed)
- Spec SC-003 (`spec.md:390`): "Cache removal frees all associated resources (memory-tier slots and SSD extents) so that they can be reused." FR-009 (`spec.md:288`) frees the extent only "if in BlockDevice state", so the FR text and the SC disagree.
- Code: `remove` learns the extent offset only from `LookupResult::BlockDevice` (`src/lib.rs:3189-3199`). `LookupResult::MemoryTier` carries no `ssd_offset` (`components/interfaces/src/idispatch_map.rs:22-27`). For a MemoryTier entry whose write-through has landed (`ssd_offset: Some`), the dispatch-map entry and the slot are freed, but `remove_extent` is never called (`:3220-3226`). Nothing else will free it either: the SSD evictor walks dispatch-map keys (`src/background.rs:458-499`), and the key is gone. The extent leaks until the next format. With `format_on_init=false`, the next `initialize` re-creates the removed key from that extent as a live BlockDevice entry (`src/lib.rs:1975-1976`; dispatch-map `recover_extent` `:582-607`). The removed key **comes back**.
- The ledger handles only the extent a still-running job writes (`src/lib.rs:1433-1436`). The existing test `remove` after `flush_to_ssd` (`src/lib.rs:~6328-6336`) checks `entry_count` only.
- Direction: ALIGN (A-2). It needs the offset of a MemoryTier entry, which today means an `IDispatchMap` addition.

**D-07 · FR-028 / User Story 9 AS3 — no-drives `promote_and_serve` never re-registers the entry** — unchanged from the earlier sweep (task **A-04**): `create_memory_tier_entry` on an existing entry → `AlreadyExists` ignored; stray `release_write` (`src/lib.rs:815-821`). Severity moderate.

**D-11 · FR-058 `store_already_resident` — counted on load paths too** — unchanged (open decision **A-07**). The site is `src/lib.rs:1222`. *New on this branch:* when `remove` leaves the slot to a running write-through (`src/lib.rs:3214-3218`), an immediate re-store of the same key gets `MemoryTierError::AlreadyExists` from `mt.insert`, and this counter counts it as well (see P-04).

#### Partially implemented ◐

**P-04 · FR-018 — the remove/write-through handshake has two residual windows** — severity: minor. Confidence: LOW (reasoning only; the Loom proof covers `remove` against one job, not these interleavings).
- (a) **Give-back window.** `remove_entry` releases the job's pin (`src/background.rs:342`), tries `dm.remove`, and, if a reader still holds the key, re-takes the pin with `take_read` (`:343-346`). Between the release and the re-take, the job's pin is not held. Suppose the reader releases in that window, and a second `remove` (its `dm.remove` at `src/lib.rs:3203` is outside the ledger lock) or a `clear_memory_tier` (`:3957-3969`) runs. It sees `read_ref == 0`, removes the entry and frees the slot. The job, still `running` with `removed == false`, then reads freed memory and settles with `convert_to_storage` → `KeyNotFound`, which leaks the extent it published (`src/lib.rs:1424-1428`). Also, `take_read` blocks on `write_ref == 0` for the dispatch-map timeout while holding the ledger mutex, so every writer's `start`/`finish` stalls behind it.
- (b) **Re-store during the slot hand-off.** After `remove` returns `Ok` with `job_frees_slot == true`, the slot stays in the memory tier until the job's `finish` (`src/lib.rs:1432`). A `populate`/`reserve_memory` of the same key in that interval fails with `AlreadyExists` (`src/lib.rs:1214-1223` → `:3269`). US-1 AS3 then reports "already exists" for a key `check()` says is absent. The repair test re-stores only in the *queued* case (`src/repair_test_remove_no_wait.rs:485-496`), not while the job runs.
- Direction: ALIGN (A-4).

**P-05 · FR-056 (reserve → copy → complete) vs FR-038 — `clear_memory_tier` frees reserved-but-not-completed slots** — severity: moderate (pre-existing; the repair did not cover keys without a dispatch-map entry).
- Spec FR-056 (`spec.md:338-341`) gives the reserved slot to the caller until `copy_gpu_to_memory_completed` or `release_memory`.
- Code: `reserve_memory` inserts the slot and the memory tier tracks the key at once (`components/memory-tier/src/lib.rs:345-367`), so `oldest_keys` returns it. Its dispatch-map entry is created only at completion (`src/lib.rs:3618`). In `clear_memory_tier`, `try_evict_to_block` → `KeyNotFound`, then `dm.remove` → `KeyNotFound`. That is not `ActiveReferences`, so the key counts as `done` and `mt.remove` frees the slot (`src/lib.rs:3957-3969`), while the caller's D2H into it may still be in flight (the shm-queue reserve / CopyToStore / complete flow spans separate requests). The later `copy_gpu_to_memory_completed` fails `KeyNotFound` (`:3616`). Meanwhile a reallocated slot may already be receiving that GPU copy. The count also includes these keys.
- Direction: ALIGN (A-3), decision needed (see the proposal).

**P-01 · FR-003 / FR-056 phase-3 slot release** — unchanged (**A-05**), `src/lib.rs:3651-3678`.
**P-02 · FR-042 SSD-evictor sender snapshot** — unchanged (**A-06**), `src/lib.rs:2108`.
**P-03 · `batch_lookup` cold path drops its pin before promotion** — unchanged (**A-02**), `src/lib.rs:2482-2483`. *Note:* the `clear_memory_tier` fix does not cover this case. An unpinned cold key whose fresh slot is mid-NVMe-read is a BlockDevice entry with `read_ref == 0`. `try_evict_to_block` → `InvalidState`, then `dm.remove` → `Ok`, then `mt.remove` frees the slot under the read (`src/lib.rs:3963-3969`). A-02 fixes it at the source.

#### Not Implemented ✗
None.

#### Not verifiable here ⧗
SC-012 (FIFO NVMe correctness, hardware), SC-014 (≥5× batch-cold throughput, benchmark), User Story 11 AS4 under real NVMe errors, and the "write-ref downgrade lets a concurrent lookup proceed" edge case (dispatch-map semantics, `src/lib.rs:3677`). That is 4 items here; with the 5 in spec 002, 9 in total.

### Spec: 002-served-by-tier-attribution — Serving-Tier Attribution (`served_by`)

Not touched by this branch. Every verdict was re-anchored and spot-checked.

#### Aligned ✓
- FR-001 → `idispatcher.rs:248-274`. FR-002 / SC-001 → `src/lib.rs:3008-3036` (`debug_assert` `:3023`). FR-003 → `lib/shmq-dispatcher/src/translate.rs:727-731`. FR-004 / FR-011 → `src/lib.rs:2489`. FR-005 → `:3026`. FR-006 → `:3030`. FR-007 → `:3031`. FR-008 (as amended) → sub-paths `:2513-2792`. FR-009 / FR-010 → `:2998-3003`. FR-012 → `:2929`. FR-013 → `:2366`. FR-017 → `:2873-2929`. FR-019 – FR-022 → `translate.rs:700-710`. FR-023 / FR-023a → `translate.rs:764`, `wire.rs:107`. FR-024 / FR-025 / SC-004 → `translate.rs:712-734`, `src/lib.rs:3046-3048`. FR-026 → `:4040`. FR-033 → `:1072-1089`. FR-034 → `:1112`. FR-036 → `:4052-4058`.
- US1 AS1, AS2, AS4, AS5; US4 AS1–AS3; edge cases "local miss then remote hit", "concurrent promotion", "cold-load staging", "empty batch", "mixed batch, one fails".

#### Drifted ⚠️
- **D-13 · US1 AS3 / FR-009**: a failed warm batched sync is only logged, and the keys stay `Dram` hits (`src/lib.rs:2799-2804`). Major, task **A-01**, unchanged.
- **D-15 · FR-027 / SC-009**: value-specific tests exist only for `Dram` and `Miss` (`src/lib.rs:6043`; `src/repair_test_served_by.rs`). Task **A-08**, unchanged.

#### Not verifiable here ⧗
SC-002, SC-003, SC-006, SC-007, SC-008.

#### Owned by other components (listed, not judged)
FR-014, FR-015, FR-035 (dispatcher-p2p); FR-016-replacement, FR-018 (remote-lookup); FR-030 – FR-032 (documentation process); SC-005; US3, US5.

### Unspecced Code 🆕

| # | Feature | Location | Lines | Suggested Spec |
|---|---------|----------|-------|----------------|
| U-06 | `WriteThroughPins` ledger (`pin` / `start` / `finish` / `remove_entry`; `JobStart::{Owned, Stale, Untracked}`) and the job's "removed meanwhile" clean-up (frees the slot and the extent it wrote) | `src/background.rs:244-366`, `src/lib.rs:1409-1438` | ~150 | 001 FR-018 (folded into **B-1**) |
| U-07 | `clear_memory_tier` `Timeout` after 5 s without progress, widening window, exact count | `src/lib.rs:3936-3986` | ~50 | 001 FR-038 + `contracts/errors.md` (folded into **B-3**, **B-5**) |
| U-04 | `remove` result for reader-pinned / write-locked / timed-out entries (`KeyNotFound`) | `src/lib.rs:3198`, `:3207-3211` | 14 | 001 edge case + contracts (folded into **B-2**, **B-5**, **B-6**). Resolves N-04. |
| U-01 | `integrity-check` verify-before-serve | `src/lib.rs:1239-1260`, `2454-2466`, `3095-3104`, `3645-3675` | ~70 | Open item **N-01** in `align-tasks.md`. Not re-proposed. |

## Inter-Spec Conflicts

1. **001 SC-003 vs 001 FR-009.** SC-003 says removal frees "SSD extents"; FR-009 frees an extent only "if in BlockDevice state". The code follows FR-009, so the extent of a persisted resident entry leaks (D-17).
2. **001 FR-038 vs 001 FR-024 / FR-049**, the earlier conflict: **resolved in code**. The FR-038 text still allows removal regardless of pins until B-3 lands.
3. **001 FR-018 / Assumption vs interfaces FR-031a.** FR-031a (`schedule_write_through`) says nothing about the job's pin. The 001 Assumption says every write-through holds one. The code gives the scheduled job none, and still settles as if it did (D-16).
4. **001 FR-056 vs 001 FR-038.** FR-056 gives a reserved slot to its caller until completion or release. FR-038, as implemented, frees it (P-05).
5. **`contracts/errors.md:37` vs code.** It lists `Timeout` for `remove`, which never returns it, and leaves out `clear_memory_tier` and `pin`, which do (`src/lib.rs:3978`, `:3725-3727`).

## Recommendations

1. **ALIGN, major:** D-16 / A-1. Give a `schedule_write_through` job its own ledger-recorded pin, as `copy_gpu_to_memory_completed` does. Fix it before the remote-lookup publish path sees concurrent readers in production. A-01 and A-02 (carried over) remain the other majors.
2. **BACKFILL, in this branch's PR**, so the next stamp certifies the two repairs: B-1 (FR-018), B-2 (remove edge case), B-3 (FR-038), B-4 (writer Assumption), B-5 (`contracts/errors.md`), B-6 (`contracts/idispatcher.md`). Exact text is in `proposals.md`.
3. **Close A-03 and N-04** in `align-tasks.md` once B-2/B-3 are applied. The code meets A-03's acceptance criteria (`src/repair_test_clear_mt_pinned.rs`).
4. **ALIGN, moderate:** D-17 / A-2 (extent leak and resurrection on `remove`); P-05 / A-3 (clear vs reserved slots; decide the contract first).
5. **ALIGN, minor, LOW confidence:** P-04 / A-4. Write a Loom or Spin model of two `remove`s plus a reader against one running job before changing code.
6. Update the `IDispatcher::clear_memory_tier` doc comment (`components/interfaces/src/idispatcher.rs:802-806`) together with B-3. It is outside `specs/**`, so it is listed as a companion edit, not a BACKFILL.
7. Re-run `scripts/spec-sync-hash.sh components/dispatcher` only after the spec edits land. The frontmatter's `clean` stamp (`spec_sync_git_commit: 4f8b5554b`) predates this branch's `src/**` changes.

---

## Full sweep 2026-10-14 — branch `fix/bundle-dispatcher-ems` (HEAD `4f8b5554b`; line numbers refer to that commit; its BACKFILL items were applied in `a9306c3f6`, and its ALIGN items are in `align-tasks.md`)

Generated: 2026-10-14 (analysis run 2026-10-18)
Project: `components/dispatcher` (crate `dispatcher`)
Specs: `specs/001-dispatcher-cache-interface/spec.md` (415 lines), `specs/002-served-by-tier-attribution/spec.md` (777 lines)
Branch: `fix/bundle-dispatcher-ems` — HEAD `4f8b5554b`, base `origin/unstable` `acd240255`
Mode: read-only drift analysis, batch (no questions). Nothing outside `.specify/sync/` was edited.

## Scope and method

**What changed on the branch** (`git diff origin/unstable.. -- components/dispatcher/src`): `src/lib.rs` (165 lines changed), `src/background.rs` (51 lines changed), six new `#[cfg(test)]` modules `src/repair_test_*.rs`. `specs/**` and `components/interfaces/**` are **unchanged** (`git diff --stat` empty). Commits (dispatcher only; `4a8fe3e75` touches `extended-metadata-store` and is out of scope):

| Commit | Finding id (from the repair test header) | Code change |
|---|---|---|
| `0d3fcde43` | DIS-INV-EVICTION-EVENTS | `EvictionSender { tx, dropped: Arc<AtomicU64> }` with `emit()`; background evictors now count drops (`src/background.rs:225-241`, `:361`, `:565`; `src/lib.rs:433-434`, `:562-584`) |
| `a7ccc6e48` | DIS-BATCH-LOOKUP-SERVEDBY-CONCURRENT-DRAM / -OVERWRITE | recovery pass rewrites `tier[i] = Dram` (`src/lib.rs:2946-2954`) |
| `1d24ff27f` | DIS-SPIN-COLD-LOOKUP-KEEPS-PIN | `lookup_async` BlockDevice arm holds the read pin across `promote_and_serve`; `AlreadyExists` → `serve_concurrently_promoted` (`src/lib.rs:3072-3103`); SSD evictor reorders `dm.remove` before `mt.remove` (`src/background.rs:349-358`) |
| `d450c7cc1` | DIS-SPIN-SSD-EVICTOR-RELEASES-AFTER-REMOVE | regression test only (`src/repair_test_ssd_evictor_release.rs:290`) |
| `ad3654207` | DIS-COPY-COMPLETED-EXISTING-ENTRY-PRESERVED | on `AlreadyExists`, free the slot only if the existing entry provably lives elsewhere (`src/lib.rs:3560-3585`) |
| `9defa9e1a` | DIS-POPULATE-FAILURE-RELEASES-SLOT | `populate` releases the reserved slot on GPU-unbound / D2H / sync failure (`src/lib.rs:3215-3246`) |
| `4f8b5554b` | — | `#[cfg(test)]` per test module (`src/lib.rs:72-74, 3970-3972, 7186-7197`; `src/background.rs:757-759`) |

**Why this is a full sweep and not a delta.** The previous dated notes are delta sweeps scoped to one input each. They state that their method validates nothing outside that scope ("this stamp is not evidence against it"). The 2026-09-28 sweep also recorded that this component cites only 4 requirement ids in code, so counting citations proves nothing here. This sweep therefore checked **meaning**: each active FR/SC/acceptance scenario was read against the function that implements it. Each verdict cites `file:line`. Requirements marked REMOVED/WITHDRAWN (001 FR-020/021/022/026/027/044, SC-008; 002 FR-016-as-original) are not counted.

**Cross-component evidence consulted (read only):** `components/interfaces/src/idispatcher.rs` (trait, `DispatcherConfig::default` `:105-127`, `ServedBy` `:248-274`), `components/interfaces/src/idispatch_map.rs`, `components/dispatch-map/src/lib.rs` (real `lookup` pins every found entry incl. BlockDevice `:142-150`; `remove` refuses `read_ref>0` with `ActiveReferences` `:341-343`; `try_evict_to_block` `:548-570`), `lib/shmq-dispatcher/src/translate.rs` (`op_pin` `:596-608`, `wire_tier` `:700-710`, `tally_lookup` `:712-734`, `op_lookup` `:736-770`), `apps/certus-server-yaml/build.rs:447-471` and `apps/certus-server/src/main.rs:469,529` (dispatcher `initialize` precedes `create_eviction_channel`).

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 117. That is 108 in this component's scope: the active FRs and SCs of both specs plus the acceptance scenarios and edge cases singled out below. The other 9 items of spec 002 belong to other components and are only listed |
| ✓ Aligned | 81 (75.0% of 108). This is the remainder after the other categories; the aligned items are listed below, grouped where one function implements several requirements |
| ⚠️ Drifted | 15 (13.9%). 5 are this branch's fixes, which the spec does not yet describe (BACKFILL). 10 were already there before this branch |
| ◐ Partially implemented | 3 (2.8%). All 3 are gaps this branch's repairs left open or closed on only one path |
| ✗ Not implemented | 0 |
| ⧗ Not verifiable here (hardware / cluster / benchmark) | 9 (8.3%) |
| 🆕 Unspecced Code | 5 |

What this branch's repairs mean for the spec (one line each):
1. D-01 · FR-036 / FR-051: the cold `lookup` pin and race recovery are now implemented but not specified (**BACKFILL**).
2. D-02 · FR-032 + edge case: SSD evictor order `dm.remove` → `mt.remove` (**BACKFILL**).
3. D-03 · FR-003 / FR-059: populate releases its slot after a post-reserve failure (**BACKFILL**). P-01: phase-3 failures are still uncovered (**ALIGN**).
4. D-04 · FR-056 `copy_gpu_to_memory_completed`: keeps the live slot on `AlreadyExists` (**BACKFILL**).
5. D-05 · FR-042 / FR-050: background evictor drops are now counted (**BACKFILL**). P-02: the SSD evictor's sender, copied at init, is still dead in production (**ALIGN**).
6. 002 FR-009/FR-010: now aligned (previously drifted). No spec change needed.
7. P-03: the cold-pin repair fixed `lookup_async` only; `batch_lookup` still drops the pin (**ALIGN**, major).

## Detailed Findings

### Spec: 001-dispatcher-cache-interface — Dispatcher Cache Interface (Memory-Tier Architecture)

#### Aligned ✓

- FR-001 method inventory → `components/interfaces/src/idispatcher.rs:499-877`. All 24 listed trait methods exist (initialize … tier_event_stats). `create_eviction_channel` / `eviction_dropped_count` are inherent (`src/lib.rs:562-577`).
- FR-002 `DispatcherError` → `idispatcher.rs:167-181` (NotInitialized, KeyNotFound, AlreadyExists, AllocationFailed, IoError, Timeout, InvalidParameter).
- FR-004 write-through via `peek` + `convert_to_storage` → `src/lib.rs:1395-1470` (`mt.peek` `:1404`, `convert_to_storage` `:1469`).
- FR-005 slot not freed on write-through completion → `src/lib.rs:1466-1469` (no `mt.remove`).
- FR-006 warm path per-region `memcpy_h2d_async` on `warm`, `dma_copy_to_device` fallback → `src/lib.rs:1267-1341`, call `:3057-3064`.
- FR-007 miss → `KeyNotFound` → `src/lib.rs:3038`, `:2394-2395`.
- FR-008 `check` no transfer → `src/lib.rs:3110-3129`.
- FR-009 `remove` frees slot / extent / map entry → `src/lib.rs:3131-3171`; extent drive by `drive_index` `:3164`.
- FR-010 `define_component!`, provides `[IDispatcher]` → `src/lib.rs:408-411`.
- FR-011 receptacles incl. `IRemoteLookup` (`src/lib.rs:412-419`). NUMA auto-placement skips the node's first 2 cores (`:604-654`, `.skip(2)` `:641`). `poller_base_cpu + i` (`:1617-1622`). Remote `NotFound`→`KeyNotFound`, `TransportError`→`IoError` (`:2837-2842`).
- FR-012 validates dispatch_map + memory_tier → `src/lib.rs:1897-1903`.
- FR-013 dispatch-map ref locking → read pins taken/released throughout (`pins::PinnedKeys` `src/pins.rs:32-49`).
- FR-014 shutdown joins workers, checkpoints extent managers → `src/lib.rs:2120-2142`.
- FR-015 / FR-016 N drives × N extent managers, FormatParams from data partition → `src/lib.rs:1817-1839`.
- FR-017 write-through failure drops silently → `src/lib.rs:1443-1461`.
- FR-018 `remove` does not wait for write-through → `src/lib.rs:3131-3171` (no flush).
- FR-023 `touch` → `src/lib.rs:3685-3701`.
- FR-024 pin-safe `evict_for_space` / `evict_one_clean` (skip unpersisted, never drop) → `src/lib.rs:1055-1173`; fragmentation relief `:1202-1211`.
- FR-025 `format_on_init=false` recovery via `for_each_extent` + `recover_extent` → `src/lib.rs:1919-1937`.
- FR-029 / FR-030 / FR-031 SSD evictor start, interval, threshold/low-watermark → `src/lib.rs:2043-2076`, `src/background.rs:311-394`.
- FR-033 config defaults → `idispatcher.rs:105-127`. All 12 listed defaults match (0.9/0.8/64/5/2048/0.0/0.70/64/2/64/16 MiB/5000), as do the 2026-08-07 fields (128 MiB/128 MiB/10).
- FR-034 register_host_memory, `ChannelPool` + RAII `ChannelLease`, drain on checkout, connect outside lock → `src/lib.rs:299-357` (drain `:325`, connect outside lock `:315-321`), register `:1983-2003`.
- FR-035 unregister at shutdown → `src/lib.rs:2144-2149`.
- FR-039 batch_lookup steps (1)–(6): Err-retry ×5 (`src/lib.rs:2382-2391`). Warm copies queued and pins adopted (`:2418-2431`). `batch_touch` before cold (`:2460-2462`). Deferred warm sync after the cold block (`:2747-2755`), then pin release (`:2759`), then remote (`:2761`). Group by `drive_index` (`:2515-2520`). One lease per drive plus a single scatter-gather call with depth 128 (`:2522`, `:2641-2678`). Input order kept (`:2961-2986`).
- FR-040 `promote_to_memory_tier` best-effort → `src/lib.rs:3703-3859`.
- FR-041 `pipelined_ssd_to_dram_only` / `pipelined_multi_ssd_to_dram_only` exist → `src/pipeline.rs:1166`, `:1279`; used per drive thread `src/lib.rs:3826`.
- FR-043 `PipelineMetrics` trait, optional → `src/metrics.rs:12-47`, `src/lib.rs:556-558`.
- FR-045 remote forwarding of `KeyNotFound` → `src/lib.rs:2761-2940`.
- FR-046 / FR-047 / FR-048 / FR-049 memory-tier evictor: start (`src/lib.rs:2079-2105`), idle interval, 200 ms / 500 ms pacing, quadratic pressure, 4× widening (`src/background.rs:507-597`, `:534`, `:547`, `:589-597`), `try_evict_to_block` before `mt.remove` (`:556-562`).
- FR-051 (batch_lookup half) concurrent-promotion recovery 5 s / 50 µs → `src/lib.rs:1354-1393`, `:2946-2955`.
- FR-052 per-device `DEVICE_STREAMS` {warm, store, pipe[2]} → `src/lib.rs:456-506`; batch device `:2325-2331`.
- FR-053 `StagingPool` / `StagingLease`, staging post-pass → `src/pipeline.rs:96-151`, `src/lib.rs:924-1035`, `:2725-2743`.
- FR-054 drain-all-on-error (`stop_submitting`) → `src/pipeline.rs:310-406`, `:569-682`, `:882-1101`.
- FR-055 partition-table guard → `src/lib.rs:1778-1788`.
- FR-057 DI setters → `src/lib.rs:544-558`.
- FR-060 backpressure 20 ms poll / shared deadline / drop-on-full → `src/lib.rs:3437-3471`, `:3194-3212`, `:3322-3333`.
- FR-061 `scatter_gather_multi_drive_zero_copy` + `DriveWork` → `src/pipeline.rs:746`, `:771`; proportional split `:853`; single final dual sync `:1124-1125`.
- User Story 1 AS1–AS5; User Story 2 AS1, AS3; User Story 3 AS1–AS2; User Story 4 AS1–AS3; User Story 5 AS1–AS4 (empty `data_pci_addrs` rejected first `src/lib.rs:1905-1909`); User Story 7 AS1–AS5; User Story 8; User Story 10 AS1–AS5; User Story 11 AS1–AS3, AS5–AS6 (`mixed_batch_defers_warm_sync_past_cold_block` `src/lib.rs:6136`); User Story 12 AS1–AS5.
- Edge cases: populate-full drop; `evict_for_space` exhaustion; concurrent lookups; SSD-full write-through; unpersisted skip; partition guard.
- SC-001 – SC-007, SC-009 – SC-011, SC-013, SC-015, SC-016, SC-017 (partition test `the_route_counters_partition_the_served_keys` `src/lib.rs:5954`).

#### Drifted ⚠️

**D-01 · FR-036 / FR-051 — cold single-key `lookup` now holds its read pin across promotion and recovers a lost insert race (branch `1d24ff27f`)** — severity: moderate (spec silent on a correctness property)
- Spec FR-036 (`spec.md:313`) says only that `lookup_async` "performs the same cache lookup as `lookup` (dispatch-map query, memory-tier hit, BlockDevice promotion)". FR-051 (`spec.md:328`) scopes race recovery to "a `batch_lookup` cold promotion".
- Code: `lookup_async`'s BlockDevice arm now keeps the `dm.lookup` read pin until `promote_and_serve` returns, so a concurrent `remove` (`dm.remove` → `ActiveReferences`) or SSD eviction cannot delete the entry or free the fresh slot (`src/lib.rs:3072-3076`, release `:3099`). It maps `AlreadyExists` to `serve_concurrently_promoted` (`:3086-3097`). Before the fix (base `lib.rs:3063-3065`) the pin was released *before* promotion.
- Test: `concurrent_remove_cannot_delete_entry_during_cold_promotion` (`src/repair_test_cold_pin.rs:508`).
- Direction: BACKFILL (code is a verified fix).

**D-02 · FR-032 + Edge Case "When the SSD evictor removes an entry …" — removal order** — severity: moderate
- Spec FR-032 (`spec.md:309`): MemoryTier entries are skipped; pinned entries "skipped gracefully (dm.remove fails without panic)". Edge case (`spec.md:236`): "it frees the extent … and removes the dispatch-map entry. No memory-tier operation is needed (the entry was already evicted from DRAM)."
- Code: the evictor calls `mt.remove(key)` — so a memory-tier operation *is* performed — and since `1d24ff27f` it does so only **after** `dm.remove` succeeds (`src/background.rs:349-358`). The reason is in the comment: a cold lookup can promote the candidate into a fresh slot between `get_evictable_offset` (`:409-429`) and the removal, and freeing first released that slot. The edge case's claim that the entry is always already out of DRAM is false for exactly that race.
- Test: `ssd_evictor_keeps_slot_when_remove_fails` (`src/repair_test_ssd_evictor_release.rs:290`).
- Direction: BACKFILL.

**D-03 · FR-003 / FR-059 — populate failure after reservation releases the slot (branch `9defa9e1a`)** — severity: moderate
- Spec FR-003 (`spec.md:280`) lists the happy path and the drop-on-full case. It says nothing about failures between reserve and registration. FR-059 (`spec.md:354`) says "Entries that fail allocation or DMA do not prevent other entries from succeeding" but not that their slot is released.
- Code: `populate` calls `release_memory(key)` on gpu-services-unbound (`src/lib.rs:3220-3228`), `copy_gpu_to_memory_async` error (`:3235-3240`) and `stream_synchronize` error (`:3241-3246`). The comment at `:3216-3218` explains that before Phase 3 the slot has no dispatch-map entry, so no evictor can reclaim it. A leak would make every retry fail with `AlreadyExists`. `batch_populate` already did this (`:3316-3319`, `:3346-3354`).
- Tests: `populate_without_gpu_service_releases_the_slot`, `populate_sync_failure_releases_the_slot_and_allows_retry`, `populate_null_stream_sync_failure_releases_the_slot` (`src/repair_test_populate_slot.rs:485,504,534`).
- Direction: BACKFILL (the uncovered remainder is P-01 below).

**D-04 · FR-056 `copy_gpu_to_memory_completed` — existing entry preserved on `AlreadyExists` (branch `ad3654207`)** — severity: moderate
- Spec FR-056 (`spec.md:338`): "finalizes the entry after the async copies complete, transitioning it to a resident `MemoryTier` entry (design invariant P10)." The duplicate case is not specified.
- Code: on `DispatchMapError::AlreadyExists` it looks the key up again. It frees the slot only if the key no longer exists, or if the existing entry points at a different place. If the existing entry points at **this** slot, or the lookup errors, the slot is kept, and `AlreadyExists` is returned either way (`src/lib.rs:3560-3580`). Before the fix it called `mt.remove(key)` unconditionally, which freed the live slot of a resident entry: a use-after-free for every later reader.
- Test: `completed_on_registered_key_keeps_the_live_slot` (`src/repair_test_copy_completed.rs:428`).
- Direction: BACKFILL.

**D-05 · FR-042 / FR-050 — eviction event drop accounting covers the background evictors (branch `0d3fcde43`)** — severity: moderate
- Spec FR-042 (`spec.md:319`): "Eviction events are emitted … whenever an entry is evicted from the memory-tier by `evict_for_space`. … when the receiver falls behind, events are dropped and a dropped-event counter is incremented". FR-050 (`spec.md:327`): the memory-tier evictor "MUST emit `EvictionEvent { key, reason: Demoted }` (or `Removed` on transition failure)".
- Code: the sender is now `EvictionSender { tx, dropped: Arc<AtomicU64> }`. `emit()` either queues the event or increments the shared counter (`src/background.rs:225-241`). Users: the foreground path (`src/lib.rs:579-584`), the MT evictor's `Demoted` events (`src/background.rs:564-569`) and the SSD evictor's `Removed` events (`:360-365`). So FR-042's emitter list ("by `evict_for_space`") is too narrow: the background MT evictor and the SSD evictor emit too, and since this branch their drops are counted. FR-050's "(or `Removed` on transition failure)" is stale. The MT evictor `continue`s on transition failure (`:556-558`) and emits nothing. `Removed` comes only from the SSD evictor.
- Test: `background_evictor_counts_demoted_events_that_do_not_fit` (`src/repair_test_eviction_events_dropped.rs:170`).
- Direction: BACKFILL.

**D-06 · FR-038 `clear_memory_tier` — frees the slot of a pinned / in-flight entry** — severity: **major** (pre-existing; not caused by this branch)
- Spec FR-038 (`spec.md:315`): "evicts ALL entries … by calling `IMemoryTier::evict_next()` in a loop until empty. For each evicted key, it transitions the dispatch-map entry to BlockDevice state via `convert_memory_tier_to_block`. If the transition fails (write-through not complete), the entry is removed from the dispatch map entirely."
- Code: `src/lib.rs:3875-3890` loops `mt.oldest_keys(64)`. It calls `try_evict_to_block`. On **any** failure, including `ActiveReferences` (a pinned, in-flight load), it calls `mt.remove(key)` **unconditionally** and then `dm.remove(key)`. The real dispatch map refuses that `remove` while the key is pinned (`components/dispatch-map/src/lib.rs:341-343`). The result: the slot is freed, but the dispatch-map entry still says `MemoryTier { pointer }`. That is exactly the dangling-pointer pattern this branch's repairs D-02/D-04 remove elsewhere, and what FR-024/FR-049 forbid ("dispatch-map transition happens BEFORE the DRAM slot is freed"). The spec text is also stale in two places: it names `evict_next()` and `convert_memory_tier_to_block`, but the code uses `oldest_keys()` and `try_evict_to_block`. Exposed over shm-queue `CLEAR_MEMORY_TIER` (`lib/shmq-dispatcher/src/translate.rs:355,891-894`) and used by the workload node agent (`apps/workload-generator/crates/workload-node-agent/src/agent.rs:256`).
- Direction: ALIGN (code fix) plus a spec update of the mechanism wording.

**D-07 · FR-028 / User Story 9 AS3 — no-drives `promote_and_serve` never re-registers the entry** — severity: moderate (pre-existing; memory-tier-only / test mode)
- Spec FR-028 (`spec.md:305`): "On BlockDevice lookup (promotion), the pipelined reader MUST re-insert the entry into the memory-tier and re-register it as a MemoryTier entry". US-9 AS3 (`spec.md:166`): with no hardware, it "creates a memory-tier slot …, performs a direct GPU DMA from the slot, and re-registers the entry".
- Code: `src/lib.rs:800-824` calls `dm.create_memory_tier_entry(key, …)` on an entry that already exists (BlockDevice), which returns `AlreadyExists` and is discarded (`.is_ok()` `:814-819`). It then calls `dm.release_write(key)` without holding a write ref (`:820`), which the real map rejects (`RefCountUnderflow`, `components/dispatch-map/src/lib.rs:294-296`). The entry stays BlockDevice, and the inserted slot is orphaned: tracked by the memory tier, unreachable through the map. `batch_lookup`'s no-drives branch does it correctly with `promote_block_to_memory_tier` (`src/lib.rs:2495-2500`). The no-drives branch of `promote_and_serve` was not updated. The failure is hidden, because the mock dispatch map in `src/lib.rs:4428-4451` also returns `AlreadyExists`.
- Direction: ALIGN.

**D-08 · FR-019 / User Story 9 AS1 / Key Entities "Pipelined Reader" — a `pipelined_ssd_to_gpu` ring-buffer fallback that does not exist** — severity: minor (pre-existing)
- Spec US-9 AS1 (`spec.md:164`): "The fallback path (ring-buffer `pipelined_ssd_to_gpu`) is used when the memory-tier pool is not CUDA-pinned/SPDK-registered." The Q&A (`spec.md:265`) describes it too, with 8 DmaBuffers.
- Code: no such function. The pipeline module's public functions are `pipelined_ssd_to_gpu_zero_copy`, `pipelined_multi_object_zero_copy`, `scatter_gather_multi_drive_zero_copy`, `pipelined_ssd_to_dram_only` and `pipelined_multi_ssd_to_dram_only` (`src/pipeline.rs:235,457,771,1166,1279`). `promote_and_serve` always uses the multi-object zero-copy path (`src/lib.rs:875-886`), with no registration check. `pipelined_ssd_to_gpu_zero_copy` is used only by the staging serve (`:998-1011`), not as a "single-entry fallback" for registration failure.
- Direction: BACKFILL (delete the stale fallback text, and say where `pipelined_ssd_to_gpu_zero_copy` is used now).

**D-09 · FR-037 — "All per-device streams MUST be destroyed during `shutdown()`"** — severity: minor (pre-existing)
- Spec (`spec.md:314`) says exactly that.
- Code: `shutdown` destroys only the fallback `warm_stream` and the `PipelineRing` (`src/lib.rs:2151-2161`). `DEVICE_STREAMS` is a process-global `OnceLock<Mutex<HashMap>>` whose entries "live until the process exits" (`src/lib.rs:453-455`, `:467-469`). Nothing destroys them.
- Direction: BACKFILL (the design comment is explicit and deliberate).

**D-10 · Edge Case "When remove is called … memory-tier slot is freed first, then the dispatch-map entry is removed"** — severity: minor (pre-existing; spec contradicts the safe order)
- Spec (`spec.md:231`) says exactly that.
- Code: `remove` calls `dm.remove(key)` first. That refuses pinned entries and stops new lookups from getting the pointer. Only then does it call `mt.remove(key)` (`src/lib.rs:3152-3160`). This is the order FR-024/FR-049 require, and the order this branch now applies to the SSD evictor (D-02). The pre-branch code was already in this order (base `lib.rs:3117`), so the spec is simply stale. It also leaves out that `remove` of a pinned entry returns `KeyNotFound` (see U-04).
- Direction: BACKFILL.

**D-11 · FR-058 `store_already_resident` — counted on load paths too** — severity: minor (pre-existing)
- Spec FR-058 (`spec.md:352`) → interfaces FR-018a: "reserves refused because the key was already cached".
- Code: the counter is incremented in `evict_and_insert` on `MemoryTierError::AlreadyExists` (`src/lib.rs:1213-1222`). `evict_and_insert` is called by `reserve_memory` (store) but also by `promote_and_serve` (`:773`) and the `batch_lookup` cold prep (`:2562-2563`). So every concurrent-promotion race loser (FR-051) on the **load** path also counts as a "store already resident". The counter therefore over-reports store declines in exactly the TP-sibling workload FR-051 describes.
- Direction: ALIGN (count only on the store path), or BACKFILL the wider meaning. Human decision; see the proposal.

**D-12 · FR-003 — names `IGpuServices::dma_copy_to_host` for populate** — severity: minor (pre-existing)
- Spec FR-003 (`spec.md:280`): "initiate DMA copy … via `IGpuServices::dma_copy_to_host`". Assumptions (`spec.md:410`) repeat it.
- Code: `populate` calls `copy_gpu_to_memory_async` → per-region `memcpy_d2h_async` on the `store` stream, then `stream_synchronize` (`src/lib.rs:3232-3246`, `:3519-3537`). `dma_copy_to_host` is not called anywhere in non-test dispatcher code (only the mock at `src/lib.rs:4724`). FR-056 and FR-037 already describe the real path, so FR-003 contradicts them inside the same spec.
- Direction: BACKFILL.

#### Partially implemented ◐

**P-01 · FR-003 / FR-056 — slot release on phase-3 failure (residual of `9defa9e1a`)** — severity: moderate
- The populate repair's own invariant (`src/lib.rs:3216-3218`: "every failure here must release it, or the DRAM leaks and a retry of this key fails with AlreadyExists") is enforced for phase 2 only.
- Gaps in `copy_gpu_to_memory_completed`:
  - **Missing slot** (`:3558`): returns `KeyNotFound` with nothing to release. Benign.
  - **integrity-check, after the entry exists** (`:3599-3616`): a `stream_synchronize` or `set_checksum` failure calls `mt.remove(key)` but leaves the just-created dispatch-map entry with `write_ref` held. That is a dangling `MemoryTier` entry that blocks every lookup until timeout.
  - **`downgrade_reference` failure** (`:3619-3620`): returns `IoError` and leaves the entry write-locked.
- Direction: ALIGN.

**P-02 · FR-042 — SSD evictor's sender is captured at `initialize()`** — severity: moderate (pre-existing wiring, left open by `0d3fcde43`)
- `BackgroundEvictor::start` receives `self.eviction_tx.lock().unwrap().clone()` at `initialize()` (`src/lib.rs:2059-2071`). That is a one-time copy, unlike the MT evictor, which gets the shared `Arc<Mutex<Option<…>>>` (`:2101`). Both servers call `create_eviction_channel` **after** dispatcher `initialize` (`apps/certus-server-yaml/build.rs:447-471`; `apps/certus-server/src/main.rs:469` then `:529`). So in production the SSD evictor's copy is `None`: its `Removed` events are never sent, and with this branch's counting they are not counted either. That defeats the "never lost silently" claim the new `EvictionSender` doc makes (`src/background.rs:227-228`).
- Direction: ALIGN (pass the shared handle, as the MT evictor does).

**P-03 · `batch_lookup` cold path drops its read pin before promotion (the cold-pin repair `1d24ff27f` fixed `lookup_async` only)** — severity: **major**
- Spec: FR-051 (`spec.md:328`) says "The caller's load pin keeps the entry from being evicted while waiting." D-01's backfill makes the pin-through-promotion guarantee explicit for `lookup`. Nothing in the spec allows `batch_lookup` to be weaker.
- Code: `batch_lookup` releases the classification pin right away on BlockDevice (`src/lib.rs:2433-2434`). Then it runs `evict_and_insert` → NVMe read → `promote_block_to_memory_tier` with no pin held (`:2562-2717`). An SSD-evictor `dm.remove` or a client `remove` in that window succeeds (the real map refuses only `read_ref>0`, `components/dispatch-map/src/lib.rs:341-343`). The fresh slot is then either freed under the in-flight NVMe/H2D (remove path: `mt.remove` after `dm.remove`) or orphaned, and `promote_block_to_memory_tier` fails with `KeyNotFound` → `IoError`. This is the same race the repair closed for `lookup`, on the path the shm-queue `LOOKUP` op actually uses (`lib/shmq-dispatcher/src/translate.rs:756`). It is safe **only if** the client pinned the key first (`op_pin` → `dispatcher.pin` `translate.rs:596-602`), which the vLLM connector does in its Check→Pin sequence, but `IDispatcher::batch_lookup` does not require it. The same pattern exists in `promote_to_memory_tier` (`src/lib.rs:3726-3729`).
- Direction: ALIGN (keep the pin as `lookup_async` now does, or make "caller holds a pin" a documented precondition of `batch_lookup`).

#### Not Implemented ✗
None.

#### Not verifiable here ⧗
- SC-012: data correctness under FIFO NVMe ordering. Needs NVMe hardware (`--features hardware-test`).
- SC-014: ≥5× batch-cold throughput. Benchmark only.
- User Story 11 AS4: per-entry error isolation is implemented (`src/lib.rs:2681-2718`). It is not proven under real NVMe errors. The scenario's wording ("When the thread encounters the error") is also stale, because the cold path has no threads any more; a wording fix is proposed.
- Edge case "write-ref downgrade lets a concurrent lookup proceed". This depends on dispatch-map semantics (`downgrade_reference` `src/lib.rs:3619`). Not exercised here.

### Spec: 002-served-by-tier-attribution — Serving-Tier Attribution (`served_by`)

#### Aligned ✓
- FR-001 six values → `idispatcher.rs:248-274` (Dram, Ssd, Remote, Miss, SizeMismatch, Error).
- FR-002 / SC-001 exactly one value per key and none unspecified → `src/lib.rs:2961-2986`. `None` on `Ok` → `Error` with a `debug_assert` (`:2973-2976`).
- FR-003 hit/non-hit split → `is_hit()` used by `tally_lookup` (`lib/shmq-dispatcher/src/translate.rs:727-731`); iff test `src/lib.rs:5914-5922`.
- FR-004 route, not residency → cold marked `Ssd` at classification (`src/lib.rs:2440`).
- FR-005 `Miss` ⇔ `KeyNotFound` after remote → `src/lib.rs:2977`; test `:5925-5933`.
- FR-006 `SizeMismatch` from `InvalidParameter` → `src/lib.rs:2981`.
- FR-007 `Error` otherwise → `src/lib.rs:2982`.
- **FR-009 / FR-010 — now aligned on this branch** (`a7ccc6e48`). The recovery pass rewrites `tier[i] = Some(ServedBy::Dram)` together with the result (`src/lib.rs:2946-2954`). On base, the key kept the `Ssd` from classification. Test `concurrent_promotion_loser_is_attributed_dram` (`src/repair_test_served_by.rs:432`). *(FR-009 is not fully aligned; see D-13.)*
- FR-011 staging → `Ssd` (inherits classification, `src/lib.rs:2440`, `:2730-2742`).
- FR-012 remote overwrite → `tier[pos] = Remote` (`src/lib.rs:2880`). A remote `NotFound` keeps `KeyNotFound` → `Miss`, counted once.
- FR-013 no lock / allocation / IDispatchMap call per key for attribution → `tier` is one `Vec` per batch (`src/lib.rs:2319`). *(The SSD and DRAM counts are two extra O(n) passes per batch, `:2997-2998`; no lock, so within the requirement.)*
- FR-017 remote attribution from per-key success → `src/lib.rs:2845-2848`, `:2880`.
- FR-019 / FR-020 / FR-021 / FR-022 wire byte → `wire_tier` 1/2/3 and 0 for non-served (`lib/shmq-dispatcher/src/translate.rs:700-710`).
- FR-023 server gets the tier only from the dispatcher → `translate.rs:764`.
- FR-023a `check_state` untouched → `lib/shmq-dispatcher/src/wire.rs:107`.
- FR-024 / SC-004 hits + misses + errors = requested → `tally_lookup` (`translate.rs:712-734`).
- FR-025 aggregates agree with attribution → `tally_lookup` reads `served_by` directly (`translate.rs:724-731`). The route counters come from the same `out` (`src/lib.rs:2997-2999`).
- FR-026 no dependence on the eviction stream → counters polled via `tier_event_stats` (`src/lib.rs:3945-3967`).
- FR-033 per-variant counters, per candidate → `src/lib.rs:1071-1089`.
- FR-034 `eviction_scans_exhausted` → `src/lib.rs:1111`.
- FR-036 gauges sampled over the 64 oldest → `src/lib.rs:3959-3964`.
- US1 AS1, AS2, AS4, AS5; US4 AS1–AS3; edge cases "local miss then remote hit", "concurrent promotion", "cold-load staging", "empty batch" (`src/lib.rs:2234-2236`), "mixed batch, one fails".

#### Drifted ⚠️

**D-13 · US1 AS3 / FR-009 / FR-003 — a failed warm batched sync is reported as `Dram` hits** — severity: **major** (pre-existing; identical on base `lib.rs:2745-2749`)
- Spec 002 US1 AS3 (`spec.md:303-305`): "**Given** a batch in which the batched GPU synchronization fails, **When** `batch_lookup` returns, **Then** every key whose copy was in flight is attributed `ERROR` and none is reported as a hit." The edge case at `spec.md:430-434` says the same: "A failed batched sync rewrites already-`Ok` results to `IoError` … Attribution MUST be rewritten in lockstep".
- Code: the warm batched sync failure is only **logged** (`src/lib.rs:2750-2755`, `self.log_info(...)`). `results[i]` stays `Ok(())` and `tier[i]` stays `Dram`, so every warm key counts as a hit (`promotions_to_gpu`, `lookup_hits_dram`, wire byte 1) even though the H2D copy may not have landed. The remote-delivery sync does it right: on failure it rewrites each submitted key to `IoError` (`:2907-2917`), and that is tested (`remote_delivery_reports_sync_failure_per_key` `:5817`). No test covers the warm-sync failure.
- Direction: ALIGN.

**D-14 · FR-008 — lists an "inline fallback" cold sub-path that was removed** — severity: minor
- Spec (`spec.md:474-477`): "the cold SSD promotion (all of its sub-paths — pooled, inline fallback, no-drives, and staging)". The comment at `src/lib.rs:2435-2437` repeats the list.
- Code: the inline single-key `promote_and_serve` fallback was removed from `batch_lookup` (spec 001 FR-039(3), `src/lib.rs:2469-2476`). Today's sub-paths are scatter-gather, no-drives and staging (`:2486-2743`).
- Direction: BACKFILL.

**D-15 · FR-027 / SC-009 — per-value attribution tests exist only for `Dram` and `Miss`** — severity: moderate (pre-existing)
- Spec (`spec.md:554-555`): "Each attribution value MUST have a test that fails if that value is mis-assigned, in both dispatchers."
- Code: in this crate, value-specific assertions exist only for `Dram` (`src/repair_test_served_by.rs:484`) and `Miss` (`src/lib.rs:5927`). The other tests check that `is_hit ⇔ is_ok` (`:5916`) and that the route counters sum to the served count (`:5963`), and neither of those can tell `Ssd`, `Dram` and `Remote` apart. A grep for `ServedBy::` across `src/` tests, `tests/` and `benches/` finds no assertion on `Ssd`, `Remote`, `SizeMismatch` or `Error`.
- Direction: ALIGN (add the tests).

#### Notes on other test requirements (not counted again)
- FR-028 (tests shown to fail first): met for the repairs on this branch, whose commit messages note a "red-first test", but not evidenced for the attribution values above.
- FR-029 `integrity-check` configuration: the repair test mocks gate `set_checksum`/`get_checksum` behind the feature (e.g. `src/repair_test_cold_pin.rs:211-215`), so they *compile* under it. No evidence that the suite is *run* under it (no CI workflow in this worktree). Unchanged from the 2026-09-28 coverage table.

#### Not verifiable here ⧗
- SC-002 (capacity sweep), SC-003 (multi-node), SC-006 (old/new client matrix), SC-007 (unmodified peer), SC-008 (n ≥ 8 benchmark).

#### Owned by other components (listed, not judged)
FR-014, FR-015, FR-035 (dispatcher-p2p); FR-016-replacement, FR-018 (remote-lookup); FR-030 – FR-032 (documentation process); SC-005 (p2p comparison); US3, US5 (other components).

### Unspecced Code 🆕

| # | Feature | Location | Lines | Suggested Spec |
|---|---------|----------|-------|----------------|
| U-01 | `integrity-check` serve-side CRC-32: computed at `copy_gpu_to_memory_completed`, checked before each warm / remote / single-key H2D, mismatch → `IoError` | `src/lib.rs:1237-1259`, `2408-2417`, `2861-2869`, `3047-3055`, `3593-3617` | ~70 | 001 (new FR-062). The interfaces spec specifies only the `IDispatchMap` checksum storage (interfaces `001-interfaces` spec.md:31,167-168), not the dispatcher's verify-before-serve behaviour |
| U-02 | Remote-delivery stage probe (`CERTUS_LOG_REMOTE_DELIVERY`) | `src/remote_probe.rs` (160), call sites `src/lib.rs:2786-2938` | 160 | 001 (FR-045 note) |
| U-03 | Opt-in per-call reserve logging (`CERTUS_LOG_RESERVE`) | `src/lib.rs:3379-3394` | 16 | 001 (FR-056 note) |
| U-04 | `remove` of a pinned or write-locked entry returns `KeyNotFound` (the `ActiveReferences` from `dm.remove` is collapsed; a `dm.lookup` timeout is collapsed too) | `src/lib.rs:3140-3155` | 16 | 001 FR-009 / `contracts/errors.md:33,37` (`Timeout` is listed for `remove` but never returned) |
| U-05 | SSD evictor emits `EvictionEvent { reason: Removed }` | `src/background.rs:360-365` | 6 | 001 FR-042 / FR-050 (folded into the D-05 proposal) |

## Inter-Spec Conflicts

1. **001 edge case `spec.md:231` vs 001 FR-024/FR-049.** The edge case says `remove` frees the memory-tier slot before the dispatch-map entry. FR-024 ("the dispatch-map transition happens BEFORE the DRAM slot is freed") and FR-049 ("Only after the dispatch-map reflects BlockDevice state MUST the DRAM slot be freed") require the reverse order for eviction, and the code uses the reverse order for `remove` too (D-10).
2. **001 FR-003 vs 001 FR-056 / FR-037.** FR-003 says populate copies via `dma_copy_to_host`. FR-056/FR-037 say `copy_gpu_to_memory_async` → `memcpy_d2h_async` on `store` (D-12).
3. **001 FR-038 vs 001 FR-024 / FR-049.** FR-038 allows "removed from the dispatch map entirely" after the slot is gone, with no pin check. FR-024/FR-049 forbid freeing a slot a pinned load points at (D-06).
4. **001 FR-019 (single-entry fallback "`pipelined_ssd_to_gpu_zero_copy`") vs 001 FR-053 / code.** That function is now the staging-serve primitive, not a registration fallback (D-08).
5. **002 US1 AS3 vs 001 FR-039 step (2).** 001 describes the deferred warm sync, but not its failure semantics. 002 requires `ERROR` attribution on failure, and the code follows neither (D-13).

## Recommendations

1. **ALIGN, major, fix first:** D-13 (warm-sync failure reported as hits), P-03 (`batch_lookup` cold pin), D-06 (`clear_memory_tier` frees pinned slots). D-13 and D-06 predate this branch. P-03 is the same race this branch's formal-verification repair closed for `lookup`, and it remains on the hot path.
2. **BACKFILL the five behaviour-changing repairs into spec 001** in the same change as this branch, so the next stamp certifies them: D-01 (FR-036/FR-051), D-02 (FR-032 + edge case), D-03 (FR-003/FR-059), D-04 (FR-056), D-05 (FR-042/FR-050). The sixth repair (`a7ccc6e48`) needs no spec change, since it makes the code match 002 FR-009/FR-010. Exact text is in `proposals.md`.
3. **ALIGN, moderate:** P-01 (phase-3 slot release), P-02 (SSD evictor sender snapshot), D-07 (no-drives promote).
4. **BACKFILL, minor, stale text:** D-08, D-09, D-10, D-12, D-14, and US-11 AS4 wording.
5. **Decide** D-11 (`store_already_resident` scope; ALIGN is proposed, with a BACKFILL alternative) and the U-01…U-04 specs.
6. **Add per-value attribution tests** (D-15): one each for `Ssd`, `Remote`, `SizeMismatch` and `Error`.
7. Re-run `scripts/spec-sync-hash.sh components/dispatcher` only after the spec edits land. The current `spec_sync_drift_status: clean` stamp in the frontmatter (`spec_sync_git_commit: 474ba0a6`) predates this branch's `src/**` changes and does **not** describe this branch.

---

## Sweep 2026-09-21 — Spec Drift Report — dispatcher

Generated: 2026-09-21
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)
Mode: Read-only drift analysis, then apply — **BACKFILL** to `spec.md` (Finding 1,
code authoritative) plus a **CODE deletion** of dead `ColdReadPool` scaffolding
(Finding 2, user-directed).
Branch: `evolve-throughput`

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Drift findings this sweep | 2 |
| ⚠️ Drifted → resolved by backfill (spec→code) | 1 (Finding 1: scatter-gather batch cold path) |
| 🧹 Dead code → resolved by deletion (code) | 1 (Finding 2: unused `ColdReadPool`) |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 0 (after this sweep; `scatter_gather_multi_drive_zero_copy` now specced as FR-061) |

Scope of this sweep: since the last clean sync the only input-changing commit on
this component's data path is **`6f35b13a`** ("optimizer iter-005:
iter005-prop001-scatter-gather"), which rewrote `batch_lookup`'s cold (BlockDevice)
promotion path. All prior sweeps' findings (store backpressure + drop-on-full
FR-060; Check→Pin graceful degrade FR-024/FR-039; per-device streams FR-037/FR-052;
coalesced per-object H2D FR-019) were re-verified against the current source and
remain **aligned** — see "Regression re-verification" below.

## Detailed Findings

### 1. `batch_lookup` cold path: per-drive threads + `ColdReadPool` → single-thread multi-drive scatter-gather — severity: major — RESOLVED BY BACKFILL (code authoritative)

- Commit: `6f35b13a`.
- New shipped code (`src/pipeline.rs`):
  - `pub struct DriveWork<'a> { channels: &ClientChannels, drive: &dyn IBlockDevice, jobs: &[ColdReadJob] }` (`src/pipeline.rs:746`).
  - `pub unsafe fn scatter_gather_multi_drive_zero_copy(gpu, streams: &[GpuStream; 2], drive_works: &[DriveWork], chunk_size, max_queue_depth, metrics) -> Vec<Vec<Result<(), DispatcherError>>>` (`src/pipeline.rs:766`). Fans NVMe reads out to all drives and fans completions in via **one multiplexed poll loop on the caller thread**; distributes `max_queue_depth` proportionally per drive by segment count (`src/pipeline.rs:848`); preserves coalesced per-object H2D and per-object completion routing.
- Call site (`src/lib.rs`, `batch_lookup` cold path):
  - Group by drive via `Self::drive_index(entry.key, num_drives)` (`src/lib.rs:2341`), which is a **splitmix64 finalizer mod `num_drives`** (`src/lib.rs:494-503`), *not* the raw `key % num_drives` the pre-sync FR-039(4) claimed.
  - Per-drive cold-prep builds `pipeline::ColdReadJob`s via `evict_and_insert`; `AllocationFailed` entries defer to the staging post-pass (`src/lib.rs:2385-2411`).
  - Checks out **one** `ChannelLease` per active drive from that drive's `ChannelPool` (`src/lib.rs:2468-2481`), builds one `DriveWork` per drive, and calls `scatter_gather_multi_drive_zero_copy` **once** on the caller thread (`src/lib.rs:2492-2501`). No `std::thread::scope`, no `MAX_QUEUES_PER_DRIVE`, no `pipelined_multi_object_zero_copy` per thread.
  - Uses the per-device pipeline stream **pair** `dev_streams.pipe[0]/[1]` (`src/lib.rs:2431-2461`), falling back to temporary created streams.
  - Staging post-pass unchanged in intent — `serve_cold_staged` one lease at a time (`src/lib.rs:2553-2566`).
- Spec (pre-sync) described the superseded model: FR-039(4)/(5), FR-019 ("`max_queue_depth=128` per thread"), User Story 11 narrative + scenarios 1 & 3 ("per-drive thread groups", "`MAX_QUEUES_PER_DRIVE` … threads per drive", "single thread per drive"), Key Entities "Pipelined Reader", and the Session Q&A all described spawning per-drive threads each running `pipelined_multi_object_zero_copy`.
- Why code is authoritative: `6f35b13a` is the shipped throughput optimization on `evolve-throughput`; the scatter-gather loop is what runs today and the per-drive-thread model no longer exists in source. Direction confirmed with the user: **backfill spec → scatter-gather.**
- Backfill applied to `spec.md`:
  - **New FR-061** — specifies `scatter_gather_multi_drive_zero_copy` + `DriveWork`: single caller-thread multiplexed loop, proportional `max_queue_depth` split by segment count, coalesced per-object H2D on the per-device pipe stream pair, per-drive drain-all-on-error (FR-054), results indexed by drive then job.
  - **FR-039** steps (4)/(5) rewritten to the checkout-one-channel-per-drive + single scatter-gather model; `key % num_drives` corrected to the splitmix64-hash-mod-`num_drives` that `drive_index` computes.
  - **FR-019** last sentences: `promote_and_serve` (single-entry) still uses `pipelined_multi_object_zero_copy`; `batch_lookup` now uses `scatter_gather_multi_drive_zero_copy`.
  - **FR-052** — `cold_pool::ColdReadRequest { gpu_device }` device-selection sentence replaced with the caller-thread batch-device → pipe-stream-pair path.
  - **User Story 11** — narrative + acceptance scenarios 1 & 3 rewritten to the scatter-gather model (no threads, proportional queue-depth share).
  - **Key Entities "Pipelined Reader"** + **Session Q&A** — scatter-gather listed as the primary batch cold variant.
  - New **Last Synced: 2026-09-21** metadata line.

### 2. Unused `ColdReadPool` scaffolding — severity: moderate (dead code) — RESOLVED BY DELETION (user-directed)

- After `6f35b13a` routed the batch cold path through scatter-gather, the `ColdReadPool` (module `src/cold_pool.rs`: `ColdReadPool`, `ColdReadRequest`, `Drop`) was still **constructed** in `initialize()` and **torn down** in `shutdown()` but was **never submitted to** — the batch cold path builds `DriveWork` and calls `scatter_gather_multi_drive_zero_copy` directly. Verified: no `submit`/enqueue call to the pool remained in `batch_lookup` or elsewhere in `src/lib.rs`.
- FR-044 (pre-sync) still *required* a `ColdReadPool` ("The dispatcher MUST provide a `ColdReadPool` … `batch_lookup` dispatches cold entries to the pool"). This was drift: a load-bearing requirement whose subject was dead in the implementation.
- User decision (interactive step): **"Remove ColdRealPool redundant code."** — i.e. delete the dead scaffolding rather than merely document it.
- Code change applied (`src/lib.rs`, `src/cold_pool.rs`):
  - Removed `pub mod cold_pool;`.
  - Removed the `cold_pool: Mutex<Option<cold_pool::ColdReadPool>>` field from the `define_component!` `fields: {}` block.
  - Removed the `initialize()` construction block (`ColdReadPool::new(...)`, `COLD_POOL_QUEUES_PER_DRIVE`).
  - Removed the `shutdown()` teardown block (`pool.shutdown()`).
  - `git rm src/cold_pool.rs`.
  - Fixed the resulting positional-constructor arity: the `define_component!`-generated `DispatcherComponent::new(...)` dropped one field, so all 23 test call sites had one `Mutex::new(None)` (the `cold_pool` slot) removed via a uniform `replace_all` edit.
- Spec change: **FR-044 marked `~~REMOVED~~`** (superseded 2026-09-21, `6f35b13a`) with a pointer to FR-034 (per-drive `ChannelPool`) + FR-061 (scatter-gather) for cold-read resource management. FR-052's `cold_pool::ColdReadRequest` reference removed (see Finding 1).
- Verification: `grep -rn 'cold_pool|ColdReadPool|ColdReadRequest|COLD_POOL' components/dispatcher/src` → none. `cargo check -p dispatcher --tests` → clean (arity break resolved, no other references). **Note:** `components/dispatcher-p2p` has a *separate* `P2pColdReadPool` that is still actively used — it was deliberately **not** touched (out of scope).
- Stale source comments corrected in the same pass (no behavior change): module threading-model doc (`src/lib.rs:46`), the cold-promotion comment (`src/lib.rs:2291`), and the staging post-pass comment's `pool_guard` reference (`src/lib.rs:2548`).

## Regression re-verification (prior sweeps still aligned)

- **FR-060** (store backpressure + drop-on-full): `reserve_memory` retry loop and `populate`/`batch_populate` unconditional drop-on-full intact.
- **FR-024 / FR-039(1)/(3)** (Check→Pin graceful degrade, `1d55b9c2`): skip-not-drop eviction and the removal of the single-entry inline `promote_and_serve` fast path intact; single-key cold still takes the pooled (now scatter-gather) path.
- **FR-034** (per-drive `ChannelPool` + RAII `ChannelLease`, completion-drain on checkout): now the *sole* cold-read channel manager after `ColdReadPool` deletion; scatter-gather checks out exactly one lease per active drive.
- **FR-037 / FR-052** (per-device warm/store streams + pipeline pair): scatter-gather consumes the per-device pipe pair; warm/store unchanged.
- **FR-053** (cold-load staging fallback) and **FR-054** (drain-all-on-error, no early break): staging post-pass and per-drive drain semantics preserved in the scatter-gather loop.

## Not Implemented
None.

## Unspecced Code
None after this sweep. `scatter_gather_multi_drive_zero_copy` and `DriveWork` are now specified by FR-061; the retired `ColdReadPool` is deleted.

## Recommendations
1. Commit this `drift-report.md` (with the freshness stamp above) together with the
   `spec.md` backfill, the `src/lib.rs` edits, and the `src/cold_pool.rs` deletion it
   certifies, so the CI Spec-Sync Gate sees a fresh, matching report.
2. Follow-up (carried over from earlier sweeps) — ✅ **RESOLVED**: the two stale
   transport-example source comments in `copy_gpu_to_memory_async` were removed in
   commit `6d7ba234` ("revert dispatcher GPU experiments to specified stream
   model"). Verified this sweep: `grep -rin grpc components/dispatcher/src` returns
   nothing, and the canonical spec's FR-040 / FR-042 already describe the shm-queue
   control transport (`2026-08-31` sweep). `align-tasks.md` T1 is closed. No
   dispatcher source or spec references to the removed control transport remain;
   residual mentions live only in dated `.specify/sync/` changelog/backup records
   and unrelated `certus-server` gRPC-server specs, which are left intact as
   historical / legitimate.

---

# Appended sweep — feature 002 Phase 1 (2026-09-28)

Generated: 2026-09-28
Component: `components/dispatcher` (+ `lib/shmq-dispatcher`, `apps/certus-server-yaml`)
Trigger: feature 002 Phase 1 — lookup classification fix, remote-lookup counters, FR-024 accounting
Branch: `certus-lookup-observability`

## THE METHOD USED HERE IS NOT THE ONE THAT WORKED ON THE WORKLOAD GENERATOR

The generator's sweep counted each requirement id across `crates/**/*.rs` and treated an
uncited requirement as a signal, because that code cites requirements densely. **That
method is invalid in this component and would have produced a garbage report.** Measured:

| | dispatcher | workload-generator |
|---|---|---|
| requirements defined | 120 (78 + 42) | 99 |
| distinct ids cited in code | **4** | 75+ |

Four citations against 120 requirements. Reporting "116 not implemented" would have been
the output of the method, and it would have been false. This sweep therefore checks
**meaning**, scoped to what Phase 1 could have falsified, and says so rather than
implying whole-spec coverage.

## Summary

| Category | Count |
|----------|-------|
| Specs analysed | 2 |
| Requirements checked (scoped) | 11 |
| ✓ Aligned after fixes | 8 |
| ⚠️ Drifted — **FIXED in this pass** | 3 |
| ✗ Not implemented | 0 in scope |
| 🆕 Unspecced surface | 1 (pre-existing, structural — see below) |

## Findings

### 1. ⚠️ MAJOR — FR-058 enumerated six counters; the struct has eight

`spec.md` FR-058 (001): "`TierEventStats` is a `Copy` struct of **six** `u64` fields",
then names them. `components/interfaces/src/idispatcher.rs` now has **eight** —
`remote_lookup_hits` and `remote_lookup_misses` were added by `9416c997`.

**This is drift I introduced and then missed.** Phase 1 updated FR-011 and
`contracts/errors.md` for the classification change, but not the one requirement that
*enumerates these very counters*. The precedent was already in the file and I did not
follow it: the 2026-09-09 sync updated FR-058 from four fields to six when
store-backpressure added two.

**FIXED**: FR-058 now says eight and documents both, including the accounting rule that a
`TransportError` is neither hit nor miss, and that zeroes mean "no remote traffic" rather
than "remote lookup unwired".

**SC-017 also fixed**, because it covered FR-058 and would otherwise silently
under-verify it: a populate/lookup/evict cycle does not exercise the remote counters at
all, so they needed their own criterion (must not move on a purely local hit; hits +
misses equals keys forwarded less transport failures).

### 2. ⚠️ MODERATE — `contracts/idispatcher.md` claimed the interface change is what makes FR-024 enforceable

`contracts/idispatcher.md:51` said `served_by` being meaningful on failure paths "is what
makes 'every request lands in exactly one bucket' enforceable."

**Phase 1 disproved this.** FR-024's aggregate identity is now enforced server-side by a
pure tally in `shmq-dispatcher`, with **no interface change at all**. The claim tied a
requirement to a design that turned out not to be necessary for it.

**FIXED**: the contract now claims what `LookupOutcome` actually adds — per-key
attribution, which a tally cannot recover — and explicitly warns against re-arguing the
identity as a justification for the change. The argument for Phase 2 has to be
attribution, or it is not an argument.

### 3. ⚠️ MODERATE — the same contract's "Migration: compiler-enforced" was unqualified

True of the return-type widening it was written about; **false as a blanket property**.
Adding two fields to `TierEventStats` compiled with zero errors because the other three
implementors use `..Default::default()`, so a future implementor can silently
under-report.

**FIXED**: scoped to the return-type change, with the struct-field counter-example
recorded as measured rather than predicted.

### ✓ Aligned (verified, not assumed)

- **FR-011 + `contracts/errors.md`** — `RemoteLookupError::NotFound` → `KeyNotFound`,
  `TransportError` → `IoError`. Matches `dispatcher/src/lib.rs` exactly; both tables
  updated in the same commit as the behaviour change.
- **FR-024** — hits + misses + errors == entries requested, enforced by
  `Translator::tally_lookup` and tested as the identity, mutation-verified.
- **FR-026** — attribution does not depend on draining the eviction stream; the counters
  are polled via `tier_event_stats()`, which does not touch that stream.
- **FR-030/031** — per-component documentation obligations met: 001 for the behaviour
  change, definition-site docs in `shmq-dispatcher` (which owns no `specs/`).
- **FR-025, FR-027, FR-028, FR-029** — correctly **not** claimed by Phase 1; recorded as
  awaiting later phases in 002's new coverage table rather than left to inference.

### 🆕 Unspecced surface — the Prometheus endpoint has no owning spec (PRE-EXISTING)

`serve_metrics` renders 16 `certus_*` metrics. **No requirement in any spec names any of
them**, including the three Phase 1 added (`certus_lookup_errors_total`,
`certus_remote_lookup_hits_total`, `certus_remote_lookup_misses_total`).

**Not attributed to this change.** `apps/certus-server-yaml` has no `specs/` and no
`.specify/` at all, so it cannot own a feature, and the same gap already covers
`certus_populates_total`, the memory-tier and the NVMe metrics. Phase 1 made an existing
structural gap two metrics wider; it did not create it.

Worth a decision, not a silent backfill: **metric names are an external contract** — a
scraper or dashboard breaks if they change — and right now nothing in the repo pins them.
Options are to give the server app a spec, or to name the Prometheus surface in the
dispatcher spec that produces the counters. Both are choices for the user.

## Inter-spec conflicts

None found between 001 and 002 in the checked scope. The known **seven-value taxonomy
versus five-value wire projection** tension is internal to 002, deliberately recorded
there, and unresolved pending sign-off — a declared open decision, not drift.

## Recommendations

1. **Commit findings 1–3** (done in this pass) — all three are documentation catching up
   to shipped code, no behaviour change.
2. **Decide who owns the `/metrics` name surface** before Phase 3 widens it further.
3. **Do not run the citation-count method on this component again** without stating that
   it does not apply; record the 4-of-120 measurement so the next sweep starts from it.
