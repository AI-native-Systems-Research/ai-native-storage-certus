---
spec_sync_component: block-device-kernel
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-23T18:00:34Z
spec_sync_git_commit: d7a24d58
spec_sync_inputs_sha256: aa8be9f9d6bf4978be0cf8087db90c5de3fa6b75624c53145fa5018f1e31f2b4
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Re-stamp 2026-10-23 (branch `unstable`, commit `d7a24d58` "Fix workspace build warnings" plus the spec-sync follow-up).** That commit only fixed warnings under the workspace lint posture: it added `Debug` impls, narrowed unreachable `pub` items to `pub(crate)`, wrapped `unsafe fn` bodies in explicit `unsafe {}` blocks, and the follow-up ran `rustfmt`. It also touched `components/interfaces` (a `Debug` impl for `Command`), which changes every component's digest. This component's own `src/` was in that commit. Every changed item was checked: none is part of a provided interface, none is named as public by the spec, and no behaviour changed. No spec citation into this component's source moved. All citations were re-verified by diff-mapping each base line to its new position. Status stays `clean`.

> **Re-stamp 2026-10-21 (transitive, `components/interfaces` changed on branch `fix/dispatcher-remove-clear-mt`).** Only `components/interfaces` changed for this component: `IDispatcher` doc comments (`flush_to_ssd` errors, `clear_memory_tier`, `batch_lookup`) and the interfaces spec (FR-008, FR-009, FR-017, FR-018, FR-018a, FR-021, FR-023, Overview, NFR-002, Implementation Notes; text only, no type or signature changed). This component uses some of the named items (4 files), but no type, signature or behaviour of them changed. This component's last full sync below stands; no re-analysis was needed.

