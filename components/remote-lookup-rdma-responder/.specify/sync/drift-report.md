Generated: 2026-08-07T15:28:11Z

# Spec-vs-Implementation Drift Report — remote-lookup-rdma-responder

Spec analyzed: `specs/001-rdma-lookup-responder/spec.md`
Implementation: `src/{lib.rs,connection.rs,rdma.rs,ffi.rs,telemetry.rs,loopback_test.rs}`, `benches/connection_telemetry.rs`
Interface: `components/interfaces/src/iremote_lookup_rdma_responder.rs`

## Summary

| Spec | Requirements | Aligned | Drifted | Not Implemented |
|------|-------------:|--------:|--------:|----------------:|
| 001-rdma-lookup-responder | 23 (17 FR + 6 SC) | 19 | 4 | 0 |

Unspecced code items: 2. Conflicts: 1 (stale interface doc).

## Detailed Findings

### Functional Requirements

| ID | Status | Severity | Evidence | Note |
|----|--------|----------|----------|------|
| FR-001 | Aligned | — | `src/lib.rs:312-323` | Dedicated `rdma-responder-accept` thread runs `run_accept_loop`. |
| FR-002 | Aligned | — | `src/rdma.rs:239-268` | Binds `htons(0)` (ephemeral), `rdma_listen`, reads back `rdma_get_src_port`; `AlreadyInitialized` guarded at `src/lib.rs:268`. |
| FR-002a | Aligned | — | `src/lib.rs:278-283`, `src/rdma.rs:215-219,76-129` | Explicit `set_bind_ip` else `first_active_rdma_ipv4()` auto-detect. (`CERTUS_RDMA_BIND_IP` wiring is in the `certus-server-yaml` mainline, out of this crate.) |
| FR-003 | Aligned | — | `src/lib.rs:382-386` | Returns bound `{ip,port}`; `NotInitialized` before init. |
| FR-004 | Aligned | — | `src/rdma.rs:507-554,342-344` | `epoll` over `{cm fd, cmd eventfd, stop eventfd}`; SPSC command inbox bridged to `cmd_eventfd`. Mock uses event-driven `recv`. |
| FR-005 | Aligned | — | `src/rdma.rs:410-424`, `src/connection.rs:127-162`, `src/lib.rs:150-153` | Reads `private_data`, keys table by `PeerId`, accepts, emits `ConnectionEstablished{Some}`. |
| FR-006 | Aligned | — | `src/connection.rs:152-160,227-236` | Absent/malformed → unidentified side-list, `Established(None)`; only `teardown_all`/`shutdown` reclaims. |
| FR-007 | Aligned | — | `src/connection.rs:74-82,134-140` | `Active→Draining→Dead`; connect refused while `Draining`. (Draining is transient within the single-threaded loop; the refusal path is exercised via `force_state` in tests.) |
| FR-008 | Drifted | Low | `src/rdma.rs:144-169` | QP→ERROR ordered before ack and asserted (`src/connection.rs:172-186`, `src/rdma.rs:145-151`); idempotent for unknown node. **Drift:** spec says best-effort `rdma_destroy_qp` failure "MUST be logged"; `Drop` ignores `rdma_disconnect`/`rdma_destroy_qp` return values and logs nothing. |
| FR-009 | Aligned | — | (whole crate) | No data path anywhere; responder is registrar/connection-manager only. |
| FR-010 | Drifted | Medium | `src/lib.rs:183-204`, `src/rdma.rs:300-314,557-575` | Registers whole pool once, exposes `local_region`, deregisters MR before PD free. **Drift:** FR-010 says `ibv_reg_mr` failure MUST return `Registration`, but `RealCmSeam::bind` returns `Err(String)` mapped via `.map_err(RemoteLookupRdmaResponderError::Bind)` (`src/lib.rs:195-196`), so a registration failure surfaces as `Bind`, not `Registration`. (Unbound receptacle / uninitialized pool are correctly mapped to `Registration`.) |
| FR-011 | Aligned | — | `src/lib.rs:365-380` | Single-client channel; second `open_control_channel` → `ChannelClosed`. |
| FR-011a | Aligned | — | `src/lib.rs:129-132,533-572` | `send_event` uses blocking `tx.send` (backpressure, never drop); lossless-under-backpressure test present. |
| FR-012 | Aligned | — | `src/lib.rs:170-172,310-321` | `set_actor_cpu` stored pre-init; thread pins via `set_thread_affinity` (best-effort). |
| FR-013 | Aligned | — | `src/lib.rs:214-247,166` | `signal_stop` exits without join; `shutdown` stops+joins, `teardown_all` tears down connections, idempotent via `state.take()`. |
| FR-014 | Drifted | Low | `src/lib.rs:116-120`, `src/rdma.rs:455-461` | Diagnostics route through optional `ILogger`; missing logger is not an error. **Drift:** the async-event diagnostic path logs via `eprintln!` directly, bypassing the `ILogger` receptacle. |
| FR-015 | Aligned | — | `src/lib.rs:395-590`, `src/connection.rs:296-486` | Unit tests cover lifecycle, `NotInitialized`/`AlreadyInitialized`/`ChannelClosed`, and the state machine. |
| FR-016 | Drifted | Low | `src/telemetry.rs`, `src/connection.rs:148-158,184` | Feature-gated ZST no-op telemetry with the required metrics is present and wired for accepted/identified/unidentified/teardowns. **Drift:** the "accept-loop errors" metric (`record_accept_loop_error`) is never called and `ResponderEvent::Error` is never emitted anywhere, so that metric is defined but never recorded. |

