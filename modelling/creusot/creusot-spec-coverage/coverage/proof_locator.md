# Proof Locator

Purpose:
- Answer the two questions colleagues ask most:
  1. **"Where is the proof for property Px?"** → which verif crate, which proof function, which `.coma` artifact.
  2. **"What does function `<odd-name>` prove?"** → one line of plain English per proof function.
- This is a navigation index. For status/counts see `coverage_report.md`; for the full contract text and scope caveats see `properties_to_prove.md`.

_Last refreshed: 2026-07-16._

## How to read an entry

- **Verif crate** → the Rust source with the annotated proof function lives at `<crate>/src/lib.rs`.
- **Proof function** → the `#[check(ghost)]` / annotated function name; grep for it in that `lib.rs`.
- **`.coma`** → the generated verification artifact, replayable with `why3find prove -r --summary <file>.coma`.

Artifact path prefixes:
- Dispatcher: `components/dispatcher/verif/verif/dispatcher_verif_rlib/<name>.coma`
- Dispatch-map: `components/dispatch-map/verif/verif/dispatch_map_verif_rlib/<name>.coma`

## Property → proof location (P1–P31)

| Px | Verif crate | Proof function | `.coma` | In plain English, this proves… |
|---|---|---|---|---|
| P1 | dispatcher | `initialize_dependency_guards` | `initialize_dependency_guards.coma` | `initialize` fails when a required dependency is unbound (`NotInitialized`) or `data_pci_addrs` is empty (`InvalidParameter`), and only succeeds when both are bound and non-empty — in that check order. |
| P2 | dispatcher | `ensure_initialized` | `ensure_initialized.coma` | Operational APIs return `NotInitialized` before a successful init. |
| P3 | dispatcher (map model) | `create_entry` | `create_entry.coma` | Inserting a duplicate key fails `AlreadyExists` and leaves existing data untouched; a fresh key is inserted. |
| P4, P5 | dispatcher | `register_memory_tier` | `register_memory_tier.coma` | Populate Phase-3 registration: on success the key maps to `MemoryTier`; on failure the map is unchanged and the key absent (atomic, no leaked partial entry). |
| P6 | dispatcher (map model) | `check_key` | `check_key.coma` | `check(key)` returns a bool exactly equal to map membership; read-only, no mutation. |
| P7 | dispatcher | `lookup_miss_decision` | `lookup_miss_decision.coma` | Lookup on a missing key returns `KeyNotFound` (or `NotInitialized` pre-init) and mutates nothing. |
| P8 | dispatcher | `memtier_lookup_hit` | `memtier_lookup_hit.coma` | A MemoryTier lookup hit keeps the entry `MemoryTier` in both outcomes (a read never demotes/removes it); the refresh flag fires exactly when served. |
| P9 | dispatcher | `promote_block_lookup` | `promote_block_lookup.coma` | A BlockDevice lookup: success promotes the entry in-place to `MemoryTier`; failure leaves it `BlockDevice` (never lost). |
| P10 | dispatch-map | `lifecycle_staging_read`, `lifecycle_staging_to_block` | `lifecycle_staging_read.coma`, `lifecycle_staging_to_block.coma` | *(legacy)* Staging-path reads stay safe if encountered; staging no longer emphasized by runtime. |
| P11 | dispatcher | `resolve_lookup` | `resolve_lookup.coma` | Lookup copies `min(requested, stored)` on a hit (no over-copy), `KeyNotFound` on a miss; the `MismatchSize ⇒ InvalidParameter` arm is proved but defensively unreachable in production. |
| P12, P13 | dispatcher (map model) | `remove_entry` | `remove_entry.coma` | Remove: present+unreferenced ⇒ key absent afterward (P12); absent ⇒ `KeyNotFound` unchanged; busy (`read_ref>0 ∨ write_ref>0`) ⇒ `ActiveReferences` unchanged (P13). |
| P14 | dispatcher | `touch_decision` | `touch_decision.coma` | Touch on a present key refreshes metadata; absent ⇒ `KeyNotFound`; refresh flag fires exactly on the hit path. |
| P15 | dispatcher | `evict_attempt_budget` | `evict_attempt_budget.coma` | The eviction loop is bounded: attempts ≤ `max_attempts+1`, and `Err(AllocationFailed)` occurs exactly at full budget — for *any* memory-pressure behavior (opaque oracle). |
| P16, P17 | dispatcher | `evict_for_capacity` | `evict_for_capacity.coma` | On success, freed capacity satisfies `used+needed ≤ capacity` (P16); on failure the target was not reached, `used+needed > capacity`, and the budget is exhausted (P17). |
| P18 | dispatch-map | `convert_memory_tier_to_block` (+ `lifecycle_memory_tier_to_block`, `lifecycle_write_through_safety`) | `convert_memory_tier_to_block.coma`, `lifecycle_memory_tier_to_block.coma`, `lifecycle_write_through_safety.coma` | Clean eviction transitions a MemoryTier entry to BlockDevice (demote, not delete); write-through keeps SSD data safe. Per-entry (L1). |
| P19 | dispatcher | `blind_evict_fallback` | `blind_evict_fallback.coma` | Blind-LRU fallback never leaves a dangling `MemoryTier`: success ⇒ `BlockDevice` (data preserved); failure ⇒ key dropped. Sequential single-map skeleton (Partial). |
| P20 | dispatcher | `prepare_store_guards` (re-anchored to `populate`) | `prepare_store_guards.coma` | *(legacy)* Zero-size / invalid input is rejected safely; guard semantics stay valid after direct-store API removal. |
| P21 | dispatcher | `insert_pending`, `consume_once` | `insert_pending.coma`, `consume_once.coma` | *(stale, legacy)* Pending-write consume-once protocol; mirrors removed `pending_writes` API. |
| P22 | — | — (retired) | — | *(retired)* Commit-path pending-write clearing; workflow removed. |
| P23 | — | — (retired) | — | *(retired)* Cancel-path pending-write clearing; workflow removed. |
| P24 | dispatcher | `consume_pending` | `consume_pending.coma` | *(stale, legacy)* Commit/cancel without pending write ⇒ `KeyNotFound`; mirrors removed API. |
| P25, P26 | dispatcher (map model) | `clear_all` | `clear_all.coma` | `clear_memory_tier` drains the tier to empty (P25) and returns a count equal to the initial entry count (P26). Loop invariant+variant proof. |
| P28 | dispatcher | `drive_index` | `drive_index.coma` | Drive-selection index is always in range (`num_drives>0 ⇒ result < num_drives`); deterministic pure function. |
| P29 | dispatcher | `evictor_decisions` | `evictor_decisions.coma` | SSD-evictor thresholds run in the intended direction (start iff `util≥threshold`, stop iff `util<low_watermark`) and never both at once, given a well-formed band. |
| P30, P31 | dispatch-map | `map_inv` + `lemma_exclusive_state` + `map_create_entry` / `map_update_entry` / `map_remove_entry` | `lemma_exclusive_state.coma`, `map_create_entry.coma`, `map_update_entry.coma`, `map_remove_entry.coma` | Map-wide invariant: every key is in exactly one state (Staging XOR BlockDevice XOR MemoryTier) with binary write_ref (P30/P31), preserved by insert-fresh / overwrite / remove. |

