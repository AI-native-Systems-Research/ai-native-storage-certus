---
name: tools-verify-creusot-with-properties
description: Create a Creusot verification for a Certus component from BOTH its spec and its Rust code — at function granularity with spec-derived contracts — prove it, and emit a plain-English `<component>_properties.md` recording exactly what was proved (with evidence). Attempt every property you can express as a contract — do not pre-filter by lane, effort, or difficulty. Use for the normal verify-and-document workflow (not the blind property-extraction experiment).
argument-hint: "<component-name-or-path>"
---

## Goal
Verify a component with Creusot **and** leave a human-readable record of the proven properties.
Inputs are the component's **spec** (the intended behavior) and its **Rust code** (the functions to
prove). Outputs are (1) the Creusot `verif/` artifacts and (2) `<component>_properties.md` beside them.

This skill = **`tools-verify-creusot` + explicit spec pairing + a documented-properties step.** Use
`tools-verify-creusot` for the create/proof mechanics (pure-core extraction, `verif/` crate,
`#[requires]`/`#[ensures]`, drift/equality check, fault-injection validation). This skill adds the
spec-derived-contract sourcing and the plain-English output.

## Steps
1. **Resolve inputs — consume the inventory, do NOT re-extract.** Resolve to `components/<name>/` and
   load the **property inventory** `verif/<name>_property_inventory.md` (Role 1,
   `build-property-inventory`): the agreed, id-keyed, tool-independent property set. Also open the Rust
   functions to prove (`src/**`); use the spec only to read the wording of an obligation, never to mint a
   new one. **Do not build a private per-tool property list.** If the inventory is missing, stop and run
   `build-property-inventory` first. If you spot a real obligation the inventory lacks, **flag it back**
   to the inventory so it gets an `id` there — never keep a private property.
2. **Prove the shared/global invariants FIRST, once — then reuse.** Before any per-method work, take the
   inventory properties whose bundle spans multiple methods (the inventory marks these as global /
   maintained invariants and gives each an attachment count). **Order them by attachment count, highest
   first** — the one in the most bundles has the most leverage. Prove each as a **single maintained
   invariant** (a type `#[invariant]` or a `logic` predicate): establish it at construction/`initialize`,
   and prove each mutator **preserves** it (`#[requires] inv(self)` → `#[ensures] inv(result)`). Record it
   under its **one** inventory `id` — its `proved` status then propagates to every bundle it belongs to
   automatically (Role 3 attaches by id; you do NOT re-prove it per method). A cheap invariant in many
   bundles (e.g. an init-gate or an accounting invariant) unblocks all those methods in one proof — never
   skip it because "no single method owns it," and never re-assert it vacuously in each method.
3. **Then, for each remaining per-method obligation, express it as a Creusot contract bound to the CODE.**
   Take the obligation verbatim from its inventory row (`id`, `statement`, `kind`, `object_fn`) and render
   it as `#[requires]`/`#[ensures]` on the real function, **citing the already-proved invariants as known
   facts** rather than re-proving them. Where the crate can't build under Creusot, use a **faithful
   whole-function mirror** in `verif/` with the **same** contract, guarded by a drift/equality check.
   Function granularity — never a lifted statement.
4. **Prove.** `cargo creusot` to green; iterate. Record every `#[trusted]` boundary and assumption.
5. **Validate (anti-vacuity).** Fault-inject each function (a contract-violating change) and confirm the
   proof goes **red**; if it stays green, the contract is vacuous — strengthen it. Then revert. For a
   maintained invariant, also fault-inject a mutator that *breaks* it and confirm the preservation VC fails.
6. **Document → `components/<name>/verif/<name>_properties.md`** (see shape below): for each proven
   property, the operation, the property in **plain English**, its **spec source**, and the **evidence**.
   Be honest — a green proof of a *mirror* only covers the mirror; say so, and list trusted boundaries.
   Every property that was *not* proved is reported under one of the two buckets in the shape below —
   with a concrete signature, never a bare verdict.

## Coverage discipline (mandatory)
**Attempt every property in the inventory that you can express as a contract. You do not get to pick
lanes here** — routing properties across tools is a separate step, not this one. Your job is to prove or
fail *trying*, and to record the outcome **keyed to the inventory `id`** so Role 3 can aggregate it.
- **No pre-filtering by lane, effort, or budget.** "Better suited to Kani", "intractable", "needs
  induction", "to stay within budget" are **not** acceptable reasons to omit a property. If you can state
  the contract, you must attempt the proof.
- **Attack the highest-leverage properties first.** A property in many bundles is not lower priority for
  being "shared infrastructure" — it is *higher* priority, because one proof discharges it everywhere. Prove
  the shared/global invariants (ordered by attachment count) before spending effort on method-specific
  obligations; a cheap invariant left at `not_yet` silently keeps every method in its bundle unproved.
