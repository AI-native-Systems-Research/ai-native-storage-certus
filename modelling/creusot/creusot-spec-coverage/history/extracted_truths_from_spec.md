# Extracted Truths From Dispatcher Spec

Purpose: define non-negotiable semantic truths directly from the dispatcher spec before writing formal properties.

Format: `Tx: Abbreviation: Description`.

- `T1: InitGate`: Dispatcher operations require successful initialization before normal use. Implementation hint: every public operation should guard with `ensure_initialized()` (or equivalent) and fail with `NotInitialized` before any mutation/copy/IO side effect.
- `T2: RequiredDependencyBinding`: Required receptacles must be bound before operations that depend on them. The required set is operation-specific (e.g., `dispatch_map` + `memory_tier` required for core cache operations; some services validated lazily). Implementation hint: fail deterministically when missing dependency is first needed.
- `T3: PresenceSemantics`: Key presence/absence must be reflected consistently in `check`, `lookup`, `remove`, and related errors. Implementation hint: ensure miss paths return canonical not-found errors and preserve state.
- `T4: SizeConsistency`: Requested size and stored size semantics must be consistent; mismatch is a contract failure. Implementation hint: reject mismatch with `InvalidParameter`; do not truncate copy lengths.
- `T5: TierStateSemantics`: Entry state transitions across `MemoryTier`, `BlockDevice`, `Staging`, and `PendingWrite` must follow defined rules. Implementation hint: encode explicit transition guards and forbid impossible transitions.
- `T6: FailureAtomicity`: On failure paths, avoid partial/contradictory state updates. Implementation hint: if any step fails (allocation/copy/convert), either rollback or leave state unchanged.
- `T7: StoreLifecycle`: `prepare_store -> commit_store/cancel_store` is a protocol with single-consume and cleanup guarantees. Implementation hint: pending write entry must be consumed exactly once and not leak intermediate state. Current check focus: dispatcher tests `prepare_then_commit_consumes_pending_once` and `prepare_then_cancel_consumes_pending_once`.
- `T8: MutationDiscipline`: `touch/remove/update` operations must preserve invariants and return precise miss/invalid-state outcomes. Implementation hint: operations should not over-map distinct failures into generic errors when semantic distinction matters.
- `T9: EvictionBoundedness`: Eviction logic is bounded and must either establish capacity condition or return explicit failure. Implementation hint: loops need explicit attempt bounds and success/failure postconditions.
- `T10: EvictionSafety`: Clean and blind eviction paths must maintain representational safety (including fallback behavior). Implementation hint: clean path transitions to block state when safe; blind path must still avoid dangling entries.
- `T11: RecoverySoundness`: Recovery/reconstruction must create entries matching persisted extent metadata. Implementation hint: recovered `(key, offset, size)` must be reflected exactly in map state.
- `T12: PlacementDeterminism`: Deterministic placement/config relations (drive mapping, threshold/low-water ordering) must hold. Implementation hint: placement function and watermark predicates must be stable and auditable.
- `T13: ExclusiveStateInvariant`: A key must not occupy contradictory logical states simultaneously. Implementation hint: represent state as a single variant and prove exclusivity globally.
- `T14: ReferenceConsistencyInvariant`: Reference ownership/counting constraints must stay consistent with state transitions. Implementation hint: disallow remove/unsafe transitions while active refs exist.
- `T15: TemporalBehavior`: Background/shutdown/async stream behavior has temporal obligations. Implementation hint: model progress assumptions explicitly (fairness/step bounds) and track them in assumption ledger.

## Notes

- These truths are specification semantics, not implementation details.
- Properties (`Pxx`) are derived from these truths and then encoded in Creusot artifacts.
- Truths should remain relatively stable; properties may be split/combined for proof engineering.