## Decoding an unfamiliar proof-function name

Names follow a small set of conventions, not random strings:

- **`<verb>_<noun>` decision skeletons** (`lookup_miss_decision`, `touch_decision`, `memtier_lookup_hit`, `promote_block_lookup`, `blind_evict_fallback`, `resolve_lookup`) — a pure model of one API's branch logic; the name is the runtime path it mirrors.
- **`map_<op>`** (`map_create_entry`, `map_update_entry`, `map_remove_entry`) — a map-*wide* mutation shape proved to preserve `map_inv` (the L1→L2 lift for P30/P31).
- **`lemma_<claim>`** (`lemma_exclusive_state`) — a standalone logical lemma, often precondition-free and structural.
- **`lifecycle_<transition>`** (`lifecycle_memory_tier_to_block`, `lifecycle_recover_extent`, …) — per-entry state-transition proofs in dispatch-map (L1 evidence).
- **`roundtrip_<op>`** (`roundtrip_read`, `roundtrip_write`, `roundtrip_downgrade`, `roundtrip_two_concurrent_reads`) — refcount take/release symmetry for P31.
- **`<state>_<action>` / `evict_*` / `*_guards`** — capacity, eviction-budget, or input-guard proofs; the noun tells you the concern.

If a `.coma` name is not in the table above, it is **supporting dispatch-map evidence** (per-entry lifecycle, refcount roundtrips, `create_*`, `take_*`/`release_*`, `check_removable`, `is_evictable`, `convert_to_storage`, `eviction_fairness`, `lifecycle_cold_evicted_before_hot`, `take_read_prevents_eviction`) that underpins the map-wide P18/P27/P30/P31 claims rather than owning a top-level Px.
