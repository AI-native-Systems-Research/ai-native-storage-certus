# Role 2 (Creusot) progress — eviction-policy-session-lists @ 937838a9

Resume protocol: read this file AND `../verif/creusot_advisory.yaml` first. Everything under DONE
already has a module in `src/lib.rs`. The crate is deliberately ONE file: the emitted `.coma`
basename is the function name, and that name is how the gate finds a proof unit.

## Environment
```
export PATH="$HOME/.local/share/creusot/bin:$HOME/.cargo/bin:$PATH"
cd components/eviction-policy-session-lists/verif-creusot
cargo creusot                 # whole crate (slow: ~230 modules)
cargo creusot <module>        # one module — still re-translates the whole crate, so every .coma
                              # on disk is always current for the source as it stands
why3find prove -X verif/eviction_policy_session_lists_verif_rlib/<module>.coma   # NAME the open goal
```
`why3find.json` is hand-written (`cargo creusot init` refuses inside a cargo workspace) and sets
`time: 20, depth: 14`; several invariant-preservation goals do not close at the default 6s.
The `creusot-std` patch lives in `verif-creusot/Cargo.toml` and must NOT move to
`components/<component>/.cargo/config.toml` — at component scope it breaks `cargo metadata` and so
the Kani gate.

## Four lessons that cost most of the run — do not relearn them
1. **Never put a multi-conjunct invariant behind one `#[ensures(pool_inv(&^p))]`.** `split_vc` can
   only split a goal that is syntactically a conjunction; it cannot look inside a predicate
   application. N conjuncts behind one call become ONE goal and fail, while the same conjuncts each
   close on their own. Every mutator therefore carries ~31 separate `#[ensures]` clauses, one per
   named conjunct.
2. **A composite predicate must be DEFINED as the conjunction of its named halves**, e.g.
   `leaf_set_exact(p) = leaf_in_set(p) && set_only_leaves(p)`, not as the two bodies inlined.
   Otherwise re-deriving the composite from two already-discharged halves is a fresh proof instead
   of a definitional unfolding.
3. **Pearlite has no `Int as u32` cast.** Quantify an arena position as `Int` and use the `_i`
   spellings (`map_has_i`, `leaves_mem_i`, `leaves_has_idx_i`, `opt_is`).
4. **`cargo check` does not reproduce the pearlite-side errors** (`Int as u32`, `Clone` ambiguous
   against the prelude glob, `old()` in a loop invariant, attribute-expansion recursion limit).
   Only `cargo creusot` does. `#![recursion_limit]` is 16384 because ~31 `#[ensures]` on one
   function overflows 1024 with `recursion limit reached while expanding
   #[creusot::clause::ensures]`.

## Mirror architecture
- `Map` = `HashMap<u64,u32>` as a distinct-key association list (`map_find/get/insert/remove/clear`).
- `Leaves` = `BTreeSet<(u64,u32)>` as a strictly ascending `Vec` (`leaves_pos/insert/remove/first/
  clear`), so `iter().next()` is `e[0]` and `iter().take(n)` is the length-n prefix.
- `Pool`/`Pools`/`Node`/`Handle` field-for-field with `src/`, plus ONE ghost/audit field:
  `Node::birth`, the registration stamp. `chain_birth_decreases` says it strictly DECREASES along
  `parent` and is bounded by the clock — a strictly decreasing measure bounded below, which is the
  first-order content of acyclicity and is also what rules out the self-link and two-cycle cases
  that otherwise make `unlink`'s relink postconditions FALSE.
- `Log` / `LockTrace` are counters standing in for the `ILogger` receptacle and the `RwLock`/`Mutex`
  discipline (trusted boundary — see the advisory).
- Fat mutator bodies are `macro_rules!` (`touch_body!`, `unlink_body!`, `register_body!`) so a second
  contract over the same code is a new PROOF of that code, never a second implementation.
