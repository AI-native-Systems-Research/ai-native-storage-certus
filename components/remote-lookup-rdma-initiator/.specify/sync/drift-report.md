# Spec Drift Report

Generated: 2026-07-09T22:50:03Z
Project: remote-lookup-rdma-initiator

---

## Update 2026-07-15 (delta — warm-connect + connect telemetry)

Two additions landed since the analysis below (commit `06743cd`,
`feat(remote-lookup-rdma-initiator): add warm-connect + per-phase connect telemetry`).
Both are new relative to spec `002-rdma-push-initiator`:

### 🆕 Unspecced — warm `connect(endpoint)`
- New public interface method `IRemoteLookupRdmaInitiator::connect(&str)` — proactively
  establish (warm) a connection without writing, so a later `push` to the same endpoint hits the
  established-connection fast path. Idempotent/connection-caching like `push`; a connection that
  cannot be established returns `Ok(())` with nothing cached (next `connect`/`push` retries), so
  warming never surfaces a transient failure as an error. Without the `rdma` feature it returns
  `NotInitialized`.
  - Location: `interfaces/src/iremote_lookup_rdma_initiator.rs`, `src/lib.rs` `connect`,
    `src/connection.rs` `ConnectionTable::connect`
  - Driving use case: warm-at-discovery from `remote-lookup`, moving the multi-second cold
    connect off the poll-loop hot path.
  - Resolution: extend US2 (connection reuse and self-repair) with a warm-connect FR + acceptance
    scenario. **APPLIED** to `002-rdma-push-initiator/spec.md` (FR-014, US2 scenario 4).

### ⚠️ Drifted — FR-011 telemetry metric set
- Telemetry now additionally records a **per-phase connect-latency breakdown** (rdma_cm address
  resolution / route resolution / connect handshake / MR registration, µs) via
  `record_connect_phases`, `connect_samples`, `avg_connect_phases_us` — beyond FR-011's listed set.
  - Location: `src/telemetry.rs`; recorded from `src/connection.rs` `ensure_connected`.
  - Severity: minor (additive; used to retune `op_deadline`/`phase1_timeout` against measured latency).
  - Resolution: broaden FR-011's metric set. **APPLIED** to `002-rdma-push-initiator/spec.md`.

(The Jul 09 analysis below remains valid for FR-001..FR-013/SC-001..SC-003 and the superseded
spec-001 findings.)

---

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 2 |
| Requirements Checked | 40 |
| ✓ Aligned | 16 (40%) |
| ⚠️ Drifted | 0 (0%) |
| ✗ Not Implemented | 24 (60%) |
| 🆕 Unspecced Code | 4 |

> **Read this first.** The 60% "Not Implemented" figure is entirely the
> **superseded** spec `001-rdma-remote-lookup-rdma-initiator` (the old passive
> listener/session/protobuf responder), which the rework deliberately removed.
> Against the **current** spec `002-rdma-push-initiator`, the code is **16/17
> aligned (94%)** with **zero drift** — the single gap is an acknowledged open
> task (a telemetry-overhead benchmark). The headline action is to **archive
> spec-001**, not to change code.

## Detailed Findings

### Spec: 002-rdma-push-initiator — RDMA Push Initiator (CURRENT)

#### Aligned ✓

- **FR-001** (`push` returns one `PushStatus` per item, in order) → `src/lib.rs:112-150`, `src/connection.rs:121-224`
- **FR-002** (resolve via `IMemoryTier::peek`; `KeyNotFound` / `SizeMismatch` before any write) → `src/lib.rs:126-142`
- **FR-003** (RDMA-write into `(addr, rkey)`) → `src/connection.rs:362-372` (`RealConn::write` → `rdma_write_from_pool`)
- **FR-004** (register pool from `pool_info` base+size once per connection) → `src/lib.rs:97-103`, `src/connection.rs:340-348` (`RealTransport::connect` → `register_existing_mr`)
- **FR-005** (connection table keyed by `"ip:port"`, per-host state disconnected/connecting/connected/disconnecting, lazy + reuse) → `src/connection.rs:63-72,97-138,268-297`
- **FR-006** (concurrent to different hosts; serialize same host) → per-slot `Mutex<ConnState>`, `src/connection.rs:75-77,140-141`
- **FR-007** (detect QP error / failed write, rebuild once, retry; 2nd failure → `UnableToConnect`) → `src/connection.rs:150-219` (`reconnect_used` guard)
- **FR-008** (`disconnect` idempotent + `disconnect_all`) → `src/connection.rs:227-261`, `src/lib.rs:152-162`
- **FR-009** (parse `"ip:port"`, else `InvalidEndpoint`) → `src/connection.rs:300-317`
- **FR-010** (optional `ILogger`, no-op when unbound) → `src/lib.rs:49-58,144-148`
- **FR-011** (telemetry feature-gated, ZST no-op when disabled; full metric set) → `src/telemetry.rs` (connections established/failed, reconnects, disconnects, pushes, `total_push_duration_us` for average, per-item outcomes, `bytes_written`)
- **FR-012** (trusted-fabric isolation; no app auth) → design constraint; no auth code present (consistent)
- **FR-013** (unit tests: state machine, `PushStatus` mapping, mock transport seam, telemetry wiring) → `src/connection.rs:375-655`, `src/rdma.rs:880-935`
- **SC-001** (one status per item; correct terminal statuses) → test `statuses_mapped_in_order` (`src/connection.rs:471-488`)
- **SC-002** (second push reuses connection) → test `reused_connection_does_not_reconnect` (`src/connection.rs:556-574`)
- **SC-003** (QP error → exactly one reconnect-and-retry) → test `write_failure_triggers_single_reconnect_then_succeeds` (`src/connection.rs:513-529`)

