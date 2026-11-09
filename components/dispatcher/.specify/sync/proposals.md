# Drift Resolution Proposals — dispatcher

Generated: 2026-11-09T06:53:49Z (analysis run 2026-11-08)
Based on: `drift-report.json` from the same run (HEAD `d45ab524d`, branch `fix/dispatcher-j33-repairs`, merge-base `7c12e427a`)
Mode: BATCH. Every proposal is `approved: false`. Nothing was applied.

These proposals come on top of the 2026-11-07 apply (`b153ea11e`), which is already in the specs. That apply's proposals were re-checked against today's code (`drift-report.md`, "Applied proposals of 2026-11-07, re-checked"):
- 12 still match the code;
- **B-44 is now wrong** (fix 8 was taken out) → B-50;
- B-36's known-gap sentence is now false (A-16 was fixed) → B-52.

**Numbering.** New ids continue from the 2026-11-07 run, which used B-35…B-49 and A-13…A-17; this run starts at **B-50** and **A-18**. **B-43 and A-13** (fix 7) do not apply and are not re-proposed; A-13's finding stays as note N-13.

**`components/interfaces`.** No proposal edits it.

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 3 (B-50, B-51, B-52) |
| Align (Spec → Code) | 1 (A-18) |
| Human Decision | 0 (notes N-13, N-14 recorded, no proposal) |
| New Specs | 0 |
| Remove from Spec | 0 |
| Supersede | 0 |

| Id | Type | Fix | Requirement | Confidence |
|---|---|---|---|---|
| B-50 | BACKFILL (reverses applied B-44) | 8 | 001 FR-061 drain-all-on-error bullet, plus a new `Last Synced` header line | HIGH |
| B-51 | BACKFILL | 8 | 001 User Story 11, acceptance scenario 4 | HIGH |
| B-52 | BACKFILL (closes A-16) | 3 | 001 FR-056 `read_write_stats` | HIGH |
| A-18 | ALIGN | 8 | 001 US-11 AS4 / FR-061: redo per-job timeout isolation safely | MEDIUM |

**The nine fixes, and what this run proposes for each:**

| # | Covered by | Spec right today? | Proposal |
|---|---|---|---|
| 1 | 001 FR-012 (`spec.md:344`) | yes (B-35) | none |
| 2 | 001 FR-034 (`spec.md:366`), FR-035 | yes (B-48); gpu-services `d45ab524d` needs no dispatcher text | none |
| 3 | 001 FR-056 (`spec.md:398`) | rule yes; A-16 gap sentence stale | **B-52** |
| 4 | 002 FR-033, FR-035 | yes (B-37, B-38); A-17 still open | none |
| 5 | 001 FR-039 step 1 (`spec.md:371`), `contracts/errors.md:36-37` | yes (B-39, B-40) | none (note N-14) |
| 6 | 001 FR-028 (`spec.md:360`), US-9 AS3 | yes (B-41, B-42) | none |
| 7 | 001 FR-036 (`spec.md:368`) | as unstable; deferred | none (B-43 / A-13 do not apply; N-13 kept) |
| 8 | 001 FR-061 (`spec.md:471`), US-11 AS4 (`:219`) | **no**: they describe the reverted fix | **B-50**, **B-51**, **A-18** |
| 9 | 001 FR-058, SC-017; 002 FR-025 | yes (B-45) | none |

---

## Proposals

### Proposal B-50: 001-dispatcher-cache-interface / FR-061 (drain-all-on-error bullet) — reverse B-44

**Direction**: BACKFILL (code → spec). **Fix**: 8, taken out by `709153326`.

**Requirement**: FR-061, the scatter-gather multi-drive cold read: how a read timeout is attributed.

**Spec says** (`specs/001-dispatcher-cache-interface/spec.md:471`, written by B-44 in the 2026-11-07 apply):
> "A read timeout (`Completion::Timeout`) carries only the actor's operation handle, not the job tag, and the actor forgets the timed-out operation; it MUST NOT fail the drive's other jobs. Submission and draining continue, and after the loop each job whose segments did not all complete is reported `Err` with the timeout message — that job only (User Story 11 scenario 4). … *(Sync 2026-11-07 … commit `408a568d0` … Before it one timeout failed every job of its drive. …)*"

The header line `spec.md:43` (`Last Synced 2026-11-07`) also lists `408a568d0` and "FR-061 (a read timeout fails only its own job)".