- Container primitives carry, besides their functional contracts, a *directly instantiable*
  bound-preservation clause ("every entry of the result is the new one or one that was already
  there"). The `forall<b: Int>` form of the same fact did NOT get instantiated by alt-ergo.

## State at hand-off
- **Artifacts: complete.** 228 `.coma`. All 95 verifiable ids have `verify_<id>`; 86 also have a
  `verify_<id>__mutant` twin; the 9 `origin: divergent` ids have `refute_<id>` instead (for those the
  `verify_` module states the obligation as the specification means it and is EXPECTED to fail).
- **Proving: three helpers still open** — for the CURRENT numbers see the "Helper state" table
  further down, which was measured clean with `-f`; the first-pass figures that used to sit here
  (`pool_register` 1 of 68, `pool_unlink` 31 of 219) were read from a partially-written
  `proof.json` and are superseded. None of the three is a translate error.
- Two contract bugs of mine, found and fixed by the same census: `log_site` was missing
  `#[requires(kind@ <= 2)]` (its `else` arm bumps `warn`, so `kind == 3` falsified the
  `kind != 2 ==> warn unchanged` clause), and `pool_batch_touch` did not expose the clock advance
  that the partial-application refutation needs. `refute_epsl_inv_failed_operations_change_nothing`
  was then rewritten to inline `batch_touch`'s loop body for a two-handle group instead of going
  through `pool_batch_touch`, which removed the dependency on the open loop invariant and closed it.
- Because Creusot is modular, every driver that calls those two closes against their CONTRACTS.
  Crediting the drivers alone would credit conclusions resting on undischarged lemmas, so all 66
  affected ids name the helper in `creusot.evidence.modules` in the advisory. That is deliberate:
  it makes the gap fail the gate instead of disappearing into a `proved`.

- `pool_register` now also exposes the slot-reuse behaviour it inherits from `pool_alloc` (a fresh
  key lands in the slot the spare list last gave up, or at the end of the arena, and never in an
  occupied one). Three of the stale-handle refutations and two `track` drivers turn on exactly that
  clause; without it they could not see that the freed slot comes straight back.

## SECOND PASS — what closed the invariant-preservation goals
The open goals on `pool_unlink` and `pool_register` were **instantiation** failures, not missing
facts. Every fact needed was already a postcondition of a container primitive — in a shape the solver
never instantiated against the goal.

The invariants are phrased with the `_i` predicates (`leaves_mem_i`, `map_has_i`), because pearlite
has no `Int as u32` cast, and they discriminate entries **by arena position**. The primitives stated
preservation as `forall<y: (u64,u32)> ... y != x ==> ...` and `forall<b: Int> (bound) ==> (bound)`.
Same facts, wrong shape — alt-ergo never instantiated either against a `leaves_mem_i(.., s, ix)`
goal. Adding **goal-shaped** clauses to `map_insert` / `map_remove` / `leaves_insert` /
`leaves_remove` — preservation stated over `map_has_i` / `leaves_mem_i`, keyed on the same position
the goal discriminates by — closed them. Measured: with those clauses in, every one of
`pool_unlink`'s 51 `#[ensures]` discharges as its own probe.

**General rule: state a preservation postcondition in the same shape as the invariant that will
consume it.** A logically equivalent phrasing is not an equivalent hypothesis.

### Two measurement traps found while doing this — read before trusting any census
1. **Do NOT read `proof.json` while `why3find` is still running.** It is written INCREMENTALLY, so a
   mid-run read gives a partial picture: `pool_register` read `nulls 0` mid-run and `nulls 2` once
   finished, matching its printed `✘ (66/68)`. (Counting `null` entries is a sound proxy *after* the
   run — it was the timing that misled, not the encoding.) The authoritative signal either way is the
   `Proved (...) ✔` / `Goal ...: ✘ (x/y)` line that `cargo creusot <module>` prints, so gate a census
   on that line, not on the file.
2. **`why3find` replays cached results, so a stale `proof.json` can make a provable module look
   broken and vice versa.** Delete the module's `verif/<crate>_rlib/<module>/` directory (or pass
   `--why3find-arg=-f`) before believing a re-measurement after a source change. The committed
   `proof.json` files should come from a clean run for exactly this reason — otherwise the gate can
   replay a stale failure.

### Why the monolith still fails where each clause passes
With the goal-shaped clauses in place, every one of `pool_unlink`'s 51 `#[ensures]` clauses
discharges when it is the ONLY clause on a function over the same `unlink_body!` (46 of 51 probes
measured clean, 5 still running when the probe run was stopped). The same clauses on ONE function
do not all discharge. So there are two distinct effects, and lesson 1 at the top of this file only
covers the first:
  * a conjunction hidden inside one predicate application is one big goal — fixed by splitting into
    separate `#[ensures]`;
  * a *large number* of goals in one why3 file still degrades the harder ones, because the search
    budget (`time`/`depth`) is spent per file over a much deeper split tree.
