---
name: extract-verifiable-properties
description: Extract verifiable correctness properties from ONE component artifact — a spec, the source code, or a verification harness/verif — at a fixed, comparable granularity, with coverage by construction.
argument-hint: "<artifact files> <output file>"
---

## Purpose
From **one** artifact (a spec, OR the code, OR a harness/verif) produce the list of **verifiable
properties** it implies, at a **fixed granularity** so lists from different artifacts compare
row-by-row. Read only the named artifact; ground everything in it.

## What a verifiable property is (the unit)
> A **single, falsifiable, machine-checkable assertion about the component's observable behavior or
> state**, bound to **one subject**, expressed as **one specification-level obligation** — one
> precondition, one postcondition, or one invariant (NOT the solver VCs it expands into) — and
> **anchored to its source**.

All five: (1) **atomic** — one obligation, not a bundle, not a code fragment; (2) **subject-bound** —
one public operation OR one named global invariant; (3) **observable & falsifiable** — a counterexample
must be *possible*; reject tautologies, type facts, and any precondition so strong it admits no inputs
(vacuous); (4) **decidable** by a verifier; (5) **source-anchored** — spec: FR/US/AS/SC; code: `fn`+line;
harness: the assertion.

## Granularity — fixed
**One property = one obligation per (subject × kind).** Subjects = {each public operation} ∪ {each named
global invariant}. Bundle clauses of the same obligation; split distinct ones. (Count is not the goal;
coverage is.)

## Completeness — coverage by construction (do this; don't rely on noticing)
1. **Coverage ledger.** Walk **every unit of the source artifact**; each maps to ≥1 property OR is listed
   under *Not verifiable* with a reason — **nothing unaccounted**. Units by artifact:
   - **spec** → each FR, user story, acceptance scenario, edge case, clarification
   - **code** → each public fn, each branch/error-return, each field, each state transition
   - **harness** → each assertion / `#[ensures]` / `#[requires]` / goal
2. **Per-operation rubric.** For each operation ask the fixed five: (a) precondition to call it;
   (b) success postcondition; (c) **each** error case + its trigger; (d) **frame** — what must NOT change;
   (e) invariants it must preserve.
3. **Invariant sweep.** Walk **every data field** (its legal range/relation) and **every state +
   transition** (which are legal) to derive global invariants.

## Output (same shape for every artifact)
- **Coverage ledger** (source unit → property IDs / "not verifiable: reason").
- **Property table**, one row per operation:
  `Operation | Precondition(s) | Postcondition(s) | Error cases | Frame (unchanged) | Invariants touched | Source`
- **Global invariants** list (add one only if the artifact genuinely implies it).
- **Not verifiable** section.

## Not verifiable (list here; don't force into rows)
Unbounded liveness / deadlock-freedom; wall-clock **timing** (a timeout's *occurrence* is verifiable, its
*duration* is not); caller/environment **assumptions** (I/O, allocation, pointer validity); pure
layout/size facts unless they gate behavior; logging (except a negative "does not log on error path");
non-deterministic / relaxed-ordering quantities.

## Method
1. **Discover** — read the artifact; note every candidate property *and* anything surprising
   (extra/undocumented behavior, missing guard, contradiction).
2. **Cover** — build the ledger, apply the rubric per operation, run the invariant sweep.
3. **Normalize** — one obligation per cell; drop non-falsifiable ones to *Not verifiable*; keep surprises
   as their own rows/notes.
4. **Write** to the requested output file, with sources. Read no files other than the named artifact.

## Harness note
A property = *what an assertion / `#[ensures]` actually checks*. A harness property with no counterpart in
spec or code = the harness verifies something unintended (a refinement gap) — flag it.