**Code does**: `components/dispatcher/src/pipeline.rs:1398-1411`.
- On `Completion::Timeout`, every job of drive `d` that is still `Ok` becomes `Err("NVMe read timeout drive={d} (handle …)")`.
- `obj_failed[d].fill(true)` and `stop_submitting[d] = true`.
- So objects that later complete get no H2D (`:1361`); objects already delivered are reported `Err` as well; un-submitted segments are failed (`:1462-1472`).
- Outstanding reads are still drained (`while total_completed < total_submitted`, `:1275`). Other drives are not affected.
- The incomplete-object pass of `408a568d0` is gone.
- `git diff origin/unstable...HEAD -- components/dispatcher/src/pipeline.rs` is empty.
- The test `src/repair_test_nvme_timeout_fails_whole_drive.rs` was deleted.

**Old text** (`spec.md:471`, whole line):
```
  - Preserve the **drain-all-on-error** invariant of FR-054 per drive: never break early while reads are in flight; retain and report the first error per job while continuing to drain outstanding completions. A read timeout (`Completion::Timeout`) carries only the actor's operation handle, not the job tag, and the actor forgets the timed-out operation; it MUST NOT fail the drive's other jobs. Submission and draining continue, and after the loop each job whose segments did not all complete is reported `Err` with the timeout message — that job only (User Story 11 scenario 4). Completions that indicate a broken channel rather than a failed read — an invalid tag, an unexpected completion kind, a disconnected completion channel — still fail every job of that drive and stop its submissions. *(Sync 2026-11-07, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `408a568d0`, `src/pipeline.rs:1402-1420`, `:1481-1495`. Before it one timeout failed every job of its drive. Known gap, tracked as A-15: the timed-out job's memory-tier slot is then released at once, although the drive may still write into it.)*
```

**New text**:
```
  - Preserve the **drain-all-on-error** invariant of FR-054 per drive: never break early while reads are in flight; retain and report the first error per job while continuing to drain outstanding completions. A read timeout (`Completion::Timeout`) carries only the actor's operation handle, not the job tag, so the job it belongs to cannot be named: it fails **every** job of that drive that has not already failed — including jobs whose reads had already landed — and stops further submissions to that drive; the drive's outstanding reads are still drained, and jobs on other drives are unaffected. Completions that indicate a broken channel rather than a failed read — an invalid tag, an unexpected completion kind, a disconnected completion channel — do the same. *(Sync 2026-11-08, branch `fix/dispatcher-j33-repairs` — code authoritative, `src/pipeline.rs:1398-1411`: commit `709153326` reverted `408a568d0` after review of PR #591 — continuing to submit to a drive after a timeout made a late write into a freed slot more likely and let a hung drive hold `batch_lookup` for about 155 s instead of one read timeout. The 2026-11-07 text ("MUST NOT fail the drive's other jobs") is withdrawn. Known gaps: per-job isolation of a timeout (User Story 11 scenario 4) is tracked as A-18; every failed job's memory-tier slot, the timed-out one included, is released at once although the drive may still write into the timed-out one, tracked as A-15.)*
```

**Additional edit: a new header line.** The dated 2026-11-07 header line records what that apply did and is left unchanged. A new line is inserted after it.
- **Old text** (end of `spec.md:43`, unique in the file):
  ```
  Code-side follow-ups A-15, A-16, A-17 in `.specify/sync/align-tasks.md`. No source changes.
  ```
- **New text**:
  ```
  Code-side follow-ups A-15, A-16, A-17 in `.specify/sync/align-tasks.md`. No source changes.

  **Last Synced**: 2026-11-08 — spec-sync on branch `fix/dispatcher-j33-repairs`. BACKFILL: FR-061 and User Story 11 scenario 4 describe the code again after repair `408a568d0` was taken out (`709153326`, review of PR #591); the 2026-11-07 FR-061 text from that repair is withdrawn, and per-job timeout isolation is tracked as A-18. FR-056 `read_write_stats`: the A-16 hand-over gap is closed (`d45ab524d`). No source changes.
  ```

**Rationale**: the code was put back on purpose, after review. Nara's two points in the `709153326` commit message are real hazards, so the code is authoritative and the spec must stop promising behaviour that was taken out. The re-repair is A-18. The spec is not changed to forbid per-job isolation; it records today's behaviour as a known gap.

**Confidence**: HIGH.

**Action**: [ ] Approve [ ] Reject [ ] Modify — `approved: false`

---

### Proposal B-51: 001-dispatcher-cache-interface / User Story 11, acceptance scenario 4

**Direction**: BACKFILL (code → spec), with the target kept and tracked as A-18. **Fix**: 8.

**Requirement**: US-11 AS4: a failed cold read is isolated to its entry.

**Spec says** (`spec.md:219`):
> "4. **Given** a cold entry's NVMe read or GPU DMA fails, **When** the scatter-gather loop encounters the error, **Then** the error is reported for that entry only; other entries in the batch continue to be processed independently. Exception: if the scatter-gather's final pipe-stream sync fails, …"

