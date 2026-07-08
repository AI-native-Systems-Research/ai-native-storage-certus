# Dispatcher Verif — Property Coverage (July 7)

Scope:

- Verification target: `components/dispatcher/verif`
- Property baseline: spec-derived `P1..P31` (canonical namespace) — see
  `components/dispatch-map/verif/property_coverage_matrix_codex_july2.md`.
- This crate owns **dispatcher / system-level** properties. Per-entry
  properties remain owned by `components/dispatch-map/verif`.

Annotation buckets (consistent with the interface-annotation style):

- `# Verified` — a Creusot proof artifact exists and `cargo creusot` is green.
- `# Unchecked` — targeted but not yet proved (or only partially modeled).

## Proof artifacts in this crate

| Artifact (`verif/dispatcher_verif_rlib/*.coma`) | Property | Mirrors (dispatcher/src/lib.rs) | Status |
|---|---|---|---|
| `ensure_initialized` | **P2** — operational APIs fail `NotInitialized` before init | `ensure_initialized()?` prefix (`:2131`, `:2228`, `:2276`, `:2298`) | `# Verified` |
| `prepare_store_guards` | **P20** — `prepare_store(size==0) → InvalidParameter`, no mutation | `prepare_store` guard prefix (`:2130-2136`) | `# Verified` |

`cargo creusot` → **Proved (2 files)**.

Note on "no mutation": both guards return before any `dispatch_map` /
`pending_writes` access, so state-preservation is structural on these paths
(there is no state touched before the early return). Full map-level
no-mutation clauses are added in the P21–P24 work once the pending-write
`FMap` carrier is wired.

## Dispatcher-owned properties — status against `P1..P31`

Only properties this crate owns are listed. Per-entry properties (P3–P10,
P12, P13, P17, P18, P26, P27, P30, P31) stay in `dispatch-map/verif`.

| Property | Owner | Status here | Next proof target |
|---|---|---|---|
| P1  — `initialize()` iff required receptacles bound | dispatcher | `# Unchecked` | Model receptacles as `Option<Handle>`; prove `Ok <==> (dispatch_map & memory_tier bound)`. |
| P2  — operational APIs fail `NotInitialized` pre-init | dispatcher | `# Verified` (`ensure_initialized`) | — |
| P11 — lookup size-mismatch hard-fail, no partial copy | dispatcher | `# Unchecked` | Ownership fixed at dispatcher (dispatch-map `lookup` has no requested-size arg). Prove `stored != requested ==> Err(InvalidParameter)` + copy branch unreachable. |
| P14 — eviction attempt bound (`MAX_EVICT_ATTEMPTS`) | dispatcher | `# Unchecked` | Bounded-loop proof over `evict_for_space` (variant `512 - attempts`). |
| P15 — eviction success ⇒ `used + needed <= cap` | dispatcher | `# Unchecked` | Loop postcondition on `evict_for_space`. |
| P16 — eviction failure ⇒ capacity not achieved | dispatcher | `# Unchecked` | Loop postcondition on `evict_for_space`. |
| P19 — blind eviction fallback removes key | dispatcher | `# Unchecked` | Depends on dispatch-map transition lemmas. |
| P20 — `prepare_store(size==0) → InvalidParameter` | dispatcher | `# Verified` (`prepare_store_guards`) | — |
| P21 — pending-write consume-once (prepare/commit/cancel) | dispatcher | `# Unchecked` | `FMap<CacheKey, PendingModel>` ghost; `remove_ghost` mirror. **Keystone.** |
| P22 — commit ⇒ PendingWrite → BlockDevice, pending cleared | dispatcher | `# Unchecked` | Builds on P21 + trusted dispatch-map `convert_to_storage` lemma. |
| P23 — cancel ⇒ key absent, pending cleared | dispatcher | `# Unchecked` | Builds on P21 + trusted dispatch-map `remove` lemma. |
| P24 — commit/cancel miss ⇒ `KeyNotFound`, no mutation | dispatcher | `# Unchecked` | Direct from `FMap::remove` on absent key (feasibility-proved). |
| P25 — `clear_memory_tier` map postcondition + count | dispatcher | `# Unchecked` | Map-level loop proof. |
| P28 — drive-index determinism (`key % num_drives`) | dispatcher | `# Unchecked` | Pure arithmetic contract on `drive_index`. |
| P29 — watermark/threshold consistency | dispatcher | `# Unchecked` | Config-comparison direction proof. |

## Trusted / assumption ledger

Currently **empty** — the two proved artifacts use no trusted lemmas or
external assumptions. Assumptions will be recorded here as P21–P24 land:

- dispatch-map per-entry postconditions imported as lemmas
  (`convert_to_storage`, `remove`, `create_staging`) — proved in
  `dispatch-map/verif`, reused here as assumptions.
- I/O effects (`reserve_extent`, `publish`, SSD write, DMA alloc) — modeled
  as nondeterministic return values.
- `AtomicBool` / `Mutex` — collapsed to sequential ghost values.

## Modeling notes (for reviewers)

- `FMap` (finite map) from `creusot_std::logic` is confirmed usable in this
  toolchain: a feasibility probe proved empty⇒absent, insert⇒present,
  remove⇒absent (consume-once), and miss⇒no-op.
- std `HashMap::insert`/`remove` carry **no** Creusot extern specs in this
  creusot-std version (only `get`/`get_mut`/iterators do), so the pending-write
  map is modeled with a logic-level `FMap` threaded through the mirror
  functions rather than by mirroring `std::HashMap` calls directly.
