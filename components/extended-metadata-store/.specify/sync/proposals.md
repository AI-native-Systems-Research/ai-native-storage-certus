# Drift Resolution Proposals

Generated: 2026-10-18 (sync note dated 2026-10-14)
Based on: drift-report 2026-10-14 (branch `fix/bundle-dispatcher-ems` @ `4f8b5554`; component source at `4a8fe3e7`)
Mode: batch (no questions). **Every proposal is `approved: false`.** Nothing has been applied.

Paths: `spec.md`, `plan.md` and `tasks.md` mean `components/extended-metadata-store/specs/001-extended-metadata-store/*`.
`src/` and `tests/` are relative to `components/extended-metadata-store/`. Spec line numbers are from the current `spec.md`.

This file replaces the 2026-10-14 proposals from branch `fix/extended-metadata-store-flush`.
Those 14 proposals were all approved and handled (see `apply-report.md`). Their IDs P1–P14 are
**reused here with new meanings**. The earlier ALIGN tasks keep their `ALIGN-EMS-*` IDs.

## Summary

| Resolution Type | Count | Proposals |
|-----------------|-------|-----------|
| Backfill (Code → Spec) | 7 | P1, P2, P3, P4, P5, P12, P14 |
| Align (Spec → Code) | 7 | P6, P7, P8, P9, P10, P11, P13 |
| Human Decision | 0 | none (P6 and P10 contain sub-choices; a recommendation is given) |
| New Specs | 0 | none |
| Supersede | 1 | P15 |
| Remove from Spec | 0 | none |
| **Total** | **15** | |

P8–P11 restate code-side drift that is already tracked (ALIGN-EMS-004..007) and that this branch
did not change. They need no new task; approving them only refreshes the evidence in `align-tasks.md`.

---

### Proposal P1: 001/FR-05 — drop the "Open" clause now that failures propagate

**Direction**: BACKFILL

**Requirement**: FR-05: `force_flush()` ensures durability, mapping trigger errors to `StorageError`.

**Current State**:
- Spec says (`spec.md:166`, status column): "Implemented in the component (`src/lib.rs:221-235`; trigger install `attach_flush_trigger` at `src/lib.rs:131`, `FlushTrigger` alias at `src/lib.rs:72`). **Open:** with the documented `FlushManager::trigger_flush()` trigger, flush failures are not propagated (`src/flush.rs:177`) — see drift-report 2026-10-14 / proposal P1"
- Code does:
  - `trigger_flush` returns `Ok` iff `durable >= ticket`, else `Err(last_error)` (`src/flush.rs:181-185`).
  - `do_flush` advances `durable` only on success and records the error otherwise (`src/flush.rs:260-270`).
  - `force_flush` maps the error to `StorageError` (`src/lib.rs:232`).
  - Test: `force_flush_reports_failed_disk_write` (`src/repair_test_force_flush_error.rs:42-61`). Line `flush.rs:177` is now part of the `wait_while` call.

**Proposed Resolution** — `spec.md:166`, status cell of the FR-05 row:

Old:
> Implemented in the component (`src/lib.rs:221-235`; trigger install `attach_flush_trigger` at `src/lib.rs:131`, `FlushTrigger` alias at `src/lib.rs:72`). **Open:** with the documented `FlushManager::trigger_flush()` trigger, flush failures are not propagated (`src/flush.rs:177`) — see drift-report 2026-10-14 / proposal P1

New:
> Implemented (`src/lib.rs:221-235`; trigger install `attach_flush_trigger` at `src/lib.rs:131`, `FlushTrigger` alias at `src/lib.rs:72`). With the documented `FlushManager::trigger_flush()` trigger, a failed flush is reported: `trigger_flush()` returns the error of the pass that released the call (`src/flush.rs:181-185`, recorded by `do_flush` at `src/flush.rs:260-270`), which `force_flush()` returns as `StorageError`; test `force_flush_reports_failed_disk_write` (`src/repair_test_force_flush_error.rs`). See FR-13 for the ownership caveat of this wiring

