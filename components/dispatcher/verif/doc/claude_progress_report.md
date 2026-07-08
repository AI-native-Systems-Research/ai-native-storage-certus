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
| 2 | **P24** | commit/cancel miss ⇒ `KeyNotFound`, no mutation | ⏳ Next |
| 3 | **P21** | pending-write consume-once (prepare/commit/cancel) — *keystone* | ⏳ |
| 4 | **P22** | commit ⇒ PendingWrite → BlockDevice, pending cleared | ⏳ |
| 5 | **P23** | cancel ⇒ key absent, pending cleared | ⏳ |
| 6 | **P11** | lookup size-mismatch hard-fail, no partial copy | ⏳ |
| 7 | **P1**  | `initialize()` iff required receptacles bound | ⏳ |

Later (medium priority, dispatcher-owned): P14–P16 (eviction bound/postconds),
P19 (blind fallback), P25 (`clear_memory_tier`), P28 (drive determinism), P29
(watermark consistency).

**Progress:** 2 / 8 high-priority dispatcher properties proved (P2, P20).

---

## 2. Proof log

### 2026-07-07 — commit `da38e77` — P2, P20 (crate bootstrap)

**What was proved** (`cargo creusot` → Proved, 2 files):

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
| P21 | `# Unchecked` | — |
| P22 | `# Unchecked` | — |
| P23 | `# Unchecked` | — |
| P24 | `# Unchecked` | — |
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
- **Handoff for next session:** P24 → P21 need one FMap-*mutation* idiom probe
  (ghost-threaded `remove_ghost` mirror) before the consume-once proof; there
  is no in-repo precedent yet (dispatch-map/verif is all per-entry).

---

## 5. How to update this report

On each proof commit:
1. Add a dated entry under **§2 Proof log** (commit hash, properties, contract
   summary, trusted lemmas used, how it advances the plan).
2. Flip the property row(s) in **§1** and **§3** to ✅ / `# Verified`.
3. Update the in-crate `property_coverage_dispatcher_july7.md` to match.
4. Note any new trusted assumptions in the crate's trusted ledger.
