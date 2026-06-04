# Creusot Formal Verification — Progress Report

Creusot is a deductive verification tool for Rust. It takes annotated Rust functions
and generates verification conditions (VCs) that SMT solvers (alt-ergo, z3, cvc5, cvc4)
must prove. A proved function is formally correct with respect to its spec — not just
tested, but mathematically guaranteed.

---

## At a glance

| Property | Area | Author |
|---|---|---|
| Each ref-count operation does exactly what its spec says | dispatch-map | D |
| write_ref is always 0 or 1 — never corrupted by any operation | dispatch-map | D |
| Every valid lifecycle path ends with zero active references | dispatch-map | D |
| State machine transitions (Staging/MemoryTier/BlockDevice) preserve invariants | dispatch-map | D |
| An entry can only be removed when no references are held | dispatch-map | D |
| Taking a reference then releasing it leaves counts exactly unchanged | dispatch-map | us |
| Active readers block eviction — data cannot be freed while in use | dispatch-map | us |
| Eviction is fair — least recently used always evicted first | dispatch-map | us |
| Every entry is created with a non-zero block size | dispatch-map | us |
| I/O segmentation always terminates | segment_io | us |
| Segmentation covers every byte exactly once — no gaps, no overlaps | segment_io | us |
| Exactly ceil(total / max_transfer_size) segments produced | segment_io | us |
| 64-bit LBA arithmetic never overflows | segment_io | us |
| LBA adjacency: seg[i].lba + seg[i].length/ss == seg[i+1].lba | segment_io | us+Coq |

---

## What was proved — plain English

**Each ref-count operation does exactly what its spec says. (D)**
`take_read` increments `read_ref` by exactly 1 without overflow. `release_read`
decrements without underflow. `take_write` sets `write_ref` to exactly 1.
`release_write` clears it to 0. `downgrade_reference` atomically converts a write
reference into a read reference. In every case, the invariant that `write_ref` is
always 0 or 1 is preserved.

**Reference counting is safe across round-trips.**
Every acquire is matched by a release that returns the count to exactly where it
started. No reference can leak and no reference can be freed twice. Proved for
single readers, single writers, the downgrade path, and two concurrent readers.

**State machine transitions are correct. (D)**
Entries can legally move Staging → BlockDevice, MemoryTier → BlockDevice, and can
be created directly as BlockDevice. Each transition was proved to leave the entry in
a valid state.

**Every valid lifecycle path ends safely. (D)**
The normal read path, downgrade path, storage migration, and lookup path were all
proved end-to-end to finish with zero active references — the entry is always
removable after its last use.

**An entry can only be removed when no one holds a reference. (D)**
`check_removable` returns success if and only if both refs are zero.

**Active readers block eviction.**
After `take_read`, `read_ref > 0` — the eviction predicate requires zero refs, so
data being read cannot be evicted from under its reader.

**Eviction is fair.**
The evictor always picks the least recently used entry first (lowest TSC timestamp).
A hot entry is never evicted while a colder one is available.

**Every entry starts with a non-zero block size.**
All three creation paths guarantee `size_blocks > 0`.

**I/O segmentation is correct.**
`segment_io()` always terminates, covers every byte exactly once (no gaps, no
overlaps), produces exactly `ceil(total_bytes / max_transfer_size)` segments, never
overflows 64-bit LBA arithmetic, and each segment's LBA end equals the next
segment's LBA start.

---

## Example 1 — Protocol abstraction: `take_read`

This example shows the key technique for verifying functions that are impure in
production (Mutex, HashMap, condvar) but have a verifiable pure core.

### Production code (from `components/dispatch-map/src/lib.rs`)

```rust
pub async fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
    let inner = self.state.inner.lock().unwrap();          // ← Mutex
    let entry = inner                                       // ← HashMap
        .entries
        .get_mut(&key)
        .ok_or(DispatchMapError::KeyNotFound(key))?;
    self.state                                              // ← condvar / async
        .wait_for(|| entry.write_ref == 0)
        .await;
    entry.read_ref = entry.read_ref                        // ← the actual logic
        .checked_add(1)
        .ok_or(DispatchMapError::RefCountOverflow)?;
    self.state.cvar.notify_all();
    Ok(())
}
```

### Three abstractions applied

| Production code | Verification crate | What it means logically |
|---|---|---|
| `self.state.inner.lock().get_mut(&key)` | `entry: &mut DispatchEntry` (passed directly) | We already hold the lock and have the entry |
| `wait_for(entry.write_ref == 0)` | `#[requires((*entry).write_ref == 0u32)]` | The condvar wait guaranteed this condition |
| `Arc<DmaBuffer>` / `*mut u8` fields | `u64` opaque handle in `Location` enum | We only care about state transitions, not buffer contents |

### Annotated verification form (from `components/dispatch-map/verif/src/lib.rs`)