The requirement text (middle column) is unchanged. It is now true.

**Rationale**: The code fix `4a8fe3e7` implements exactly what the spec required (ALIGN-EMS-003). The only stale part is the status note. It was accepted by an independent check, as its commit message says, and its test asserts the behaviour.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P2: 001/US6 — list the new tests and retire the "follow-up test" note

**Direction**: BACKFILL

**Requirement**: US6 AC1 ("`force_flush()` triggers an immediate flush and blocks until durable") and its test list.

**Current State**:
- Spec says (`spec.md:154`): "… The `flush_manager_*` tests listed below still drive `FlushManager::trigger_flush()` directly; a follow-up test that installs a trigger and asserts durability (and error propagation) through `force_flush()` is tracked by ALIGN-EMS-001 in `.specify/sync/align-tasks.md` and by proposal P1 in `.specify/sync/proposals.md`."
- Spec test list (`spec.md:146-152`) has no entry for `repair_test_force_flush_error.rs`.
- Code does: `src/repair_test_force_flush_error.rs:19-38` installs `move || mgr.trigger_flush()` via `attach_flush_trigger`.
  - `:42-61` asserts `force_flush()` → `Err(StorageError(_))` and that the write stays dirty when every write fails.
  - `:63-72` asserts `Ok` and `dirty_count() == 0` on a healthy device, and `Ok` again when nothing is dirty.

**Proposed Resolution**:

(a) Insert after `spec.md:152`.

Old: *(nothing)*

New:
> - `force_flush_reports_failed_disk_write`, `force_flush_ok_on_healthy_device` (unit, `src/repair_test_force_flush_error.rs`, `--features testing`): with the documented `FlushManager::trigger_flush()` trigger attached via `attach_flush_trigger`, `force_flush()` returns `StorageError` when the device write fails (and the write stays dirty), and `Ok` on a healthy device (FR-05)

(b) `spec.md:154`, last sentence.

Old:
> The `flush_manager_*` tests listed below still drive `FlushManager::trigger_flush()` directly; a follow-up test that installs a trigger and asserts durability (and error propagation) through `force_flush()` is tracked by ALIGN-EMS-001 in `.specify/sync/align-tasks.md` and by proposal P1 in `.specify/sync/proposals.md`.

New:
> The `flush_manager_*` tests listed above drive `FlushManager::trigger_flush()` directly; `src/repair_test_force_flush_error.rs` drives `force_flush()` through an attached trigger and checks error propagation (mock device). The SSD integration test still obtains durability through the FR-17 API rather than `force_flush()` (ALIGN-EMS-001 in `.specify/sync/align-tasks.md`).

The old text also says "listed below" although the list is above; the new text fixes that.

**Rationale**: The tests exist and pass the acceptance check (commit message of `4a8fe3e7`). The note is now wrong about what is still missing.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P3: 001/Known Gaps (FR-05 entry) — refresh line references and mention failure propagation

**Direction**: BACKFILL

**Requirement**: FR-05 (Known Gaps narrative).

**Current State**:
- Spec says (`spec.md:183`): "… is now **present and merged** in the working tree (`src/lib.rs:201-215`). The component carries an optional type-erased `FlushTrigger` (`pub type FlushTrigger = Box<dyn Fn() -> Result<(), String> + Send + Sync>`, `src/lib.rs:68`), installed by the wiring layer through `attach_flush_trigger` (`src/lib.rs:111`, typically `move || flush_manager.trigger_flush()`). …"
- Code does: `force_flush` is at `src/lib.rs:221-235`, `FlushTrigger` at `src/lib.rs:72`, `attach_flush_trigger` at `src/lib.rs:131`. Failure propagation is at `src/flush.rs:181-185`.

**Proposed Resolution** — `spec.md:183`:

