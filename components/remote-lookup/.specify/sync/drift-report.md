---
spec_sync_component: remote-lookup
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:39:18Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: 3a4808dac9b2152d96d57ac564354186022e9decbae7329624dba50b42911989
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
Generated: 2026-09-02T21:39:18Z (re-verification + stamp pass; findings carried from the 2026-08-20 sweep, re-checked against source at commit 2fc1cd3c)
# Spec-vs-Implementation Drift Report — remote-lookup

Analysis of `components/remote-lookup` source against its two specs. Spec
`002-remote-lookup-rdma` is the **design-of-record**; spec
`001-remote-lookup-placeholder` is **superseded** (its Supersession Notice is
honored — divergences from 001 are intentional and non-actionable). READ-ONLY:
no source was built or modified.

## Summary

| Metric | Count |
| --- | --- |
| Specs Analyzed | 2 |
| Requirements Checked | 60 |
| Aligned | 54 |
| Drifted | 6 (1 actionable Low; 5 superseded / non-actionable) |
| Not Implemented | 0 |
| Unspecced Features | 3 |

The single actionable finding is **FR-018 (Low)**: unknown/malformed wire frames
are ignored correctly but not *logged*, so the "logged and ignored" requirement
is half-met. This was already queued as ALIGN Task 3 in the 2026-08-07 sweep and
remains open. All other divergences are intentional supersession of spec 001.

No stale crate-path references were found in either spec (no
`components/component-framework` or `components/spdk-sys` references to flag as
the components/→lib/ move MINOR drift).

---

## Spec 002 — Remote Lookup over Zyre + RDMA (design-of-record)

45 requirements checked (37 FR incl. 006a/016a/016b, 8 SC). Aligned 44, Drifted 1, Not Implemented 0.

### Aligned ✓

