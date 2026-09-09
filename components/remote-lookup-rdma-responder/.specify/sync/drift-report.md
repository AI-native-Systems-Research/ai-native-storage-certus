---
spec_sync_component: remote-lookup-rdma-responder
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-09T22:27:18Z
spec_sync_git_commit: 3411518a
spec_sync_inputs_sha256: 492225acbe8f14f2a51c26843712dad767b124e3338af21fa3704476240b2499
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Re-stamp 2026-09-09 (merged-branch interfaces-fold; no drift).** Branch
> `fix-dispatcher-store-backpressure` was merged into `unstable` at `3411518a`; that
> branch adds `DispatcherConfig::store_backpressure_ms` plus two `TierEventStats`
> counters (`store_backpressure_events`, `store_drops_on_full`) to
> `components/interfaces/src/idispatcher.rs`. `scripts/spec-sync-hash.sh` folds
> the whole `components/interfaces/` tree into every component's hash, so this
> component's digest moved even though its own `src/`+`specs/` are byte-for-byte
> unchanged and it references none of those new dispatcher symbols (verified by
> grep across `components/` and `lib/`). The merge's conflict resolution had
> reverted this stamp to unstable's `b220a1c8` value; the digest is recomputed
> here at merged HEAD. Drift status remains `clean`; the report body stands
> verbatim.

# Spec-vs-Implementation Drift Report — remote-lookup-rdma-responder

**Generated**: 2026-09-03 (Spec-Sync re-sweep + independent verification)
**Spec analyzed**: `001-rdma-lookup-responder`
**Mode**: read-only drift analysis, then **ALIGN** (code fix) for FR-010; then
freshness stamp.

This sweep re-verified **every FR/SC against the actual implementation** rather
than re-trusting the prior artifact. The prior report marked FR-010 "Aligned ✓";
independent verification found it was **not** aligned — an `ibv_reg_mr` failure
returned `RemoteLookupRdmaResponderError::Bind`, but FR-010 mandates
`Registration`. That drift is resolved this sweep (ALIGN).

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 24 (FR-001..FR-016 incl. FR-002a, FR-011a + SC-001..SC-006) |
| Aligned (after this sweep) | 24 |
| Drifted → resolved this sweep | 1 (FR-010 ALIGN) |
| Not Implemented | 0 |
| Unspecced (documented) | 2 (both in spec Known Limitations) |

**Verification runs this sweep** (all green):
- `cargo build -p remote-lookup-rdma-responder` — clean
- `cargo clippy -p remote-lookup-rdma-responder --all-targets -- -D warnings` — clean
- `cargo test -p remote-lookup-rdma-responder -- --test-threads 1` — 22 passed; 0 failed
- `cargo clippy -p remote-lookup-rdma-responder --features rdma --all-targets -- -D warnings`
  — clean (compiles the `#[cfg(feature = "rdma")]` FR-010 path; rdma-core present
  in this environment)

## Resolved this sweep

### 001-rdma-lookup-responder / FR-010 — ALIGN (code fix)

- **Severity**: minor (error-classification contract gap on the registration path).
- **Spec**: FR-010 requires the responder to register the whole pool once via
  `ibv_reg_mr` and, on failure, return
  `RemoteLookupRdmaResponderError::Registration` (spec.md:346-353) — distinct from
  the `Bind` variant used for bind / `rdma_listen` / device-resolution failures
  (FR-002).
- **Actual (before this sweep)**: `RealCmSeam::bind` returned `Result<_, String>`
  and the caller mapped **every** failure — including the `ibv_reg_mr` failure — to
  `Bind` via `.map_err(RemoteLookupRdmaResponderError::Bind)`. A registration failure
  was therefore misreported as `Bind`, violating FR-010's contract and blurring the
  FR-002/FR-010 diagnostic distinction.
- **Direction — code fixed to match spec (ALIGN)**: FR-010 is an authored contract
  that deliberately separates the two failure classes; the code collapsed them. The
  spec states the intended behavior, so the code carried the defect. (Backfilling
  `Bind` into FR-010 would weaken a legitimate contract to match a bug — forbidden by
  the HARD RULE.)
- **Fix applied**: introduced a typed `CmBindError { Bind(String), Registration(String) }`
  in `src/rdma.rs` with `From<String>`/`From<&str>` defaulting to `Bind` (so the
  existing bind / listen / device-resolution error sites are unchanged); the sole
  `ibv_reg_mr` failure site now returns `CmBindError::Registration`. `bind()`'s return
  type changed from `Result<_, String>` to `Result<_, CmBindError>`, and the caller in
  `src/lib.rs` matches the two variants onto
  `RemoteLookupRdmaResponderError::{Bind, Registration}`.