### Success Criteria

| ID | Status | Severity | Evidence | Note |
|----|--------|----------|----------|------|
| SC-001 | Aligned | — | `src/lib.rs:402-409,431-445`, `src/loopback_test.rs:304-315` | `NotInitialized` before init tested; non-placeholder ephemeral port asserted only in the `#[ignore]` hardware loopback test (mock uses port 0). |
| SC-002 | Aligned | — | `src/connection.rs:377-394` | Exactly one ack; QP→ERROR ordered strictly before ack. |
| SC-003 | Aligned | — | `src/connection.rs:438-457` | Structural: command serviced on event-driven wake with no pending connect, no poll cycle. |
| SC-004 | Aligned | — | `src/loopback_test.rs:317-332`, `src/lib.rs:574-589` | Distinct-ephemeral-port distinctness validated in `#[ignore]` hardware test; mock test only asserts independent IPs (both port 0). |
| SC-005 | Aligned | — | `src/connection.rs:330-356`, `src/loopback_test.rs:334-386` | Known UUID → `Some(peer)`; empty `private_data` → `None`. |
| SC-006 | Aligned | — | `benches/connection_telemetry.rs` | Criterion benchmark drives accept/disconnect over the seam; two-baseline on/off workflow documented. Actual <5% is a runtime measurement, not asserted in code. |

## Unspecced Code

| Item | Location | Note |
|------|----------|------|
| Device async-event instrumentation | `src/rdma.rs:44-70,351-356,437-462`; `src/ffi.rs:296-302`; `src/wrapper.c` | `TAG_ASYNC` epoll wiring + `drain_async_events`/`async_event_name` + `responder_async_fd`/`responder_drain_async_event` shims log QP_FATAL/QP_REQ_ERR/etc. No FR mentions async-event diagnostics. |
| MR access widened beyond `REMOTE_WRITE` | `src/rdma.rs:297-299` | `ibv_reg_mr` uses `LOCAL_WRITE | REMOTE_WRITE | REMOTE_READ`; FR-010 specifies only `REMOTE_WRITE`. `REMOTE_READ` grants peers one-sided read of the pool, which no requirement calls for. |

## Conflicts / Stale References

| Note | Location |
|------|----------|
| Interface trait doc contradicts the current spec. `set_bind_ip`'s doc states "The responder never auto-detects the address" and that `initialize` fails with `Bind` if no IP was supplied — the pre-clarification behavior. The current spec (FR-002a) mandates auto-detect of the first active RDMA device, which the implementation actually follows. The interface doc is stale. | `components/interfaces/src/iremote_lookup_rdma_responder.rs:253-263` |

No spec references to nonexistent files/dirs/proofs were found (`benches/` exists; the backup spec under `.specify/sync/backups/` was excluded as instructed).

## Recommendations

1. **FR-010 (Medium):** Map `ibv_reg_mr` failure to `RemoteLookupRdmaResponderError::Registration` rather than `Bind`. Either split `RealCmSeam::bind`'s error type or have it signal registration failures distinctly so `src/lib.rs:195-196` can route them to `Registration`.
2. **FR-010 / Unspecced (Low–Medium):** Either narrow the MR access flags to `REMOTE_WRITE` (drop `REMOTE_READ`) to match the spec, or add a requirement documenting why remote read access is needed.
3. **FR-008 (Low):** Log `rdma_disconnect`/`rdma_destroy_qp` failures in `RealCmConn::drop` (best-effort), as the spec requires.
4. **FR-014 (Low):** Route the async-event diagnostics through the `ILogger` receptacle instead of `eprintln!`, or document the async instrumentation in the spec as an explicit stderr diagnostic.
5. **FR-016 (Low):** Either wire `record_accept_loop_error` / emit `ResponderEvent::Error` on non-fatal accept-loop errors (e.g. the `accept_child` reject path at `src/rdma.rs:421-423`), or drop the unused metric/variant.
6. **Conflict:** Update the `set_bind_ip` interface doc to reflect FR-002a auto-detection so the trait documentation stops contradicting the spec and the code.
