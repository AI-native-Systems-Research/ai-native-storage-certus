Generated: 2026-08-07T15:31:02Z

# Spec-vs-Implementation Drift Report — remote-lookup-rdma-initiator

Component: `components/remote-lookup-rdma-initiator`
Specs analyzed:
- `specs/001-rdma-remote-request-handler/spec.md` (**SUPERSEDED** — original passive RDMA responder design; retained for historical intent only)
- `specs/002-rdma-push-initiator/spec.md` (**CURRENT** — outbound RDMA push initiator; Draft)

Implementation reviewed: `src/lib.rs`, `src/connection.rs`, `src/rdma.rs`, `src/ffi.rs`, `src/telemetry.rs`, `src/loopback_test.rs`, `src/wrapper.c`, `benches/push_telemetry.rs`, `tests/mr_registration_bench.rs`, `Cargo.toml`, and the trait definition in `../interfaces/src/iremote_lookup_rdma_initiator.rs`.

> **Reading note.** Spec-001 is explicitly marked *Superseded by* spec-002: the entire inbound responder stack (listener → session state machine → protobuf handshake) was removed and reworked into an outbound initiator. Its "Not Implemented" and "Drifted" findings below are therefore **expected by design**, not defects in the current code — with the exception of two *stale self-annotations* (FR-014, FR-015) that were edited into spec-001 and now actively describe behavior the current code contradicts. The current design (spec-002) is fully aligned with the implementation.

## Summary

| Spec | Aligned | Drifted | Not Implemented | Total |
|------|:-------:|:-------:|:---------------:|:-----:|
| 001-rdma-remote-request-handler (superseded) | 3 | 9 | 11 | 23 |
| 002-rdma-push-initiator (current) | 23 | 0 | 0 | 23 |
| **Total** | **26** | **9** | **11** | **46** |

Unspecced code items: **3**. Conflicts (specs referencing nonexistent artifacts): **2**.

All 9 drifted + 11 not-implemented items belong to the superseded spec-001. The current spec-002 shows zero drift.

---

## Detailed Findings — Spec 002 (Current: RDMA Push Initiator)