The second effect is what remains. The clean fix is fewer goals per module, but the redundant
composite clauses cannot simply be dropped: callers need `pool_inv` and would then have to unfold it
themselves, moving the same work onto ~40 driver modules. Not attempted — it is churn with real
regression risk, and the helper naming keeps the gap honest meanwhile.

Dead end, recorded so nobody retries it: `proof_assert!` is **not usable inside these
`macro_rules!` mutator bodies** — `error: Use of borrowed or uninitialized variable p`, whether the
helper calls are written `f(p)` or explicitly reborrowed `f(&mut *p)`. Bridging facts therefore have
to live on the **callee contracts**, which is where they belong anyway.

## Refutation state — 8 of 9 machine-proved
Re-verified module by module against the source as it now stands (not from cached `proof.json`):

| refutation | |
|---|---|
| `refute_epsl_track_session_comes_from_caller` | PROVED |
| `refute_epsl_touch_invalid_handle_is_an_error` | PROVED |
| `refute_epsl_batch_touch_invalid_handle_is_an_error` | PROVED |
| `refute_epsl_remove_invalid_handle_is_an_error` | PROVED |
| `refute_epsl_inv_failed_operations_change_nothing` | PROVED |
| `refute_epsl_clear_invalidates_existing_handles` | PROVED |
| `refute_epsl_inv_recency_strictly_advances` | PROVED |
| `refute_epsl_inv_handles_keep_naming_their_block` | PROVED |
| `refute_epsl_candidates_listed_in_eviction_order` | OPEN, `✘ (6/7)` |

So all four handle divergences plus the session, recency and partial-application ones are
machine-checked defects. The one still open needs `candidates(n)` over a two-entry candidate set to
be exactly `[k2, k3]` — sequence-level reasoning about the mirror list, nothing to do with the defect
itself. Both refutations that were open after the first pass (`batch_touch`, and earlier the
partial-application one) closed by the SAME move: inline `batch_touch`'s loop body for a one- or
two-handle group instead of calling `pool_batch_touch`, so the refutation stops depending on that
function's open loop invariant.

## The refutations — what actually unblocked them
Three of the four stale-handle refutations (`touch`, `remove`, `handles-keep-naming-their-block`)
failed at first for a reason that had nothing to do with the defect: the scenario is
`remove(index); register(newkey)`, and the proof could not see (a) that the freed slot comes
straight back, nor (b) that `newkey` is still untracked after the removal. Two frame clauses fixed
both — `pool_register`'s slot-reuse clauses and `pool_unlink`/`pool_remove`'s key-index frame
(`k2 != removed_key ==> tracked-status unchanged`). Worth remembering: a refutation that will not
close is usually missing a FRAME fact, not a stronger one.

## Helper state — measured clean (deleted each module dir, then `--why3find-arg=-f`)
| helper | verdict | open |
|---|---|---|
| `pool_register` | `✘ (66/68)` | 2 |
| `pool_unlink` | `✘ (162/184)` | 22 (was 31 of 219 before the goal-shaped clauses) |
| `pool_batch_touch` | `✘ (60/61)` | 1 (the loop invariant) |

Everything else closes: `pool_touch`, `pool_alloc`, `pool_clear`, `pool_candidates`, `pool_tick`,
`pool_is_active`, `pool_len`, `pool_fresh`, `pool_remove`, `pool_evict_oldest`, `log_site`, the lock
model, all four container primitives, and the whole `state_*` layer.

**No helper is fully discharged, so NOTHING has been un-named.** All 66 ids still carry the helper in
`creusot.evidence.modules` (47 name `pool_unlink`, 42 name `pool_register`, 23 name both). Do not
remove a name until that helper actually prints `Proved`.

## TODO, in order of value
1. Close `pool_register`'s 2 and `pool_unlink`'s 22. Both are invariant-preservation goals that pass
   individually as probes, so the blocker is goals-per-file, not the clauses (see the analysis above).
   The two untried levers, in order of expected value:
   a. Cut goals per module. The composite clauses (`free_exact`, `links_agree`, `leaf_set_exact`,
      `sessions_ok`, `one_leaf_per_session`, `by_key_ok`, `counts_bounded`) are definitional
      consequences of the fine halves already ensured, so dropping them from the three mutators
      removes ~7 goals each. Cost: callers must unfold `pool_inv` themselves, which moves that work
      onto ~40 driver modules. Measure before committing to it.
      b. Raise `why3find.json` beyond time=20/depth=14. Note the runtime cost: every FAILING goal
      burns the whole ladder, and 86 mutant twins are *required* to fail, so a bigger budget slows
      the gate far more than it slows a clean run. Size `--cap-seconds/--cap-max` accordingly.