Old:
> is now **present and merged** in the working tree (`src/lib.rs:201-215`). The component carries an optional type-erased `FlushTrigger` (`pub type FlushTrigger = Box<dyn Fn() -> Result<(), String> + Send + Sync>`, `src/lib.rs:68`), installed by the wiring layer through `attach_flush_trigger` (`src/lib.rs:111`, typically `move || flush_manager.trigger_flush()`). `force_flush()` invokes the trigger and blocks until it returns, mapping any error to `ExtendedMetadataStoreError::StorageError`;

New:
> is now **present and merged** in the working tree (`src/lib.rs:221-235`). The component carries an optional type-erased `FlushTrigger` (`pub type FlushTrigger = Box<dyn Fn() -> Result<(), String> + Send + Sync>`, `src/lib.rs:72`), installed by the wiring layer through `attach_flush_trigger` (`src/lib.rs:131`, typically `move || flush_manager.trigger_flush()`). `force_flush()` invokes the trigger and blocks until it returns, mapping any error to `ExtendedMetadataStoreError::StorageError`; since `4a8fe3e7` `FlushManager::trigger_flush()` returns an error when the flush pass serving the call failed (`src/flush.rs:181-185`), so a device or capacity failure reaches the caller (2026-10-14 ALIGN-EMS-003, resolved);

**Rationale**: These are factual corrections of locations that moved, plus the resolved state.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P4: 001/FR-12 — specify the result of a released call; refresh line references

**Direction**: BACKFILL

**Requirement**: FR-12 (ticket coalescing).

**Current State**:
- Spec says (`spec.md:173`): "… No call is ever released by a flush whose snapshot predates the call | Implemented (`src/flush.rs:88-94`, `:152-178`, `:211-213`, `:251-257`)"
- The spec is silent on what a released call returns.
- Code does:
  - A released call returns `Ok` iff some successful pass (or a signalled pass that found nothing dirty) has served its ticket (`durable >= ticket`); otherwise it returns the most recent recorded flush error (`src/flush.rs:181-185`, `:233-238`, `:260-270`).
  - The current locations are `FlushTickets` `:88-97`, `trigger_flush` `:155-186`, `serving` `:219-221`, completion `:260-270`.

**Proposed Resolution** — `spec.md:173`:

Old:
> No call is ever released by a flush whose snapshot predates the call | Implemented (`src/flush.rs:88-94`, `:152-178`, `:211-213`, `:251-257`)

New:
> No call is ever released by a flush whose snapshot predates the call. A released call returns `Ok(())` only if a pass that served its ticket succeeded (or found nothing dirty), so its writes are on disk; otherwise it returns the error of the failed pass, its writes stay dirty, and a later pass (timer or another `trigger_flush()`) retries them | Implemented (`src/flush.rs:88-97`, `:155-186`, `:219-221`, `:233-238`, `:260-270`)

**Rationale**: This is the new behaviour of `4a8fe3e7`, and FR-05 depends on it. Spec text and code would otherwise describe different contracts for `trigger_flush`.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P5: 001/FR-11 and FR-13 — refresh line references

**Direction**: BACKFILL

**Requirement**: FR-11 (timer/threshold triggers), FR-13 (final flush on Drop).

**Current State**:
- Spec says (`spec.md:172`): "Implemented (`src/flush.rs:14-16`, `:59-75`, `:185-206`, `:234`)". Spec says (`spec.md:174`): "Implemented (`src/flush.rs:215-219`, `:263-277`)".
- Code does: the behaviour is unchanged, but `4a8fe3e7` inserted lines. The wait loop is now `src/flush.rs:193-214`, the disarm `:243`, the shutdown pass `:223-227`, and `Drop` `:276-290`.

**Proposed Resolution**:

(a) `spec.md:172`.

Old:
> Implemented (`src/flush.rs:14-16`, `:59-75`, `:185-206`, `:234`)

New:
> Implemented (`src/flush.rs:14-16`, `:59-75`, `:193-214`, `:243`)

(b) `spec.md:174`.

Old:
> Implemented (`src/flush.rs:215-219`, `:263-277`)

New:
> Implemented (`src/flush.rs:223-227`, `:276-290`); see P6 for the reachability of Drop under the recommended trigger wiring

