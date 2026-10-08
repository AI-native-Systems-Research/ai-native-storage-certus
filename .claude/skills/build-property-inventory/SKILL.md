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
- `tools-verify-{creusot,kani}-with-properties` (Role 2) consume this inventory and produce id-named proof
  artifacts; the shipped **scorer** (not the agent, not this skill) writes `status` per id by reproducing each proof.
- `tools-aggregate-coverage-by-interface` (Role 3) renders the bundle view + scoreboard from inventory + scorer-written statuses.

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
Deliberately **absent**: `lane` and `status`. Status is written **downstream by the scorer** — Role 2 produces
the proof artifacts, the scorer reproduces them and writes status. If you find yourself wanting to write
"Creusot proves this" here — stop; that is not this skill's job, and no agent writes status at all.

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
1. the component **spec** — `components/<name>/specs/**/spec.md` → `verif/spec_properties.yaml`
2. the component **code** — `components/<name>/src/**` (+ the `I<name>` interface) → `verif/code_properties.yaml`

Each blind pass emits **one YAML file** (FROM_SPEC schema — see `extract-verifiable-properties`), written
directly to its deliverable name: **`spec_properties.yaml`** and **`code_properties.yaml`**. No `.md` is
produced. They carry the two blind denominators `spec_total` and `code_total`, and every property's
`statement` is full plain English (non-specialist-readable) per the extraction skill's rule.

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
Write **three YAML files** under `components/<name>/verif/` — **no `.md`**:

**(1) `spec_properties.yaml`** and **(2) `code_properties.yaml`** — the two blind extraction outputs (from
step 2), carrying the two blind denominators. Do not re-derive these; they are the FROM_SPEC-schema YAML
the two `extract-verifiable-properties` passes already wrote.

**(3) `unified_properties.yaml`** — the **reconciled single source of truth** that Roles 2 and 3 consume,
in the extended schema documented in `component-verify` (superset of the inventory record + FROM_SPEC
fields + **null-placeholder** per-tool `creusot:`/`kani:` blocks). Per property carry
`id/subject/methods/object_fn/kind/verifiable/global/attachments/origin/source/statement/traces`; emit
each tool block as a pure placeholder — **`{evidence: null, fidelity: null, note: null}` with NO `status`
key at all**. Do **not** write `status: not-attempted` (or any status): under the reproduction gate the
scorer is the only writer of `status`, and absence-of-`status` is exactly how Role 3 detects a property
that has not been scored yet (renders as pending `·`, not as a failure). Requirements:
- **`statement` in full plain English** (non-specialist-readable — carry the extraction rule through; do
  not compress back to symbols during reconciliation).
- **Bundle & global data inline:** `methods` (the bundle), `global: true` iff |methods|>1, `attachments`
  (= |methods|), so Role 2 can order the prove-once worklist by `attachments` descending and Role 3 can
  size bundles — all by counting, never eyeballing.
- **Not-verifiable ledger** as `NV-*` records (`verifiable: false`, `reason` in plain English, no tool blocks).
- **Reconciliation captured in-file**, not in a dropped `.md`: each property's `origin`
  (spec+code | spec-only | code-only | divergent) plus a top-level `reconciliation:` block listing the
  spec-only / code-only / divergent / refinement-gap flags and the implementing-obligation map (code
  internals collapsed up to the observable property they serve).
- **`counts:`** with `methods (N) / spec_total / code_total / unified (M) / attachments / not_verifiable`.
- **Integrity rule:** record count == `counts.unified` + `counts.not_verifiable`; `verifiable: true`
  count == `counts.unified`.

**Uniformity (fixes em/mt gaps):** every verifiable record MUST carry a per-property `kind`, and the
**NV ledger is mandatory** for every component (empty list only if genuinely none — state that explicitly).
Do not fall back to the older section-`group:`/no-NV format.

`unified_properties.yaml` is the input to Roles 2 and 3. The HTML is rendered later, not here.

## Honesty rules (non-negotiable)
- **Tool-independent.** No prover named, no `proved`/`not_yet` written. If a property is hard for *some*
  tool, that is irrelevant here — it still belongs in the inventory.
- **M is discovered, not chosen.** Do not target a round number. Whatever the reconciled walk yields is M.
- **Every id carries a source pin** (spec FR/US/AS/SC and/or code fn+line). No source → not a property yet.
- **Divergence is surfaced, never smoothed.** Spec-only, code-only, and wording deltas are findings.
- Under-claim coverage; a bundle you are unsure is complete is flagged incomplete, not padded.

