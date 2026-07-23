# Spec Drift Report — remote-lookup

Generated: 2026-07-22T22:40:55Z
Project: `components/remote-lookup`
Specs analyzed: 2 (`001-remote-lookup-placeholder`, `002-remote-lookup-rdma`)

## Summary

| Category | Count |
|---|---|
| Specs analyzed | 2 |
| Requirements/criteria checked | 53 |
| ✅ Aligned | 42 |
| ⚠️ Drifted | 9 |
| ❌ Not implemented | 2 |
| 🆕 Unspecced code items | 3 |

Verified in-sandbox: `cargo check -p remote-lookup` (clean), `cargo clippy -p remote-lookup -- -D warnings` (clean), `cargo doc -p remote-lookup --no-deps` (warning-free). `cargo test`/`--doc` could not run — `deps/zyre-build/include/zyre.h` is absent in this environment (a `zyre` dev-dependency of `tests/mesh.rs`), so those criteria are assessed by static/code review only, noted as caveats below.

**Most significant drift**: Spec `001-remote-lookup-placeholder` is stamped `Status: Synced (2026-06-19)` but is comprehensively superseded by `002-remote-lookup-rdma` (which says so explicitly) — its `batch_lookup` signature (`&[(CacheKey, IpcHandle)]`, placeholder logging, always-`NotFound`) no longer matches the shipped code (`&[(CacheKey, u32)]`, full zyre+RDMA protocol). `001`'s own SC-002 ("compiles with the same `(CacheKey, IpcHandle)` parameter types as `IDispatcher::batch_lookup`") is now false by design: `IDispatcher::batch_lookup` still takes `IpcHandle` (`components/interfaces/src/idispatcher.rs:371`) while `IRemoteLookup::batch_lookup` takes `u32` (`components/interfaces/src/iremote_lookup.rs:141`) — `dispatcher-p2p` explicitly converts between them (`components/dispatcher-p2p/src/lib.rs:1891-1897`). This is intentional (per `002`'s Clarifications), but `001`'s `Status` field should read `Superseded`, not `Synced`.

---

## Spec 001: `001-remote-lookup-placeholder` — "Remote Lookup Batch Interface"

**Status field**: `Synced (2026-06-19)` — **misleading**; superseded by 002 (see conflicts).

### Aligned ✅

| Requirement | Notes |
|---|---|
| FR-002 | `batch_lookup` still returns one `Result` per entry, positional order — `src/lib.rs:271-300`. |
| FR-006 | Empty slice → empty `Vec` — `src/lib.rs:272-274`. |
| FR-007 | Interface still lives at `components/interfaces/src/iremote_lookup.rs`. |
| SC-001 | Unit tests present and pass by code review (`src/lib.rs:362-441`); full `cargo test` run blocked by missing `deps/zyre-build` in this sandbox (see caveat above). |
| SC-003 | `batch_lookup` doc example (`src/lib.rs:255-270`) matches actual pre-init behavior (`NotFound`); `cargo doc` built clean, `cargo test --doc` not run (same caveat). |
| SC-004 | `cargo clippy -p remote-lookup -- -D warnings` — clean (verified). |
| SC-005 | `cargo doc -p remote-lookup --no-deps` — warning-free (verified). |

### Drifted ⚠️