If P6 is rejected, drop the clause after the semicolon in (b).

**Rationale**: These are factual updates only.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P6: 001/FR-13 — the recommended trigger wiring prevents the final flush (reference cycle)

**Direction**: ALIGN (severity moderate, new)

**Requirement**: FR-13: "`FlushManager` performs a final flush on Drop (graceful shutdown) …".

**Current State**:
- Spec says (`spec.md:174`): "`FlushManager` performs a final flush on Drop (graceful shutdown): Drop sets the shutdown flag, wakes the worker and joins it; the worker runs one last flush pass …". `spec.md:311` and `src/lib.rs:127` recommend the trigger `move || flush_manager.trigger_flush()`.
- Code does:
  - The trigger owns the `FlushManager` (`src/lib.rs:131-133`; field `:61`).
  - The manager's worker thread owns the callbacks (`src/flush.rs:107-109`, `:142-144`), which in every wiring in the tree capture `Arc<ExtendedMetadataStoreComponent>` (`src/repair_test_force_flush_error.rs:24`, `:33-35`).
  - The worker exits only after `shutdown` is set by `Drop` (`src/flush.rs:223-227`, `:276-290`), and `Drop` runs only when the component's last `Arc` goes away. That `Arc` is held by the running worker, so the cycle never breaks.
  - Effects: no final flush, a leaked worker thread (polling every 10 ms, `:205`, `:211`) and a leaked mock device or client. No API exists to detach the trigger.

**Proposed Resolution** (code; option 2 is recommended):
1. **Wiring fix with no API change.** Document, and use in tests, callbacks that capture `Weak<ExtendedMetadataStoreComponent>`: `let w = Arc::downgrade(&comp); Box::new(move || w.upgrade().map(|c| c.snapshot_entries()).unwrap_or_default())`, and the same for `dirty_count`/`mark_flushed`. The component's drop then drops the trigger, which drops the manager and runs the final flush. One catch: by the time that final flush runs, the component is already gone, so `snapshot_fn` would return an empty set and the final flush would **write an empty region**. That is data loss. Therefore option 1 also needs `do_flush` to skip the shutdown pass when the snapshot callback cannot reach the component. That needs a sentinel such as `Option<Vec<…>>`, which is a signature change to `FlushManager::start`.
2. **Explicit detach (recommended).** Add `pub fn detach_flush_trigger(&self) -> Option<FlushTrigger>` to `ExtendedMetadataStoreComponent` (`src/lib.rs`). The wiring layer calls it at shutdown and drops the result, so `Drop` runs while the component is alive and the final flush persists the real state. Document in FR-05/FR-13 that the wiring layer MUST detach before shutdown. Update `repair_test_force_flush_error.rs` to detach at the end of each test.
- Add a test: attach the trigger, `put`, detach and drop, reboot the mock, and assert the entry was recovered (final flush ran).
- Files: `src/lib.rs`, `src/repair_test_force_flush_error.rs` (or `tests/persistence.rs`), and `spec.md` FR-13/FR-05 after the fix (backfill at apply time).

**Rationale**: The spec's guarantee is reasonable and is what callers rely on for graceful shutdown. The defect is in the wiring contract, not in the spec. Option 2 is recommended because option 1 risks the empty-region overwrite described above.

**Confidence**: HIGH (the cycle); MEDIUM (choice of fix)

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P7: 001/FR-05 — finish ALIGN-EMS-003's acceptance tests and restate criterion 3

**Direction**: ALIGN (minor)

**Requirement**: FR-05; `align-tasks.md` ALIGN-EMS-003 acceptance criteria 2 and 3.

