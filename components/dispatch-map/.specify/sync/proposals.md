# Drift Resolution Proposals

Generated: 2026-08-06T23:27:50Z
Based on: drift-report from 2026-08-06T23:27:50Z

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code → Spec) | 3 |
| Align (Spec → Code) | 2 |
| Human Decision | 3 |
| New Specs | 0 |
| Remove from Spec | 0 (1 acceptance scenario marked obsolete) |

---

## Proposal 1: 001-dispatch-map / FR-014 (error logging)

**Direction**: HUMAN_DECISION (recommend BACKFILL)

**Current State**:
- Spec says: "System MUST use the `ILogger` receptacle for info, debug, and **error** logging throughout the component."
- Code does: only `logger.info(...)` / `logger.debug(...)`; `logger.error(...)` is never called on any error-return path.

**Options**:
- **BACKFILL (recommended)**: narrow FR-014 to "info and debug logging"; note that error conditions are surfaced to the caller as typed `DispatchMapError` values rather than logged. Lowest-risk; matches the existing error-return contract.
- **ALIGN**: add `logger.error(...)` at the ~13 error-return sites (timeout, invalid-state, ref-count, key-not-found). More invasive; introduces log noise on expected control-flow errors (e.g. `Timeout`, `AlreadyExists`).

**Rationale**: The component already communicates failures via `Result`; logging every expected error can be noisy. But error logging is a legitimate observability choice — a call the maintainer should make.

**Confidence**: MEDIUM

---

## Proposal 2: 001-dispatch-map / FR-012 (unbound eviction policy panics)

**Direction**: ALIGN (Spec → Code)

**Current State**:
- Spec/contract says: `initialize()` "returns an error if unbound" (`IEvictionPolicy` mandatory).
- Code does: `initialize()` → `get_pool_id()` → `eviction_policy.get().unwrap()` **panics** when unbound; the `.map_err(NotInitialized)` guard below it is unreachable.

**Proposed Resolution**: Make `get_pool_id` fallible (return `Result<PoolId, DispatchMapError>`), or reorder `initialize` to check `self.eviction_policy.get()` (mapping to `NotInitialized`) *before* creating the pool, so an unbound eviction policy yields `Err(NotInitialized)` instead of a panic. Add a unit test for the unbound case.

**Rationale**: The spec/contract is the agreed behavior; the panic is a defect that violates the "no panics" error-semantics rule in the contract.

**Confidence**: HIGH

---

## Proposal 3: 001-dispatch-map / SC-004 (struct size "varies")

**Direction**: BACKFILL (Code → Spec)

**Current State**:
- Spec says: "The `DispatchEntry` struct size varies by `Location` variant."
- Code does: `Location` is a Rust `enum`; `DispatchEntry` is sized for its largest variant at compile time — a fixed constant.

**Proposed Resolution**: Reword SC-004 to: "Per-entry metadata is kept compact. `DispatchEntry` is sized (at compile time) to its largest `Location` variant (`MemoryTier`: pointer + size + optional offset); `BlockDevice` entries occupy the same footprint. `size_of::<DispatchEntry>()` is exposed via the free `entry_size()` function for benchmarks/assertions."

**Rationale**: Code is correct; the spec wording misdescribes Rust enum layout.

**Confidence**: HIGH

---

## Proposal 4: 001-dispatch-map / US1-AS3 (null-pointer rejection)

**Direction**: HUMAN_DECISION (recommend ALIGN)

**Current State**:
- Spec says (US1 AS3 + Edge Cases): "`create_memory_tier_entry` with a null pointer returns an error; no entry is recorded."
- Code does: no null check; a null `*mut u8` is accepted and stored. No null-pointer error variant exists.

**Options**:
- **ALIGN (recommended)**: add a `pointer.is_null()` guard returning a new `DispatchMapError::NullPointer(key)` (or reuse `InvalidSize`-style variant) before insertion; add a test.
- **BACKFILL**: drop AS3 / the edge-case bullet, documenting non-null as an unsafe caller contract (the `*mut u8` is inherently unsafe and caller-provided).

**Rationale**: A null-pointer guard is cheap defensive safety, but the pointer is already an unsafe caller-supplied value, so treating non-null as a caller contract is defensible.

