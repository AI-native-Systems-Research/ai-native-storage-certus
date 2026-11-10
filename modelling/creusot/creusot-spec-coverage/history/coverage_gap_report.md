# Coverage Gap Report (Auto-Generated)

## Summary
- Total properties in matrix: **31**
- Covered: **2**
- Partial: **14**
- Not covered: **15**
- High-priority gaps: **8**

## Annotation Status Snapshot
- Verified: **4**
- Stale: **2**
- Unchecked: **23**
- Retired: **2**

## High-Priority Gap Worklist
- `P1` (Not covered): Add dispatcher-level initialization/dependency gate contracts and proofs.
- `P2` (Not covered): Add dispatcher-level initialization/dependency gate contracts and proofs.
- `P11` (Not covered): Decide ownership of size-mismatch contract (dispatch-map vs dispatcher) and prove hard-fail semantics at owner layer.
- `P20` (Not covered): Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees.
- `P21` (Not covered): Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees.
- `P22` (Not covered): Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees.
- `P23` (Not covered): Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees.
- `P24` (Not covered): Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees.

## Not Covered Properties
| ID | Owner | Interface | Priority | Why it matters (notes) | Suggested action |
|---|---|---|---|---|---|
| P1 | dispatcher | `components/interfaces/src/idispatcher.rs` | High | `initialize()`/component lifecycle gate is dispatcher/system level. | Add dispatcher-level initialization/dependency gate contracts and proofs. |
| P2 | dispatcher | `components/interfaces/src/idispatcher.rs` | High | API `NotInitialized` dependency-gate behavior is not modeled here. | Add dispatcher-level initialization/dependency gate contracts and proofs. |
| P11 | dispatcher | `components/interfaces/src/idispatcher.rs` | High | Lookup size-mismatch hard-fail contract is not modeled in this crate. | Decide ownership of size-mismatch contract (dispatch-map vs dispatcher) and prove hard-fail semantics at owner layer. |
| P14 | dispatcher | `components/interfaces/src/idispatcher.rs` | Medium | Eviction-attempt boundedness (`MAX_EVICT_ATTEMPTS`) belongs to dispatcher eviction orchestration. | Prove dispatcher policy/workflow property in dispatcher-verif crate and link dispatch-map lemmas as assumptions. |
| P15 | dispatcher | `components/interfaces/src/idispatcher.rs` | Medium | Capacity-success postcondition is dispatcher memory management scope. | Prove dispatcher policy/workflow property in dispatcher-verif crate and link dispatch-map lemmas as assumptions. |
| P16 | dispatcher | `components/interfaces/src/idispatcher.rs` | Medium | Capacity-failure postcondition is dispatcher memory management scope. | Prove dispatcher policy/workflow property in dispatcher-verif crate and link dispatch-map lemmas as assumptions. |
| P19 | dispatcher | `components/interfaces/src/idispatcher.rs` | Medium | Dispatcher blind fallback and key removal behavior across components is out of this crate scope. | Prove dispatcher policy/workflow property in dispatcher-verif crate and link dispatch-map lemmas as assumptions. |
| P20 | dispatcher | `components/interfaces/src/idispatcher.rs` | High | `prepare_store(size=0)` is dispatcher API-level. | Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees. |
| P21 | dispatcher | `components/interfaces/src/idispatcher.rs` | High | Pending-write lifecycle (`prepare/commit/cancel`) is dispatcher-level protocol. | Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees. |
| P22 | dispatcher | `components/interfaces/src/idispatcher.rs` | High | Commit transition contract is dispatcher-level. | Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees. |
| P23 | dispatcher | `components/interfaces/src/idispatcher.rs` | High | Cancel transition contract is dispatcher-level. | Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees. |
| P24 | dispatcher | `components/interfaces/src/idispatcher.rs` | High | Commit/cancel miss semantics is dispatcher-level. | Add dispatcher store-lifecycle proof harnesses (prepare/commit/cancel) with mode split and consume-once guarantees. |
| P25 | dispatcher | `components/interfaces/src/idispatcher.rs` | Medium | `clear_memory_tier` map-level behavior is dispatcher-level. | Prove dispatcher policy/workflow property in dispatcher-verif crate and link dispatch-map lemmas as assumptions. |
| P28 | dispatcher | `components/interfaces/src/idispatcher.rs` | Medium | Drive-index determinism (`key % num_drives`) is dispatcher-level routing logic. | Prove dispatcher policy/workflow property in dispatcher-verif crate and link dispatch-map lemmas as assumptions. |
| P29 | dispatcher | `components/interfaces/src/idispatcher.rs` | Medium | Watermark consistency is dispatcher config/policy level. | Prove dispatcher policy/workflow property in dispatcher-verif crate and link dispatch-map lemmas as assumptions. |

