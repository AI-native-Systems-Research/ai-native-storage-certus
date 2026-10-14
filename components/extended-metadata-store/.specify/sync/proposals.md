# Drift Resolution Proposals

Generated: 2026-10-14
Based on: drift-report from 2026-10-14 (`.specify/sync/drift-report.{md,json}`, branch `fix/extended-metadata-store-flush` @ `2970e0a8`)
Mode: batch (no questions asked). **Every proposal is `approved: false`; nothing has been applied.**
Paths: `spec.md`, `plan.md` and `tasks.md` mean `components/extended-metadata-store/specs/001-extended-metadata-store/{spec,plan,tasks}.md`. `src/…` and `tests/…` are relative to `components/extended-metadata-store/`.

This file replaces the 2026-08-20 Phase-B proposals (P1–P8 there, all recorded as applied).
Their outcomes are already in spec.md and `align-tasks.md`, and nothing in them is re-proposed
here except the still-parked `CapacityExhausted` item (P6 below).

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 7 |
| Align (Spec → Code) | 6 |
| Human Decision | 0 (P4 includes one sub-decision, the error variant; its direction, ALIGN, is not in doubt) |
| New Specs | 0 (the 5 unspecced behaviours belong to existing FRs; backfilled in P7–P11) |
| Supersede | 1 |
| Remove from Spec | 0 |
| **Total** | **14** |

| ID | Requirement | Type | Severity | Confidence | approved |
|----|-------------|------|----------|------------|----------|
| P1 | FR-05 / US6 AC1 — `trigger_flush` reports a failed flush as `Ok` | ALIGN | major | HIGH | false |
| P2 | FR-09 — corrupt entries do not cause fallback | ALIGN | moderate | HIGH | false |
| P3 | NFR-04 — after a fallback, the next flush overwrites the only good region | ALIGN | moderate | HIGH | false |
| P4 | FR-01 — keys longer than 65 535 bytes are accepted but cannot be persisted | ALIGN | moderate | HIGH | false |
| P5 | SC-7 — public items without doc comments | ALIGN | minor | HIGH | false |
| P6 | Parked: `CapacityExhausted` never constructed (ALIGN-EMS-002) | ALIGN | moderate | HIGH | false |
| P7 | FR-12 / US6 AC2, AC5 — coalescing semantics (U1) | BACKFILL | minor | HIGH | false |
| P8 | FR-11 — threshold/timer trigger details (U2) | BACKFILL | minor | HIGH | false |
| P9 | FR-14 / FR-17 — snapshot-scoped `mark_flushed` (U3) | BACKFILL | minor | HIGH | false |
| P10 | FR-13 + `FlushManager` entity — unconditional final flush, `completed_seq()` (U4) | BACKFILL | minor | HIGH | false |
| P11 | FR-05 row — stale line refs; trigger lock held during flush (U5) | BACKFILL | minor | HIGH | false |
| P12 | US6 test list — new repair tests; T048 cannot isolate the threshold | BACKFILL | minor | HIGH | false |
| P13 | plan.md / tasks.md stale notes (conflicts 1–4) | BACKFILL | minor | HIGH | false |
| P14 | spec.md references to the untracked 002 spec + stale line refs (conflict 5) | SUPERSEDE | minor | MEDIUM | false |

---

## Proposals

### Proposal P1: 001/FR-05 (and US6 AC1) — a failed flush is reported as durable

**id**: P1 · **Direction**: ALIGN · **approved**: false

**Requirement**: FR-05. `force_flush()` makes all mutations durable, or reports the failure.

**Spec says**:
- `spec.md:163`: "`force_flush()` ensures all mutations are durable on disk: it invokes a durable-flush trigger installed by the wiring layer via `attach_flush_trigger` and blocks until it returns, mapping trigger errors to `StorageError`"
- `spec.md:139` (US6 AC1): "`force_flush()` triggers an immediate flush and blocks until durable"
- `spec.md:180`: the trigger is "typically `move || flush_manager.trigger_flush()`"
- `components/interfaces/src/iextended_metadata_store.rs:44`: "Returns when all data is durable."

**Code does**:
- `src/lib.rs:230-234`: `force_flush` correctly maps `Err` to `StorageError`.
- `src/flush.rs:177`: `FlushManager::trigger_flush` ends with an unconditional `Ok(())`.
- `src/flush.rs:246-257`: `do_flush` marks the served tickets complete (`tickets.completed = tickets.completed.max(serving)`) whether or not `flush_to_disk` returned `Err`. It also discards that `Err`; only a `bool` reaches the worker (`:259`, used at `:234`).
- Result: with the documented trigger, a device write failure or a capacity overflow (`src/flush.rs:36-42`) makes `force_flush()` return `Ok(())` although nothing new is on disk.
- No test covers this; `attach_flush_trigger` has no caller anywhere in the workspace.

**Proposed resolution (code change)**:
1. Add a field to `FlushTickets` (`src/flush.rs:91-94`):
   - `ok_through: u64` — the highest ticket served by a successful pass, or by a pass that found nothing dirty;
   - `last_error: Option<String>`.