> **Re-stamp 2026-10-14 (transitive, merge of unstable 6f5743cc into this branch).** Only `components/interfaces` changed for this component (`DispatcherConfig::cold_staging_buf_bytes` default 4 -> 16 MiB, #550); `block-device-kernel` does not use it (grep over its src/ and specs/: 0 hits). This branch's full sync below stands; no re-analysis was needed.

> **Re-stamp 2026-10-14 (transitive: `components/interfaces` changed, branch `fix/cold-staging-buf-16mib`).** The spec-sync digest folds `components/interfaces/{src,specs}` into every component's hash, so the single interface change on this branch — `DispatcherConfig::cold_staging_buf_bytes` default 4 MiB → 16 MiB (commit `8e12e49f`) and the matching correction to interfaces FR-027 — invalidates every component's stamp at once; this is the design, not a defect. **No code re-analysis was performed for this component, and none was needed:** its own `src/**` and `specs/**` are unchanged on this branch, and it does not reference `cold_staging_buf_bytes` (consumed only by the dispatcher's host-DRAM cold-staging pool). Digest refreshed to the recomputed value.

> **Apply 2026-10-14 (branch fix/block-device-kernel-timeout-cancel, approved by Cornel).** All 9 proposals in `proposals.json` were approved. **Applied to `specs/001-block-device-kernel/spec.md`:** `bdk-2026-10-14-01` (NFR-006: `on_idle()` is a park hint and is also `true` while `pending_cancels` is non-empty; the same wording went into `plan.md:75`), `bdk-2026-10-14-02` (FR-024: shutdown ends the loop via `ActorHandle::deactivate()`; the sentence "Async operations still in flight at shutdown receive no completion." was **dropped** per the approval note), `bdk-2026-10-14-03` (async-timeout edge case: the cancel is queued and never dropped, and may be deferred), `bdk-2026-10-14-04` (`KernelHandler` key entity), `bdk-2026-10-14-05` (bit-63 `user_data` note; no fsync markers) and `bdk-2026-10-14-06` (SUPERSEDE: the old `on_idle()` implementation note was replaced, pointing to NFR-006/FR-024). The header Last Synced was updated. **Became ALIGN tasks in `align-tasks.md` (no code changed):** `bdk-2026-10-14-07` (FR-012, `handle_abort` drops its cancel on a full SQ), `bdk-2026-10-14-08` (FR-025/FR-002, `DisconnectClient` never sent), `bdk-2026-10-14-09` (NFR-004, five missing `// SAFETY:` comments). **Stays open:** what happens to in-flight async ops at shutdown (from the `bdk-2026-10-14-02` approval note; `tasks.md` T011), recorded as an open item in `align-tasks.md`; plus the not-proposed buffer-lifetime spec gap. Status remains **drift** until the ALIGN tasks land. Backups: `backups/20261014T181542Z/`. Details: `apply-report.{md,json}`. The body below is the pre-apply analysis and was not edited.
>
> **Stamp: clean.** Every drift item of this analysis is resolved (spec updates applied with Cornel's approval) or tracked in `align-tasks.md` (ALIGN tasks and open items), as in earlier `clean` stamps of this and other components (e.g. dispatcher-p2p).

> **Sync 2026-10-14 (branch fix/block-device-kernel-timeout-cancel) — full re-analysis, every FR/NFR/SC/Key Entity re-verified against HEAD `5d123390` (base `origin/unstable` `a052af01`).** The fix (`1e6d0b3f`, test skip `a2bd2c45`) is confirmed correct against the spec: `check_timeouts` (`src/actor.rs:821-848`) still sends `Completion::Timeout` at once (`:843`) and now queues the cancel key (`:844`); `submit_pending_cancels` (`:856-875`) pushes, on a full SQ calls `submit()` and retries once (`:864-868`), and leaves whatever still does not fit queued for the next cycle (`:869-871`); the cancel queue is retried even when `inflight` is empty (`:822-825`). The timeout edge case (`spec.md:103`) and SC-005 (`spec.md:195`) hold. Result: 51 numbered items checked (27 FR, 8 NFR, 9 SC, 7 Key Entities), **45 aligned, 6 drifted, 0 not implemented**, plus 3 drifted non-numbered statements (edge case `spec.md:103`, implementation notes `spec.md:206`, `:207`). **New finding not in earlier reports:** the framework's `on_idle()` return value only means "did work, don't park" (`lib/component-framework/crates/component-core/src/actor.rs:83-92`; `on_idle` called at `:661`, park at `:678`, loop exits only on `ChannelError::Closed` at `:684`). `false` never ends the actor loop. The loop ends only when `shutdown()` → `ActorHandle::deactivate()` closes the channel (`src/lib.rs:186-192`, core `actor.rs:218-226`). So FR-024, NFR-006 and the implementation note `spec.md:207` describe the wrong mechanism, and earlier sweeps marked them aligned without checking. Also new: `ControlMessage::DisconnectClient` is never sent anywhere in the crate, and `IBlockDevice` has no disconnect method, so the session/backlog clean-up that FR-025 and `spec.md:104` rely on never happens (`src/actor.rs:925-930` is dead code). Missing `// SAFETY:` comments at `src/config.rs:87`, `:179` and in the benches (NFR-004). Still present from before: `handle_abort` drops its `AsyncCancel` on a full SQ (`src/actor.rs:667`, FR-012). Not run: `cargo test` (the command needed approval in this session and was not run); the test status below comes from reading the code only. Verdict: **drift** — 9 proposals (5 BACKFILL, 3 ALIGN, 1 SUPERSEDE), all `approved: false`.

> **Sync 2026-10-13 (branch fix/block-device-kernel-timeout-cancel, fix: a timed-out op's `AsyncCancel` is queued and retried instead of dropped when the SQ is full).** Scope: `src/actor.rs` (`check_timeouts` :821, new `submit_pending_cancels` :856, `on_idle` :946, `pending_cancels` :113) + new test `src/repair_test_timeout_cancel.rs`. Findings: the fix now satisfies the timeout edge case (`spec.md:103`) and SC-005 (`spec.md:195`); `Completion::Timeout` still sent immediately; no new error variant or return type. New undescribed behaviour: (1) `on_idle` stays `true` while `pending_cancels` is non-empty (`actor.rs:946`), beyond NFR-006's clients/inflight condition (`spec.md:149`); (2) the cancel may be deferred across poll cycles, which `spec.md:103` does not say; (3) the `KernelHandler` entity (`spec.md:156`) omits the cancel queue. Pre-existing, not introduced, outside the changed functions: `handle_abort` (`actor.rs:667`) still drops its `AsyncCancel` on a full SQ, against FR-012 (`spec.md:123`). Verdict: drift (needs a small spec backfill for NFR-006, `spec.md:103`, and Key Entities).

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

> **Re-stamp 2026-09-15 (workspace `cargo fmt` sweep; no drift).** Commit `f9bcd965` ("Add shmq RESERVE batch shared-deadline regression test") ran `cargo fmt` across the whole workspace, reflowing this component's `src/*.rs` (multi-line ↔ single-line argument lists and struct literals, import reordering). `git diff -w` confirms no token-level logic, signature, or contract change — the only substantive addition in that commit is a regression test in `lib/shmq-dispatcher/src/translate.rs`, which is outside this component and outside the spec-sync gate's `components/` scope. The formatting moved this component's `spec_sync_inputs_sha256`, but its spec↔implementation alignment is unchanged. Report body below stands unchanged; drift status remains `clean`. Digest recomputed over a clean tree matching CI.

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

# Spec Drift Report — block-device-kernel

Generated: 2026-10-14T17:31:02Z
Project: block-device-kernel (`components/block-device-kernel`)
Spec: `specs/001-block-device-kernel/spec.md` (Status: Backfilled; header "Last Synced: 2026-09-03", `spec.md:11`)
Branch / HEAD: `fix/block-device-kernel-timeout-cancel` @ `5d123390` (base `origin/unstable` @ `a052af01`)
Mode: BATCH, read-only. Only `.specify/sync/` was written. No spec or source edited.

All `file:line` references are to HEAD and relative to `components/block-device-kernel/` unless prefixed.
"core `actor.rs`" means `lib/component-framework/crates/component-core/src/actor.rs`.

## What changed on this branch (`git diff origin/unstable.. -- src`)

| Change | Location |
|--------|----------|
| New field `pending_cancels: VecDeque<u64>` (keys of timed-out ops whose cancel is not yet pushed) | `src/actor.rs:110-113`, initialised `:135`, `:156` |
| `check_timeouts`: sends `Completion::Timeout` and queues the key, instead of pushing the cancel inline and ignoring a push failure | `src/actor.rs:841-847` |
| `check_timeouts`: with an empty `inflight` map it still drains the cancel queue | `src/actor.rs:822-825` |
| New `submit_pending_cancels`: push oldest-first; on a full SQ, `submit()` then retry the push once; stop and keep the rest queued if it still fails; final `submit()` | `src/actor.rs:850-875` |
| `on_idle` also returns `true` while `pending_cancels` is non-empty | `src/actor.rs:946` |
| New unit test `timeout_with_full_submission_queue_still_submits_async_cancel` (fills the SQ with NOPs whose `user_data` has bit 63 set, calls `check_timeouts`, waits for a CQE with `user_data == 0`); skips itself by returning early when `IoUring::new` fails | `src/repair_test_timeout_cancel.rs:55-115`, skip `:30-38`; wired in `src/actor.rs:954-956` |

Before the fix (`origin/unstable`) the cancel push result was discarded (`let _ = self.ring.submission().push(&cancel_sqe)`), so on a full SQ the cancel was lost and the timed-out kernel op kept running, untracked. The fix removes that case for timeouts. **It does not touch `handle_abort`**, which still has the same drop (see FR-012).

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 (`001-block-device-kernel`) |
| Requirements Checked | 51 numbered (27 FR + 8 NFR + 9 SC + 7 Key Entities) + 12 edge cases + 7 implementation notes + the US1/US2 acceptance scenarios |
| ✓ Aligned | 45 of 51 (88%) |
| ⚠️ Drifted | 6 of 51 (12%): FR-012, FR-024, FR-025, NFR-004, NFR-006, Key Entity `KernelHandler`; plus 3 non-numbered statements: edge case `spec.md:103`, impl notes `spec.md:206`, `spec.md:207` |
| ✗ Not Implemented | 0 (0%) |
| 🆕 Unspecced Code | 0 standalone features (the new cancel queue is behaviour inside existing requirements and is handled as backfills of NFR-006, `spec.md:103`, and Key Entities) |

Introduced by this branch: 3 of the 9 drift items (NFR-006 condition, `spec.md:103` deferral, `KernelHandler` field). These are small, and all three are spec wording that is now incomplete, not code defects. The other 6 were already there before the branch. 3 of those 6 (FR-024, NFR-006 semantics / `spec.md:207`, FR-025 unreachable disconnect) are **newly found** in this sweep; earlier reports marked them aligned.

**Verification runs**: none. `cargo test -p block-device-kernel` (with `CARGO_TARGET_DIR` outside the repo) needed approval in this session and was not run. Test status below comes from reading the code only: 13 unit tests (8 in `src/lib.rs:353-409`, 4 in `src/config.rs:202-226`, 1 in `src/repair_test_timeout_cancel.rs:55`), 2 non-ignored integration tests (`tests/integration.rs:74`, `:83`), 13 `#[ignore]` hardware integration tests. Note: the 2026-09-03 body said "2 unit tests per set"; the real count is 13.

## Detailed Findings

### Spec: 001-block-device-kernel — Block Device Kernel Component

#### Drifted ⚠️

1. **NFR-006** — severity: moderate. Part was introduced by this branch (the new condition); the wrong meaning was already there.
   - Spec (`spec.md:149`): "The actor `on_idle()` remains active (returns `true`) while clients are connected or inflight operations exist."
   - Code: `on_idle` returns `!clients.is_empty() || !inflight.is_empty() || !pending_cancels.is_empty()` (`src/actor.rs:946`). So it is also `true` while timed-out cancels are still queued, which the spec does not list. Also, the return value does not control whether the actor stays "active". The framework reads it as "useful work was done" (core `actor.rs:83-92`). `false` only increases `idle_count`, and after `PARK_THRESHOLD` (10,000,000) idle calls the thread parks for 10 ms (core `actor.rs:661-681`). It never ends the loop. In practice, returning `true` whenever a client is connected means the actor busy-polls and never parks while any client is attached.
   - Location: `src/actor.rs:946`; core `actor.rs:83-92`, `:661-681`.

2. **FR-024** — severity: moderate. Already there before this branch; newly found.
   - Spec (`spec.md:135`): "Component MUST support graceful shutdown via `ControlMessage::Shutdown` which causes `on_idle()` to return `false`, terminating the actor loop."
   - Code: `Shutdown` sets `shutdown_requested` (`src/actor.rs:931-933`). After that, `on_idle` returns `false` straight away without polling clients, harvesting completions or checking timeouts (`:938-940`). But returning `false` does not end the loop (see NFR-006). What ends it is `BlockDeviceKernelComponent::shutdown()`: it sends `Shutdown` and then calls `ActorHandle::deactivate()` (`src/lib.rs:186-192`). That closes the channel and joins the thread (core `actor.rs:218-226`), and the loop breaks on `ChannelError::Closed` (core `actor.rs:684-697`). In-flight async ops get no completion at shutdown. Neither the spec nor the code says what happens to them.
   - Location: `src/actor.rs:931-940`; `src/lib.rs:179-195`.

3. **FR-025** (also affects FR-002 and edge case `spec.md:104`) — severity: moderate. Already there before this branch; newly found.
   - Spec (`spec.md:104`, `:136`): the backlog is unbounded "until either the ring frees up or `ControlMessage::DisconnectClient` removes the session (dropping the backlog with it)". FR-002 (`spec.md:113`) requires debug logging of "client connect/disconnect operations".
   - Code: the delivery/backlog part is aligned (`src/actor.rs:38`, `:55-60`, `:65-76`, `:895-897`). But nothing in the crate ever builds `ControlMessage::DisconnectClient`. The only mentions are the enum variant (`src/actor.rs:82`) and its handler (`:925-930`). `IBlockDevice` has no disconnect method (`components/interfaces/src/iblock_device.rs:557-589`). So a client that drops its channels keeps its `ClientSession` and an ever-growing `pending` backlog until component shutdown. The debug log on disconnect (`src/actor.rs:927`) is dead code.
   - Location: `src/actor.rs:82`, `:925-930`; `src/lib.rs:216-256` (connect only).

4. **FR-012** — severity: moderate. Already there before this branch; listed as residual in the 2026-10-13 note.
   - Spec (`spec.md:123`): "Component MUST support `AbortOp` by submitting an `AsyncCancel` SQE to io_uring and sending an `AbortAck` completion."
   - Code: `handle_abort` discards the push result (`let _ = self.ring.submission().push(&cancel_sqe);`, `src/actor.rs:667`). It removes the op from `inflight` (`:671`) and sends `AbortAck` (`:672`) either way. On a full SQ no cancel is ever submitted, yet the client is told the abort happened. The kernel op keeps running untracked and its CQE is dropped silently (no `inflight` entry, `src/actor.rs:777`). This is the same defect the branch fixed for timeouts. The `pending_cancels` mechanism (`:856-875`) is already there to reuse.
   - Location: `src/actor.rs:662-673`.

5. **NFR-004** — severity: minor. Already there before this branch.
   - Spec (`spec.md:147`): "Unsafe code blocks MUST include `// SAFETY:` justification comments."
   - Code: these `unsafe` blocks have no `// SAFETY:` comment: `src/config.rs:87` (`std::mem::zeroed()` for `libc::stat`), `src/config.rs:179` (`libc::close(raw_fd)` on the O_DIRECT-check error path), `benches/throughput.rs:88`, `benches/latency.rs:52`, `benches/latency.rs:76` (`posix_memalign` blocks). Every `unsafe` in `src/actor.rs`, including the two new ones at `:863`, `:867`, is commented.
   - Location: as listed.

6. **Key Entity `KernelHandler`** — severity: minor. Introduced by this branch.
   - Spec (`spec.md:156`): "Owns the file descriptor (`OwnedFd`), `DeviceConfig`, `IoUring` ring, client sessions map, inflight operations map, and optional logger."
   - Code: the struct also holds `pending_cancels: VecDeque<u64>` (new, `src/actor.rs:113`), plus `next_handle` (`:109`), `shutdown_requested` (`:115`) and the feature-gated `telemetry` (`:116-117`). Only `pending_cancels` matters for behaviour (it is the new timeout-cancel guarantee).
   - Location: `src/actor.rs:103-118`.

7. **Edge case "Async operation timeout"** (non-numbered) — severity: minor. Introduced by this branch (made stricter, not broken).
   - Spec (`spec.md:103`): "Timed-out operations produce `Completion::Timeout` and an `AsyncCancel` is submitted to io_uring."
   - Code: `Timeout` is delivered at once (`src/actor.rs:843`). The cancel is guaranteed but may be **deferred**: if the SQ is still full after one `submit()` + retry, the key stays in `pending_cancels` and is retried every `on_idle` tick (`:856-875`, `:822-825`), oldest first, until it fits. It is never dropped. The spec does not say the cancel can come after `Timeout`, possibly several poll cycles later, or that it is never lost. A cancel CQE has `user_data == 0` and is ignored (no handle 0 is ever issued, `next_handle` starts at 1, `:134`).
   - Location: `src/actor.rs:821-875`.

8. **Implementation note "fsync markers"** (non-numbered) — severity: minor. Already there before this branch.
   - Spec (`spec.md:206`): "CQEs with `user_data & (1 << 63) != 0` are treated as internal fsync markers and silently ignored."
   - Code: the filter exists (`src/actor.rs:700`, `:769`), but no code path ever sets bit 63 or submits an fsync SQE. The crate has no fsync at all (FR-004 relies on `O_DSYNC`). The only user of the bit is the new test, which uses it to mark its filler NOPs so the actor ignores them (`src/repair_test_timeout_cancel.rs:9-10`, `:82`). The filter reserves bit 63 for internal SQEs; "fsync markers" is a leftover description.
   - Location: `src/actor.rs:700`, `:769`.

9. **Implementation note on `on_idle`** (non-numbered) — severity: moderate (same root cause as NFR-006/FR-024). It also contradicts the spec's own text.
   - Spec (`spec.md:207`): "The `on_idle()` actor method returns `false` only when shutdown is requested; it returns `true` while clients or inflight ops exist to keep the actor thread alive."
   - Code: `on_idle` also returns `false` whenever there are no clients, no inflight ops and no pending cancels (`src/actor.rs:946`). So "only when shutdown is requested" is wrong. "to keep the actor thread alive" is also wrong: the value is a park hint (core `actor.rs:661-681`). `plan.md:75` repeats the same claim ("Actor stays alive while clients connected").
   - Location: `src/actor.rs:937-947`.

#### Aligned ✓ (re-verified at HEAD)

| Req | Evidence |
|-----|----------|
| FR-001 IBlockDevice + IBlockDeviceAdmin | `impl IBlockDeviceAdmin` `src/lib.rs:198-214`; `impl IBlockDevice` `src/lib.rs:216-335` |
| FR-002 ILogger receptacle; info on init; debug on connect/disconnect; no warn | receptacle `src/lib.rs:59-61`; info `src/lib.rs:116-123`, `:171-173`; debug connect `src/lib.rs:228-230`, `src/actor.rs:920-922`; no `warn(` anywhere in the crate. *Caveat:* the disconnect debug log `src/actor.rs:926-928` can never run (see FR-025 drift). |
| FR-003 `define_component!` provides/receptacles | `src/lib.rs:55-71` |
| FR-004 `O_DIRECT \| O_DSYNC` | `src/config.rs:169` |
| FR-005 io_uring only, no pread/pwrite | only `opcode::Read` `src/actor.rs:302`, `:458`; `opcode::Write` `:374`, `:532`, `:617`; `opcode::AsyncCancel` `:663`, `:861`; no `pread`/`pwrite` call anywhere in the crate |
| FR-006 S_IFBLK only | `assert_block_device` `src/config.rs:86-114` (mode check `:105`), called from `open_block_device` `:164` and `query_device_size` `:118` |
| FR-007 block size ≥512, pow2; 0 → BLKGETSIZE64 | `src/config.rs:34-39`; auto-detect `:41-50`; ioctl `:129`; multiple-of-block-size check `:146-151` |
| FR-008 sync R/W via SQE + `submit_and_wait(1)` | `handle_read_sync` `src/actor.rs:268-349` (`:327`); `handle_write_sync` `:353-421` (`:399`) |
| FR-009 async R/W, timeout, OpHandle inflight map; tag not propagated | `handle_read_async` `src/actor.rs:423-501` (deadline `:482-486`, insert `:488`); `handle_write_async` `:503-575` (`:556-560`, `:562`); `_tag` unused `:430`, `:510`; completions `tag: 0` `:805`, `:811` |
| FR-010 WriteZeros posix_memalign(512), io_uring, free | `src/actor.rs:577-660`; `posix_memalign` `:598`; free on push-fail `:625`, after completion `:647` |
| FR-011 BatchSubmit sequential recursion | `src/actor.rs:224-228` |
| FR-013 NsProbe single namespace ns_id=1 | `src/actor.rs:675-687` |
| FR-014 NotSupported for NsCreate/NsDelete/NsFormat/ControllerReset | `src/actor.rs:250-263` |
| FR-015 actor; `handle()` + `on_idle()` polls/harvests/checks timeouts | `impl ActorHandler<ControlMessage>` `src/actor.rs:916-948`; `on_idle` body `:942-944` |
| FR-016 per-client SPSC, capacity 64 | `CLIENT_CHANNEL_CAPACITY` `src/lib.rs:50`; `SpscChannel::new` `:232`, `:237` |
| FR-017 LBA bounds with `checked_add` | `validate_lba` `src/actor.rs:169-185` (`checked_add` `:175`, bound `:178`) |
| FR-018 `ns_id == 1` for IO + `sector_size`/`num_sectors` | `src/actor.rs:170`; `src/lib.rs:259`, `:268` |
| FR-019 `posix_fadvise(DONTNEED)` on init | `src/config.rs:190-192` |
| FR-020 O_DIRECT verified via `fcntl(F_GETFL)` | `src/config.rs:177-185` |
| FR-021 telemetry min/max/mean latency, bytes, throughput; FeatureNotEnabled | `src/telemetry.rs:13-103`; real latency at all 5 sites `src/actor.rs:333-334`, `:405-406`, `:651-652`, `:732-733`, `:797-798`; `FeatureNotEnabled` `src/lib.rs:322-327` |
| FR-022 Criterion latency + throughput benches | `Cargo.toml` `[[bench]]` ×2; `benches/latency.rs:17` (command construction), `:38` (sync 4K R/W); `benches/throughput.rs:30`, `:80` (1/8/32/128 blocks) |
| FR-023 admin no-ops | `src/lib.rs:199`, `:201`, `:207`, `:213` |
| FR-026 numa -1, nvme_version string, zeroed rw stats | `src/lib.rs:292-294`, `:296-298`, `:332-334` |
| FR-027 FlushSync validated no-op / InvalidNamespace | `src/actor.rs:235-249` |
| NFR-001 512-aligned buffers via O_DIRECT | fd flags `src/config.rs:169`; WriteZeros 512-aligned `src/actor.rs:598`. Caller buffers are not checked by the component; misalignment shows up as an io_uring `-EINVAL` error completion |
| NFR-002 ring depth 128 | `DEFAULT_RING_DEPTH` `src/lib.rs:53`, used `:128`, `:277` |
| NFR-003 no panic on IO errors | IO errors become `NvmeBlockError` completions (`src/actor.rs:705-709`, `:778-788`, `:719-729`); the only `expect`s are on poisoned `DmaBuffer` mutexes (`:278`, `:298`, `:434`, `:454`), which are not IO errors |
| NFR-005 Send | `unsafe impl Send for KernelHandler` `src/actor.rs:952` (with SAFETY `:950-951`) |
| NFR-007 `Instant::now()` vs per-op deadline | deadlines `src/actor.rs:483`, `:557`; compare `:827-833` |
| NFR-008 Linux ≥5.1 | io_uring only; platform claim, not checkable in code |
| SC-001 read-after-write, O_DSYNC | `tests/integration.rs:91`, `:441` (`#[ignore]`, hardware) |
| SC-002 auto-detect | `tests/integration.rs:65` (`#[ignore]`); `src/config.rs:117-154` |
| SC-003 typed rejections | `src/config.rs:202-218` unit tests; `tests/integration.rs:74`, `:83` (non-ignored), `:251` (`#[ignore]`) |
| SC-004 multi-client | `tests/integration.rs:377` (`#[ignore]`) |
| SC-005 async timeout → `Completion::Timeout` | `src/actor.rs:841-845`. *Coverage gap:* no test checks that `Timeout` is delivered. The new unit test calls `check_timeouts` with an unregistered client 7, so `send_completion` does nothing (`:885`). It checks only that the op leaves `inflight` and that a cancel CQE appears (`src/repair_test_timeout_cancel.rs:91-114`). Tasks T009 (`tasks.md:30`) is still open. |
| SC-006 accurate TelemetrySnapshot | all 5 record sites use real elapsed time (see FR-021); `snapshot()` `src/telemetry.rs:68-102` |
| SC-007 stable benches | `benches/*` (measurement-quality target; cannot be checked statically) |
| SC-008 drop-in IBlockDevice | full surface `src/lib.rs:198-335`; `tests/integration.rs:328` |
| SC-009 unit tests without hardware; integration `#[ignore]` | 13 unit tests need no block device. The new test needs io_uring and returns early with "SKIPPED" when `IoUring::new` fails (`src/repair_test_timeout_cancel.rs:30-38`, `:58-61`), so it counts as passed, not as `#[ignore]`d. 13 `#[ignore]` integration tests. **Not executed in this sweep.** |
| KE `BlockDeviceKernelComponent` | `src/lib.rs:55-71`, version `:57` |
| KE `DeviceConfig` | `src/config.rs:12-83` |
| KE `ClientSession` | `src/actor.rs:26-77` |
| KE `ControlMessage` | `src/actor.rs:80-84` |
| KE `InflightOp` (handle, client_id, deadline, is_read, optional telemetry) | `src/actor.rs:90-100` |
| KE `TelemetryStats` | `src/telemetry.rs:13-20` |

Edge cases re-checked and aligned: non-block path → `NotInitialized` (`src/lib.rs:111-112`, `:125-126`); non-existent path (`src/config.rs:96-102` → same mapping); LBA OOR (`src/actor.rs:178-183`); invalid ns (`:170-174`); block size <512 / non-pow2 (`src/config.rs:34-39`); size not multiple (`:146-151`); SQ full for sync ops → error completion (`src/actor.rs:309-321`, `:381-393`, `:624-636`); `connect_client` before init (`src/lib.rs:222-224`); telemetry without feature (`src/lib.rs:322-327`). Implementation notes `spec.md:203-205`, `:208-209` are aligned (`wait_for_cqe` `src/actor.rs:691-760`; admin no-ops; O_DIRECT check fails init `src/config.rs:178-185`).

#### Not Implemented ✗

None.

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| (none standalone) — the deferred timed-out-cancel queue is behaviour inside existing requirements and is covered by the NFR-006, `spec.md:103` and Key Entities backfills | `src/actor.rs:110-113`, `:850-875` | ~30 | 001 (backfill, not a new spec) |

### Spec gap (no requirement either way; human decision, not a drift item)

- **Buffer lifetime after `Timeout` / `AbortAck`.** `InflightOp` does not keep the caller's `DmaBuffer` `Arc` (`src/actor.rs:90-100`). The `Arc` passed to `handle_read_async` / `handle_write_async` is dropped when the function returns (`:501`, `:575`), while the kernel still has the raw pointer (`:453-461`, `:530-535`). After `Completion::Timeout` (`:843`) or `AbortAck` (`:672`) a client may reasonably think the buffer is free to reuse or drop. But the kernel op may still be running: the cancel can now be delayed several poll cycles (`:869-871`), and `AsyncCancel` cannot stop a request already handed to the device. Freeing or reusing the buffer then risks the kernel writing into freed memory, or reading data that is being changed. The spec says nothing about buffer ownership after these completions. This is not caused by the fix (the window existed before), but the fix makes it slightly wider. Recommend a human decide whether the spec should (a) say that after `Timeout`/`AbortAck` the caller must keep the buffer alive (there is no signal today that says when it is safe), or (b) require `InflightOp` to hold the buffer `Arc` until the kernel's CQE for the original op arrives.

## Inter-Spec Conflicts

- `spec.md:207` contradicts itself: it says `on_idle` returns `false` "only when shutdown is requested" and also that it returns `true` only "while clients or inflight ops exist". When neither is true and no shutdown was requested, it returns `false` (`src/actor.rs:946`).
- `spec.md:135` (FR-024), `:149` (NFR-006), `:207` and `plan.md:75` all read `on_idle()`'s return value as keeping the actor alive or ending it. The framework contract (core `actor.rs:83-92`) says it is only a "did useful work" park hint.
- `spec.md:104` / `:136` (FR-025) rely on `ControlMessage::DisconnectClient`, but no caller exists and the shared `IBlockDevice` interface has no disconnect method (`components/interfaces/src/iblock_device.rs:557-589`).
- Header `spec.md:11` ("Last Synced: 2026-09-03") is out of date compared with this branch's change to timeout/cancel behaviour. Update it when proposals are applied.

## Recommendations

1. Apply the 5 BACKFILL proposals and the 1 SUPERSEDE proposal (NFR-006, FR-024, `spec.md:103`, Key Entities `KernelHandler`, `spec.md:206`, `spec.md:207`). These are spec-text-only changes and describe the code as it is. Also fix `plan.md:75` with the same wording as NFR-006.
2. ALIGN FR-012: send `handle_abort`'s cancel through the existing `pending_cancels` queue (`src/actor.rs:856-875`) so an abort cancel is never dropped either. This is the same bug class the branch fixed and should go in the same series.
3. ALIGN FR-025/FR-002: decide how dead clients are cleaned up (e.g. drop the session when `callback_tx.try_send` returns `Closed`, or add a disconnect path). The `DisconnectClient` handler is dead code today and dropped clients leak memory without limit.
4. ALIGN NFR-004: add the 5 missing `// SAFETY:` comments.
5. Human decision on the buffer-lifetime gap above, before relying on `Timeout`/`AbortAck` to mean "buffer is free".
6. Run `cargo test -p block-device-kernel -- --test-threads 1` (default and `--features telemetry`) on a host where io_uring is enabled, so the new test really runs instead of skipping. Add a test that checks `Completion::Timeout` is delivered to a registered client (SC-005, `tasks.md:30` T009).