| Requirement | Spec text | Actual | Location | Severity |
|---|---|---|---|---|
| FR-001 | `batch_lookup(&self, entries: &[(CacheKey, IpcHandle)]) -> Vec<Result<(), RemoteLookupError>>` | Signature is `batch_lookup(&self, entries: &[(CacheKey, u32)]) -> Vec<...>` — `IpcHandle` removed, replaced by an expected-size `u32` (per 002 Clarification Q1). | `components/interfaces/src/iremote_lookup.rs:141-144` | high (intentional, superseded) |
| FR-003 | "Each entry ... MUST produce a log message via the `ILogger` receptacle (placeholder behavior)." | No per-entry log call exists; `batch_lookup` submits to the actor, which SHOUTs a KEY_QUERY — no log statement per entry. | `src/lib.rs:271-300`, `src/actor.rs:264-292` | medium (superseded) |
| FR-004 | "the placeholder implementation MUST return `Err(RemoteLookupError::NotFound)` for each entry (no actual network I/O)." | Real network I/O now occurs: zyre SHOUT/WHISPER + one-sided RDMA writes via the initiator/responder receptacles. | `src/actor.rs` (`on_submit`, `on_key_response`, `on_rdma_status`) | high (superseded by design) |
| FR-008 | "The component MUST expose functionality only through the `IRemoteLookup` interface — no public functions outside the interface." | `RemoteLookupComponent` has three `pub fn`s outside `IRemoteLookup`: `peers_seen`, `signal_shutdown`, `shutdown`. | `src/lib.rs:325`, `:334`, `:342` | low (used only by `apps/certus-server` wiring and the crate's own `tests/mesh*.rs`; not carried forward as a requirement in 002) |
| FR-009 | "The placeholder implementation MUST log the endpoint and return `Ok(())`." | `join_cluster` routes to the actor (`ActorMsg::Join`) which really calls `node.join`; returns `Err` if uninitialized rather than always `Ok`. | `src/lib.rs:304-310`, `src/actor.rs:202-203` | medium (superseded) |
| FR-010 | "The placeholder implementation MUST log the call and return `Ok(())`." | `leave_cluster` really leaves the zyre group via the actor; returns `Err` if uninitialized. | `src/lib.rs:313-319`, `src/actor.rs:205-206` | medium (superseded) |
| SC-002 | "The interface compiles with the same `(CacheKey, IpcHandle)` parameter types as `IDispatcher::batch_lookup`." | `IDispatcher::batch_lookup` still uses `IpcHandle`; `IRemoteLookup::batch_lookup` uses `u32`. The two signatures no longer match — `dispatcher-p2p` converts explicitly. | `components/interfaces/src/idispatcher.rs:371-374` vs `components/interfaces/src/iremote_lookup.rs:141-144`; conversion at `components/dispatcher-p2p/src/lib.rs:1888-1899` | high (deliberate design change, but 001 not marked superseded) |

### Not Implemented ❌

| Requirement | Notes |
|---|---|
| `IpcHandle` key entity | 001 defines `IpcHandle` as the GPU-DMA handle carried in `batch_lookup`'s entries. 002 removes it from the `IRemoteLookup` boundary entirely (remote-lookup is CPU/DRAM-only; the dispatcher does DRAM→GPU delivery). `IpcHandle` no longer appears anywhere in `remote-lookup`'s interface surface. |

---

## Spec 002: `002-remote-lookup-rdma` — "Remote Lookup over Zyre + RDMA" (current design, `Status: Draft`)

### Aligned ✅ (27 of 29 functional requirements, 8 of 8 success criteria)

| Requirement | Location |
|---|---|
| FR-001 (`IRemoteLookup` impl, `(key,u32)` signature, `initialize`/`join_cluster`/`leave_cluster`) | `src/lib.rs:77-320` |
| FR-002 (actor on dedicated thread owning `IZyreNode`) | `src/actor.rs::run` |
| FR-003 (join on activation / leave on deactivate) | `src/lib.rs::initialize`, `src/actor.rs::run` (`ActorMsg::Leave`), `ActorState::shutdown` |
| FR-004 (`(key,size)` wire identity, no size-mismatch status) | `src/wire.rs`, `src/server.rs::classify_query` |
| FR-005 (SHOUT KEY_QUERY, chunked by `max_keys_per_query`) | `src/actor.rs::on_submit` |
| FR-006 / FR-006a (private landing slot, publish-on-success, greedy per-reply fetch) | `src/actor.rs::on_key_response`, `::publish_success` |
| FR-007 (RDMA_REQUEST carries endpoint + pool rkey + slot) | `src/actor.rs::on_key_response` |
| FR-008 (publish on RDMA_STATUS success) | `src/actor.rs::publish_success` |
| FR-010 (Phase-1→Phase-2 on quorum% or timeout) | `src/actor.rs::advance`, `Operation::quorum_reached`, `tick_deadlines` |
| FR-011 (bounded retry to untried alternates, memory-first) | `src/actor.rs::try_retry` |
| FR-012 (completion criteria) | `src/actor.rs::advance`, `::finalize`, `::tick_deadlines` |
| FR-013 (peer Exit handling) | `src/actor.rs::on_exit` |
| FR-014 (teardown-before-reclaim) | `src/actor.rs::on_exit`, `::teardown_peer`, `::finalize` (orphans) |
| FR-015 (KEY_QUERY classification) | `src/server.rs::classify_query` |
| FR-016 / FR-017 (serve + disk promotion + promotion-failure handling) | `src/server.rs::serve_rdma_request` |
| FR-018 (framing, unknown-type log-and-ignore) | `src/wire.rs` |
| FR-019 (stale `op_id` discarded) | `src/actor.rs::on_key_response`, `::on_rdma_status` |
| FR-020 (concurrent ops by `op_id`) | `src/actor.rs::ActorState::ops` |
| FR-021 (self-SHOUT filter) | `src/actor.rs::handle_key_query` |
| FR-022 (`LookupConfig`, all fields consulted, `Default`) | `components/interfaces/src/iremote_lookup.rs:26-76`, consulted throughout `src/actor.rs` |
| FR-024 (no direct RDMA transport logic) | `src/server.rs`, `src/worker.rs` delegate to `IRemoteLookupRdmaInitiator`/`Responder` |
| FR-025 (responder wiring, endpoint/rkey caching, control channel) | `src/lib.rs::initialize` (lines 140-165) |
| FR-026 (single-flight coalescing) | `src/actor.rs::InFlight`, `on_key_response`, `try_phase2` |
| FR-027 (warm-at-discovery) | `src/actor.rs::on_enter`, `RDMA_ENDPOINT_HEADER`; test `warms_connections_to_discovered_peers` |
| FR-028 (off-loop worker, poll-loop stays non-blocking) | `src/worker.rs` |
| SC-001 (RDMA-dominated latency, no polling gap) | Design supports it structurally; no hardware-timed measurement in this review. |
| SC-002 (deadline-bounded `NotFound`, default 50ms) | `LookupConfig::default().op_deadline == 50ms`; `tick_deadlines` finalizes on expiry. |
| SC-003 (RDMA_REQUEST whispered same event as first satisfiable KEY_RESPONSE) | `on_key_response` whispers synchronously within the same event-handling pass. |
| SC-004 (≥95% first-retry success to an alternate) | Functional retry path is tested (`failed_fetch_retries_alternate_peer`); the 95% figure is a statistical/HW claim not verifiable by static review or the mock mesh. |
| SC-006 (prompt re-evaluation on peer departure) | `on_exit` calls `check_all_completions()` immediately. |
| SC-007 (size mismatch never partial/mismatched write) | `classify_query` treats a size mismatch as `Avail::None`. |
| SC-008 (single-flight: exactly one fetch per key) | `InFlight` index; test `concurrent_same_key_lookups_issue_one_rdma`. |

### Drifted ⚠️

| Requirement | Spec text | Actual | Location | Severity |
|---|---|---|---|---|
| FR-009 | "On RDMA_STATUS(UnableToConnect \| KeyNoLongerAvailable) ... MUST NOT yet reclaim the landing slot if a late write could still be in flight (see FR-014)." | `on_rdma_status` reclaims the slot (`memory_tier.remove`) immediately on any failure status, with no additional wait. This matches `contracts/wire-protocol.md:73-76` ("frees a slot ... only on (1) RDMA_STATUS received, or (2) peer Exit after DisconnectAck") — a *received* status means the push attempt is complete, so no late write is in flight; FR-009's prose is more conservative than the contract and than the code. | `src/actor.rs::on_rdma_status` (lines ~494-509) vs `specs/002-remote-lookup-rdma/contracts/wire-protocol.md:73-76` | low (spec-internal wording ambiguity, not a safety gap — carried over from the prior drift report) |
| FR-023 | Receptacle list: `IZyre`, `IDispatchMap`, `IMemoryTier`, `IDispatcher`, `IRemoteLookupRdmaInitiator`, `IRemoteLookupRdmaResponder`, `ILogger` (7 named). | The component declares **8** receptacles — the same 7 plus `responder_admin: IRemoteLookupRdmaResponderAdmin` (required by FR-025 for bind-IP/lifecycle admin). `responder_admin` *is* documented in `data-model.md:19`, `contracts/iremote_lookup.md:24`, and `tasks.md:101/114` — only `spec.md`'s FR-023 prose omits it. | `src/lib.rs:38-51` vs `specs/002-remote-lookup-rdma/spec.md:378-380` | low (documentation completeness gap within the spec package itself; code and the other spec artifacts already agree) |

### Not Implemented ❌

| Requirement | Notes |
|---|---|
| User Story 7 dedicated test coverage (`tasks.md` T025, backing SC-005/SC-006) | `tasks.md:280-283` requires a `tests/mesh.rs` scenario asserting: (1) a peer `Exit` drops its cached reply, returns in-progress keys to unsatisfied, and re-evaluates completion; (2) an in-flight landing slot's reclaim blocks on `DisconnectAck`. No such test exists — `tests/mesh.rs` has no `Exit`/departure scenario at all (checked: no match for "exit" beyond a module comment). The underlying implementation (`ActorState::on_exit`, `::teardown_peer`) is present and reviewed as correct, so this is a **test-coverage gap**, not a missing feature. |

---

## Unspecced Code 🆕

| Feature | Location | Lines | Suggested spec |
|---|---|---|---|
| `RemoteLookupComponent::peers_seen()` — public peer-count accessor for test discovery barriers | `src/lib.rs:322-327` | 6 | Document as an out-of-interface test/observability hook in 002's Key Entities, or move under `#[cfg(test)]`/a test-only trait if not needed by production callers. |
| `RemoteLookupComponent::signal_shutdown()` — non-blocking actor-stop signal, used for two-phase multi-node teardown | `src/lib.rs:329-338` | 10 | Add to 002 as an explicit lifecycle requirement (teardown ordering across a group of actors sharing a zyre/czmq context) — it's real production-relevant behavior (`apps/certus-server` wiring), not test scaffolding. |
| `RemoteLookupComponent::shutdown()` — public join-actor-and-worker teardown, also called from `Drop` | `src/lib.rs:340-353` | 14 | Same as above — 002 has no FR for component teardown/`Drop` ordering (actor before worker) even though it is a real correctness concern (worker channel closure). |

`wire`/`seams` modules and `benches/correlation.rs` are already spec-anticipated (Key Entities `WireMessage`; research Decision 8 mock seams; Constitution-mandated benchmark) — not counted as unspecced.

## Conflicts

1. **001 `Status: Synced` vs. 002 `Supersedes: 001-remote-lookup-placeholder`** — 001's header claims it is synced with the code, but 002 explicitly supersedes it and the code implements 002's contract, not 001's. 001's `Status` field should be updated to `Superseded (2026-07-12)` to avoid a future reader trusting its FRs.
2. **FR-009 (002 spec.md) vs. `contracts/wire-protocol.md`** — see the FR-009 drift row above; the two 002 artifacts disagree on when a slot may be reclaimed after a failure status, and the code follows the contract, not the FR prose.

## Recommendations

1. **Update `specs/001-remote-lookup-placeholder/spec.md`'s `Status` field to `Superseded`** and add a one-line pointer to 002, so no future reader treats its FR-001/003/004/008/009/010/SC-002 as current.
2. **Reword FR-009 in `specs/002-remote-lookup-rdma/spec.md`** to match `contracts/wire-protocol.md`: a *received* RDMA_STATUS (success or failure) is itself a safe reclaim point; "must not reclaim while a late write could land" applies only to the *no-status/timeout* path, which the orphan mechanism already covers.
3. **Add `responder_admin: IRemoteLookupRdmaResponderAdmin` to FR-023's receptacle list** in `spec.md` (it is already correct in `data-model.md`/`contracts/iremote_lookup.md`/`tasks.md`).
4. **Implement `tasks.md` T025** — a `tests/mesh.rs` scenario for peer `Exit` during an operation (cached-reply drop, in-progress→unsatisfied, `DisconnectAck`-gated slot reclaim) so SC-005/SC-006 and User Story 7's acceptance scenarios have direct test coverage, not just code-reviewed correctness.
5. **Decide the disposition of `peers_seen`/`signal_shutdown`/`shutdown`**: either document them in 002 as intentional lifecycle/test hooks (they are used by `apps/certus-server` and `tests/mesh*.rs`, both exempt call sites under the project's leakage rules) or gate them behind `#[cfg(test)]`/`pub(crate)` if no non-test caller needs `peers_seen`/`signal_shutdown` specifically.
6. **Housekeeping (unrelated to spec drift)**: `components/remote-lookup/transcript_ee4cdaf9-d444-4a24-82ef-d0566d1bb34f_2026-07-15.md` is a committed session-transcript artifact (2253 lines) that does not belong in the crate; consider removing it or adding it to `.gitignore`.