## Procedure
1. List the `I<component>` public methods → fixes N and the empty bundles. Read them FROM THE TRAIT in
   `components/interfaces/src/` and write them to the bundle as `interface_methods:`. Properties may also
   attach to other entry points (Drop, Default, inherent helpers), but the method headline counts only the
   trait's methods: memory-tier attached properties to 22 names against a 17-method trait.
2. Run `extract-verifiable-properties` on spec and on code as two BLIND passes (each sees only its own
   artifact) → `spec_properties.yaml` and `code_properties.yaml`. Do NOT use prior verif artifacts / an
   older inventory as a source.
3. Reconcile by obligation; assign stable ids; set `origin`; flag divergences (into the `reconciliation:`
   block of `unified_properties.yaml`).
4. Assign each id its `methods` (subject + every dependent method); compute bundle sizes by counting.
5. Build the three ledgers (method / spec / code coverage); confirm nothing is unaccounted.
6. Write the three YAML outputs (`spec_properties.yaml`, `code_properties.yaml`, `unified_properties.yaml`).
   Report: "N methods; M distinct properties; A attachments; spec_total/code_total; bundle sizes […];
   K reconciliation flags; NV ledger of size Q."

## Reconciling at scale — do it in two stages, with a mechanical orphan check (measured)

Reconciliation is the step where a property can silently vanish, and on a real component it is large:
`eviction-policy-session-lists` reconciled 59 spec + 88 code records into M=95. Doing it as one
in-flight job over the two full YAMLs failed twice to infrastructure timeouts, losing everything both
times because nothing was written until the end. Only **3 of the 59/88 ids coincided by chance**, so
pairing must be by MEANING — never by id, and never with difflib or any fuzzy matcher.

**Stage 1 — decisions only, written incrementally.** Digest both extractions into one compact file
(`id | subject | methods | kind` + a truncated statement; this halved the input, 113 KB → 57 KB) and
emit a **JSON Lines pairing table**, one line per reconciled obligation:
`{id, subject, kind, methods, origin, spec_id, code_id, statement, divergence_note}`.
Append after each subject group (one method at a time, then the globals, then the NV ledgers), so a
failure costs one group and a successor resumes from where the file stops.

**Stage 2 — assemble `unified_properties.yaml` deterministically** from the two source YAMLs plus that
table, with a script rather than by hand. Then the completeness check becomes **exact** instead of
depending on an agent being careful:

- every spec id and every code id appears in **exactly one** `paired_from` — report orphans, ids used
  twice, and ids that do not exist in either source, and FAIL on any of them;
- `reconciliation.paired + spec_only + code_only + divergent == counts.unified`;
- no record carries a non-empty `creusot:`/`kani:` block or any `status`.

Measured outcome doing it this way: 0 orphans, 0 double-use, 0 ghosts across 147 input ids, and the
assembly step cannot misplace a property because it never makes a judgement.

## LEVEL 1 — the spec<->code sync check (mandatory, after reconciliation; 2026-10-04)

Reconciliation produces the unified records. **Level 1 then CHECKS them**, read-only, before anything
reaches a prover. It is not enough to trust the origin labels: on extent-manager the reconciler produced
**0 divergent** records — the disagreements had been merged into "agreed" ones — and the provers then
rediscovered them as 75 refutations, 28 of them one input-range mismatch. Three checks, in order:

**1. Merge check.** For every `origin: spec+code` record, read its two sides (`paired_from`). If the
merge had to NARROW, BROADEN or REWORD either side's claim to make them agree — a scope narrowed to some
callers, "at most" vs "fewer than", one side naming an error the other does not — the record is
`origin: divergent`, with both readings in `divergence_note`. A `delta_note` is only for a difference of
wording with identical meaning. *If you would need to explain the difference to a reviewer, it is divergent.*

**2. Input-range check, both directions.** List every constraint the SPECIFICATION places on inputs,
configuration and state (validation lists such as "format() MUST validate …", allowed values, ranges,
maxima, alignment, power-of-two) with its pointer. Independently list every assumption the CODE makes
about them — a mask `x & !(n-1)` assumes a power of two, a fixed buffer assumes a maximum, a division
assumes non-zero, an `as u32` assumes a range — with file:line. Compare:
- the spec constrains a value and the code never enforces it (dpm: sector size 512 or 4096, unchecked), or
- the code assumes something the spec neither requires nor validates (extent-manager: `buddy.rs:136`
  assumes a power-of-two sector size; FR-002 only requires `sector_size > 0`).
