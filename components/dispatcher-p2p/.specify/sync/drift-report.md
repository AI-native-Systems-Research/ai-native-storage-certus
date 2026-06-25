# Spec Drift Report
Generated: 2026-06-18
Project: dispatcher-p2p

## Summary
| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 13 |
| Aligned | 10 (77%) |
| Drifted | 2 (15%) |
| Not Implemented | 0 (0%) |
| Unspecced Code | 6 |

## Detailed Findings
### Spec: 001-gpudirect-cold-path - GPUDirect Storage Cold Path

#### Aligned
- FR-001: System MUST read evicted data from SSD directly into GPU staging buffers, bypassing host DRAM. → `src/pipeline.rs:703-861` (`pipelined_ssd_to_gpu_p2p` reads NVMe directly into P2P ring GPU BAR1 slots)
- FR-002: System MUST copy data from staging buffers to the client's GPU destination. → `src/pipeline.rs:795-807` (D2D `cudaMemcpyAsync` from ring slot dev_ptr to `gpu_dst`)
- FR-003: System MUST pre-allocate a fixed ring of 64 GPU staging buffers at initialization via cudaMalloc + GDRCopy BAR1 mapping + spdk_mem_register. Each slot is 128 KiB (MDTS). → `src/p2p_ring.rs:19,52-126`
- FR-004: System MUST partition the staging ring for concurrent thread access using ThreadPartition. → `src/p2p_ring.rs:160-181`
- FR-005: System MUST pipeline SSD reads with D2D GPU copies using FIFO completion ordering, 4 CUDA streams round-robin, sync at ring wrap. → `src/pipeline.rs:703-861`
- FR-006: System MUST panic on first cold lookup if the P2P ring was not initialized. → `src/lib.rs:1532` (`p2p_ref.expect(...)`)
- FR-008: System MUST implement the same interface as the standard dispatcher. → `define_component!` provides `[IDispatcher]`
- FR-010: System MUST release all staging resources on shutdown with no leaks. → `src/lib.rs:1223,1227` (`ring.destroy(&*gpu)`)
- FR-011: System MUST handle read failures gracefully without corrupting ring state. → `src/pipeline.rs:766-786`
- FR-013: System MUST implement `promote_to_memory_tier(keys)` using `pipelined_ssd_to_dram_only`. → `src/lib.rs:2050-2099`

#### Drifted
- **FR-009** (moderate): Spec says "System MUST promote successfully read cold entries back to DRAM after completing the read." Implementation now uses **lazy async backfill** — cold entries stay as BlockDevice after P2P read, and a background `DramBackfillWorker` re-reads from SSD into DRAM with a configurable delay (`backfill_delay_ms`, default 10ms, 0 = disabled). The key is NOT immediately promoted; there is a window where repeat lookups go through P2P again.
  - Location: `src/lib.rs:420-428` (promote_and_serve P2P branch), `src/lib.rs:1557-1561` (batch_lookup P2P branch), `src/background.rs:218-296` (DramBackfillWorker)
  - Rationale: Immediate DRAM backfill caused 30% cold throughput regression due to NVMe bandwidth contention between foreground P2P reads and background DRAM fills on the same drives. Lazy backfill with throttle preserves cold throughput while still enabling hot lookups after the delay window.

- **FR-007** (minor): Spec says "There is no runtime path selection." Implementation has a runtime check `if let Some(ref p2p) = *p2p_guard` with a DRAM fallback path (`pipelined_ssd_to_gpu_zero_copy`) in `promote_and_serve`. However, FR-006 ensures the P2P ring is always present in production (panic if not). The DRAM fallback exists only for the single-key `lookup()` path in test/staging environments.
  - Location: `src/lib.rs:402-456`

- **FR-012** (minor, unchanged): Performance measurement handled externally by `certus-api-bench_v2.py`. No in-component hooks. Spec satisfied at system level.

#### Not Implemented
(none)

## Unspecced Code

| Feature | Location | Lines | Suggested Spec |
|---------|----------|-------|----------------|
| DramBackfillWorker (lazy DRAM promotion after P2P) | `src/background.rs:218-296`, `src/lib.rs:1058-1133` | ~120 | Update FR-009 or add FR-014 |
| `backfill_delay_ms` config option | `interfaces/src/idispatcher.rs:63` | 4 | Add FR-014 |
| Background write-through to SSD | `src/background.rs` (ParallelBackgroundWriter) | ~100 | Unspecced (inherited from standard dispatcher) |
| Background SSD evictor | `src/background.rs:300+` (BackgroundEvictor) | ~170 | Unspecced |
| prepare_store / commit_store / cancel_store | `src/lib.rs:1784-1943` | ~160 | Unspecced (direct-write path) |
| Pipeline zero-copy and multi-object variants | `src/pipeline.rs:244-675` | ~430 | Unspecced (DRAM fallback pipelines) |

## Inter-Spec Conflicts

- FR-007 ("no runtime path selection") conflicts with the DRAM fallback in `promote_and_serve`. Mitigated by FR-006 which panics if P2P ring is absent in production. Fallback only reachable in test environments.
- FR-009 ("MUST promote after completing the read") now conflicts with actual behavior (lazy async promotion). Spec update recommended.

## Recommendations

1. **Update FR-009** to: "System MUST asynchronously promote cold entries to DRAM via a throttled background worker after serving the client via P2P. The backfill delay is configurable (`backfill_delay_ms`; default 10ms; 0 = disabled). During the backfill window, repeat lookups of the same key use the P2P path (correct data, no DRAM involvement)."
2. **Add FR-014**: "System MUST support configurable DRAM backfill throttling via `backfill_delay_ms` in `DispatcherConfig`. When set to 0, no background DRAM backfill occurs and cold-promoted keys remain as BlockDevice indefinitely (repeat lookups always use P2P)."
3. **Clarify FR-007**: "In production (full-p2p profile), the P2P ring is always initialized. A DRAM fallback path exists in `promote_and_serve` for unit tests and staging environments where GDRCopy is unavailable."

---

## Previous History

### Resolved 2026-06-18

- FR-009 DRAM backfill redesigned: immediate (broken — served garbage) → lazy async with throttle (correct, configurable)
- Added DramBackfillWorker, backfill_delay_ms config

### Resolved 2026-06-16

- DRIFT-A: P2P ring failure behavior -- spec and code both specify panic on first cold lookup, not at startup.
- DRIFT-B: `promote_to_memory_tier` unspecced -- Added FR-013 to spec.
- DRIFT-C: Thread topology and CUDA streams -- Updated FR-004 and FR-005.

### Resolved 2026-06-12

- DRAM fallback removed: fail-fast at startup (US2, FR-006, FR-007, SC-006)
- P2P ring uses real BAR1 (FR-003)
- Pipeline sync strategy aligned (FR-005)
- Performance measurement references standard dispatcher (US4, SC-005)
