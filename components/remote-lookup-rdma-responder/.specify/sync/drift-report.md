---
spec_sync_component: remote-lookup-rdma-responder
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:46:21Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 45750ede7251dd6ce802b981f67feb46a83c16e84fbd24cdecad02406662f570
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec-vs-Implementation Drift Report — remote-lookup-rdma-responder

Generated: 2026-09-02 (fresh verification pass; supersedes 2026-08-20 report).

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 24 |
| Aligned | 21 |
| Drifted | 3 |
| Not Implemented | 0 |
| Unspecced | 2 |
| Conflicts | 0 |

Requirements = 18 functional (FR-001..FR-016 incl. FR-002a, FR-011a) + 6 success
criteria (SC-001..SC-006).

Spec status: **Draft** (with clarifications resolved through 2026-07-10 and
access-flag / async-event / command-bridge notes backfilled 2026-08-07 and
2026-08-20). No requirement is marked deferred or out-of-scope, so all are analyzed.

### Change vs the 2026-08-20 report (honest re-classification)

The prior report recorded **23 aligned / 1 drifted**, marking FR-008 and FR-010 as
"aligned/resolved" on the strength of the 2026-08-07 sweep having *filed* align-tasks
for them. This pass re-verified the shipped `--features rdma` code line-by-line and
finds those two **MUST sub-clauses are still unmet in code** — they were queued, not
fixed (align-tasks.md Tasks 5 & 6 remain open, no source change landed). Filing an
align-task does not make a requirement compliant, so this report classifies FR-008
and FR-010 as **Drifted (partial)** alongside the already-tracked FR-014, giving
**21 aligned / 3 drifted**. In every case the *load-bearing* behavior of the FR is
fully implemented; only a secondary MUST clause (failure logging / error-variant
mapping / diagnostics sink) drifts. All three are **ALIGN** (fix the code, do not
weaken the spec) and all three are already tracked as align-tasks. All three are
reachable only under `--features rdma` on real hardware, so they are invisible to
the default-members / CI build.

Note on relocated crates: the spec/plan reference `component-framework`,
`component-core`, `component-macros` by **crate name only** (never by a
`components/…` path), and `components/interfaces` has not moved. No stale-path
drift from the components/→lib/ relocation was found.

---

## Spec: 001-rdma-lookup-responder — RDMA Lookup Responder

### Aligned ✓

| Requirement | Location |
|-------------|----------|
| FR-001 actor owns dedicated `rdma_cm` accept-loop thread | `src/lib.rs:319-331` (spawn `rdma-responder-accept` → `run_accept_loop`) |
| FR-002 bind ephemeral port 0 on effective IP, `rdma_listen`, read port via `rdma_get_src_port`, not by name, `Bind` on missing/unusable IP, twice→`AlreadyInitialized` | `src/rdma.rs:239-268`; `src/lib.rs:275-278` |
| FR-002a bind-IP precedence (explicit `set_bind_ip` else auto-detect first active device) | `src/lib.rs:181-183,285-290`; `src/rdma.rs:214-219,76-129` |
| FR-003 `local_endpoint()` returns bound `{ip,port}` after init, `NotInitialized` before | `src/lib.rs:389-393` |
| FR-004 accept loop epolls `{cm fd, command eventfd, stop eventfd, async fd}` together; SPSC→eventfd command bridge | `src/rdma.rs:511-558` (TAG_CM/TAG_CMD/TAG_STOP); bridge `src/rdma.rs:358-373` |
| FR-005 read UUID from `private_data`, key by `PeerId`, accept, emit `ConnectionEstablished{Some}` | `src/connection.rs:136-160`; `src/rdma.rs:410-428,634-641` |
| FR-006 absent/malformed `private_data` accepted as `node:None`, reclaimable only via shutdown | `src/connection.rs:161-169,246-255` |
| FR-007 `Active→Draining→Dead` state machine; new connects refused while Draining | `src/connection.rs:82-91,143-149` |
| FR-009 never reads/copies value bytes (control traffic only) | design-wide; no data-path code (`src/connection.rs`, `src/rdma.rs`) |
| FR-010 (core) register whole pool once (`ibv_reg_mr`), expose via `local_region()`, dereg before PD freed; precondition `Registration` on unbound receptacle / uninitialized pool | `src/rdma.rs:293-314,570-573`; `src/lib.rs:191-211,395-399` — **error-variant sub-clause drifts, see below** |
| FR-011 single-client control channel; 2nd open→`ChannelClosed` | `src/lib.rs:372-387` |
| FR-011a lossless event delivery via backpressure (blocking send) | `src/lib.rs:129-132`; test `src/lib.rs:539-579` |
| FR-012 `set_actor_cpu` pins accept thread | `src/lib.rs:177-179,317,324-328` |
| FR-013 `signal_stop` no-join; `shutdown` stops+joins+tears-down, idempotent | `src/lib.rs:221-254` |
| FR-015 unit tests: lifecycle, error paths, state machine | `src/lib.rs:402-643`; `src/connection.rs:320-511` |
| FR-016 telemetry behind feature, ZST no-op when off, metric set matches, `record_accept_loop_error` wired into production | `src/telemetry.rs`; `Cargo.toml:10`; wired `src/lib.rs:160-166`, `src/rdma.rs:420-427`, `src/connection.rs:216-218` |
| SC-001 endpoint non-placeholder port + `NotInitialized` before, unit-tested | `src/rdma.rs:263-268`; tests `src/lib.rs:409-452` |
| SC-002 exactly one `DisconnectAck`, QP→ERROR ordered before ack | test `src/connection.rs:401-418` |
| SC-003 idle loop services command event-driven (no poll cycle) | `src/connection.rs:301-318`; test `src/connection.rs:462-481` |
| SC-004 co-resident instances bind distinct ephemeral ports (port-0 mechanism; hw-gated) | `src/rdma.rs:241-268`; mock sanity test `src/lib.rs:628-643`; hw `src/loopback_test.rs` |
| SC-005 known UUID→`Some(peer)`, empty→`None` | tests `src/connection.rs:354-380` |
| SC-006 telemetry <5% overhead via Criterion two-run | `benches/connection_telemetry.rs` |

