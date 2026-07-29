# Worked Example: P4/P5 — Where Proof Stops and Fault-Injection Testing Begins

Purpose:
- A third worked property, following `P1_example.md` and `P12_P13_example.md`, chosen to make one point for a mixed **engineering + management** audience: **some guarantees are provable, and some must be *tested* — and a mature verification story says clearly which is which.**
- P4/P5 is the archetype of a guarantee that is **half-proved by Creusot and half-closed by targeted testing (fault injection).** The Creusot proof is real and green; it just has an honest, *named* boundary — and the right tool past that boundary is a fault-injection test, not more proof.
- Written so a reader who does not know Rust or formal methods can follow *why* the boundary exists and *what the test buys us*.

_Read `P1_example.md` first (it introduces the artifact layout, `#[ensures]`, VCs, `.coma`) and `P12_P13_example.md` (it introduces the ghost `FMap` and `*map`/`^map` before/after notation). This doc reuses both without re-explaining them._

---

## 0. The property in one line

> **P4 — "a successful `populate` really registers the entry."**
> **P5 — "a failed `populate` leaves nothing behind — no half-written, leaked entry."**

P5 is an **atomicity** claim: `populate` either fully succeeds or fully cleans up. There is no in-between state where the system has reserved a resource but forgotten about it.

## 1. What it means in plain terms

`populate` is the primary write path: a client hands the cache a key and a handle to data sitting in GPU memory, and the cache pulls that data into its fast DRAM tier. This is **not one action — it's three, in sequence:**