2. In `do_flush`, under the `flush_state` lock:
   - on success, set `ok_through = max(ok_through, serving)`;
   - on failure, set `last_error = Some(e)`;
   - in both cases set `completed = max(completed, serving)`.
   In the `dirty == 0` branch (`:223-231`), also set `ok_through = max(ok_through, serving)`.
3. In `trigger_flush`, after the wait (`:172-175`): if `tickets.ok_through >= ticket`, return `Ok(())`; otherwise return `Err(last_error.clone().unwrap_or_default())`.
   - This is correct even when a later successful pass runs before the waiter wakes up: that pass's snapshot was taken after the ticket, so it contains the caller's writes.
4. Fix the doc comment `src/flush.rs:152-153` to say that the call returns an error when the flush serving it failed.
5. Add a `--features testing` test:
   - build a component, start a `FlushManager` on `MockBlockDevice::with_fault_config(…, FaultConfig { fail_after_n_writes: Some(0) })`;
   - `attach_flush_trigger(Box::new(move || mgr.trigger_flush()))`;
   - `put`, then assert `force_flush()` returns `Err(StorageError(_))`;
   - add a control case without a fault that asserts `Ok` and that the data is on disk after a reboot.

**Rationale**: The spec and the interface contract agree with each other and describe the
intended behaviour (durability, or an error). The code breaks that contract silently, which
is the kind of error that loses data. Changing the spec to "may return `Ok` without
durability" would void the `force_flush` interface contract.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P2: 001/FR-09 — fall back to the inactive region when entries are corrupt

**id**: P2 · **Direction**: ALIGN · **approved**: false

**Requirement**: FR-09 and Recovery Algorithm steps 4–5.

**Spec says**:
- `spec.md:167`: "Recovery falls back to inactive region if active region is corrupt"
- `spec.md:299-300`: "4. Read active region, verify header CRC and entry CRCs / 5. If active region corrupt: try inactive region (best-effort recovery)"

**Code does**:
- `src/recovery.rs:53-59` accepts the active region whenever its **header** parses and `header.flush_seq == sb.flush_seq`.
- `src/on_disk.rs:341-344` (`deserialize_region`) handles an entry whose CRC fails by advancing **one sector** (`offset += sector_size`) and continuing. It still returns `Some`, with fewer entries than `header.entry_count`.
- No warning is pushed, although `src/recovery.rs:21` says warnings cover "skipped corrupt entries".
- A record that spans several sectors (values up to 128 KiB, so up to 33 sectors) leaves the parse misaligned for every later record.
- The only fallback test corrupts the header (`tests/persistence.rs:268-277`).

**Proposed resolution (code change)**:
1. Make `deserialize_region` (`src/on_disk.rs:322-349`) report how many records it skipped, e.g. return `(RegionHeader, Vec<KvEntry>, usize /*skipped*/)`, or a `Result` that distinguishes the cases.
2. In `try_read_region` / `recover_from_disk` (`src/recovery.rs:47-66`), treat the active region as corrupt when either:
   - `skipped > 0`, or
   - `entries.len() != header.entry_count`.

   Push the warning `"active region: N corrupt entries, trying inactive"` and fall back.
3. If the inactive region is also partially corrupt, keep the current best-effort behaviour of returning the partial set, but with a warning; do not format fresh while any region still has a valid header.
4. Add a test that flips a byte inside an entry record (not the header) of the active region and asserts that the fallback warning appears and the previous checkpoint is restored.

**Rationale**: The spec states the intended design explicitly ("verify … entry CRCs"). The
current code turns detectable corruption into silent data loss. A spec change to "entries
with bad CRCs are silently dropped" would weaken a crash-consistency guarantee.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P3: 001/NFR-04 — after a fallback recovery, the next flush must not overwrite the recovered region

**id**: P3 · **Direction**: ALIGN · **approved**: false

**Requirement**: NFR-04. A partial flush never corrupts previously-committed data.

**Spec says**:
- `spec.md:193`: "Crash consistency: partial flush never corrupts previously-committed data"
- `spec.md:290-291`: "This ensures that a crash at any point leaves either the old or new data intact: - Crash during step 2: old active region is still valid"

**Code does**:
- On fallback, `src/recovery.rs:68-77` returns `superblock: sb` **unchanged**. `active_region` still names the corrupt region, while the entries came from the other region.
- `initialize_from_client` returns this superblock to the wiring layer (`src/lib.rs:155-157`, `:173`), which hands it to `flush_to_disk`/`FlushManager::start`.
- The next flush writes `superblock.inactive_region_offset()` (`src/flush.rs:45-46`), which is the region that holds the only valid checkpoint.
- A crash before the superblock write (`src/flush.rs:54`) leaves the active region corrupt and the inactive region half-written. The next recovery formats fresh (`src/recovery.rs:79-84`): total data loss, and `flush_seq` resets to 0 (`src/recovery.rs:106-112`).

**Proposed resolution (code change)**:
1. In the fallback branch of `recover_from_disk` (`src/recovery.rs:70-78`), return a superblock whose `active_region` is flipped to the region actually loaded. Keep `flush_seq` at `sb.flush_seq`, so the next flush still uses a larger sequence number.
   - Effect: the next flush targets the corrupt region and leaves the good copy alone.
   - The on-disk superblock does not need rewriting: until the next flush commits, a crash simply repeats the same fallback.