### Drifted ⚠️

All three are partial drifts: the load-bearing behavior of the FR is fully
implemented; a secondary MUST sub-clause is unmet. Direction is **ALIGN** for all
three (fix the code; the spec is correct and load-bearing). Each is already tracked
as an align-task and is only reachable under `--features rdma`.

| Requirement | Spec text (unmet clause) | Actual | Location | Severity |
|-------------|--------------------------|--------|----------|----------|
| FR-008 | "Freeing the queue pair (`rdma_destroy_qp`) is best-effort cleanup … its failure MUST be logged, not fatal." | Load-bearing part aligned: QP→ERROR is asserted (fail-stop) **before** the ack, and unknown/dead nodes are acked idempotently (`src/connection.rs:181-195`, `src/rdma.rs:144-152`). **But** `RealCmConn::drop` calls `rdma_disconnect`/`rdma_destroy_qp`/`rdma_destroy_id` and ignores every return code — nothing is logged on a destroy failure, so the "its failure MUST be logged" clause is unmet. | `src/rdma.rs:154-169` | Low |
| FR-010 | "If … `ibv_reg_mr` fails, `initialize()` MUST return `Registration`." | Register-once / expose / dereg and the precondition `Registration` paths (unbound receptacle, uninitialized pool) are all aligned (`src/lib.rs:191-211`). **But** `RealCmSeam::bind` returns `Err(String)` for *all* real-CM failures, mapped uniformly via `.map_err(RemoteLookupRdmaResponderError::Bind)`, so a genuine `ibv_reg_mr` failure surfaces to the caller as `Bind`, not `Registration`. | `src/rdma.rs:300-309` (reg_mr `Err`) → `src/lib.rs:203` (uniform `Bind` map) | Medium |
| FR-014 | "Diagnostics MUST route through an optional `ILogger` receptacle; a missing logger MUST NOT turn any operation into an error." | Primary diagnostics route through `ILogger` via `log_debug` and tolerate a missing logger (aligned — `src/lib.rs:116-120`, test `lifecycle_succeeds_with_no_logger_bound`). **But** the device async-event instrumentation prints via `eprintln!`, bypassing `ILogger`; `src/rdma.rs` holds no logger reference. Spec's Known Limitations flags this and tracks an align-task. | `src/lib.rs:116-120` (aligned path); `src/rdma.rs:459-464` (`eprintln!` bypass) | Low |

### Not Implemented ✗

None.

---

## Unspecced Code

| Feature | Location | Lines | Note |
|---------|----------|-------|------|
| Device async-event instrumentation (TAG_ASYNC epoll fd, `drain_async_events`, `async_event_name`, FFI `responder_async_fd`/`responder_drain_async_event`) | `src/rdma.rs`, `src/ffi.rs`, `src/wrapper.c` | rdma.rs:41,44-70,351-356,440-466; ffi.rs:294-302; wrapper.c | No FR mandates it — best-effort operator diagnostics. Already documented in spec Known Limitations (backfilled 2026-08-07); no new spec needed. Emits via `eprintln!` (see FR-014 drift). |
| Command-bridge thread (`rdma-responder-cmd-bridge`) draining the SPSC command inbox onto the command eventfd | `src/rdma.rs` | 358-373 | Implementation mechanism enabling FR-004 (SPSC channel has no pollable fd). Documented in spec FR-004 note + data-model (backfilled 2026-08-20). No new spec needed. |

---

## Conflicts

None. (The July stale-`set_bind_ip` interface-doc conflict is tracked as a code-side
align-task against `components/interfaces/**`, which is outside this component's
spec-sync write scope and is never edited by this pass.)

---

## Recommendations

1. **FR-010 (Medium):** Split `RealCmSeam::bind`'s error channel so an `ibv_reg_mr`
   failure (`src/rdma.rs:300-309`) is routed to `RemoteLookupRdmaResponderError::Registration`
   at `src/lib.rs:203`, while listen/`rdma_get_src_port` failures still map to `Bind`.
   Tracked as align-tasks.md Task 5.
2. **FR-014 (Low):** Route the async-event diagnostics in `drain_async_events`
   (`src/rdma.rs:459-464`) through the `ILogger` receptacle instead of `eprintln!`.
   Tracked as align-tasks.md (FR-014 async-event task, 2026-08-20 section).
3. **FR-008 (Low):** Capture the `rdma_disconnect`/`rdma_destroy_qp` return codes in
   `RealCmConn::drop` (`src/rdma.rs:154-169`) and log on nonzero return (via the same
   accept-loop logger handle FR-014 needs, or `eprintln!` as an interim), without
   changing the QP→ERROR-before-ack ordering or making `Drop` fallible. Tracked as
   align-tasks.md Task 6.
4. **Unspecced items:** No action — both are best-effort/mechanism details already
   captured in the spec (Known Limitations / FR-004 note / data-model). Promote to an
   FR only if either becomes load-bearing for operators.
5. No spec/implementation conflicts and no stale relocated-crate paths — the component
   is otherwise fully synchronized with spec 001-rdma-lookup-responder.
