# Drift Resolution Proposals: dispatcher-p2p

Generated: 2026-11-04T01:26:34Z — branch `feat/dispatcher-p2p-mr-pool` @ `6e2d2816`

| Resolution Type | Count |
|---|---|
| Backfill (Code → Spec) | 5 |
| Align / Human / New spec / Remove | 0 |

All five approved by the user in one decision (2026-11-03). The code is authoritative for each: every change is a fix or a capability brought level with `dispatcher`, tested in this crate and on hardware.

1. **FR-005 — BACKFILL.** FIFO slot recycling replaced by tag-tracked slots (`SlotTracker`, `src/pipeline.rs:838`). FIFO was the bug: out-of-order NVMe completions copied segments from another read's slot. HIGH.
2. **FR-034 (new) — BACKFILL.** N-region blocks (vLLM 0.23+ LBHNC): gather, scatter, cold split at region boundaries. HIGH.
3. **FR-035 (new) — BACKFILL.** Null-stream store copies synced before return, as `dispatcher` FR-056. HIGH.
4. **FR-036 (new) — BACKFILL.** Per-reader `ChannelPool` (#583), as `dispatcher`'s. HIGH.
5. **FR-015 — BACKFILL (wording).** Cold pipeline is `pipelined_multi_object_p2p`; device routing still not wired. HIGH.
