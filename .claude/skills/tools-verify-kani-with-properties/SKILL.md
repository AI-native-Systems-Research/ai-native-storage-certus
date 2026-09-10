---
name: tools-verify-kani-with-properties
description: Create Kani harnesses for a Certus component from BOTH its spec and its Rust code — at function granularity, calling the real function under spec-derived pre/postconditions — run them, and emit a plain-English `<component>_properties.md` of what was verified (with evidence). Attempt every property you can express as a harness — do not pre-filter by lane, effort, or difficulty. Use for the normal verify-and-document workflow (not the blind property-extraction experiment).
argument-hint: "[component-path] [interface-path]"
---

## Goal
Verify a component with Kani **and** leave a human-readable record of the verified properties.
Inputs are the component's **spec** (the intended behavior) and its **Rust code** (functions +
interface). Outputs are (1) the `#[cfg(kani)] mod verification` harnesses and (2)
`<component>_properties.md` in the component directory.

This skill = **`tools-verify-kani` + explicit spec pairing + a documented-properties step.** Use
`tools-verify-kani` for the create mechanics (stub unsafe/FFI, `kani::assume` mirroring production
guards, run/fix, its "core rule"). This skill adds spec-derived contracts and the plain-English output.

## Steps
1. **Resolve inputs — consume the inventory, do NOT re-extract.** Load the **property inventory**
   `verif/<name>_property_inventory.md` (Role 1, `build-property-inventory`): the agreed, id-keyed,
   tool-independent property set. Also open the Rust functions + interface (`src/**`, `interfaces/`); use
   the spec only for an obligation's wording, never to mint a new one. **Do not build a private per-tool
   property list.** If the inventory is missing, stop and run `build-property-inventory` first. If you
   spot a real obligation the inventory lacks, **flag it back** to the inventory so it gets an `id`.
