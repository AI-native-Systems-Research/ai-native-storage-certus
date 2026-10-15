# Alignment Tasks — memory-tier

**Regenerated**: 2026-08-20 (Spec-Sync Phase B)
**Policy**: `.specify/sync/PHASE_B_POLICY.md`

## ALIGN tasks (code violates a correct spec): **none**

Phase B resolved the memory-tier drift by **backfilling the spec to the working implementation**
(the flagship "backfill to reality" case). None of the drift items is a code bug against an agreed,
correct spec, so **no ALIGN tasks are generated** and **no source code is to be changed** for them.

The five tasks previously deferred here (2026-07-22, re-affirmed in the 2026-08-07 sweep) are now
**superseded**:

| Former align-task | Phase B disposition |
|-------------------|---------------------|
| `sharding-not-implemented` (FR-005/006/007, NFR-002, FR-013, FR-021) | **BACKFILL applied** — spec rewritten to the single `RwLock<Pool>` design. Not a code change. |
| `evict-lru-for-key-ignores-key` (FR-014) | **BACKFILL applied** — spec now documents `evict_next_for_key` as a global-eviction alias whose `key` is ignored (correct for an unsharded pool). |
| `creusot-proofs-absent` (SC-8) | **BACKFILL applied** — SC-8 and all `Creusot P#` "Verified" annotations removed from spec. Interface-doc overclaiming was already removed on the main thread. |
| `version-mismatch` (NFR-008) | **HUMAN_DECISION** — see below. |
| `readme-source-layout-drift` | Out of spec-sync scope (doc-only, `README.md` not under `specs/**`); see note below. |

---

## HUMAN_DECISION — NFR-008 component version (not an ALIGN task)

**Not a code-vs-correct-spec bug**, so it is not an ALIGN task; recorded here for a maintainer.

Three version strings disagree and none is authoritative:

- `components/memory-tier/Cargo.toml:3` — package `version = "0.1.0"`
- `components/memory-tier/src/lib.rs:140` — `define_component!` macro `version: "0.3.0"`
- `spec.md` NFR-008 — `0.2.0`

**Decision needed**: pick one real version and reconcile all three. Drivers: whether a
`0.1.0 → 0.2.0/0.3.0` release actually happened (check git tags / release history), and whether the
now-backfilled single-pool design warrants a version label change. Once chosen, update `Cargo.toml`,
the `define_component!` macro, and `spec.md` NFR-008 to match. `Cargo.toml` and `src/lib.rs` are
outside the spec-sync edit scope, so this pass left NFR-008's spec text unchanged.

## Follow-up (doc-only, outside spec-sync scope) — README source layout

`components/memory-tier/README.md` reportedly still describes a nonexistent `lru.rs`/`LruList`
module; eviction is delegated to the `IEvictionPolicy` receptacle (FR-024). This is a plain doc edit
outside `specs/**` and was not touched by this pass. `tasks.md` already tracks it.