2. Optional: set `entry_count` from the recovered header.
3. Add a test:
   - flush twice, corrupt the active region's header, then `initialize_from_client`;
   - flush once more under `FaultConfig::fail_after_n_writes` so the region write is interrupted;
   - reboot and assert that the previous checkpoint is still recovered.

**Rationale**: The spec states the invariant outright, and the code breaks it in a
reachable sequence: a corruption followed by a crash. Weakening NFR-04 to "only when both
regions are healthy" would remove the reason for having two regions.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P4: 001/FR-01 — reject keys that cannot be persisted

**id**: P4 · **Direction**: ALIGN (one sub-decision: which error variant) · **approved**: false

**Requirement**: FR-01. `put` stores data by string key.

**Spec says**: `spec.md:159`: "`put(key, value)` stores arbitrary binary data (0 to 128 KiB) by string key". It sets no limit on key length; `tasks.md:7` lists "Add any missing requirements (e.g., key size limits, character restrictions)".

**Code does**:
- `src/lib.rs:179-181` checks only `value.len()`.
- `src/on_disk.rs:233` writes `key_bytes.len() as u16`, which truncates silently, while `:242` copies the full key and `:223` sizes the buffer from the full key.
- On recovery, `:262`, `:267` and `:276-283` read the truncated length, the CRC check fails, and the record is dropped (with the knock-on effect described in P2).
- So a key of 65 536 bytes or more is acknowledged by `put`, served by `get`, and silently lost after restart.
- Related: `src/on_disk.rs:306-307` casts `entry_count` and `total_data_bytes` to `u32` without a check. Only regions larger than 4 GiB are affected.

**Proposed resolution (code change, then spec)**:
1. In `put` (`src/lib.rs:178`), before taking the lock, reject `key.len() > u16::MAX as usize`.
2. **Decision needed on the error variant:**
   - (a) reuse `ValueTooLarge`: no interface change, but the name is misleading;
   - (b) add `ExtendedMetadataStoreError::KeyTooLarge` in `components/interfaces/src/iextended_metadata_store.rs`: an interface change that every match on the enum must handle.
   - Recommendation: (b).
3. After the code change, update FR-01 to: "`put(key, value)` stores arbitrary binary data (0 to 128 KiB) by a string key of at most 65 535 bytes; longer keys are rejected with `<variant>`". Add a unit test.
4. Optional: add a `debug_assert!`/`try_from` on the `u32` casts in `serialize_region`.

**Rationale**: Silent loss of acknowledged writes is a defect, not a design choice. The
on-disk format fixes the key-length field at 16 bits (`spec.md:210`, "key_len(2B)"), so the
limit already exists implicitly; `put` just doesn't enforce it.

**Confidence**: HIGH that it is a bug; MEDIUM on which variant to use.

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P5: 001/SC-7 — document every public item

**id**: P5 · **Direction**: ALIGN · **approved**: false

**Requirement**: SC-7.

**Spec says**: `spec.md:257`: "All public items have doc comments; `cargo doc --no-deps` is warning-free"

**Code does**: these public items have no `///` doc comment:
- `src/on_disk.rs`:
  - `Superblock` fields `:12-23`; `Superblock::new` `:27`, `serialize` `:59`, `deserialize` `:85`, `inactive_region_offset` `:131`, `active_region_offset` `:139`, `region_capacity_bytes` `:147`;
  - `RegionHeader` fields `:155-158`, `serialize` `:162`, `deserialize` `:176`;
  - `EntryRecord` fields `:206-208`, `new` `:214`, `serialized_size` `:222`, `serialize` `:227`, `deserialize` `:257`;
  - `pad_to_sector` `:353`, `bytes_to_sectors` `:360`.
- `src/block_io.rs`: `BlockDeviceClient::sector_size` `:12`, `BlockDeviceClient::new` `:18`.
- `src/recovery.rs`: `RecoveryResult` fields `:14-16`.

No `missing_docs` lint is enabled, so `cargo doc` does not flag these.

**Proposed resolution (code change)**: Add doc comments to the items above. Add
`#![warn(missing_docs)]` to `src/lib.rs`, so the clippy `-D warnings` gate (SC-6) enforces
SC-7 from then on. No behaviour change.