2. `refute_epsl_candidates_listed_in_eviction_order` (6/7): needs `candidates(n)` over a two-entry
   candidate set to be exactly `[k2, k3]`. `counts_bounded` gives `|leaves| == |sessions|`, so with
   two sessions the length is pinned; what is missing is that the two known members ARE the whole
   list, which needs `set_only_leaves` instantiated at each position.
3. Only then drop the closed helper's name from `evidence.modules` (use `/tmp/unname_helper.py`,
   which removes one named helper and drops the disclosure paragraph only from entries that no longer
   name any open helper) and run the gate:
   `python3 <gate>/scorer_creusot.py ../verif --crate-dir . --cap-seconds 120 --cap-max 600`

---

# THIRD PASS (2026-09-29) — the 24 open goals, NAMED

## How to name an open goal without re-running anything
`why3find` does not print subgoal names, but the committed `proof.json` is a faithful map. The
first `split_vc` under `vc_<fn>` yields **one child per `#[ensures]`, in source order** (measured:
`pool_unlink` 51 children / 51 `#[ensures]`; `pool_register` 48/48; `pool_batch_touch`'s exit VC
at `/0/5` 42/42). A `null` leaf is an open goal, so the path's second index IS the clause index.
`#[ensures(A && B)]` stays ONE child — the first split does not flatten conjunctions — which is why
the mapping is exactly 1:1. Script pattern: walk `proofs.Coma.vc_<fn>`, follow `children`,
collect paths to `null`.

## The open goals are NOT what the second pass assumed
| fn | open | clause(s) |
|---|---|---|
| `pool_unlink` | 22 | `idx_ok` (4), `free_covers_inactive` (4), `free_exact` (4), `session_entries_ok` (1), `sessions_ok` (1), `by_key_entries_live` (4), `by_key_ok` (4) |
| `pool_register` | 1 | `free_exact`, 2nd subgoal |
| `pool_batch_touch` | 1 | exit clause #39 — `idxs[0] invalid ==> result != Ok(())` |

Two corrections to the hand-off, both load-bearing:

1. **`pool_unlink` has 4 root causes, not 22 problems and not a uniform budget problem.**
   `free_exact`, `sessions_ok` and `by_key_ok` fail *only because* the fine half inside each of them
   fails (`free_covers_inactive`, `session_entries_ok`, `by_key_entries_live` respectively — each
   fails on its own line too). So **lever (a) as written cannot close `pool_unlink`**: dropping the
   7 composites removes the 3 collateral positions (9 of the 22 goals) and leaves the 4 real ones
   (13 goals) exactly where they are. It is a partial measure at best.

2. **"every clause proves on its own" was never measured for these clauses.** The second pass
   measured 46 of 51 probes and was stopped with 5 still running; the unmeasured remainder is the
   same size as the failing set. The three `pool_unlink` root causes look like **missing facts**,
   not starvation, and the missing facts are identifiable by inspection:
   - `idx_ok`, `session_entries_ok`, `by_key_entries_live` all quantify over **a list POSITION**
     into a mirror list and assert something about `nodes[list[k].1]`. Discharging them across
     `map_insert`/`map_remove` needs "the entry now at position k is the new pair or a pair that was
     already an entry". `map_insert` carries **no** such clause (its positional clause covers only
     `j < old len` with `old[j].0 != k`, and says nothing about the appended slot);
     `map_remove` carries it only in the `forall<b: Int>` bound shape that the second pass itself
     recorded as never instantiated by alt-ergo. So the fact is absent, not starved.
   - `free_covers_inactive` is the one genuinely **existential** invariant
     (`forall i inactive. exists k. free[k] == i`) and is expensive wherever it appears — in
     `pool_register` it proves, but takes **29.0 s**, and its copy inside `free_exact` is the single
     goal that starves there. That one IS a budget/shape problem.

So the split is: `pool_register` = budget on an existential; `pool_batch_touch` = one missing loop
invariant; `pool_unlink` = 3 missing container facts + 1 existential.

## THIRD PASS — measured results

