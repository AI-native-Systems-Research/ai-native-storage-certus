# Worked Example: Proving P1 End-to-End

Purpose:
- A single property followed all the way from the spec, through the Rust code, to a machine-checked Creusot proof — written so a non-formal-methods reader can follow it.
- **P1 is representative.** The same recipe applies to the whole "pure-core decision" group (see the last section), so understanding this one explains roughly half of the dispatcher proofs.

_Companion docs: `proof_locator.md` (where every Px lives), `coverage_report.md` (status dashboard), `background/background_on_properties.md` §4 (the pattern catalogue)._

---

## 0. The property in one line

> **P1 — "the dispatcher refuses to start half-configured."**

Before the dispatcher will serve any cache operation, it must confirm its essential parts are wired up. If they are not, it must fail immediately with a clear error instead of running in a broken state. P1 is the formal, machine-checked promise that this check really happens — and happens in the right order.

## 1. What it means in plain terms

The dispatcher depends on two things it cannot work without:

1. a **dispatch-map** (the index that says where each piece of data lives), and
2. a **memory tier** (the fast GPU-adjacent memory pool it caches into).

It also needs to be told **at least one hardware address** (`data_pci_addrs`) for the drives it manages.

P1 says: initialization must

- **fail** with `NotInitialized` if either dependency is missing,
- **fail** with `InvalidParameter` if no hardware address was given,
- **succeed** only when all three conditions are met,

…and it must check them **in that order**, so the *specific* error you get tells you the *first* thing that was wrong. Think of it as the pre-flight checklist: engines, then fuel, then clearance — you never take off having skipped one, and the checklist tells you exactly which item failed.

## 2. Where the property comes from (the spec)

`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`:

- **User Story 5 — Dispatcher Initialization and Wiring (Priority P1)** (line 78): *"Without correct initialization and wiring, no cache operations can proceed. This is the prerequisite for all other stories."*
- **Acceptance scenario #2** (line 89): *"When initialize is called without the dispatch_map or memory_tier receptacle bound, Then an error is returned indicating the missing dependency."*
- **FR-012** (line 254): *"The `initialize` method MUST validate that the `dispatch_map` and `memory_tier` receptacles are bound before proceeding."*
- **Scenario #4** (line 91): *"`initialize()` rejects an empty `data_pci_addrs` with `InvalidParameter` before evaluating the spdk_env connection state."*

Those requirements are exactly the three checks P1 formalizes.

## 3. The Rust code being proved

**Repo pointer:** `components/dispatcher/src/lib.rs:1103–1121` (the guard prefix of `fn initialize`; the checks are lines **1109–1121**).

```rust
fn initialize(&self, config: DispatcherConfig) -> Result<(), DispatcherError> {
    self.log_info("dispatcher: initializing");

    self.max_eviction_attempts
        .store(config.max_eviction_attempts, Ordering::Relaxed);

    self.dispatch_map                                                   // check 1
        .get()
        .map_err(|_| DispatcherError::NotInitialized("dispatch_map not bound".into()))?;

    self.memory_tier                                                    // check 2
        .get()
        .map_err(|_| DispatcherError::NotInitialized("memory_tier not bound".into()))?;

    if config.data_pci_addrs.is_empty() {                               // check 3
        return Err(DispatcherError::InvalidParameter(
            "data_pci_addrs must not be empty".into(),
        ));
    }
    // ... on success, initialization proceeds to build drives, recover extents, etc.
```

The three guards are the safety-relevant part. Everything after them (building drives, recovering extents, starting the background writer) is the "happy path" work that only runs once the checks pass.

## 4. The Creusot proof

Creusot cannot run on the real method — it contains a `Mutex`, `Arc`, SPDK FFI, threads, and logging, none of which the prover models. So the verif crate mirrors **just the decision logic** as a small, pure function, and attaches the contract to it.

**Repo pointer:** `components/dispatcher/verif/src/lib.rs:41–60` — function `initialize_dependency_guards`.
**Artifact:** `components/dispatcher/verif/verif/dispatcher_verif_rlib/initialize_dependency_guards.coma` (1 verification condition, discharged).

```rust
#[ensures(!dispatch_map_bound ==> match result { Err(DispatcherError::NotInitialized) => true, _ => false })]
#[ensures(dispatch_map_bound && !memory_tier_bound ==> match result { Err(DispatcherError::NotInitialized) => true, _ => false })]
#[ensures(dispatch_map_bound && memory_tier_bound && pci_addrs_empty ==> match result { Err(DispatcherError::InvalidParameter) => true, _ => false })]
#[ensures(dispatch_map_bound && memory_tier_bound && !pci_addrs_empty ==> match result { Ok(_) => true, _ => false })]
pub fn initialize_dependency_guards(
    dispatch_map_bound: bool,
    memory_tier_bound: bool,
    pci_addrs_empty: bool,
) -> Result<(), DispatcherError> {
    if !dispatch_map_bound {
        return Err(DispatcherError::NotInitialized);
    }
    if !memory_tier_bound {
        return Err(DispatcherError::NotInitialized);
    }
    if pci_addrs_empty {
        return Err(DispatcherError::InvalidParameter);
    }
    Ok(())
}
```

