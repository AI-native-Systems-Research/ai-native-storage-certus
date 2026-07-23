# Spec-Sync — Consolidated Deferred Code Defects

Generated: 2026-07-22. Auto-backfill pass across 23 components complete (spec `.md` files updated
where code was authoritative). The items below are **code defects / judgment calls** — the spec was
kept as-is and the resolution deferred. No production source was edited. Each lives in the component's
`.specify/sync/align-tasks.md`.

Ordered build-breaking → correctness/durability → moderate → minor/doc.

---

## 🔴 P0 — Build-breaking (workspace does not fully compile)

1. **extended-metadata-store not in workspace members**
   Root `Cargo.toml` `[workspace].members` omits `components/extended-metadata-store`, so the crate is
   never built, tested, or linted in CI.
   *Fix:* add it to `members`. → `Cargo.toml`
   (Reported by both extended-metadata-store and interfaces.)

2. **interfaces: `iextended_metadata_store` module never declared**
   `components/interfaces/src/lib.rs` does not `mod iextended_metadata_store;`, so the implemented trait
   isn't compiled into the crate — its consumer (extended-metadata-store) can't build either.
   *Fix:* add the `mod` declaration. → `components/interfaces/src/lib.rs`

## 🔴 P1 — Correctness / durability

3. **extended-metadata-store `force_flush()` is an unconditional no-op**
   Never invokes flush machinery in any build config, contradicting FR-05 (durability guarantee).
   → `components/extended-metadata-store/src/…`

4. **extent-manager `volatile_write_cache` cfg-polarity inverted**
   Default build never flushes checkpoint writes; enabling the feature *adds* the flush — opposite of the
   spec's intended design. Potential data-durability bug. → `components/extent-manager/src/…`

5. **block-device-filesys telemetry latency always 0ns**
   Per-op latency hard-coded to 0 instead of measured; all latency telemetry is meaningless.
   → `components/block-device-filesys/src/…`

6. **block-device-kernel telemetry latency hard-coded 0** (same class as #5).
   → `components/block-device-kernel/src/…`

7. **block-device-spdk-nvme telemetry test suite fails to compile**
   `record()` arity mismatch — telemetry tests are dead. → `components/block-device-spdk-nvme/…/tests`

8. **block-device-spdk-nvme NUMA node hard-coded to 0**
   `probe_controller()` pins the actor to node 0 regardless of hardware; mispins on multi-socket hosts.
   → `components/block-device-spdk-nvme/src/…`

9. **block-device-filesys io_uring: no submission-queue back-pressure**
   On SQ-full the op fails immediately instead of buffering/retrying. → `components/block-device-filesys/src/…`

10. **rdma-test FR-012 connection-retry / partial-results not implemented**
    `poll_completion_with_retry` retries only CQ polling (not connection setup); `partial` hard-coded
    `false`; `main()` drops results on error. → `tools/rdma-test/src/…`

## 🟠 P2 — Moderate (declared-but-dead / missing validation)

11. **dispatch-map FR-014 — no error logging on any error path** (spec requires `logger.error(...)`).
12. **dispatch-map `create_memory_tier_entry` accepts null pointers** with no rejection (US1-AS3).
13. **remote-lookup-rdma-responder — dead error diagnostics** (3 linked items):
    `record_accept_loop_error()` (FR-016) never called; `ResponderEvent::Error` (contract §3) never
    constructed; FR-014 accept-loop diagnostics not routed to `ILogger`. QP-creation failures are silent.
14. **spdk-env device enumeration is NVMe-only** vs SC-001 "all VFIO device types" — capability gap.
15. **spdk-env partial-init cleanup** — `do_init()` error path never calls `spdk_env_fini()` (latent today).
16. **block-device-spdk-nvme `nvme_version`/`max_transfer_size` hard-coded** in `attach()` instead of read
    from hardware (FR-010).

## 🟡 P3 — memory-tier architectural drift (needs a direction decision)

17. **16-way shard design spec'd but absent** — code is a single `RwLock<Pool>` (FR-005/006/007, NFR-002,
    FR-013/021, SC-3). Decide: implement sharding, or backfill the spec *down* to the simpler reality.
18. **`evict_lru_for_key()` ignores its `key` arg** — pure alias for `evict_lru()` (FR-014); tied to #17.
19. **Creusot proofs absent** — SC-8 claims 10 verified properties; no proof artifacts exist.
20. **Version mismatch** — spec 0.2.0 / Cargo.toml 0.1.0 / macro 0.3.0.

## 🟢 P4 — Minor / doc-only (outside spec-edit scope)

- **component-framework** `Actor::activate()` panics via `.expect(...)` on re-activation instead of a typed
  `ActorError` (contradicts FR-004 / 005 FR-001).
- **gpu-services** inter-spec conflict: 001 FR-008 ("all via IGpuServices") vs 002 FR-021–023 standalone DMA
  ctors — recommend relaxing FR-008 with a p2p carve-out (needs human sign-off). Plus `unpin_memory` never
  fully unregisters; `register_host_memory` treats SPDK EBUSY(-16) as success (both ambiguous intent).
- **certus-server** stale `README.md` (describes removed extent-manager/metadata-device architecture);
  unused `ERROR_CODE_DUPLICATE_KEY` proto enum.
- **Stale non-spec docs** flagged but not edited (outside `specs/**`): spdk-sys README (28 vs 30 libs),
  extent-manager README (checkpoint interval 5min vs 30s; format version 5 vs 6), block-device-spdk-nvme
  README (64 vs 256 channel slots), dispatch-map README/CLAUDE.md, block-device-filesys missing doctests.
- **Test-coverage gaps**: spdk-sys (no NVMe-type binding tests), remote-lookup (missing US7 peer-Exit mesh
  test), example-helloworld-dylib (no automated dylib-load test), block-device-filesys SC-002/003/005
  perf criteria unenforced.