**Code does**: the scenario holds for some errors and not others.
- **Isolated to the entry:** a per-segment read error (`src/pipeline.rs:1317-1322`) and a GPU copy submit error (`:1385-1389`).
- **Fails every not-yet-failed entry on that drive** (other drives unaffected): a read timeout (`:1398-1411`), an invalid completion tag (`:1307-1316`), an unexpected completion (`:1412-1425`), a disconnected channel (`:1439-1454`), and a failed initial submit (`:1253-1256`).
- This is the same as on `origin/unstable`. The 2026-11-07 analysis rated AS4 aligned only because fix 8 was in.

**Old text** (`spec.md:219`, unique):
```
**Then** the error is reported for that entry only; other entries in the batch continue to be processed independently.
```

**New text**:
```
**Then** the error is reported for that entry only; other entries in the batch continue to be processed independently. Exception: an NVMe read **timeout**, or a completion that shows the drive's channel is broken (invalid tag, unexpected completion, disconnect, failed submit), fails every entry on that drive not yet failed, because the timeout does not name its entry; entries on other drives are unaffected (FR-061; per-entry isolation of a timeout is tracked as A-18 — *Sync 2026-11-08*, `src/pipeline.rs:1398-1411`).
```

**Rationale**: AS4 must not say something the code does not do. The exception is narrow and names the task that would remove it.

**Confidence**: HIGH.

**Action**: [ ] Approve [ ] Reject [ ] Modify — `approved: false`

---

### Proposal B-52: 001-dispatcher-cache-interface / FR-056 `read_write_stats` — close A-16

**Direction**: BACKFILL (code → spec). **Fix**: 3, plus the review fix `d45ab524d`.

**Requirement**: FR-056: `read_write_stats` counters are cumulative and MUST NOT decrease between two calls.

**Spec says** (`spec.md:398`, written by B-36):
> "… each call returns that baseline merged with the live drives. *(Sync 2026-11-07 … commit `d65de1fef` … Before it the counters read zero after `shutdown`. Known gap, tracked as A-16: a call made while the drive list is being handed over can count the outgoing drives twice, so the next call can read lower.)*"

**Code does** (`d45ab524d`):
- `initialize` takes `data_drives.write()` and, under that one guard, folds the outgoing drives into the baseline and swaps in the new list (`src/lib.rs:2172-2176`).
- `shutdown` takes `data_drives.write()`, folds, then `std::mem::take`s the list (`src/lib.rs:2445-2449`).
- `absorb_drive_rw_stats(&self, drives: &[DataDrive])` is the only writer of `rw_stats_baseline` (`src/lib.rs:1833-1838`).
- `read_write_stats` holds `data_drives.read()` across the baseline clone and the live-drive merge (`src/lib.rs:4610-4615`).
- These are the only non-test `data_drives.write()` sites; the lock order is `data_drives` → baseline in all three.
- So a reader sees either the old list with an unfolded baseline, or the new list with a folded one. The double count is gone.
- There is no regression test: the commit says a mock cannot hold `shutdown` in that window, and a Loom model is the right tool. Test `src/repair_test_rw_stats_after_shutdown.rs:620` covers the after-shutdown value.

**Old text** (`spec.md:398`, unique):
```
 Known gap, tracked as A-16: a call made while the drive list is being handed over can count the outgoing drives twice, so the next call can read lower.)*
```

**New text**:
```
)* *(Sync 2026-11-08, branch `fix/dispatcher-j33-repairs` — code authoritative, commit `d45ab524d`, `src/lib.rs:1833-1838`, `:2172-2176`, `:2445-2449`, `:4610-4615`:)* The fold and the replacement or clearing of the drive list MUST happen under one hold of the `data_drives` write lock, and `read_write_stats` MUST read the baseline and the live drives under one hold of its read lock (lock order `data_drives` → baseline), so a call made while the drive list is handed over never counts the outgoing drives twice. *(This closes align task A-16 of the 2026-11-07 apply.)*
```

The result reads "…the counters read zero after `shutdown`.)* *(Sync 2026-11-08 …)* The fold … *(This closes …)*".

**On apply, also**: mark **A-16 ✅ RESOLVED by `d45ab524d`** in `align-tasks.md`. Its acceptance criterion "a reader thread that loops on `read_write_stats` while `shutdown` runs sees a sequence that never decreases" should become a Loom-test follow-up; it is not met by a test today.

**Rationale**: the code now does what A-16 asked for (hold the write lock across fold and swap). The spec should record the rule and drop the stale gap sentence.

**Confidence**: HIGH.

**Action**: [ ] Approve [ ] Reject [ ] Modify — `approved: false`

---

### Proposal A-18: 001-dispatcher-cache-interface / US-11 AS4 + FR-061 — per-job timeout isolation, done safely