### Lever (b) (raise the budget) is DISQUALIFIED on cost, measured
`why3find prove -t 90 -d 20 -j 16` on `pool_unlink` alone, retrying only its 22 open goals with the
rest replayed from cache, **ran for 35 minutes without finishing** and was killed. The gate runs with
`--cap-seconds 120 --cap-max 600` *per module* and has 86 mutant twins that are REQUIRED to fail, so
each of those burns the full ladder. A 4.5x time budget is therefore not affordable at gate scale,
and it was not even shown to close anything. Budget left at `time: 20, depth: 14` — **`why3find.json`
is UNCHANGED.**

### Lever (a) (drop the composite clauses) was NOT used
It was shown above to be incapable of closing `pool_unlink` (it removes the 3 collateral positions,
not the 4 root causes) and it carries the ~40-driver unfolding cost. Not attempted. No `#[ensures]`
was removed from any mutator; every mutator still promises the full `pool_inv` conjunct list, so no
caller has to unfold anything and there is no driver churn to regress.

### What was actually done — a THIRD lever: positional provenance on the container primitives
Five purely ADDITIVE changes. Nothing was weakened, nothing was deleted, no invariant definition
changed, so no caller can regress by losing a promise.

1-4. `map_insert`, `map_remove`, `leaves_insert`, `leaves_remove` each gained an **existential-free
   positional provenance** clause: "the entry now at position j is the new one, or the entry that was
   at j (or at the one neighbouring position the shift can bring it from)". The second pass had added
   *membership*-shaped (`map_has_i` / `leaves_mem_i`) preservation, which is the right shape for
   `leaf_in_set`-style invariants but the WRONG shape for `idx_ok`, `by_key_entries_live` and
   `session_entries_ok`: those three quantify over a POSITION, and a membership hypothesis makes the
   solver invent the position first. `map_insert` in particular had no whole-range clause at all.
5. `free_push` — a one-line `Vec::push` wrapper for the spare list, used from `unlink_body!`. Its
   contract states the two halves of `free_covers_inactive` in the invariant's own existential shape,
   including the witness `free@.len()` for the slot just freed. That witness is the reason
   `free_covers_inactive` was unprovable after an unlink while it PROVED (in 29.0 s) after a
   register: `pool_alloc` pops, so the register case can reuse the witness it already has from the
   precondition, whereas the unlink case needs a brand-new one and no prover invents it.
6. `pool_batch_touch` gained two loop invariants: liveness is frozen across the batch (`touch` never
   clears a slot), and "reaching iteration i means every earlier handle was in range and live IN THE
   PRE-STATE". Its one open goal was the exit clause "an invalid first handle forces an error", which
   had nothing to contradict because every other invariant speaks about the CURRENT arena while the
   obligation is about the arena on entry. Neither lever (a) nor (b) — a plainly missing fact.

Measured after 1-6 (`cargo creusot <mods> --why3find-arg=-f`, clean forced run, 61 s total incl.
whole-crate translation):
```
Proved (6 files) ✔     # map_insert, map_remove, leaves_insert, leaves_remove, free_push, pool_batch_touch
```
**`pool_batch_touch`: 1 open -> 0. CLOSED.** All four strengthened primitives still prove, so the
additions are themselves discharged and not assumptions.

### Step 2 — the remaining two clauses were `free_covers_inactive` and its composite `free_exact`
Forced clean run after the positional-provenance clauses (`cargo creusot pool_unlink pool_register
--why3find-arg=-f -j 24`, 10 m 56 s wall for the pair):
```
Goal Coma.vc_pool_unlink:   ✘ (163/171)   # 22 open -> 8
Goal Coma.vc_pool_register: ✘ (64/66)     # 1 open -> 2
```
`pool_unlink`'s remaining 8 sit at clause indices 23 and 24 ONLY — `free_covers_inactive` and
`free_exact` (which contains it). **`idx_ok`, `session_entries_ok`, `sessions_ok`,
`by_key_entries_live` and `by_key_ok` all closed**, which confirms those five were missing-fact
failures and that positional provenance was the fact they needed.

`pool_register` went 1 -> 2 because its `free_covers_inactive` had been proving at **29.0 s** against
a `time: 20` budget — i.e. it was already only intermittently provable, and a run under load tipped
it over. Treat a goal that close to the ladder's end as failing, not passing.

