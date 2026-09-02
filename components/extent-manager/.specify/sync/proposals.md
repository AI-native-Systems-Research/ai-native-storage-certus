# Drift Resolution Proposals — extent-manager

Generated: 2026-09-01T22:59:04Z
Based on: drift-report 2026-09-01T22:59:04Z (commit 33bddaba)

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 2 |
| Align (Spec → Code) | 0 |
| Human Decision | 1 (FR-030 direction) |
| New Specs | 0 |
| Remove from Spec | 0 |
| Plan-doc fixes | 2 |

All findings favour correcting spec/doc prose toward the shipped, tested code. No
`src/` change is proposed. The single judgment call is FR-030: correct the prose
(doc-only) vs. add a format-path flush (behavior change) — see Proposal 1.

---

## Proposal 1: 001-extent-manager-v2 / FR-030

**Direction**: BACKFILL (Code → Spec) — *human decision on alternative*

**Current State**:
- Spec says: the component issues a flush "to the metadata device after
  checkpoint writes **(and in the format path)**".
- Code does: `flush()` is implemented (`block_io.rs:167-180`) and called **only**
  in the checkpoint path (`lib.rs:308-310`, feature-gated). `format()` writes the
  superblock (`lib.rs:496-498`) with no flush. `README.md:66` and
  `Cargo.toml:24-27` also describe checkpoint-only flush.

**Proposed Resolution (recommended — doc-only)**:
Drop the "(and in the format path)" parenthetical from FR-030 prose:

> When **enabled**, the component issues a `BlockDeviceClient::flush()` to the
> metadata device after checkpoint writes, forcing data onto non-volatile media
> at a throughput cost …

**Alternative (behavior change — NOT auto-applied)**:
If format-time durability is actually required, add
`#[cfg(feature = "volatile_write_cache")] metadata_client.flush()?;` after
`lib.rs:498` and keep the spec prose. This changes runtime behavior and should be
a deliberate code decision, not a silent sync.

**Rationale**: The code is tested and internally consistent (README + Cargo.toml
agree on checkpoint-only). Only the leading FR-030 prose is out of step; its own
implementation-status note already scopes the call site to the checkpoint path.

**Confidence**: HIGH (recommended direction)

---

## Proposal 2: 001-extent-manager-v2 / FR-016

**Direction**: BACKFILL (Code → Spec)

**Current State**:
- Spec says: interface doc + README "incorrectly state 'five minutes' … stale doc
  strings that should be corrected to reference the true 30-second default."
- Code does: `iextent_manager.rs:205` and `README.md:13` already say 30 seconds.

**Proposed Resolution**: Rewrite the FR-016 note to drop the obsolete remediation
language and simply state the default:

> **FR-016**: A background thread MUST call `checkpoint()` at a configurable
> interval (default 30 seconds, set in `ExtentManager::new_inner`). The
> `IExtentManager` interface doc comment and this component's `README.md` document
> the 30-second default consistently.

**Rationale**: The remediation completed (verified in code); the present-tense
"should be corrected" note now describes a non-existent defect.

**Confidence**: HIGH

---

## Proposal 3: plan.md factual fixes (planning doc)

**Direction**: BACKFILL (Code → Spec doc)

- `plan.md:59` — `checkpoint_interval_ms: AtomicU64 (default 5000)` →
  `checkpoint_timer_state: Arc<CheckpointTimerState>` (Mutex<Option<Duration>> +
  Condvar + shutdown), **default 30 s** (`lib.rs:62-79,95,112`).
- `plan.md:229` — `superblock.rs … (v5)` → `(v6)` (`superblock.rs:6`
  `FORMAT_VERSION = 6`).

**Rationale**: plan.md was refreshed 2026-08-20 for receptacles/layout but these
two diagram details remained stale.

**Confidence**: HIGH
