# Spec Sync Proposals
Generated: 2026-09-09
Project: dispatcher
Spec: 001-dispatcher-cache-interface
Source: `.specify/sync/drift-report.md`
Branch: `fix-dispatcher-store-backpressure`

Summary: 1 BACKFILL proposal (one two-layer store-backpressure feature spanning
FR-003/FR-033/FR-056/FR-058/FR-059 + new FR-060 + one edge case + one User
Story 1 acceptance scenario), APPROVED interactively. No ALIGN, no
HUMAN_DECISION. Editing scope: `specs/**` only (source untouched — code is
authoritative and already shipped in `39ffee9b` / `c400ced8`).

---

## Proposal 1 — Two-layer store backpressure + drop-on-full [APPROVED]

- **Direction**: BACKFILL (code authoritative — shipped fix; aligning code→spec
  would reintroduce the fatal vLLM `assert transfer_result.success` →
  `EngineDeadError` on a full memory tier)
- **Requirements touched**: new **FR-060**; FR-003, FR-033, FR-056, FR-058,
  FR-059; one edge-case bullet; User Story 1 acceptance scenario 5
- **Commits**: `39ffee9b` (bounded store backpressure), `c400ced8` (best-effort
  drop on full after budget)
- **Rationale**: On a full memory tier the store path previously surfaced
  `AllocationFailed`, which the vLLM connector turns into a fatal
  `assert transfer_result.success`. The fix introduces two layers:
  1. **Bounded store backpressure** in `reserve_memory` (`src/lib.rs:3115`) —
     retry `evict_and_insert` on a 20 ms poll until the `store_backpressure_ms`
     budget (default 5000 ms) is spent, giving the async evictor a window to
     free space, then fail fast. `store_backpressure_ms = 0` restores the
     original single-shot fail-fast.
  2. **Best-effort drop-on-full** in `populate` (`src/lib.rs:2950`) and
     `batch_populate` (`src/lib.rs:3017`) — when `reserve_memory` ultimately
     fails, both callers **unconditionally** log, bump `store_drops_on_full`,
     and return `Ok(())` **without caching**, rather than propagating
     `AllocationFailed`.
  Config/telemetry surface: `DispatcherConfig.store_backpressure_ms` and the two
  new `TierEventStats` counters (`store_backpressure_events`,
  `store_drops_on_full`) in `components/interfaces/src/idispatcher.rs`.
- **Accuracy note (verified against `src/lib.rs:3150-3202`)**: the drop-on-full
  is *unconditional* and does not depend on `store_backpressure_ms`. Setting
  `store_backpressure_ms = 0` disables only the retry in `reserve_memory`; the
  caller still drops. Proposed spec text was worded to reflect this and does NOT
  claim `store_backpressure_ms = 0` makes `populate` return `AllocationFailed`.
- **Proposed spec text**:
  - **New FR-060** — two-layer requirement: (1) bounded store backpressure in
    `reserve_memory`; (2) best-effort drop-on-full in `populate`/`batch_populate`
    (unconditional). Complements FR-053 read-path staging.
  - **FR-003** (populate) — append backpressure/drop note referencing FR-060.
  - **FR-033** (config) — add `store_backpressure_ms` (u64, default 5000) to the
    field list; disable semantics (0 = `reserve_memory` fails fast, drop-on-full
    FR-060 still applies).
  - **FR-056** (`reserve_memory`) — bounded store backpressure, deadlock-free
    rationale, `store_backpressure_ms = 0` restores fail-fast.
  - **FR-058** — "four `u64` fields" → "six `u64` fields"; add
    `store_backpressure_events` and `store_drops_on_full`.
  - **FR-059** (`batch_populate`) — append drop-on-full note (`c400ced8`).
  - **Edge case** — populate applies bounded backpressure then best-effort drops
    (returns `Ok(())`), does NOT return `AllocationFailed`.
  - **User Story 1** — new acceptance scenario 5 (drop-on-full returns success,
    no dispatch-map entry, increments `store_drops_on_full`).
- **Confidence**: HIGH (behavior read directly from `src/lib.rs` and
  `idispatcher.rs`; failure mode corroborated by memory
  `certus-async-full-tier-crash`).
- **Action**: [x] Approve
