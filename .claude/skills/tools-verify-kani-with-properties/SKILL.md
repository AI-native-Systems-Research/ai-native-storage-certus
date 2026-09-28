---
name: tools-verify-kani-with-properties
description: Create Kani harnesses for a Certus component from BOTH its spec and its Rust code — at function granularity, calling the real function under spec-derived pre/postconditions — run them, and record what was verified (with evidence) by writing each property's status/symbol/fidelity/evidence into the shared `unified_properties.yaml` under its `kani:` block. Attempt every property you can express as a harness — do not pre-filter by lane, effort, or difficulty. Use for the normal verify-and-document workflow (not the blind property-extraction experiment).
argument-hint: "[component-path] [interface-path]"
---

## Goal
Verify a component with Kani **and** record exactly what was verified.
Inputs are the component's **spec** (the intended behavior) and its **Rust code** (functions +
interface). Outputs are (1) the `#[cfg(kani)] mod verification` harnesses and (2) the per-property
status/symbol/fidelity/evidence written back into the shared `unified_properties.yaml` under each
property's `kani:` block.

This skill = **`tools-verify-kani` + explicit spec pairing + a status-write-back step.** Use
`tools-verify-kani` for the create mechanics (stub unsafe/FFI, `kani::assume` mirroring production
guards, run/fix, its "core rule"). This skill adds spec-derived contracts and the structured
write-back into `unified_properties.yaml`.

## Steps
0. **Environment — confirm the toolchain FIRST (mandatory).** Confirm `cargo kani` is reachable and run
   from the **component directory**, not the workspace root — the root triggers a spurious `gpu-services`
   feature error that is an environment artifact, never a property verdict. A missing/erroring toolchain is
   a **blocker to fix or escalate here**, never a `tool-boundary` and never a reason to leave a property unattempted:
   ```bash
   command -v cargo-kani >/dev/null || { echo "FATAL: cargo-kani unreachable — escalate to user"; exit 1; }
   cargo kani --version    # expect a version line, not an error
   cd components/<name>    # run all `cargo kani` from HERE, not the workspace root
   ```
1. **Resolve inputs — consume the inventory, do NOT re-extract.** Load the **property inventory**
   `verif/unified_properties.yaml` (Role 1, `build-property-inventory`): the agreed, id-keyed,
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
6. **Produce artifacts the scorer can reproduce — you do NOT write your own status.** The verdict is
   assigned in Step 7 by the shipped scorer, which *runs* your harnesses. Your job in Step 6 is to leave,
   for every property, a runnable artifact named so the scorer finds it:
   - **One harness per property `id`**, named `verify_<id>` (id lowercased, `-`→`_`). If a single
     descriptive harness covers a property, point the scorer at it with `kani.evidence.harness: <name>`
     instead — but it must exist and pass on its own.
   - **For any property you cannot prove on the first clean attempt, build the lever-battery variants**
     that `gate/lever_battery_kani.yaml` requires for the failure class you hit — as *named runnable
     harnesses*: `verify_<id>__nounwindcheck`, `verify_<id>__concrete`, `verify_<id>__split_*`,
     `verify_<id>__stub`. A tool-boundary is **inadmissible** until every required variant exists and the
     scorer has run each — a missing variant scores UNRESOLVED (gate fails), not tool-boundary.
   - **An anti-vacuity twin** `verify_<id>__mutant` wherever the property could pass vacuously — a
     deliberately contract-violating copy that the scorer requires to FAIL.
   - In `unified_properties.yaml` under the `kani:` block you may fill **only the advisory fields the
     scorer does not own**: `fidelity` (`real-type-bounded | representative | arithmetic-core |
     bounded-shallow`) and a human `note`; and for a genuine cross-*component* delegation, a resolvable
     `delegate_to:` (named component + concrete obligation). **Do NOT write `status`, `symbol`, or
     `evidence`** — the scorer overwrites them from the live run and stamps `_scored_by: scorer_kani`.
     Anything you type there is discarded; the only way to move a property's status is to make its
     artifact reproduce.
   `fidelity` meanings: `real-type-bounded` = real product type/code under a stated `#[kani::unwind(N)]`
   (a bounded proof is a real proof); `representative` = the ∀ shrunk to one symbolic representative;
   `arithmetic-core` = an extracted arithmetic core behind a stub (`★`); `bounded-shallow` = a pass under
   `--no-unwinding-checks` at low unwind — every loop *cut* at N, sound only for executions where each
   loop runs ≤N (disclose the cut in `note`; route a fully-sound proof of the same obligation to Creusot).