- **Location (fixed)**: `src/rdma.rs` (`CmBindError`, `bind()`, `ibv_reg_mr` site);
  `src/lib.rs` (`initialize()` caller).

## Aligned ✓ (verified this sweep)

Init/accept-loop (FR-001, `src/lib.rs:319-330`); bind ephemeral-port-0 + `rdma_listen`
+ `rdma_get_src_port`, `Bind` on failure, twice→`AlreadyInitialized` (FR-002,
`src/rdma.rs:239-268`; `src/lib.rs:275-278`); bind-IP precedence (FR-002a,
`src/lib.rs:285-290`; `src/rdma.rs:214-219`); `local_endpoint()`
after/`NotInitialized`-before (FR-003); tri-fd epoll accept loop (FR-004,
`src/rdma.rs:511-558`); UUID from `private_data`→`PeerId`, `ConnectionEstablished{Some}`
(FR-005); absent/malformed `private_data`→`node:None` (FR-006); `Active→Draining→Dead`
(FR-007); QP→ERROR before ack, best-effort destroy, idempotent (FR-008); never
reads/copies value bytes (FR-009); **whole-pool `ibv_reg_mr` once + `local_region()` +
dereg-before-PD-free + `Registration` on failure (FR-010 — now aligned, above)**;
single-client control channel (FR-011); lossless event delivery via backpressure
(FR-011a); `set_actor_cpu` pins accept thread (FR-012); `signal_stop`/`shutdown`
idempotent teardown (FR-013); primary diagnostics via optional `ILogger`, missing
logger never errors (FR-014 — specced contract met; see documented residual below);
unit tests for lifecycle/error/state-machine (FR-015); telemetry behind feature, ZST
no-op when off (FR-016); SC-001..SC-006 (`src/rdma.rs:263-268`;
`src/connection.rs:354-418,462-481`; `benches/connection_telemetry.rs`).

## Documented known-gaps (non-blocking; carried in spec Known Limitations)

### FR-014 async-event instrumentation — `eprintln!` bypass

- FR-014's **specced** contract (route primary diagnostics through the optional
  `ILogger`, never error on a missing logger) **is met** (`src/lib.rs:116-120`).
- The residual: `drain_async_events` (`src/rdma.rs:473-491`) emits device async-event
  diagnostics via `eprintln!` rather than `ILogger`, because `RealCmSeam`
  (`src/rdma.rs:176`) — the accept-loop thread — holds no logger handle. This path is
  **unspecced** ("No FR mandates this diagnostic path — best-effort operator
  instrumentation") and the `eprintln!`→`ILogger` gap is explicitly documented in the
  spec's Known Limitations with a tracked align-task
  (`.specify/sync/align-tasks.md`, FR-014 accept-loop diagnostics).
- **Not blocking `clean`**: the correctness contract is satisfied; this is a
  documented, accepted deviation on a best-effort diagnostic, not masked drift. If it
  becomes operator-load-bearing, plumb an `ILogger` into the seam and close the
  align-task.

## Unspecced Code (documented in spec Known Limitations)

1. **Device async-event instrumentation** (`src/rdma.rs`, `src/ffi.rs`,
   `src/wrapper.c`) — best-effort operator diagnostics; documented (see FR-014
   residual). No new spec needed.
2. **Command-bridge thread** (`rdma-responder-cmd-bridge`, `src/rdma.rs:358-373`) —
   internal mechanism enabling FR-004 (bridges the SPSC command inbox onto the command
   eventfd, since the SPSC channel has no pollable fd). Behavior is implied by FR-004.

## Conflicts
None.

## Recommendations

1. **FR-014 (Low, cross-component doc):** the *interface* doc for
   `set_bind_ip` in `components/interfaces/src/iremote_lookup_rdma_responder.rs:256-262`
   states the responder "never auto-detects", which contradicts FR-002a's explicit
   precedence rule (explicit `set_bind_ip` **else** auto-detect first active device).
   This is an interfaces-crate doc nit, not a responder `src/`/`specs/` drift; fixing
   it invalidates the folded input hash of **every** stamped component, so it is
   deferred to a coordinated interfaces-doc pass + re-stamp rather than fixed here.
2. **FR-014 async-event align-task:** route `drain_async_events` through `ILogger`
   (requires giving `RealCmSeam` a logger handle) to close the documented gap.
3. **Command-bridge thread:** a one-line mention in plan.md/data-model would remove the
   only undocumented internal surprise in the FR-004 mechanism.
