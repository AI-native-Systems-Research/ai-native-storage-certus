# Spec Drift Report

Generated: 2026-07-22T21:31:30Z
Project: remote-lookup-rdma-initiator

## Summary

| Spec | Title | Status | Reqs Checked | Aligned | Drifted | Not Implemented |
|------|-------|--------|-------------:|--------:|--------:|-----------------:|
| 001-rdma-remote-request-handler | RDMA Remote Lookup Initiator | **Superseded** by 002 | 23 (17 FR + 6 SC) | 0 | 0 | 23 |
| 002-rdma-push-initiator | RDMA Push Initiator | Current | 18 (14 FR + 4 SC) | 17 | 1 | 0 |

Unspecced features found: **2**. Inter-spec conflicts: **0** (001 carries an explicit `SUPERSEDED` banner pointing at 002, so the former role-inversion conflict identified by an earlier drift pass is resolved).

This supersedes the prior report body dated 2026-07-09/2026-07-15 in this same file: both items that report had flagged (`connect()` warm-connect and the per-phase connect-latency telemetry) are now **present in `002-rdma-push-initiator/spec.md`** as FR-014 and the broadened FR-011, and the `integration-test` Cargo feature and the dead `RdmaListener`/`send_msg`/`recv_msg`/`post_rdma_write_unsignaled` primitives that report called out as unspecced have all been **removed** from `src/rdma.rs`/`src/ffi.rs`/`Cargo.toml`. Only two open items remain (see below), one of them new.

---

## Spec 001-rdma-remote-request-handler (SUPERSEDED)

`specs/001-rdma-remote-request-handler/spec.md` carries an explicit banner:
"SUPERSEDED (2026-07-09) ... reworked into an outbound initiator, and the
entire responder stack was removed." Its 17 functional requirements (FR-001
through FR-017) and 6 success criteria (SC-001 through SC-006) describe an
**inbound passive responder** (listener, per-connection sessions, protocol
handshake, ≤64-entry batch protocol, standalone test client, `serve::run_blocking`)
that no longer exists in this crate's source tree.

### Aligned
_None._

### Drifted
_None._ (Spec explicitly disclaims itself; there is no live requirement to drift against.)

### Not Implemented (obsolete by design — role replaced by spec-002)

| Requirement | Spec text (summary) | Actual |
|---|---|---|
| FR-001..FR-003, FR-008, FR-009 | Inbound listener port, version-handshake session lifecycle, close/disconnect-event cleanup | No listener/session code exists in `src/`; `push`/`connect`/`disconnect` are outbound-only (`src/connection.rs`, `src/lib.rs`) |
| FR-004..FR-006, FR-016 | ≤64-entry batch protocol, serial-resolve + unsignaled-pipelined writes, custom binary wire protocol | Removed; `push` takes a plain `&[(CacheKey, RemoteRegion)]` slice with no batch-size cap or custom framing (`src/lib.rs:148-186`) |
| FR-007 | Transfer results into caller remote memory | Survives in spirit via `002` FR-003 (`RdmaConn::write` / `rdma_write_from_pool`, `src/rdma.rs:205-228`) |
| FR-010 | Delegate to a dispatch-service placeholder | No dispatch service; resolution is direct `IMemoryTier::peek` (`src/lib.rs:163`) |
| FR-011, FR-017 | Logging interface; network-trust-only security | Survive generically — `ILogger` receptacle (`src/lib.rs:75`) and no application-level auth — reshaped into `002` FR-010/FR-012 |
| FR-012 | Standalone test client program | No such binary in this crate; `src/loopback_test.rs` is a `#[cfg(test)]`, `#[ignore]`d integration test, not a standalone client |
| FR-013 | Unit tests for session/protocol/listener/RDMA-mock | Reshaped into `002` FR-013 (connection-table/status/mock-transport/telemetry tests, `src/connection.rs` `mod tests`) |
| FR-014, FR-015 | Optional telemetry; "full-remote" profile stub methods | Reshaped into `002` FR-011/FR-014; `push`/`connect` are now fully functional (not stubs) behind the `rdma` feature |
| SC-001..SC-006 | ≤500µs 64-entry batch; 100 concurrent sessions; test-client clean run; 1s disconnect cleanup; <5% telemetry overhead; handshake-reject latency | Not applicable to the push-initiator shape; only the telemetry-overhead theme carries forward as `002` SC-004 |

---

## Spec 002-rdma-push-initiator (current)

