---
spec_sync_component: remote-lookup-rdma-initiator
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:46:01Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: d8a87b5d0e107f3e2703700bb98f2dd6bef4672bf706cb08daaae819adfb1905
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec-vs-Implementation Drift Report — remote-lookup-rdma-initiator

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 2 (1 active, 1 superseded) |
| Requirements Checked | 23 (spec-002 only; spec-001 is superseded) |
| Aligned | 22 |
| Drifted | 1 (SC-004 — minor, benchmark doc-comment only) |
| Not Implemented | 0 |
| Unspecced Features | 0 |

**Verdict: DRIFT (one minor, doc-comment-only ALIGN item outstanding).** The
active spec (`002-rdma-push-initiator`) is aligned with `src/` and `tests/` in
every shipped behavior. The single outstanding item is a stale doc comment in
`benches/push_telemetry.rs` that still states SC-004's superseded literal
`<5%` pass/fail bar; it is tracked as a code-side ALIGN task (out of scope to
edit here). Spec-001 is formally superseded and honored as such; its historical
drift is already self-annotated in the spec text.

**Re-analysis note (2026-09-02):** component `src/` was last modified
2026-07-30 (commit `00bd4002`, async-submission rework); no source, test, or
spec change has landed since the 2026-08-07 drift sweep. Every FR/SC below was
re-verified against the current code at `file:line`; the line references still
resolve exactly. The only delta from the 2026-08-07 report is that this pass
classifies the still-open SC-004 bench-comment align-task honestly as *drift*
rather than folding it under an otherwise-"clean" verdict.

**Concurrency caveat (2026-09-02):** a concurrent spec-sync run was actively
editing `components/interfaces/` at stamp time (new `src/iipc.rs`, edits to
`specs/001-interfaces/spec.md`). Because `spec-sync-hash.sh` folds the entire
interfaces tree into every component's digest, `spec_sync_inputs_sha256` is the
value at stamp time and may already have shifted — the `IRemoteLookupRdmaInitiator`
interface file itself (`components/interfaces/src/iremote_lookup_rdma_initiator.rs`)
is **unchanged**, so these interfaces edits do not affect any FR/SC verdict here.
Recompute the hash once the interfaces sync settles if the CI gate flags a mismatch.

---

## Spec 001-rdma-remote-request-handler — SUPERSEDED (honored)

**Status:** `Superseded by spec-002 (RDMA Push Initiator)` (spec.md:15-17).

This spec describes the original *passive RDMA responder* (inbound listener →
session state machine → protobuf handshake → RDMA-write-back). That entire stack
was removed in the initiator rework. Per the supersession notice, its FR-001..
FR-017 / SC-001..SC-006 are **not** evaluated as live drift against the current
outbound-initiator code — they describe a design that no longer exists by intent.

The spec already carries two in-line `⚠️ STALE SELF-ANNOTATION (2026-08-07)`
markers on FR-014 and FR-015 that document exactly how those sentences invert the
current code (telemetry *is* wired in; the trait methods are *not* stubs and there
is no `serve` module). These are retained historical markers, not new drift.

No action required beyond eventual regeneration/retirement of the stale spec
(also noted in the component CLAUDE.md).

---

## Spec 002-rdma-push-initiator — Detailed Findings