## Partial Properties
| ID | Owner | Interface | Priority | Current limit (notes) | Suggested action |
|---|---|---|---|---|---|
| P3 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | Local per entry creation shape is modeled; map-wide uniqueness (`AlreadyExists` across key-space) is not fully modeled. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P4 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Low | MemoryTier per entry creation modeled locally; full dispatcher populate semantics are out of scope. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P5 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Low | Some allocation/transition guards modeled; full failure-atomicity at system boundary not covered. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P6 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | Single-entry membership-like behavior only; map-wide `check(key)` equivalence not modeled. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P7 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | Lookup local ref behavior covered; full miss/no-mutation API semantics at component boundary not fully modeled. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P8 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | Touch/read timing aspects partially represented; not as full dispatcher lookup+touch contract. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P9 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | BlockDevice/MemoryTier transitions are modeled, but dispatcher promotion semantics are broader. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P10 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | Staging behavior exists locally; full staging lookup compatibility at API/system level is broader. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P12 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | `check_removable` local guard exists; full remove semantics over key map/API not complete. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P13 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | Local timestamp/ref updates modeled; complete miss/no-mutation API-level contracts not complete. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P17 | dispatcher | `components/interfaces/src/idispatcher.rs` | Low | Clean/evictability safety modeled locally; full clean-eviction workflow is broader. | Prove dispatcher policy/workflow property in dispatcher-verif crate and link dispatch-map lemmas as assumptions. |
| P26 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | `recover_extent` per entry construction is covered locally; full recovery soundness over key map/workflow is broader. | Strengthen from per-entry to map-level relation where required; add cross-key ghost state and preservation lemmas. |
| P30 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | Some local state exclusivity invariants exist; no full unbounded map-level global invariant proof. | Introduce map-level ghost model and prove global invariants (not only per-entry). |
| P31 | dispatch-map (+ map-level ghost upgrade) | `components/interfaces/src/idispatch_map.rs` | Medium | Strong local refcount invariants; no full global reference/state consistency across map + dispatcher workflows. | Introduce map-level ghost model and prove global invariants (not only per-entry). |

## Interface Annotation Backlog (Daniel Skill Alignment)
| Property | Interface target | Annotation bucket | Source |
|---|---|---|---|
| P1 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P2 | `components/interfaces/src/idispatcher.rs` | Verified | annotation-doc |
| P3 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P4 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P5 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P6 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P7 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P8 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P9 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P10 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P11 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P12 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P13 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P14 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P15 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P16 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P17 | `components/interfaces/src/idispatcher.rs` | Unchecked | coverage-matrix |
| P18 | `components/interfaces/src/idispatch_map.rs` | Verified | coverage-matrix |
| P19 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P20 | `components/interfaces/src/idispatcher.rs` | Verified | annotation-doc |
| P21 | `components/interfaces/src/idispatcher.rs` | Stale | annotation-doc |
| P22 | `components/interfaces/src/idispatcher.rs` | Retired | annotation-doc |
| P23 | `components/interfaces/src/idispatcher.rs` | Retired | annotation-doc |
| P24 | `components/interfaces/src/idispatcher.rs` | Stale | annotation-doc |
| P25 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P26 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P27 | `components/interfaces/src/idispatch_map.rs` | Verified | coverage-matrix |
| P28 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P29 | `components/interfaces/src/idispatcher.rs` | Unchecked | annotation-doc |
| P30 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |
| P31 | `components/interfaces/src/idispatch_map.rs` | Unchecked | coverage-matrix |

## Spec Cross-Check Table (Template)
| Spec Requirement | Status | Property | Owner | Notes |
|---|---|---|---|---|
| <requirement text> | ✔ Verified / ⚠ Unchecked / ✗ No match | Pxx | dispatch-map/dispatcher | <gap or evidence> |

## Notes
- This report summarizes matrix status labels; it does not run Creusot.
- Use together with concrete proof runs and trusted/assumption ledgers.
