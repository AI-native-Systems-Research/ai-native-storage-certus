# Property-to-Truth Mapping (Pxx -> Tx)

Purpose: show which extracted truths each formal property is based on.

| Property | Description (expanded) | Based on Truths |
|---|---|---|
| P1 | Initialization dependency gate: `initialize()` succeeds only when required receptacles are bound; otherwise fails cleanly with no partial operational state. | `T1:InitGate`, `T2:RequiredDependencyBinding` |
| P2 | Operational API precondition: before successful init, public operations return `NotInitialized` and do not perform side effects. | `T1:InitGate`, `T2:RequiredDependencyBinding` |
| P3 | Uniqueness on insert-like paths: `populate`/`prepare_store` on existing key return `AlreadyExists` and preserve existing entry state. | `T3:PresenceSemantics`, `T6:FailureAtomicity`, `T8:MutationDiscipline` |
| P4 | Populate success transition: successful `populate` creates a `MemoryTier` entry with expected size metadata and presence. | `T5:TierStateSemantics` |
| P5 | Populate failure atomicity: allocation/capacity failure leaves no partial key insertion and no inconsistent accounting deltas. | `T6:FailureAtomicity` |
| P6 | Membership correctness: `check(key)` is semantically equivalent to map membership in current state. | `T3:PresenceSemantics` |
| P7 | Lookup miss semantics: missing key returns not-found outcome and preserves map/slot state. | `T3:PresenceSemantics`, `T6:FailureAtomicity` |
| P8 | MemoryTier lookup discipline: hit keeps key present and applies touch/recency behavior without illegal state drift. | `T5:TierStateSemantics`, `T8:MutationDiscipline` |
| P9 | Promotion transition: BlockDevice lookup success transitions key back to MemoryTier in a valid state. | `T5:TierStateSemantics` |
| P10 | Staging compatibility: staging lookup path remains semantically valid and does not violate state invariants. | `T5:TierStateSemantics` |
| P11 | Size mismatch contract: lookup size mismatch returns `InvalidParameter`; no partial copy and no hidden state mutation. | `T4:SizeConsistency`, `T6:FailureAtomicity` |
| P12 | Remove success postcondition: successful `remove(key)` implies key absence afterward. | `T3:PresenceSemantics`, `T8:MutationDiscipline` |
| P13 | Remove miss discipline: absent-key removal returns `KeyNotFound` and preserves state. | `T3:PresenceSemantics`, `T6:FailureAtomicity`, `T8:MutationDiscipline` |
| P14 | Touch contract: existing key updates recency/timestamp semantics only; miss returns `KeyNotFound`. | `T8:MutationDiscipline` |
| P15 | Eviction boundedness: eviction loop attempts are capped (`MAX_ATTEMPTS`) to avoid unbounded behavior. | `T9:EvictionBoundedness` |
| P16 | Eviction success condition: successful eviction must imply `used + needed <= capacity`. | `T9:EvictionBoundedness` |
| P17 | Eviction failure condition: `AllocationFailed` implies capacity condition was not achieved. | `T9:EvictionBoundedness` |
| P18 | Clean eviction safety: safe clean candidate transitions to BlockDevice state without contradiction. | `T10:EvictionSafety` |
| P19 | Blind fallback safety: failed conversion in blind path removes entry (no dangling contradictory state). | `T10:EvictionSafety` |
| P20 | `prepare_store` validation: invalid size (`0`) yields `InvalidParameter` with unchanged state. | `T7:StoreLifecycle`, `T8:MutationDiscipline` |
| P21 | Pending-write lifecycle: prepare creates pending state; commit/cancel consume it exactly once. Current runtime probe: consume-once tests were added to dispatcher (`prepare_then_commit_consumes_pending_once`, `prepare_then_cancel_consumes_pending_once`). | `T7:StoreLifecycle` |
| P22 | Commit transition: successful commit ends in persisted/BlockDevice state and clears pending status. | `T7:StoreLifecycle`, `T5:TierStateSemantics` |
| P23 | Cancel transition: successful cancel removes pending entry and clears transitional state. | `T7:StoreLifecycle`, `T5:TierStateSemantics` |
| P24 | Commit/cancel miss behavior: if no pending write exists, operation returns not-found and preserves state. | `T3:PresenceSemantics`, `T6:FailureAtomicity`, `T7:StoreLifecycle` |
| P25 | Clear-memory-tier postcondition: after clear, no key remains in MemoryTier state. | `T5:TierStateSemantics`, `T13:ExclusiveStateInvariant` |
| P26 | Clear-memory-tier accounting: returned cleared-count matches entries transitioned/removed from MemoryTier. | `T5:TierStateSemantics`, `T6:FailureAtomicity` |
| P27 | Recovery soundness: recovered extents produce dispatch entries with matching persisted metadata. | `T11:RecoverySoundness` |
| P28 | Drive mapping determinism: key-to-drive selection remains deterministic and stable. | `T12:PlacementDeterminism` |
| P29 | Threshold consistency: threshold/low-water predicates preserve intended ordering and trigger semantics. | `T12:PlacementDeterminism` |
| P30 | Global exclusivity invariant: each key has exactly one logical state representation at a time. | `T13:ExclusiveStateInvariant` |
| P31 | Global reference/state invariant: reference ownership/counting remains compatible with allowed state transitions. | `T14:ReferenceConsistencyInvariant` |

## Secondary/temporal note

- Secondary properties (background writer, shutdown drain/join, stream semantics, hysteresis) are primarily grounded in `T15:TemporalBehavior` plus supporting truths (`T5`, `T6`, `T9`, `T14`).