Active spec. Drift-swept 2026-08-07 (`sync/spec-drift-sweep-20260807`, "zero drift
against code"). This 2026-09-02 re-analysis re-verified every FR/SC against the
current source and confirms they remain aligned, with the single SC-004
doc-comment exception below.

### Aligned ✓

| Req | Requirement (short) | Location |
|---|---|---|
| FR-001 | `push_async(endpoint, items, on_complete)` enqueues and returns; one status per item in order | `lib.rs:196-204`, `connection.rs:550-569` |
| FR-001a | `on_complete` invoked exactly once on every outcome; dropped (not invoked) when `push_async` returns `Err` | `connection.rs:370-385` (`Batch::finish`, idempotent), `connection.rs:387-398` (`Batch::drop`); tests `the_callback_fires_exactly_once` (1566), `invalid_endpoint_drops_the_callback` (1969) |
| FR-001b | `push()` blocking wrapper over `push_async`, identical per-item semantics | `lib.rs:218-225`, `connection.rs:576-599` |
| FR-002 | Resolve each key via `IMemoryTier::peek`; absent→`KeyNotFound`, size≠length→`SizeMismatch` before any write | `lib.rs:146-174` (`plan`); test `statuses_mapped_in_order` (1329) |
| FR-003 | RDMA-write matching value into remote region using `addr`+`rkey` | `connection.rs:1156-1169` (`RealConn::post_write`), `lib.rs:163-168` |
| FR-004 | Register memory-tier pool (`pool_info` base+size) as RDMA MR once per connection; writes from `peek` pointer within region | `lib.rs:107-111,125-129`, `connection.rs:1122-1140` (`RealTransport::connect` → `register_existing_mr`) |
| FR-005 | Connection table keyed by normalized `"ip:port"`, per-host state, lazy establish + reuse; one thread per entry (QP `Send` not `Sync`) | `connection.rs:447-455,479-525` (`slot`), `ConnWorker`; tests `reused_connection_does_not_reconnect` (1936) |
| FR-006 | Different hosts concurrent, same host queues; multiple batches in flight bounded by send-queue depth, overlapping | per-host thread + `PUSH_WINDOW` credits (`connection.rs:203,767-851`); test `successive_batches_overlap_on_the_wire` (1808) |
| FR-006a | Bound submit queue and tracked batches; reject-as-`UnableToConnect` rather than queue when bounds reached | `SUBMIT_QUEUE_DEPTH=256` (123), `MAX_TRACKED_BATCHES=64` (136), `run` drain cap (703); test `a_full_submit_queue_fails_fast` (1596) |
| FR-007 | Detect QP error / failed / stalled write; destroy QP before reporting; rebuild once; replay all outstanding; second failure → `UnableToConnect` | `connection.rs:944-980` (`recover`), `983-997` (`check_stalled`), `recoveries` budget; tests `lost_writes_are_replayed...` (1422), `failing_again...gives_up` (1452), `a_stalled_transfer...` (1731) |
| FR-008 | `disconnect(endpoint)` idempotent + `disconnect_all()`; both block until threads exit and report every held batch | `connection.rs:619-641`, `435-443` (`shutdown_and_join` joins), `1058-1071` (`shutdown`); test `teardown_reports_batches_it_still_holds` (1671) |
| FR-009 | Parse `"ip:port"`; `InvalidEndpoint` for anything else | `connection.rs:1075-1090` (`parse_endpoint`); tests `parse_endpoint_rejects_bad_input` (1319), `invalid_endpoint_is_method_error` (1956) |
| FR-010 | Diagnostics via optional `ILogger`; no-op logger when unbound (never fails a push) | `lib.rs:59-66` (`NoopLogger`), `121-124` (fallback) |
| FR-011 | Telemetry behind `telemetry` feature, ZST no-op when off; full metric set incl. per-phase connect breakdown + running averages | `telemetry.rs` (enabled 19-202, ZST 207-276), wired at `connection.rs:968,1025-1032,1064,370-384`; tests `telemetry_records_*` (2069-2127) |
| FR-012 | Security = trusted-fabric isolation, no app auth | No auth code by design (nothing to implement) |
| FR-013 | Unit tests: connection-table state machine, `PushStatus` mapping, mock transport seam, telemetry wiring | `connection.rs:1178-2128` test module (mock `RdmaTransport`/`RdmaConn`) |
| FR-014 | `connect(endpoint)` warm-connect: idempotent, caching; unestablishable→`Ok(())` caching nothing; `NotInitialized`/`InvalidEndpoint` as applicable | `lib.rs:236-242`, `connection.rs:606-613`; tests `warm_connect_establishes_and_push_reuses` (1997), `warm_connect_failure_is_ok_and_caches_nothing` (2021), `warm_connect_invalid_endpoint...` (2045) |
| FR-015 | `set_local_peer_id(peer)`; stamp into connect `private_data`; snapshotted at first table build; later call ignored | `lib.rs:256-261,112-118`, `connection.rs:1097-1140` (`RealTransport` stores + passes to `rdma::client_connect`); test `set_local_peer_id_is_stored` (306) |
| SC-001 | One `PushStatus` per item in order, correct terminal statuses | test `statuses_mapped_in_order` (1329) |
| SC-001a | Every accepted batch reports exactly once (incl. rejected / torn-down); multiple batches in flight | tests `the_callback_fires_exactly_once` (1566), `a_full_submit_queue_fails_fast` (1596), `successive_batches_overlap_on_the_wire` (1808) |
| SC-002 | Second push to connected host reuses connection (no new CM connect) | test `reused_connection_does_not_reconnect` (1936) |
| SC-003 | QP error → exactly one reconnect-and-retry before `UnableToConnect` | tests `lost_writes_are_replayed...` (1422), `failing_again...gives_up` (1452) |

### Drifted ⚠️

| Req | Severity | What drifted | Evidence | Resolution |
|---|---|---|---|---|
| SC-004 | minor (doc-comment only; shipped behavior + benchmark mechanics are correct) | The benchmark's header doc comment still states SC-004's *superseded* literal bar — "enabling the `telemetry` feature adds less than 5% overhead" and "SC-004 holds when every `push/*` case is within +5%" — which contradicts spec-002 SC-004's 2026-07-15 reframing to "small fixed absolute cost / ZST-when-off" (a `<5%`-of-mock gate cannot hold by construction against a ~200–700 ns mock push). | Spec: `specs/002-rdma-push-initiator/spec.md` SC-004 (lines 316-329). Code comment: `benches/push_telemetry.rs:1-18` (esp. lines 3-4, 17-18). The metric-recording behavior itself is aligned — `telemetry.rs` counters are `Relaxed` atomics, ZST no-op when off (207-276), wired at `connection.rs:370-384`. | **ALIGN** (spec is correct; the code comment is stale). Reword the bench header to the "fixed absolute cost / ZST-when-off" criterion and cross-reference SC-004's 2026-07-15 note. `.rs` edit is out of scope for a Markdown-only sync-apply → tracked in `align-tasks.md` (Task A). |

### Not Implemented ✗

None.

---

## Unspecced Features

| Feature | Location | Lines | Note |
|---|---|---|---|
| _(none)_ | — | — | All implementation surfaces map to a spec-002 requirement, assumption, or explicitly-backfilled Known-Limitations item. |

Notes on items considered but *not* flagged as unspecced:
- `src/rdma.rs`, `src/ffi.rs`, `src/wrapper.c` — the real rdma-core transport
  internals; they are the mechanism behind FR-003/FR-004/FR-015 and are gated by
  the `rdma` Cargo feature backfilled into spec-002 Assumptions (2026-08-07).
- `src/loopback_test.rs`, `tests/mr_registration_bench.rs` — explicitly
  acknowledged as hardware-gated validation/measurement tooling in spec-002
  "Known Limitations / Follow-ups" (no FR behind them, by design).
- Telemetry read accessors (`avg_push_duration_us`, `throughput_bytes_per_sec`,
  `avg_connect_phases_us`) fall within FR-011's metric surface.

---

## Stale-Path Check (components/ → lib/ move)

No stale-path drift. `component-framework` and `spdk-sys` moved to `lib/`, but
this component's specs do not reference either. The only crate-path reference in
the specs is `components/interfaces/...` (superseded spec-001 `tasks.md:135`),
which is correct — the `interfaces` crate still lives at `components/interfaces`.

---

## Recommendations

1. **Resolve the SC-004 bench-comment align-task.** Reword
   `benches/push_telemetry.rs:1-18` from the literal `<5% vs disabled` framing to
   spec-002 SC-004's "small fixed absolute cost / ZST-when-off" criterion. This is
   the only actionable drift; it is a code (doc-comment) change, so it is queued
   in `align-tasks.md` rather than applied by this Markdown-only pass.
2. **No active-spec content changes needed** — spec-002 text is accurate; the
   drift is code→spec (a stale comment), not spec→code.
3. **Retire or regenerate spec-001.** It is formally superseded and now serves
   only as historical intent; the component `CLAUDE.md` already flags it as stale.
   Consider archiving it (or replacing its body with a pointer to spec-002) so it
   stops appearing in requirement counts and drift sweeps.
4. **Keep the SC-004 benchmark result honest.** The `push_telemetry` overhead
   figure is measured against a ~200 ns mock; if the mock path changes, re-run
   the two-baseline workflow so the "small fixed absolute cost" claim stays valid.
