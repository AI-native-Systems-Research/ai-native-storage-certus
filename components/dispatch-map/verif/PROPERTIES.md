# dispatch-map — Verified Properties (Creusot)

Formal verification of the dispatch-map reference-count and location
state-machine invariants, proved with [Creusot](https://github.com/creusot-rs/creusot)
and discharged by the `alt-ergo`, `z3`, `cvc5`, and `cvc4` SMT solvers.

- **Crate:** `components/dispatch-map/verif/`
- **Source under proof (mirror of):** `components/dispatch-map/src/lib.rs`, `src/entry.rs`
- **Spec reference:** `components/dispatch-map/specs/001-dispatch-map/data-model.md`
- **Status:** `Proved (14 files) ✔`

## Reproduce

```bash
cd components/dispatch-map/verif
cargo creusot --only coma   # fast syntax check (no solvers)
cargo creusot               # full proof — expect: Proved (14 files) ✔
```

## What is a "mirror"

The shipped functions cannot compile under Creusot (they use `Mutex`,
`HashMap`, `Condvar`, raw pointers, an async logger). This crate proves a
**byte-faithful copy** of each function's pure core:

| Shipped code | Verification mirror |
|---|---|
| `state.inner.lock().unwrap().entries.get_mut(&key)` | `entry: &mut DispatchEntry` passed directly (lock already held) |
| `wait_for(\|e\| e.write_ref == 0)` | `#[requires((*entry).write_ref == 0)]` (the condvar wait established it) |
| `Arc<DmaBuffer>` / `*mut u8` / `EvictionHandle` / `AtomicU32` | opaque `u64` / dropped field (not part of the invariant) |
| `.checked_add(1).ok_or(e)?` | equivalent `match … { Some(v) => v, None => return Err(e) }` |

Because the proof runs on a mirror, a **residual drift gap** exists between it
and the shipped function. Two mechanisms keep the proof honest:

1. Each mirror body is kept line-faithful to cited source lines, with any
   intentional divergence annotated in-code as a `DRIFT:` comment.
2. Contracts are validated by **fault injection** (see below) so no proof is
   vacuous.

## Invariants (logical predicates)

| Predicate | Meaning | Spec basis |
|---|---|---|
| `inv_write_binary(e)` | `write_ref == 0 \|\| write_ref == 1` — the writer count is a binary lock, never a counter | data-model.md: "write_ref … Active writer count (0 or 1)" |
| `no_active_refs(e)` | `read_ref == 0 && write_ref == 0` — precondition for removal/eviction | remove/evict guards |
| `is_memory_tier(e)` | location is `MemoryTier` | Location enum |
| `is_block_device(e)` | location is `BlockDevice` | Location enum |
| `is_memory_tier_persisted(e)` | `MemoryTier` **and** `ssd_offset: Some` — write-through complete, eligible to demote/evict | data-model.md state machine |

## Proved operations (11)

Each lists the precondition (what the caller's lock/guard guaranteed) and the
postcondition proved. `*entry` = value on entry, `^entry` = value on exit.

| Function | Requires | Ensures (key properties) |
|---|---|---|
| `take_read` | `write_ref == 0`; `read_ref < u32::MAX`; `inv_write_binary` | `read_ref' = read_ref + 1`; `write_ref` unchanged; `inv_write_binary` preserved |
| `take_write` | `read_ref == 0 && write_ref == 0` | `write_ref' = 1`; `read_ref` unchanged; `inv_write_binary` preserved |
| `release_read` | `inv_write_binary` | `read_ref > 0 ⇒ read_ref' = read_ref - 1` (**no underflow**); unchanged when `read_ref == 0`; `inv_write_binary` preserved |
| `release_write` | `inv_write_binary` | `write_ref' = 0` on both paths; `read_ref` unchanged; location preserved; `inv_write_binary` preserved |
| `downgrade_reference` | `inv_write_binary`; `read_ref < u32::MAX` | write→read handoff: `write_ref > 0 ⇒ write_ref' = 0 ∧ read_ref' = read_ref + 1` (**no underflow, stays binary**) |
| `convert_to_storage` | — | MemoryTier ⇒ `ssd_offset` becomes `Some` (`is_memory_tier_persisted`); conditional `read_ref` decrement is underflow-safe; `write_ref` unchanged |
| `convert_memory_tier_to_block` | — | `is_memory_tier_persisted ⇒ is_block_device` after; refs untouched |
| `promote_block_to_memory_tier` | — | `is_block_device ⇒ is_memory_tier_persisted` after (retains SSD offset + refs) |
| `try_evict_to_block` | `is_memory_tier_persisted`; `no_active_refs` | `is_block_device` after; refs untouched |
| `check_removable` | — | `Ok ⇒ no_active_refs(entry)` (removal only permitted with zero refs) |
| `is_evictable` | — | `result ⇔ (no_active_refs ∧ is_memory_tier_persisted)` |

## Proved lifecycle sequences (3)

Whole paths through the state machine preserve the invariants end-to-end.

| Lifecycle | Path | Final-state guarantee |
|---|---|---|
| `lifecycle_memtier_read` | create (write_ref=1) → `release_write` → `take_read` → `release_read` | `no_active_refs` ∧ `inv_write_binary` |
| `lifecycle_downgrade` | create (write_ref=1) → `downgrade_reference` → `release_read` | `no_active_refs` ∧ `inv_write_binary` |
| `lifecycle_memtier_to_block` | create → `release_write` → `convert_to_storage` → `convert_memory_tier_to_block` | `is_block_device` ∧ `no_active_refs` |

## Fault-injection validation

Proofs were confirmed non-vacuous by perturbing mirror bodies and observing a
verification condition go red (`✘`); both were reverted afterward.

| Injected fault | Result |
|---|---|
| `take_write` sets `write_ref = 2` | `vc_take_write` ✘ — `inv_write_binary` is load-bearing |
| `release_read` drops the `read_ref == 0` guard | `vc_release_read` ✘ — the underflow guard is load-bearing |

## Known drift

- `promote_block_to_memory_tier`: the shipped function also sets
  `size_blocks = size.div_ceil(4096)` (`src/lib.rs:494`). Omitted in the mirror —
  `div_ceil` is a contractless external call (yields an impossible precondition,
  which would make the postcondition vacuously true), and `size_blocks` is not
  part of the location/ref invariant.
- Injecting a fault into the **shipped** function will not fail this proof; that
  is the residual mirror gap the line-faithfulness discipline and `DRIFT:`
  annotations guard against.