7. **Gate — the shipped scorer decides, by reproduction (mandatory; you cannot grade yourself).** The
   reproduction gate is `scorer_kani.py`, which the `component-verify` orchestrator runs in its Step 2.5.
   If you are running this skill standalone, invoke it yourself before returning and paste its output into
   your report:
   ```bash
   python3 "$(git rev-parse --show-toplevel)/.claude/skills/component-verify/gate/scorer_kani.py" \
       components/<name>/verif --component-dir components/<name> --cap-seconds <cap>
   ```
   The scorer *executes* each `verify_<id>` harness (rebuilding from source, so a hand-edited artifact
   cannot fake a pass), captures wall-clock + peak RSS **from the run**, checks the anti-vacuity mutant,
   and writes the scorer-owned `status`/`evidence`/`note`/`_scored_by`. It assigns `proved` /
   `tool-boundary` (only after the full lever battery ran and the residual signature is not a known defeat)
   / `delegated` (resolvable referent) / **UNRESOLVED** (no harness, an unreproducible pass, a missing
   required lever variant, a known-defeat signature, or a broken harness) — and **exits non-zero if any
   verifiable property is UNRESOLVED.** If it exits non-zero you are **not done**: for each UNRESOLVED
   `id`, produce the missing harness / the demanded lever variant / fix the broken harness, and re-run the
   scorer. Iterate until it exits 0. You may not hand back the run, write a "partial run" summary, or ask
   the user to accept the remainder while the scorer fails — no matter how many properties are proved.
   Editing the YAML to say `proved` does nothing: the scorer reproduces from source and overwrites it.

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
  CBMC timeout). Exceeding the cap converts the property to a **`tool-boundary` with the timeout signature**
  (`miss_class: TOOL`, `note:` the exact `>N min at unwind=K` string) — it does **not** license leaving the
  property unattempted. The cap decides *when the captured signature is ready for the scorer*, never *whether you may skip*.
- **Every non-success carries a reproducible signature**, never a subjective verdict: `VERIFICATION FAILED`,
  an unwinding-assertion failure (N too small), `unsupported_construct: <name>` (e.g. `_xgetbv` from
  `crc32fast`), or a SAT/solver timeout (`>N min at unwind=K`). Ban bare "intractable"/"out of scope".
- **Only two legitimate non-proofs**, each needing a one-line technical reason and a suggested route:
  (a) **Not expressible for a bounded checker** — concurrency interleavings, liveness/timing, or a property
  that only holds at unbounded scale and no finite geometry witnesses it;
  (b) **Depends on effects Kani cannot model** — real I/O, hardware, FFI (and no stub is faithful).
  Everything else must be attempted at a bounded geometry and reported with its signature under bucket (a).
- **Concrete string/char CONTENT is EXPRESSIBLE here — prove it, do not claim a boundary.** A property
  Creusot cannot state because its string view is opaque (contains a tag, equals `"ERROR"`, ends in `'\n'`,
  excludes an ESC byte, or is built by `format!`) is exactly what a bounded checker CAN witness on the
  concrete bytes. When `unified_properties.yaml` shows such a property with `creusot.claims_inexpressible`
  and `covered_by: kani`, that is **your obligation to discharge bounded** (small symbolic representative,
  real type), not to punt. There is **no inexpressibility hatch for Kani** in this pipeline — bounded byte
  reasoning is Kani's home turf, so a Kani inexpressibility claim on strings is illegitimate.

## Coverage-extending levers — exhaust these BEFORE recording ⊘ (mandatory)
A "tool boundary" is only legitimate after the applicable levers below have been *tried and shown to
fail with a captured signature*. A wall that a documented lever clears is a **recipe gap, not a tool
boundary** — recording ⊘ without trying the lever is a false negative (this is exactly how a
HashMap-backed component was once mis-recorded as whole-component ⊘ when a one-line stub proved it in
3.5 s). Enable unstable levers with the matching `-Z` flag: `cargo kani -Z stubbing`,
`-Z function-contracts`, `-Z loop-contracts`.

- **HashMap / HashSet construction wall (`RandomState::new` → `getrandom` → `syscall`).** This is
  DEFEATABLE, not a boundary. Stub `RandomState::new` with a deterministic seed and construct the real
  type in situ:
  ```rust
  use std::collections::hash_map::RandomState;
  use std::mem::{size_of, size_of_val, transmute};
  fn concrete_state() -> RandomState {
      let keys: [u64; 2] = [0, 0];
      assert_eq!(size_of_val(&keys), size_of::<RandomState>());
      unsafe { transmute(keys) }              // fixed [0,0] seed, no syscall
  }
  #[kani::proof]
  #[kani::stub(RandomState::new, concrete_state)]   // also covers the Default path
  #[kani::unwind(5)] #[kani::solver(minisat)]
  fn harness() { /* build + drive the real HashMap-backed struct */ }
  ```
  Fidelity `real-type-bounded` (the real product type, in situ; symbol `✓`). Two measured caveats:
  (a) **use concrete representative keys** — symbolic keys (`kani::any()`) drive SipHash over symbolic
  bytes and blow up hashbrown's probe loop (record such a harness as `representative` fidelity, not a
  boundary); (b) the seed stub is sound only for map *semantics* (insert/get/remove/len/containment) —
  never assert anything depending on hasher randomness (iteration order, DoS-resistance) under it.