**Direction**: ALIGN (spec → code), as a task with no code change now. **Fix**: 8, to be repaired again.

**Requirement**: US-11 AS4, "the error is reported for that entry only", as its exception reads after B-51; FR-061's known gap after B-50.

**Spec says**: see B-50 / B-51. Today the spec states the target and records the timeout case as a tracked exception.

**Code does**: one `Completion::Timeout` fails every job of its drive (`src/pipeline.rs:1398-1411`); then `batch_lookup` releases every failed job's slot at once (`src/lib.rs:3074-3076`). `promote_and_serve` does the same for its one job (`src/lib.rs:1014-1022`).

**Proposed change**: redo `408a568d0`'s per-job isolation with the two review properties from `709153326` as acceptance criteria:
1. **No late write into a reused slot.** The timed-out job's slot is not released. It is leaked and logged, as FR-062 does for a bounce buffer, until the drive is known to have stopped. Or the drive is fenced first (controller reset or abort) before its slots are freed. This subsumes **A-15**.
2. **A hung drive is bounded.** After a timeout, stop submitting new reads to that drive: abandon it for the rest of the call and fail its unsubmitted jobs. Keep draining only what is already in flight, so `batch_lookup` waits at most about one `READ_TIMEOUT_MS` (5 s, `src/pipeline.rs:61`) per drive, not one per remaining segment (about 155 s with the reverted fix).
   - Only the job whose segment timed out can be named, and only if the actor reports the tag. Otherwise use the reverted fix's inference: a job with outstanding segments after the drain.
   - Jobs whose reads all landed before the timeout keep their `Ok` and their delivery.

**Files**: `src/pipeline.rs` (`scatter_gather_multi_drive_zero_copy`), `src/lib.rs` (`batch_lookup` cold-result loop, `promote_and_serve`), a new `repair_test_*` module. On apply: remove B-50's and B-51's exception text.

**Acceptance criteria**:
- [ ] A mock drive that times out one read: the other jobs on that drive whose reads landed are `Ok` and delivered.
- [ ] Every job with unsubmitted segments is `Err`.
- [ ] A mock drive that writes into the buffer after `Timeout`: the slot is not reused by a later `insert`.
- [ ] A mock drive that never completes after one timeout: `batch_lookup` returns within about 2 × `READ_TIMEOUT_MS`.
- [ ] Each test is written first and fails at HEAD `d45ab524d` (the first and the third; the fourth passes today and guards against regressing).

**Rationale**: AS4 is the agreed target. It was in the spec before this branch, and the reviewer did not reject the goal, only this implementation. So the spec keeps the target and the code is the side to change.

**Confidence**: MEDIUM. The design choice (leak vs. fence) belongs to whoever does the repair.

**Severity**: moderate.

**Action**: [ ] Approve [ ] Reject [ ] Modify — `approved: false`

---

## Not proposed (recorded)

- **B-43 / A-13** (fix 7, `lookup_async` returning a real stream) do not apply. Fix 7 is out (`209b01de5`), and `lookup_async` copies synchronously and returns the null stream (`src/lib.rs:3428-3449`, `:3496`).
- **N-13** (from A-13). An asynchronous `lookup_async` MUST hold the entry's read pin until its copy has **completed**, not merely been queued, and record its hit counters only then. `lookup` (`src/lib.rs:2482-2493`) inherits this. It needs an interface decision (a pin guard returned with the stream, or a caller-held pin) and is deferred to the `lookup_async` issue, together with FR-036's "instead of blocking".
- **N-14** (new). `lookup_async` maps every `dm.lookup` error to `KeyNotFound`, with no retry (`src/lib.rs:3500`). `contracts/errors.md:37` documents this, so it is not drift. But a writer-held live key is reported as a miss by the single-key path: fix 5 removed that hazard from `batch_lookup` but not from `lookup`. It needs a decision, best taken with the `lookup_async` issue.
- **A-08** (carried). The `Ssd` value test it relied on went out with fix 8's test file. The task text already asks for `Ssd`, `Remote` and `SizeMismatch` tests, so it is unchanged.
- **A-15, A-17** (carried, open). A-15 is re-anchored to `src/pipeline.rs:1398-1411` and `src/lib.rs:3074-3076`, `:1014-1022`; A-18 subsumes it. A-17 is unchanged (`components/dispatch-map/src/lib.rs:576-578`).
- **gpu-services `d45ab524d`** (`unregister_host_memory` always calls `cudaHostUnregister`). No dispatcher spec text is needed: FR-035 only requires the call, and the dispatcher ignores its result (`src/lib.rs:2424`). The gpu-services spec is for that component's own sync.
- **Unspecced code** (`repair_test_*` without `#[cfg(test)]`) is closed by `d45ab524d`. Nothing to propose.
