# Spec Sync Apply Report — dispatcher-p2p

Applied: 2026-10-09T23:49:23Z  
Branch: `fix/dispatcher-p2p-clean-eviction`  
Spec: 001-gpudirect-cold-path  
Backups: `.specify/sync/backups/20261009T234923Z/spec.md`, `components/dispatcher/.specify/sync/backups/20261009T234923Z/spec-002.md`

## Applied

| # | Requirement | Direction | Result |
|---|---|---|---|
| 1 | FR-017 | BACKFILL | ✅ Foreground eviction only demotes; Removed comes from the SSD evictor only. |
| 2 | FR-025a | BACKFILL | ✅ Drop 'no tier-movement counters'; point to FR-029. |
| 3 | FR-029 | BACKFILL | ✅ Rewrite: foreground FR-033/034 counters reported; gap narrowed to MemoryTierEvictor + listed fields. |
| 7 | eviction_scans_exhausted (dispatcher 002 FR-034) | ALIGN | ✅ Count per scan that frees nothing. |
| 8 | dispatcher 002 FR-035 | BACKFILL (other component) | ✅ Narrow FR-035 to the background evictor gap. |

## Not applied

| # | Requirement | Reason |
|---|---|---|
| 4 | FR-030 (new) clean eviction | Not approved |
| 5 | FR-031 (new) store backpressure | Not approved |
| 6 | FR-027/FR-028 anchors | Not approved |

## Verification

- Every `file:line` anchor in the edited spec text was re-read against the code after `cargo fmt`.
- `cargo test -p dispatcher-p2p --lib`: 77 passed. `cargo clippy -p dispatcher-p2p --no-deps --all-targets`: no new warnings vs `3c001478`.
- ALIGN #7 was applied in code at the user's direction (the skill's default is to emit a task instead).
- Drift status `drift`: #4-#6 remain open.
