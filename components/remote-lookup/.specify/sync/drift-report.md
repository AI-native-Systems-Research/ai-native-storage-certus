Generated: 2026-08-07T15:31:15Z

# Spec-vs-Implementation Drift Report — remote-lookup

Two specs govern this component:

- `specs/001-remote-lookup-placeholder/spec.md` — **Superseded** (2026-07-22) by 002. Retained
  for history; its placeholder FRs are intentionally not implemented (divergence resolved by 002).
- `specs/002-remote-lookup-rdma/spec.md` — **design-of-record** (marked `Status: Synced`).

Implementation reviewed: `src/{lib,actor,operation,server,worker,wire,seams}.rs`,
interface `components/interfaces/src/iremote_lookup.rs`, `Cargo.toml`, `tests/mesh.rs`,
`tests/mesh_rdma.rs`, `benches/correlation.rs`.

## Summary

| Spec | Aligned | Drifted | Not Implemented |
|------|:-------:|:-------:|:---------------:|
| 001-remote-lookup-placeholder (superseded) | 8 | 4 (all superseded/intentional) | 3 (placeholder behavior removed) |
| 002-remote-lookup-rdma (design-of-record) | 38 | 2 | 0 |
| **Totals** | **46** | **6** | **3** |

Unspecced code items: **5**.

**Headline finding:** the code matches spec 002 almost exactly, but `src/lib.rs`'s own module and
`batch_lookup` doc comments are **stale** — they claim the KEY_QUERY→RDMA protocol is not yet built
and that `batch_lookup` "finalizes every key as NotFound." `src/actor.rs` fully implements the
protocol (US1–US7) and `tests/mesh.rs` exercises it end-to-end. This directly contradicts spec 002's
`Synced` status and the shipped behavior. (Recorded as 002 FR-001 doc-drift, Medium.)

All 001-superseded divergences are the ones the 002 Supersession Notice already documents
(`IpcHandle` dropped, placeholder logging/NotFound removed, out-of-interface `pub fn`s added).
They are **not actionable** here — do not "fix" the code back to 001.

---

## Detailed Findings — Spec 001 (Superseded, intentional divergence)

| ID | Status | Sev | Evidence | Note |
|----|--------|-----|----------|------|
| FR-001 | Drifted | Low (superseded) | `interfaces/src/iremote_lookup.rs:163` | Signature is `&[(CacheKey,u32)]`, not `&[(CacheKey,IpcHandle)]`. Intentional per 002 Clarifications Q1. |
| FR-002 | Aligned | — | `src/operation.rs:149-160`; `src/lib.rs:271` | One positional `Result` per entry, caller order preserved. Still holds under 002. |
| FR-003 | Not Implemented | Low (superseded) | `src/actor.rs` (no per-entry log) | Placeholder "log each entry" behavior removed; component now does real network I/O. |
| FR-004 | Drifted | Low (superseded) | `src/actor.rs:280-308` | No longer returns NotFound-for-all with no I/O; real KEY_QUERY→RDMA runs. Intentional. |
| FR-005 | Aligned | — | `src/lib.rs:271-285` | `batch_lookup` callable any time; returns NotFound when uninitialized (no actor). |
| FR-006 | Aligned | — | `src/lib.rs:272-274` | Empty slice → empty `Vec`. |
| FR-007 | Aligned | — | `interfaces/src/iremote_lookup.rs:120` | Interface lives at `components/interfaces/src/iremote_lookup.rs`. |
| FR-008 | Drifted | Low (superseded) | `src/lib.rs:335-363` | Component exposes `pub fn`s outside `IRemoteLookup` (`peers_seen`,`signal_shutdown`,`shutdown`). Superseded/authorized by 002 FR-029. |
| FR-009 | Not Implemented | Low (superseded) | `src/lib.rs:314-320` | `join_cluster` no longer "log endpoint + Ok(())"; routes to actor and errors if uninitialized. |
| FR-010 | Not Implemented | Low (superseded) | `src/lib.rs:323-329` | `leave_cluster` no longer "log + Ok(())"; routes to actor and errors if uninitialized. |
| SC-001 | Aligned | — | `src/lib.rs:372-451`; `src/wire.rs:394-563` | Unit tests present. |
| SC-002 | Drifted | Low (superseded) | `interfaces/src/iremote_lookup.rs:163` | Interface no longer uses `(CacheKey,IpcHandle)`; `IpcHandle` deliberately dropped. |
| SC-003 | Aligned | — | `interfaces/src/iremote_lookup.rs:130-166` | Runnable doctests on interface methods. |
| SC-004 | Aligned (assumed) | — | — | No obvious clippy triggers; not run in this pass. |
| SC-005 | Aligned (assumed) | — | doc comments throughout | Public APIs documented; `cargo doc` not run in this pass. |

## Detailed Findings — Spec 002 (design-of-record)

