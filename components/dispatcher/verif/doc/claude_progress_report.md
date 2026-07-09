# Claude Progress Report — Dispatcher Creusot Verification

**Living document.** Updated with each proof / set of proofs at commit time, so
Claude's and Codex's work stay synchronized against the shared `P1..P31`
baseline.

- Verification target: `components/dispatcher/verif/` (branch `unstable-creusot`)
- Property baseline: canonical `P1..P31` (see
  `components/dispatch-map/verif/property_coverage_matrix_codex_july2.md`)
- Companion doc (in-crate): `property_coverage_dispatcher_july7.md`
- Routing rule: `verif/` content is committed **directly to `unstable-creusot`**
  (not via PR to `unstable`).

---

## 1. The plan and where we are

Dispatcher-verif owns the **system-level** half of `P1..P31`; per-entry
properties stay in `dispatch-map/verif`. Ordered proof roadmap (minimal →
high-impact, dependency-aware):

| Order | Property | Description | Status |
|---|---|---|---|
| 1 | **P20** | `prepare_store(size==0) → InvalidParameter`, no mutation | ✅ Proved |
| — | **P2** | operational APIs fail `NotInitialized` before init | ✅ Proved |
| 2 | **P24** | commit/cancel miss ⇒ `KeyNotFound`, no mutation | ✅ Proved |
| 3 | **P21** | pending-write consume-once (prepare/commit/cancel) — *keystone* | ✅ Proved |
| 4 | **P22** | commit ⇒ PendingWrite → BlockDevice, pending cleared | ⏳ Next |
| 5 | **P23** | cancel ⇒ key absent, pending cleared | ⏳ |
| 6 | **P11** | lookup size-mismatch hard-fail, no partial copy | ⏳ |
| 7 | **P1**  | `initialize()` iff required receptacles bound | ⏳ |

Later (medium priority, dispatcher-owned): P14–P16 (eviction bound/postconds),
P19 (blind fallback), P25 (`clear_memory_tier`), P28 (drive determinism), P29
(watermark consistency).

**Progress:** 4 / 8 high-priority dispatcher properties proved (P2, P20, P24, P21).

---

## 2. Proof log

### 2026-07-08 — P24, P21 (pending-write consume-once)

**What was proved** (`cargo creusot` → all proofs green, 5 functions):

- **P24** — `consume_pending(map: &mut FMap<u64, PendingModel>, key)`. Mirrors the
  consume step shared by `commit_store` (`:2236`) and `cancel_store` (`:2283`):
  `.remove(&key).ok_or(KeyNotFound)?`. Contract: `Ok ⇒` key contained before and
  absent after; `Err(KeyNotFound) ⇒` key absent before and map unchanged
  (`(^map).ext_eq(*map)`). The `_ => false` arm pins the error variant so the
  miss branch is exactly `KeyNotFound`.
- **P21** — two artifacts:
  - `insert_pending` (prepare side) mirrors `pending_writes…insert(key, …)`
    (`:2213`); contract: `(^map).contains(key)` after insert.
  - `consume_once` (keystone) — given `(*map).contains(key)`, the first consume
    returns `Ok` and the second returns `Err(KeyNotFound)`. This is the
    consume-exactly-once guarantee: a pending write cannot be committed then also
    cancelled (or committed twice).

**How it covers the plan:** closes the P21/P24 cluster — the pending-write
lifecycle carrier. `consume_once` is the keystone the P22/P23 outcome proofs
build on (they extend it with the dispatch-map transition lemma).

**FMap-mutation idiom (probe result):** the `#[check(ghost)]` + `remove_ghost` /
`insert_ghost` pattern verifies cleanly. This was the missing idiom flagged in
the previous handoff (dispatch-map/verif is all per-entry, non-ghost) — now
established as the in-repo precedent for map-level proofs.

**Trusted lemmas / assumptions:** none project-specific. Relies only on the
`#[trusted]` `FMap` ghost primitives in creusot-std (toolchain-level).

**One-line fix during the proof:** `consume_once` first failed 2/3 because
`consume_pending`'s `Err(_)` arm erased the variant; pinning it to
`Err(KeyNotFound)` with a `_ => false` closer made all 5 files green.

