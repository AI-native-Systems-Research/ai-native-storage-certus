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

## Summary — what has been proved (plain English)

We are using Creusot (a deductive verifier for Rust) to mathematically prove
that the dispatcher behaves correctly on specific safety-critical paths. Because
the real dispatcher uses locks, threads, and hardware calls that Creusot can't
read, each proof works on a small faithful *model* of one method's logic.

So far, five proofs are green. Two of them prove behaviour that still matches
today's code:

- **You cannot use the dispatcher before it is initialized.** Every operational
  call first checks an "initialized" flag; if it isn't set, the call fails with
  `NotInitialized` and touches no state. Proved. *(P2)*
- **A store request with size zero is rejected cleanly.** The dispatcher rejects
  a zero-size request with `InvalidParameter` before doing any work. Proved. The
  guard used to live on `prepare_store`; it now lives on `populate`, but the
  logic we proved is identical. *(P20)*

The other three proofs are still mathematically correct, but they describe a
part of the dispatcher that **no longer exists**. They modelled the old
"staging / pending-write" workflow (`prepare_store` → `commit_store` /
`cancel_store`), and proved its key safety guarantee:

- **A staged write is finalized exactly once** — once you commit or cancel a
  pending write, a second commit-or-cancel on the same key correctly reports
  "not found". This rules out double-commit or commit-then-cancel bugs. *(P21,
  P24)*