### Step 3 — put the model's ONE existential behind a predicate symbol
`free_covers_inactive` was the only invariant with an `exists`. Written inline, every caller had to
re-find a witness; and after an unlink the witness for the slot just freed is `free@.len()`, a term
no prover invents. So:
- new `#[logic] free_covers(f: Seq<u32>, i: Int)` = "slot `i` is somewhere on the spare list";
- `free_covers_inactive` restated as `forall i inactive. free_covers(p.free@, i)` — same proposition,
  but now carrying coverage across a mutation is equality reasoning on `free_covers(..)` terms;
- `free_push`'s two clauses restated as `free_covers((^v)@, x@)` and
  `forall i. free_covers((*v)@, i) ==> free_covers((^v)@, i)`;
- `pool_alloc` gained `forall i. free_covers((*p).free@, i) && i != result ==> free_covers((^p).free@, i)`,
  so the register side stops re-deriving coverage through the `subsequence` clause (that was the 29 s).

The existential is now discharged only where a witness actually appears — inside `free_push` and
`pool_alloc`, each a handful of lines.

Measured (forced, 1 m 38 s including whole-crate translation):
```
Proved (8 files) ✔
```
= `free_push`, `pool_alloc`, `pool_touch`, `pool_clear`, `pool_remove`, `pool_evict_oldest`,
`pool_batch_touch`, and **`probe_unlink_free_covers`** — a temporary module carrying
`free_covers_inactive(&^p)` as its ONLY obligation over `unlink_body!`. It proves. Seven pool modules
that already proved still prove, so redefining `free_covers_inactive` regressed nothing.

## THIRD PASS — a SECOND gap the helper naming was hiding
Closing the helpers does NOT resolve all 64. Cross-referencing `verif/creusot_advisory.yaml` against
a per-module census of every `proof.json` (`/tmp/census.py`, walks `proofs.Coma.*.children` and counts
`null` leaves — 218 modules):

| | count |
|---|---|
| ids naming a helper in `evidence.modules` | 66 (47 `pool_unlink`, 42 `pool_register`) |
| ...of those, every OTHER named module clean | **44** |
| ...of those, the id's OWN `verify_` module still has open goals | **22** |
| ids naming no helper, all modules clean | 29 |

So **closing all three helpers resolves about 44 of the 64; the remaining ~20 have a second,
independent blocker in their own driver module.** Spot-checked by re-running three of them from
source (not from cache): `verify_epsl_create_pool_starts_empty` ✘ (12/13),
`verify_epsl_inv_size_accounting_is_exact` ✘ (9/10),
`verify_epsl_remove_disturbs_only_the_chain_neighbours` ✘ (3/5). Real, not cap artifacts — though
partly: `create_pool_starts_empty` went 2 open -> 1 on a plain re-run, so the gate's
`--cap-seconds 120` does leave some partially-written `proof.json` behind and a census taken straight
after a capped gate run slightly OVER-counts.

Where those driver goals sit: **not** in the `#[ensures]` block. They are in the top-level children
that precede the exit VC — the **`#[requires]` obligations of the calls the driver makes**
(`verify_epsl_no_input_can_make_an_operation_panic` fails at `[1,1]`, `[3,1]`, `[3,3]` of 9;
`verify_epsl_only_pool_creation_takes_the_exclusive_lock` at `[1,1]`, `[3,1]`, `[3,3]`, `[8,2]` of 9;
`verify_epsl_track_keys_are_scoped_to_one_pool` at `[1,*]`, `[2,*]` of 3). `pools_inv` chains fine
(every `state_*` ensures it), so the suspects are the ARITHMETIC side conditions that each `state_*`
requires and none of them re-establishes: `t.reads@ < 4294967280`, `log_room(log)`,
`pools[k].clock@ + n < 18446744073709551000`, `pools[k].nodes@.len() < 4294967280`. A driver that
chains several operations has to carry those across each call, and the `state_*` contracts do not
expose "the arena grew by at most one" / "the clock advanced by at most n" at the `Pools` level.
That is the next work item after the helpers, and it is contract-exposure work on the `state_*`
layer, not prover tuning.

## THIRD PASS — RESULT: all three helpers are CLOSED
Forced clean run, `cargo creusot pool_unlink pool_register --why3find-arg=-f -j 24`:
```
Proved (2 files) ✔      real 8m02s   (user 163m, i.e. the wall time is almost all parallelism)
```
| helper | open before | open after | what closed it |
|---|---|---|---|
| `pool_register` | 2 (was 1, see above) | **0** | goal-shaped `free_covers` on `pool_alloc` |
| `pool_unlink` | 22 | **0** | positional provenance on the 4 container primitives (19 goals) + goal-shaped `free_covers` via `free_push` (3 goals) |
| `pool_batch_touch` | 1 | **0** | two loop invariants (pre-state liveness) |