| ID | Status | Evidence | Note |
|----|--------|----------|------|
| FR-001 (`push_async` enqueue-and-return, one status/item in order) | Aligned | `lib.rs:196`, `connection.rs:550` (`push_async`), `connection.rs:370` (`Batch::finish` orders statuses) | Callback receives `Vec<PushStatus>` in caller order. |
| FR-001a (callback exactly once on every outcome; dropped without invoke on `Err`) | Aligned | `connection.rs:387` (`Batch::drop`), test `the_callback_fires_exactly_once` (`connection.rs:1566`), `invalid_endpoint_drops_the_callback` (`connection.rs:1969`) | `Err` path in `lib.rs` returns before a `Batch` is built, so the caller's callback drops. |
| FR-001b (`push` blocking wrapper over `push_async`) | Aligned | `lib.rs:218`, `connection.rs:576` | Uses an `mpsc` channel to block on the async completion. |
| FR-002 (resolve via `IMemoryTier::peek`; absent→KeyNotFound, size≠length→SizeMismatch before write) | Aligned | `lib.rs:146` (`plan`), specifically `lib.rs:157-169` | `peek` (not `get`) deliberately avoids refreshing local LRU. |
| FR-003 (RDMA-write matching values into remote `addr`/`rkey`) | Aligned | `connection.rs:1156` (`RealConn::post_write` → `post_write_from_pool`), `rdma.rs:281` | |
| FR-004 (register pool `pool_info` base+size as MR once per connection) | Aligned | `connection.rs:1130` (`register_existing_mr`), `lib.rs:107` (`pool_info`) | Re-registered per connection (see `tests/mr_registration_bench.rs` rationale). |
| FR-005 (connection table keyed by `"ip:port"`; lazy establish + reuse; one thread does post+reap since QP is Send not Sync) | Aligned | `connection.rs:447` (`ConnectionTable`), `connection.rs:479` (`slot` spawns thread), `connection.rs:684` (`ConnWorker`) | |
| FR-006 (different hosts concurrent, same host queues; multiple batches in flight bounded by send-queue depth) | Aligned | `connection.rs:203` (`PUSH_WINDOW=128` credits), `post_ready` `connection.rs:767`; test `successive_batches_overlap_on_the_wire` (`connection.rs:1807`) | |
| FR-006a (bound submit queue + tracked batches; reject with UnableToConnect rather than queue) | Aligned | `connection.rs:123` (`SUBMIT_QUEUE_DEPTH=256`), `connection.rs:136` (`MAX_TRACKED_BATCHES=64`); test `a_full_submit_queue_fails_fast` (`connection.rs:1596`) | |
| FR-007 (detect QP-error/failed-write/stall; destroy QP before reporting; rebuild once; replay all outstanding; 2nd failure→UnableToConnect) | Aligned | `connection.rs:944` (`recover`), `connection.rs:983` (`check_stalled`, `STALL_TIMEOUT=2s`); tests `lost_writes_are_replayed_in_full_after_one_reconnect`, `failing_again_after_the_reconnect_gives_up`, `a_stalled_transfer_is_abandoned_and_the_connection_rebuilt` | Ordering (destroy-before-report) enforced in `recover`. |
| FR-008 (`disconnect` idempotent + `disconnect_all`; block until threads exit and report every held batch) | Aligned | `connection.rs:619` (`disconnect`), `connection.rs:633` (`disconnect_all`), `connection.rs:435` (`shutdown_and_join`); test `teardown_reports_batches_it_still_holds` | |
| FR-009 (parse `"ip:port"`, `InvalidEndpoint` otherwise) | Aligned | `connection.rs:1075` (`parse_endpoint`); test `parse_endpoint_rejects_bad_input` | |
| FR-010 (optional `ILogger`, no-op logger when unbound) | Aligned | `lib.rs:59` (`NoopLogger`), `lib.rs:121-124` (fallback) | |
| FR-011 (telemetry behind `telemetry` feature, ZST no-op off; per-phase connect-latency breakdown) | Aligned | `telemetry.rs` (both cfg variants), `record_connect_phases` `telemetry.rs:80`, wired at `connection.rs:1026` | |
| FR-012 (network-isolation security; no app auth) | Aligned | No auth code by design; consistent with trusted-fabric assumption | Nothing to implement. |
| FR-013 (unit tests: connection-table state machine, PushStatus mapping, mock transport seam, telemetry wiring) | Aligned | `connection.rs:1178-2128` test module; telemetry tests `connection.rs:2069+` and `telemetry.rs:278+` | |
| FR-014 (`connect` warm; idempotent/caching; unestablishable→`Ok(())` nothing cached; NotInitialized/InvalidEndpoint) | Aligned | `lib.rs:236`, `connection.rs:606`; tests `warm_connect_establishes_and_push_reuses`, `warm_connect_failure_is_ok_and_caches_nothing`, `warm_connect_invalid_endpoint_is_method_error` | |
| FR-015 (`set_local_peer_id`; stamp `PeerId` into `private_data` of every outbound connect) | Aligned | `lib.rs:256`, peer bytes flow `lib.rs:112-118`→`RealTransport::new`→`rdma::client_connect(private_data)` `connection.rs:1127`, `rdma.rs:555-565` | Minor caveat (Low): `local_peer_id` is snapshotted when the connection table is lazily built on first `push`/`connect`; a `set_local_peer_id` call *after* that has no effect. Spec's "call once before the first push" guidance covers this; the reverse ordering is silently ignored. |
| SC-001 (exactly one status/item, in order, correct terminal statuses) | Aligned | test `statuses_mapped_in_order`, `done_items_are_never_posted` | |
| SC-001a (every accepted batch reports once incl. rejected/torn-down; multiple batches in flight, high-water mark) | Aligned | tests `a_full_submit_queue_fails_fast`, `teardown_reports_batches_it_still_holds`, `successive_batches_overlap_on_the_wire` | |
| SC-002 (second push reuses connection, no new CM connect) | Aligned | test `reused_connection_does_not_reconnect` (`connection.rs:1936`) | |
| SC-003 (QP error → exactly one reconnect-and-retry before UnableToConnect) | Aligned | tests `lost_writes_are_replayed_in_full_after_one_reconnect`, `failing_again_after_the_reconnect_gives_up` | |
| SC-004 (telemetry small fixed constant / ZST off; measured by `push_telemetry` Criterion bench) | Aligned | `benches/push_telemetry.rs` exists; `[[bench]] name="push_telemetry"` in `Cargo.toml`; README "Benchmark" section present | Benchmark and README workflow both exist. |

