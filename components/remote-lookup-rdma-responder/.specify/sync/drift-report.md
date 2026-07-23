# Spec-Drift Report — `remote-lookup-rdma-responder`

**Generated**: 2026-07-22T22:39:05Z
**Specs analyzed**: 1 (`specs/001-rdma-lookup-responder/spec.md`)
**Requirements checked**: 25 — 18 FR (FR-001..FR-016, FR-002a, FR-011a) + 6 SC (SC-001..SC-006) + 1 contract-doc protocol rule (`ResponderEvent::Error`)

## Summary

| Metric | Count |
|---|---|
| Specs analyzed | 1 |
| Requirements checked | 25 |
| Aligned | 22 |
| Drifted | 3 |
| Not implemented | 0 |
| Unspecced features | 1 |
| Cross-doc conflicts | 1 |

The implementation is a faithful build of the spec's "skeleton first" plan: the
actor, control channel, `Active → Draining → Dead` state machine,
teardown-before-ack ordering, lossless event delivery, NUMA pinning, the real
`rdma_cm`/`ibv_reg_mr` path behind the `rdma` feature, and the hardware-gated
loopback tests all match their FRs/SCs closely, with real code citations for
every acceptance scenario. Drift is concentrated in the **error/diagnostics
path**: the spec's `ResponderEvent::Error` variant, the `accept_loop_errors`
telemetry counter, and broad `ILogger`-routed diagnostics (FR-014) are declared
but never wired into the accept loop's actual failure branches.

## Per-Spec Findings — `001-rdma-lookup-responder` ("RDMA Lookup Responder")

### Aligned (15)

| Requirement | Evidence |
|---|---|
| FR-001 (dedicated actor thread) | `src/lib.rs:312-324` spawns `"rdma-responder-accept"` thread in `initialize_inner` |
| FR-002 (bind port 0, `rdma_listen`, `rdma_get_src_port`, no device-by-name, `Bind`/`AlreadyInitialized`) | `src/rdma.rs:178-350` (`RealCmSeam::bind`); `src/lib.rs:267-272` (`AlreadyInitialized` guard) |
| FR-002a (precedence: `set_bind_ip` else auto-detect first active device) | `src/lib.rs:174-176,278-283`; `src/rdma.rs:46-99,185-189` (`first_active_rdma_ipv4`) |
| FR-003 (`local_endpoint()` / `NotInitialized`) | `src/lib.rs:382-386` |
| FR-004 (epoll over `{cm fd, cmd inbox, stop}`) | `src/rdma.rs:292-314,438-480` (epoll + tagged fds); mirrored by `MockCmSeam::next_events`, `src/connection.rs:274-294` |
| FR-005 (identify by `private_data` → `PeerId`, emit `ConnectionEstablished{Some}`) | `src/connection.rs:127-151,227-236` |
| FR-006 (unidentified → `node: None`, reclaimable only via shutdown) | `src/connection.rs:152-161,188-199` |
| FR-007 (`Active → Draining → Dead`; refuse connect while `Draining`) | `src/connection.rs:74-97,132-140` |
| FR-008 (QP→ERROR before ack, asserted fail-stop, idempotent) | `src/connection.rs:172-186`; `src/rdma.rs:114-122` (`assert_eq!` on `ibv_modify_qp`) |
| FR-009 (no touching value bytes) | No read/copy of pool bytes anywhere in `src/**` — writes land via the peer's one-sided RDMA write into the MR registered in `src/rdma.rs:263-284` |
| FR-010 (register whole pool once via `IMemoryTier::pool_info`, expose via `local_region()`, dereg on drop, `Registration` error) | `src/lib.rs:183-203,388-392`; `src/rdma.rs:263-284,482-510` |
| FR-011 (single-client channel, `ChannelClosed` on 2nd open) | `src/lib.rs:365-380` |
| FR-011a (lossless/backpressure event delivery) | `src/lib.rs:130-132` (`send_event`) + `component-core` `Sender::send` blocks/spins/parks rather than dropping (`component-core/src/channel/mod.rs:205-230`); covered by `event_delivery_is_lossless_under_backpressure` test (`src/lib.rs:533-572`) |
| FR-012 (`set_actor_cpu` honored pre-`initialize`, NUMA pin) | `src/lib.rs:170-172,310,317-321` |
| FR-013 (`signal_stop` no-join / `shutdown` join+teardown+idempotent) | `src/lib.rs:214-247` |
| FR-015 (unit tests: lifecycle, error paths, state machine) | `src/lib.rs:395-590`, `src/connection.rs:296-487` |
| SC-001..SC-005 | Covered by mock-seam unit tests and the hardware `#[ignore]`d loopback tests (`src/loopback_test.rs:304-404`), consistent with the spec's own "Skeleton first, hardware loop later" limitation |
| SC-006 (telemetry <5% overhead, two-baseline Criterion workflow) | `benches/connection_telemetry.rs` implements exactly the documented `--save-baseline off` / `--baseline off` workflow |