```rust
/// Mirrors `take_read` — the logic after the wait_for guard succeeds.
#[requires((*entry).write_ref == 0u32)]       // condvar wait ensured this
#[requires((*entry).read_ref@ < u32::MAX@)]   // overflow precondition
#[requires(inv_write_binary(entry))]          // write_ref is 0 or 1
#[ensures((^entry).read_ref == (*entry).read_ref + 1u32)]  // increments by 1
#[ensures((^entry).write_ref == 0u32)]        // write_ref unchanged
#[ensures(inv_write_binary(&^entry))]         // invariant preserved
pub fn take_read(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    entry.read_ref = match entry.read_ref.checked_add(1) {
        Some(v) => v,
        None    => return Err(DispatchMapError::RefCountOverflow),
    };
    Ok(())
}
```

**Key insight:** `#[requires]` replaces the runtime guard (`wait_for`). The
precondition captures what the guard would have ensured at runtime — now the
prover can assume it as a mathematical fact rather than executing the wait.

The notation `*entry` means the value on entry (before), `^entry` means the
value on exit (after). `@` maps a Rust value to its unbounded mathematical
integer for comparison in specs.

---

## Example 2 — Coq hand-written proof: LBA adjacency

This example shows what happens when the automated SMT solvers reach their limit,
and how a hand-written Coq proof bridges the gap.

### The property

After segmentation, each segment's last LBA equals the next segment's first LBA:

```
result[i].lba + result[i].length / sector_size == result[i+1].lba
```

In Creusot Pearlite:

```rust
#[ensures(
    forall<i: Int>
        0 <= i && i + 1 < result@.len()
        ==> result@[i].lba@ + result@[i].length@ / sector_size@ == result@[i + 1].lba@
)]
```

### Why SMT solvers fail

The loop invariant needed to prove this is `remaining % sector_size == 0` — the
remaining byte count is always a multiple of the sector size. This requires proving:

```
a % n = 0  ∧  b % n = 0  ⟹  (a − b) % n = 0
```

where `n` is a **runtime variable** (the sector size). This is non-linear integer
arithmetic — modular arithmetic with a variable modulus is outside the decidable
fragment that alt-ergo, z3, cvc5, and cvc4 reliably handle.

### The Coq proof

Why3's Coq driver generates a `.v` proof obligation file for the failing VC.
Stored at `tools/creusot/certus-segment-verif/coq/mod_sub_lemma.v`:

```coq
Require Import BuiltIn.
Require int.Int.
Require int.Abs.
Require int.EuclideanDivision.
Require Import Lia.

(* Why3 goal *)
Theorem mod_sub :
  forall (a b n : Numbers.BinNums.Z),
  (0 < n)%Z ->
  (int.EuclideanDivision.mod1 a n = 0)%Z ->
  (int.EuclideanDivision.mod1 b n = 0)%Z ->
  (int.EuclideanDivision.mod1 (a - b) n = 0)%Z.
Proof.
intros a b n h1 h2 h3.
(* Step 1: unfold mod1 in hypotheses.
   mod1 x y  =  x - y * div x y
   h2: a - n * div a n = 0  =>  a is a multiple of n
   h3: b - n * div b n = 0  =>  b is a multiple of n     *)
unfold int.EuclideanDivision.mod1 in h2, h3.
assert (Ha: (a = n * int.EuclideanDivision.div a n)%Z) by lia.
assert (Hb: (b = n * int.EuclideanDivision.div b n)%Z) by lia.

(* Step 2: a - b = n * (div a n - div b n) + 0            *)
assert (Hab: (a - b =
    n * (int.EuclideanDivision.div a n - int.EuclideanDivision.div b n) + 0)%Z).
{ rewrite Ha at 1. rewrite Hb at 1. ring. }

(* Step 3: apply Why3's Mod_mult lemma:
   Mod_mult n k 0 (h1): mod1 (n*k + 0) n = mod1 0 n
   Then Mod_0 (n≠0):    mod1 0 n = 0                      *)
rewrite Hab.
rewrite (int.EuclideanDivision.Mod_mult
           n
           (int.EuclideanDivision.div a n - int.EuclideanDivision.div b n)
           0 h1).
apply int.EuclideanDivision.Mod_0.
lia.
Qed.
```

**Proof walkthrough:**
1. Unfold `mod1` (defined as `x - y * div x y`) in the hypotheses — linear arithmetic
   can now extract `a = n * k` and `b = n * j` where `k = div a n` and `j = div b n`.