### Aligned ✓

| Requirement | Spec text (summary) | Evidence |
|---|---|---|
| FR-001 | `push(endpoint, items) -> Result<Vec<PushStatus>, ...>`, one status per item, in order | `interfaces/src/iremote_lookup_rdma_initiator.rs:123-127`; `src/lib.rs:148-186` |
| FR-002 | Resolve via `IMemoryTier::peek`; absent→`KeyNotFound`, size-mismatch→`SizeMismatch`, before any write | `src/lib.rs:161-178` |
| FR-003 | RDMA-write into `region.addr`/`region.rkey` | `src/rdma.rs:205-228` (`rdma_write_from_pool`), invoked from `src/connection.rs:241-247` |
| FR-004 | Register memory-tier pool as an RDMA MR once per connection | `src/connection.rs:463-484` (`RealTransport::connect` calls `register_existing_mr` on every new connect) — corroborated by `tests/mr_registration_bench.rs` header, which explicitly describes this as "re-registers the whole pool *per connection*" |
| FR-005 | Connection table keyed by normalized `"ip:port"`, states disconnected/connecting/connected/disconnecting, lazy + reused | `src/connection.rs:128-178` (`ConnState`, `HostSlot`, `ConnectionTable`) |
| FR-006 | Different hosts concurrent; same host serialized on its slot | `src/connection.rs:10-20` (module doc), `push`/`connect` both lock `slot.state` (`src/connection.rs:206`, `314`) |
| FR-007 | QP-error/failed-write → reconnect once → retry batch; second failure → `UnableToConnect` | `src/connection.rs:215-284` (`reconnect_used` flag in `push`) — unit-tested by `write_failure_triggers_single_reconnect_then_succeeds` (`src/connection.rs:658-673`) |
| FR-008 | `disconnect`/`disconnect_all`, idempotent | `src/connection.rs:329-363`; unit-tested (`disconnect_forces_fresh_connection`, `disconnect_before_any_push_is_noop` in `src/lib.rs:266-270`) |
| FR-009 | Parse `"ip:port"`, `InvalidEndpoint` otherwise | `src/connection.rs:418-433` (`parse_endpoint`) |
| FR-010 | Diagnostics via optional `ILogger`, no-op when unbound | `src/lib.rs:56-68, 180-184` (`NoopLogger`/`NOOP_LOGGER`) |
| FR-011 | Telemetry feature-gated, ZST when off; metric set incl. per-phase connect-latency breakdown with running average | `src/telemetry.rs` (full struct + no-op mirror), phase timing sourced from `src/rdma.rs:376-517` (`CmTiming`) and `src/connection.rs:388-403` |
| FR-012 | Network-level trust only; no app-level auth | No auth code anywhere in the crate; documented in `README.md:69-72` |
| FR-013 | Unit tests: connection-table state machine, `PushStatus` mapping, mock transport seam, telemetry wiring | `src/connection.rs:512-849` (`mod tests`, incl. `#[cfg(feature = "telemetry")]` cases) |
| FR-014 | `connect(endpoint)` warm-connect: idempotent, caching, `Ok(())`+nothing-cached on failure, `NotInitialized`/`InvalidEndpoint` errors | `src/lib.rs:196-209`, `src/connection.rs:291-326`; unit-tested (`warm_connect_establishes_and_push_reuses`, `warm_connect_failure_is_ok_and_caches_nothing`, `src/connection.rs:730-767`) |
| SC-001 | One `PushStatus` per item, in order, correct terminal statuses | `statuses_mapped_in_order`, `connect_failure_yields_unable_to_connect_for_writes_only` (`src/connection.rs:615-655`) |
| SC-002 | Reused connection avoids a second CM connect | `reused_connection_does_not_reconnect` (`src/connection.rs:700-718`) |
| SC-003 | Exactly one reconnect-and-retry before `UnableToConnect` | `write_failure_triggers_single_reconnect_then_succeeds` (`src/connection.rs:657-673`) |

### Drifted ⚠️

