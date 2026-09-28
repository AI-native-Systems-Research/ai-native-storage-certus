---
name: tools-spin-model
description: Create a new Spin/Promela formal verification model for a system property
argument-hint: "[property-number or description of property to prove]"
---

Create a new Spin/Promela specification in `modelling/spin/<name>/` to verify a concurrency property of the Certus system.

## Input

If no argument is provided, first ask the user to define the **scope**, then present the property menu.

### Step A: Scope Selection

Ask the user to choose the modeling scope:

- **Single component** — Model the internal concurrency of one component in isolation (e.g., just the dispatcher, just the extent-manager, just the block-device actor). Threads and channels within that component are modeled explicitly; interactions with other components are abstracted as nondeterministic environment stubs.
- **Whole system** — Model the interaction between multiple components as wired together in certus-server (e.g., client → dispatcher → memory-tier → block-device → extent-manager). Includes cross-component communication channels, the gRPC thread pool, and background workers.

The scope determines:
- Which source files to read (single component = one crate's `src/`; whole system = multiple crates + `apps/certus-server/`)
- How to abstract boundaries (single = environment stubs with nondeterministic responses; whole = explicit `proctype` per component role)
- Model complexity (single = fewer processes, deeper per-component detail; whole = more processes, coarser per-component abstraction)

### Step B: Property Selection

Present the full property menu and ask the user to choose. The user may also describe a custom property.

**System-level properties** (scope: whole system recommended):

1. **Reference-count balance** — Every read_ref increment has a matching decrement on all paths. No leaked refs (memory leak) or double-decrements (use-after-free).
2. **Write-before-evict** — Models the dispatcher's populate → write-through → eviction lifecycle to verify that memory-tier eviction never produces dangling SSD references.
3. **Cold-path promotion atomicity** — At most one thread succeeds in promoting a given key from SSD to memory-tier. No double-allocation or lost updates.
4. **Populate-lookup linearizability** — After populate(key) returns Ok, a concurrent lookup(key) never observes KeyNotFound unless an explicit remove or eviction intervened. ⚠️ The "unless … eviction intervened" clause deliberately *tolerates* an eviction-caused miss — this is the exact loophole through which the Check→Pin race shipped. If the client observes the key before loading it, that tolerance is unsafe: use **#12** instead (or in addition), which forbids removing a key any live client has observed.
5. **Shutdown ordering** — No NVMe command is issued after its block device shuts down. Background writer drains before drives are torn down.
6. **No lost extents** — Every reserve_extent is followed by either publish() or drop (abort). No extent is permanently allocated but uncommitted.
12. **Observe→use resolvability under eviction (Check→Pin)** — Once a client (e.g. a vLLM connector) has *observed* a key resident in one RPC without pinning it, a later *pinned* load of that key in a separate RPC never observes NotExist. Concurrent eviction may **demote** the key (still resolvable) but must never **remove** it out from under the observing client. This is strictly stronger than #4: #4 permits an eviction-drop as a legitimate departure, whereas this property forbids removing a key any live client has observed. (This is the class of bug fixed by `evolve-dispatcher-dw`; see `modelling/spin/check-pin-eviction-race/`.)

**Component-level properties** (scope: single component recommended):

7. **Pipeline ring slot safety** — A ring buffer slot is never resubmitted for NVMe read while its CUDA DMA transfer is still in flight. (Component: dispatcher/pipeline)
8. **Starvation freedom** — No client is starved indefinitely waiting to enqueue a write job under backpressure. (Component: dispatcher/background)
9. **Eviction fairness** — An entry touched within the last N operations is never evicted while untouched entries exist. (Component: dispatcher/eviction)
10. **Actor mailbox ordering** — Messages sent to a block-device actor are processed in FIFO order; no command reordering across the channel boundary. (Component: block-device-spdk-nvme)
11. **Extent bitmap consistency** — Concurrent reserve/publish/remove never leave the bitmap in a state where an extent is both allocated and free. (Component: extent-manager)

If the user provides a number (1-12), use that property. If they provide free text, treat it as a custom property description and ask for a short kebab-case name for the subdirectory.

## Steps

1. **Determine the property and directory name.**

   Map selections to directory names:
   - 1 → `ref-count-balance`
   - 2 → `write-before-evict`
   - 3 → `promotion-atomicity`
   - 4 → `populate-lookup-linearizability`
   - 5 → `shutdown-ordering`
   - 6 → `no-lost-extents`
   - 7 → `pipeline-ring-safety`
   - 8 → `starvation-freedom`
   - 9 → `eviction-fairness`
   - 10 → `actor-mailbox-ordering`
   - 11 → `extent-bitmap-consistency`
   - 12 → `check-pin-eviction-race`
   - Custom → ask user for a kebab-case name

2. **Read BOTH the specification and the source code.** The spec tells you which states are *allowed* (the intended contract); the code tells you which states are *reachable* (the real protocol). A model built from code alone encodes what the code *does*, not what it *should* do — so it will faithfully reproduce a bug and pass. Derive the safety **assertion from the spec**; derive the process structure and reachable transitions from the code.

   **Read the spec first (this is Spec-Kit driven — every component has `components/<c>/specs/<NNN-feature>/`):**
   - Dispatcher (properties 1–4, 9, 12): `components/dispatcher/specs/001-dispatcher-cache-interface/` — `spec.md` (functional requirements, FR-IDs), `data-model.md` (entity states & transitions), `contracts/idispatcher.md` (per-operation pre/post-conditions), **`contracts/errors.md`** (which error each operation may legally return — the authority for "may a lookup return NotExist after populate?"), `checklists/requirements.md`. Also `info/DESIGN.md` and the `knowledge/` wiki (e.g. `size-mismatch-handling.md`).
   - Extent-manager (properties 6, 11): `components/extent-manager/specs/001-extent-manager-v2/spec.md` (+ any `contracts/`, `data-model.md`).
   - Block-device (properties 5, 10): `components/block-device-spdk-nvme/specs/001-spdk-nvme-block-device/spec.md`, `data-model.md`, `contracts/iblock_device.md`.

   For the chosen property: locate the requirement / contract clause that states the guarantee (cite its FR-/requirement ID), and phrase the model's `assert()` as the *negation of a spec-permitted-outcome violation* — not as your paraphrase of the code. If the spec is silent on the property (e.g. it does not say whether eviction may render an observed key unresolvable), that gap is itself a finding: note it, model the stronger safe interpretation, and flag it for the user / spec owner.

   **Then read the source code** to understand the real synchronization protocol and build the reachable state space. Key source files by property and scope:

   **Whole-system scope** (read certus-server wiring + component internals):
   - 1,4: `components/dispatcher/src/lib.rs` (populate, lookup, evict_for_space, batch_lookup)
   - 2: `components/dispatcher/src/lib.rs` + `components/dispatcher/src/background.rs`
   - 3: `components/dispatcher/src/lib.rs` (batch_lookup cold-path promotion, lines ~1045-1215)
   - 5: `components/dispatcher/src/lib.rs` (shutdown), `components/dispatcher/src/background.rs` (BackgroundWriter::shutdown), `apps/certus-server/src/main.rs` (shutdown sequence)
   - 6: `components/dispatcher/src/lib.rs` (prepare_store, commit_store, cancel_store, process_write_job)
   - 12: `components/dispatcher/src/lib.rs` (`evict_one_clean` demote-vs-remove, `batch_lookup` classify→load, `dm.lookup`/`dm.remove`/`convert_to_storage`), plus the connector's Check RPC vs Load RPC split. Reference model: `modelling/spin/check-pin-eviction-race/check_pin_eviction_race.pml`.

   **Single-component scope** (read only the target component):
   - 7: `components/dispatcher/src/pipeline.rs` (PipelineRing, pipelined_ssd_to_gpu_zero_copy)
   - 8: `components/dispatcher/src/background.rs` (channel + worker_loop)
   - 9: `components/dispatcher/src/lib.rs` (evict_for_space, mt.evict_lru, mt.oldest_keys)
   - 10: `components/block-device-spdk-nvme/src/` (actor thread, command channel, completion channel)
   - 11: `components/extent-manager/src/` (bitmap allocation, reserve_extent, publish, remove_extent)

   Also reference the existing model at `modelling/spin/write-before-evict/write_before_evict.pml` for style and conventions.

3. **Create the subdirectory** `modelling/spin/<name>/`.

4. **Write the Promela specification** `modelling/spin/<name>/<name_underscored>.pml`.

   Follow these conventions (from the existing write-before-evict model):
   - Header comment block explaining what is being verified, the scope, and how to run
   - `#define` parameters section (N_CLIENTS, N_KEYS, etc.) tuned small for tractable state space
   - Per-key or per-resource state as `mtype` enums and arrays
   - `proctype` for each thread role in the system
   - `inline` helpers for shared logic (like eviction)
   - Inline `assert()` statements for safety properties
   - `init` process that starts all proctypes, waits for completion, signals shutdown, runs final invariant checks
   - Aim for 100% state coverage (no unreached states)
   - Use `atomic{}` for operations that share a mutex in the real code
   - Use `d_step{}` for deterministic sequences (scans)
   - Use generation counters to handle key reuse across lifecycles
   - Model channels as bounded `chan` (even if real code uses unbounded)

   **Scope-specific conventions:**

   For **single component** models:
   - Abstract external interfaces as nondeterministic stubs (e.g., `if :: success :: failure fi`)
   - Focus on internal thread interactions and shared state
   - Name the header comment "Component-level model: <component-name>"
   - Document which interfaces are stubbed and what assumptions are made

   For **whole system** models:
   - One `proctype` per major thread role across components
   - Use channels to model inter-component communication (gRPC, receptacles)
   - Name the header comment "System-level model: <property-name>"
   - Document the component wiring in the header

   **Bug-finding conventions (whole system) — these are what make a model able to *catch* a race, not just pass:**

   - **Model observe-then-act client flows as SEPARATE steps.** When a client learns something in one RPC and acts on it in a later RPC (e.g. a connector *Checks* a key resident, then *Pins+Loads* it in a separate call; or a store that prepares then commits), model the two as distinct steps with the intervening window left open to all other processes. Collapsing them into one atomic pin-on-hit hides exactly the races that live in the gap. Only the *use* step takes the read-ref/pin; the *observe* step must not pin. (This gap is where the Check→Pin eviction race lived.)
   - **Distinguish eviction/removal OUTCOMES, not just "evicted."** A demote (`convert_to_storage`: entry flips to BlockDevice, still resolvable via cold-path promote) and a full remove (`dm.remove`: entry becomes NotExist) have completely different consequences for a client. Model them as separate transitions. Treat *removing a key that any live client has observed but not yet pinned* as a candidate hazard, and assert against it.
   - **Inject the failure/error branches that make hazardous states reachable.** Many bugs are only reachable after a fallible op fails: a write-through that fails (`IoError`) releases the writer's pin while leaving the entry unpersisted; a `dm.lookup` that times out; an allocation that fails. Add nondeterministic failure branches (`if :: ok :: fail fi`) for every fallible cross-component op the property depends on. Without them the hazardous state (e.g. unpinned-AND-unpersisted) is unreachable and the model is *falsely green*.
   - **Classify each terminal client outcome as FATAL or GRACEFUL, and assert no reachable path is fatal.** Map the modeled outcome back to the real consequence: a load miss forwarded to remote-lookup is *fatal* (IoError → `EngineDeadError` → vLLM crash); an `AllocationFailed` that serves uncached is *graceful*. The safety assertion should forbid reaching a fatal terminal, not merely "some error." (A property that lumps fatal and graceful misses together as "KeyNotFound is allowed" is too weak — that is exactly why property #4 tolerated this bug.)

5. **Add a known-bad MUTANT and prove the model rejects it (MANDATORY).**

   A model that only ever passes on the current code proves nothing about its power to *catch* a bug — it may simply be too weak. For every safety property, gate the suspected-hazardous behavior behind a Promela compile-time switch and prove the model **fails** on the mutant:

   - Pick the hazard the property guards against and express its buggy variant behind `#ifdef INJECT_<HAZARD>` (e.g. `#ifdef BUGGY_DROP_FALLBACK` toggles a `dm.remove` of an unpinned unpersisted victim vs. skipping it). Keep the two branches minimal and directly mirror a real code path that either exists in history or is a plausible regression.
   - **The fixed build (switch off) must verify with 0 errors; the mutant build (switch on) must produce an assertion violation.** If the mutant still passes, the property is too weak — strengthen the assertion (usually: make it forbid a *fatal* outcome, or assert on a finer-grained state) until the mutant fails, then confirm the fixed build still passes.
   - Wrap the buggy-only `#ifdef` arms so that in the fixed build they are *pruned*, not left as dead unreached states. A branch that is deliberately unreachable in the fixed build (e.g. the fatal-miss arm) is expected and is itself evidence that the fatal path is dead — call it out in the README rather than forcing it reachable.
   - **Gotcha:** `#define`/`#ifdef` in Promela are processed by `spin -a` (which runs the model through the C preprocessor), NOT by `cc`. Pass the switch as `spin -DINJECT_<HAZARD> -a model.pml`. The generated `pan.c` differs between builds, so each build must regenerate it. To replay a mutant trail you must pass the same `-D` to `spin -t`.

   For a worked example see `modelling/spin/check-pin-eviction-race/` (`make` = fixed, 0 errors; `make buggy` = mutant, assertion violation + trail).

6. **Write the Makefile** `modelling/spin/<name>/Makefile` (includes the mandatory `mutant` target — rename `INJECT_HAZARD` to your switch):

   ```makefile
   MODEL = <name_underscored>.pml
   CC    = cc
   CFLAGS = -O2
   DEPTH = 200000
   MUTANT = INJECT_HAZARD

   .PHONY: all safety mutant liveness clean

   # Default: verify the FIXED system. Expected: 0 errors, 0 unreached states.
   all: safety

   safety:
   	spin -a $(MODEL)
   	$(CC) $(CFLAGS) -DSAFETY -o pan pan.c
   	./pan -m$(DEPTH)

   # Known-bad mutant. Expected: an assertion violation + a .trail.
   # The -D is a Promela (cpp) switch, so it goes to `spin -a`, not to cc.
   mutant:
   	spin -D$(MUTANT) -a $(MODEL)
   	$(CC) $(CFLAGS) -DSAFETY -o pan-mutant pan.c
   	./pan-mutant -m$(DEPTH)

   liveness:
   	spin -a $(MODEL)
   	$(CC) $(CFLAGS) -o pan-live pan.c
   	./pan-live -a -m$(DEPTH)

   clean:
   	rm -f pan pan-mutant pan-live pan.* *.trail
   ```

7. **Write the README.md** `modelling/spin/<name>/README.md`:

   Structure (follow the pattern in `modelling/spin/write-before-evict/README.md`):
   - Title: `# <Property Name>`
   - **Scope**: Single component (`<component-name>`) or Whole system
   - One-paragraph description of what the model verifies
   - **Properties Verified** table (ID, Property description, Type: Safety/Liveness)
   - **System Abstraction** table (Real component → Promela process)
   - **Assumptions / Stubs** section (for single-component scope: list what is abstracted away and how)
   - **Specification basis** section: for each asserted property, cite the spec clause it enforces (`spec.md` FR-ID, `contracts/errors.md`, or the requirements checklist), and note any spec gap the model had to resolve by choosing the stronger safe interpretation
   - **Mutant** section: name the known-bad switch, what it toggles, and the expected fixed-vs-mutant outcomes
   - **Running** section with shell commands (including `make mutant` and the mutant trail replay)
   - **Tuning the Model** section explaining parameters
   - **Correspondence** table with THREE columns: Model location → *Intended behavior (spec §/FR-ID)* → *Implemented behavior (source file:line)*. Where the spec and code columns diverge, that row is a finding.

8. **Run BOTH verifications** in the new directory: `make` (fixed) and `make mutant`.

   - If Spin is not installed, inform the user and skip this step.
   - The fixed build (`make`) must report **0 errors**. The mutant build (`make mutant`) must report **≥1 assertion violation**; replay its trail with `spin -t -p -g -l -D<MUTANT> <file>.pml` and confirm the interleaving is the intended hazard.
   - **If the fixed build fails**, first decide whether it is a real defect in the code (analyze the trail, report it to the user — do not silently weaken the model to make it green) or a modeling artifact (over-abstraction, a missing pin, wrong atomicity). Only relax the *model* when it is genuinely an artifact; a real counterexample against current code is a finding, not a bug in the model.
   - **If the mutant build passes**, the property is too weak — strengthen it (per the bug-finding conventions above) until the mutant fails, then re-confirm the fixed build still passes.
   - **If the fixed build passes but contradicts the spec** — i.e. the code is self-consistent yet the modeled behavior violates a `spec.md`/`contracts/` clause — that is a code-vs-spec divergence. Report it as a finding (the model verified the wrong contract); do not treat a green run as success when the assertion came from the code instead of the spec.

9. **Report results** to the user:
   - Scope (single component or whole system)
   - Property being verified, and the spec clause (FR-ID / contract) it was derived from — plus any spec gap or code-vs-spec divergence surfaced
   - Fixed build: pass/fail status, state space size, depth, coverage (target: 0 unreached in all proctypes, modulo deliberately-pruned mutant-only branches)
   - Mutant: which hazard it injects, and confirmation the model catches it (violation + trail)
   - Path to the new model directory

## Notes

- **Assert from the spec, not from the code.** The property you check must come from the specification (`spec.md` / `contracts/` / requirements checklist), because a model whose assertion is paraphrased from the implementation can only ever confirm the code agrees with itself — it cannot catch a bug the code and your reading of it share. The code is for the *reachable states*; the spec is for the *allowed states*. (See step 2.)
- **A model that only passes proves nothing.** The next most important discipline (step 5) is the known-bad mutant: a property is only as good as its ability to reject a buggy variant. If you cannot construct a plausible mutant that the model catches, the property is probably too weak to catch a real regression either. This is precisely why property #4 (populate-lookup linearizability) *passed* while the Check→Pin race shipped — it was written to tolerate the eviction-drop, and no mutant ever forced it to confront one.
- Spin is installed at `~/.local/bin/spin` on this machine (run `modelling/spin/install-spin.sh --prefix $HOME/.local` and put `$HOME/.local/bin` on `PATH`; the older `/usr/local/bin/spin` location also works if present). The optional Tcl/Tk GUI is `ispin` (needs the `tk` package for `wish`).
- Keep parameters small (2-3 clients, 2-4 keys, pool cap < key count) to maintain tractable state spaces (<10M states).
- The model should be self-contained — no external dependencies beyond Spin and a C compiler.
- Use the generation-counter technique from the write-before-evict model to handle key reuse without conflating lifecycles.
- Single-component models are faster to verify (fewer processes) and better for finding internal races.
- Whole-system models catch cross-component protocol violations but have larger state spaces.