| ID | Status | Sev | Evidence | Note |
|----|--------|-----|----------|------|
| FR-001 | **Drifted** | **Medium** | `src/lib.rs:5-8,251-253` vs `src/actor.rs:280-1010` | Interface + signature aligned, BUT lib.rs module/`batch_lookup` docstrings falsely state the protocol is unbuilt / returns all-NotFound. Stale doc contradicts shipped code and 002 `Synced`. |
| FR-002 | Aligned | — | `src/lib.rs:232-235`; `src/actor.rs:174-275` | Actor on dedicated OS thread owns `IZyreNode`, polls in loop. |
| FR-003 | Aligned | — | `src/lib.rs:189`; `src/actor.rs:1042`; `iremote_lookup.rs:84` | Joins configured group (default `remote_lookup`), leaves on shutdown. |
| FR-004 | Aligned | — | `src/wire.rs:99-101`; `src/server.rs:138-158` | `(key,size)` wire identity; size mismatch → not available. |
| FR-005 | Aligned | — | `src/actor.rs:280-308` | SHOUT KEY_QUERY, split by `max_keys_per_query`, per-op state init. |
| FR-006 | Aligned | — | `src/actor.rs:444-468,561-591` | Private slot via `memory_tier.insert`; publish-on-success; discard on failure. |
| FR-006a | Aligned | — | `src/actor.rs:405-488` | Greedy per-KEY_RESPONSE memory-hit dispatch; caches full reply; skips in-progress/satisfied. |
| FR-007 | Aligned | — | `src/actor.rs:478-484`; `src/lib.rs:158-161` | RDMA_REQUEST carries endpoint + `RemoteRegion{addr,rkey,length}`; pool-wide rkey cached at startup. |
| FR-008 | Aligned | — | `src/actor.rs:515-518,561-591` | On RDMA_STATUS(Success) publish + mark satisfied. |
| FR-009 | Aligned | — | `src/actor.rs:519-533` | On failure: reclaim slot immediately (peer live), return to Unsatisfied, note tried. |
| FR-010 | Aligned | — | `src/actor.rs:682-810` | Phase-2 re-scans cached replies (no new SHOUT), disk holders, prefers untried. |
| FR-011 | Aligned | — | `src/actor.rs:599-660` | Bounded `max_retry_rounds`; memory-then-disk; never re-targets a failed peer. |
| FR-012 | Aligned | — | `src/actor.rs:682-727,815-838,889-942` | Completion at all-satisfied / no-holder / retry-cap / deadline; unsatisfied → NotFound. |
| FR-013 | Aligned | — | `src/actor.rs:950-1010` | On Exit: drop cached reply, return in-progress keys, re-evaluate. |
| FR-014 | Aligned | — | `src/actor.rs:1015-1034` | `Disconnect`→block for `DisconnectAck` before physical reclaim. |
| FR-015 | Aligned | — | `src/server.rs:131-162` | KEY_QUERY classification memory/disk/none with size check. |
| FR-016 | Aligned | — | `src/server.rs:198-316` | Batched `promote_to_memory_tier`, re-lookup, pin, `push_async`, mapped RDMA_STATUS. |
| FR-016a | Aligned | — | `src/server.rs:47-121` | `PinnedBatch`/`ServeReport` own pins; released when callback runs or is dropped. |
| FR-016b | Aligned | — | `src/server.rs:111-121,289-315` | `ServeReport::drop` reports UnableToConnect so every request gets exactly one status. |
| FR-017 | Aligned | — | `src/server.rs:249-287` | Post-promotion non-resident key → `KeyNoLongerAvailable`. |
| FR-018 | **Drifted** | **Low** | `src/wire.rs:235-310`; `src/actor.rs:330` | Framing/echo correct and Unknown is ignored, but spec says unknown types MUST be **logged** and ignored; no log is emitted (`WireMessage::Unknown => {}`, no `logger` call). |
| FR-019 | Aligned | — | `src/actor.rs:412-414,498-509` | Stale op_id discarded without error. |
| FR-020 | Aligned | — | `src/actor.rs:161,672-676` | Multiple concurrent ops keyed by op_id; interleaved. |
| FR-021 | Aligned | — | `src/actor.rs:337-339` | Self-SHOUT (peer == own uuid) filtered in `handle_key_query` (only SHOUT type). |
| FR-022 | Aligned | — | `iremote_lookup.rs:28-98`; `src/lib.rs:105` | `LookupConfig` public, `Default`; all FR-022 knobs present with sensible defaults. |
| FR-023 | Aligned | — | `src/lib.rs:42-51` | All 8 receptacles declared (IZyre, IDispatchMap, IMemoryTier, IDispatcher, initiator, responder, responder_admin, ILogger). |
| FR-024 | Aligned | — | `src/server.rs`, `src/worker.rs` | No ibverbs/MR in-crate; outbound via initiator, inbound via responder. |
| FR-025 | Aligned | — | `src/lib.rs:140-164` | responder_admin init, reads endpoint + rkey, opens control channel. |
| FR-026 | Aligned | — | `src/actor.rs:102-108,428-434` | Per-key `in_flight` index coalesces same-key fetches; followers wait. |
| FR-027 | Aligned | — | `src/lib.rs:175-178`; `src/actor.rs:393-399`; `src/worker.rs:77-80` | Advertises responder endpoint header; warms via `initiator.connect` on Enter; best-effort. |
| FR-028 | Aligned | — | `src/actor.rs:356-371`; `src/worker.rs` | Warm + serve run on off-loop worker; poll loop never blocks on RDMA. |
| FR-029 | Aligned | — | `src/lib.rs:335-363` | `peers_seen`, `signal_shutdown`, `shutdown` present; documented as out-of-interface. |
| SC-001 | Aligned | — | `tests/mesh.rs:229` (`memory_hit_is_fetched_from_peer`) | Structurally validated (latency claim not measured). |
| SC-002 | Aligned | — | `tests/mesh.rs:578`; `src/actor.rs:305-307,815-825` | Total miss → NotFound within deadline, no slot left. |
| SC-003 | Aligned | — | `src/actor.rs:477-485` | RDMA_REQUEST whispered on the same event as the satisfying KEY_RESPONSE. |
| SC-004 | Aligned | — | `tests/mesh.rs:338` (`failed_fetch_retries_alternate_peer`) | Retry-to-alternate mechanism present/tested (≥95% is statistical, not asserted). |
| SC-005 | Aligned | — | `src/actor.rs:889-942,1015-1034`; `src/server.rs:47-121` | Orphan/teardown gates slot reclaim; pins guarded; write_ref held only briefly. |
| SC-006 | Aligned | — | `src/actor.rs:1009` | Peer departure re-evaluates completion immediately. |
| SC-007 | Aligned | — | `src/server.rs:138-158,217-235` | Size-mismatch → not available; never a partial write. |
| SC-008 | Aligned | — | `tests/mesh.rs:253` (`concurrent_same_key_lookups_issue_one_rdma`) | Single-flight verified. |