---

### 2026-07-07 — commit `da38e77` — P2, P20 (crate bootstrap)

**What was proved** (`cargo creusot` → all proofs green, 2 functions):

- **P2** — `ensure_initialized(initialized: bool)`. Mirrors
  `self.ensure_initialized()?` at the top of every operational API
  (dispatcher/src/lib.rs `:2131`, `:2228`, `:2276`, `:2298`).
  Contract: `!initialized ==> Err(NotInitialized)`; `initialized ==> Ok`.
- **P20** — `prepare_store_guards(initialized, size)`. Mirrors the guard
  prefix of `prepare_store` (`:2130–2136`):
  `ensure_initialized()?` then `if size == 0 { return InvalidParameter }`.
  Contract covers the NotInitialized path, the size==0 path, and the
  size>0 pass-through.

**How it covers the plan:** closes the two cheapest system-level guards and
bootstraps the crate (manifest, `why3find.json`, proof-artifact tracking)
so the P21–P24 cluster can build on a green baseline.

**No-mutation note:** both guards return before any `dispatch_map` /
`pending_writes` access, so state-preservation is structural on these paths.
Full map-level no-mutation clauses arrive with the pending-write `FMap`
carrier in P21–P24.

**Trusted lemmas / assumptions:** none.

**Enabling findings (from feasibility work this session):**
- `creusot_std::logic::FMap` is usable in this toolchain — probe proved
  empty⇒absent, insert⇒present, remove⇒absent (consume-once), miss⇒no-op.
- std `HashMap::insert`/`remove` have **no** Creusot extern specs here (only
  get/get_mut/iterators), so the pending-write map must be modeled with a
  logic-level `FMap`, not by mirroring `std::HashMap` calls.

---

## 3. Coverage snapshot (dispatcher-owned properties)

| Property | Status | Artifact |
|---|---|---|
| P1  | `# Unchecked` | — |
| P2  | `# Verified`  | `ensure_initialized.coma` |
| P11 | `# Unchecked` | — |
| P14 | `# Unchecked` | — |
| P15 | `# Unchecked` | — |
| P16 | `# Unchecked` | — |
| P19 | `# Unchecked` | — |
| P20 | `# Verified`  | `prepare_store_guards.coma` |
| P21 | `# Verified`  | `insert_pending.coma`, `consume_once.coma` |
| P22 | `# Unchecked` | — |
| P23 | `# Unchecked` | — |
| P24 | `# Verified`  | `consume_pending.coma` |
| P25 | `# Unchecked` | — |
| P28 | `# Unchecked` | — |
| P29 | `# Unchecked` | — |

---

## 4. Synchronization notes (Claude ↔ Codex)

- **Namespace:** single canonical `P1..P31`. No alternate P-namespaces.
- **Ownership:** dispatch-map = per-entry; dispatcher = system-level. P11 is
  dispatcher-owned (dispatch-map `lookup` lacks a requested-size argument).
- **Annotation style:** `# Verified` only with a `.coma` artifact behind it;
  otherwise `# Unchecked`.
- **Routing:** verif crates + proof artifacts → `unstable-creusot` directly;
  modelling docs/skills outside `verif/` → `unstable` via PR.
- **Handoff for next session:** FMap-mutation idiom is now established
  (`#[check(ghost)]` + `remove_ghost`/`insert_ghost`, proved in P24/P21) — it is
  the in-repo precedent for map-level proofs. Next is P22 (commit ⇒ PendingWrite →
  BlockDevice, pending cleared): extend `consume_once`'s Ok branch with the
  trusted dispatch-map `convert_to_storage` lemma, then P23 (cancel) with the
  `remove` lemma.

---

## 5. How to update this report

On each proof commit:
1. Add a dated entry under **§2 Proof log** (commit hash, properties, contract
   summary, trusted lemmas used, how it advances the plan).
2. Flip the property row(s) in **§1** and **§3** to ✅ / `# Verified`.
3. Update the in-crate `property_coverage_dispatcher_july7.md` to match.
4. Note any new trusted assumptions in the crate's trusted ledger.
