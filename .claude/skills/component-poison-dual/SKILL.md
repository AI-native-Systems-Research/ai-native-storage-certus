---
name: component-poison-dual
description: Deliberately introduce a controlled number of misalignments into BOTH sides of a single Certus component on a throwaway branch — some seeded as spec.md/code mismatches (concrete side) and some as code/verif mismatches (abstract side) — so a red-team run can exercise `component-check-spec-translation` and `component-check-verif-translation` at once. The two poison sets are independent (no coupling): each is meant to be caught by its own audit. Use only for evaluation of the audit skills, never on a branch intended to merge.
argument-hint: "[component-name] [poison-count | spec-count+verif-count]"
---

# Poison a Component on Both Sides (spec↔code and code↔verif)

This skill combines the two single-sided poisoners into one pass. It seeds **independent**
misalignments into a component so that a single throwaway branch exercises *both* audit skills:

- **Concrete side** — spec.md ↔ implementation mismatches, exactly as `component-poison` seeds
  them. Detected by `component-check-spec-translation`.
- **Abstract side** — implementation ↔ verification-artifact mismatches (Creusot mirror, Kani
  harness, or Spin model), exactly as `component-poison-verif` seeds them. Detected by
  `component-check-verif-translation`.

The two poison sets are **not coupled**: a spec↔code poison and a code↔verif poison target
different claims, and each is meant to be caught by its own audit. (If you instead want a single
coordinated defect that hides *because* code and verif agree with each other while both violate
the spec, that is a different — stealth — design; this skill deliberately keeps the sides
independent so each detection path is exercised cleanly.)

This is a destructive, adversarial skill. It only ever operates on a dedicated
`poison-dual-<component-name>` branch that is **not** meant to be merged. Do not run it on
`unstable`, a feature branch you intend to ship, or any branch with uncommitted work you care
about.

The component name is `$0` and the poison budget is `$1`.

## 1. Resolve parameters and both artifact families

1. If `$0` (component name) was not supplied, interactively ask the user which component to
   poison and stop until provided.
2. Resolve `$0` to exactly one component directory under `components/` (the lower-case,
   hyphen-ized form, or the directory whose `define_component!` block declares it). If it does
   not exist or is ambiguous, return an error and stop.
3. Discover **both** kinds of target and record what exists:
   - **Specs** (concrete side): `rg --files <component>` → files ending in `/spec.md` (support
     `specs/<feature>/spec.md`, `.specify/specs/<feature>/spec.md`, and deeper nesting).
   - **Verification artifacts** (abstract side): a co-located Creusot `verif/` crate (or any
     `src/**` using `creusot_std` / `#[requires]` / `#[ensures]` / `pearlite!`), a
     `#[cfg(kani)] mod verification` block, and/or a Spin model at `modelling/spin/<name>/`.
4. This skill needs **both** a `spec.md` and at least one verification artifact. If only one
   exists, stop and point the user at the matching single-sided skill: specs-only →
   `component-poison`; verif-only → `component-poison-verif`. Do not silently degrade to one
   side.
5. Resolve the poison budget `$1`:
   - Bare integer (e.g. `4`): split it across the two sides, as evenly as possible, with at
     least one poison per side (for `1`, ask which side to target).
   - Explicit split `S+V` (e.g. `3+2` = 3 spec↔code, 2 code↔verif): honor it exactly.
   - If `$1` was not supplied, ask for it and stop. Require positive integers; reject `0`/
     negatives. If either sub-count exceeds the distinct claims that side exposes, warn and
     offer to cap.
   Record the resolved split as `S` (spec↔code) and `V` (code↔verif); the total applied must
   equal `S + V`.

## 2. Create and check out the poison branch

1. Confirm the working tree is clean enough to proceed. If uncommitted changes would be swept
   onto the new branch, warn and ask whether to continue.
2. Record the current branch as the base branch.
3. Create a **new local branch derived from the current branch**, named
   `poison-dual-<component-name>`, and check it out — e.g.
   `git checkout -b poison-dual-dispatch-map`. If it already exists, ask whether to delete and
   recreate or append a numeric suffix. Do not push to any remote.

## 3. Seed the concrete-side poisons (spec ↔ code) — apply `S`

Build a short inventory of the component's spec claims (requirements/IDs, interfaces,
parameters, data types, defaults, constants, errors, semantics, exclusions) and apply exactly
`S` distinct changes that mis-align `spec.md` and the code. Spread them across claims and
across these categories (from `component-poison`):

- **Remove functionality** — delete a function/branch/validation the spec requires (→ *Missing*).
- **Add unspecified functionality** — behavior the spec never mentions (→ *Extra behavior*).
- **Change syntax / parameters** — alter a signature, arg order/count, or name (→ *Contradiction*).
- **Change a data type** — field/return/param type mismatching the spec (`u32` vs `u64`,
  `Option<T>` vs `T`).
- **Change semantics** — keep the signature, change behavior: invert a condition, change a
  default/units/rounding, weaken an error into silent success, change ordering.
- **Over-specify the spec** — add a requirement/scenario/default to `spec.md` the code does not
  implement (poisons the spec side).

Keep each poison a *semantic* mismatch (not cosmetic), small, self-contained, and targeting
distinct requirement IDs where possible. Note the exact `path:line` on both the spec and code
sides.

## 4. Seed the abstract-side poisons (code ↔ verif) — apply `V`

