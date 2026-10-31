# Drift Resolution Proposals — dispatcher-p2p

Generated: 2026-10-09T23:49:23Z  
Branch: `fix/dispatcher-p2p-clean-eviction`  
Based on: drift-report 2026-10-09

| # | Requirement | Direction | Decision | Location |
|---|---|---|---|---|
| 1 | FR-017 | BACKFILL | Approved | `src/lib.rs:606-655` |
| 2 | FR-025a | BACKFILL | Approved | `src/lib.rs:2919-2950` |
| 3 | FR-029 | BACKFILL | Approved | `src/lib.rs:83-117, 606-655, 2919-2950` |
| 4 | FR-030 (new) clean eviction | BACKFILL | Not approved | `src/lib.rs:606-743` |
| 5 | FR-031 (new) store backpressure | BACKFILL | Not approved | `src/lib.rs:721-743, 2442-2530` |
| 6 | FR-027/FR-028 anchors | BACKFILL | Not approved | `spec.md` |
| 7 | eviction_scans_exhausted (dispatcher 002 FR-034) | ALIGN | Approved | `src/lib.rs:606-655` |
| 8 | dispatcher 002 FR-035 | BACKFILL (other component) | Approved | `components/dispatcher/specs/002-served-by-tier-attribution/spec.md` |

- **1. FR-017** — Foreground eviction only demotes; Removed comes from the SSD evictor only.
- **2. FR-025a** — Drop 'no tier-movement counters'; point to FR-029.
- **3. FR-029** — Rewrite: foreground FR-033/034 counters reported; gap narrowed to MemoryTierEvictor + listed fields.
- **4. FR-030 (new) clean eviction** — Specify the #220 invariant.
- **5. FR-031 (new) store backpressure** — Specify reserve_memory bounded retry and config-driven limits.
- **6. FR-027/FR-028 anchors** — Refresh line anchors.
- **7. eviction_scans_exhausted (dispatcher 002 FR-034)** — Count per scan that frees nothing.
- **8. dispatcher 002 FR-035** — Narrow FR-035 to the background evictor gap.