### How the model maps to the real code

| Real code (`lib.rs`) | Model (`verif`) | Why the abstraction is faithful |
|---|---|---|
| `self.dispatch_map.get()` succeeds/fails | `dispatch_map_bound: bool` | We only care *whether* the receptacle is bound, not the object behind it. |
| `self.memory_tier.get()` succeeds/fails | `memory_tier_bound: bool` | Same. |
| `config.data_pci_addrs.is_empty()` | `pci_addrs_empty: bool` | We only care whether the list is empty. |
| `?`-early-return order | `if` order in the body | The check order is preserved, so the *exact* error variant matches the live code. |

The three runtime conditions become three boolean inputs; the three early-returns become three `if`s in the same order. That one-to-one correspondence is what makes this an **L0 (near-runtime)** proof — the model sits right next to the code, with almost no abstraction gap.

## 5. Reading the contract

Each `#[ensures(...)]` is a promise about the return value, written as *precondition ⇒ outcome*. Read `==>` as "implies" and `result` as the returned `Result`:

1. `!dispatch_map_bound ==> Err(NotInitialized)` — dispatch-map missing ⇒ always `NotInitialized`.
2. `dispatch_map_bound && !memory_tier_bound ==> Err(NotInitialized)` — dispatch-map present but memory-tier missing ⇒ `NotInitialized`. (Together with (1), the `NotInitialized` reason is pinned to the *first* missing dependency — this is what encodes the **order**.)
3. `dispatch_map_bound && memory_tier_bound && pci_addrs_empty ==> Err(InvalidParameter)` — both deps present but no address ⇒ `InvalidParameter`, a *different* error, so callers can tell the two failures apart.
4. `dispatch_map_bound && memory_tier_bound && !pci_addrs_empty ==> Ok` — all three satisfied ⇒ success.

The four clauses **partition every possible input**, so the contract fully specifies the guard's behavior: there is no combination of inputs left unconstrained. Proving them all means the function can never, for any input, return the wrong variant.

## 6. How the proof is actually run

Creusot compiles the annotated Rust into verification conditions (VCs) in the `.coma` intermediate language, which the Why3 platform then discharges with SMT solvers (Z3, CVC5, etc.). In this repo:

```bash
# From components/dispatcher/verif/

# 1. Regenerate the .coma proof obligations from the annotated Rust:
cargo creusot --only coma

# 2. Search for a proof of this function's goals and save it:
why3find prove -t 30 --goals verif/dispatcher_verif_rlib/initialize_dependency_guards.coma

# 3. Replay (re-check) — fast, deterministic, used in CI:
why3find prove -r --summary verif/dispatcher_verif_rlib/initialize_dependency_guards.coma
```

For P1 the whole contract collapses to **1 verification condition**, which the solver discharges immediately: the body is straight-line boolean branching, so the SMT solver just case-splits on the three booleans and checks each of the four implications. Green replay = proved.

## 7. What this does — and does not — prove

**Proves:** for *every* combination of the three conditions, `initialize`'s guard prefix returns exactly the right `Result` variant, in the right priority order. No missing dependency can slip through; the two failure modes never get confused.

**Does not claim (honest scope):**
- The *rest* of `initialize` (drive creation, extent recovery, starting the writer) is not modeled here — those are separate concerns.
- The proof abstracts the receptacle objects to booleans; it certifies the *decision*, not the behavior of `.get()` itself (a trusted framework boundary).
- Concurrency is not in scope (initialization is a single call; Creusot reasons sequentially).

This is the standard L0 honesty line: **we proved the branch logic exactly, over a model that sits right next to the code.** For P1 the abstraction gap is tiny, which is why it makes such a clean teaching example.

## 8. Why P1 represents a whole group

P1 is the archetype of **Pattern A — "pure-core decision"** in the background catalogue. The recipe is always the same:

> take a guard/decision prefix from a real method → mirror it as a small pure function whose inputs are the runtime conditions → write one `#[ensures]` per outcome so the clauses partition the input space → discharge with SMT.

Every property below is proved with this identical shape — only the conditions and outcomes change:

| Px | Same recipe, applied to… | Proof function |
|---|---|---|
| **P1** | initialization dependency guards | `initialize_dependency_guards` |
| P2 | the "already initialized?" gate on every operational API | `ensure_initialized` |
| P6 | `check(key)` returning membership truth | `check_key` |
| P7 | lookup-miss returning `KeyNotFound` without mutation | `lookup_miss_decision` |
| P14 | touch on present/absent key | `touch_decision` |
| P28 | drive-selection index staying in range | `drive_index` |
| P29 | evictor threshold comparisons running in the intended direction | `evictor_decisions` |

So the reader who follows P1 can read any of those six proofs with no new concepts. The other pattern groups (FMap map-mutation, loop invariant/variant, map-wide lift) each have their own archetype — P12/P13, P15, and P30/P31 respectively — which we can write up the same way if useful.