*(The 22 aligned items = 16 FR numbers FR-001..FR-013/FR-015 + FR-002a + FR-011a, plus all 6 SCs; the 3 drifted items below are FR-014, FR-016, and the contract-doc `ResponderEvent::Error` protocol rule.)*

### Drifted (3)

| Requirement | Spec text | Actual | Location | Severity |
|---|---|---|---|---|
| FR-016 (telemetry metric set incl. "accept-loop errors") | "Metric set: inbound connections accepted, identified vs unidentified (`node: None`), teardowns (disconnect-acks emitted), and **accept-loop errors**." Also `tasks.md` T023 claims this was wired: "Wire `TelemetryCollector` call sites... **accept-loop errors**, in `src/connection.rs` + `src/lib.rs`". | `TelemetryCollector::record_accept_loop_error()` / `accept_loop_errors()` are defined and unit-tested in isolation, but **no production code path ever calls `record_accept_loop_error()`**. `RealCmSeam::drain_cm_events`'s failure branch (QP creation failure → `rdma_reject`) and `RealCmSeam::bind`'s many early-return error paths never touch the collector. The counter is permanently 0 in a running system. | `src/telemetry.rs:58-60,107` (defined); no call site in `src/rdma.rs` or `src/connection.rs` (verified via `grep -rn record_accept_loop_error src/` — only hits in `telemetry.rs` itself and its own test at `src/telemetry.rs:139`) | Medium — the metric is dead code; an operator relying on FR-016's documented metric set for capacity/error diagnosis gets no signal from it |
| Contract §3 / spec Key Entities (`ResponderEvent::Error`) | `specs/001-rdma-lookup-responder/contracts/responder-control-interface.md:66-67`: "`Error { message }` reports a **non-fatal** accept-loop error." Spec Key Entities (spec.md:378-379) list `Error { message }` as part of `ResponderEvent`. | `ResponderEvent::Error` is never constructed anywhere in the crate (`grep -rn "ResponderEvent::Error" src/` → no hits). The one candidate non-fatal failure in production code — `RealCmSeam::drain_cm_events`'s QP-creation-failure branch, which calls `rdma_reject` and silently drops the connection — reports nothing to `remote-lookup` and does not log. | `src/rdma.rs:373-382` (`accept_child` error path: `Err(_) => { ffi::rdma_reject(...) }`, no `Error` event, no log, no telemetry) | Medium — `remote-lookup` has no way to observe accept-loop errors at all, contradicting the documented control-channel protocol |
| FR-014 (diagnostics MUST route through `ILogger`) | "Diagnostics MUST route through an optional `ILogger` receptacle; a missing logger MUST NOT turn any operation into an error." | Only two call sites exist: `initialize()` success and `shutdown()` (`log_debug("rdma responder initialized")` / `"...shut down"`). The accept-loop closure spawned in `initialize_inner` (`src/lib.rs:312-324`) captures no reference to `self.logger`, so no diagnostic from inside `run_accept_loop`, `RealCmSeam::next_events`/`drain_cm_events`, or `accept_child`'s failure branch can ever reach the logger. The "MUST NOT turn a missing logger into an error" half is satisfied (verified by `lifecycle_succeeds_with_no_logger_bound`), but the "MUST route diagnostics through it" half is only true for 2 of the many diagnostic-worthy events in the module. | `src/lib.rs:115-120,244,334` (only call sites); `src/rdma.rs` has zero `ILogger`/logger references | Low-Medium — functionally harmless (nothing becomes an error) but the requirement's affirmative half is under-implemented |