- **Exhaust Creusot's capabilities before declaring a limit.** Bit-level properties → `#[bitwise_proof]`
  (reasons over `&`/`|`/`<<`/`>>` and `Vec<u64>` words — a bitmap round-trip is *in scope*, prove it).
  Maps/collections → logic-level `FMap`/`Seq` models. Nonlinear/division facts → `proof_assert!` bridges.
  Do not defer a bit-vector or container obligation as "out of reach" without first trying these.
- **Bounded effort, not infinite grind.** Give each proof a real attempt against a stated cap (e.g. one
  full portfolio pass, or a per-VC solver timeout). If it exceeds the cap, record the concrete outcome and
  move on — do not silently drop it.
- **Every non-success carries a reproducible signature**, never a subjective verdict: the failing VC, a
  solver timeout (`>N min, portfolio alt-ergo/z3/cvc5`), or a specific SMT model gap (e.g. `leading_zeros`
  modelled only relationally, so `leading_zeros ↔ log2` won't discharge). Ban bare "intractable"/"out of scope".
- **Only two legitimate non-proofs**, each needing a one-line technical reason and a suggested route:
  (a) **Not expressible** in Creusot's sequential logic at all — concurrency/interleaving, liveness/timing;
  (b) **Depends on effects Creusot cannot model** — real I/O, hardware, FFI byte layout.
  Everything else must be attempted and reported with its signature under bucket (a) of the shape.

## Definition of done — "not attempted" is not an outcome (mandatory)
Every inventory property must reach **exactly one** of three end states. There is no fourth box.
1. **Proved** — a green proof (native), or a disclosed faithful whole-function mirror / `#[trusted]` boundary, named.
2. **Delegated** — the obligation is owned by a **different component** across a real interface boundary; name it and route it. "Belongs to another *tool*" is **not** delegation — that is still your property to attempt here.
3. **Tool boundary hit** — you **wrote the contract, ran the proof, and captured a reproducible failure signature** (failing VC / solver timeout `>N min portfolio` / specific SMT model gap). Evidence is mandatory; a limit asserted *without a proof attempt* is not a legitimate boundary.

**"Contract not written" / "authorable but not done" is NOT an end state.** A property with no contract is *unfinished work*, never a rating. Do not report it, do not park it, do not hand back the run with it open. The only exit from the not-done set is an actual attempt, which forces the property to (1) or (3). On uncertainty the default is **attempt**, never "tool limitation." A run returned with unattempted properties has not met the bar, no matter how many were proved.

## Clean-slate re-run protocol (mandatory for re-runs)
A re-run must not inherit credit from a prior run's artifacts.
- **Start from a stripped, isolated tree.** New worktree; **reduce every contract to a bare signature** (drop `#[ensures]`, `#[requires(true)]`) / empty the `verif/` mirror, so every contract is re-authored from the inventory. You may not point at pre-existing contracts to claim coverage.
- **Do NOT delete the previous verified run.** It stays on its committed `verif/creusot/<component>` branch as the baseline and as evidence — deletion destroys expensive proofs (`.coma`/VCs) and the ability to catch regressions.
- **Provenance rule:** a property counts as proved **only** if its contract was authored and its VCs are discharged **in this run's tree**. A prior-run pass does not count until reproduced here.
- **Diff against the baseline** and report three sets: re-proved, regressed, and prior-"proved" that did **not** reproduce (phantom coverage). That diff is the audit.
- Capture **wall-clock + peak RSS** for every proof in the fresh run; report proofs as VCs/goals discharged + `.coma` count, never bare "files".

## `<component>_properties.md` shape
```
# Verified properties — <component> (Creusot)
Proven from spec `specs/<...>/spec.md` against code `src/<...>`. Artifacts: `verif/`.

## <operation>
- `<inventory-id>` **[Postcondition]** <property in plain English>. — spec FR-nnn / US-n — proved: `<fn>.coma` (N/N VCs)
- **[Precondition]** ...
- **[Invariant]** ...

## Assumptions / trusted boundaries
- <#[trusted] item / mirror / environment assumption> — why trusted, and what it therefore does NOT prove.

## Not proved — attempted, tool boundary hit
- <property> — **attempted** with <capability tried: `#[bitwise_proof]` / `FMap` / `proof_assert!`>,
  blocked by <concrete signature: failing VC / solver timeout >N min / SMT model gap>.

## Not proved — not expressible in Creusot (shape)
- <property> — one-line reason: concurrency/interleaving, liveness/timing, or I/O/hardware/FFI layout.
  Route: <Loom / Spin / fault-injection test>.
```

## Notes
- Construction + documentation skill — **distinct** from the read-only audits
  `component-check-spec-translation` / `component-check-verif-translation`, from the source-agnostic
  extraction primitive `extract-verifiable-properties`, and from the inventory builder
  `build-property-inventory` (Role 1) whose output this skill **consumes** by `id`.
- Every proved / not-proved line **cites its inventory `id`**, so `tools-aggregate-coverage-by-interface`
  (Role 3) can attach the status to the right bundle without re-reconciling.
- Routing: Creusot verifs → `unstable-creusot`.
