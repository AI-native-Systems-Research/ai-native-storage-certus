---
name: build-property-inventory
description: Build the ONE tool-independent, source-traced verifiable-property inventory for a component and bundle every property onto the I<component> public methods that implement it. Reconciles two independent, BLIND per-artifact extractions (spec and code, isolated from each other) into a single ID-keyed set with owning-method bundles and counts. Does NOT name a prover and does NOT record proof status — that is added downstream. This is Role 1 (the single source of truth) that `tools-verify-*-with-properties` and `tools-aggregate-coverage-by-interface` consume.
argument-hint: "<component-name>"
---

## Purpose
Produce the **single source of truth** for a component's verifiable properties, before any prover is
named. Output is one inventory in which every property is (a) **traced** to spec + code, (b) given a
**stable id**, and (c) **bundled onto the `I<component>` public method(s)** it helps implement. Bundle
sizes then fall out *by counting* — never eyeballed.

This is **tool-independent** (Role 1). It does NOT choose a lane, run a prover, or record proved/not_yet.
- `tools-verify-{creusot,kani}-with-properties` (Role 2) consume this inventory and write `status` per id.
- `tools-aggregate-coverage-by-interface` (Role 3) renders the bundle view + scoreboard from inventory + statuses.

## The unit and the granularity (do not change)
- **Aggregation unit = the public method** of the `I<component>` trait (the `fn` lines in
  `define_interface!`, excluding `Display::fmt` and any `#[cfg(test)]`). This fixes the denominator **N**.
- **One property = one obligation per (subject × kind)** — one precondition, OR one postcondition, OR one
  error-case, OR one frame condition, OR one invariant. Not a bundle, not a code fragment, not a solver VC.
- Reuse the definition and the five tests (atomic / subject-bound / observable-falsifiable / decidable /
  source-anchored) from `extract-verifiable-properties`. **This skill composes that primitive; it does not
  restate the property definition.**

## Property record (canonical schema — what Roles 2 & 3 consume)
```
- id:        MT-INSERT-DEDUP           # stable, component-prefixed, semantic; never renumbered on reorder
  subject:   insert                    # the ONE public method OR named global invariant it is bound to
  methods:   [insert]                  # EVERY public method whose bundle includes it (⊇ {subject}).
                                       #   a shared helper's property attaches to many — that is expected & fine
  object_fn: MemoryTier::insert -> HashMap::insert   # the concrete function(s) that implement it
  kind:      precondition | postcondition | error-case | frame | invariant
  statement: "duplicate key -> AlreadyExists, no state change"
  source:    [spec: FR-009] [code: allocator.rs:142]   # BOTH when present; at least one REQUIRED
  origin:    spec+code | spec-only | code-only | divergent
  global:    false                     # true iff |methods| > 1 (a maintained/shared invariant); see below
  attachments: 1                       # = |methods|; the property's leverage. REQUIRED, computed by counting
```
Deliberately **absent**: `lane` and `status`. Those are added by Role 2 / Role 3. If you find yourself
wanting to write "Creusot proves this" here — stop; that is not this skill's job.

**`global` and `attachments` are the leverage signal Role 2 relies on.** Set `global: true` when a property
is a maintained/shared invariant sitting in more than one bundle, and always emit `attachments` (= |methods|).
Role 2 (`tools-verify-*-with-properties`) proves globals **once, ordered by `attachments` descending**, so the
highest-leverage invariant is discharged first and its status propagates to every bundle by `id`. An
un-flagged or un-counted global is the exact defect that leaves a cheap high-fan-out invariant (e.g. an
init-gate in 16 bundles) stuck at `not_yet` while it silently blocks every method that contains it.

### Bundle rule
- A property's `methods` list is every public method whose correctness **depends on** its `object_fn`.
- A property on a **shared internal helper** (e.g. `align_up`) attaches to **every** dependent public
  method. This is intended; the user has confirmed "if a property is needed by many methods, so be it."
- **Two counts, both reported:** *distinct properties* = number of ids; *attachments* = Σ|methods|.
- **Bundle size of method m** = number of ids whose `methods` contains m. This is a **count**, never a guess.

## Inputs — two BLIND extractions, then reconcile
Run `extract-verifiable-properties` **once per artifact, blind** — each extraction sees ONLY its own
artifact, never the other's output and never any prior inventory. Independence is the point: agreement
between two lists derived in isolation is real corroboration, and it prevents anchoring (reading code first
silently shrinks the spec list to the shape of what happens to be implemented).
1. the component **spec** — `components/<name>/specs/**/spec.md` → `props_from_spec.md`
2. the component **code** — `components/<name>/src/**` (+ the `I<name>` interface) → `props_from_code.md`