---

## Detailed Findings — Spec 001 (Superseded: passive RDMA responder)

Spec-001 describes an **inbound** handler (listener, per-connection sessions, protobuf version handshake, dispatch-service delegation, standalone test client). That stack was removed. Findings are relative to the current (initiator) code.

| ID | Status | Severity | Evidence / Note |
|----|--------|:--------:|-----------------|
| FR-001 (accept incoming connections on a configurable port) | Not Implemented | — | No inbound listener exists; component is outbound-only. Accept side lives in `remote-lookup` per spec-002 boundary. |
| FR-002 (protocol version handshake, reject on mismatch) | Not Implemented | — | No handshake/version negotiation in code. |
| FR-003 (dedicated session per accepted connection) | Not Implemented | — | No session objects; outbound connections are keyed by `"ip:port"` (`connection.rs:447`). |
| FR-004 (inbound batched lookup ≤64 CacheKeys with addr+rkey) | Drifted | Low | Reworked into outbound `push(endpoint, items)`; per-item `(CacheKey, RemoteRegion)` exists but there is **no 64-entry cap** in code (`lib.rs:146`). |
| FR-005 (reject batches >64) | Not Implemented | — | No batch-size cap enforced anywhere. |
| FR-006 (serial key resolution + tight serial loop of unsignaled RDMA writes, only last signaled) | Drifted | Low | Current engine uses a credit-window (`PUSH_WINDOW=128`) post/reap loop reaping all completions, not a single-signaled serial loop (`connection.rs:767`, `855`). |
| FR-007 (transfer results into caller memory via addr+rkey) | Aligned | — | Carried forward as spec-002 FR-003; `connection.rs:1156`. |
| FR-008 (close-connection releasing all resources) | Drifted | Low | Session close replaced by endpoint-keyed `disconnect`/`disconnect_all` (`connection.rs:619`); the session concept is gone. |
| FR-009 (detect session failure w/o heartbeat; CM disconnect events) | Drifted | Low | Failure detection is QP-error / stall-timeout driven rebuild (`recover`/`check_stalled`), not session/CM-disconnect monitoring. Spec's own annotation admits CM-event monitoring "not yet wired". |
| FR-010 (delegate to a dispatch service, initially a logging placeholder) | Not Implemented | — | Resolution is via `IMemoryTier::peek` (spec-002 FR-002), not a dispatch service. No dispatch receptacle exists. |
| FR-011 (route output through a logging interface) | Aligned | — | `ILogger` used throughout (`connection.rs`, `lib.rs:59`). |
| FR-012 (standalone test client program) | Not Implemented | — | No test-client binary. `src/loopback_test.rs` is a hardware-gated `#[ignore]` integration test, not a standalone client. |
| FR-013 (unit tests: session state, protocol enc/dec, listener registry, RDMA mock) | Drifted | Low | Only the RDMA-mock portion exists (extensive mock-transport tests). Session/protocol/listener tests are absent because that design was removed. |
| FR-014 (telemetry `#[cfg(feature="telemetry")]` "exists but is not yet integrated into the serve loop") | Drifted | Medium | **Stale self-annotation.** Telemetry IS integrated into the current runtime path: recorded in `ConnWorker::ensure_connected`/`reap`/`recover`/`shutdown` and `Batch::finish` (`connection.rs:1025`, `968`, `1064`, `380`). |
| FR-015 (component wired into "full-remote" profile; `IRemoteLookupRdmaInitiator` trait methods are `NotInitialized` stubs; serving via `serve::run_blocking()`/`serve::bind_listener()`) | Drifted | Medium | **Stale self-annotation.** Trait methods are NOT stubs — `push`/`push_async`/`connect`/`disconnect`/`set_local_peer_id` are fully functional (`lib.rs:177-262`). There is no `serve` module in `src/` (see Conflicts). |
| FR-016 (lightweight binary protocol, not HTTP/REST) | Not Implemented | — | No application-level wire protocol on this side; it issues raw one-sided RDMA writes. Control plane lives in `remote-lookup`. |
| FR-017 (network-level isolation security; no app auth) | Aligned | — | Carried forward as spec-002 FR-012. |
| SC-001 (64-entry batch < 500µs, excl. network) | Drifted | Low | Not measured; latency target predates the initiator rework. |
| SC-002 (≥100 concurrent sessions) | Not Implemented | — | No session model to bound. |
| SC-003 (test client connect/lookup/disconnect zero errors) | Not Implemented | — | No test client. |
| SC-004 (session cleanup on CM disconnect, 100% within 1s) | Not Implemented | — | No CM-disconnect event monitoring wired. |
| SC-005 (telemetry <5% overhead) | Drifted | Low | Superseded by spec-002 SC-004 (which reframes the criterion as a small fixed absolute cost / ZST-when-off). |
| SC-006 (version mismatch detected within one round-trip) | Not Implemented | — | No handshake exists. |