**Confidence**: MEDIUM

---

## Proposal 5: 001-dispatch-map / US2-AS4 (size-mismatch → MismatchSize)

**Direction**: BACKFILL (mark obsolete)

**Current State**:
- Spec says (US2 AS4): "size mismatch → `ErrorMismatchSize`."
- Code does: `lookup(key)` takes no expected-size parameter; `MismatchSize` is unreachable. FR-004 already notes `MismatchSize` "exists in the return enum for future use but is not currently triggered."

**Proposed Resolution**: Mark US2 AS4 as deferred/obsolete for v0, consistent with FR-004's existing note. Either remove AS4 or annotate it "(deferred — `lookup` currently takes no expected-size argument; `MismatchSize` reserved for a future size-checked lookup variant)."

**Rationale**: Aligns the acceptance scenario with the already-acknowledged FR-004 wording; avoids an apparent gap that is actually a deliberate deferral.

**Confidence**: HIGH

---

## Proposal 6: 001-dispatch-map / integrity-check feature (unspecced)

**Direction**: BACKFILL (Code → Spec)

**Feature**: Optional per-entry CRC-32 integrity checksum.
**Location**: `Cargo.toml:8-11` (feature), `src/entry.rs:41-42` (field), `src/lib.rs:594-615` (`set_checksum`/`get_checksum`), `components/interfaces/src/idispatch_map.rs` (trait methods, feature-gated).

**Draft additions to 001-dispatch-map**:
- **User Story 12 — Optional Data-Integrity Checksums (Priority: P3)**: When built with the `integrity-check` feature, a caller records a CRC-32 for a stored block via `set_checksum(key, checksum)` on the store path; the checksum travels with the index entry (surviving demote/promote) and is fetched via `get_checksum(key)` on load. `None` means "not recorded — skip verification".
- **FR-027**: When compiled with the `integrity-check` feature, the system MUST provide `set_checksum(key, checksum)` (returns `KeyNotFound` if absent) and `get_checksum(key) -> Option<u32>` (returns `None` if absent or checksum is 0/unset). The `checksum` field adds 4 bytes to `DispatchEntry` only when the feature is enabled.
- **FR-028**: The `integrity-check` feature MUST be off by default; when off, `DispatchEntry` and the `IDispatchMap` trait surface are unchanged (no `checksum` field, no checksum methods).

**Rationale**: Real, feature-gated, trait-level public API with zero spec coverage. Backfilling documents intentional evolution.

**Confidence**: MEDIUM

---

## Proposal 7: 001-dispatch-map / reuse_count (dead metric)

**Direction**: HUMAN_DECISION (recommend ALIGN = remove)

**Current State**:
- `reuse_count: AtomicU32` is incremented on `lookup`/`take_read`/`downgrade_reference` but never read or exposed through any `IDispatchMap` method (only printed in the `Debug` impl).

**Options**:
- **ALIGN (recommended)**: remove the `reuse_count` field and its `fetch_add` sites — dead instrumentation adds an atomic write to every hot-path access.
- **BACKFILL**: add an FR + interface method (e.g. `reuse_count(key) -> u32`) exposing per-entry hit telemetry, if that metric is planned.

**Rationale**: Currently pure overhead on the hot path with no consumer. Remove unless telemetry is on the roadmap.

**Confidence**: MEDIUM

---

## Proposal 8: interfaces / Creusot verification claims (P1–P10)

**Direction**: HUMAN_DECISION (recommend ALIGN = correct/remove stale comment)

**Current State**:
- `components/interfaces/src/idispatch_map.rs:84-99` documents P1–P10 "formally proved with Creusot ... see `components/dispatch-map/verif/`", plus per-method `# Verified:` doc annotations. The `verif/` directory **does not exist**.

**Options**:
- **ALIGN (recommended)**: remove or soften the verification claims (and the `# Verified:` doc tags) to match reality — no proofs currently exist — OR restore the `verif/` proofs if they were lost.
- **BACKFILL**: add a "Formal Verification" section to spec.md describing intended proof properties as future work, and correct the comment to say "planned" rather than "proved".

**Rationale**: Documentation currently asserts proofs that don't exist, which is misleading. Correct the claim regardless of direction.

**Confidence**: MEDIUM