---

## Unspecced Code

| Item | Location | Note |
|------|----------|------|
| `LookupConfig.caller_wait` + background-continue `batch_lookup` | `src/lib.rs:304-309`; `iremote_lookup.rs:44-50` | Decouples caller blocking from `op_deadline`; op keeps running after caller returns. Beyond FR-020 ("blocks until its operation finalizes"). Tested (`tests/mesh.rs:600`). Low. |
| `LookupConfig.connection_teardown_timeout` + `tick_orphans` force-reclaim backstop | `src/actor.rs:113-121,849-885`; `iremote_lookup.rs:51-57` | Timer-driven force-teardown for a peer that neither reports a late status nor exits. FR-014 only covers the departure-triggered path. Tested (`tests/mesh.rs:650`). Low. |
| Orphan-reuse guard (skip re-reserving a key with an orphaned slot) | `src/actor.rs:441-443,630-632,772-774` | Memory-safety detail preventing buffer aliasing; not called out in any FR. Low. |
| Extra `LookupConfig` fields not enumerated by FR-022 (`actor_cpu`, `discovery`, `node_endpoint`) | `iremote_lookup.rs:64-78`; `src/lib.rs:145-147,169-171` | FR-022 lists only quorum/phase1/deadline/retry-cap/max-keys. These support NUMA pinning + gossip discovery. Low. |
| `integrity-check` cargo feature | `Cargo.toml:8-14` | Forwards `interfaces/integrity-check` for checksum accessors; build plumbing, no spec mention. Info/Low. |

## Conflicts (spec references / status mismatches)

| Note | Location |
|------|----------|
| lib.rs docstrings claim the KEY_QUERY→RDMA protocol is unbuilt and `batch_lookup` returns all-NotFound, contradicting the fully-implemented `actor.rs` and spec 002's `Status: Synced`. Doc-vs-code drift. | `src/lib.rs:5-8,251-253` |

No spec references to nonexistent files were found: `.specify/sync/drift-report.md`,
`contracts/wire-protocol.md`, `contracts/iremote_lookup.md`, `knowledge/size-mismatch-handling.md`,
and `research.md` all exist.

## Recommendations

1. **Fix the stale lib.rs docstrings (002 FR-001, Medium).** Delete/rewrite the "until then
   `batch_lookup` finalizes every key as NotFound" prose in the module header (`src/lib.rs:5-8`) and
   the `batch_lookup` doc (`src/lib.rs:251-253`). They contradict the shipped implementation and the
   spec's Synced status — the top source of confusion for a maintainer.
2. **Log unknown wire frames (002 FR-018, Low).** `on_wire`'s `WireMessage::Unknown => {}` should
   emit a `logger.debug/warn` to satisfy "MUST be logged and ignored." Currently silently dropped.
3. **Backfill 002 for the unspecced behaviors (Low).** Add FRs (or extend FR-014/FR-020/FR-022) for
   `caller_wait`, the `connection_teardown_timeout` orphan force-reclaim backstop, the orphan-reuse
   guard, and the extra config fields — they are load-bearing and mesh-tested but currently
   undocumented in the spec.
4. **Spec 001 needs no code action** — its divergences are the intentional, documented supersession.
   Leave as-is.
