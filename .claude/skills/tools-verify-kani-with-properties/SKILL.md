---
name: tools-verify-kani-with-properties
description: Create Kani harnesses for a Certus component from BOTH its spec and its Rust code — at function granularity, calling the real function under spec-derived pre/postconditions — run them, and emit a plain-English `PROPERTIES.md` of what was verified (with evidence). Use for the normal verify-and-document workflow (not the blind property-extraction experiment).
argument-hint: "[component-path] [interface-path]"
---

## Goal
Verify a component with Kani **and** leave a human-readable record of the verified properties.
Inputs are the component's **spec** (the intended behavior) and its **Rust code** (functions +
interface). Outputs are (1) the `#[cfg(kani)] mod verification` harnesses and (2)
`PROPERTIES.md` in the component directory.

This skill = **`tools-verify-kani` + explicit spec pairing + a documented-properties step.** Use
`tools-verify-kani` for the create mechanics (stub unsafe/FFI, `kani::assume` mirroring production
guards, run/fix, its "core rule"). This skill adds spec-derived contracts and the plain-English output.

## Steps
1. **Resolve inputs (both required).** Read the component's `specs/**/spec.md` (intended contracts)
   **and** the Rust functions + interface (`src/**`, `interfaces/`).
2. **Per function, derive the contract from the SPEC, harness the CODE.** Function granularity:
   `kani::assume(<spec precondition — mirror the production guard>)` → **call the real function** →
   `assert!(<spec postcondition>)`. Never lift a statement into the harness. Label the source requirement.
3. **Run.** `cargo kani` to green; fix gaps; **audit** that each `assume` matches a real production
   guard (no over-assuming that would make the proof vacuous).
4. **Validate (anti-vacuity).** Fault-inject (a contract-violating change) and confirm the harness
   **FAILS**; if it still passes, it isn't bound to the real code — fix it. Then revert.
   (`--harness` substring-matches; use `--exact` with `module::name` to run one in isolation.)
5. **Document → `components/<name>/PROPERTIES.md`** (see shape below): for each verified
   property, the operation, the property in **plain English**, its **spec source**, and the **evidence**
   (harness name; Kani result). State the **bounded scope**: Kani proves over the full input domain
   within `#[kani::unwind(N)]`, symbolically (not by sampling).

## `PROPERTIES.md` shape
```
# Verified properties — <component> (Kani)
Verified from spec `specs/<...>/spec.md` against code `src/<...>`. Harnesses: `#[cfg(kani)] mod verification`.

## <operation>
- **[Postcondition]** <property in plain English>. — spec FR-nnn / US-n — harness `verify_<...>` (SUCCESSFUL, K checks)
- **[Precondition]** ...
- **[Invariant]** ...

## Assumptions / bounds
- `kani::assume(<...>)` — the production guard it mirrors.
- stubbed FFI / `#[kani::unwind(N)]` bounds — and what that leaves unproven.

## Attempted but not proven
- <property> — reason (e.g. unbounded loop/heap, requires induction → Creusot territory).
```

## Notes
- Construction + documentation skill — **distinct** from the read-only audits
  `component-check-spec-translation` / `component-check-verif-translation`, and from the source-agnostic
  extraction primitive `extract-verifiable-properties`.
- Routing: Kani harnesses → `unstable-kani`.
