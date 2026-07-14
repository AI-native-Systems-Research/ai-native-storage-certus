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

Only properties this crate owns are listed. Per-entry properties (P3–P10,
P12, P13, P17, P18, P26, P27, P30, P31) stay in `dispatch-map/verif`.

| Property | Owner | Status here | Next proof target |
|---|---|---|---|
| P1  — `initialize()` iff required receptacles bound | dispatcher | `# Verified` (`initialize_dependency_guards`) | 1/1 VC discharged in 1 `.coma` file (`initialize_dependency_guards.coma`). Mirrors the `initialize` guard prefix (`lib.rs:1053-1065`): `dispatch_map` unbound → `NotInitialized`; then `memory_tier` unbound → `NotInitialized`; then empty `data_pci_addrs` → `InvalidParameter`; both bound + non-empty → `Ok`. Check order preserved so the error variant matches live code. Pairs with P2. |
| P2  — operational APIs fail `NotInitialized` pre-init | dispatcher | `# Verified` (`ensure_initialized`) | — |
| P11 — lookup size-mismatch hard-fail, no partial copy | dispatcher | `# Verified (product path)` (`resolve_lookup`) | 3/3 VCs discharged in 1 `.coma` file (`resolve_lookup.coma`). Models the `LookupResult → Result` match in `lookup_async` (`:1786-1830`) / `batch_lookup` (`:1420-1442`). Proves the reachable product path: `MemoryTier`/`BlockDevice` hit ⇒ `Ok` with `n ≤ requested ∧ n ≤ stored` (copy clamped to `min`, so **no partial/over-copy**); `NotExist ⇒ KeyNotFound`, no copy. The `MismatchSize ⇒ InvalidParameter` (no copy) arm is also proved but is **defensive, intentionally-unreachable code**: production `dispatch-map::lookup` is key-only (`dispatch-map/src/lib.rs:115`) and never emits `MismatchSize`; only the `MockDispatchMap` test injects it (`dispatcher/src/lib.rs:2601`). Safety rests on the invariant *requested size == stored size* at lookup — **TODO: trace `ipc_handle.size` vs stored size to confirm** (until then an assumption, not a proved fact). |
| P14 — eviction attempt bound (`MAX_EVICT_ATTEMPTS`) | dispatcher | `# Unchecked` | Bounded-loop proof over `evict_for_space` (variant `512 - attempts`). |
| P15 — eviction success ⇒ `used + needed <= cap` | dispatcher | `# Unchecked` | Loop postcondition on `evict_for_space`. |
| P16 — eviction failure ⇒ capacity not achieved | dispatcher | `# Unchecked` | Loop postcondition on `evict_for_space`. |
| P19 — blind eviction fallback removes key | dispatcher | `# Unchecked` | Depends on dispatch-map transition lemmas. |
| P20 — `size==0 → InvalidParameter` (now on `populate`) | dispatcher | `# Verified` (`prepare_store_guards`, re-anchored to `populate`) | — |
| P21 — pending-write consume-once (prepare/commit/cancel) | dispatcher | `# Stale` (`insert_pending`, `consume_once`) | Mirrors deleted `pending_writes` map. Retire, or reconceive against `reserve_memory` → `copy_gpu_to_memory_completed` / `release_memory` lifecycle. |
| P22 — commit ⇒ PendingWrite → BlockDevice, pending cleared | dispatcher | `# Retired` | Depended on removed `commit_store` / `pending_writes`. No live counterpart. |
| P23 — cancel ⇒ key absent, pending cleared | dispatcher | `# Retired` | Depended on removed `cancel_store` / `pending_writes`. No live counterpart. |
| P24 — commit/cancel miss ⇒ `KeyNotFound`, no mutation | dispatcher | `# Stale` (`consume_pending`) | Mirrors deleted `pending_writes` map. Retire, or reconceive against `release_memory` idempotent-cancel semantics. |
| P25 — `clear_memory_tier` map postcondition + count | dispatcher | `# Unchecked` | Map-level loop proof. |
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
