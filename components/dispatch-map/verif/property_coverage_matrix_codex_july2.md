# Property Coverage Matrix (Codex) - July 2

Scope:

- Verification target: `components/dispatch-map/verif`
- Property baseline: spec-derived `P1..P31` from dispatcher workstream
- Interpretation rule:
  - `Covered`: explicitly addressed by current dispatch-map verif artifacts.
  - `Partial`: related intent is modeled, but not complete at required system scope.
  - `Not covered`: not represented in this crate at meaningful level.

## Matrix

| Property | Status in `dispatch-map/verif` | Notes |
|---|---|---|
| P1 | Not covered | `initialize()`/component lifecycle gate is dispatcher/system level. |
| P2 | Not covered | API `NotInitialized` dependency-gate behavior is not modeled here. |
| P3 | Partial | Local per entry creation shape is modeled; map-wide uniqueness (`AlreadyExists` across key-space) is not fully modeled. |
| P4 | Partial | MemoryTier per entry creation modeled locally; full dispatcher populate semantics are out of scope. |
| P5 | Partial | Some allocation/transition guards modeled; full failure-atomicity at system boundary not covered. |
| P6 | Partial | Single-entry membership-like behavior only; map-wide `check(key)` equivalence not modeled. |
| P7 | Partial | Lookup local ref behavior covered; full miss/no-mutation API semantics at component boundary not fully modeled. |
| P8 | Partial | Touch/read timing aspects partially represented; not as full dispatcher lookup+touch contract. |
| P9 | Partial | BlockDevice/MemoryTier transitions are modeled, but dispatcher promotion semantics are broader. |
| P10 | Partial | Staging behavior exists locally; full staging lookup compatibility at API/system level is broader. |
| P11 | Not covered | Lookup size-mismatch hard-fail contract is not modeled in this crate. |
| P12 | Partial | `check_removable` local guard exists; full remove semantics over key map/API not complete. |
| P13 | Partial | Local timestamp/ref updates modeled; complete miss/no-mutation API-level contracts not complete. |
| P14 | Not covered | Eviction-attempt boundedness (`MAX_EVICT_ATTEMPTS`) belongs to dispatcher eviction orchestration. |
| P15 | Not covered | Capacity-success postcondition is dispatcher memory management scope. |
| P16 | Not covered | Capacity-failure postcondition is dispatcher memory management scope. |
| P17 | Partial | Clean/evictability safety modeled locally; full clean-eviction workflow is broader. |
| P18 | Covered | Blind/per entry transition safety and evictability-related per entry transitions are strongly modeled at local level. |
| P19 | Not covered | Dispatcher blind fallback and key removal behavior across components is out of this crate scope. |
| P20 | Not covered | `prepare_store(size=0)` is dispatcher API-level. |
| P21 | Not covered | Pending-write lifecycle (`prepare/commit/cancel`) is dispatcher-level protocol. |
| P22 | Not covered | Commit transition contract is dispatcher-level. |
| P23 | Not covered | Cancel transition contract is dispatcher-level. |
| P24 | Not covered | Commit/cancel miss semantics is dispatcher-level. |
| P25 | Not covered | `clear_memory_tier` map-level behavior is dispatcher-level. |
| P26 | Partial | `recover_extent` per entry construction is covered locally; full recovery soundness over key map/workflow is broader. |
| P27 | Covered | Recovery per entry invariants/refcount safety are explicitly proved locally. |
| P28 | Not covered | Drive-index determinism (`key % num_drives`) is dispatcher-level routing logic. |
| P29 | Not covered | Watermark consistency is dispatcher config/policy level. |
| P30 | Verified (L2, map-wide) | `map_inv` over `FMap<u64, DispatchEntry>` conjoins exclusive-state + binary-write_ref for every present key. `lemma_exclusive_state` proves exclusivity holds unconditionally for any entry (`Location` is a single-variant enum). `map_create_entry`/`map_update_entry`/`map_remove_entry` prove all three map-mutation shapes preserve `map_inv` — the L1→L2 gluing lift (assumption A7 discharged for these ops). 7 VCs / 7 goals across 4 `.coma` files. Sequential (Mutex collapsed to a ghost map); does not model concurrent interleavings. |
| P31 | Verified (L2, map-wide) | Co-proved by the same `map_inv`: the binary-`write_ref` conjunct is the cross-cutting refcount well-formedness the per-entry ops preserve locally, now quantified over all present keys and shown preserved by insert-fresh / overwrite / remove. Per-entry refcount roundtrips (`take_read`/`release_read`/`take_write`/`release_write`/`downgrade_reference`) remain the L1 evidence beneath it. |

## Summary

Current `dispatch-map/verif` is strong for **local per entry correctness** and reference/state transition safety.
It does **not** by itself provide full dispatcher/system property coverage.

## Recommended Next Step

Use this matrix together with:

- [crosscheck_codex_july2.md](/home/cornel/ai-native-storage-certus/components/dispatch-map/verif/crosscheck_codex_july2.md)
- [explanation_crosscheck_codex_july2.md](/home/cornel/ai-native-storage-certus/components/dispatch-map/verif/explanation_crosscheck_codex_july2.md)

to drive a staged plan:

1. keep dispatch-map local proofs as trusted building blocks,
2. close dispatcher-level gaps (`P11`, `P21-P24`, init gate, temporal/system properties),
3. add explicit map-level ghost model where global key-space claims are required.
