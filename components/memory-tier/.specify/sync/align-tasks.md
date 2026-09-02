# Alignment Tasks — memory-tier

**Regenerated**: 2026-09-02 (spec-sync re-run)

## ALIGN tasks (code violates a correct spec): **none**

`spec.md` is aligned with `src/` (single `RwLock<Pool>` design). No code bug
against a correct spec exists, so **no ALIGN tasks are generated** and **no
source code is changed**.

This pass extended the earlier spec.md backfill to the two supporting spec
artifacts that were still stale:

| Artifact | Disposition |
|----------|-------------|
| `plan.md` | **BACKFILL applied** — sharded architecture + Creusot P1–P10 sections rewritten to the single-`RwLock<Pool>` reality. Not a code change. |
| `tasks.md` | **BACKFILL applied** — Creusot/shard-targeting tasks removed; shard-layout / configurable-shard-count items reworded. Not a code change. |

---

## HUMAN_DECISION — NFR-008 component version (not an ALIGN task)

**Not a code-vs-correct-spec bug**, so it is not an ALIGN task; recorded here for
a maintainer. This is the item that keeps the drift report stamped `drift`.

Three version strings disagree and none is authoritative:

- `components/memory-tier/Cargo.toml:3` — package `version = "0.1.0"`
- `components/memory-tier/src/lib.rs:140` — `define_component!` macro `version: "0.3.0"`
- `spec.md` NFR-008 — `0.2.0`

**Decision needed**: pick one real version and reconcile all three. Once chosen,
update `Cargo.toml`, the `define_component!` macro, and `spec.md` NFR-008 to
match. `Cargo.toml` and `src/lib.rs` are outside the spec-sync edit scope, so
this pass left NFR-008's spec text unchanged.

## Follow-up (out of spec-sync scope)

- **Interface doc comment** — `components/interfaces/src/imemory_tier.rs:87-91`
  still describes `evict_next_for_key` as evicting "from the same shard as `key`"
  and returning `None` if "the target shard is empty". The pool is unsharded and
  `key` is ignored. `components/interfaces/**` is outside this component's edit
  scope; needs a cross-cutting interface fix.
- **README source layout** — `components/memory-tier/README.md:23-30` still lists
  a nonexistent `src/lru.rs`; eviction is delegated to `IEvictionPolicy`. Doc-only,
  not under `specs/**`; tracked in `tasks.md` Documentation.