- **SC-004** — telemetry-overhead acceptance criterion: **stale pass/fail wording left in the benchmark harness's own doc comment.**
  - **Spec text**: SC-004 was revised (2026-07-15 measurement) to state the naive "< 5% vs disabled" framing is "superseded" and that "the criterion is therefore 'small fixed absolute cost / ZST-when-off', not a percentage against the mock" — because the mock push is a ~200–700ns no-op, so a few atomics legitimately read as 6–13% of the mock. `README.md:153-159` reflects this correctly.
  - **Actual**: `benches/push_telemetry.rs:1-18` still documents the pre-revision framing verbatim: *"SC-004 requires that enabling the telemetry feature adds less than 5% overhead to push versus the disabled build ... SC-004 holds when every push/* case is within +5%."* This contradicts the spec's own note that a straight <5% read against the mock will *not* hold "by construction," and could mislead a future reader running the benchmark into treating a 6-13% delta as a regression.
  - **Location**: `components/remote-lookup-rdma-initiator/benches/push_telemetry.rs:1-18` vs. `specs/002-rdma-push-initiator/spec.md` SC-004 (lines 260-273).
  - **Severity**: minor (doc-comment only; the benchmark's mechanics and the shipped behavior both match the spec — only the stated pass/fail bar in the comment is outdated).

### Not Implemented ✗
_None._ All 14 FRs and SC-001..SC-003 of spec-002 are implemented and exercised by tests; SC-004 is implemented and measured but is the drift above.

---

## Unspecced Code 🆕

| Feature | Location | Lines | Notes | Suggested spec |
|---|---|---:|---|---|
| `set_local_peer_id(PeerId)` + zyre-peer stamping into `rdma_cm` `private_data` | `interfaces/src/iremote_lookup_rdma_initiator.rs:161-168`; `src/lib.rs:81, 114-120, 223-228`; `src/connection.rs` (`RealTransport::new`/`local_peer_id`, `src/connection.rs:440-461`); `rdma.rs:386-495` (`client_connect(.., private_data)`, `rdma_connect` `conn_param.private_data`) | ~40 | A fully implemented, documented interface method (5th method on `IRemoteLookupRdmaInitiator`) that stamps this node's zyre `PeerId` into every outbound connect so the remote **responder** can correlate the inbound QP to a peer for its "teardown-before-reclaim" flow. Not mentioned anywhere in `002-rdma-push-initiator/spec.md` — no User Story, FR, or Key Entity references peer identification, `PeerId`, or the responder correlation protocol at all. | Add a User Story + FR (and a `PeerId`/correlation Key Entity) to `002-rdma-push-initiator/spec.md`, or a short cross-component note describing the initiator's half of the `remote-lookup-rdma-responder` teardown-before-reclaim contract. |
| `mr_registration_cost` hardware benchmark (single-MR-vs-per-connection investigation) | `components/remote-lookup-rdma-initiator/tests/mr_registration_bench.rs` (whole file) | 248 | An `#[ignore]`d, `--features rdma`-gated measurement tool (not a correctness test) that sweeps `ibv_reg_mr` cost by pool size/page type to inform a future decision on whether the per-connection pool MR re-registration (FR-004) should become a single shared MR. Its own header explicitly frames this as an open design trade-off, but no spec artifact (Known Limitations, a follow-up FR, etc.) in `002` mentions it. | Either fold a line into `002`'s "Known Limitations / Follow-ups" section referencing this investigation, or leave as internal tooling with a comment noting it is deliberately unspecced research, not a requirement. |

## Inter-Spec Conflicts

_None currently active._ 001 vs 002 previously conflicted (passive responder vs. outbound initiator describing "the same component"); 001 now carries an explicit `> ⚠️ SUPERSEDED (2026-07-09)` banner at its top pointing to 002, which resolves the ambiguity a reader would otherwise hit landing on 001 first.

## Recommendations

1. **Spec the peer-identification contract.** `set_local_peer_id` / private-data peer stamping is real, shipped, cross-component behavior (it's the initiator's half of the responder's teardown-before-reclaim design) and belongs in `002-rdma-push-initiator/spec.md` as its own FR + Key Entity, not just interface doc comments.
2. **Fix the stale SC-004 comment in `benches/push_telemetry.rs`.** Update its header to match the spec's revised framing ("small fixed absolute cost, ZST when off" rather than a literal <5%-of-mock gate) so a future contributor running the benchmark doesn't chase a bar the spec itself says cannot be met by construction.
3. **Optionally note the MR-registration investigation** (`tests/mr_registration_bench.rs`) in spec-002's Known Limitations, since it is directly relevant to FR-004's "once per connection" registration choice.
4. **Consider physically relocating/archiving `specs/001-rdma-remote-request-handler/`** (e.g. under an `archive/` or `superseded/` subfolder) now that its banner is in place — this is optional polish, not required for correctness, since the banner already prevents confusion.