**Current State**:
- Spec says (ALIGN-EMS-003 AC2 and AC3, `align-tasks.md:174-175`): "A control case without a fault returns `Ok` and the data is recovered after a reboot." "A ticket served by a failed pass but later covered by a successful pass returns `Ok`."
- Code does:
  - The control test (`src/repair_test_force_flush_error.rs:63-72`) asserts `Ok` and `dirty_count() == 0`, but it never reboots or recovers.
  - For AC3: the failed pass's `notify_all` (`src/flush.rs:269`) normally wakes the waiter, and the waiter returns `Err` at `:181-185` before any retry. It returns `Ok` only if it is scheduled after a later successful pass. So AC3 as written is not a deterministic property and cannot be tested reliably.

**Proposed Resolution**:
- Extend `force_flush_ok_on_healthy_device` or add a test: `put` → `force_flush()` → reboot with `MockBlockDevice::reboot_from` → `initialize_from_client` → `get` returns the value.
- Restate AC3 in `align-tasks.md` as: "After a failed `force_flush()`, a later `force_flush()` on a healthy device returns `Ok` and persists the write that the failed call left dirty." Then test it with a mock that fails the first N writes and then succeeds (`FaultConfig::fail_after_n_writes` is a one-way threshold; this needs a small `FaultConfig` extension or a fresh mock on reboot).
- File: `src/repair_test_force_flush_error.rs` (or `tests/persistence.rs`).

**Rationale**: The fix is correct, but its regression coverage is thinner than the agreed acceptance criteria.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P8: 001/FR-09 — corrupt entries still do not cause fallback (carried over; ALIGN-EMS-004)

**Direction**: ALIGN (moderate, unchanged)

**Requirement**: FR-09 and Recovery Algorithm steps 4–5.

**Current State**:
- Spec says (`spec.md:170`): "Recovery falls back to inactive region if active region is corrupt". Spec says (`spec.md:302-303`): "Read active region, verify header CRC and entry CRCs … If active region corrupt: try inactive region".
- Code does:
  - Fallback happens only on a bad header or a `flush_seq` mismatch (`src/recovery.rs:53-66`).
  - A bad-CRC entry is skipped by one sector and the partial region is accepted (`src/on_disk.rs:341-344`).
  - There is no warning, contrary to `src/recovery.rs:21`.

**Proposed Resolution**: Keep ALIGN-EMS-004 as written. The evidence is unchanged: the branch did not touch `on_disk.rs` or `recovery.rs`. No new task.

**Rationale**: The spec states the intended safety property. The code is weaker.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P9: 001/NFR-04 — fallback recovery leaves the good region exposed (carried over; ALIGN-EMS-005)

**Direction**: ALIGN (moderate, unchanged)

**Requirement**: NFR-04.

**Current State**:
- Spec says (`spec.md:196`): "Crash consistency: partial flush never corrupts previously-committed data". Spec says (`spec.md:294`): "Crash during step 2: old active region is still valid".
- Code does:
  - The fallback returns the superblock unchanged (`src/recovery.rs:68-77`), and it is passed on (`src/lib.rs:155-157`).
  - The next flush writes the region just recovered (`src/flush.rs:45-46`).
  - A crash before `:54` destroys both copies (`src/recovery.rs:79-84`).
- New since the last report: with `4a8fe3e7`, a write failure during that flush is now reported to the `force_flush()` caller. The on-disk damage is the same.

**Proposed Resolution**: Keep ALIGN-EMS-005 as written. No new task.

**Rationale**: Unchanged code-side defect against an explicit spec guarantee.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P10: 001/FR-01 — over-long keys accepted but not persistable (carried over; ALIGN-EMS-006)

**Direction**: ALIGN (moderate, unchanged; the error-variant sub-decision is still open)

**Requirement**: FR-01.

**Current State**:
- Spec says (`spec.md:162`): "`put(key, value)` stores arbitrary binary data (0 to 128 KiB) by string key". Spec says (`spec.md:213`): "key_len(2B)".
- Code does: only the value length is checked (`src/lib.rs:179`). `key_len` is truncated with `as u16` (`src/on_disk.rs:233`) while the full key is copied (`:242`). The CRC fails on recovery (`:276-283`).