**Rationale**: SC-7 matches the workspace rule in CLAUDE.md ("Public APIs require doc
comments"), so the spec is authoritative.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P6: Parked — `CapacityExhausted` never constructed (ALIGN-EMS-002, carried over)

**id**: P6 · **Direction**: ALIGN (carried over; still parked by the 2026-09-03 maintainer decision) · **approved**: false

**Requirement**: The Known Gaps entry for `CapacityExhausted` (`spec.md:182`) and `align-tasks.md:66-110` (ALIGN-EMS-002).

**Spec says**: `spec.md:182`: "The interface defines `ExtendedMetadataStoreError::CapacityExhausted` … but no code path constructs it … Surfacing capacity to the caller is a code-side change tracked in `.specify/sync/align-tasks.md` (Task ALIGN-EMS-002 …)"

**Code does**:
- Unchanged: the variant is never constructed (grep over `components/extended-metadata-store/src`).
- Capacity is a `String` error at `src/flush.rs:36-42`.
- `tests/integration_ssd.rs:526-547` cannot reach its `CapacityExhausted` arm (`:542`). The align-tasks entry cites `:513-547`, which is stale.
- **New interaction:** ALIGN-EMS-002's alternative fix, "map the flush-time `String` capacity error to `CapacityExhausted` and propagate it out of `force_flush()`" (`align-tasks.md:89-91`), cannot work until P1 lands, because `trigger_flush` discards every flush error (`src/flush.rs:177`).

**Proposed resolution**: Keep ALIGN-EMS-002 open and parked, with two changes:
- refresh its line references (`src/lib.rs:179`, `src/flush.rs:36-42`, `src/on_disk.rs:147`, `tests/integration_ssd.rs:526-547`);
- record that its `force_flush`-propagation alternative depends on P1.

**Rationale**: The spec and the existing task are correct; the code has not changed. The only update is a dependency note.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P7: 001/FR-12 and US6 AC2/AC5 — coalescing semantics after the fix

**id**: P7 · **Direction**: BACKFILL · **approved**: false · covers unspecced U1

**Requirement**: FR-12; US6 acceptance criteria 2 and 5.

**Spec says**:
- `spec.md:170`: "Multiple `force_flush()` calls coalesce into single flush operation"
- `spec.md:140`: "Multiple concurrent `force_flush()` calls coalesce into a single flush operation"
- `spec.md:143`: "When no entries are dirty, `force_flush()` returns immediately (no-op)"

**Code does**:
- `src/flush.rs:156-161`: each caller takes ticket `requested + 1`.
- `:213`: a pass serves every ticket taken before it reads `requested`, and does so before it snapshots (`:240`).
- `:252-257`: those tickets complete after the pass.
- `:171-175`: a caller registered while a pass is in flight is not released by that pass and waits for the next one.
- With nothing dirty (`:222-231`), the worker releases served callers without any I/O, but each call still waits for one worker wake-up.

The code is the authority here: tests `src/repair_test_flush_lost_dirty.rs:30,90` pin this
behaviour, and it is what makes FR-05 correct.

**Exact spec edits**:

`spec.md:170`, old:
```
| FR-12 | Multiple `force_flush()` calls coalesce into single flush operation | Implemented |
```
new:
```
| FR-12 | Concurrent `force_flush()` / `FlushManager::trigger_flush()` calls coalesce: each call registers a request (ticket) before waking the worker; a flush pass serves every request registered before it reads the request counter and snapshots the store, so all of them are released by that one pass. A call registered while a pass is already in flight is not released by that pass (its writes may postdate the snapshot); it is served, together with every other call registered meanwhile, by the next pass. No call is ever released by a flush whose snapshot predates the call | Implemented (`src/flush.rs:88-94`, `:152-178`, `:211-213`, `:251-257`) |
```

`spec.md:140`, old:
```
- Multiple concurrent `force_flush()` calls coalesce into a single flush operation
```
new:
```
- Concurrent `force_flush()` calls coalesce: all calls registered before a flush pass begins are released by that pass; calls registered while a pass is in flight wait for, and share, the next pass, whose snapshot includes their writes
```

`spec.md:143`, old:
```
- When no entries are dirty, `force_flush()` returns immediately (no-op)
```
new:
```
- When no entries are dirty, `force_flush()` performs no disk I/O: the flush worker observes a zero dirty count and releases the caller without flushing (one worker wake-up, no flush)
```

(Code-comment follow-ups, outside the spec: `src/flush.rs:79` "share a single in-flight flush" and `src/flush.rs:153` "Returns immediately" should be reworded to match.)

**Rationale**: The code change is intentional and tested, and it is required for
correctness. The old "single flush" wording described the pre-fix behaviour, which was the
bug.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P8: 001/FR-11 — background trigger details

**id**: P8 · **Direction**: BACKFILL · **approved**: false · covers unspecced U2

**Requirement**: FR-11.

**Spec says**: `spec.md:169`: "`FlushManager` provides background flush with timer + dirty threshold"

**Code does**:
- `src/flush.rs:194`: the deadline is `Instant::now().checked_add(interval)`, recomputed each time the worker starts waiting; `None` disables the timer.
- `:187`: the threshold is `dirty_threshold.max(1)`.
- `:197-206`: the worker wakes on a signal, at the deadline, or when `threshold_armed && dirty_count() >= threshold`. It polls every `THRESHOLD_POLL_INTERVAL` = 10 ms (`:14-16`), because `put`/`delete` do not notify it.
- `:188-190`, `:234`: a failed pass disarms the threshold trigger until a later pass succeeds.
- `:68-74`: defaults are 5 s and 100.
- Tested by `src/repair_test_flush_threshold.rs:62,75,91`.

**Exact spec edit**:

`spec.md:169`, old:
```
| FR-11 | `FlushManager` provides background flush with timer + dirty threshold | Implemented |
```
new:
```
| FR-11 | `FlushManager` provides background flush with two triggers. (a) Timer: a pass runs once `FlushConfig::interval` (default 5 s) has elapsed since the worker last began waiting; an interval too large to add to the current `Instant` disables the timer. (b) Dirty threshold: a pass runs as soon as `dirty_count() >= max(dirty_threshold, 1)` (default 100), detected within at most 10 ms (`THRESHOLD_POLL_INTERVAL`) because `put`/`delete` do not wake the worker. A failed flush disarms the threshold trigger until a later pass succeeds, so a failing device is retried on the timer or by an explicit `trigger_flush()` rather than in a hot loop. A timer pass with nothing dirty does no I/O | Implemented (`src/flush.rs:14-16`, `:59-75`, `:185-206`, `:234`) |
```

**Rationale**: The behaviour is working, intended (see the code comments at
`src/flush.rs:186-190`) and tested. The spec states only the headline, so this backfill adds
detail.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P9: 001/FR-14 and FR-17 — snapshot-scoped dirty accounting

**id**: P9 · **Direction**: BACKFILL · **approved**: false · covers unspecced U3

**Requirement**: FR-14 and FR-17.

**Spec says**:
- `spec.md:172`: "Dirty count tracks mutations since last flush"
- `spec.md:175`: FR-17 lists `snapshot_entries` and `mark_flushed` with no semantics
- `plan.md:101`: "-> mark_flushed(new_seq) [reset dirty_count]"

**Code does**:
- `src/lib.rs:50-53`: new field `snapshot_dirty`.
- `:99-105`: `snapshot_entries` stores the current `dirty_count`, read under the read lock.
- `:113-121`: `mark_flushed(seq)` sets `flush_seq`, swaps `snapshot_dirty` to 0, and subtracts the old value from `dirty_count` with `saturating_sub`. With no snapshot before it, the subtraction is 0.
- `:92-93`: `load_entries` zeroes both counters.
- `src/flush.rs:246-249`: `reset_dirty_fn` is called only after a successful flush.
- Precondition: a second `snapshot_entries()` between a flusher's snapshot and its `mark_flushed` overwrites the one recorded slot.
- Callers follow the pairing (`tests/integration_ssd.rs:206-209`, `src/flush.rs:240-247`).

**Exact spec edits**:

`spec.md:172`, old:
```
| FR-14 | Dirty count tracks mutations since last flush | Implemented |
```
new:
```
| FR-14 | Dirty count tracks mutations not yet covered by a successful flush: `put` and `delete` each increment it after releasing the store write lock; a successful flush subtracts only the count recorded with the snapshot it wrote, so mutations made after that snapshot (including during the flush) stay dirty and are persisted by a later flush; a failed flush subtracts nothing; `load_entries` resets it to 0 | Implemented (`src/lib.rs:86-121`, `:188`, `:209`) |
```

`spec.md:175`, old:
```
| FR-17 | External persistence-wiring API (`initialize_from_client`, `snapshot_entries`, `mark_flushed`, `load_entries`, `dirty_count()`, `flush_seq()` on `ExtendedMetadataStoreComponent`) provides the startup-recovery and flush-on-demand mechanism for persistent-mode deployments; a caller/dispatcher wires a `BlockDeviceClient` and drives these methods (directly or via `FlushManager`) to obtain durability | Implemented |
```
new:
```
| FR-17 | External persistence-wiring API (`initialize_from_client`, `snapshot_entries`, `mark_flushed`, `load_entries`, `dirty_count()`, `flush_seq()` on `ExtendedMetadataStoreComponent`) provides the startup-recovery and flush-on-demand mechanism for persistent-mode deployments; a caller/dispatcher wires a `BlockDeviceClient` and drives these methods (directly or via `FlushManager`) to obtain durability. `snapshot_entries()` records the dirty count its snapshot covers; `mark_flushed(seq)` sets `flush_seq` to `seq` and subtracts exactly that recorded count, then clears it. Callers MUST therefore call `mark_flushed` only after successfully persisting the entries returned by the immediately preceding `snapshot_entries()`, and only one flusher (one `FlushManager`, or one manual caller) may drive the snapshot/mark pair at a time: an interleaved second `snapshot_entries()` overwrites the recorded count. `mark_flushed` with no preceding snapshot leaves the dirty count unchanged | Implemented (`src/lib.rs:50-53`, `:96-121`) |
```

(plan.md:101 is fixed in P13.)

**Rationale**: This is the fix's intended mechanism and it is tested
(`src/repair_test_flush_lost_dirty.rs:90`). It changes the public FR-17 contract: "reset"
became "subtract what was snapshotted". So it must be written down, including the
single-flusher precondition, which the code does not enforce.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P10: 001/FR-13 and the `FlushManager` entity — final flush and `completed_seq()`

**id**: P10 · **Direction**: BACKFILL · **approved**: false · covers unspecced U4

**Requirement**: FR-13; Key Entities, Runtime Structures.

**Spec says**:
- `spec.md:171`: "`FlushManager` performs final flush on Drop (graceful shutdown)"
- `spec.md:218`: "| `FlushManager` | Background flush thread with configurable timer/threshold, coalesced flush support | `src/flush.rs` |"

**Code does**:
- `src/flush.rs:263-277`: `Drop` sets `shutdown`, signals the worker and joins it.
- `:215-219`: the worker calls `do_flush` **without checking the dirty count**, so a clean store still gets a region write and a `flush_seq` bump. The result is ignored.
- `:180-183`: public `completed_seq()` returns the last successful flush's sequence number, initialised from the starting superblock (`:123`). Several tests rely on it, e.g. `tests/persistence.rs:530`, `:588`, `:626` and `src/repair_test_flush_threshold.rs:53`.

**Exact spec edits**:

`spec.md:171`, old:
```
| FR-13 | `FlushManager` performs final flush on Drop (graceful shutdown) | Implemented |
```
new:
```
| FR-13 | `FlushManager` performs a final flush on Drop (graceful shutdown): Drop sets the shutdown flag, wakes the worker and joins it; the worker runs one last flush pass unconditionally (even with a zero dirty count, which writes a fresh region and advances `flush_seq`) and exits. A failure of this final flush is not reported to the dropping thread | Implemented (`src/flush.rs:215-219`, `:263-277`) |
```

`spec.md:218`, old:
```
| `FlushManager` | Background flush thread with configurable timer/threshold, coalesced flush support | `src/flush.rs` |
```
new:
```
| `FlushManager` | Background flush thread with configurable timer/threshold triggers, ticket-based coalesced `trigger_flush()`, and `completed_seq()` (sequence number of the last successful flush, initialised from the starting superblock's `flush_seq`) | `src/flush.rs` |
```

**Rationale**: This documents existing behaviour. The unconditional final flush is harmless
(the extra region write is crash-safe), and `completed_seq()` is public API that tests rely on.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P11: 001/FR-05 row — refresh evidence and record the trigger-lock behaviour

**id**: P11 · **Direction**: BACKFILL · **approved**: false · covers unspecced U5 (independent of P1; if P1 lands first, drop the "open" clause)

**Requirement**: FR-05, the status and evidence column.

**Spec says**: `spec.md:163` (quoted in full under "old" below). Its evidence cites `src/lib.rs:201-215`, `:111` and `:68`.

**Code does**:
- `force_flush` is at `src/lib.rs:221-235`, `attach_flush_trigger` at `:131-133`, and the `FlushTrigger` alias at `:72`.
- `force_flush` holds `flush_trigger.read()` while the trigger runs (`:230-234`), so `attach_flush_trigger` (`:132`, which takes the write lock) blocks until any in-progress `force_flush` returns.
- Error propagation through the `FlushManager` trigger is broken (P1).

**Exact spec edit**:

`spec.md:163`, old:
```
| FR-05 | `force_flush()` ensures all mutations are durable on disk: it invokes a durable-flush trigger installed by the wiring layer via `attach_flush_trigger` and blocks until it returns, mapping trigger errors to `StorageError`; in pure in-memory mode (no trigger installed) it is a correct no-op | Implemented (`src/lib.rs:201-215`; trigger install `attach_flush_trigger` at `src/lib.rs:111`, `FlushTrigger` alias at `src/lib.rs:68`) |
```
new:
```
| FR-05 | `force_flush()` ensures all mutations are durable on disk: it invokes a durable-flush trigger installed by the wiring layer via `attach_flush_trigger` and blocks until it returns, mapping trigger errors to `StorageError`; in pure in-memory mode (no trigger installed) it is a correct no-op. The trigger runs under the trigger slot's read lock, so `attach_flush_trigger` (which replaces the trigger) blocks until any in-progress `force_flush()` returns | Implemented in the component (`src/lib.rs:221-235`; trigger install `attach_flush_trigger` at `src/lib.rs:131`, `FlushTrigger` alias at `src/lib.rs:72`). **Open:** with the documented `FlushManager::trigger_flush()` trigger, flush failures are not propagated (`src/flush.rs:177`) — see drift-report 2026-10-14 / proposal P1 |
```

**Rationale**: The evidence links point at the wrong lines, and the lock-hold behaviour is
real and observable. The "Open" clause stops the row from claiming more than the code
delivers until P1 is applied.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P12: 001/US6 test list — add the repair tests; T048 cannot isolate the threshold

**id**: P12 · **Direction**: BACKFILL · **approved**: false

**Requirement**: US6 "Tests" list, which backs FR-11, FR-12 and FR-14.

**Spec says**: `spec.md:147`: "- `flush_manager_dirty_threshold_triggers` (persistence): threshold-based flush". The list at `spec.md:145-149` names no test for coalescing correctness or for the dirty count after a mid-flush write.

**Code does**:
- `tests/persistence.rs:548-592` (T048) uses `interval: 50 ms` (`:563`) and waits 200 ms (`:585`). The timer alone satisfies its assertion, so it passed while the threshold was unimplemented. That is how the pre-fix drift report listed FR-11 as aligned.
- New tests, compiled under `--features testing`:
  - `src/repair_test_flush_threshold.rs:62` `threshold_reached_before_start_flushes_without_timer`;
  - `:75` `threshold_reached_while_worker_sleeps_flushes_without_timer`;
  - `:91` `below_threshold_does_not_flush_before_timer`;
  - `src/repair_test_flush_lost_dirty.rs:30` `write_during_flush_is_durable_when_its_trigger_flush_returns`;
  - `:90` `write_during_flush_stays_dirty`.

**Exact spec edits**:

`spec.md:147`, old:
```
- `flush_manager_dirty_threshold_triggers` (persistence): threshold-based flush
```
new:
```
- `flush_manager_dirty_threshold_triggers` (persistence): flush after exceeding the threshold. Its 50 ms timer also fires within the 200 ms wait, so it passes without a working threshold trigger; the threshold path is isolated by the `repair_test_flush_threshold` tests below
```

Insert after `spec.md:149` (the line "- `force_flush_succeeds` (unit): in-memory mode no-op"):
```
- `threshold_reached_before_start_flushes_without_timer`, `threshold_reached_while_worker_sleeps_flushes_without_timer`, `below_threshold_does_not_flush_before_timer` (unit, `src/repair_test_flush_threshold.rs`, `--features testing`): the dirty-threshold trigger with a 1 h timer (FR-11)
- `write_during_flush_is_durable_when_its_trigger_flush_returns`, `write_during_flush_stays_dirty` (unit, `src/repair_test_flush_lost_dirty.rs`, `--features testing`): a `trigger_flush()` caller is never released by a flush whose snapshot predates its write, and a write made mid-flush stays dirty until a later flush persists it (FR-12, FR-14)
```

**Rationale**: The tests exist and pin the fixed behaviour. Recording that T048 is weak
protects against the same false "aligned" verdict in future sweeps.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P13: plan.md and tasks.md — stale notes that contradict spec.md and the code

**id**: P13 · **Direction**: BACKFILL · **approved**: false · resolves inter-spec conflicts 1–4

**Requirement**: Consistency of the spec-kit project with spec.md (FR-05, FR-12, FR-17, and the SC workspace note).

**Spec says / code does**: see each edit. Evidence:
- `src/lib.rs:47-62` (fields), `:221-235` (`force_flush` wired), `:99-121` (snapshot-scoped `mark_flushed`);
- `src/flush.rs:152-178` (ticket coalescing);
- root `Cargo.toml:23` and `:125` (workspace member and dependency);
- `align-tasks.md:9-17` (ALIGN-001 and ALIGN-002 resolved).

**Exact edits**:

`plan.md:25`, old:
```
- Fields: `store: RwLock<HashMap<String, Vec<u8>>>`, `dirty_count: AtomicU64`, `flush_seq: AtomicU64`
```
new:
```
- Fields: `store: RwLock<HashMap<String, Vec<u8>>>`, `dirty_count: AtomicU64`, `snapshot_dirty: AtomicU64` (dirty count covered by the last `snapshot_entries()`), `flush_seq: AtomicU64`, `flush_trigger: RwLock<Option<FlushTrigger>>`
```

`plan.md:32`, old:
```
- Flush coalescing: multiple `trigger_flush()` callers share a single in-flight flush via `Condvar` wait
```
new:
```
- Flush coalescing: each `trigger_flush()` caller takes a ticket (`FlushTickets::requested`); a flush pass serves every ticket taken before it reads that counter and snapshots the store, then advances `FlushTickets::completed` and notifies the `Condvar`; callers registered while a pass is in flight wait for the next pass
```

`plan.md:96`, old:
```
  -> snapshot_entries() [acquire read lock, clone HashMap, release]
```
new:
```
  -> snapshot_entries() [acquire read lock, record dirty_count, clone HashMap, release]
```

`plan.md:101`, old:
```
  -> mark_flushed(new_seq) [reset dirty_count]
```
new:
```
  -> mark_flushed(new_seq) [subtract the dirty count recorded by that snapshot; mutations made since stay dirty]
```

`plan.md:104`, old:
```
> **Note**: `IExtendedMetadataStore::force_flush()` (the public trait method) is currently *not* wired into this path in any build configuration — it is an unconditional no-op. A caller/dispatcher must instead hold a reference to the component's FR-17 inherent API and drive `FlushManager::trigger_flush()` (or call `flush::flush_to_disk()` directly) to actually invoke this data flow. See spec.md FR-05 Known Gaps and `.specify/sync/align-tasks.md` (Task ALIGN-002) for the code-fix tracking this gap.
```
new:
```
> **Note**: `IExtendedMetadataStore::force_flush()` reaches this data flow when the wiring layer has installed a `FlushTrigger` via `attach_flush_trigger` (typically `move || flush_manager.trigger_flush()`); with no trigger installed (pure in-memory mode) it is a no-op. Callers may also drive the FR-17 inherent API directly. See spec.md FR-05.
```

`plan.md:162`, old:
```
> **Note**: The commands below require `components/extended-metadata-store` to be a member of the root workspace `Cargo.toml`. It currently is **not** (see `.specify/sync/align-tasks.md` Task ALIGN-001), so `cargo test -p extended-metadata-store` presently fails with "package ID specification did not match any packages", and running `cargo test`/`clippy`/`doc` from inside the crate directory fails with "current package believes it's in a workspace when it's not."
```
new:
```
> **Note**: `components/extended-metadata-store` is a member of the root workspace `Cargo.toml` (`[workspace] members` and `[workspace.dependencies]`), so the commands below run from the repository root.
```

`tasks.md:13`, old:
```
- [ ] T056 Run `cargo doc --no-deps` and ensure all public items have doc comments without warnings — blocked on Task ALIGN-001 (crate not in workspace) in `.specify/sync/align-tasks.md`
```
new:
```
- [ ] T056 Run `cargo doc --no-deps` and ensure all public items have doc comments without warnings — no longer blocked (ALIGN-001 resolved); undocumented public items are listed in `.specify/sync/drift-report.md` (SC-7) and proposal P5
```

`tasks.md:18`, old:
```
- [ ] ALIGN-001 (MAJOR): `components/extended-metadata-store` is missing from the root `Cargo.toml` `[workspace] members` array — crate is not built or tested by `cargo build`/`cargo test --all`/CI.
```
new:
```
- [x] ALIGN-001 (MAJOR, RESOLVED 2026-08-20): `components/extended-metadata-store` is now a member of the root `Cargo.toml` `[workspace] members` array and is built and tested by CI.
```

`tasks.md:19`, old:
```
- [ ] ALIGN-002 (MAJOR): `IExtendedMetadataStore::force_flush()` is an unconditional no-op in all build configurations, contradicting FR-05.
```
new:
```
- [x] ALIGN-002 (MAJOR, RESOLVED 2026-08-20): `IExtendedMetadataStore::force_flush()` invokes the `FlushTrigger` installed via `attach_flush_trigger` (FR-05). Follow-up: flush-failure propagation through `FlushManager::trigger_flush()` (proposal P1, 2026-10-14).
```

**Rationale**: These notes describe states the code left behind long ago, or (for 32/101)
before this branch. They contradict spec.md, which is already correct.

**Confidence**: HIGH

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify

---

### Proposal P14: spec.md — replace references to the untracked 002 spec and refresh stale line references

**id**: P14 · **Direction**: SUPERSEDE · **approved**: false · resolves inter-spec conflict 5

**Requirement**: The US6 note, and the Known Gaps entries for `CapacityExhausted` and "Retained public API surface".

**Spec says**:
- `spec.md:151`: "… a follow-up test that installs a trigger and asserts durability through `force_flush()` is noted under 002-FR-007's ALIGN task."
- `spec.md:182`: "… `put()` enforces only the 128 KiB per-value `ValueTooLarge` limit (`src/lib.rs:159`), while region/partition capacity is enforced at flush time inside `flush::flush_to_disk` as a `String` error ("exceeds region capacity", `src/flush.rs:32-38`). Consequently the 002 SSD test `test_capacity_exhaustion` never reaches the `CapacityExhausted` branch. Surfacing capacity to the caller is a code-side change tracked in `.specify/sync/align-tasks.md` (Task ALIGN-EMS-002 for 002-FR-007) …"
- `spec.md:184`: "`Superblock::region_capacity_bytes()` (`src/on_disk.rs:142`) … `create_test_component_from_state()` (`src/test_support.rs:272`, `testing`-gated) …"

**Code does**:
- Only `specs/001-extended-metadata-store/` is tracked. `002-ssd-integration-test` survives only as `.specify/sync/backups/**/spec-002.md` / `spec.md.bak`.
- Current lines are: `put` size check `src/lib.rs:179`; capacity check `src/flush.rs:36-42`; `region_capacity_bytes` `src/on_disk.rs:147`; `create_test_component_from_state` `src/test_support.rs:275`; `test_capacity_exhaustion` `tests/integration_ssd.rs:526-547`.

**Proposed resolution**: Replace the 002 requirement IDs with the ALIGN task IDs that
already carry those requirements, and refresh the line references. The 002 spec stays
superseded by `align-tasks.md`; recreating it is not proposed.

`spec.md:151`, replace the fragment:
```
a follow-up test that installs a trigger and asserts durability through `force_flush()` is noted under 002-FR-007's ALIGN task.
```
with:
```
a follow-up test that installs a trigger and asserts durability (and error propagation) through `force_flush()` is tracked by ALIGN-EMS-001 in `.specify/sync/align-tasks.md` and by proposal P1 in `.specify/sync/proposals.md`.
```

`spec.md:182`, replace the fragments, in order:
- `(`src/lib.rs:159`)` → `(`src/lib.rs:179`)`
- `` `src/flush.rs:32-38` `` → `` `src/flush.rs:36-42` ``
- `Consequently the 002 SSD test `test_capacity_exhaustion` never reaches the `CapacityExhausted` branch.` → `Consequently the SSD integration test `test_capacity_exhaustion` (`tests/integration_ssd.rs:526-547`) never reaches its `CapacityExhausted` arm (`:542`).`
- `(Task ALIGN-EMS-002 for 002-FR-007)` → `(Task ALIGN-EMS-002)`

`spec.md:184`, replace the fragments:
- `` `src/on_disk.rs:142` `` → `` `src/on_disk.rs:147` ``
- `` `src/test_support.rs:272` `` → `` `src/test_support.rs:275` ``

**Rationale**: spec.md must not depend on a spec that is not in the tree. The ALIGN tasks
already hold the content those IDs referred to. Line references are refreshed so the
evidence can be checked.

**Confidence**: MEDIUM. A maintainer may instead prefer to restore and track the 002 spec. In that case, reject this proposal and keep only the line-reference refresh.

**Action**:
- [ ] Approve
- [ ] Reject
- [ ] Modify