**Only spec and code are extraction sources.** Do **not** feed prior harnesses / verif artifacts / an older
inventory into extraction: they carry stale, renamed, or dead properties and let "what a prover already did"
decide which properties exist (Defect #3, tool-contamination). If a refinement-gap check is wanted, run it
**after** the inventory is built, as a one-way cross-check that can only *flag* a missing obligation for
spec/code confirmation — never as a source that seeds ids.

### How the two lists merge (this is NOT an intersection and NOT a raw sum)
The merge is **dedup-by-obligation-identity + granularity-normalization** — a *union of distinct
observable obligations*, deduped by what each asserts (subject × kind × statement), not by which file it
came from. Two mechanisms:
- **(a) Match by obligation, keep divergence.** present in spec **and** code → one id, `origin: spec+code`;
  spec-only (required, maybe unimplemented) or code-only (undocumented behavior) → **keep, mark origin,
  flag** — never drop, never silently merge; same obligation worded differently → one id, note the delta.
- **(b) Collapse implementation-internal obligations up to the observable property they serve.** Code
  extraction surfaces machinery (e.g. `ALLOC-USED-INC`, `DEALLOC-COALESCE-PREV`) — several such internals
  collapse into ONE observable invariant (→ `USED-CONSERVE`). Record them in the **implementing-obligation
  map**, NOT as extra ids and NOT as bundle members.
Consequence: M can exceed the spec count (code surfaces real observable properties the spec omitted — frame
conditions, absent-key returns, error cases) and sits far below the raw code count (internals collapse).
**M is whatever the reconciled walk yields — discovered, not chosen.**
**Never invent a property to round out a bundle. Never delete one to improve a number.**

## Coverage by construction (nothing unaccounted)
- **Method ledger:** every one of the N public methods appears with a bundle (possibly of size 0 — a
  genuinely empty bundle is a finding worth stating, not a blank).
- **Spec ledger:** every FR / user story / acceptance scenario / edge case maps to ≥1 property id OR is
  listed under *Not verifiable* with a reason (reuse the primitive's *Not verifiable* list).
- **Code ledger:** every public fn, error-return branch, and state transition maps to ≥1 id or a reason.

## Required output
`components/<name>/verif/<name>_property_inventory.md` (the canonical source of truth), containing:
1. **Header:** the N public methods listed (the denominator), and the date/commit of spec + code read.
2. **Property table:** one row per id, full schema above.
3. **Bundle rollup:** per public method — its bundle size and the ids in it (mark shared ids).
4. **Global-invariant ledger:** every `global: true` property, **sorted by `attachments` descending**, with
   its methods — the prioritized prove-once worklist Role 2 consumes.
5. **Counts:** distinct properties (M), attachments (Σ|methods|), and bundle size per method.
6. **Reconciliation notes:** every spec-only / code-only / divergent / refinement-gap flag.
7. **Coverage ledgers:** method / spec / code, per above.
This `.md` is the input to Roles 2 and 3. The HTML views are rendered later, not here.

## Honesty rules (non-negotiable)
- **Tool-independent.** No prover named, no `proved`/`not_yet` written. If a property is hard for *some*
  tool, that is irrelevant here — it still belongs in the inventory.
- **M is discovered, not chosen.** Do not target a round number. Whatever the reconciled walk yields is M.
- **Every id carries a source pin** (spec FR/US/AS/SC and/or code fn+line). No source → not a property yet.
- **Divergence is surfaced, never smoothed.** Spec-only, code-only, and wording deltas are findings.
- Under-claim coverage; a bundle you are unsure is complete is flagged incomplete, not padded.

## Procedure
1. List the `I<component>` public methods → fixes N and the empty bundles.
2. Run `extract-verifiable-properties` on spec and on code as two BLIND passes (each sees only its own
   artifact) → separate files. Do NOT use prior verif artifacts / an older inventory as a source.
3. Reconcile by obligation; assign stable ids; set `origin`; flag divergences.
4. Assign each id its `methods` (subject + every dependent method); compute bundle sizes by counting.
5. Build the three ledgers; confirm nothing is unaccounted.
6. Write `<name>_property_inventory.md`. Report: "N methods; M distinct properties; A attachments;
   bundle sizes […]; K reconciliation flags."

## Anti-patterns
- ❌ Eyeballing a bundle size instead of counting ids. (Defect #4.)
- ❌ A property with no `source` pin. (Defect #1.)
- ❌ Extracting from spec+code but not walking the whole spec, so FRs go uncovered. (Defect #2.)
- ❌ Letting "what a prover can do" shape which properties exist. (Defect #3 — contamination.)
- ❌ Dropping a spec-only/code-only property because it is inconvenient.
- ❌ Re-extracting differently inside Role 2 or Role 3. Extraction happens once, here.

## Routing
General methodology → `unstable` (promote via PR; do not leave siloed on a verif branch).
Generated `<name>_property_inventory.md` → PR to `unstable`.