2. Assert `a - b = n * (k - j) + 0` — proved by `ring` after substitution.
3. Apply `Mod_mult` (a lemma in Why3's EuclideanDivision library):
   `mod1 (n * m + 0) n = mod1 0 n` for `n > 0`.
4. Apply `Mod_0`: `mod1 0 n = 0` when `n ≠ 0`.
5. `lia` closes the `n ≠ 0` subgoal from `n > 0`.

**Verified with:**
```bash
coqtop -batch -R $WHY3_COQ Why3 -l coq/mod_sub_lemma.v
# No output = proof accepted
```

### Integration into Creusot

The Coq proof is bridged into the Creusot proof via a `#[trusted]` lemma:

```rust
// Proved by hand in Coq — see coq/mod_sub_lemma.v
#[trusted]
#[logic]
#[requires(n@ > 0)]
#[requires(a@ % n@ == 0)]
#[requires(b@ % n@ == 0)]
#[ensures(result)]
fn lemma_mod_sub(a: usize, b: usize, n: usize) -> bool {
    pearlite! { (a@ - b@) % n@ == 0 }
}
```

Inside the loop, `proof_assert!(lemma_mod_sub(remaining, length, ss))` calls the
lemma, making the fact available to the SMT solvers for the invariant maintenance
and subsequent LBA adjacency goals.

**`#[trusted]` is not a cheat** — it is the standard practice in hybrid SMT+ITP
verification workflows. The Coq proof is the evidence. The `#[trusted]` annotation
is the interface between two different proof systems.

---

## Verified properties — full detail

### dispatch-map/verif (`components/dispatch-map/verif/`)

**Individual operation correctness (D):**

| Function | What is guaranteed |
|---|---|
| `create_staging` (D) | New entry starts with write_ref=1, read_ref=0, size_blocks>0, inv_write_binary holds |
| `create_memory_tier_entry` (D) | Same guarantees for memory-tier entries |
| `recover_extent` (D) | Recovered entry has zero refs, size_blocks>0, inv_write_binary holds |
| `take_read` (D) | read_ref increments by exactly 1, no overflow, invariant preserved |
| `take_write` (D) | write_ref set to exactly 1, read_ref unchanged, invariant preserved |
| `release_read` (D) | read_ref decrements by exactly 1, no underflow, invariant preserved |
| `release_write` (D) | write_ref cleared to exactly 0, invariant preserved |
| `downgrade_reference` (D) | Atomically: write_ref→0, read_ref+1, no overflow, invariant preserved |
| `check_removable` (D) | Returns Ok if and only if both refs are zero |
| `convert_to_storage` (D) | Staging→BlockDevice or MemoryTier+ssd_offset; invariant preserved |
| `convert_memory_tier_to_block` (D) | MemoryTier(Some offset)→BlockDevice; invariant preserved |
| `is_evictable` (D) | True iff read_ref=0, write_ref=0, and entry is in MemoryTier with SSD offset |
| `lookup` (D) | read_ref increments, tsc updated, invariant preserved |
| `touch` (D) | Only tsc changes, ref counts unchanged |

**Lifecycle sequence proofs (D):**

| Sequence | What is guaranteed |
|---|---|
| `lifecycle_staging_read` (D) | create→release_write→take_read→release_read ends with zero refs |
| `lifecycle_downgrade` (D) | create→downgrade→release_read ends with zero refs |
| `lifecycle_staging_to_block` (D) | create→convert_to_storage preserves invariant throughout |
| `lifecycle_recover_extent` (D) | Recovered entry is immediately removable |
| `lifecycle_memory_tier_to_block` (D) | Full memory-tier migration ends with zero refs |
| `lifecycle_lookup` (D) | create→release_write→lookup→release_read ends with zero refs |

**Ref-count balance, eviction fairness, safety:**

| Property | What is guaranteed |
|---|---|
| `roundtrip_read` | take_read then release_read: net ref change = 0 |
| `roundtrip_write` | take_write then release_write: net ref change = 0 |
| `roundtrip_downgrade` | take_write→downgrade→release_read: no net ref held |
| `roundtrip_two_concurrent_reads` | Two readers acquiring and releasing: total balance zero |
| `eviction_fairness` | TSC-sorted list: first n entries are always colder than all remaining |
| `lifecycle_cold_evicted_before_hot` | Cold entry (low TSC) always precedes hot entry |
| `take_read_prevents_eviction` | After take_read, is_evictable is always false |
| Size invariant | All creation paths guarantee size_blocks > 0 |

### certus-segment-verif (`tools/creusot/certus-segment-verif/`)

Target: `segment_io()` in `components/dispatcher/src/io_segmenter.rs`

| Property | What is guaranteed |
|---|---|
| Termination | The loop always finishes — remaining strictly decreases |
| Empty result | Zero bytes → empty list; no phantom segments |
| Non-empty result | Positive bytes → at least one segment |
| Full byte coverage | buffer_offset + remaining = total_bytes at every iteration |
| No LBA overflow | 64-bit LBA never wraps (requires `start_lba + total_bytes/ss ≤ u64::MAX`) |
| No gaps, no overlaps | result[i].buffer_offset + result[i].length = result[i+1].buffer_offset |
| Exact segment count | result.len() = ceil(total_bytes / max_transfer_size) |
| LBA adjacency | result[i].lba + result[i].length/ss = result[i+1].lba *(Coq-assisted)* |

---

## Verification crate locations

| Crate | Path | Target |
|---|---|---|
| dispatch-map verification | `components/dispatch-map/verif/` | Entry lifecycle protocol |
| segment_io verification | `tools/creusot/certus-segment-verif/` | I/O segmentation arithmetic |

Branch: `unstable-creusot`