| Requirement | Location |
| --- | --- |
| FR-001 (IRemoteLookup: batch_lookup(&[(CacheKey,u32)]), initialize, join/leave) | `src/lib.rs:79-334`; `components/interfaces/src/iremote_lookup.rs:120-196` |
| FR-002 (actor on dedicated thread owning IZyreNode, polls) | `src/actor.rs:174-275`; `src/lib.rs:234-237` |
| FR-003 (join configured group default `remote_lookup`; leave on shutdown) | `src/lib.rs:191-192`; `src/actor.rs:1042`; `iremote_lookup.rs:84` |
| FR-004 (wire identity `(key,size)`; wrong size ⇒ not available) | `src/wire.rs:99-108`; `src/server.rs:131-162` |
| FR-005 (SHOUT KEY_QUERY, split under max_keys_per_query, init op state) | `src/actor.rs:280-308` |
| FR-006 (private landing slot; publish-on-success; discard on failure) | `src/actor.rs:444-472,561-591`; `src/operation.rs:42-49` |
| FR-006a (greedy per-KEY_RESPONSE memory fetch; cache full reply; skip in-progress) | `src/actor.rs:405-488` |
| FR-007 (RDMA_REQUEST carries Endpoint + RemoteRegion{addr,rkey,length}) | `src/actor.rs:477-485`; `src/wire.rs:109-115` |
| FR-008 (RDMA_STATUS Success ⇒ publish + satisfied) | `src/actor.rs:513-518,561-591` |
| FR-009 (failure ⇒ in-progress→unsatisfied, slot reclaimable immediately) | `src/actor.rs:519-533` |
| FR-010 (Phase 2 re-scans cached replies, no new SHOUT, disk holders) | `src/actor.rs:682-810` |
| FR-011 (bounded retry rounds; memory-then-disk; never re-try failed peer) | `src/actor.rs:599-660` |
| FR-012 (completion: all-satisfied / no holder / retry cap / deadline) | `src/actor.rs:682-838,889-942` |
| FR-013 (peer Exit drops cached reply, returns keys, re-evals completion) | `src/actor.rs:950-1010` |
| FR-014 (teardown-before-reclaim: Disconnect + block for DisconnectAck) | `src/actor.rs:993-1034` |
| FR-015 (server classify KEY_QUERY against dispatch-map) | `src/server.rs:131-162`; `src/actor.rs:337-348` |
| FR-016 (server push: promote disk keys, re-lookup, pin, push_async, map status) | `src/server.rs:198-316` |
| FR-016a (pins owned by completion callback, released on run or drop) | `src/server.rs:47-121,296-311` |
| FR-016b (every RDMA_REQUEST yields exactly one RDMA_STATUS) | `src/server.rs:86-121,289-293` |
| FR-017 (promotion failure ⇒ KeyNoLongerAvailable) | `src/server.rs:249-287` |
| FR-019 (stale op_id discarded without error) | `src/actor.rs:412-414,498-509` |
| FR-020 (multiple concurrent ops keyed by op_id; caller blocks on its op) | `src/actor.rs:161,682-727`; `src/lib.rs:291-313` |
| FR-021 (ignore SHOUT whose peer id == own uuid) | `src/actor.rs:337-340` |
| FR-022 (LookupConfig public, Default, sensible defaults) | `components/interfaces/src/iremote_lookup.rs:28-98` |
| FR-023 (8 receptacles: zyre/dispatch_map/memory_tier/dispatcher/initiator/responder/responder_admin/logger) | `src/lib.rs:44-53` |
| FR-024 (no direct RDMA logic; via initiator/responder) | `src/actor.rs`, `src/worker.rs`, `src/server.rs` (delegated) |
| FR-025 (responder_admin bring-up; read Endpoint + rkey; open control channel) | `src/lib.rs:142-166` |
| FR-026 (single-flight per-key in-flight index; follower waits) | `src/actor.rs:102-108,428-434,541-554` |
| FR-027 (advertise responder endpoint header; warm on Enter, best-effort) | `src/lib.rs:177-180`; `src/actor.rs:393-399`; `src/worker.rs:77-79` |
| FR-028 (no blocking RDMA on poll loop; off-loop worker returns statuses) | `src/actor.rs:356-371`; `src/worker.rs:71-114` |
| FR-029 (out-of-interface hooks peers_seen/signal_shutdown/shutdown) | `src/lib.rs:336-374` |
| FR-030 (caller_wait decouples caller block from op_deadline) | `src/lib.rs:301-313`; `iremote_lookup.rs:44-50` |
| FR-031 (timer-driven force-reclaim backstop; tick_orphans) | `src/actor.rs:110-121,264-268,849-885` |
| FR-032 (orphan-reuse guard: no re-reserve of orphaned key) | `src/actor.rs:441-443,630-632,772-774` |
| FR-033 (LookupConfig also carries actor_cpu/discovery/node_endpoint) | `iremote_lookup.rs:64-78`; `src/lib.rs:146-149,171-173` |
| FR-034 (`integrity-check` Cargo feature forwards interfaces/integrity-check) | `Cargo.toml:9-14` |
| SC-001 (peer memory hit becomes locally resident in one batch_lookup) | test `memory_hit_is_fetched_from_peer` `tests/mesh.rs:228-250` |
| SC-002 (no peer ⇒ NotFound within deadline, no slot left) | test `total_miss_returns_not_found_within_deadline` `tests/mesh.rs:577-597` |
| SC-003 (greedy Phase-1: RDMA_REQUEST on same event as first satisfiable KEY_RESPONSE) | `src/actor.rs:420-485` (whisper inline in on_key_response) |
| SC-004 (retry to alternate succeeds) | test `failed_fetch_retries_alternate_peer` `tests/mesh.rs:337-366` |
| SC-005 (no reclaim of slot exposed to departed peer w/o ack; no pin/write-ref leak) | `src/actor.rs:916-937`; `src/server.rs:47-121`; tests `slot_survives_timeout...` / `stuck_orphan...` |
| SC-006 (peer departure completes as criteria met, no spurious wait) | `src/actor.rs:1009` (check_all_completions in on_exit) |
| SC-007 (wrong-size request ⇒ not-available, never partial write) | `src/server.rs:137-160,218-236` |
| SC-008 (concurrent same-key ⇒ exactly one remote fetch) | test `concurrent_same_key_lookups_issue_one_rdma` `tests/mesh.rs:252-290` |

### Drifted ⚠️

| Requirement | Spec text | Actual | Location | Severity |
| --- | --- | --- | --- | --- |
| FR-018 | "Unknown message types MUST be **logged** and ignored." | Framing header (`[version][msg_type][op_id]`) and op_id echo are correct, and unknown/malformed frames are ignored — but no log line is emitted: `WireMessage::Unknown { .. } => {}` and the malformed-decode arm `Err(_) => return` are both silent. | `src/actor.rs:314,330` | Low |

### Not Implemented ✗

None. (US7 peer-departure behavior is fully implemented in `on_exit`/`teardown_peer`; note the automated mesh test for it — ALIGN Task 1 / tasks.md T025 — is still a test-coverage gap, not a missing feature.)

---

## Spec 001 — Remote Lookup Batch Interface (SUPERSEDED)

15 requirements checked (10 FR, 5 SC). **Superseded by 002** — divergences below
are intentional and NON-ACTIONABLE (severity Low/superseded), per 001's
Supersession Notice.

### Aligned ✓ (requirements that still hold under 002)