1. **Reserve** a slot in the DRAM memory-tier (evicting older entries first if the pool is full).
2. **Copy** the client's data from GPU into that reserved slot (a DMA transfer, then wait for it to finish).
3. **Register** the key in the dispatch-map (the index that records *where each key's data lives*), then enqueue the background SSD write-through.

The system now holds state in **two separate places**: the **memory-tier** (which owns the DRAM slot) and the **dispatch-map** (which owns the index entry). P4/P5 is about keeping those two in agreement:

- **P4 (success):** all three steps done ⇒ the dispatch-map has a `MemoryTier` entry for the key.
- **P5 (atomic failure):** if any step fails, the system must not leak. In particular it must never end up holding a **reserved DRAM slot that no index entry points to** — that slot would be occupied forever, invisible to eviction, a permanent capacity leak.

That "two places must stay in agreement, even when a step fails midway" is exactly the kind of property that is **easy to state, hard to fully prove, and natural to fault-inject.**

## 2. Where the property comes from (the spec)

`components/dispatcher/specs/001-dispatcher-cache-interface/spec.md`:

- **User Story 1 — the populate write path** (line 12): the narrative that populate reserves a slot, DMA-copies, and *"registers the entry in the dispatch map as a `MemoryTier` entry"*.
- **Acceptance #1** (line 20): new key ⇒ slot allocated, DMA copy performed, *entry registered in the dispatch map as MemoryTier*, success returned. *(P4)*
- **Acceptance #3** (line 22): key already exists ⇒ `AlreadyExists` error. *(a P5 failure path)*
- **Edge case** (line 206): *"When memory-tier insertion fails during populate … no dispatch map entry is created."* *(P5 — the no-leak statement, in the spec's own words)*
- **FR-003** (line 245): `populate` MUST allocate the mt slot, DMA-copy, *register in the dispatch map*, and enqueue write-through.
- **FR-049** (line 291): the related **ordering rule** — the dispatch-map must reflect a state change *before* the DRAM slot is freed, *"to prevent a race where concurrent lookups obtain a freed memory-tier pointer."* This is the spec explicitly flagging a **cross-map, concurrency-sensitive** hazard — the exact zone Creusot cannot reach.

## 3. The Rust code being proved (and its three phases)

**Repo pointer:** `components/dispatcher/src/lib.rs:2043–2082` — the real `populate`.

```rust
fn populate(&self, key: CacheKey, ipc_handle: IpcHandle) -> Result<(), DispatcherError> {
    self.ensure_initialized()?;
    if ipc_handle.size == 0 {
        return Err(DispatcherError::InvalidParameter("IPC handle size must be > 0".into()));
    }
    let size: u32 = ipc_handle.size;

    // Phase 1: Evict if needed and RESERVE a memory-tier slot (mt.insert inside).
    let _mem_ptr = self.reserve_memory(key, size)?;                    // (1)

    // Phase 2: Async DMA copy GPU -> reserved slot, then wait for it.
    self.copy_gpu_to_memory_async(key, ipc_handle, stream)?;          // (2a)  <-- can fail
    gpu.stream_synchronize(stream)
        .map_err(|e| DispatcherError::IoError(...))?;                 // (2b)  <-- can fail

    // Phase 3: REGISTER in dispatch-map + enqueue SSD write-through.
    self.copy_gpu_to_memory_completed(key, size)?;                    // (3)

    Ok(())
}
```

**Phase 3 has a rollback built in.** `components/dispatcher/src/lib.rs:2163–2211`:

```rust
fn copy_gpu_to_memory_completed(&self, key: CacheKey, size: u32) -> Result<(), DispatcherError> {
    // ... get dm, mt handles, get the reserved pointer ...
    dm.create_memory_tier_entry(key, mem_ptr, size)                   // register in dispatch-map
        .map_err(|e| match e {
            interfaces::DispatchMapError::AlreadyExists(k) => {
                let _ = mt.remove(key);        // <-- ROLLBACK: give the DRAM slot back
                DispatcherError::AlreadyExists(k)
            }
            other => {
                let _ = mt.remove(key);        // <-- ROLLBACK on any other dm failure
                DispatcherError::IoError(other.to_string())
            }
        })?;
    // ... downgrade read pin, enqueue background write-through ...
    Ok(())
}
```

So **Phase 3 is atomic by construction**: if the dispatch-map registration fails, the code immediately releases the DRAM slot (`mt.remove(key)`), leaving both maps clean. **This is the part P4/P5 proves.**

**The hazard lives in Phase 2, not Phase 3.** Look again at `populate`: if the DMA copy (2a) or the `stream_synchronize` (2b) fails, the `?` returns early — and by then **Phase 1 has already reserved the DRAM slot, but Phase 3 never runs, so nothing releases it.** There is no `mt.remove(key)` on that early-exit path. That is a **leaked memory-tier slot with no dispatch-map entry** — precisely the state P5 says must never persist.

## 4. The Creusot proof — what it does cover

**Repo pointer:** `components/dispatcher/verif/src/lib.rs:794–813` — function `register_memory_tier`.
**Artifact:** `components/dispatcher/verif/verif/dispatcher_verif_rlib/register_memory_tier.coma` (**2 VCs**, both green — see appendix).

```rust
#[check(ghost)]                                   // logic-only: proved, never executed
#[requires(!(*map).contains(key))]                // precondition: fresh key (slot reserved in Phase 1)
#[ensures(match result {
    // P4: success => the key is now present as a MemoryTier entry.
    Ok(_) => (^map).get(key) == Some(TierState::MemoryTier),

    // P5: failure => the dispatch-map is byte-for-byte unchanged, key still absent (no leak).
    Err(_) => (^map).ext_eq(*map) && !(^map).contains(key),
})]
pub fn register_memory_tier(
    map: &mut FMap<u64, TierState>,               // the ghost dispatch-map (has *map / ^map)
    key: u64,
    create_ok: bool,                              // did dm.create_memory_tier_entry succeed?
) -> Result<(), DispatcherError> {
    if create_ok {
        let _ = map.insert_ghost(key, TierState::MemoryTier);   // Phase-3 success: register it
        Ok(())
    } else {
        // Phase-3 failure: mt slot rolled back (in runtime), dispatch-map untouched.
        Err(DispatcherError::AllocationFailed)
    }
}
```

Read the contract with the `P12_P13_example.md` notation (`*map` = before, `^map` = after):

| Branch | Condition on the result | Plain meaning |
|---|---|---|
| `Ok(_)` | `(^map).get(key) == Some(MemoryTier)` | after success, the key is registered as a MemoryTier entry *(P4)* |
| `Err(_)` | `(^map).ext_eq(*map) && !(^map).contains(key)` | after failure, the dispatch-map is identical to before and the key is absent — **no partial/leaked entry** *(P5)* |

The proof discharges in **2 VCs**: `vc_register_memory_tier` (the main goal — both arms hold) and `vc_insert_ghost_u64` (the `insert_ghost` call is well-formed). Green replay = proved.

## 5. What this proof genuinely establishes — and its exact boundary

**Proves (real, not hand-waved):** the **dispatch-map-side decision of Phase 3** is atomic. On successful registration the entry appears; on a registration failure the dispatch-map is left `ext_eq` to its prior state — the strong, concrete "unchanged," not a proxy flag. Within Phase 3, P5's no-leak claim holds by proof.

**The boundary — stated plainly (this is the honest scope line):** the proof is a **single-map, sequential** model. It reasons about *one* ghost `FMap` (the dispatch-map). It deliberately does **not** cover:

1. **The Phase-2 leak (the big one).** The model's precondition is `!(*map).contains(key)` and its universe is one map. It cannot even *express* "a DRAM slot was reserved in the memory-tier but no dispatch-map entry exists," because the **memory-tier is a second component the model abstracts away entirely.** The `create_ok: bool` input collapses the whole GPU-copy-then-register sub-story into one boolean. So the reserve-succeeds-but-copy-fails hazard is invisible to this proof by construction.
2. **Cross-map (mt ↔ dm) consistency.** P4/P5 as proved is a claim about the dispatch-map alone. The real atomicity guarantee spans *two* maps, and their agreement under a mid-operation failure is a cross-map invariant (it lives with the P30/P31 track and assumption A7).
3. **Concurrency.** Creusot reasons sequentially; the lock is collapsed away. FR-049's "concurrent lookup grabbing a freed pointer" race is outside the model.

None of this is a defect in the proof — it is the proof being **precise about what it is**. But it means the property as a *whole* is not closed. Something else has to cover (1)–(3). That something is **targeted testing.**

## 6. Closing the gap: why *testing* — and specifically *fault injection*

Why not just prove more? Because the uncovered part is exactly the part formal deductive proof is worst at and testing is best at:

- The leak is a **multi-component, mid-operation-failure** interaction. Proving it would require a faithful formal model of the memory-tier, the GPU DMA engine, *and* their failure modes and interleavings — a modeling effort far larger than the property is worth, and one that would still rest on trusted axioms about the DMA/FFI boundary.
- A **fault-injection test drives the real code**: the real memory-tier, the real reserve/release, the real early-return path. It observes the *actual* resource, not a model of it. Where the proof abstracts the second map to a boolean, the test *inspects that second map directly.*

This is the reframing that matters for the reviewer objection *"in verification we don't verify performance statements."* We are **not** testing performance here. We are testing a **functional consequence of an abstraction the proof made** — "did a slot leak?" is a crisp yes/no state assertion, not a timing measurement. The test closes a **correctness** gap the proof named, on the same footing as the proof.

### The fault-injection test design

**Goal:** assert the atomicity guarantee (P5) on the paths the proof cannot see — the Phase-2 failure and the Phase-3 rollback *actually executing* against the real memory-tier.

**Mechanism:** a mock `IMemoryTier` / `IGpuServices` that can be told to fail a chosen step, plus a probe on the real memory-tier's occupied-byte count and the dispatch-map's key set.

| # | Inject a fault at… | Then assert (the invariant) | Which gap it closes |
|---|---|---|---|
| **T1** | Phase 2a — DMA copy returns error | `populate` returns `Err` **and** memory-tier used-bytes == baseline (slot released) **and** dispatch-map has no entry for `key` | **The Phase-2 leak** — the case the proof structurally cannot express |
| **T2** | Phase 2b — `stream_synchronize` returns error | same as T1 | same (the second early-exit on the Phase-2 path) |
| **T3** | Phase 3 — `create_memory_tier_entry` returns `AlreadyExists` | `populate` returns `AlreadyExists` **and** `mt.remove` fired (used-bytes back to baseline) **and** dispatch-map unchanged | Confirms the **rollback the proof asserts *as a decision*** truly frees the real DRAM slot (proof modeled the mt side as abstracted-away) |
| **T4** | Phase 1 — `mt.insert` fails (pool full after eviction) | `populate` returns `AllocationFailed` **and** dispatch-map has no entry (spec edge case, line 206) | The reserve-fails path — never reaches the ghost model's precondition |
| **T5** *(stretch)* | concurrent lookup during a Phase-3 rollback | no lookup ever observes a freed pointer (FR-049) | The **concurrency** boundary — Creusot is sequential |

T1/T2 are the headline: they exercise the leak that the green Creusot proof, by its own honest scope, says nothing about. T3/T4 turn the proof's *modeled decisions* into *executed behavior against the real resource*. T5 addresses the ordering race the spec itself calls out.

**What "pass" means:** the memory-tier's occupied-byte count returns to its pre-`populate` baseline on every failure path, and the dispatch-map never retains a key for a failed `populate`. That is P5, verified end-to-end on the real code — the half the proof could not reach.

## 7. The division of labor (the slide for management)

| Concern | Covered by | Why that tool |
|---|---|---|
| Phase-3 registration decision is atomic (success registers; failure leaves dm unchanged) | **Creusot proof** `register_memory_tier` (2 VCs, green) | Pure sequential decision over one map — proof gives an *all-inputs* guarantee, no test enumeration needed |
| Phase-2 failure leaks no DRAM slot (reserve-then-copy-fails) | **Fault-injection test** T1/T2 | Multi-component, mid-operation failure — the proof's single-map model cannot express it; the test inspects the real second resource |
| Rollback actually frees the real DRAM slot | **Fault-injection test** T3 | Proof models the mt side as a boolean; the test drives the real `mt.remove` |
| Reserve-fails path | **Fault-injection test** T4 | Below the proof's precondition |
| Concurrent freed-pointer race (FR-049) | **Fault-injection / stress test** T5 | Creusot reasons sequentially |

The headline for leadership: **we did not test because we failed to prove — we tested because the uncovered region is inherently a runtime, multi-component, failure-interleaving property, and a fault-injection test is the *correct instrument* for it.** The proof and the tests together make one honest claim; neither alone does.

## 8. Why P4/P5 represents a group

P4/P5 is the archetype of **"cross-map atomicity under fault"** — a guarantee that is (a) partly a clean sequential decision Creusot proves, and (b) partly a multi-component failure interaction that a fault-injection test closes. The same split applies to:

| Property group | Proved part (Creusot) | Test-closed part (fault injection) |
|---|---|---|
| **P4/P5** populate atomicity | Phase-3 dm registration decision | Phase-2 slot leak; rollback executes; concurrency |
| Eviction **P15/P16/P17** | attempt-budget bound; success ⇒ capacity predicate | the trusted `under_pressure` / `tier_used` oracles — drive real memory pressure and assert eviction makes progress |
| **P9** cold-promotion | BlockDevice→MemoryTier transition decision | the mt-reserve + SSD-read failure interleavings during promote |

So a reader who follows P4/P5 has the mental model for the whole "proof handles the decision, fault injection handles the multi-component failure" pattern — the pattern that answers *when* a verification effort should stop proving and start testing.

---

## 9. Implementation plan for the fault-injection tests (resume guide)

> This section is a **hand-off note to our future selves.** It records exactly what exists, what to build, and where, so the T1–T5 tests can be picked up cold — specifically once **node7** (the new GPU-equipped node) is provisioned. As of writing we run on **green**, which has no GPU.

### 9.1 What needs a GPU (node7) and what does not — read this first

The important planning fact: **T1–T5 are dispatcher *unit* tests against mocks. They do NOT need a GPU and can be written and run on green today.** They exercise the dispatcher's *orchestration and cleanup* logic (does a failed `populate` leak a slot?), with `MockMemoryTier` and `MockGpuServices` standing in for the real components. No CUDA, no DMA hardware.

node7 unlocks a **different, later tier** of validation, not these tests:

| Tier | What it validates | Needs GPU / node7? | Status |
|---|---|---|---|
| **A. Fault-injection unit tests (T1–T5)** | dispatcher orchestration: failed `populate` leaks no mt slot; rollback frees the real (mock) slot | **No** — runs on green now | to write |
| **B. Real-hardware end-to-end** | the *same* P4/P5 invariants with the **real** `IGpuServices` (actual GPU→DRAM DMA) and real memory-tier — confirms the mock faithfully modeled the hardware failure modes | **Yes — node7** | blocked on node7 |
| **C. Full certus integration suite** | cross-component behavior under real drivers | **Yes — node7** | blocked on node7 |

So the sequencing is: **do Tier A on green now** (it's the direct complement to the Creusot proof and needs nothing new from infra), then **on node7 re-assert the headline invariants at Tier B** by swapping `MockGpuServices` for the real DMA path and injecting faults at the hardware boundary (e.g. a GPU stream error). Tier B is where "we tested the real thing, not a model of it" becomes literally true.

### 9.2 What already exists in the repo (as of this revision)

All in `components/dispatcher/src/lib.rs`, test module `mod tests` at **:2483**. Line numbers drift — anchor on the symbol names.

- **The leak probe — `MockMemoryTier::used()` (:2626).** Returns occupied bytes; `remove` (:2603) decrements it, `insert` (:2554) increments it. The core assertion for every P5 test is `mt.used() == baseline`. Also `MockMemoryTier::contains(key)` (:2618).
- **An existing fault toggle — `MockMemoryTier::with_fail_insert()` (:2532).** Sets `fail_insert`, making `insert` return `PoolFull`. This is the Phase-1 fault, and the model for how to add the others.
- **A test to extend, not write — `populate_allocation_failure` (:3622).** Already wires the four mocks, calls `populate`, asserts `Err(AllocationFailed)`. It just never checks the leak → it is **T4 minus one assertion**.
- **An `AlreadyExists` populate test (ends ~:3618)** to model T3's error path from.
- **A wiring helper — `setup_initialized()`** (used e.g. at :3665) that builds a connected+initialized `DispatcherComponent`. Prefer it; fall back to the explicit builder in `populate_allocation_failure` when a test needs a custom mock instance.
- **The code under test — `populate` (:2043–2082):** Phase-1 reserve at :2057, **Phase-2 early-exit with no cleanup at :2067–2069 (the suspected leak)**, Phase-3 at :2073; the Phase-3 rollback (`mt.remove` on dm-register failure) at :2182–2192.

### 9.3 What needs building (small — the harness already carries the weight)

1. **Fault-injecting `MockGpuServices` (currently a unit struct at :3001).** Needed for T1/T2. Add toggles mirroring `with_fail_insert`:

   ```rust
   struct MockGpuServices { fail_copy: AtomicBool, fail_sync: AtomicBool }
   impl MockGpuServices {
       fn ok() -> Self        { Self { fail_copy: AtomicBool::new(false), fail_sync: AtomicBool::new(false) } }
       fn fail_copy() -> Self { Self { fail_copy: AtomicBool::new(true),  fail_sync: AtomicBool::new(false) } }
       fn fail_sync() -> Self { Self { fail_copy: AtomicBool::new(false), fail_sync: AtomicBool::new(true)  } }
   }
   // in dma_copy_to_host (:3028): if self.fail_copy.load(Relaxed) { return Err("injected DMA failure".into()); }
   // in stream_synchronize (called from populate :2068): if self.fail_sync.load(Relaxed) { return Err("injected sync failure".into()); }
   ```
   Note: every existing `MockGpuServices` construction site (grep for `MockGpuServices` — e.g. :3142, :3626, integration.rs:72) must switch to `MockGpuServices::ok()`. Confirm `stream_synchronize` is on the `IGpuServices` trait and add the toggle there.

2. **Fault toggle on `MockDispatchMap::create_memory_tier_entry`** (impl `IDispatchMap` at :2714). Needed for T3 — make it return `DispatchMapError::AlreadyExists(key)` on demand so Phase-3's rollback branch (:2182–2192) actually executes.

### 9.4 The five tests, spec'd for direct implementation

Each is a `#[test] fn` in `mod tests`. Shape: wire mocks → inject one fault → `populate` → assert **error variant AND no-leak invariant**. The no-leak assertion (`mt.used()`/`!mt.contains`) is the P5 content; the error-variant assertion alone would only be an error test.

| Test | Inject | Expected error | **P5 invariant to assert** | Expected result today |
|---|---|---|---|---|
| **T1** `populate_dma_failure_leaks_no_slot` | `MockGpuServices::fail_copy()` | `Err(IoError)` | `mt.used()==0` && `!mt.contains(key)` && dm has no entry | **May FAIL** — Phase-2 `?` at :2067 has no `mt.remove`. A failing test here is the *deliverable*: it pins the leak. |
| **T2** `populate_sync_failure_leaks_no_slot` | `MockGpuServices::fail_sync()` | `Err(IoError)` | same as T1 | same as T1 (other Phase-2 exit, :2068–2069) |
| **T3** `populate_dm_register_failure_rolls_back` | `MockDispatchMap` create → `AlreadyExists` | `Err(AlreadyExists)` | `mt.used()==0` && `!mt.contains(key)` | **PASS expected** — rollback at :2185 fires; this test *confirms the decision Creusot proved actually frees the real slot* |
| **T4** `populate_allocation_failure` (extend existing :3622) | `with_fail_insert` | `Err(AllocationFailed)` | add `assert_eq!(mt.used(), 0)` | PASS expected (never reserved) |
| **T5** `populate_rollback_no_freed_pointer_race` | concurrent lookup during a T3-style rollback | — | no lookup observes a freed pointer (FR-049) | **Separate effort** — threaded stress test; no GPU needed but heaviest to write. Defer. |

Illustrative T1 (uses the mocks from §9.3):

```rust
#[test]
fn populate_dma_failure_leaks_no_slot() {              // P5 on the Phase-2 path the proof can't see
    let mt  = Arc::new(MockMemoryTier::new(1024 * 1024));
    let gpu = Arc::new(MockGpuServices::fail_copy());    // inject the Phase-2 DMA fault
    let (c, _dm) = setup_with(mt.clone(), gpu);          // wire four mocks + initialize (see note)
    let d = query_interface!(c, IDispatcher).unwrap();

    let mut buf = vec![0u8; 4096];
    let err = d.populate(1, make_handle(&mut buf));

    assert!(matches!(err, Err(DispatcherError::IoError(_))));
    assert_eq!(mt.used(), 0, "P5 VIOLATED: Phase-2 failure leaked a memory-tier slot");
    assert!(!mt.contains(1), "orphaned mt slot with no dispatch-map entry");
}
```

Note: `setup_initialized()` likely constructs its own mocks internally; T1–T3 need to *inject* specific mock instances, so either add a `setup_with(mt, gpu)` variant or use the explicit `DispatcherComponent::new(...).connect(...)` builder from `populate_allocation_failure` (:3629–3648). Prefer adding `setup_with` to avoid copy-pasting the 14-field constructor five times.

### 9.5 How to run

```bash
# Tier A (green, now): fault-injection unit tests — no GPU required
cargo test -p dispatcher populate_          # runs all populate_* tests incl. T1–T4
cargo test -p dispatcher                     # whole dispatcher unit suite

# Tier B (node7, later): real-GPU end-to-end — swap MockGpuServices for the real IGpuServices,
# inject a fault at the hardware boundary, assert the same mt.used() invariant.
# (New integration test under components/dispatcher/tests/, gated to run only where a GPU is present.)
```

### 9.6 Open decisions to settle when we pick this up

1. **Is the T1/T2 leak a real bug or accepted behavior?** If Phase-2 failure genuinely leaks a slot, either (a) add a cleanup guard (`mt.remove(key)` on the Phase-2 early-exit in `populate`, :2067–2069), making T1/T2 pass, or (b) document it as accepted (e.g. a background reaper reclaims orphaned slots) and assert *that* instead. This is a design call, not a test call — surface it to the team.
2. **Error-variant mapping.** Confirm Phase-2 failures surface as `IoError` (from the `stream_synchronize` `map_err`, :2069) and adjust T1/T2's `matches!` if the DMA-copy path (:2067) yields a different variant.
3. **Tier B fault mechanism on node7.** Decide how to inject a hardware-boundary fault with the real GPU (e.g. an env-var-gated failpoint in the real `IGpuServices` DMA wrapper) without shipping test hooks into production paths.

---

## Appendix — inspecting this proof yourself

Same recipe as `P1_example.md`; use `register_memory_tier` in the paths:

```bash
cd components/dispatcher/verif

# What was proved (expect 2 VCs, both alt-ergo, green):
python3 -c "import json;d=json.load(open('verif/dispatcher_verif_rlib/register_memory_tier/proof.json'));[print(k,'->',v['prover'],v['time'],'s') for k,v in d['proofs']['Coma'].items()]"
# -> vc_insert_ghost_u64   -> alt-ergo 0.015 s
# -> vc_register_memory_tier -> alt-ergo 0.018 s

# Re-check it still holds (what CI runs):
why3find prove -r --summary verif/dispatcher_verif_rlib/register_memory_tier.coma
# -> Proved (…/register_memory_tier.coma) ✔

# See the obligation, incl. the P4/P5 before/after contract at the tail:
cat verif/dispatcher_verif_rlib/register_memory_tier.coma
```

The runtime code the fault-injection tests would target:
- `populate` three-phase flow: `components/dispatcher/src/lib.rs:2043–2082`
- Phase-3 rollback (`mt.remove` on dm-register failure): `components/dispatcher/src/lib.rs:2182–2192`
- The Phase-2 early-exit with no cleanup (the leak): `components/dispatcher/src/lib.rs:2067–2069`
