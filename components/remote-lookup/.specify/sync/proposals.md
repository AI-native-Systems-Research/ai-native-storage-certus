# Drift Resolution Proposals — remote-lookup

Generated: 2026-08-20
Based on: `.specify/sync/drift-report.{json,md}` (current pending report — 6 drift, 3 unspecced)
Policy: `.specify/sync/PHASE_B_POLICY.md` (no special per-component note; classify each drift item
by reading its `location` code — spec-lag → BACKFILL, real bug → ALIGN — and BACKFILL the 3
unspecced features).

> Supersedes the 2026-06-19 proposals set (kept only in git history) and the 2026-08-07 pass
> (`proposals-20260807.json`). This file reflects the current drift report.

## Summary

| Resolution Type | Count |
|-----------------|-------|
| BACKFILL applied (drifted requirements) | 5 |
| ALIGN tasks (spec correct, code gap) | 1 |
| UNSPECCED backfilled | 3 |
| RESOLVED (already fixed) | 0 |
| HUMAN_DECISION | 0 |

All five BACKFILL items are on spec `001-remote-lookup-placeholder` (superseded placeholder — the
shipped code intentionally implements the `002` design). The single ALIGN item and all three
unspecced backfills are on the design-of-record spec `002-remote-lookup-rdma`.

---

## Drifted requirements

### Proposal 1 — 002-remote-lookup-rdma / FR-018 → **ALIGN**

- **Spec (correct)**: unknown/malformed wire frames MUST be **logged** and ignored.
- **Code (`src/actor.rs:314,330`)**: framing + `op_id` echo are correct and unknown/malformed
  frames are ignored, but no log line is emitted — `WireMessage::Unknown { .. } => {}` (:330) and
  the malformed-decode `Err(_) => return` (:314) are both silent.
- **Direction**: correct spec + incomplete code → **ALIGN** (no source edit this pass). Emit a
  `logger` line on both arms before dropping. See `align-tasks.md` Task 3. The FR-018 *ignore*
  behavior is aligned; only the *logging* half is a code gap.

### Proposals 2–6 — 001-remote-lookup-placeholder → **BACKFILL** (superseded-placeholder annotations)

Spec 001 is a retained-for-history placeholder; the shipped component intentionally implements the
`002` design. These five requirements are spec-lag (stale spec, intentional code), so each drifted
requirement is annotated inline as **superseded-by-002** rather than the placeholder being
rewritten.

| # | Req | Before | After (annotation appended) |
|---|-----|--------|-----------------------------|
| 2 | FR-001 | `batch_lookup(&[(CacheKey, IpcHandle)])` | *Superseded by 002 FR-001*: shipped signature `&[(CacheKey, u32 /* size */)]`; `IpcHandle` dropped (CPU/DRAM-only); `Ok(())` ⇒ resident in local memory tier. |
| 3 | FR-003 | per-entry placeholder log | *Superseded by 002*: real KEY_QUERY→RDMA protocol; no per-entry placeholder log. |
| 4 | FR-004 | return `Err(NotFound)`, no network I/O | *Superseded by 002 FR-005..FR-012*: real zyre + one-sided RDMA; `Ok(())` when resident, `Err(NotFound)` only on deadline. |
| 5 | FR-008 | interface-only, no public fns outside | *Superseded by 002 FR-029*: intentional out-of-interface hooks `peers_seen`/`signal_shutdown`/`shutdown` for teardown/tests. |
| 6 | SC-002 | compiles with `(CacheKey, IpcHandle)` | *Superseded by 002 FR-001*: compiles with `(CacheKey, u32)`; the `IpcHandle` drop removes the type-equality goal. |

---

## Unspecced features → **BACKFILL-UNSPECCED**

### Proposal 7 — DISCONNECT_ACK_TIMEOUT (fixed 500 ms bounded wait) → FR-014

- **Code**: `src/actor.rs:37,1020-1033`. `teardown_peer` blocks for `DisconnectAck` bounded by a
  hardcoded 500 ms `DISCONNECT_ACK_TIMEOUT`; a lost ack makes the actor give up rather than hang.
- **Backfill**: added a sentence to FR-014 documenting the fixed bound, stated it is deliberately
  **not** a `LookupConfig` knob, and distinguished it from FR-031's configurable
  `connection_teardown_timeout` orphan grace.

### Proposal 8 — Malformed/truncated wire frame silent drop → FR-018

- **Code**: `src/actor.rs:314`. Frames that fail `WireMessage::decode` are dropped — a class FR-018
  never named (it covered only unknown `msg_type`).
- **Backfill**: FR-018 now names **both** ignore classes (unknown `msg_type` + malformed decode)
  and requires **both** to be logged. The logging code work is folded into the FR-018 ALIGN task
  (Task 3), which now explicitly covers the `Unknown` arm and the malformed `Err(_)` arm.

### Proposal 9 — publish_success AlreadyExists size-collision guard → FR-006

- **Code**: `src/actor.rs:576-591`. On `create_memory_tier_entry` → `AlreadyExists`, success is
  counted **only** when `entry_size(key) == len`; a differing size discards the private slot
  (`memory_tier.remove`) and marks the key unsatisfied — the resident entry is never evicted.
- **Backfill**: FR-006's bare "a racing `AlreadyExists` counts as success" was refined with the
  size-equality condition and the never-evict-on-collision rule. Documented in
  `knowledge/size-mismatch-handling.md`.

---

## Acceptance scenarios added (spec 002)

- Edge Case: unknown/malformed frame → logged and dropped, poll loop continues (FR-018).
- Edge Case: `AlreadyExists` publish race → success only at matching size; size mismatch discards
  the private slot and leaves the resident entry untouched (FR-006).