Each mismatch is ONE `domain_discordances:` entry (top level of the bundle). On the pages it is called a
**code assumption** (Cornel, 2026-10-07): an assumption the code makes that the specification does not state.
It is NOT a fault - the spec may deliberately leave room - and finding it is the value. Its `assume` is the
plain-English assumption ("a pool never allocates more than 2^32 - 1 slots"); the page marks it a **range**
(numeric limit) or a **condition** (setup/state requirement: not null, connected, no duplicate key) from
`assume_rust`, or from an explicit `assumption_kind:` field. Only a counterexample INSIDE the assumption is a defect.
A proof must never assume one silently: EPO's September proofs did, and fresh proofs exposed it.
```yaml
domain_discordances:
  - spec_says: format() must validate only that the sector size is greater than zero (FR-002)
    code_does: the buddy allocator assumes the sector size is a power of two (buddy.rs:136)
    spec_pointers: [FR-002]
    code_pointers: [buddy.rs:136, lib.rs:<where format takes the value>]
    methods: [format, recover, ...]
    assume: the sector size is a power of two       # plain English: the narrower range both sides meet
    assume_rust: "sector_size.is_power_of_two()"    # how a proof states it, on the entry input
```

**3. Filtering.** `gate/level1.py` then writes `discordances.yaml` and excludes from level 2 every record
that IS a discordance (divergent, spec-only, from either side). Records that merely DEPEND on a
domain discordance are kept, and level 2 proves them **under its `assume`** — stated on the page as
"verified for <assume>" — so one mismatch is reported once, at level 1, instead of being rediscovered as
dozens of refutations. A level-2 refutation that disappears under a level-1 assumption was never a level-2
finding.

## Verification scope — what goes to the provers (corrected 2026-10-04)

**Only `origin: spec+code` and `origin: code-only` records are verification obligations.** They are
what the component is meant to do, according to both sides or according to the code's own documented
intent. **`divergent` and `spec-only` records are kept in the YAML as extraction data — both readings,
with sources — and are NOT sent to the provers, NOT scored and NOT shown on the page.** Whether the spec
or the code should change is spec<->code synchronisation, which belongs to whoever owns that, not to
formal verification.

**Why this was corrected — measured, and it is the most important lesson in this skill.** From
2026-09-28 this section said "`origin: divergent` is the highest-value output … 8 became machine-proved
defects", and the full union went to the provers. Within days the same components we had verified for
weeks with no defects produced tens of "refuted" properties: **43 of 46 across four components came from
divergent, spec-only or hazard-worded records.** A divergent record states one side's version of
behaviour the other side does differently; "refuting" it only re-discovers a disagreement the
extraction had already found. It is not a verification result, and it buried the one real defect in
pages nobody could read. Restricted to the scope above, the same proofs give: disk-partition-manager
0 refuted (4/4 methods), eviction-policy-session-lists 0 (9/9), memory-tier 3 (8/17; 5/17 on Sept 22).

A **defect is pronounced only after verification**, never by extraction: an in-scope obligation that the
tools refute, triaged to a root cause by a human or the repair agent. It reaches the page once fixed.

Never drop a record to make the two lists agree: M may exceed either input, and code-only records are
real guarantees the specification never mentions. Keeping a record and verifying it are different things.

## Anti-patterns
- ❌ Eyeballing a bundle size instead of counting ids. (Defect #4.)
- ❌ A property with no `source` pin. (Defect #1.)
- ❌ Extracting from spec+code but not walking the whole spec, so FRs go uncovered. (Defect #2.)
- ❌ Letting "what a prover can do" shape which properties exist. (Defect #3 — contamination.)
- ❌ Dropping a spec-only/code-only property because it is inconvenient.
- ❌ Sending a `divergent` or `spec-only` record to the provers. It can only re-discover the
  disagreement already recorded; 43 of 46 published "refutations" were exactly this (2026-10-04).
- ❌ Writing a record as a HAZARD ("a new entry's bytes can still hold the previous occupant's data").
  Every record states what the code SHOULD do ("a new entry starts cleared"); a proof then means the
  requirement holds. memory-tier had 22 hazard records, for which "proved" meant "the bug is real".
- ❌ A strict relation where the code keeps a non-strict one ("always smaller" for `<=`). It refutes on
  wording alone (memory-tier MT-INV-SIZE-ACCOUNTING).
- ❌ Re-extracting differently inside Role 2 or Role 3. Extraction happens once, here.

## Routing
General methodology → `unstable` (promote via PR; do not leave siloed on a verif branch).
When run under the `component-verify` orchestrator, the orchestrator owns all git: `unified_properties.yaml`
rides the per-component `verif/<tool>/<name>` branches with the proof code, and all three
(`spec_properties.yaml` / `code_properties.yaml` / `unified_properties.yaml`) ship as deliverables under
`formal-verification/<name>/`.