---

## Unspecced Code

| Item | Location | Note |
|------|----------|------|
| `rdma` Cargo feature and its `#[cfg(not(feature = "rdma"))]` stubs (return `NotInitialized`) | `Cargo.toml` (`rdma` feature), `lib.rs:178-233` | Neither spec mentions a build-configuration toggle for the real transport; spec-002 assumes RDMA hardware. The no-rdma build path returning `NotInitialized` from `push`/`push_async`/`connect` is undocumented behavior. Severity: Low. |
| Hardware loopback integration test | `src/loopback_test.rs` (`#[cfg(all(test, feature = "rdma"))]`, `#[ignore]`) | Single-host real-hardware end-to-end test with test-only `rdma_cm` responder scaffolding. Not referenced by any FR (spec-002 FR-013 only mandates unit tests over the mock seam). Severity: Low. |
| `ibv_reg_mr` cost-measurement benchmark | `tests/mr_registration_bench.rs` | Hardware measurement tool feeding the "single-MR vs per-connection registration" decision. Not tied to any requirement. Severity: Low. |

---

## Conflicts (specs referencing nonexistent artifacts)

| Note | Location |
|------|----------|
| Spec-001 FR-006/FR-014/FR-015 reference a serving entry point `serve::run_blocking()` / `serve::bind_listener()` and a "serve loop". No `serve` module exists in `src/` (files are `lib`, `connection`, `rdma`, `ffi`, `telemetry`, `loopback_test`, `wrapper.c`). | `specs/001-rdma-remote-request-handler/spec.md` FR-006/FR-009/FR-014/FR-015 |
| Spec-002 header `Supersedes: 001-rdma-remote-lookup-rdma-initiator`, and spec-001 `Superseded By: 002-rdma-push-initiator` — the referenced spec-001 id (`001-rdma-remote-lookup-rdma-initiator`) does not match the actual directory name `001-rdma-remote-request-handler`. (Spec-001's link to `../002-rdma-push-initiator/spec.md` does resolve.) | `specs/002-rdma-push-initiator/spec.md` line 9; `specs/001-rdma-remote-request-handler/spec.md` line 17 |

---

## Recommendations

1. **Regenerate or retire spec-001.** It is correctly flagged superseded, but the two edited-in self-annotations (FR-014 "telemetry not integrated", FR-015 "trait methods are NotInitialized stubs" served via a `serve` module) now describe the *opposite* of the current code and reference a nonexistent module. These Medium-severity stale claims are the most misleading artifacts in the component. Either drop the annotations or move spec-001 to an archive that clearly states it documents no current behavior.
2. **Fix the supersession id mismatch.** Update spec-002's `Supersedes:` (and spec-001's `Superseded By:`) to use the real directory ids so tooling that follows the chain resolves.
3. **Document the `rdma` Cargo feature in spec-002.** Add an assumption/note that the real transport is feature-gated and that a no-`rdma` build returns `NotInitialized`; this is real, tested behavior with no requirement behind it.
4. **Optionally backfill lightweight requirements** for the hardware loopback test (`src/loopback_test.rs`) and the MR-registration benchmark (`tests/mr_registration_bench.rs`), or note them as engineering/validation tooling in spec-002's Known Limitations.
5. **Consider a note on `set_local_peer_id` ordering** in spec-002 FR-015: setting the peer id after the connection table is first built is silently ignored (the id is snapshotted at table construction). The existing "call once before the first push" guidance is consistent but does not state the after-build no-op explicitly.

Spec-002 is in strong shape: every functional requirement and success criterion maps to code and tests with no observed drift.