Neither of the two hand-off levers was used: **`why3find.json` is untouched at `time: 20, depth: 14`,
and not one `#[ensures]` was removed from any mutator.** Every change is additive — four callee
contracts strengthened, one 3-line `free_push` wrapper, one named `free_covers` predicate, two loop
invariants — so no caller lost a promise and there is no unfolding work pushed onto the drivers.

Side effect worth knowing: making the existential opaque made the whole layer CHEAPER, not just
provable. `pool_touch`'s proof went from 61 recorded prover calls to 8.

### The one lesson to carry to the next component
The second pass's rule ("state a preservation postcondition in the same shape as the invariant that
will consume it") is right but was applied to only one of the two shapes an invariant can have:
- an invariant that quantifies over a **VALUE** (`leaf_in_set`, phrased with `leaves_mem_i`) consumes
  a MEMBERSHIP-shaped preservation clause;
- an invariant that quantifies over a **POSITION** (`idx_ok`, `by_key_entries_live`,
  `session_entries_ok`) consumes a POSITIONAL one, and a membership clause is useless to it because
  the solver has to invent the position first.
A container primitive needs BOTH. And any invariant containing an `exists` should be written against
a NAMED logic predicate, so the witness is discharged once, where it exists, instead of being
re-hunted by every caller.

### Cost, for sizing the gate
`pool_unlink` alone is the expensive module: the pair `pool_unlink`+`pool_register` takes **8 min
wall at `-j 24`** on a forced run with everything proving. That is well past `--cap-max 600`, so if
the gate keeps re-running `pool_unlink` as named evidence for 47 ids it will be killed by the cap and
those ids will fail for a reason that has nothing to do with them. Size the cap for it, or drop the
name now that it is discharged.

## THIRD PASS — regression check (forced, 3 m 32 s)
`cargo creusot 'state_*' pool_fresh pools_have_room pools_inv <4 driver controls>
--why3find-arg=-f -j 24`:
```
Error: 4 unproved files
  verify_epsl_inv_size_accounting_is_exact          ✘ (5/6)
  verify_epsl_create_pool_starts_empty              ✘ (8/9)
  verify_epsl_no_input_can_make_an_operation_panic  ✘ (11/14)
  verify_epsl_remove_disturbs_only_the_chain_neighbours ✘ (2/4)
```
Those four are the CONTROLS — the pre-existing driver failures described in the section above,
included on purpose, and every one is at the SAME count as before the change. Everything else in the
run proved: all twelve `state_*` modules, `pool_fresh`, `pools_inv`, `pools_have_room`. Together with
the earlier `Proved (8 files)` (which covered `pool_touch`, `pool_clear`, `pool_remove`,
`pool_evict_oldest`, `pool_batch_touch`, `pool_alloc`, `free_push` and the probe) and
`Proved (6 files)` (the four container primitives), **nothing that proved before stopped proving.**
No driver module regressed.

## Advisory: names KEPT, stale sentence CORRECTED
The 66 ids that name a helper still name it. Removal is now legitimate (all three print Proved), but
it was deliberately not done:
- it buys nothing yet — ~20 of those ids still fail on their OWN driver module, so the gate fails
  either way;
- keeping the name is what makes the scorer re-derive the helper from source rather than trust this
  file, which is the whole point of the wiring;
- it is one script away (`python3 /tmp/unname_helper.py pool_register pool_unlink pool_batch_touch`,
  script verified present) whenever the gate's per-module cap makes re-running `pool_unlink` too
  expensive.

What WAS changed: the `DISCLOSED DEPENDENCY` paragraph in all 66 notes said the helpers "still had
open subgoals (pool_register 1 of 68, pool_unlink 31 of 219)". That is now false, and a false
disclosure is worse than none, so it was rewritten to the measured current state. Advisory keys are
still exactly `fidelity` / `note` / `evidence` / `delegate_to` — checked; no `status`, no `symbol`,
no `_scored_by` anywhere.