Build a short inventory of what the verification artifacts claim (mirrored function bodies,
each `#[requires]`/`#[ensures]`/loop `#[invariant]`, each `kani::assume`/`assert`, each Promela
property and correspondence row). Apply exactly `V` distinct changes that desync concrete code
from its verif artifact. Choose across these categories (from `component-poison-verif`), and
mark each **loud** (VC red / counterexample) or **silent** (green proof, hollow meaning):

- **Creusot** — mirror-body drift (shipped ≠ mirror, silent); vacuous contract (`#[ensures(true)]`,
  silent); precondition/postcondition mismatch; type/structure drift in the mirror.
- **Kani** — unmatched assume (no production guard, silent); harness detached from the real
  function (silent); stub divergence; wrong/trivial postcondition; coverage gap.
- **Spin** — protocol drift (code changes, `.pml` not updated, silent until sync); model
  weakening (removed/weakened property); correspondence rot (stale line ranges/names).

Include at least one **silent** poison when `V >= 2` — those test whether the drift/equality
check, assume-audit, and sync-divergence report actually work. Note the exact `path:line` on
both the concrete and abstract sides, and name the detection mechanism you expect to catch each.

Keep the two poison sets **independent**: do not let a code change made for a spec↔code poison
also happen to satisfy or break a verif contract. If a concrete-side edit would incidentally
move a verif artifact, either choose a different concrete target or pick a verif poison that
targets a genuinely unrelated claim, so each side's audit sees a clean, attributable signal.

## 5. Keep the code compiling and the artifacts runnable

After all poisons are applied, two things must hold:

1. **The shipped code still compiles** — `cargo build -p <crate-name>`.
2. **The verification artifacts still parse/typecheck**, so each audit *runs* rather than dying
   on a syntax error: Creusot `cargo creusot --only coma` in the mirror crate; Kani confirm the
   `#[cfg(kani)]` module compiles under `cfg(kani)` (or `cargo kani --only-codegen` if
   available); Spin `spin -a` (do **not** run the full model check — that is the detection step).

If a component is SPDK-gated and cannot be built here, build with its default configuration and
note in the report that a full `--workspace`/hardware build was not verified. If a poison cannot
be made to compile/parse without also removing the misalignment, back it out and choose a
different one so the applied count still equals `S + V` (or, if impossible, stop and report how
many were successfully applied on each side).

Do **not** run formatters/linters that would reconcile a mismatch, and do **not** run
`component-sync-specs`, `tools-verify-*`, `tools-spin-sync`, or any sync/repair skill — running
those *is the test* and would defeat the purpose.

## 6. Summarize the poisons

Write a `POISON-DUAL.md` file in the component directory with a single combined answer key. Tag
each row's `Side` as `spec↔code` or `code↔verif` and name the audit expected to catch it:

```markdown
# Dual Poison Report

Component: <component-dir>
Verification forms present: <Creusot verif/ | Kani mod verification | Spin model>
Base branch: <base branch>
Poison branch: poison-dual-<component-name>
Budget: <$1>   Applied: <N>   (spec↔code: <S>, code↔verif: <V>; loud: <x>, silent: <y>)
Concrete build verified: <cargo build -p <crate> result / caveat>
Artifacts parse-checked: <creusot --only coma / kani codegen / spin -a result / caveat>

> Generated by the `component-poison-dual` skill to test the spec↔code and code↔verif audits
> together. This branch is intentionally broken and must not be merged.

| # | Side | Form | Category | Poisoned | Abstract/Spec claim (path:line) | Concrete location (path:line) | What was changed | Loud/Silent | Catching audit | Expected result |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | spec↔code | spec.md | Change data type | code | specs/001-.../spec.md:88 | src/lib.rs:142 | Return `u64`→`u32` | — | component-check-spec-translation | Contradiction (High) |
| 2 | spec↔code | spec.md | Over-specify | spec | specs/001-.../spec.md:120 (added) | (none) | Added FR not implemented | — | component-check-spec-translation | Missing (Medium) |
| 3 | code↔verif | Creusot | Vacuous contract | abstract | verif/src/lib.rs:182 | src/lib.rs:184 | `#[ensures(...)]`→`#[ensures(true)]` | Silent | component-check-verif-translation | Vacuous (must go red) |
| 4 | code↔verif | Kani | Unmatched assume | abstract | src/lib.rs:220 (harness) | src/lib.rs:96 (bare `+= 1`) | added `assume(x<MAX)` no guard | Silent | component-check-verif-translation | Unsound assumption |
```

Map "Expected result" to the vocabulary each audit uses: spec↔code → *Aligned / Partial /
Contradiction / Missing / Extra behavior / Unverifiable*; code↔verif → *Faithful / Drift /
Vacuous / Contradiction / Unsound assumption / Missing coverage / Stale correspondence /
Unverifiable*.

## 7. Report

Print a concise summary: the poison branch name, the base branch, the spec↔code / code↔verif
split (and loud/silent split), whether the concrete build passed, whether the artifacts still
parse, and the path to `POISON-DUAL.md`. Remind the user this branch is intentionally broken and
must not be merged, and that a full evaluation runs **both** audits against the component:

- `component-check-spec-translation` should catch the `S` spec↔code poisons.
- `component-check-verif-translation` should catch the `V` code↔verif poisons (watch the silent
  ones — VCs that should go red but stay green, unmatched assumes, drifted mirrors).

Cross-contamination is itself a finding: if the spec↔code audit flags a verif-side change (or
vice versa), the two poison sets were not as independent as intended — note it.

Do not commit or push unless the user explicitly asks.