**Proposed Resolution**: Keep ALIGN-EMS-006 and its open sub-decision (reuse `ValueTooLarge`, or add `KeyTooLarge`; `KeyTooLarge` is recommended). No new task.

**Rationale**: Silent loss of an acknowledged write.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P11: 001/SC-7 — undocumented public items (carried over; ALIGN-EMS-007)

**Direction**: ALIGN (minor, unchanged)

**Requirement**: SC-7.

**Current State**:
- Spec says (`spec.md:260`): "All public items have doc comments; `cargo doc --no-deps` is warning-free".
- Code does: the items listed in the drift report (`src/on_disk.rs:12-23` … `:360`; `src/block_io.rs:12`, `:18`; `src/recovery.rs:14-16`) have no `///`. No `missing_docs` lint is enabled.
- The branch's new items are private (`FlushTickets` fields), so this branch adds nothing to the list.

**Proposed Resolution**: Keep ALIGN-EMS-007 as written. No new task.

**Rationale**: The project CLAUDE.md requires doc comments on public APIs.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P12: tasks.md and align-tasks.md — mark ALIGN-EMS-003 resolved

**Direction**: BACKFILL

**Requirement**: FR-05 tracking (inter-spec conflicts 1 and 2).

**Current State**:
- Spec says (`tasks.md:19`): "… Follow-up: flush-failure propagation through `FlushManager::trigger_flush()` (proposal P1, 2026-10-14)."
- `align-tasks.md:20` says "ALIGN-EMS-003 | P1 | FR-05 / US6 AC1 — flush failure reported as `Ok` | major | open".
- `align-tasks.md:147-152`, "Current Code", says: "the documented trigger, `FlushManager::trigger_flush`, always ends in `Ok(())` (`src/flush.rs:177`) …".
- Code does: the failure is propagated (`src/flush.rs:181-185`, `:260-270`; commit `4a8fe3e7`).

**Proposed Resolution**:

(a) `tasks.md:19`.

Old:
> Follow-up: flush-failure propagation through `FlushManager::trigger_flush()` (proposal P1, 2026-10-14).

New:
> Follow-up flush-failure propagation through `FlushManager::trigger_flush()` (ALIGN-EMS-003) — RESOLVED by `4a8fe3e7` (`src/flush.rs:181-185`; test `src/repair_test_force_flush_error.rs`).

(b) `align-tasks.md:20`, status cell.

Old: `open`

New: `resolved (4a8fe3e7); AC2/AC3 tests → P7 (2026-10-14 bundle sync)`

(c) `align-tasks.md`, ALIGN-EMS-003 section. Append after the "Current Code" paragraph (`:152`).

Old: *(nothing)*

New:
> **Status (2026-10-14, branch `fix/bundle-dispatcher-ems`):** RESOLVED by `4a8fe3e7`. `FlushTickets` gained `durable`/`last_error` (`src/flush.rs:88-97`); `trigger_flush` returns `Err(last_error)` unless `durable >= ticket` (`:181-185`). AC1 met (`src/repair_test_force_flush_error.rs:42-61`); AC2/AC3 tracked by proposal P7; AC4 by proposal P1.

(d) `align-tasks.md:111-116`, ALIGN-EMS-002 dependency note. In the file the old text is wrapped across lines 112–116; match it ignoring line breaks.

Old:
> **Dependency:** the alternative fix (propagate the flush-time capacity error out of `force_flush()`) cannot work until ALIGN-EMS-003 (P1) lands, because `FlushManager::trigger_flush` currently discards every flush error (`src/flush.rs:177`).

New:
> **Dependency:** ALIGN-EMS-003 has landed (`4a8fe3e7`): the flush-time capacity error now reaches `force_flush()` callers as `StorageError("… exceeds region capacity …")`. The alternative fix only needs to map it to `CapacityExhausted`.

**Rationale**: The tracking documents must match the code.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P13: code doc comments that contradict the spec

**Direction**: ALIGN (minor; doc-only code change)

**Requirement**: FR-12, US6 AC5, FR-14 (doc comments are the in-code contract).

