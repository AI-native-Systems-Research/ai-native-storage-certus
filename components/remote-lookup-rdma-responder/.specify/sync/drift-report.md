Generated: pending
# Spec-vs-Implementation Drift Report — remote-lookup-rdma-responder

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 24 |
| Aligned | 23 |
| Drifted | 1 |
| Not Implemented | 0 |
| Unspecced | 2 |

Requirements = 18 functional (FR-001..FR-016 incl. FR-002a, FR-011a) + 6 success
criteria (SC-001..SC-006).

Spec status: **Draft** (with clarifications resolved through 2026-07-10 and
access-flag / async-event notes backfilled 2026-08-07). No requirement is marked
deferred or out-of-scope, so all are analyzed.

Note on relocated crates: the spec/plan reference `component-framework`,
`component-core`, `component-macros` by **crate name only** (never by a
`components/…` path), and `components/interfaces` has not moved. No stale-path
drift from the components/→lib/ relocation was found.

---

## Spec: 001-rdma-lookup-responder — RDMA Lookup Responder

### Aligned ✓

| Requirement | Location |
|-------------|----------|
| FR-001 actor owns dedicated `rdma_cm` accept-loop thread | `src/lib.rs:319-330` (spawn `rdma-responder-accept` → `run_accept_loop`) |
| FR-002 bind ephemeral port 0 on effective IP, `rdma_listen`, read port via `rdma_get_src_port`, not by name, `Bind` on failure, twice→`AlreadyInitialized` | `src/rdma.rs:239-268`; `src/lib.rs:275-278` |
| FR-002a bind-IP precedence (explicit `set_bind_ip` else auto-detect first active device) | `src/lib.rs:285-290`; `src/rdma.rs:214-219,76-129` |
| FR-003 `local_endpoint()` returns bound `{ip,port}` after init, `NotInitialized` before | `src/lib.rs:389-393` |
| FR-004 accept loop epolls `{cm fd, command eventfd, stop eventfd}` together | `src/rdma.rs:511-558` (TAG_CM/TAG_CMD/TAG_STOP); command bridge `src/rdma.rs:358-373` |
| FR-005 read UUID from `private_data`, key by `PeerId`, accept, emit `ConnectionEstablished{Some}` | `src/connection.rs:136-160`; `src/rdma.rs:410-428,634-641` |
| FR-006 absent/malformed `private_data` accepted as `node:None`, reclaimable only via shutdown | `src/connection.rs:161-169,246-255` |
| FR-007 `Active→Draining→Dead` state machine; new connects refused while Draining | `src/connection.rs:82-91,143-149` |
| FR-008 QP→ERROR (asserted, fail-stop) before ack, destroy QP best-effort, idempotent unknown | `src/connection.rs:181-195`; `src/rdma.rs:144-169` |
| FR-009 never reads/copies value bytes (control traffic only) | design-wide; no data-path code (`src/connection.rs`, `src/rdma.rs`) |
| FR-010 register whole pool once (`ibv_reg_mr`), expose via `local_region()`, dereg before PD freed, `Registration` on failure | `src/rdma.rs:293-314,570-573`; `src/lib.rs:186-211,395-399` |
| FR-011 single-client control channel; 2nd open→`ChannelClosed` | `src/lib.rs:372-387` |
| FR-011a lossless event delivery via backpressure (blocking send) | `src/lib.rs:129-132`; test `src/lib.rs:539-579` |
| FR-012 `set_actor_cpu` pins accept thread | `src/lib.rs:317,324-328` |
| FR-013 `signal_stop` no-join; `shutdown` stops+joins+tears-down, idempotent | `src/lib.rs:221-254` |
| FR-015 unit tests: lifecycle, error paths, state machine | `src/lib.rs:402-643`; `src/connection.rs:320-511` |
| FR-016 telemetry behind feature, ZST no-op when off, metric set matches | `src/telemetry.rs`; `Cargo.toml:10` |
| SC-001 endpoint non-placeholder port + `NotInitialized` before, unit-tested | `src/rdma.rs:263-268`; tests `src/lib.rs:409-452` |
| SC-002 exactly one `DisconnectAck`, QP→ERROR ordered before ack | test `src/connection.rs:401-418` |
| SC-003 idle loop services command event-driven (no poll cycle) | `src/connection.rs:301-318`; test `src/connection.rs:462-481` |
| SC-004 co-resident instances bind distinct ephemeral ports (port-0 mechanism; hw-gated) | `src/rdma.rs:241-268`; mock sanity test `src/lib.rs:628-643`; hw `src/loopback_test.rs` |
| SC-005 known UUID→`Some(peer)`, empty→`None` | tests `src/connection.rs:354-380` |
| SC-006 telemetry <5% overhead via Criterion two-run | `benches/connection_telemetry.rs` |

### Drifted ⚠️

| Requirement | Spec text | Actual | Location | Severity |
|-------------|-----------|--------|----------|----------|
| FR-014 | "Diagnostics MUST route through an optional `ILogger` receptacle; a missing logger MUST NOT turn any operation into an error." | Primary diagnostics route through `ILogger` via `log_debug` and tolerate a missing logger (aligned), **but** the async-event instrumentation prints via `eprintln!`, bypassing `ILogger`. Spec's Known Limitations flags this and tracks an align-task. | `src/lib.rs:116-120` (aligned path); `src/rdma.rs:459-464` (`eprintln!` bypass) | Low |

### Not Implemented ✗

None.

---

## Unspecced Code

| Feature | Location | Lines | Note |
|---------|----------|-------|------|
| Device async-event instrumentation (TAG_ASYNC epoll fd, `drain_async_events`, `async_event_name`, FFI `responder_async_fd`/`responder_drain_async_event`) | `src/rdma.rs`, `src/ffi.rs`, `src/wrapper.c` | rdma.rs:44-70,351-356,440-466; ffi.rs:296-302 | No FR mandates it — best-effort operator diagnostics. Already documented in spec Known Limitations (backfilled 2026-08-07); no new spec needed. Emits via `eprintln!` (see FR-014 drift). |
| Command-bridge thread (`rdma-responder-cmd-bridge`) draining the SPSC command inbox onto the command eventfd | `src/rdma.rs` | 358-373 | Implementation mechanism enabling FR-004 (SPSC channel has no pollable fd). Behavior is implied by FR-004; the bridge itself is an undocumented internal detail. |

---

## Conflicts

None.

---

## Recommendations

1. **FR-014 (Low):** Route the async-event diagnostics in `drain_async_events`
   (`src/rdma.rs:459-464`) through the `ILogger` receptacle instead of
   `eprintln!`, to fully satisfy FR-014. This is already tracked as an align-task
   (`.specify/sync/align-tasks.md`, FR-014 accept-loop diagnostics); close it or
   keep it as an accepted, documented deviation.
2. **Unspecced async instrumentation:** No action required — it is best-effort and
   already captured in the spec's Known Limitations. If it becomes load-bearing
   for operators, promote it to an FR.
3. **Command-bridge thread:** Consider a one-line mention in plan.md / data-model
   so the FR-004 mechanism (SPSC→eventfd bridge) is not an undocumented surprise.
4. No spec/implementation conflicts and no stale relocated-crate paths — the
   component is otherwise fully synchronized with spec 001-rdma-lookup-responder.