That staging API was deleted in commit `25a7273` ("remove vestigial staging
buffer concept") — a separate refactor, **not** the P11 fix — so those three
proofs (`consume_pending`, `insert_pending`, `consume_once`) are now marked
`# Stale`: kept for history, but they no longer describe running code. The
current write path is `populate` → `reserve_memory` → `copy_gpu_to_memory_async`
→ `copy_gpu_to_memory_completed`, with `release_memory` as cancel.

**Next:** retarget onto live code. The top target is **P11** — proving that a
lookup whose stored size doesn't match the requested size *hard-fails* instead
of doing a partial copy (today `lookup_async` and `batch_lookup` silently copy
`min(stored, requested)` bytes).

---

## 1. The plan and where we are

Dispatcher-verif owns the **system-level** half of `P1..P31`; per-entry
properties stay in `dispatch-map/verif`. Ordered proof roadmap (minimal →
high-impact, dependency-aware):

| Order | Property | Description | Status |
|---|---|---|---|
| — | **P2** | operational APIs fail `NotInitialized` before init | ✅ Proved (live) |
| — | **P20** | `size==0 → InvalidParameter` (now on `populate`) | ✅ Proved (live, re-anchored) |
| — | **P21** | pending-write consume-once (prepare/commit/cancel) | ⚠️ Stale — mirrors removed API |
| — | **P24** | commit/cancel miss ⇒ `KeyNotFound`, no mutation | ⚠️ Stale — mirrors removed API |
| — | **P22** | commit ⇒ PendingWrite → BlockDevice, pending cleared | ✖️ Retired — API removed |
| — | **P23** | cancel ⇒ key absent, pending cleared | ✖️ Retired — API removed |
| 1 | **P11** | lookup size-mismatch hard-fail, no partial copy — **next keystone** | ⏳ Next |
| 2 | **P1**  | `initialize()` iff required receptacles bound | ⏳ |

Later (medium priority, dispatcher-owned): P14–P16 (eviction bound/postconds),
P19 (blind fallback), P25 (`clear_memory_tier`), P28 (drive determinism), P29
(watermark consistency).

**Progress:** 2 live proofs against current code (P2, P20). 3 proofs (P21/P24)
are green but `# Stale` — they mirror the `pending_writes` staging API deleted
by `25a7273`; P22/P23 retired (never started, no live counterpart). Retargeting
onto the current write/lookup path; P11 is next.

---

## 2. Proof log

### 2026-07-09 — retargeting: P21/P24 marked `# Stale`, P11 promoted

**No new proof.** Reconciliation against the current tree (with Codex, who
made the recent dispatcher changes).

**Finding:** commit `25a7273` (*"remove vestigial staging buffer concept"*)
deleted the `prepare_store` / `commit_store` / `cancel_store` / `pending_writes`
API from the dispatcher. Verified this is on **both** `unstable` and
`unstable-creusot` — the two branches' `dispatcher/src/lib.rs` is byte-identical
and `unstable-creusot` is 0 commits behind, so **CI sync is healthy**. (An
earlier alarm about branch drift was a false positive caused by a stale local
`origin/unstable` ref before fetch.) Codex confirmed this was a standalone
staging refactor, **not** the P11 fix.

**Impact on existing proofs:**
- **P2** (`ensure_initialized`, `:271`) — still live. `# Verified`.
- **P20** (`prepare_store_guards`) — the `size==0` guard moved from
  `prepare_store` to `populate` (`:1915`, tests `:3315`/`:3100`); logic
  identical, so still `# Verified`, re-anchored.
- **P21 / P24** (`consume_pending`, `insert_pending`, `consume_once`) — mirror
  the deleted `pending_writes` map. Green but no longer evidence for running
  code → `# Stale`. The new write lifecycle (`reserve_memory` →
  `copy_gpu_to_memory_async` → `copy_gpu_to_memory_completed`, `release_memory`
  cancel) has no pending-write map, so consume-once has no direct counterpart.
- **P22 / P23** — depended on `commit_store` / `cancel_store`; retired.

**Next target (Codex-endorsed):** **P11** — `lookup_async` (`:1784`) and
`batch_lookup` (`:1391`) currently copy `min(ipc_handle.size, size)` bytes,
allowing a partial copy on size mismatch. Prove `stored != requested ==>
Err(InvalidParameter)` with the copy branch unreachable. Ownership is at the
dispatcher (dispatch-map `lookup` is key-only).

---

### 2026-07-08 — P24, P21 (pending-write consume-once)  ⚠️ later marked `# Stale` (see 2026-07-09)

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
| P2  | `# Verified` (live) | `ensure_initialized.coma` |
| P11 | `# Unchecked` — **next** | — |
| P14 | `# Unchecked` | — |
| P15 | `# Unchecked` | — |
| P16 | `# Unchecked` | — |
| P19 | `# Unchecked` | — |
| P20 | `# Verified` (live, re-anchored to `populate`) | `prepare_store_guards.coma` |
| P21 | `# Stale` (mirrors removed API) | `insert_pending.coma`, `consume_once.coma` |
| P22 | `# Retired` (API removed) | — |
| P23 | `# Retired` (API removed) | — |
| P24 | `# Stale` (mirrors removed API) | `consume_pending.coma` |
| P25 | `# Unchecked` | — |
| P28 | `# Unchecked` | — |
| P29 | `# Unchecked` | — |

---

## 4. Synchronization notes (Claude ↔ Codex)

- **Namespace:** single canonical `P1..P31`. No alternate P-namespaces.
- **Ownership:** dispatch-map = per-entry; dispatcher = system-level. P11 is
  dispatcher-owned (dispatch-map `lookup` lacks a requested-size argument).
- **Annotation style:** `# Verified` only with a `.coma` artifact behind it AND
  the mirrored code still live; `# Stale` if the artifact is green but the code
  it mirrors was removed/reworked; `# Retired` if abandoned; else `# Unchecked`.
- **Routing:** verif crates + proof artifacts → `unstable-creusot` directly;
  modelling docs/skills outside `verif/` → `unstable` via PR.
- **Lesson learned:** each proof's mirror must be re-checked against the current
  tree at commit time — line-number anchors and whole APIs drift as the 6-person
  team lands changes on `unstable`. The P21/P24 cluster went stale within a day
  of a refactor we didn't track.
- **Handoff for next session:** the FMap-mutation idiom (`#[check(ghost)]` +
  `remove_ghost`/`insert_ghost`) is proven usable and carries forward to future
  map-level proofs, even though the pending-write map is gone. Next is **P11**
  on `lookup_async` (`:1784`) / `batch_lookup` (`:1391`): prove size mismatch
  hard-fails (`Err(InvalidParameter)`) with the `min(...)` partial-copy branch
  unreachable.

---

## 5. How to update this report

On each proof commit:
1. Re-check every proof's mirror against the current `unstable-creusot` tree
   (`git fetch` first — a stale `origin/*` ref will lie). Confirm the mirrored
   method/line still exists; downgrade any drifted proof to `# Stale`.
2. Add a dated entry under **§2 Proof log** (commit hash, properties, contract
   summary, trusted lemmas used, how it advances the plan).
3. Flip the property row(s) in **§1** and **§3** (`# Verified` / `# Stale` /
   `# Retired` / `# Unchecked`).
4. Update the in-crate `property_coverage_dispatcher_july7.md` to match.
5. Note any new trusted assumptions in the crate's trusted ledger.