### Not implemented (0)

None — every FR/SC has at least a partial, spec-consistent implementation; the three drift items above are gaps in the error/diagnostics surface of otherwise-implemented requirements, not missing features.

## Unspecced Code

| Feature | Location | Lines | Suggested spec |
|---|---|---|---|
| Cargo feature `rdma` gating the entire real `rdma_cm`/`ibv_reg_mr` implementation, with `initialize()` returning a hard-coded `Bind` error when the crate is built without it | `Cargo.toml:11-15`; `src/lib.rs:37-46,206-212` | Cargo.toml L11-15, lib.rs L40-46 (mod gating), L206-212 (`not(feature="rdma")` stub) | Add a short "Build & feature flags" subsection to spec.md (or the plan/contract doc) naming the `rdma` feature explicitly and documenting that `initialize()` without it is a build-configuration error, not one of the FR-002/FR-010 runtime failure modes it currently gets lumped under (`RemoteLookupRdmaResponderError::Bind`) |

## Cross-Document Conflicts

| Conflict | Locations | Detail |
|---|---|---|
| Interface doc contradicts FR-002a and the actual auto-detect behavior | `components/interfaces/src/iremote_lookup_rdma_responder.rs:256-262` vs. `specs/001-rdma-lookup-responder/spec.md` FR-002a (lines 297-305) vs. `src/rdma.rs:184-189` and `src/lib.rs:421-429` (test `initialize_without_bind_ip_defers_to_autodetect`) | The `IRemoteLookupRdmaResponderAdmin::set_bind_ip` doc comment states: *"The responder never auto-detects the address... If no IP was supplied (or it is unusable on this host), `initialize()` fails with `Bind`."* This is the **pre-clarification** behavior; the shipped spec (FR-002a, added in the 2026-07-10 clarify session) and the actual code both implement auto-detection of the first active RDMA device when no IP is supplied. The doc comment on the shared `interfaces` crate — which is the "source of truth" cited by `contracts/responder-control-interface.md:9` — is stale and actively misleading to any `remote-lookup` or mainline author reading only the interface docs. |

## Recommendations

1. **Wire `accept_loop_errors` and `ResponderEvent::Error`** into `RealCmSeam`'s actual failure branches (at minimum: `accept_child` failure in `drain_cm_events`, `src/rdma.rs:373-382`) so the telemetry counter and the documented control-channel protocol rule are not dead code. This closes the FR-016 and contract-§3 drift together, since both consume the same failure signal.
2. **Route accept-loop diagnostics through `ILogger`.** Either pass a cloned/`Arc` handle to the logger receptacle into the accept-loop closure, or have `run_accept_loop` return diagnostic strings that the caller (which does have `self.logger`) logs — closing the FR-014 gap for the accept-loop's internal failure paths.
3. **Fix the stale doc comment** on `IRemoteLookupRdmaResponderAdmin::set_bind_ip`/`initialize` in `components/interfaces/src/iremote_lookup_rdma_responder.rs` to describe the auto-detect fallback (FR-002a), matching both the spec and the shipped behavior.
4. **Name the `rdma` Cargo feature in the spec/plan/contract docs** so the build-configuration failure mode (`initialize()` → `Bind` when built without `rdma`) is traceable to a requirement rather than only to `tasks.md`/README prose.
