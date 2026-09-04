---
spec_sync_component: remote-lookup
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-04T00:20:50Z
spec_sync_git_commit: 85c17e8e
spec_sync_inputs_sha256: 9095d0013c312b91de32d0bfa6f4cdadd0e3382b50290f2b0e2dc84433139cab
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec-vs-Implementation Drift Report — remote-lookup

Analysis of `components/remote-lookup` source against its two specs. Spec
`002-remote-lookup-rdma` is the **design-of-record**; spec
`001-remote-lookup-placeholder` is **superseded** (its Supersession Notice is
honored — divergences from 001 are intentional and non-actionable).

**This sweep (2026-09-03)** independently re-verified every spec-002 FR/SC against
`src/` and applied three fixes:
1. **FR-018 (Low) — ALIGN**: `on_wire` now *logs* both the malformed-decode arm and
   the `WireMessage::Unknown` arm via the optional `ILogger` before dropping the
   frame (previously both silent). The spec's "logged and ignored" requirement is now
   fully met.
2. **`bind_ip` — BACKFILL**: FR-033's `LookupConfig` field enumeration omitted the
   load-bearing `bind_ip` field (the prior report also missed this — it marked FR-033
   aligned). Backfilled into FR-033.
3. **`seams.rs:692` — ALIGN (lint)**: a `clippy::clone_on_copy` `-D warnings` error
   (`h.clone()` on the `Copy` type `IpcHandle`) that the prior report never noted —
   it lurked because remote-lookup is **not** a default CI member, so CI never clippy's
   it. Fixed to `*h`.

## Summary

| Metric | Count |
| --- | --- |
| Specs Analyzed | 2 |
| Requirements Checked | 60 |
| Aligned (after this sweep) | 60 (spec-002) + superseded-honored (spec-001) |
| Drifted → resolved this sweep | 2 (FR-018 ALIGN, `bind_ip` BACKFILL) + 1 lint (seams.rs:692) |
| Drifted (superseded / non-actionable) | 5 (intentional supersession of spec 001) |
| Not Implemented | 0 |
| Unspecced Features | 2 (down from 3 — malformed-frame drop is now covered by FR-018(b)) |

**Verification this sweep.** Lib build + lib clippy are green:
- `cargo build -p remote-lookup --lib` — clean
- `cargo clippy -p remote-lookup --lib -- -D warnings` — clean (confirms the
  `seams.rs:692` lint fix and the FR-018 logging edit compile lint-clean)

> **Environmental verification gap (not drift).** The full `tests/mesh.rs` suite
> (SC-001..SC-008 automated coverage) and `cargo clippy --all-targets` could **not**
> be executed this sweep: the crate's `zyre` dev-dependency requires `deps/zyre-build/`,
> which is absent in this environment, so `--all-targets`/`cargo test` fail to link.
> The SC test bodies are present in the tree and passed in prior sweeps; the code paths
> they exercise are unchanged by this sweep's edits (which touch only `on_wire`
> logging, a mock-seam lint, and a spec doc). This is an environment limitation, not a
> code or spec defect.

---

## Spec 002 — Remote Lookup over Zyre + RDMA (design-of-record)

45 requirements checked (37 FR incl. 006a/016a/016b, 8 SC). All aligned after this sweep.

### Resolved this sweep

**FR-018 — ALIGN (code fix).** Spec: "Unknown message types MUST be **logged** and
ignored"; the 2026-08-20 backfill extended this to malformed/truncated frames
(class (b)). Before this sweep both arms were silent (`WireMessage::Unknown { .. } => {}`
and `Err(_) => return`). `on_wire` (`src/actor.rs`) now logs both via the optional
`ILogger` receptacle before dropping: the malformed arm logs sender + byte length +
decode error; the `Unknown` arm logs sender + `version`/`msg_type`/`op_id`. Both
classes remain ignored (poll loop continues). FR-018's spec note was updated to record
the logging half as met.

**FR-033 — BACKFILL (spec doc).** FR-033 enumerated `LookupConfig` fields beyond
FR-022's knobs (`actor_cpu`, `discovery`, `node_endpoint`) but omitted `bind_ip`
(`interfaces/src/iremote_lookup.rs:63`, default `String::new()` at :92), which is
load-bearing: `src/lib.rs:146` forwards `config.bind_ip` to the responder admin via
`set_bind_ip` during `initialize`. Code is authoritative (the field exists and is
used); the spec under-enumerated it → BACKFILL, not ALIGN. Verified the remaining
non-FR-022 fields are already specced (`caller_wait` at the FR near spec.md:479;
`connection_teardown_timeout` at FR-014/spec.md:376), so `bind_ip` was the only gap.