## Next work-list, sharpest first
1. **The ~20 driver modules.** Their open goals are `#[requires]` obligations at call sites, not
   `#[ensures]`. Read the top-level `split_vc` children BEFORE the exit VC to see which call.
   Hypothesis to test first: the arithmetic side conditions (`t.reads@ < 4294967280`,
   `log_room(log)`, `pools[k].clock@ + n < 18446744073709551000`,
   `pools[k].nodes@.len() < 4294967280`) are required by every `state_*` and re-established by none,
   so a driver that chains two or more operations cannot discharge the second call's precondition.
   The fix is contract exposure on the `state_*` layer ("the arena grew by at most one", "the clock
   advanced by at most n", "reads grew by at most one"), not prover tuning.
2. `refute_epsl_candidates_listed_in_eviction_order` (still `✘ (9/10)`) — unchanged, and the second
   pass's analysis of it still stands.
3. Only then unname and run the gate.

---

# FOURTH PASS (2026-09-29) — the 28 driver modules

Gate baseline at the start of this pass: **60 proved / 7 refuted / 0 tool-boundary / 0 delegated /
28 UNRESOLVED**. The 28 were located precisely with a corrected census (`/tmp/census2.py` — the
old `/tmp/census.py` counted any node carrying a `prover` key as proved, which mis-read the 83
mutant twins; and a module whose own `vc_<m>` key is ABSENT from `proof.json` also failed, it is
not "unmeasured"). Result: **28 modules have a recorded `null` under their own `vc_`**, and they
are exactly the 28 UNRESOLVED. The 7 `refuted` ids are the 7 divergent ids whose `verify_` module
is ABSENT (expected to fail) and whose `refute_` proves.

## The 28, classified by the FACT that is missing (from `proof.json` open-goal paths)
`/tmp/openpaths.py <mods>` walks `proofs.Coma.vc_<m>` and prints the path of every `null`. The
first `split_vc` child index is the CALL index for a driver (one child per call whose `#[requires]`
must be discharged, then one for the exit VC), and within a call child the second index is that
callee's `#[requires]` index in source order.

Three classes, all **contract exposure**, none prover tuning:

1. **Budget (arithmetic side conditions).** `pool_touch` / `pool_register` / `pool_evict_oldest`
   never said how much of the recency counter or the arena they consumed, so the SECOND call in a
   chain could not discharge `p.clock@ < 18446744073709551615` /
   `p.nodes@.len() < 4294967295`. Same one level up: no `state_*` re-establishes
   `pools_have_room`, and none bounds `log.warn` (`state_track`'s `InvalidPool` arm bumps it) or
   `log.debug` (`state_create_pool`'s banner).
2. **Lock-trace frame.** `t.held`/`t.peak`/`t.acquires` are preserved by every `state_*` except
   `state_batch_touch2`, and NONE said so — so `state_batch_touch2`'s
   `#[requires(t.held@ == 0 && t.peak@ == 0)]` was undischargeable after any earlier call, and
   `(^t).held@ == 0` / `(^t).peak@ <= 1` were unprovable at a driver exit.
3. **Missing whole-node frames** on `pool_remove` / `pool_evict_oldest` / `pool_register` — facts
   that `pool_unlink` already proves but that its wrappers did not re-export.

## Step 1 — pool-level budget + a "touch changes only recency" frame (ADDITIVE)
- `pool_touch`: `(^p).clock@ <= (*p).clock@ + 1`, `(^p).nodes@.len() == (*p).nodes@.len()`, and one
  unconditional `forall<j>` frame (key/session/parent/child/active/birth all preserved) plus
  `free@`/`by_key.e@`/`sessions.e@`/`len` equality. Stated UNCONDITIONALLY, not under
  `result ==>`: every caller discards the boolean, and a `result ==>` clause makes the solver
  case-split on a value it has thrown away.
- `pool_register`: `(^p).clock@ <= (*p).clock@ + 1` (both paths tick exactly once).
- `pool_evict_oldest`: `(^p).nodes@.len() == (*p).nodes@.len()`.

Measured, forced clean (`--why3find-arg=-f -j 24`, 2 m 54 s):
```
5 of 6 Proved: pool_touch, pool_evict_oldest,
               verify_epsl_inv_active_stamps_are_distinct        (was open)
               verify_epsl_inv_size_accounting_is_exact          (was open)
               verify_epsl_inv_arena_only_grows_except_on_clear  (was open, 2 goals)
Goal Coma.vc_verify_epsl_inv_block_belongs_to_exactly_one_session: ✘ (3/4)   # 2 open -> 1
```
`pool_touch` still proves WITH the new clauses, so they are discharged, not assumed.
