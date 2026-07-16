# Dispatcher Verif — Property Coverage (July 7)

Scope:

- Verification target: `components/dispatcher/verif`
- Property baseline: spec-derived `P1..P31` (canonical namespace) — see
  `components/dispatch-map/verif/property_coverage_matrix_codex_july2.md`.
- This crate owns **dispatcher / system-level** properties. Per-entry
  properties remain owned by `components/dispatch-map/verif`.

Annotation buckets (consistent with the interface-annotation style):

- `# Verified` — a Creusot proof artifact exists, `cargo creusot` is green, and
  the model still mirrors live dispatcher code.
- `# Stale` — a green Creusot proof exists, but the dispatcher code it mirrors
  was removed/reworked, so the proof no longer stands as evidence for the
  current runtime. Kept for history; must be retargeted or retired.
- `# Unchecked` — targeted but not yet proved (or only partially modeled).

> **Retargeting notice (2026-07-09).** Commit `25a7273` (*"remove vestigial
> staging buffer concept"*) deleted the `prepare_store` / `commit_store` /
> `cancel_store` / `pending_writes` API from the dispatcher on **both**
> `unstable` and `unstable-creusot` (branches are byte-identical; CI sync is
> healthy). This was a standalone staging refactor — **not** the P11 fix. The
> current write/store lifecycle is `populate` → `reserve_memory` →
> `copy_gpu_to_memory_async` → `copy_gpu_to_memory_completed`, with
> `release_memory` as idempotent cancel. Consequences:
> - **P2** stays `# Verified` — `ensure_initialized` still exists (`:271`).
> - **P20** stays `# Verified` — the `size==0 → InvalidParameter` guard simply
>   moved from `prepare_store` to `populate` (`:1915`); the guard logic the
>   proof models is unchanged.
> - **P21 / P24** are now `# Stale` — they mirror the deleted `pending_writes`
>   map. There is no pending-write map in the new lifecycle, so consume-once has
>   no live counterpart. Retarget or retire.
> - **P11** is the new keystone target (see status table).

## Proof artifacts in this crate

| Artifact (`verif/dispatcher_verif_rlib/*.coma`) | Property | Mirrors (dispatcher/src/lib.rs) | Status |
|---|---|---|---|
| `ensure_initialized` | **P2** — operational APIs fail `NotInitialized` before init | `ensure_initialized()?` prefix, called by every operational API (`:271`) | `# Verified` |
| `prepare_store_guards` | **P20** — `size==0 → InvalidParameter`, no mutation | init + size guard now on `populate` (`:1915`); tests at `:3315`, `:3100` | `# Verified` (re-anchored) |
| `consume_pending` | **P24** — commit/cancel miss ⇒ `KeyNotFound`, map unchanged | *removed* `pending_writes…remove(&key).ok_or(KeyNotFound)?` (deleted by `25a7273`) | `# Stale` |
| `insert_pending` | **P21** (insert side) — after prepare, key present | *removed* `pending_writes…insert(key, …)` (deleted by `25a7273`) | `# Stale` |
| `consume_once` | **P21** — consume-exactly-once: first `Ok`, second `KeyNotFound` | *removed* prepare-insert then commit/cancel remove (deleted by `25a7273`) | `# Stale` |

`cargo creusot` → **all 5 proofs still green**. Greenness reflects internal
consistency of the models; the `# Stale` rows are green but no longer mirror
live code (see retargeting notice above).

Note on "no mutation": the P2/P20 guards return before any `dispatch_map` /
`pending_writes` access, so state-preservation is structural on those paths.
The map-level no-mutation clause proved for P24 (`ext_eq` on the miss branch)
remains a correct fact about the `FMap` model, but the `pending_writes` map it
mirrored no longer exists.

## Dispatcher-owned properties — status against `P1..P31`

Only properties this crate owns are listed. The per-entry map-mutation
decision logic behind several of these (P3, P6, P7, P12, P13) is now mirrored
here directly over a logic-level `FMap` (see rows below); the remaining
per-entry properties (P4–P5, P8–P10, P17, P18, P26, P27, P30, P31) stay in
`dispatch-map/verif`.

| Property | Owner | Status here | Next proof target |
|---|---|---|---|
| P1  — `initialize()` iff required receptacles bound | dispatcher | `# Verified` (`initialize_dependency_guards`) | 1/1 VC discharged in 1 `.coma` file (`initialize_dependency_guards.coma`). Mirrors the `initialize` guard prefix (`lib.rs:1053-1065`): `dispatch_map` unbound → `NotInitialized`; then `memory_tier` unbound → `NotInitialized`; then empty `data_pci_addrs` → `InvalidParameter`; both bound + non-empty → `Ok`. Check order preserved so the error variant matches live code. Pairs with P2. |
| P2  — operational APIs fail `NotInitialized` pre-init | dispatcher | `# Verified` (`ensure_initialized`) | — |
| P3  — duplicate-key insert ⇒ `AlreadyExists`, no overwrite | dispatcher | `# Verified` (`create_entry`) | 3/3 VCs discharged in 1 `.coma` file (`create_entry.coma`). Mirrors dispatch-map `create_memory_tier_entry` (`dispatch-map/src/lib.rs:367-399`): key already present ⇒ `AlreadyExists` with the map extensionally unchanged (`(^map).ext_eq(*map)`, so no overwrite of existing data); absent ⇒ inserted (`!(*map).contains(key) && (^map).contains(key)`). Modeled over a logic-level `FMap`. |
| P6  — `check(key)` result matches membership truth | dispatcher | `# Verified` (`check_key`) | 2/2 VCs discharged in 1 `.coma` file (`check_key.coma`). Mirrors `check` (`lib.rs:1887-1906`): `!initialized ⇒ NotInitialized`; `initialized ⇒ Ok(b)` with `b == (*map).contains(key)` — the returned bool equals map membership exactly. Read-only (`&FMap`), so no-mutation is structural. |
| P7  — lookup miss ⇒ `KeyNotFound`, state preserved | dispatcher | `# Verified` (`lookup_miss_decision`) | 1/1 VC discharged in 1 `.coma` file (`lookup_miss_decision.coma`). Mirrors the miss branch of `lookup_async` (`lib.rs:1812-1816`): `!initialized ⇒ NotInitialized`; `initialized ∧ absent ⇒ KeyNotFound`; `initialized ∧ present ⇒ Ok`. No-mutation on miss captured by a `refreshed`-style flag proved `== (initialized && key_present)`. |
| P12 — successful remove ⇒ key absent afterward | dispatcher | `# Verified` (`remove_entry`, `Ok` arm) | Co-proved by the P13 artifact — 4/4 VCs in 1 `.coma` file (`remove_entry.coma`). The `Ok` arm proves `(*map).contains(key) && !(^map).contains(key)`: the key was present and is absent after a successful remove. See P13 row for the full contract. |
| P13 — remove absent ⇒ `KeyNotFound`, no mutation | dispatcher | `# Verified` (`remove_entry`) | 4/4 VCs discharged in 1 `.coma` file (`remove_entry.coma`). Mirrors dispatch-map `remove` (`dispatch-map/src/lib.rs:310-333`): absent ⇒ `KeyNotFound` with `(^map).ext_eq(*map)`; busy entry (`read_ref>0 ∨ write_ref>0`) ⇒ `InvalidParameter`/`ActiveReferences`, also unchanged; present + unreferenced ⇒ removed. `EntryModel` carries `read_ref`/`write_ref` to faithfully discharge the active-references guard. Also discharges **P12** (`Ok` arm). |
| P11 — lookup size-mismatch hard-fail, no partial copy | dispatcher | `# Verified (product path)` (`resolve_lookup`) | 3/3 VCs discharged in 1 `.coma` file (`resolve_lookup.coma`). Models the `LookupResult → Result` match in `lookup_async` (`:1786-1830`) / `batch_lookup` (`:1420-1442`). Proves the reachable product path: `MemoryTier`/`BlockDevice` hit ⇒ `Ok` with `n ≤ requested ∧ n ≤ stored` (copy clamped to `min`, so **no partial/over-copy**); `NotExist ⇒ KeyNotFound`, no copy. The `MismatchSize ⇒ InvalidParameter` (no copy) arm is also proved but is **defensive, intentionally-unreachable code**: production `dispatch-map::lookup` is key-only (`dispatch-map/src/lib.rs:115`) and never emits `MismatchSize`; only the `MockDispatchMap` test injects it (`dispatcher/src/lib.rs:2601`). Safety rests on the invariant *requested size == stored size* at lookup — **TODO: trace `ipc_handle.size` vs stored size to confirm** (until then an assumption, not a proved fact). |
| P14 — touch refreshes on hit, `KeyNotFound` on miss | dispatcher | `# Verified` (`touch_decision`) | 1/1 VC discharged in 1 `.coma` file (`touch_decision.coma`). Mirrors `touch` (`lib.rs:2172-2188`) + map `touch` (`dispatch-map/src/lib.rs:335-347`): `!initialized ⇒ NotInitialized`; `initialized ∧ absent ⇒ KeyNotFound`; `initialized ∧ present ⇒ Ok`. Metadata-refresh modeled as a `refreshed` flag proved `== is_ok`, so refresh happens exactly on the hit path and never on a miss/pre-init (the "no mutation" half). |
| P15 — eviction attempt bound (`max_eviction_attempts`) | dispatcher | `# Verified` (`evict_attempt_budget`) | 2 VCs (`vc_under_pressure`, `vc_evict_attempt_budget`) discharged in 1 `.coma` file (`evict_attempt_budget.coma`). Mirrors the attempt-budget control flow of `evict_for_space` (`lib.rs:533-586`): `attempts += 1`, then `if attempts > max_attempts { return Err }`. The while-guard `used + needed > capacity` reads concurrently-mutated external state, so it is abstracted as an opaque oracle `under_pressure` (`#[trusted]`, no postcondition ⇒ arbitrary bool each call) — this proves the bound for **every** pressure behavior, including adversarial (pressure never clears). Theorem: `result.1@ <= max_attempts@ + 1` (at most `max_attempts+1` attempts) and `result.0 ==> result.1@ == max_attempts@ + 1` (exhaustion / runtime `Err(AllocationFailed)` occurs exactly at full-budget). Variant `max_attempts + 1 - attempts`, invariant `attempts <= max_attempts`. Precondition `max_attempts < usize::MAX@` guards the `+= 1` at the boundary. Abstraction L3 (opaque external guard; counter logic mirrored exactly). |
| P16 — eviction success ⇒ `used + needed <= cap` (+ P17 dual) | dispatcher | `# Verified` (`evict_for_capacity`) | 2 VCs (`vc_tier_used`, `vc_evict_for_capacity`; 10 goals after splitting) discharged in 1 `.coma` file (`evict_for_capacity.coma`). Same loop as P15 (`lib.rs:534-586`), proving the capacity predicate on BOTH exits. **P16** (`Ok` arm): the trailing `Ok(used)` is reachable only when the guard `used + needed > capacity` is FALSE, so on success `used@ + needed@ <= capacity@` (room for the pending `needed`-byte allocation). **P17** (`Err` arm, co-proved): the budget-exhaustion `return Err((attempts, used))` sits INSIDE the loop body, reachable only because the guard was TRUE at the loop head and `used` is unchanged since, so `used@ + needed@ > capacity@` — the capacity target was NOT reached when eviction gave up. Unlike P15, the guard's *meaning* is modeled — `used`/`needed`/`capacity` are real integers with the real comparison; eviction's effect on `used` stays opaque (`tier_used`, `#[trusted]`) since neither property claims eviction makes progress. One trusted assumption: `tier_used` ensures `result@ + needed@ <= usize::MAX@` (a byte count + one allocation never overflows 64-bit `usize`), used solely for the guard's in-bounds check. Loop invariant carries that bound + `attempts <= max_attempts`. Err arm's `attempts@ == max_attempts@ + 1` pairs with P15. Abstraction L0 (capacity predicates) / trusted used-oracle. |
| P17 — eviction failure ⇒ capacity not achieved | dispatcher | `# Verified` (`evict_for_capacity`, `Err` arm) | Co-proved by the P16 artifact (`evict_for_capacity.coma`): the `Err((attempts, used))` arm proves `used@ + needed@ > capacity@` alongside `attempts@ == max_attempts@ + 1`. See P16 row. |
| P19 — blind eviction fallback removes key | dispatcher | `# Partial` (L1 decision skeleton) (`blind_evict_fallback`) | 3 VCs (`vc_insert_ghost_u64`, `vc_remove_ghost_u64`, `vc_blind_evict_fallback`; 3 goals) discharged in 1 `.coma` file (`blind_evict_fallback.coma`). Models the blind-LRU fallback of `evict_for_space` (`lib.rs:572-583`) over a single-map `FMap<u64, TierState>`: precondition the evicted key is `MemoryTier` (mt slot just freed by `evict_lru_for_key`); `convert_memory_tier_to_block` outcome as a bool. Proves the local decision — success ⇒ `(^map).get(key) == Some(BlockDevice)` (demoted, data preserved, P18-consistent); failure ⇒ `!(^map).contains(key)` (dropped) — and the P19 headline `(^map).get(key) != Some(MemoryTier)` (never left dangling). **L1 / not a full discharge:** the full guarantee is a CROSS-MAP (mt↔dm) whole-map invariant (belongs with P30/P31, L2), and the real hazard is CONCURRENT (`:566` "another thread may have concurrently evicted this key") — outside Creusot's sequential model. Certifies the sequential decision only. |
| P20 — `size==0 → InvalidParameter` (now on `populate`) | dispatcher | `# Verified` (`prepare_store_guards`, re-anchored to `populate`) | — |
| P21 — pending-write consume-once (prepare/commit/cancel) | dispatcher | `# Stale` (`insert_pending`, `consume_once`) | Mirrors deleted `pending_writes` map. Retire, or reconceive against `reserve_memory` → `copy_gpu_to_memory_completed` / `release_memory` lifecycle. |
| P22 — commit ⇒ PendingWrite → BlockDevice, pending cleared | dispatcher | `# Retired` | Depended on removed `commit_store` / `pending_writes`. No live counterpart. |
| P23 — cancel ⇒ key absent, pending cleared | dispatcher | `# Retired` | Depended on removed `cancel_store` / `pending_writes`. No live counterpart. |
| P24 — commit/cancel miss ⇒ `KeyNotFound`, no mutation | dispatcher | `# Stale` (`consume_pending`) | Mirrors deleted `pending_writes` map. Retire, or reconceive against `release_memory` idempotent-cancel semantics. |
| P25 — `clear_memory_tier` leaves no MemoryTier entries (+ P26 count) | dispatcher | `# Verified` (`clear_all`) | 2 VCs (`vc_remove_one_ghost_u64`, `vc_clear_all`; 12 goals after splitting) discharged in 1 `.coma` file (`clear_all.coma`) — the crate's first loop proof. Mirrors the drain loop of `clear_memory_tier` (`lib.rs:2343-2348`): `evict_lru` modeled by `FMap::remove_one_ghost` (identical contract: `None ⇒ empty ∧ unchanged`; `Some ⇒ one entry removed). Loop invariant `count + map.len() == initial_len` (via `snapshot!`) + `map.len()` variant. Proves **P25** `(^map).is_empty()` (no entries remain) and **P26** `result@ == (*map).len()` (count == entries cleared). Precondition `len() <= usize::MAX@` captures the runtime fact that the tier's entry count is a `usize` (guards the `count += 1` overflow). Abstraction L2: whole-map drain; `evict_lru`'s LRU *order* and the per-key dispatch-map bookkeeping (convert-to-block / remove) are abstracted, both irrelevant to emptiness/count. |
| P28 — drive-index determinism (`key % num_drives`) | dispatcher | `# Verified` (`drive_index`) | 2/2 VCs discharged (`vc_drive_index`, `vc_wrapping_mul`) in 1 `.coma` proof file (`drive_index.coma`). Theorem: `num_drives>0 ==> result < num_drives` (index always in range; no OOB drive select). Determinism/stability is structural (pure fn). Splitmix64 body kept verbatim; only the `% num_drives` bound is proved. |
| P29 — watermark/threshold consistency | dispatcher | `# Verified (direction/hysteresis)` (`evictor_decisions`) | 1/1 VC discharged in 1 `.coma` file (`evictor_decisions.coma`). Mirrors the SSD-evictor comparisons in `background.rs`: start iff `util >= threshold` (`:299`), stop iff `util < low_watermark` (`:350`). Proves the direction is intended and `!(should_start && should_stop)` given a well-formed band `low_watermark <= threshold` — catches flipped `<`/`>=` and threshold/watermark swaps. **Abstraction:** runtime uses `f64` ratio `used/capacity` vs `f64` watermarks (0.9/0.8); modeled as integer permille to keep ordering decidable (`f64` NaN makes `<=` non-total, undischargeable). Certifies comparison direction, not exact float arithmetic. |

## Trusted / assumption ledger

The live proofs (P2, re-anchored P20) use no trusted lemmas. The now-`# Stale`
P21/P24 map proofs relied only on the `creusot_std::logic::FMap` ghost
primitives (`insert_ghost`, `remove_ghost`), which are `#[trusted]` in
creusot-std itself — a toolchain-level assumption, not a project-specific lemma.
No dispatch-map lemmas are imported.

Anticipated assumptions when P11 (the next keystone) lands:

- I/O / copy effects (GPU→memory copy, DMA alloc) — modeled as nondeterministic
  return values at the trusted boundary.
- `AtomicBool` / `Mutex` — collapsed to sequential ghost values.

## Modeling notes (for reviewers)

- `FMap` (finite map) from `creusot_std::logic` is confirmed usable in this
  toolchain: a feasibility probe proved empty⇒absent, insert⇒present,
  remove⇒absent (consume-once), and miss⇒no-op.
- std `HashMap::insert`/`remove` carry **no** Creusot extern specs in this
  creusot-std version (only `get`/`get_mut`/iterators do), so any map-level
  state must be modeled with a logic-level `FMap` threaded through mirror
  functions rather than by mirroring `std::HashMap` calls directly. (This was
  established via the now-`# Stale` pending-write proofs; the technique carries
  forward to future map-level properties even though that specific map is gone.)