**seams.rs:692 — ALIGN (lint hygiene).** `batch_populate`'s mock called
`self.populate(*k, h.clone())` where `IpcHandle` is `Copy`
(`interfaces/src/idispatcher.rs:130`), tripping `clippy::clone_on_copy` under
`-D warnings`. Fixed to `*h`. (Not previously reported; surfaced only under a
component-local `cargo clippy` since remote-lookup is not a default CI member.)

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
| FR-018 (framing + op_id echo; unknown/malformed frames **logged** and ignored) | `src/actor.rs` `on_wire` (both arms log via `ILogger`); `src/wire.rs:99-129` |
| FR-019 (stale op_id discarded without error) | `src/actor.rs:412-414,498-509` |
| FR-020 (multiple concurrent ops keyed by op_id; caller blocks on its op) | `src/actor.rs:161,682-727`; `src/lib.rs:291-313` |
| FR-021 (ignore SHOUT whose peer id == own uuid) | `src/actor.rs:337-340` |
| FR-022 (LookupConfig public, Default, sensible defaults) | `components/interfaces/src/iremote_lookup.rs:28-98` |
| FR-023 (8 receptacles) | `src/lib.rs:44-53` |
| FR-024 (no direct RDMA logic; via initiator/responder) | `src/actor.rs`, `src/worker.rs`, `src/server.rs` (delegated) |
| FR-025 (responder_admin bring-up; read Endpoint + rkey; open control channel) | `src/lib.rs:142-166` |
| FR-026 (single-flight per-key in-flight index; follower waits) | `src/actor.rs:102-108,428-434,541-554` |
| FR-027 (advertise responder endpoint header; warm on Enter, best-effort) | `src/lib.rs:177-180`; `src/actor.rs:393-399`; `src/worker.rs:77-79` |
| FR-028 (no blocking RDMA on poll loop; off-loop worker returns statuses) | `src/actor.rs:356-371`; `src/worker.rs:71-114` |
| FR-029 (out-of-interface hooks peers_seen/signal_shutdown/shutdown) | `src/lib.rs:336-374` |
| FR-030 (caller_wait decouples caller block from op_deadline) | `src/lib.rs:301-313`; `iremote_lookup.rs:44-50` |
| FR-031 (timer-driven force-reclaim backstop; tick_orphans) | `src/actor.rs:110-121,264-268,849-885` |
| FR-032 (orphan-reuse guard: no re-reserve of orphaned key) | `src/actor.rs:441-443,630-632,772-774` |
| FR-033 (LookupConfig also carries bind_ip/actor_cpu/discovery/node_endpoint) | `iremote_lookup.rs:63-78`; `src/lib.rs:146-149,171-173` |
| FR-034 (`integrity-check` Cargo feature forwards interfaces/integrity-check) | `Cargo.toml:9-14` |
| SC-001..SC-008 | `tests/mesh.rs` (see environmental note above); Phase-1 greediness `src/actor.rs:420-485`; wrong-size guard `src/server.rs:137-160,218-236` |

### Not Implemented ✗

None. (US7 peer-departure behavior is fully implemented in `on_exit`/`teardown_peer`;
the automated mesh test for it — tasks.md T025 — remains a test-coverage gap, not a
missing feature. See Recommendations.)

---

## Spec 001 — Remote Lookup Batch Interface (SUPERSEDED)

15 requirements checked (10 FR, 5 SC). **Superseded by 002** — divergences are
intentional and NON-ACTIONABLE, per 001's Supersession Notice: `batch_lookup` takes
`(CacheKey, u32)` not `(CacheKey, IpcHandle)` (002 FR-001, CPU/DRAM-only); real
KEY_QUERY→RDMA protocol replaces the per-entry placeholder log (FR-003/004); FR-029
adds `peers_seen`/`signal_shutdown`/`shutdown` beyond the "only via IRemoteLookup"
rule (FR-008). All aligned-under-002 signatures still hold (`src/lib.rs`,
`iremote_lookup.rs`). No action; leave for history.

---

## Unspecced

| Feature | Location | Note |
| --- | --- | --- |
| `DISCONNECT_ACK_TIMEOUT` (500 ms bounded wait for DisconnectAck) | `src/actor.rs:37,1020-1033` | Implementation detail backing FR-014; distinct from configurable `connection_teardown_timeout`. Hardcoded constant, not a `LookupConfig` knob. Optional one-line spec note (Recommendation 3). |
| `publish_success` AlreadyExists size-collision guard | `src/actor.rs:576-591` | Refines FR-006's "racing AlreadyExists counts as success" with a size check. Documented in `knowledge/size-mismatch-handling.md`; defensive superset, harmless. |

*(The "malformed/truncated frame silent drop" formerly listed here is resolved: it is
now covered by FR-018(b) and the malformed arm logs before dropping.)*

---

## Recommendations

1. **Close the US7 test-coverage gap (medium, not drift).** tasks.md T025 — add a
   `tests/mesh.rs` scenario for peer departure (cached-reply drop, in-progress-key
   return, DisconnectAck-before-reclaim) exercising the already-correct
   `on_exit`/`teardown_peer` path (backs SC-005/SC-006). Blocked in this environment
   by the missing `deps/zyre-build/` (see environmental note).
2. **Optional spec note for the 500 ms DisconnectAck bound.** Document the fixed
   `DISCONNECT_ACK_TIMEOUT` handshake bound in FR-014/FR-031 so both teardown timers
   (ack-handshake vs. orphan grace) are spec-visible.
3. **No action on spec 001.** Correctly stamped Superseded; its IpcHandle/placeholder
   divergences are intentional, not drift.