**Current State**:
- Spec says:
  - FR-12 (`spec.md:173`): calls registered mid-pass wait for the next pass.
  - US6 AC5 (`spec.md:144`): "one worker wake-up, no flush".
  - FR-14 (`spec.md:175`): "mutations not yet covered by a successful flush".
- Code docs say:
  - `src/flush.rs:79`: "Multiple concurrent `trigger_flush` calls share a single in-flight flush." This is inexact.
  - `src/flush.rs:156`: "Returns immediately if there are no dirty entries". In fact the call waits for one worker wake-up (`:233-238`).
  - `src/lib.rs:75`: "mutations since last flush". This is inexact after a failed flush or a mid-flush write.

**Proposed Resolution** (code, comments only):
- `flush.rs:79` → "Concurrent `trigger_flush` calls are coalesced by ticket: one pass serves every call registered before it snapshots; calls registered during a pass are served by the next one."
- `flush.rs:156` → "If nothing is dirty, returns `Ok` after one worker wake-up without disk I/O."
- `lib.rs:75` → "Get the current dirty count (mutations not yet covered by a successful flush)."

**Rationale**: The spec was backfilled and approved on 2026-10-14. The comments lag behind it.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P14: 001/Known Gaps (CapacityExhausted) — record that the capacity error now reaches the caller

**Direction**: BACKFILL

**Requirement**: Known Gaps, `CapacityExhausted` entry (ALIGN-EMS-002, parked).

**Current State**:
- Spec says (`spec.md:185`): "… while region/partition capacity is enforced at flush time inside `flush::flush_to_disk` as a `String` error ("exceeds region capacity", `src/flush.rs:36-42`). Consequently the SSD integration test …"
- Code does: that `String` error now propagates through `FlushManager::trigger_flush()` (`src/flush.rs:181-185`) into `force_flush()` as `StorageError` (`src/lib.rs:232`). `CapacityExhausted` is still never constructed.

**Proposed Resolution** — `spec.md:185`:

Old:
> while region/partition capacity is enforced at flush time inside `flush::flush_to_disk` as a `String` error ("exceeds region capacity", `src/flush.rs:36-42`). Consequently

New:
> while region/partition capacity is enforced at flush time inside `flush::flush_to_disk` as a `String` error ("exceeds region capacity", `src/flush.rs:36-42`); with a `FlushManager` trigger attached, `force_flush()` returns that error as `StorageError` (not `CapacityExhausted`). Consequently

**Rationale**: This describes the current, tested behaviour accurately. The parked ALIGN-EMS-002 decision is unaffected.

**Confidence**: HIGH

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify

---

### Proposal P15: 001/spec.md:8 — "Previous Sync" history line cites the untracked 002 spec

**Direction**: SUPERSEDE

**Requirement**: Header history (inter-spec conflict 4).

**Current State**:
- Spec says (`spec.md:8`): "… Interface-only test usage (002-FR-011) and surfacing capacity to the caller (002-FR-007) remain code-side gaps tracked as ALIGN tasks in `.specify/sync/align-tasks.md`."
- Code / tree: only `specs/001-extended-metadata-store/` is tracked. The 2026-10-14 SUPERSEDE replaced live 002 references with ALIGN-EMS-001/002 but left this line.

**Proposed Resolution**: Keep the historical sentence and append the mapping, so the history is not rewritten.

Old:
> remain code-side gaps tracked as ALIGN tasks in `.specify/sync/align-tasks.md`.

New:
> remain code-side gaps tracked as ALIGN tasks in `.specify/sync/align-tasks.md` (now ALIGN-EMS-001 and ALIGN-EMS-002; the 002 spec is not tracked in this tree).

Also update `spec.md:7` ("Last Synced") at apply time to reference this sync.

**Rationale**: The history line should stay as written but must not point at a spec that does not exist.

**Confidence**: MEDIUM (cosmetic; a reviewer may prefer to leave history untouched)

**Action**: - [ ] Approve - [ ] Reject - [ ] Modify