---

## Appendix — How to inspect a proof yourself

You don't have to take the doc's word for what's proved. Every claim in this file is checkable on disk. This appendix uses P1, but the exact same steps work for any property — just swap the function name.

### A. Where the artifacts live

For the dispatcher, everything sits under `components/dispatcher/verif/`:

```
components/dispatcher/verif/
├── src/lib.rs                                  ← the annotated proof functions (#[ensures], …)
└── verif/dispatcher_verif_rlib/
    ├── initialize_dependency_guards.coma       ← compiled proof obligation (the VC)
    └── initialize_dependency_guards/
        └── proof.json                          ← proof session: what was proved, by which solver
```

(Dispatch-map is identical with `dispatch-map` / `dispatch_map_verif_rlib` in the paths.)

Three artifacts per property, in increasing rawness: **`proof.json`** (the record), the **`.coma`** (the obligation), and **`src/lib.rs`** (the source the obligation came from).

### B. "What was proved, and by whom?" — read `proof.json`

```bash
cat components/dispatcher/verif/verif/dispatcher_verif_rlib/initialize_dependency_guards/proof.json
```
```json
{
  "proofs": {
    "Coma": {
      "vc_initialize_dependency_guards": { "prover": "alt-ergo", "time": 0.011 }
    }
  }
}
```

Each key under `"Coma"` is **one verification condition (VC)**, with the solver that discharged it and the time it took. This file has one key → **1 VC**, proved by Alt-Ergo in 11 ms.

**The count-the-keys rule:** number of keys in `proof.json` = number of VCs for that function. Quick check from the shell:

```bash
cd components/dispatcher/verif
python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(len(d['proofs']['Coma']), 'VCs:', list(d['proofs']['Coma']))" \
  verif/dispatcher_verif_rlib/initialize_dependency_guards/proof.json
# → 1 VCs: ['vc_initialize_dependency_guards']
```

### C. "Is it still green?" — replay with `why3find`

`proof.json` records a *past* success; replay re-checks it *now* against the current `.coma` (this is what CI runs):

```bash
cd components/dispatcher/verif
why3find prove -r --summary verif/dispatcher_verif_rlib/initialize_dependency_guards.coma
# → Proved (…/initialize_dependency_guards.coma) ✔
```

`-r` = replay (re-run the saved proof), `--summary` = print one pass/fail line. A ✔ means the recorded proof still discharges the obligation. If the source drifted and the proof no longer holds, this is where it turns red.

### D. "What exactly is the obligation?" — read the `.coma`

```bash
cat components/dispatcher/verif/verif/dispatcher_verif_rlib/initialize_dependency_guards.coma
```

Two parts matter:
- **The header comment (line 1)** points back to the source: `src/lib.rs 45 0 49 32` = the function spans line 45 col 0 to line 49 col 32. So a `.coma` always tells you which Rust it came from.
- **The `return (result) -> { … }` block (the tail)** *is* the VC: your `#[ensures]` clauses, conjoined with `/\`, that the function must satisfy at its exit. The `[@expl:… ensures #0]`…`#3` tags label the four conjuncts — they are sub-parts of the **one** goal, not four VCs.

### E. "How was it generated?" — regenerate and re-prove

```bash
cd components/dispatcher/verif
cargo creusot --only coma        # recompile annotated Rust → fresh .coma obligations
why3find prove -t 30 --goals verif/dispatcher_verif_rlib/initialize_dependency_guards.coma
                                 # search for a proof (timeout 30s) and save it into proof.json
```

Use these two when you *change* a contract or its code. Use the replay in (C) when you only want to check that existing proofs still hold.

### F. Why some proofs show more than 1 VC

P1 is a single VC because its body is straight-line branching. Other shapes generate more — inspect them the same way:

| Function | Property | VCs (keys in `proof.json`) | Why more than one |
|---|---|---:|---|
| `initialize_dependency_guards` | P1 | 1 | straight-line guards, no loops, no `#[requires]` |
| `remove_entry` | P12/P13 | 4 | each FMap ghost call (`get_ghost`, `remove_ghost`, `elim_Some`) adds its own obligation alongside the main `vc_remove_entry` |
| `evict_for_capacity` | P16/P17 | 2 | a loop adds a separate invariant obligation |
| `clear_all` | P25/P26 | 2 | loop invariant + the ghost `remove_one_ghost` call |

Two things add VCs: **loops** (a VC for the invariant, separate from the postcondition) and **called functions with their own contracts** (a VC to prove you satisfy each callee's `#[requires]`). P1 has neither.

> **VCs vs. "goals after splitting".** A single VC can be broken into finer sub-goals by Why3 proof transformations (e.g. `split_vc`) during the search — the registry sometimes cites those (e.g. `clear_all`: "2 VCs, 12 goals after splitting"). The `proof.json` key count is the **VC** count; the split count is a finer decomposition used to make each piece easier for the solver. Both describe the same proof.
