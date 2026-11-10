# Worked Example: Proving P12/P13 End-to-End

Purpose:
- A second worked property, following the same shape as `P1_example.md`, but for the **FMap map-mutation** pattern — the part of Creusot that reasons about a *map changing over time*.
- **P12/P13 is representative** of the whole "map-mutation" group: P3 (create), P12/P13 (remove), and the per-entry layer under the map-wide P30/P31. Understanding this one explains how we prove *"the map ended up in the right state"* claims.
- Written for a reader who does **not** already know Rust, Pearlite, or `.coma`. New concepts get a comment or a glossary entry the first time they appear.

_Read `P1_example.md` first — it introduces the artifact layout, `#[ensures]`, VCs, and the "inspect it yourself" workflow, which this doc builds on._

---

## 0. The property in one line

> **P12 — "a successful remove really makes the key gone."**
> **P13 — "removing a key that isn't there (or is in use) changes nothing."**

Both are about the *same* operation (`remove`), so a **single proof** covers them: we prove what the map looks like *before* vs *after*, on every branch.

## 1. What it means in plain terms

`remove(key)` deletes an entry from the dispatch-map (the index of where each cached object lives). Three things can happen, and P12/P13 pin down all three:

1. **Key present and not in use** → it's deleted; afterward the key is genuinely absent (**P12**).
2. **Key not present** → you get `KeyNotFound`, and the map is left **exactly** as it was — no accidental side effect (**P13**).
3. **Key present but currently in use** (something is reading or writing it) → you get an "active references" error, and again the map is **unchanged** — you can't yank data out from under an active user (**P13**).

The subtle, important part is the "**changes nothing**" guarantee on the two failure paths. A naive implementation might, say, delete first and check later, or leave the map half-modified. P12/P13 proves that never happens: failure paths are perfectly inert.

## 2. Where the property comes from (the spec)

`components/dispatch-map/specs/001-dispatch-map/spec.md`:

- **FR-011** (line 187): *"System MUST provide `remove(key)` that deletes the entry from the map. The call MUST return an error if any read or write references are still active."*
- **Acceptance #1** (line 108): key exists, no refs → deleted, subsequent lookup returns `NotExist`. *(P12)*
- **Acceptance #2** (line 109): key doesn't exist → an error / no-op occurs. *(P13, absent case)*
- **Acceptance #3** (line 110): key has active refs → error returned **and the entry remains in the map**. *(P13, busy case)*

## 3. The Rust code being proved