| Requirement | Location |
| --- | --- |
| FR-002 (one Result per entry, positional order) | `src/operation.rs:149-160` |
| FR-005 (callable any time after instantiation; uninitialized ⇒ NotFound) | `src/lib.rs:275-289` |
| FR-006 (empty slice ⇒ empty Vec) | `src/lib.rs:276-278` |
| FR-007 (interface resides in components/interfaces/src/iremote_lookup.rs) | `components/interfaces/src/iremote_lookup.rs` |
| FR-009 (join_cluster signature present) | `iremote_lookup.rs:180`; `src/lib.rs:318-324` |
| FR-010 (leave_cluster signature present) | `iremote_lookup.rs:194`; `src/lib.rs:327-333` |
| SC-001 (unit tests pass) | `src/lib.rs:376-455` |
| SC-003 (doc tests compile/pass) | doc examples in `iremote_lookup.rs` / `src/lib.rs` |
| SC-004 (clippy clean) | project convention |
| SC-005 (cargo doc warning-free) | project convention |

### Drifted ⚠️ (intentional supersession — Low, non-actionable)

| Requirement | Spec text (001) | Actual (002 design-of-record) | Location | Severity |
| --- | --- | --- | --- | --- |
| FR-001 | `batch_lookup(&[(CacheKey, IpcHandle)])` | `batch_lookup(&[(CacheKey, u32 /*size*/)])` — IpcHandle dropped; remote-lookup is CPU/DRAM-only (002 FR-001) | `iremote_lookup.rs:163-166` | Low (superseded) |
| FR-003 | Log a message per entry (placeholder) | Real KEY_QUERY→RDMA protocol; no per-entry placeholder log | `src/actor.rs` | Low (superseded) |
| FR-004 | Return `Err(NotFound)` for each entry, no network I/O | Performs real zyre + one-sided RDMA I/O; `Ok(())` on resident | `src/actor.rs`, `src/server.rs` | Low (superseded) |
| FR-008 | Expose functionality only through IRemoteLookup (no public fns outside) | 002 FR-029 adds `peers_seen`/`signal_shutdown`/`shutdown` for teardown/tests | `src/lib.rs:336-367` | Low (superseded) |
| SC-002 | Compiles with `(CacheKey, IpcHandle)` param | Compiles with `(CacheKey, u32)` (002) | `iremote_lookup.rs:163-166` | Low (superseded) |

### Not Implemented ✗

None.

---

## Unspecced

| Feature | Location | Lines | Note |
| --- | --- | --- | --- |
| `DISCONNECT_ACK_TIMEOUT` (500 ms bounded wait for DisconnectAck) | `src/actor.rs` | 37, 1020-1033 | Implementation detail backing FR-014; distinct from configurable `connection_teardown_timeout`. The 500 ms handshake bound is a hardcoded constant, not a `LookupConfig` knob. Consider a one-line spec note. |
| Malformed/truncated frame silent drop | `src/actor.rs` | 312-315 | FR-018 addresses *unknown* message types; a `WireError` (truncated/bad-tag/bad-utf8) frame is dropped with no log. Same "logging" gap as the FR-018 drift; fold into that ALIGN task. |
| `publish_success` AlreadyExists size-collision guard | `src/actor.rs` | 576-591 | Refines FR-006's "racing AlreadyExists counts as success" with a size check (matching size ⇒ satisfied; differing size ⇒ reclaim, never evict). Documented in `knowledge/size-mismatch-handling.md` but not in FR text. Superset/defensive; harmless. |

---

## Recommendations

1. **Resolve FR-018 (Low, actionable).** Emit a `logger.debug`/`warn` on both the
   `WireMessage::Unknown` arm and the malformed-decode (`Err(_)`) arm in
   `on_wire` (`src/actor.rs:314,330`) before dropping the frame. This is ALIGN
   Task 3 from the 2026-08-07 sweep, still open. Trivial and low-risk. Closing it
   would make spec 002 fully aligned.
2. **Close the US7 test-coverage gap (medium, not drift).** ALIGN Task 1 /
   tasks.md T025 — add a `tests/mesh.rs` scenario for peer departure (cached-reply
   drop, in-progress-key return, and DisconnectAck-before-reclaim) to exercise the
   already-correct `on_exit`/`teardown_peer` path (backs SC-005/SC-006).
3. **Optional spec note for the 500 ms DisconnectAck bound.** Add a sentence to
   FR-014 (or FR-031) documenting the fixed `DISCONNECT_ACK_TIMEOUT` handshake
   bound so the two teardown timers (ack-handshake vs. orphan grace) are both
   spec-visible.
4. **No action on spec 001.** It is correctly stamped Superseded; leave for
   history. Do not treat its IpcHandle/placeholder divergences as drift.
