---
spec_sync_component: certus-transfer-endpoint
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-26T21:54:05Z
spec_sync_git_commit: 01055b0f
spec_sync_inputs_sha256: 018c121f3ef21fef64d1153f5e238a8a6d424e90054fb93ceb265427e21f2648
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
> **Re-stamp 2026-10-26 (specs only).** `peer-selection.md`: decode -> prefill is the case that matters (corrects the earlier "two prefill pods" lead); `offload_prompt_only` turned off on decode; the by-hand turn-2 test fetched exactly the 100 generated blocks. No source change in this component.

> **Re-stamp 2026-10-26 (specs only).** `peer-selection.md`: milestone 2 built (the router's p2p-source-producer, `pd-certus/router-values.yaml`) and deployed; recorded why one prefill pod never names a peer. No source change in this component.

> **Re-stamp 2026-10-26 (specs only).** `peer-selection.md`: milestone 1 built and verified in k8s, with the pod-to-certus mapping changed to the server publishing `<mailbox>.endpoint` (a pod cannot see the node's RoCE address). No source change in this component.

> **Re-stamp 2026-10-25 (branch `001-rdma-cache-transfer`, rebased onto unstable `a905f89b`).** Specs only since the last stamp: `channel-reclaim.md` (built: OFD-lock channel claims, ABI 2; `CLEAR_ALL`) and `peer-selection.md` (proposed: llm-d chooses the fetch peer); `contracts/shmq-fetch.md` (Prometheus/OTel names, DRAM net of fetched, the run-scaled budget); T087 marked done in tasks.md. No source changes in this component.

> **Sync 2026-10-23 (branch `001-rdma-cache-transfer`).** Since the last stamp: T067 specified (`contracts/shmq-fetch.md`: the FETCH op, its server-side window and caps, `FETCHED` attribution and its counters, the connector's gathering scheme and time budget, how it combines with NIXL); T084-T086 built in `lib/shmq-dispatcher`, `apps/certus-server-yaml` and `certus-shmq-connector` (outside this component, tested there); in this crate, `first_active_rdma_ipv4` (bind-IP auto-detect, `rdma` only, hardware-tested). Each code claim in shmq-fetch.md was checked against the implementation; tasks.md ticks match. No actionable drift.

> **Re-stamp 2026-10-22 (specs only).** Phase 11 is designed and deferred: `cutover.md` added, and tasks.md / plan.md point at it. No code changed; nothing else in the analysis below changes.

> **Sync 2026-10-22 (branch `001-rdma-cache-transfer`) — full analysis, resolved.** The analysis below found 26 items (P01-P26 in `proposals.md`); all are resolved. Code defects fixed, each with a test that fails without the fix: the two request gauges were never written (P07); `IncompatiblePeer` was never constructed (P10); bad-reply teardowns were uncounted, and `protocol_faults` now counts every `OP_ERROR` sent or received and every inbound-caused teardown (P15); a destination could not land in its own memory tier — `fetch` now takes `LocalSlot`s in the pool, `register_landing_region` is gone (P12, P11); an outside-pool lookup is `Failed`, not `Miss` (P25); receives were posted on `Up`, racing the peer's first `OP_HELLO` — they now go with the connect or accept (P18). On review, also: teardown only when it must be (FR-024's four cases), with a request before `OP_HELLO` and a duplicate `OP_HELLO` answered instead (P05); opcodes renumbered so the low bit marks a reply. Every BACKFILL applied to spec.md, the contracts, wire-protocol.md, plan.md, tasks.md and CLAUDE.md (`9e018edb` and the commits above). No actionable drift remains.

> **Sync 2026-10-22 (branch `001-rdma-cache-transfer`) — full analysis, resolved.** The analysis below found 26 items (P01-P26 in `proposals.md`); all are resolved. Code defects fixed, each with a test that fails without the fix: the two request gauges were never written (P07); `IncompatiblePeer` was never constructed (P10); bad-reply teardowns were uncounted, and `protocol_faults` now counts every `OP_ERROR` sent or received and every inbound-caused teardown (P15); a destination could not land in its own memory tier — `fetch` now takes `LocalSlot`s in the pool, `register_landing_region` is gone (P12, P11); an outside-pool lookup is `Failed`, not `Miss` (P25); receives were posted on `Up`, racing the peer's first `OP_HELLO` — they now go with the connect or accept (P18). On review, also: teardown only when it must be (FR-024's four cases), with a request before `OP_HELLO` and a duplicate `OP_HELLO` answered instead (P05); opcodes renumbered so the low bit marks a reply. Every BACKFILL applied to spec.md, the contracts, wire-protocol.md, plan.md, tasks.md and CLAUDE.md (`9e018edb` and the commits above). No actionable drift remains.

# Spec Drift Report

Generated: 2026-10-22T01:03:54Z
Project: certus-transfer-endpoint (crate `certus-transfer-endpoint`, branch `001-rdma-cache-transfer` @ c9429de6)

Scope: `specs/001-rdma-cache-transfer/spec.md` (27 FR, 16 SC), the 16 invariants of
`contracts/itransfer_endpoint.md`, the contract's type/method listings, `contracts/wire-protocol.md`,
the `[X]` tasks in `tasks.md`, and the README.md / CLAUDE.md "Current state" claims — all checked
against `src/` and `components/interfaces/src/{itransfer_endpoint,imemory_tier,idispatcher}.rs`.

Baseline confirmed on this tree: `cargo test -p certus-transfer-endpoint` exit 0 (173 + 2 doc tests),
`cargo clippy -p certus-transfer-endpoint --all-targets -- -D warnings` exit 0,
`cargo doc -p certus-transfer-endpoint --no-deps` 0 warnings. (`rdma`/`spdk` builds not run here.)

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 (+2 contracts) |
| Requirements Checked | 59 (27 FR + 16 SC + 16 contract invariants) |
| ✓ Aligned | 48 (81%) |
| ⚠️ Drifted | 11 (19%) |
| ✗ Not Implemented | 0 (0%) |
| 🆕 Unspecced Code | 10 |

In addition to the 11 requirement drifts, 15 drift items were found in the contract listings,
wire protocol, tasks and docs (CT-, WP-, TK-, DOC-, SPEC-, SRC- below). Five are behavioural and
are flagged as possible code defects: **FR-026** (two `TransferStats` gauges never filled),
**CT-1** (`IncompatiblePeer` never produced), **CT-3** (no way to land a fetch into the
destination's own memory tier), **CT-6** (bad-reply teardowns not counted), **SRC-1**
(outside-pool pointer reported `Miss`).

## Detailed Findings

### Spec: 001-rdma-cache-transfer — Explicit certus-to-certus KV cache transfer over a symmetric RDMA endpoint

#### Aligned ✓

- FR-001 symmetric QP, either end posts → `src/connection/table.rs:1-38` (table keeps either initiator's link), `src/endpoint.rs:1225` (both ends `post_sends`); test `the_same_context_id_live_both_ways_is_two_requests` `src/lib.rs:2275`
- FR-002 one connection per instance pair, reused, on first need → `table.rs:331-345` `ensure`; test `repeated_pushes_reuse_one_connection` `lib.rs:1565`
- FR-003 default build tests over in-process seam → `Cargo.toml` `default = []`; `connection/transport.rs:576-579` mock default; 173 tests pass with no rdma-core
- FR-004 no zyre → `Cargo.toml` dependencies (none zyre)
- FR-005 receives posted unconditionally → `endpoint.rs:1132-1175` `bring_up` on every `TableEvent::Up` (`endpoint.rs:1486-1489`), independent of caller use
- FR-007 per-key outcome, request order, 4 values, one reply per request, Miss≠Failed → `message.rs:313-372`; `serve.rs:278-286` (undecided ⇒ `Failed`); `endpoint.rs:645-677` single `Finish::Reply`
- FR-008 one RDMA write per entry → `endpoint.rs:908-921` one `TrackedSend` per resolved entry
- FR-009 atomic lookup-and-pin, no probe → `serve.rs:155-206` single `map.lookup`
- FR-010 release exactly what was pinned → `serve.rs:165-186` (`NotExist`/`MismatchSize` release nothing; wrong-size/outside-pool/no-block-source released); `Pin` drop-panics `serve.rs:79-88`
- FR-011 per-entry pins; block pin released at read completion before write → `endpoint.rs:1068-1078` (`Pin::release` first), lease returned on write completion `endpoint.rs:196-203`
- FR-012 bounce reserve, no promotion → `bounce.rs:1-18`, `DispatcherSource` `bounce.rs:37-46`; lease must lie in registered pool `endpoint.rs:868-870`; test `serving_the_block_tier_leaves_the_cache_alone` `lib.rs:3387`
- FR-013 concurrent serving bounded by credits → `endpoint.rs:838-975`
- FR-015 chunk, never fail, on exhaustion → `endpoint.rs:785-787` (`try_send_request` false ⇒ wait), `endpoint.rs:925-934` (defer), `table.rs:396-406` (partial post)
- FR-017 teardown before reclaim → `endpoint.rs:456-498` (`begin_teardown`→`drain`→settle→`finish_teardown`→`fail_fetches`)
- FR-018 no partial publication → `endpoint.rs:212-219` (only `Ok` completion is a hit)
- FR-019 legacy push entry point → `interfaces/src/itransfer_endpoint.rs:611-632`; `lib.rs:646-697`
- FR-021 instance-granular `PeerId`, port mandatory → `connection/mod.rs:79-131`; SC-009 test `table.rs:1024`
- FR-022 no phase barrier → `endpoint.rs:838-946` (memory writes post as resolved; block reads on reader threads `bounce.rs:91-119`); test `memory_tier_writes_do_not_wait_for_device_reads` `lib.rs:3152`
- FR-023 deadline ordering ERROR→drain→reclaim→reconnect on need, blast radius logged → `endpoint.rs:562-631`, `endpoint.rs:504-522`; test `an_expired_deadline_errors_the_qp_then_fails_every_fetch` `lib.rs:2788`
- FR-027 exactly one connection, lower-initiator wins, at accept → `table.rs:741-776`; test `a_race_counts_one_duplicate_across_the_pair` `table.rs:1748`
- SC-001 zero zyre, zero third-instance messages → test with bystander `lib.rs:2125`
- SC-003 no rdma-core/zyre build → default build green
- SC-004 pins ≤ send credits + readers → `lib.rs:1086`, `lib.rs:3195`
- SC-005 block-tier serving leaves occupancy/LRU order unchanged → `lib.rs:3387`
- SC-006 all-miss, no further traffic → `lib.rs:2184`
- SC-007 scarcity ⇒ chunks, order preserved → `lib.rs:3466`, `lib.rs:3518`, `lib.rs:2606`, `lib.rs:3232`
- SC-009 port-distinct instances → `table.rs:1024`; `connection/mod.rs:146-213`
- SC-010 late write cannot land; siblings aborted; rkey survives reconnect → `lib.rs:2788`, `lib.rs:2979`
- SC-011 memory writes posted before first device read completes → `lib.rs:3152`
- SC-012 unknown opcode answered within a round trip, connection survives → `lib.rs:2005`, `lib.rs:2035`
- SC-013 peer's smaller max respected; non-intersecting range fails at connect → `lib.rs:2606`, `lib.rs:1802`
- SC-014 commutative negotiation → `session.rs:49-55`, test `session.rs:318`
- SC-015 one connection after simultaneous connect → `table.rs` race tests (`table.rs:1131-1210`, `1748`)
- SC-016 full-quota two-way traffic progresses → `lib.rs:2710`, `lib.rs:3699`
- INV-1 order & length across chunking → `endpoint.rs:1398-1407` (placed at chunk offset); `lib.rs:1525`
- INV-2 outcome honesty; no-`spdk` block ⇒ `Miss` → `serve.rs:177-180`, `lib.rs:349-358`; `lib.rs:3357`
- INV-3 no broadcast → no code path contacts an unnamed peer; `lib.rs:2125`
- INV-4 pin discipline → as FR-009/010/011; deferral releases pin `endpoint.rs:925-934`
- INV-5 pin bound → as SC-004
- INV-6 tier neutrality — honestly disclosed as unmet for memory tier (`serve.rs:26-31`), met for block tier
- INV-7 no partial publication → as FR-018
- INV-8 teardown before reclaim → as FR-017; `lib.rs:1599`
- INV-9 progress under scarcity; `NeverFits` ⇒ `Failed`, logged, counted → `endpoint.rs:1110-1124`; `lib.rs:3261`
- INV-11 no phase barriers → as FR-022
- INV-12 instance addressing → as FR-021
- INV-14 capabilities precede data → `endpoint.rs:785-787`, `endpoint.rs:842-848`; `lib.rs:1928`
- INV-15 separate books → `endpoint.rs:258-271` (`Conn.outbound`) vs pushes with `Finish::Reply`; `session.rs` per-direction credits
- INV-16 one connection survives a race → as FR-027

#### Drifted ⚠️

- **FR-006** — spec says a request carries "a context ID, the destination's rkey, base address, per-entry size, and a list of `(block key, landing slot index)` pairs". Code (and wire-protocol.md, which explicitly rejects slot indices) carries per-entry `(key u64, addr u64, size u32, rkey u32)` with no base address or slot index.
  - Location: `src/message.rs:253-279` (`encode_fetch`), `src/message.rs:282-309`; spec.md:109
  - Also stale in Key Entities: `TransferContext` lists "landing slots, outstanding-entry count, deadline" — there is no such struct; the state is `Fetch`/`Chunk` + `Conn.outbound` (`endpoint.rs:258-305`) with no outstanding-entry count. `RemoteRegion — rkey, address, size`: the field is `length` (`interfaces/src/iremote_lookup_rdma_initiator.rs:62-69`).
  - Severity: moderate (inter-document contradiction with wire-protocol.md)

- **FR-014** — spec says send-queue credits bound "in-flight RDMA writes per connection". Code counts **every** send-queue request — message SENDs take credits too (`flush_outbox` → `table.post_sends`).
  - Location: `src/endpoint.rs:1204-1238`, `src/connection/table.rs:396-406`; spec.md:225. Same wording in `TransferStats::send_queue_in_flight` doc ("In-flight RDMA writes", `interfaces/src/itransfer_endpoint.rs:384`). CLAUDE.md and README already describe the code's behaviour.
  - Severity: minor (the code is stricter; SC-004 still holds)

- **FR-016 / INV-10** — "the deadline MUST run from when the request is sent" / EndpointConfig doc "handed to the transport". Code starts the clock when the chunk is encoded into the connection's outbox after taking a message credit (`ChunkState::Sent { at: Instant::now() }`), before `flush_outbox` posts it — which can itself wait for a send-queue credit behind RDMA writes.
  - Location: `src/endpoint.rs:796-806`, `src/endpoint.rs:1204-1238`; `interfaces/src/itransfer_endpoint.rs:467-473`
  - Severity: minor (the gap is one poll in practice; a message-credit wait correctly does not count)

- **FR-020** — "`remote-lookup` MUST NOT be modified"; tasks.md:345 "`remote-lookup` has not been modified". The branch adds `set_bounce_reserve`/`bounce_alloc`/`bounce_free`/`bounce_usage` and `read_to_bounce`/`release_bounce` impls to `components/remote-lookup/src/seams.rs` (a `pub mod`, not test-only) — forced by T083's interface growth, no logic change.
  - Location: `components/remote-lookup/src/seams.rs:589-603, 803-817` (git diff unstable...HEAD); commit cfeeccda
  - Severity: minor

- **FR-024 / INV-13** — "The endpoint MUST answer a **request** it cannot process with an explicit protocol error reply … never with silence." Several request faults tear the connection down instead of answering:
  - an `OP_FETCH` over the advertised quota or before `OP_HELLO` (`Violation::OverQuota`/`BeforeHello`) → `Err` → teardown (`endpoint.rs:1410-1413`; test `requests_past_the_quota_tear_down` `lib.rs:2342`);
  - an `OP_HELLO` that parses as a header but fails `decode_hello` (wrong length, context id, reserved field) or is a duplicate → teardown (`endpoint.rs:1311-1316`) — `OP_HELLO` is a request per wire-protocol.md;
  - an `OP_ERROR` whose `offending_opcode` is not `OP_FETCH` (e.g. a refusal of our `OP_HELLO`) → teardown (`endpoint.rs:1340-1343`), whereas the wire says `OP_ERROR` fails one context.
  - These are deliberate and tested (tasks.md T032 records the quota teardown), but the spec states no exception.
  - Severity: moderate

- **FR-025** — "A requester MUST respect the peer's maximum request size rather than its own." Code chunks to `min(peer.max_entries, local MAX_ENTRIES)` and additionally fails the connection on `max_entries == 0` (unspecced).
  - Location: `src/session.rs:215-221`; `message.rs:259-262` (`encode_fetch` asserts ≤ 340)
  - Severity: minor (a single 8 KiB send buffer cannot encode more than 340 anyway)

- **FR-026** — the contract's `stats()` promises "the credit domains (per direction)", and `TransferStats` exposes `requests_outstanding_outbound` and `requests_in_service_inbound`. **Neither is ever written**: they are only copied from `Counters` (`telemetry.rs:49-52, 100-101`), which nothing increments. `Session::outbound()`/`inbound()` (`session.rs:172-180`) hold the real values but are read only by test code (`endpoint.rs:1724-1731`). `telemetry.rs:49-52` says "Gauge; filled by T079", and T079 is `[X]`. Both gauges read 0 under load — an unexercised-looking path that is in fact exercised (Principle XI).
  - Location: `src/telemetry.rs:49-52`, `src/connection/table.rs:302-311`, `src/session.rs:172-180`
  - Severity: moderate — **code defect**

- **SC-002** — "The existing `remote-lookup` test layer and the initiator's `loopback_push_writes_into_remote_buffer` hardware loopback test pass **against this endpoint**." The endpoint reproduces the loopback scenario in its own `loopback_test.rs:220-270` (T055) and the push surface over the mock (T021). The `remote-lookup` test layer has never been run against this endpoint — it cannot be until the receptacle is re-pointed (T065, Phase 11, out of scope). The US2 "Independently testable" paragraph says the same.
  - Location: `src/loopback_test.rs:220-270`; tasks.md:1020, 914
  - Severity: moderate

- **SC-008** — "The duplicated verbs FFI layer is defined once." This crate has one copy (`src/ffi.rs`), but both predecessor copies remain (`remote-lookup-rdma-initiator/src/ffi.rs`, `remote-lookup-rdma-responder/src/ffi.rs`), so the workspace now holds three. "Defined once" holds only after T066 (out of scope). tasks.md traceability marks SC-008 discharged by T007.
  - Location: `src/ffi.rs`; tasks.md:1026, 917
  - Severity: minor

#### Not Implemented ✗

- none

### Contract, wire-protocol, task and documentation drift (not in the requirement count)

- **CT-1 `IncompatiblePeer` is never produced** — `TransferEndpointError::IncompatiblePeer` (`interfaces/src/itransfer_endpoint.rs:263-266`) is documented as a `fetch` error (`:588`) and in the contract (`itransfer_endpoint.md` error enum). Nothing constructs it (grep: only the definition and `Display`). A non-intersecting `OP_HELLO` returns `Err` from `on_message` → teardown (`endpoint.rs:1314-1316`), and any fetch waiting on that peer fails through `fail_fetches(peer, None)` as `Transport("the connection to … was lost")` (`endpoint.rs:519`). `fetch()` itself can never return it synchronously, since it returns before any connection exists (`lib.rs:613-638`). The diagnostic naming both ranges goes to the log only. — moderate, **code/doc defect**
- **CT-2 `fetch` doc claims `NotInitialized` "if no landing region is registered"** (`interfaces/src/itransfer_endpoint.rs:589-591`, and the error doc `:247`). `fetch` checks no landing region (`lib.rs:613-638`). — minor
- **CT-3 Landing into the destination's own memory tier is not reachable through the interface** — US1 says the destination "reserves unpublished slots in its own memory tier"; the contract says `register_landing_region`'s region is "in practice the memory tier's own pool" (`itransfer_endpoint.md:133-135`). The code registers the pool itself once, at `initialize`, with `REMOTE_WRITE` (`lib.rs:510-521`), but **refuses** any landing region overlapping the pool (`endpoint.rs:1519-1523`, interface doc `:550`), and no public method returns the pool's handle or rkey (`PoolKey` is `#[cfg(test)]`, `endpoint.rs:125-126`). So a real caller cannot land a fetch in memory-tier slots — only in a separately allocated region. Every in-crate test that lands in a pool uses the test-only `pool_key` (`lib.rs:983`). — **major**, integration gap
- **CT-4 Contract `IMemoryTier` listing is stale** — contract has `bounce_alloc(&self, len: u32) -> Result<*mut u8, BounceError>` and `bounce_free(&self, ptr: *mut u8, len: u32)` (`itransfer_endpoint.md:378-379`); the real trait has `bounce_alloc(&self, len: u32) -> Result<BounceLease, BounceError>` and `bounce_free(&self, lease: BounceLease)` (`interfaces/src/imemory_tier.rs:230, 234`). — minor
- **CT-5 "Until `initialize` succeeds, every `ITransferEndpoint` method returns `NotInitialized`"** (`itransfer_endpoint.md:237`). `stats()` cannot — it returns all-zero `TransferStats` (`lib.rs:707-710`). — minor
- **CT-6 `protocol_faults` semantics** — `TransferStats::protocol_faults` doc: "Protocol error replies received **or sent**" (`interfaces/src/itransfer_endpoint.rs:409`). The code counts only a *received* `OP_ERROR` (`endpoint.rs:1331-1333`); sending one does not count, and a test asserts it (`lib.rs:2019-2025`, "b answered; it did not fault"). wire-protocol.md *Bad replies* says the receiver "counts it as a protocol fault", and its *Open* paragraph relies on "the protocol-fault and deadline counters make such a loop visible at once" — but a bad-reply teardown (`endpoint.rs:1016-1023`) increments **nothing**. A bad-reply teardown loop is therefore invisible in `stats()`. — moderate, **code/doc defect**
- **WP-1 Frozen kinds' `version` "ignored"** — wire-protocol.md:40 and header table :70 say the version field is "zero and ignored" for `OP_HELLO`/`OP_ERROR`. Code requires exactly 0 for every frozen kind (incl. `OP_HELLO_REPLY`) and rejects a non-zero one as `UnsupportedVersion` (`message.rs:231-237`; test `message.rs:546-566`). — minor
- **WP-2 Reply size arithmetic** — "bounds `OP_FETCH_REPLY` at `24 + 340` = 364 bytes" (wire-protocol.md:251). The body is padded to 8 bytes (`message.rs:332-358`), so a 340-entry reply is 24 + 344 = **368** bytes. — minor
- **WP-3 Receives are posted after establishment, not before** — wire-protocol.md:142-145 ("This requires each end to have at least one reply-reserve buffer and one quota buffer posted **before the connection is established**") and T080 (tasks.md:493-494). Code posts reserve + quota in `bring_up`, run on `TableEvent::Up`, i.e. after ESTABLISHED (`endpoint.rs:1132-1175`, `1486-1489`). A test acknowledges the gap: "Bring-up can cost one RNR stall: the initiator's OP_HELLO can reach the acceptor before its thread has handled `Up` and posted receives (RNR retry absorbs it)" (`lib.rs:2731-2735`). It is safe only because verbs uses `rnr_retry_count = 7` (infinite, `connection/verbs.rs:53-57`); with a finite RNR retry the bring-up could fail. The verbs accept path already allows early posting ("Receives may be posted at once", `verbs.rs:356`). — moderate
- **WP-4 Address grammar** — contract `itransfer_endpoint.md:28` and wire-protocol.md:322 say `"ip:port"`; code accepts any host (`"host:port"`, bracketed IPv6, no resolution, `connection/mod.rs:86-131`), as the interface docs say. — minor
- **SPEC-1 "highest supported version for that kind"** — spec.md:74 (edge case) and :287 (Key Entity `ProtocolError`). One version governs the protocol (FR-025, D21); `max_supported_version` is the sender's protocol maximum, or 0 for an unknown kind (`message.rs:496`). — minor
- **TK-1 T031** says "Implement the context table in `src/message.rs`: `TransferContext` = context id, peer, key list, landing slots, outstanding-entry count, deadline". No `TransferContext` exists; the tables are in `endpoint.rs` (`Conn.outbound`, `Fetch`, `Chunk`, `endpoint.rs:258-305`). `message.rs:23` still says "Context tables (T031) and message credits (T032) land in later slices." — minor
- **DOC-1 CLAUDE.md "Current state" is internally stale** — it begins "T001–T083 are done" and ends "`push_async` (T017) serves memory-tier entries end to end … `disconnect` and `stats` work; the rest say which task they land in. The verbs side of the seam is T053." (CLAUDE.md:273-288). Every method is implemented, and block-tier serving and the verbs transport are built. — minor
- **DOC-2 plan.md "Next Step"** says "67 tasks, T001–T067 … Begin at T001–T002" (plan.md:228-241); tasks run to T083 and Phases 1-10 are done. — minor
- **SRC-1 stale source docs** (code is right, its comments are not):
  `lib.rs:643` "block-tier (until T037-T040)" — block tier is served; `telemetry.rs:15-20` "Several `TransferStats` fields belong to tasks not yet built" (true only of the two FR-026 gauges, whose tasks are marked done); `session.rs:31` and `message.rs:25` `#![allow(dead_code)]` "consumed … in slice C"/"from T030 onward"; `message.rs:23` (TK-1). — minor
- **SRC-2 outside-pool lookup reported `Miss`** — a memory-tier lookup whose pointer lies outside the registered pool is released and reported `Miss` (`serve.rs:185-187`), with only a `warn` (`endpoint.rs:902-906`). FR-007 defines `Miss` as "absent, a different size, or a tier this build cannot serve", and Failed as "any other reason". A pointer outside the pool is a wiring defect, not residency, so under Principle XI it looks like a cold cache. — minor/moderate, possible defect

### Unspecced Code 🆕

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| Abort-on-panic on endpoint and reader threads (`fatal_on_panic`) | `src/endpoint.rs:156-174`, `bounce.rs:105-107` | ~25 | new FR (Principle XI) |
| `push` refused on an endpoint thread (deadlock guard) | `src/lib.rs:676-682`, `endpoint.rs:141-150` | ~15 | FR-019 note |
| Landing-region validation: null/empty/wrap/pool-overlap/region-overlap refused; no deregistration | `src/endpoint.rs:1508-1546` | ~40 | contract + CT-3 |
| Simultaneous-connect extensions: "newer wins", `Draining` reject, dialed-name `NameMismatch`, self-connect refusal | `src/connection/table.rs:13-38, 331-334, 741-776, 806-816` | ~80 | FR-027 (research D24 only) |
| Frozen connect `private_data` identity format (`'C' 'T' 0x01 len …`) | `src/connection/identity.rs:1-110` | ~110 | wire-protocol (D24 only) |
| Shutdown drain-stall leak (`DRAIN_STALL` 5 s; table + registrations leaked rather than destroyed) | `src/endpoint.rs:75, 1600-1651` | ~55 | FR-017 / Admin contract |
| Feature-gated `Extras` connect counters | `src/telemetry.rs:117-180` | ~60 | stats contract |
| Send credits clamped to device grant, logged | `table.rs:834-845`, `endpoint.rs:1475-1483` | ~15 | FR-014 |
| Strict reply validation (non-zero padding, reserved fields ⇒ malformed) | `src/message.rs:220-229, 360-372` | ~15 | wire-protocol |
| Block-reader "stale NoSpace" free-count guard | `src/bounce.rs:66-72, 139-146` | ~15 | FR-014 bounce domain |

## Inter-Spec Conflicts

1. spec.md FR-006 (base address + slot index) vs wire-protocol.md `OP_FETCH` (per-entry absolute address + rkey, slot indices explicitly rejected).
2. contract `register_landing_region` ("in practice the memory tier's own pool") vs interface doc and code (overlapping the pool is an error).
3. `TransferStats::protocol_faults` doc ("received or sent") vs wire-protocol *Bad replies* ("counts it as a protocol fault") vs code/test (received `OP_ERROR` only).
4. wire-protocol / T080 (receives posted before establishment) vs code and the SC-016 test comment (posted at `Up`).

## Recommendations

1. Fix the two dead `TransferStats` gauges (FR-026) — the clearest Principle XI breach found.
2. Decide CT-3 (how a destination lands into its own memory tier) before the connector/shm-queue layer (T067) is specified; today only test code can do it.
3. Decide CT-1 and CT-6 together: either make `IncompatiblePeer` and bad-reply counting real, or remove the claims.
4. Backfill the remaining spec wording (FR-006, FR-014, FR-016, FR-024, FR-025, SC-002, SC-008, WP-*, SPEC-1). Note that changing `interfaces/src/itransfer_endpoint.rs` doc comments changes the interfaces digest and forces a re-stamp of every component.