2. **Harness the shared/global invariants FIRST, once — then reuse.** Before per-method work, take the
   inventory properties whose bundle spans multiple methods (the inventory marks these as global /
   maintained invariants with an attachment count). **Order by attachment count, highest first.** Prove
   each as a **single inductive harness**: build a symbolic valid state, `kani::assume(inv(state))`, call
   the mutator, `assert!(inv(state'))` — plus one establishment harness (`initialize` from scratch →
   `assert!(inv)`). Record it under its **one** inventory `id`; its status then propagates to every bundle
   automatically (Role 3 attaches by id — do NOT re-harness it per method, and do NOT skip it because "no
   single method owns it"). One cheap invariant harness unblocks every method in its bundle at once.
3. **Then, for each remaining per-method obligation, harness the CODE against it.** Function granularity,
   using the obligation from its inventory row (`id`, `statement`, `kind`):
   `kani::assume(<precondition — mirror the production guard>)` → **call the real function** →
   `assert!(<postcondition>)`, taking the already-proved invariants as assumed pre-state. Never lift a
   statement into the harness.
4. **Run.** `cargo kani` to green; fix gaps; **audit** that each `assume` matches a real production
   guard (no over-assuming that would make the proof vacuous).
5. **Validate (anti-vacuity).** Fault-inject (a contract-violating change) and confirm the harness
   **FAILS**; if it still passes, it isn't bound to the real code — fix it. Then revert. For a maintained
   invariant, also fault-inject a mutator that breaks it and confirm the inductive harness FAILS.
   (`--harness` substring-matches; use `--exact` with `module::name` to run one in isolation.)
6. **Document → `components/<name>/<name>_properties.md`** (see shape below): for each verified
   property, the operation, the property in **plain English**, its **spec source**, and the **evidence**
   (harness name; Kani result). State the **bounded scope**: Kani proves over the full input domain
   within `#[kani::unwind(N)]`, symbolically (not by sampling). Every property that was *not* verified is
   reported under one of the two buckets in the shape below — with a concrete signature, never a bare verdict.

## Coverage discipline (mandatory)
**Attempt every property in the inventory that you can express as a harness. You do not get to pick
lanes here** — routing properties across tools is a separate step, not this one. Your job is to verify or
fail *trying*, and to record the outcome **keyed to the inventory `id`** so Role 3 can aggregate it.
- **No pre-filtering by lane, effort, or budget.** "Better suited to Creusot", "intractable", "requires
  induction", "to stay within budget" are **not** acceptable reasons to omit a property. If you can write
  the harness, run it.
- **Attack the highest-leverage properties first.** A property in many bundles is *higher* priority, not
  lower, because one inductive harness discharges it everywhere. Harness the shared/global invariants
  (ordered by attachment count) before method-specific obligations; a cheap invariant left at `not_yet`
  silently keeps every method in its bundle unproved.
- **Attempt at a tractable bounded geometry.** Kani proves exhaustively *at the chosen size*: pick a small
  `#[kani::unwind(N)]` and a small structure that still exercises the property, and use `kani::stub` to
  replace unsupported/FFI dependencies rather than dropping the harness. A bounded proof is a real proof —
  report its geometry.
- **Bounded effort, not infinite grind.** Give each harness a real attempt against a stated cap (per-harness
  CBMC timeout). If it exceeds the cap, record the concrete outcome and move on — do not silently drop it.
- **Every non-success carries a reproducible signature**, never a subjective verdict: `VERIFICATION FAILED`,
  an unwinding-assertion failure (N too small), `unsupported_construct: <name>` (e.g. `_xgetbv` from
  `crc32fast`), or a SAT/solver timeout (`>N min at unwind=K`). Ban bare "intractable"/"out of scope".
- **Only two legitimate non-proofs**, each needing a one-line technical reason and a suggested route:
  (a) **Not expressible for a bounded checker** — concurrency interleavings, liveness/timing, or a property
  that only holds at unbounded scale and no finite geometry witnesses it;
  (b) **Depends on effects Kani cannot model** — real I/O, hardware, FFI (and no stub is faithful).
  Everything else must be attempted at a bounded geometry and reported with its signature under bucket (a).

## Definition of done — "not attempted" is not an outcome (mandatory)
Every inventory property must reach **exactly one** of three end states. There is no fourth box.
1. **Proved** — a passing harness (bounded scope stated), or a disclosed faithful stub/mirror with the boundary named.
2. **Delegated** — the obligation is owned by a **different component** across a real interface boundary; name it and route it. "Belongs to another *tool*" is **not** delegation — that is still your property to attempt here.
3. **Tool boundary hit** — you **wrote the harness, ran it, and captured a reproducible failure signature** (unwinding-assertion / `unsupported_construct: <name>` / SAT-timeout `>N min at unwind=K`). Evidence is mandatory; a limit asserted *without a run* is not a legitimate boundary.

**"Harness not written" / "authorable but not done" is NOT an end state.** A property with no harness is *unfinished work*, never a rating. Do not report it, do not park it, do not hand back the run with it open. The only exit from the not-done set is an actual attempt, which forces the property to (1) or (3). On uncertainty the default is **attempt**, never "tool limitation." A run returned with unattempted properties has not met the bar, no matter how many were proved.

## Clean-slate re-run protocol (mandatory for re-runs)
A re-run must not inherit credit from a prior run's artifacts.
- **Start from a stripped, isolated tree.** New worktree; **remove all existing `#[cfg(kani)] mod verification` harnesses** so every one is re-authored from the inventory. You may not point at pre-existing harnesses to claim coverage.
- **Do NOT delete the previous verified run.** It stays on its committed `verif/kani/<component>` branch as the baseline and as evidence — deletion destroys expensive proofs and the ability to catch regressions.
- **Provenance rule:** a property counts as proved **only** if its harness was authored and is passing **in this run's tree**. A prior-run pass does not count until reproduced here.
- **Diff against the baseline** and report three sets: re-proved, regressed, and prior-"proved" that did **not** reproduce (phantom coverage). That diff is the audit.
- Capture **wall-clock + peak RSS** for every harness in the fresh run.

## `<component>_properties.md` shape
```
# Verified properties — <component> (Kani)
Verified from spec `specs/<...>/spec.md` against code `src/<...>`. Harnesses: `#[cfg(kani)] mod verification`.

## <operation>
- `<inventory-id>` **[Postcondition]** <property in plain English>. — spec FR-nnn / US-n — harness `verify_<...>` (SUCCESSFUL, K checks)
- **[Precondition]** ...
- **[Invariant]** ...

## Assumptions / bounds
- `kani::assume(<...>)` — the production guard it mirrors.
- stubbed FFI / `#[kani::unwind(N)]` bounds — and what that leaves unproven.

## Not verified — attempted, tool boundary hit
- <property> — **attempted** at <geometry: unwind=K, structure size / stub used>, blocked by <concrete
  signature: unwinding-assertion at unwind=K / `unsupported_construct: _xgetbv` / SAT timeout >N min>.

## Not verified — not expressible for a bounded checker (shape)
- <property> — one-line reason: concurrency/liveness, unbounded-only (no finite geometry witnesses it),
  or I/O/hardware/FFI. Route: <Creusot / Loom / Spin / fault-injection test>.
```

## Notes
- Construction + documentation skill — **distinct** from the read-only audits
  `component-check-spec-translation` / `component-check-verif-translation`, from the source-agnostic
  extraction primitive `extract-verifiable-properties`, and from the inventory builder
  `build-property-inventory` (Role 1) whose output this skill **consumes** by `id`.
- Every verified / not-verified line **cites its inventory `id`**, so `tools-aggregate-coverage-by-interface`
  (Role 3) can attach the status to the right bundle without re-reconciling.
- Routing: Kani harnesses → `unstable-kani`.