#### Drifted ⚠️

- _None._

#### Not Implemented ✗

- **SC-004** (telemetry overhead < 5% vs disabled build): no benchmark exists.
  - Severity: minor — **the spec itself flags this as an open task** ("currently unmeasured for the push path", carried from spec-001 SC-005). Tracked, not silent drift.

---

### Spec: 001-rdma-remote-lookup-rdma-initiator — RDMA Remote Lookup Initiator (SUPERSEDED)

This spec describes the **old passive-responder** design (inbound listener,
per-connection sessions, version handshake, binary batch protocol ≤64 entries,
standalone test client, `serve::run_blocking`). It was explicitly superseded by
002 during the "rework into outbound RDMA push initiator" commit. The crate's
`CLAUDE.md` already marks it stale.

#### Aligned ✓

- _None._ The current component implements the opposite role (outbound
  initiator), so none of 001's responder requirements are met by design.

#### Not Implemented ✗ (obsolete — superseded by 002)

- **FR-001** accept inbound connections on a configurable port
- **FR-002** protocol version handshake / reject incompatible
- **FR-003** per-connection session state
- **FR-004** batched lookup of ≤64 `CacheKey`s per request
- **FR-005** reject batches > 64 entries
- **FR-006** serial resolve + unsignaled-write pipelining (only last signaled)
- **FR-007** transfer results into caller remote memory (only this survives, via the 002 push path)
- **FR-008** close-connection operation releasing session resources
- **FR-009** session-failure detection via transport I/O errors
- **FR-010** delegate resolution to a dispatch service placeholder
- **FR-011** route diagnostics through a logging interface (survives generically via `ILogger`)
- **FR-012** standalone test-client program
- **FR-013** module-level unit tests for session/protocol/listener/mock
- **FR-014** telemetry on connection + throughput/latency (reshaped into 002 FR-011)
- **FR-015** deployable in "full-remote" profile with `serve::run_blocking` entry (now `IRemoteLookupRdmaInitiator::push`)
- **FR-016** lightweight binary protocol (not HTTP/REST)
- **FR-017** network-level isolation security (reshaped into 002 FR-012)
- **SC-001..SC-006** (≤500µs batch, 100 concurrent sessions, test-client clean run, disconnect cleanup, telemetry overhead, handshake rejection): none applicable to the push-initiator design; only the telemetry-overhead criterion carries forward (as 002 SC-004).

## Unspecced Code 🆕

Low-level primitives from the 001-era responder that remain in `rdma.rs` /
`ffi.rs` but are **not referenced by the current push path** (`lib.rs` /
`connection.rs`). Kept because the accept side is planned to move to
`remote-lookup`, but currently dead relative to spec-002.

| Feature | Location | Notes | Suggested owner |
|---------|----------|-------|-----------------|
| `RdmaListener` (bind / listen / accept) | `src/rdma.rs:517-690` | Inbound accept side; 002 says this belongs in `remote-lookup` | move to remote-lookup spec |
| `send_msg` / `recv_msg` + `rdma_test_send_msg`/`recv_msg` FFI | `src/rdma.rs:201-239`, `src/ffi.rs:330-331` | Send/recv message path unused by push (push only RDMA-writes) | remote-lookup / remove |
| `post_rdma_write_unsignaled` (+ FFI) | `src/rdma.rs:297-320`, `src/ffi.rs:340-347` | Unsignaled-write pipelining from 001 FR-006; 002 push uses signaled per-item writes | future perf spec, or remove |
| `integration-test` Cargo feature | `Cargo.toml:11` | Declared but referenced by no `#[cfg]` code; README claims `#[ignore]` HW tests that do not exist | spec + implement, or drop feature |

## Inter-Spec Conflicts

- **001 vs 002 — role inversion (resolved by supersession).** 001 defines a
  passive inbound responder; 002 defines an outbound push initiator. These are
  contradictory descriptions of the same component. 002 records the supersession
  in its header; 001 carries **no superseded/obsolete banner of its own**, so a
  reader landing on 001 first would be misled.

## Recommendations

1. **Archive spec-001.** Move `specs/001-rdma-remote-lookup-rdma-initiator/` to an
   archived location or add a prominent `> **SUPERSEDED by 002**` banner to its
   `spec.md`. This eliminates the entire 60% "Not Implemented" figure, which is
   purely the obsolete spec.
2. **Decide the fate of the 001-era primitives** (`RdmaListener`, `send_msg`/
   `recv_msg`, `post_rdma_write_unsignaled`). Either (a) relocate the accept-side
   primitives to `remote-lookup` alongside a spec, or (b) remove them from this
   crate to match spec-002's "outbound data path only" scope. Leaving them
   unspecced invites confusion about the component's role.
3. **Resolve the `integration-test` feature.** It is an empty flag and the
   README describes `#[ignore]` hardware tests that do not exist. Either write
   the loopback integration test (deferred) or drop the feature and correct the
   README so docs match code.
4. **Close SC-004.** Add a Criterion benchmark comparing the `telemetry` on/off
   push path to validate the < 5% overhead claim.
5. **No code changes are required for spec-002 alignment** — the implementation
   already satisfies FR-001..FR-013 and SC-001..SC-003.
