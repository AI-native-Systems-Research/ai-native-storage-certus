# Background: What Each Property Really Means

Audience:
- You have a general CS background (you know what a hash map, a cache, and an invariant are).
- You want to understand what properties P1–P31 mean, why each is needed, and — by reading them in order — how the Certus dispatcher is designed.

This file is a **teaching companion** to `properties_to_prove.md`. That file is the authoritative registry (IDs, status, proof evidence). This file is the plain-English "why".

---

## 1. Background — a five-minute tutorial

This section gives two mental models: the **storage system** (what Certus does) and the **verification stack** (how we prove it correct). A reader can understand the properties from the first alone, but the second explains *what "proved" actually means* in this project — useful if you're reading the coverage numbers or writing about the work.

### 1.1 The storage mental model

Certus is an **AI-native tiered storage cache**. Think of it as a smart key→data map where the data for a key can physically live in one of several **tiers**, and the system moves data between tiers to balance speed and capacity.

The key→data map is called the **dispatch-map**. The orchestrator that answers API calls (`populate`, `lookup`, `remove`, …) and moves data between tiers is the **dispatcher**.

**The tiers (where a key's data can be):**
- **MemoryTier** — fast tier (host memory / cache). Limited capacity. This is where "hot" data lives.
- **BlockDevice** — slow, large tier (SSD / disk). This is where "cold" data lives after eviction.
- **Staging** — a *legacy* transitional tier from an older design. Newer specs no longer emphasize it (see P10, P20–P24).

**The core operations (the dispatcher's public API):**
- `initialize` — wire up dependencies before anything else can run.
- `populate(key, data)` — insert a new key, placing its data in MemoryTier.
- `lookup(key)` — read a key's data (possibly copying it out to a GPU buffer).
- `check(key)` — does this key exist? (no data movement).
- `remove(key)` — delete a key.
- `touch(key)` — mark a key as recently used (for eviction ordering).
- **eviction** — when MemoryTier is full, move the least-useful entries to BlockDevice to free space.
- `clear_memory_tier` — flush the whole fast tier.
- **recovery** — after a restart, rebuild the map from what's persisted on disk (**extents**).

**Two recurring themes** run through almost every property:
1. **Consistency of the map** — the map must always tell the truth about what exists and where it lives.
2. **Atomicity on failure** — if an operation fails, it must leave *no partial mess* behind (no half-inserted key, no leaked capacity, no dangling state).

**Two vocabulary words you'll meet:**
- **Extent** — a contiguous region of storage on the BlockDevice that holds a key's persisted data. Recovery reads extents to rebuild the map.
- **Reference count (refcount)** — how many active readers/writers currently hold a key. You cannot evict or delete a key someone is actively using.

**Ownership shorthand** (who is responsible for proving each property):
- **`IDispatcher`** — the system-level API and orchestration behavior.
- **`IDispatchMap`** — the per-key state machine and map-wide invariants.
- **shared** — both are needed; one is picked as the primary owner.

### 1.2 The verification stack: from Rust to a machine-checked proof

We use **Creusot**, a deductive verifier for Rust. You annotate functions with a specification and Creusot proves the code meets it:

- `#[requires(...)]` — a **precondition** the caller must satisfy.
- `#[ensures(...)]` — a **postcondition** the function guarantees on return.
- `#[invariant(...)]` — a **loop invariant**: a fact true on every iteration.
- `#[variant(...)]` — a **termination witness**: a quantity that strictly decreases, proving the loop ends.

These are written in **Pearlite**, Creusot's specification language (logic-level Rust: `x@` is the mathematical view of a value, `*x`/`^x` are the initial/final states of a mutable borrow, `forall<i: Int> ...` quantifies).

Creusot compiles the annotated Rust into a set of **verification conditions (VCs)** — logical formulas that are *valid* (always true) exactly when the code satisfies its spec. It emits these to **Why3**, a proof platform that hands each VC to backend provers. If every VC is discharged, the function is proved.

### 1.3 SAT vs SMT provers — and why the distinction matters here

The backend provers that do the automated work are **SMT solvers**. To understand what they can and can't do, start one level down:

- **SAT solvers** decide **propositional (Boolean) satisfiability**: variables are just true/false, joined by AND/OR/NOT. Modern SAT solvers (using CDCL — conflict-driven clause learning) are astonishingly fast, but they speak *only* Boolean logic. They have no built-in notion of an integer, an array, or a function.
- **SMT solvers** — **Satisfiability Modulo Theories** — put a SAT engine on top of **background theories**: linear integer/real arithmetic, fixed-size bitvectors, arrays, uninterpreted functions, algebraic datatypes. The architecture (**DPLL(T)**) lets the Boolean SAT core enumerate structure while dedicated *theory solvers* decide facts like `x + 3 <= y` or `select(store(a,i,v),i) = v`, exchanging lemmas as they go. The solvers we rely on are **Z3, CVC5, CVC4, and Alt-Ergo**.

Why this matters for Certus: **almost all** of our VCs — bounds checks, membership, linear arithmetic, map insert/remove reasoning — fall inside decidable SMT theories, so the solvers discharge them **automatically**. That automation is what makes verifying real Rust practical. The wall you eventually hit is **nonlinear arithmetic** — in particular **divisibility/modulo with a *variable* divisor** — which is undecidable in general. There the solvers time out or answer "unknown," and you must prove the fact yourself.

### 1.4 When SMT gives up: proving a lemma by hand in Coq

There is a real instance of this in the repo, at `tools/creusot/certus-segment-verif` (mirroring `components/dispatcher/src/io_segmenter.rs`):

`segment_io()` splits a large I/O transfer into device-sized segments (each within the NVMe **maximum data transfer size**). Its Creusot contract proves a genuinely strong specification — **17/17 VCs discharged** after three rounds of strengthening invariants:
- it **terminates** (via a `#[variant]` on the remaining byte count);
- the result is empty **iff** the transfer is zero bytes;
- the segments **cover the transfer with no gaps and no overlaps**;
- the segment count is **exactly** `ceil(total_bytes / max_transfer_size)`;
- **LBA adjacency**: each segment's device address begins exactly where the previous ended.

But one small fact defeats *every* SMT backend: *"if `a` and `b` are both multiples of `n`, then `a − b` is a multiple of `n`"* — for a **variable** divisor `n`. That is nonlinear divisibility reasoning, outside what Z3/CVC5/CVC4/Alt-Ergo can find.

The resolution shows the standard escape hatch:
1. State the fact as a logic lemma, `lemma_mod_sub`, and mark it `#[trusted]` in Creusot (Creusot accepts it without proving it).
2. Discharge it **by hand in Coq** — an *interactive theorem prover* — in `coq/mod_sub_lemma.v`. Why3's Coq driver emits the goal; a human writes the proof: unfold Euclidean mod (`mod1 x n = x − n·div x n`), rewrite `a − b = n·(div a n − div b n) + 0`, then apply the library lemmas `Mod_mult` and `Mod_0`. A few tactic lines a person can see but the automated search cannot.

There is also a **trust dividend** here. An SMT solver is a **trusted oracle**: when it says "valid" you generally believe it without a checkable certificate. Coq instead builds an explicit **proof term checked by a tiny, heavily-scrutinised kernel** (the *de Bruijn criterion*). So dropping to Coq for that lemma both closes a gap SMT could not *and* raises assurance for it. The cost is that it is manual — so we do it only where automation genuinely fails.

### 1.5 Two kinds of "we didn't prove this from first principles": assumptions vs trusted boundaries

No real verification proves *everything* from nothing. Certus tracks its gaps honestly in `assumptions_and_trusted.md`, split into two categories that map onto standard **assume-guarantee** reasoning and the notion of a **Trusted Computing Base (TCB)**:

**Model-level assumptions (A1–A7)** — conditions our *model* takes as given about the world or the abstraction (not about the prover). In plain terms:
- **A1** — some global claims use a **bounded key-space** model instead of a fully unbounded map (affects the map-wide invariants P30/P31).
- **A2** — liveness/progress claims assume the scheduler is **fair**.
- **A3** — some "eventually" claims are proved for a **bounded number of steps**, not as fully general temporal theorems.
- **A4** — background write-through **faults are abstracted** to plain success/failure.
- **A5** — the **async stream model is coarse** (few states, reduced concurrency realism).
- **A6** — **arithmetic preconditions are made explicit** to keep solver goals tractable — this is exactly why `segment_io` carries `#[requires]` clauses like "LBA advancement must not overflow `u64`."
- **A7** — many **map-wide** properties are inferred from **per-entry proofs plus composition** rather than a single whole-map theorem — this *is* the `L1`→`L2` gap. (The `L0`–`L3` scale itself is a project-local shorthand; see the note in `properties_to_prove.md`.)

Each assumption is a place where a careful reader should ask "is this abstraction faithful to reality?"

**Proof-level trusted boundaries** — specific lemmas or primitives accepted without a machine-checked discharge *inside the main proof*:
- `lemma_mod_sub` (above) — `#[trusted]` in Creusot, but justified **externally in Coq**. This is the best case: the trust is *transferred* to a Coq-checked artifact, not just assumed.
- `creusot_std::logic::FMap` ghost primitives (`insert_ghost`, `remove_ghost`) — `#[trusted]` in creusot-std, needed because the standard-library `HashMap` has **no Creusot extern specs** for insert/remove.
- Legacy stale lemmas (`lemma_same_slot_*`, the `p21_*` consume-once tails) — tied to the **removed** pending-write workflow, kept only as historical context.

The union of *(SMT solver soundness) + (trusted lemmas) + (model assumptions)* is this project's **TCB** — everything that must be correct for the proofs to mean what we claim. Good verification hygiene is keeping that TCB **small and explicit**, which is the whole purpose of `assumptions_and_trusted.md`.

---

## 2. The properties, grouped by design area

Each entry below gives: **What it says**, **Why it's needed**, and **What breaks without it**.

### A. Initialization — "nothing works until you're set up"

**P1 — Initialize fails if dependencies are missing, succeeds when they're bound.**
- *What it says:* `initialize` must reject startup when a required dependency (e.g. a backing device or config) is absent, and only succeed when everything it needs is present.
- *Why:* Half-initialized systems are a classic source of subtle corruption. This makes startup all-or-nothing.
- *Without it:* The dispatcher might come "up" while secretly missing a device, then fail unpredictably deep inside a later operation.

**P2 — Operational APIs fail with `NotInitialized` before init succeeds.**
- *What it says:* If you call `lookup`/`populate`/etc. before initialization has completed, you get a clean `NotInitialized` error — not a crash, not garbage.
- *Why:* Defines a hard gate: the system has exactly two phases (before/after init), and the boundary is explicit and safe.
- *Without it:* Early calls could touch uninitialized memory or return meaningless results.

### B. Insertion (`populate`) — "adding a key must be clean"

**P3 — Duplicate insertion fails with `AlreadyExists` and does not mutate existing data.**
- *What it says:* Inserting a key that already exists is rejected, and — crucially — the *existing* value is left untouched.
- *Why:* Prevents silent overwrites and guarantees the caller's existing data is safe even on a mistaken re-insert.
- *Without it:* A retry or race could clobber good data.

**P4 — Successful populate creates a correct MemoryTier entry.**
- *What it says:* When `populate` returns success, the key genuinely exists in MemoryTier with the right data/metadata.
- *Why:* "Success" must mean what it says — the entry is actually there and well-formed.
- *Without it:* Callers would trust a success that didn't really persist the entry.

**P5 — Populate failures are atomic (no partial leaked entry).**
- *What it says:* If `populate` fails partway (e.g. out of capacity mid-insert), it must leave *no* trace — no half-created entry, no leaked memory reservation.
- *Why:* This is the atomicity theme applied to insertion. Partial inserts are worse than clean failures because they corrupt the map quietly.
- *Without it:* Failed inserts would accumulate ghost entries and leak capacity over time.

### C. Reading (`check`, `lookup`) — "the map tells the truth, reads don't corrupt"

**P6 — `check(key)` matches actual membership in the map.**
- *What it says:* `check` returns "present" **iff** the key is really in the dispatch-map. No lies in either direction.
- *Why:* `check` is the cheap membership query; everything downstream trusts it.
- *Without it:* Callers could skip a needed `populate`, or redundantly insert, based on a wrong answer.

**P7 — Lookup on a missing key returns `KeyNotFound` and preserves state.**
- *What it says:* Reading a key that isn't there gives a clean `KeyNotFound` **and** changes nothing.
- *Why:* Reads of absent keys are normal (cache misses). They must be side-effect-free.
- *Without it:* A miss could accidentally create or mutate state.

**P8 — MemoryTier hit preserves the key and refreshes eviction metadata.**
- *What it says:* A successful read from the fast tier keeps the entry in place and updates its "recently used" info so eviction treats it as hot.
- *Why:* This is what makes the cache behave like a cache — using data keeps it hot.
- *Without it:* Hot data could be evicted as if it were cold, destroying cache performance.

**P9 — BlockDevice hit promotes the entry back to MemoryTier.**
- *What it says:* If the data was cold (on SSD) and you read it, the system pulls it back up into the fast tier ("promotion").
- *Why:* Recently-read cold data is likely to be read again; promotion restores fast access.
- *Without it:* Repeated reads of cold data would keep paying the slow-tier cost.

**P10 — Legacy staging lookups are safe if encountered.**
- *What it says:* Even though Staging is a legacy tier, if a lookup ever hits a staged entry, it must behave safely.
- *Why:* Defense-in-depth for a deprecated path that may still exist in old state. Marked `legacy`.
- *Without it:* Old staged entries could trigger undefined behavior after the design moved on.

**P11 — Size mismatch hard-fails with `InvalidParameter` and copies nothing.** *(a data-integrity keystone)*
- *What it says:* If a caller's buffer size doesn't match the stored object's size, the lookup must **refuse outright** — no "copy as much as fits" partial copy.
- *Why:* A silent partial copy hands the caller **truncated or mismatched data** while reporting success — a data-integrity disaster. (A tester found exactly this class of bug in an earlier implementation, which drove design changes.)
- *Without it:* Callers get corrupted data that looks valid.
- *How today's design actually ensures it:* Two layers, and it's worth being precise about them. (1) On the reachable product path the lookup copy is **clamped to `min(requested, stored)`**, so it can never read past either buffer — no over-copy. (2) There is also an explicit "size mismatch → `InvalidParameter`, copy nothing" arm, **but in the current code it is defensive and unreachable**: the map-level `lookup` is *key-only* — it never compares sizes, so it never actually reports a mismatch (only a test mock injects one). The real-world guarantee therefore rests on an **invariant that the requested size always equals the stored size at lookup**. Confirming where that invariant comes from (the source of `ipc_handle.size` vs the stored entry size) is an open follow-up; until then it's a reasonable assumption rather than a proved fact. So P11 is best read as *"the copy path is size-safe, and mismatch handling is present but currently a can't-happen guard"* — not as an open live bug.

### D. Deletion and touch — "removes are exact, touch is harmless"

**P12 — Successful remove guarantees the key is absent afterward.**
- *What it says:* After `remove` returns success, the key is genuinely gone from the map.
- *Why:* "Removed" must mean removed everywhere, not just from one tier's view.
- *Without it:* A "removed" key could linger and be read back — a use-after-delete.

**P13 — Remove on an absent key returns `KeyNotFound` with no mutation.**
- *What it says:* Deleting something that isn't there is a clean no-op error.
- *Why:* Idempotent, side-effect-free failure — same discipline as P7.
- *Without it:* A redundant delete could corrupt neighboring state.

**P14 — Touch refreshes metadata on an existing key; `KeyNotFound` on an absent one.**
- *What it says:* `touch` updates recency for a present key and cleanly errors for an absent key — and never *creates* anything.
- *Why:* `touch` is a hint, not a mutation of contents. It must not have surprising creation semantics.
- *Without it:* Touching a missing key might accidentally create a phantom entry.

### E. Eviction — "making room must be bounded, honest, and non-destructive"

**P15 — The eviction loop has a bounded attempt budget.**
- *What it says:* When the system tries to free space, its retry loop is guaranteed to terminate within a fixed number of attempts.
- *Why:* Termination. An unbounded eviction loop could hang the whole system under memory pressure.
- *Without it:* A pathological workload could spin the evictor forever (a liveness failure).

**P16 — Eviction success implies enough capacity was actually freed.**
- *What it says:* If eviction reports success, the requested amount of space is genuinely available now.
- *Why:* The caller (usually `populate`) relies on this to proceed safely.
- *Without it:* A "successful" eviction could be followed by an out-of-space failure anyway.

**P17 — Eviction failure implies the capacity target was not reached.**
- *What it says:* The converse of P16 — if eviction reports failure, it's because it truly couldn't free enough (e.g. everything is pinned by active readers).
- *Why:* Success/failure must be a truthful signal, not a guess.
- *Without it:* Callers couldn't trust the failure and might proceed into an unsafe state.

**P18 — Clean eviction *transitions* an entry MemoryTier→BlockDevice (does not delete it).**
- *What it says:* Evicting hot data doesn't throw it away — it demotes it to the slow tier, preserving the data.
- *Why:* Eviction is about *capacity*, not *deletion*. The key still exists; only its location changes. (This is why P9 promotion is possible later.)
- *Without it:* Eviction would silently lose data the caller still owns.

**P19 — Blind eviction fallback leaves no dangling map state.**
- *What it says:* If the system falls back to a last-resort ("blind") eviction, it must still leave the map consistent — no half-moved entries.
- *Why:* Even the emergency path must respect the atomicity theme.
- *Without it:* Emergency eviction under pressure would be exactly when corruption creeps in.
- *What it means to prove this one correctly — a worked example of the L1→L2 gap.* P19 is a good property to slow down on, because "leaves no dangling map state" is deceptively small and shows exactly where a proof's *scope* matters as much as its greenness.

  **First, the two data structures.** Eviction juggles two separate objects, and the whole property is a claim about the *relationship between them*:
  - **`mt` — the memory tier** (`IMemoryTier`): the in-DRAM cache. It owns the actual memory slots holding cached bytes. `mt.evict_lru_for_key(k)` frees a slot; `mt.used()`/`capacity()` are physical occupancy.
  - **`dm` — the dispatch-map** (`IDispatchMap`): the metadata index. Per key it records *where* the data lives (`MemoryTier` vs `BlockDevice`) and its refcount. `dm.convert_memory_tier_to_block(k)` flips a key's recorded tier; `dm.remove(k)` drops the entry.

  A resident key normally has an entry in **both**: `dm` says "key K is in MemoryTier" *and* `mt` holds K's bytes. "Dangling map state" is precisely these two drifting apart — `mt` frees K's slot but `dm` still claims K is resident in memory. A later `lookup(K)` would then trust `dm`, go to the memory tier, and read a slot that no longer holds K's data: a logical use-after-free.

  **The actual fallback logic** (dispatcher `evict_for_space`, `lib.rs:572-583`) is one small decision after `mt` frees the slot: try `dm.convert_memory_tier_to_block(K)`; if that fails, `dm.remove(K)`. So K must end up **either** a `BlockDevice` entry (demoted, data preserved on SSD — this is why P18/P9 promotion still works) **or** absent from `dm` — but *never* left as `MemoryTier`.

  **What we actually proved (L1, `Partial`).** We model just `dm` as a per-entry state map and prove that local decision: from a `MemoryTier` precondition, convert-success ⇒ the entry is now `BlockDevice`, convert-failure ⇒ the entry is gone, and in both arms it is never left `MemoryTier`. That is a faithful, machine-checked statement of the fallback's *intent* — but it is a **single-map, single-key, sequential** statement.

  **What a full discharge would additionally require, and why it's harder:**
  1. **It is fundamentally a cross-map (`mt` ↔ `dm`) invariant.** "No dangling state" is a relationship between two structures — *for every key, `mt`-residency agrees with the `dm` tier-state* — so it must be stated and maintained across the **whole map**, quantified over all keys. That is not a local per-entry fact; it is the map-wide (`L2`) theorem that P30 (each key in exactly one state) and P31 (refcount/state consistency) are about. In our abstraction-level shorthand this is the **L1→L2 lift**: supplying the map-wide gluing invariant that ties per-entry decisions to a whole-system guarantee (assumption **A7**).
  2. **The genuinely dangerous case is concurrent — and sits outside the prover.** The code's own comment warns "another thread may have concurrently evicted this key" (`lib.rs:566`). The corruption window is a *race* between `mt` freeing a slot and `dm` being updated. Creusot proves properties of the **sequential** skeleton (§1.2–1.3); it has no model of thread interleavings. So even a perfect single-threaded proof here certifies the decision logic, **not** the interleaving that is the real hazard — the same boundary described for background write-through (liveness/concurrency ⇒ runtime tests or a model checker, not deductive proof).

  The lesson worth capturing for the write-up: a green proof is only as strong as its *scope*. Here the cheap L1 proof is honest and useful (it pins down the convert-or-remove intent), but the property's real weight — a cross-map invariant maintained under concurrency — lives in the P30/P31 map-wide work and, for the racy part, on the runtime-test side of the line. We deliberately did the L1 proof first (a clean, checkable statement of intent) and then fold its meaning into P30/P31 rather than pretend the local proof closes the system-level property.

### F. Legacy direct-store workflow (P20–P24) — "an older design, kept honestly"

These describe a **removed** "direct-store / pending-write" workflow (prepare → commit/cancel). The design moved on (commit `25a7273` removed the API). They are retained for honesty and history — see `spec_drift_report.md`.

**P20 — Zero-size direct-store input is rejected safely.** *(still valid as a guard)*
- *What it says:* A zero-size write request is rejected up front.
- *Why:* Zero-size operations are almost always a caller bug; rejecting them early prevents degenerate states. The guard logic survived and was re-anchored to `populate`, so the proof is still live even though the original requirement is `legacy`.

**P21 — Prepare/commit/cancel is a "consume-once" protocol.** *(Stale)*
- *What it says:* A pending write, once prepared, can be committed or cancelled exactly once.
- *Why:* Prevents double-commit / double-cancel races. The proof is formally correct but mirrors **removed** code, so it's `Stale` — historical evidence, not an active guarantee.

**P22 — Commit ends in BlockDevice and clears the pending write.** *(Retired)*
**P23 — Cancel removes the key and clears the pending write.** *(Retired)*
- *What they said:* The two terminal outcomes of the old workflow left the system in a clean, defined state.
- *Status:* The workflow was removed entirely, so these are `Retired` (no active artifact).

**P24 — Commit/cancel with no pending write returns `KeyNotFound`, state preserved.** *(Stale)*
- *What it said:* Terminal ops on a non-existent pending write fail cleanly.
- *Status:* `Stale` for the same reason as P21.

> **Reading note:** `legacy` = the *requirement* is obsolete (P20 requirement is legacy, but its guard proof stays valid). `Stale` = the *proof* mirrors code that was removed (P21/P24). `Retired` = the property is gone entirely (P22/P23). These are tracked separately so no label overstates the guarantee.

### G. Bulk clear — "flushing the fast tier is complete and honest"

**P25 — `clear_memory_tier` leaves no MemoryTier entries.**
- *What it says:* After a clear, the fast tier is genuinely empty.
- *Why:* Callers (e.g. shutdown, reconfiguration) rely on a clean slate.
- *Without it:* Stragglers could survive a "clear" and cause stale reads.

**P26 — The count `clear_memory_tier` returns equals the number actually cleared.**
- *What it says:* The reported number of cleared entries is exactly correct.
- *Why:* Callers use this count for accounting/metrics; an off-by-N is a silent bookkeeping bug.
- *Without it:* Capacity accounting drifts from reality.

### H. Recovery — "after a restart, rebuild the truth from disk"

**P27 — Recovery recreates map entries consistent with the persisted extents.**
- *What it says:* On restart, the dispatch-map is rebuilt so that it exactly matches what's actually stored on disk (the extents).
- *Why:* This is durability. The in-memory map is volatile; disk is the ground truth. Recovery must reconcile them without inventing or dropping keys.
- *Without it:* A restart could resurrect deleted keys or lose live ones — the map would lie about what's really on disk.

### I. Pure arithmetic and configuration (P28, P29) — "the math is deterministic and correctly oriented"

These are the "cheapest to prove" properties — pure functions, no ghost-map machinery.

**P28 — The drive-selection formula is deterministic and stable.**
- *What it says:* Given the same inputs, the function that decides *which* drive/target a key maps to always returns the same answer.
- *Why:* Non-determinism here would scatter a key's data or route reads to the wrong drive.
- *Without it:* The same key could resolve to different drives on different calls — data loss.

**P29 — Threshold/watermark comparisons follow the intended direction.**
- *What it says:* Config comparisons (e.g. "start evicting above the high watermark, stop below the low watermark") use the correct inequality direction.
- *Why:* A flipped `<` vs `>` is a trivial-looking bug with catastrophic effect (evict when you shouldn't, or never evict).
- *Without it:* The cache could thrash or fill up because its control logic is inverted.

### J. Map-wide invariants (P30, P31) — "the whole map stays coherent, always"

These are the strongest, most global claims — they must hold across *every* key simultaneously.

**P30 — Each key is in exactly one logical state at a time (exclusive-state invariant).**
- *What it says:* A key is MemoryTier **or** BlockDevice **or** (legacy) Staging — never two at once, never none while "present".
- *Why:* This is the backbone invariant of the whole design. Every operation above assumes a key has one well-defined location. Overlapping states would make "where is this data?" unanswerable.
- *Without it:* Two tiers could each think they own a key → double-free, lost writes, or reading the wrong copy.

**P31 — Reference counters and state remain mutually consistent.**
- *What it says:* The refcount (active readers/writers) and the key's state never contradict each other — e.g. you can't evict a key with a live reader, and a zero refcount doesn't falsely pin an entry.
- *Why:* Refcounts are how the system knows what's safe to move or delete. If they drift from reality, eviction/removal become unsafe.
- *Without it:* You'd get use-after-free (evict something being read) or leaks (never free something whose readers are gone).

---

## 3. How to read the design as a whole

Read top to bottom and a coherent picture emerges:

1. **You must initialize before anything (P1, P2).**
2. **You add keys cleanly, without clobbering or leaking (P3–P5).**
3. **Reads tell the truth and keep hot data hot, without ever corrupting on a miss or a size mismatch (P6–P11).**
4. **Deletes are exact; touch is harmless (P12–P14).**
5. **When memory fills, eviction frees space in a bounded, honest, non-destructive way — demoting, not deleting (P15–P19).**
6. **An older direct-store workflow existed and was retired; we track it honestly rather than pretend it never existed (P20–P24).**
7. **Bulk clear and recovery keep the map faithful to reality — even across restarts (P25–P27).**
8. **The routing math and control thresholds are deterministic and correctly oriented (P28, P29).**
9. **And underneath everything, two global invariants guarantee the map is never internally contradictory (P30, P31).**

The recurring design DNA is two rules: **the map never lies about what exists and where it is**, and **failure never leaves a mess**. Almost every property is one of those two rules applied to a specific operation.

---

## 4. Creusot proof patterns — the recurring proof shapes

The 31 properties are many, but the *proofs* fall into a small number of reusable shapes. This section is for the reader who wants to understand **how** these were discharged, not just what they claim. Each pattern names the shape, the trick that makes it go through, and — crucially — the trust/scope boundary it leans on.

### Pattern A — Extract-the-pure-core decision proof
- *Shape:* Model the runtime's branching (init gate, membership test, size check) as a pure function over `bool` inputs returning a `Result`/tuple, and prove one `#[ensures]` per match arm.
- *Used by:* **P1** (`initialize_dependency_guards`), **P2** (`ensure_initialized`), **P6** (`check_key`), **P7** (`lookup_miss_decision`), **P14** (`touch_decision`).
- *Trick:* The runtime effects (mutex acquire, I/O) are irrelevant to the *decision* — strip them and the arms become a small case analysis the SMT solver closes instantly. The "no mutation on the miss/pre-init path" half is captured by a flag proved `== is_ok`, so refresh/copy happens exactly on the success arm.
- *Scope:* Near-runtime (L0). Faithful because the guard *ordering* is preserved (e.g. P1 checks dispatch_map, then memory_tier, then params — so the returned error variant matches live code).

### Pattern B — FMap map-mutation decision proof
- *Shape:* Thread a logic-level `FMap<K,V>` through a mirror function; model the runtime map op (`create`/`remove`/`convert`/`promote`) as `insert_ghost`/`remove_ghost` guarded by an outcome `bool`; prove the pre/post membership shape.
- *Used by:* **P3** (`create_entry` — duplicate ⇒ `AlreadyExists`, `ext_eq` no-overwrite), **P12/P13** (`remove_entry`), **P4/P5** (`register_memory_tier` — Phase-3 registration atomicity), **P8** (`memtier_lookup_hit` — read preserves state), **P9** (`promote_block_lookup` — in-place `BlockDevice→MemoryTier`), **P19** (`blind_evict_fallback`).
- *Trick:* `std::HashMap::insert`/`remove` carry **no** Creusot extern specs in this toolchain (only `get`/iterators do), so map state *must* be modeled with a logic-level `FMap`. The `FMap` ghost ops (`insert_ghost`, `remove_ghost`) supply exact pre/post specs (`^self == (*self).insert(k,v)`), so membership post-conditions (`(^map).get(key) == Some(...)`, `ext_eq` for no-mutation) discharge directly.
- *Scope:* L1 (single-map, single-key, sequential). The trust boundary is that `FMap`'s ghost ops are `#[trusted]` in creusot-std — a toolchain assumption, not a project lemma.

### Pattern C — Loop proof (invariant + variant)
- *Shape:* A `while`-loop with a `#[invariant]` that ties the loop counter to map/capacity state and a `#[variant]` that strictly decreases, so termination + the post-condition fall out at the exit.
- *Three sub-shapes:*
  - **Drain loop** — **P25/P26** (`clear_all`): invariant `count + map.len() == initial_len`, variant `map.len()`; proves `is_empty()` at exit and `result == entries_cleared`.
  - **Bounded-budget loop** — **P15** (`evict_attempt_budget`): the while-guard reads concurrently-mutated external state, so it is abstracted as an **opaque oracle** (`under_pressure`, `#[trusted]`, arbitrary bool each call) — proving the attempt bound for *every* pressure behavior, including adversarial. Abstraction L3.
  - **Real-guard loop** — **P16/P17** (`evict_for_capacity`): here the guard's *meaning* is modeled (real integer `used + needed > capacity`), so BOTH exits carry the capacity predicate (success ⇒ `<=`, failure ⇒ `>`). Eviction's *effect* on `used` stays opaque since neither property claims progress.
- *Scope:* The dial between P15 and P16/P17 (opaque guard vs. modeled guard) is the key design choice — abstract exactly as much as the property needs and no more.

### Pattern D — L1→L2 map-wide lift
- *Shape:* Define a per-entry logic predicate, quantify it over a whole `FMap` (`forall<k> m.contains(k) ==> P(m.lookup(k))`), then prove each of the three map-mutation shapes (insert-fresh, overwrite, remove) preserves the `forall`.
- *Used by:* **P30/P31** (`map_inv` + `lemma_exclusive_state` + `map_create_entry`/`map_update_entry`/`map_remove_entry`).
- *Trick:* This discharges the "per-entry proof + composition ⇒ whole-map theorem" gap (assumption A7). The `forall`-preservation goes through *automatically* provided the per-entry helper predicates are `#[logic(open)]` (they unfold in the SMT context alongside the `FMap` insert/remove specs). Exclusivity is made a machine-checked *fact* (`lemma_exclusive_state`, no precondition — `Location` is a single-variant enum) rather than folklore.
- *Scope:* L2 (map-wide, sequential — the Mutex is collapsed to a ghost map). Does **not** model concurrent interleavings.

### Pattern E — Integer-lifted ordering
- *Shape:* Runtime `f64` comparisons re-modeled as integer arithmetic (permille) to keep ordering total and decidable.
- *Used by:* **P29** (`evictor_decisions`).
- *Trick:* `f64` NaN makes `<=` non-total, so the SMT `<=` obligation is undischargeable directly. Modeling the ratio as integer permille certifies the *comparison direction* and hysteresis band (`!(start && stop)`), catching flipped `<`/`>=` and swapped watermarks — which is the actual property — while explicitly *not* certifying float arithmetic.

### Pattern F — Range / determinism on a pure function
- *Shape:* Keep the runtime body verbatim; prove only the bound that matters for safety.
- *Used by:* **P28** (`drive_index`): splitmix64 hash kept as-is, only `num_drives>0 ==> result < num_drives` (no out-of-bounds drive select) is proved. Determinism is structural (pure fn, no state/RNG).

### Pattern G — Product-path proof (reachable vs. defensive)
- *Shape:* Model an enum→`Result` match, prove the reachable product path, and explicitly flag the defensive/intentionally-unreachable arm.
- *Used by:* **P11** (`resolve_lookup`): the hit path is `min`-clamped (no over-copy), the miss ⇒ `KeyNotFound`; the `MismatchSize ⇒ InvalidParameter` arm is proved but is dead in production (map `lookup` is key-only; only the mock injects it). Safety rests on a *requested==stored* invariant flagged as a pending trace.
- *Scope:* Honest labeling — the proof is complete for the reachable path and documents which arm is defensive.

### The honest-scoping idiom (cross-cutting)
Patterns B and C repeatedly hit a boundary Creusot's *sequential* model cannot cross: **cross-map** invariants (memory-tier slot ↔ dispatch-map entry drifting apart) and **concurrency** (another thread evicting a key mid-operation). The discipline used throughout is to prove the sequential single-map decision honestly at L1 and *explicitly flag* the cross-map/concurrent part as out of scope — deferred to the map-wide P30/P31 (for cross-map) or to runtime/loom-style tests (for interleavings). P19, P4/P5, and P8/P9 all carry this caveat in their notes rather than overclaiming.

---

## Document Evolution Summary

- New file added under `background/` to give non-specialist readers a complete conceptual walkthrough of properties P1–P31.
- Each property is explained as What it says / Why it's needed / What breaks without it, grouped by design area (init, insert, read, delete, evict, legacy, clear, recovery, arithmetic, map-wide invariants).
- Companion to `properties_to_prove.md` (authoritative registry) — this file carries the "why", that file carries IDs/status/evidence.
- To be extended alongside future global properties (P32+) as component extraction proceeds.
- 2026-07-14: revised the P11 entry to match the current design — the size-mismatch arm is defensive/unreachable (map `lookup` is key-only), the reachable copy path is `min`-clamped (no over-copy), and safety rests on a *requested==stored* invariant that is a pending follow-up to confirm. P11 is no longer framed as an open live bug.
- 2026-07-15: added Section 4 ("Creusot proof patterns") grouping all 31 proofs by structural shape (A pure-core decision, B FMap map-mutation, C loop invariant/variant, D L1→L2 map-wide lift, E integer-lifted ordering, F range/determinism, G product-path) plus the cross-cutting honest-scoping idiom, for blog/paper use. Also expanded the P19 entry with a worked "what it means to prove this correctly" walkthrough of the L1→L2 gap.
- 2026-07-15: expanded the P19 entry into a worked example of the **L1→L2 gap** — introduces the `mt` (memory tier) vs `dm` (dispatch-map) distinction, explains why "no dangling state" is a cross-map whole-map invariant (belongs with P30/P31), and why the racy part sits outside Creusot's sequential model. Documents the deliberate two-step approach: an honest L1 decision proof (`blind_evict_fallback`, `Partial`) first, then fold its meaning into the P30/P31 map-wide theorem.