**Repo pointer:** `components/dispatch-map/src/lib.rs:310–333` — the real `remove`. (The dispatcher's own `remove` at `dispatcher/src/lib.rs:1908–1937` just delegates here and re-labels the error, so this map-layer method is the heart of it.)

```rust
fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
    let mut inner = self.state.inner.lock().unwrap();     // take the lock (runtime-only detail)
    let entry = inner
        .entries
        .get(&key)
        .ok_or(DispatchMapError::KeyNotFound(key))?;      // (A) absent  -> return KeyNotFound NOW,
                                                          //     before any mutation

    if entry.read_ref > 0 || entry.write_ref > 0 {        // (B) in use? -> return ActiveReferences,
        return Err(DispatchMapError::ActiveReferences(key)); //     still before any mutation
    }

    let handle = entry.eviction_handle;
    inner.entries.remove(&key);                           // (C) only reached past (A) and (B):
                                                          //     the actual deletion
    // ... eviction-policy + logger bookkeeping (not safety-relevant) ...
    Ok(())
}
```

The whole proof hinges on **ordering**: the two early returns (A) and (B) happen *before* the mutation (C). So both failure paths leave `entries` untouched — that's the "changes nothing" guarantee, and it's a structural fact about where the `return`s sit.

## 4. New concepts this example introduces

P1 was a pure decision over booleans. P12/P13 reasons about a **map that mutates**, which needs four ideas P1 didn't:

> **Glossary (read once, then the code makes sense):**
>
> - **`FMap<u64, EntryModel>`** — a *ghost* (logic-only) finite map from keys (`u64`) to values. It's a mathematical stand-in for the real `HashMap` inside dispatch-map. We use it because the prover has specifications for `FMap` operations but not for the real `std::HashMap`.
> - **`&mut map`** — a *mutable borrow*: the function may change the map. This is what lets us talk about a "before" and an "after".
> - **`*map` (pre-state) vs `^map` (post-state)** — the single most important notation here. In a specification, `*map` means *"the map as it was when the function started"* and `^map` means *"the map as it will be when the function returns."* One symbol for before, one for after. (In `.coma` these are spelled `map.current` and `map.final`.)
> - **`.contains(key)` / `.ext_eq(other)`** — logic queries on an `FMap`: does it hold `key`? and is it *extensionally equal* to another map (same keys → same values, i.e. "identical as a map")? `(^map).ext_eq(*map)` is our precise way of saying **"the map is unchanged."**
> - **`#[check(ghost)]`** — a marker meaning *"this function exists only to be proved, not to run."* It models the decision logic in the simplest form the prover can check.
> - **`get_ghost` / `remove_ghost`** — the ghost-map operations: look up a key (returns an `Option`), and delete a key. Each has its own little contract, which matters in §7.

## 5. The Creusot proof (heavily commented)

**Repo pointer:** `components/dispatcher/verif/src/lib.rs:303–333`.
**Artifact:** `components/dispatcher/verif/verif/dispatcher_verif_rlib/remove_entry.coma` (**4 VCs**, all green — see §7).

```rust
// The value stored per key. We only model the two reference counters,
// because they are the only fields the remove-decision looks at.
pub struct EntryModel {
    pub read_ref: u32,
    pub write_ref: u32,
}

#[check(ghost)]                             // logic-only: proved, never executed
#[ensures(match result {
    // (P12) Success: the key WAS present before (*map) and is ABSENT after (^map).
    Ok(_) => (*map).contains(key) && !(^map).contains(key),

    // (P13, absent) KeyNotFound: key was NOT present, and the map is unchanged
    //               ( ^map identical to *map ). This is the no-mutation guarantee.
    Err(DispatcherError::KeyNotFound) => !(*map).contains(key) && (^map).ext_eq(*map),

    // (P13, busy) ActiveReferences (surfaced as InvalidParameter here): key WAS
    //             present, and the map is likewise unchanged.
    Err(DispatcherError::InvalidParameter) => (*map).contains(key) && (^map).ext_eq(*map),

    _ => false,                             // no other result is allowed
})]
pub fn remove_entry(
    map: &mut FMap<u64, EntryModel>,        // the ghost map we may mutate (has *map / ^map)
    key: u64,
) -> Result<(), DispatcherError> {
    match map.get_ghost(&key) {             // mirror of runtime (A): look the key up
        None => Err(DispatcherError::KeyNotFound),          // absent -> KeyNotFound, no mutation

        Some(e) => {                        // present: inspect the entry `e`
            if e.read_ref > 0 || e.write_ref > 0 {          // mirror of runtime (B): in use?
                Err(DispatcherError::InvalidParameter)      // busy -> error, no mutation
            } else {
                let _ = map.remove_ghost(&key);             // mirror of runtime (C): delete it
                Ok(())                                      // success
            }
        }
    }
}
```

Notice the body has the **same three branches in the same order** as the runtime `remove`: look up (A) → check refs (B) → delete (C). The contract then states the before/after map shape for each branch.

## 6. Reading the contract, arm by arm

The `#[ensures(match result { … })]` is one postcondition split by outcome. Read `*map` as "before", `^map` as "after":

| Branch | Condition on the result | Plain meaning |
|---|---|---|
| `Ok(_)` | `(*map).contains(key) && !(^map).contains(key)` | key was there **before**, and is **not** there **after** → really removed *(P12)* |
| `Err(KeyNotFound)` | `!(*map).contains(key) && (^map).ext_eq(*map)` | key was **not** there, and after = before → nothing changed *(P13 absent)* |
| `Err(InvalidParameter)` | `(*map).contains(key) && (^map).ext_eq(*map)` | key **was** there, and after = before → nothing changed *(P13 busy)* |
| `_` | `false` | any other outcome is disallowed — the function must land in one of the three above |

The `_ => false` arm is doing real work: it forbids, for example, returning `Ok` while the key is still present, or returning `KeyNotFound` while silently deleting something. The proof must rule all of those out.

## 7. Why this proof has 4 VCs (and P1 had 1)

Run the inspection recipe from `P1_example.md` §B:

```bash
cd components/dispatcher/verif
python3 -c "import json,sys;d=json.load(open(sys.argv[1]));print(list(d['proofs']['Coma']))" \
  verif/dispatcher_verif_rlib/remove_entry/proof.json
# → ['vc_elim_Some', 'vc_get_ghost_u64', 'vc_remove_entry', 'vc_remove_ghost_u64']
```

Four VCs, because this function **calls other functions that have their own contracts**, and Creusot emits a VC to check each call is used correctly, plus the main one:

| VC | What it checks |
|---|---|
| `vc_get_ghost_u64` | the `map.get_ghost(&key)` call is well-formed and we use its `Option` result correctly |
| `vc_elim_Some` | when we take the `Some(e)` branch, the value really is a `Some` (safe to unwrap the entry) |
| `vc_remove_ghost_u64` | the `map.remove_ghost(&key)` call is well-formed |
| `vc_remove_entry` | **the main goal**: given those callees behave per their contracts, the three `#[ensures]` arms all hold |

P1 had none of these because it called nothing and only branched on booleans. This is the general rule from `P1_example.md` §F: **called functions with contracts add VCs.**

### A peek at the `.coma` (optional, for the curious)

The generated `remove_entry.coma` makes the before/after reasoning explicit. Two things worth seeing:

The mutable borrow becomes a record with `current` (= `*map`) and `final` (= `^map`) fields, and the final `return` obligation is literally the contract, arm for arm:

```
| Ok _              -> contains_u64 map.current key /\ not contains_u64 map.final key
| Err (KeyNotFound) -> not contains_u64 map.current key /\ ext_eq_u64 map.final map.current
| Err (InvalidParameter) -> contains_u64 map.current key /\ ext_eq_u64 map.final map.current
```

And `remove_ghost`'s own contract (imported from the FMap library) is what tells the solver *how* the map changed — that after removal, the map equals "the old map with `key` set to nothing":

```
self.final = remove_u64 self.current key      -- the map afterward is the map before, minus `key`
result = get_u64 self.current key             -- and it returns whatever `key` mapped to before
```

The solver chains these facts: `remove_ghost` deleted the key ⇒ `contains(^map, key)` is false ⇒ the `Ok` arm holds. That chaining is exactly the part a human would otherwise have to argue by hand.

## 8. What this does — and does not — prove

**Proves:** on all three branches, the map's before/after state is exactly right — real deletion on success, byte-for-byte no change on either failure. The "no mutation" claims are the *concrete* `ext_eq` (map identity), not a proxy flag, so they're strong.

**Does not claim (honest scope):**
- It models the value as just the two reference counters (`read_ref`, `write_ref`) — the only fields the decision reads. Eviction-handle bookkeeping and logging (runtime lines after the delete) are out of scope, being non-safety-relevant side work.
- It's a **single-map** proof (one `FMap`). It does not, by itself, argue cross-map consistency (memory-tier ↔ dispatch-map); that's the P30/P31 map-wide track.
- Sequential: the lock is collapsed away, so concurrent interleavings are not modeled.

## 9. Why P12/P13 represents a group

P12/P13 is the archetype of **Pattern B — FMap map-mutation**. The recipe:

> model the real `HashMap` as a ghost `FMap` → mirror the method's branches as `get_ghost` / `insert_ghost` / `remove_ghost` calls → state each outcome as a `*map`→`^map` before/after claim, using `.contains` for membership and `.ext_eq` for "unchanged" → the FMap callee contracts do the heavy lifting.

The same shape proves:

| Px | Same recipe, applied to… | Proof function | Key move |
|---|---|---|---|
| **P12/P13** | remove (present / absent / busy) | `remove_entry` | `remove_ghost`, `ext_eq` for no-mutation |
| P3 | create on fresh vs duplicate key | `create_entry` | `insert_ghost`, `ext_eq` for no-overwrite |
| P30/P31 | the *whole-map* invariant preserved by insert/overwrite/remove | `map_create_entry` / `map_update_entry` / `map_remove_entry` | the same ops, now under a map-wide `forall`-quantified invariant |

So P3 reads almost identically (swap "remove/absent" for "insert/duplicate"), and the map-wide P30/P31 proofs are this same mutation reasoning lifted from "one key" to "for all keys." A reader who follows P12/P13 has the mental model for the entire map-mutation family.

---

## Appendix — inspecting this proof yourself

Everything from `P1_example.md`'s appendix applies verbatim; just use `remove_entry` in the paths:

```bash
cd components/dispatcher/verif

# What was proved (expect 4 VCs, all alt-ergo, all green):
python3 -c "import json;d=json.load(open('verif/dispatcher_verif_rlib/remove_entry/proof.json'));[print(k,'->',v['prover'],v['time'],'s') for k,v in d['proofs']['Coma'].items()]"

# Re-check it still holds (what CI runs):
why3find prove -r --summary verif/dispatcher_verif_rlib/remove_entry.coma
# → Proved (…/remove_entry.coma) ✔

# See the obligation, incl. the map.current / map.final contract at the tail:
cat verif/dispatcher_verif_rlib/remove_entry.coma
```
