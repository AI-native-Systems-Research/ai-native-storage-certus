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
