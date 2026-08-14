---
name: tools-verify-creusot-with-properties
description: Create a Creusot verification for a Certus component from BOTH its spec and its Rust code — at function granularity with spec-derived contracts — prove it, and emit a plain-English `verified_properties.md` recording exactly what was proved (with evidence). Use for the normal verify-and-document workflow (not the blind property-extraction experiment).
argument-hint: "<component-name-or-path>"
---

## Goal
Verify a component with Creusot **and** leave a human-readable record of the proven properties.
Inputs are the component's **spec** (the intended behavior) and its **Rust code** (the functions to
prove). Outputs are (1) the Creusot `verif/` artifacts and (2) `verified_properties.md` beside them.

This skill = **`tools-verify-creusot` + explicit spec pairing + a documented-properties step.** Use
`tools-verify-creusot` for the create/proof mechanics (pure-core extraction, `verif/` crate,
`#[requires]`/`#[ensures]`, drift/equality check, fault-injection validation). This skill adds the
spec-derived-contract sourcing and the plain-English output.

## Steps
1. **Resolve inputs (both required).** Resolve to `components/<name>/`. Read its `specs/**/spec.md`
   (FRs, user stories, acceptance scenarios → the *intended* contracts) **and** the Rust functions to
   verify (`src/**`).
2. **Derive each contract from the SPEC, bind it to the CODE.** For every target function, take its
   precondition / postcondition / invariant from the **spec**; verify the **code** against it. Prefer
   proving the real function; where the crate can't build under Creusot, use a **faithful whole-function
   mirror** in `verif/` with the **same** contract, guarded by a drift/equality check. One property =
   one obligation, at **function granularity** (never a lifted statement).
3. **Prove.** `cargo creusot` to green; iterate. Record every `#[trusted]` boundary and assumption.
4. **Validate (anti-vacuity).** Fault-inject each function (a contract-violating change) and confirm the
   proof goes **red**; if it stays green, the contract is vacuous — strengthen it. Then revert.
5. **Document → `components/<name>/verif/verified_properties.md`** (see shape below): for each proven
   property, the operation, the property in **plain English**, its **spec source**, and the **evidence**.
   Be honest — a green proof of a *mirror* only covers the mirror; say so, and list trusted boundaries.

## `verified_properties.md` shape
```
# Verified properties — <component> (Creusot)
Proven from spec `specs/<...>/spec.md` against code `src/<...>`. Artifacts: `verif/`.

## <operation>
- **[Postcondition]** <property in plain English>. — spec FR-nnn / US-n — proved: `<fn>.coma` (N/N VCs)
- **[Precondition]** ...
- **[Invariant]** ...

## Assumptions / trusted boundaries
- <#[trusted] item / mirror / environment assumption> — why trusted, and what it therefore does NOT prove.

## Attempted but not proven
- <property> — reason (e.g. needs quantifier, unbounded structure, out of Creusot scope).
```

## Notes
- Construction + documentation skill — **distinct** from the read-only audits
  `component-check-spec-translation` / `component-check-verif-translation`, and from the source-agnostic
  extraction primitive `extract-verifiable-properties`.
- Routing: Creusot verifs → `unstable-creusot`.