- **`extern "C"` / FFI in the path.** `#[kani::stub(the_extern_fn, rust_model)]` replaces the foreign fn
  with a Rust model (symbol `★`, disclosed in `note`). Or give it a contract and `#[kani::stub_verified]`
  (`-Z function-contracts`). Only ⊘ if no faithful model exists.
- **Unbounded loop.** Prefer `#[kani::loop_invariant(..)]` (+ `#[kani::loop_decreases(..)]`,
  `-Z loop-contracts`) over an ever-growing `#[kani::unwind]` — an inductive invariant removes the bound.
  (Not `while let` loops; decreases is integer-only.)
- **Large / slow formula.** Switch `#[kani::solver(minisat|kissat|cadical|z3)]` (minisat cleared the
  hashbrown `swap_nonoverlapping` loop here); bound symbolic state with `kani::assume`; decompose a huge
  function via `#[kani::proof_for_contract]` + `#[kani::stub_verified]` so callers assume the verified
  contract instead of inlining the body.
- **Sweep the unwind number — and use `--no-unwinding-checks` for shallow coverage (the coverage-over-depth
  lever).** A single fixed unwind is a trap: too low raises a spurious `unwinding assertion loop N` FAILED
  (not a real defect — just "the bound is below the loop"), too high explodes the SAT formula into a
  timeout. **Iterate the bound** — try `--unwind 2,3,4,5,…` (cheapest first) and take the smallest that
  reaches the property without an unwinding failure; record the winning `N` in `evidence.unwind`. When the
  loop genuinely needs many iterations to *terminate* (e.g. hashbrown's probe loop) and the sound geometry
  times out, add **`--no-unwinding-checks`**: Kani then *cuts* every loop at the unwind bound and verifies
  the bounded prefix, turning a >500 s timeout into a ~7 s pass (measured ~70× on the ≥2-`HashMap`-mutation
  harnesses here — unwind 5 timed out >500 s under minisat **and** cadical; unwind 2 + `--no-unwinding-checks`
  passed in ~7 s / ~355 MB). **Soundness cost — record it honestly:** this is `bounded-shallow` fidelity
  (symbol `✓`, but the `note` must state "loops cut at N; sound only for executions where every loop runs
  ≤N times; deeper iterations out of scope"). It is *coverage over depth*: it lets you sweep many functions
  cheaply and confirm the shallow slice, but it is **not** a full proof — route a fully-sound proof of the
  same obligation to Creusot (FMap induction removes the bound entirely). Prefer real levers (loop
  contracts, solver switch, smaller symbolic state) when they fit in budget; reach for `--no-unwinding-checks`
  to *widen* coverage across source, not to *replace* a sound proof where one is affordable.
- **Reference:** `verif/skills_research/KANI_capability_catalog.md` in the eviction-policy-session-lists
  component carries the full catalog with flags, examples, and soundness costs.

## Definition of done — "not attempted" is not an outcome (mandatory)
Every inventory property must reach **exactly one** of three end states. There is no fourth box.
1. **Proved** — a passing harness (bounded scope stated), or a disclosed faithful stub/mirror with the boundary named.
2. **Delegated** — the obligation is owned by a **different component** across a real interface boundary; name it and route it. "Belongs to another *tool*" is **not** delegation — that is still your property to attempt here.
3. **Tool boundary hit** — you **exhausted the applicable coverage-extending levers, wrote the harness, ran it, and captured a reproducible failure signature** (unwinding-assertion / `unsupported_construct: <name>` / SAT-timeout `>N min at unwind=K`). Evidence is mandatory; a limit asserted *without a run*, or before trying the lever that clears it (e.g. the `RandomState` stub for a HashMap wall), is **not** a legitimate boundary — it is a recipe gap.

**"Harness not written" / "authorable but not done" is NOT an end state.** A property with no harness is *unfinished work*, never a rating. Do not report it, do not park it, do not hand back the run with it open. The only exit from the not-done set is an actual attempt, which forces the property to (1) or (3). On uncertainty the default is **attempt**, never "tool limitation." A run returned with unattempted properties has not met the bar, no matter how many were proved.

**An unattempted property — no runnable harness produced — is a transient working state, never returnable.** It means the work is *unfinished*, not that a boundary was found. You do **not** write `status` at all (Step 6); enforcement is by **reproduction**, not by reading a field you set. The **gate (Step 7) is the shipped `scorer_kani.py`**: for any property lacking a harness it can re-run to `VERIFICATION SUCCESSFUL`, a completed run lever-battery, or a resolvable delegation, it returns **UNRESOLVED and exits non-zero** — the hand-back fails. (An advisory `miss_class: AGENT` you leave behind is your own admission the item is unfinished, never a verdict.) There is no "partial run" hand-back and no asking the user to accept the remainder — you either finish (every property the scorer re-runs to proved / delegated to a named component / tool-boundary with a captured signature that survives the lever battery) or you keep working. The highest-leverage unfinished items are the shared/global invariants (Step 2, ordered by attachment count); the scorer flags them first because one left unattempted keeps every method in its bundle unproved.

## Clean-slate re-run protocol (mandatory for re-runs)
A re-run must not inherit credit from a prior run's artifacts.
- **Start from a stripped, isolated tree.** New worktree; **remove all existing `#[cfg(kani)] mod verification` harnesses** so every one is re-authored from the inventory. You may not point at pre-existing harnesses to claim coverage.
- **Do NOT delete the previous verified run.** It stays on its committed `verif/kani/<component>` branch as the baseline and as evidence — deletion destroys expensive proofs and the ability to catch regressions.
- **Provenance rule:** a property counts as proved **only** if its harness was authored and is passing **in this run's tree**. A prior-run pass does not count until reproduced here.
- **Diff against the baseline** and report three sets: re-proved, regressed, and prior-"proved" that did **not** reproduce (phantom coverage). That diff is the audit.
- Capture **wall-clock + peak RSS** for every harness in the fresh run.

## What each `kani:` block must record (per `id`)
Write these into `unified_properties.yaml`; there is **no `.md`** — the YAML is the record and the HTML
(Role 3) is the human-readable form.
- **Proved rows:** `status: proved`, `symbol: ✓` (native) or `★` (disclosed faithful stub/mirror), the
  `fidelity` (`real-type-bounded` / `representative` / `arithmetic-core` / `bounded-shallow`), and
  `evidence` with the harness name, Kani result, `unwind: N`, any `flags` (e.g. `--no-unwinding-checks`),
  wall-clock + peak RSS. State the bounded geometry in `note` — and for `bounded-shallow`, the loop-cut
  caveat (sound only when every loop runs ≤N) plus the Creusot route for a fully-sound proof.
- **Assumptions / bounds:** record each `kani::assume(...)` (the production guard it mirrors) and each
  stubbed FFI / `#[kani::unwind(N)]` bound — and what it leaves unproven — in the `note` of the property it
  affects.
- **Not verified — attempted, tool boundary hit:** `status: tool-boundary`, `symbol: ⊘`, `miss_class: TOOL`,
  and a `note` giving the geometry tried (unwind=K, structure size / stub used) and the concrete signature
  (unwinding-assertion at unwind=K / `unsupported_construct: _xgetbv` / SAT timeout `>N min`).
- **Not verified — not expressible for a bounded checker:** `status: tool-boundary`, `symbol: ⊘`, `note`
  with the one-line reason (concurrency/liveness, unbounded-only, or I/O/hardware/FFI) and the route
  (Creusot / Loom / Spin / fault-injection test). **Note:** string/char CONTENT is *not* in this bucket — it
  is expressible bounded (prove it); this bucket is for concurrency, unbounded-only, and I/O/hardware/FFI.
- **Delegated:** `status: delegated`, `symbol: ⤴`, `note` naming the owning component/test.

## Notes
- Construction + documentation skill — **distinct** from the read-only audits
  `component-check-spec-translation` / `component-check-verif-translation`, from the source-agnostic
  extraction primitive `extract-verifiable-properties`, and from the inventory builder
  `build-property-inventory` (Role 1) whose output this skill **consumes** by `id`.
- Every outcome is written to `unified_properties.yaml`'s `kani:` block by `id`, so
  `tools-aggregate-coverage-by-interface` (Role 3) renders from structured data without re-reconciling.
- Routing: Kani proof code (the `#[cfg(kani)] mod verification` harnesses + the `unified_properties.yaml`
  write-back) → the per-component **`verif/kani/<name>`** branch, overwrite-in-place, committed only under
  `components/<name>/`. Under the `component-verify` orchestrator, the orchestrator performs the
  branch/commit/push and enforces the component-folder-only contamination gate — this skill just leaves the
  artifacts in the working tree.
