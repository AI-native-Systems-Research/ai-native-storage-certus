Generated: pending
# Spec-vs-Implementation Drift Report — remote-lookup-rdma-initiator

## Summary

| Metric | Count |
|---|---|
| Specs Analyzed | 2 (1 active, 1 superseded) |
| Requirements Checked | 23 (spec-002 only; spec-001 is superseded) |
| Aligned | 23 |
| Drifted | 0 |
| Not Implemented | 0 |
| Unspecced Features | 0 |

**Verdict: CLEAN.** The active spec (`002-rdma-push-initiator`) is fully aligned
with `src/` and `tests/`. Spec-001 is formally superseded and honored as such;
its historical drift is already self-annotated in the spec text.

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
against code"). This re-analysis confirms it remains aligned.

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
| SC-004 | Telemetry per-push = small fixed cost / ZST when off; measured by benchmark | `benches/push_telemetry.rs` (Criterion two-baseline workflow) |

### Drifted ⚠️

None.

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

1. **No code or active-spec changes needed** — spec-002 and the implementation
   are in sync.
2. **Retire or regenerate spec-001.** It is formally superseded and now serves
   only as historical intent; the component `CLAUDE.md` already flags it as stale.
   Consider archiving it (or replacing its body with a pointer to spec-002) so it
   stops appearing in requirement counts and drift sweeps.
3. **Keep the SC-004 benchmark result honest.** The `push_telemetry` overhead
   figure is measured against a ~200  ns mock; if the mock path changes, re-run
   the two-baseline workflow so the "small fixed absolute cost" claim stays valid.
