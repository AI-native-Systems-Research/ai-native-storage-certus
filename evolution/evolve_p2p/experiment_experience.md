# Experiment Experience Log

Operational learnings from running the evolve-p2p experiment. Updated as issues arise.

---

## 2026-06-01: GEPA Build Failures — FFI Type Mismatch

### Problem

GEPA's LLM proposals for `dma.rs` consistently failed to compile with:
```
error[E0308]: mismatched types
  --> components/gpu-services/src/dma.rs:455:71
    | let rc = unsafe { cuda_ffi::cudaHostAlloc(&mut ptr, aligned_size, flags) };
    |                                                                   ^^^^^ expected `i32`, found `u32`
```

Every iteration (15/15) produced the same type error. GEPA's reflection LLM saw the error via `oa.log()` / `side_info` but kept regenerating the same pattern.

### Root Cause

The LLM doesn't know the exact FFI function signatures. `cuda_ffi::cudaHostAlloc` takes `flags: c_int` (i32), but the LLM generates `flags: u32` because that's the common convention for bitfield flags in most languages.

GEPA's reflection mechanism feeds the compiler error back to the proposer LLM, but the LLM doesn't correct it because:
1. It regenerates the entire 742-line `dma.rs` from scratch each iteration
2. The error message says "expected i32 found u32" but the LLM doesn't retain this across its full rewrite
3. Without explicit type annotations in context, it defaults to the "obvious" `u32` for flags

### Fix

Added FFI type signatures to GEPA's background context:
```
## FFI Type Signatures (CRITICAL — must match exactly)
pub fn cudaHostAlloc(phost: *mut *mut c_void, size: usize, flags: c_int) -> cudaError_t;
...
Use `as c_int` or `as i32` when passing integer literals to these functions.
```

### Lesson

**LLM-guided evolution on Rust code requires FFI boundary documentation in the prompt.** The compiler error feedback loop (GEPA's reflection) is insufficient when the LLM regenerates files from scratch — it doesn't "learn" from previous errors the way an iterative repair loop would. The fix is to provide the type constraints upfront rather than expecting the LLM to infer them from error messages.

This is analogous to providing a header file when asking a developer to write C code that calls a library — without it, they'll guess the signatures.

### Follow-up: Over-Correction

Adding the full FFI block with "CRITICAL — must match exactly" caused GEPA to play it completely safe — producing code identical to the seed for 30 iterations. The prescriptive tone killed exploration.

**Final approach:** Removed the detailed FFI block. Added only a soft one-liner to both GEPA and SkyDiscover configs:
- GEPA background: `"The cuda_ffi module uses std::os::raw::c_int for all flags and error return types."`
- SkyDiscover system_message: same hint + `"The build uses --features p2p, so all #[cfg(feature = \"p2p\")] code is compiled and type-checked."`

This gives the LLM enough to avoid the u32/i32 mistake without scaring it into passivity. The p2p compile warning is important — tells the LLM that cfg-gated code will actually be checked, so it can't dump untested P2P code behind gates.

### Lesson Refined

**There's a Goldilocks zone for FFI context in LLM prompts:**
- Too little → repeated type errors, 100% build failure
- Too much (with warning tone) → LLM plays safe, 0% exploration, no improvement
- One-liner hint → LLM knows the convention, still explores creatively

The same principle likely applies to any constraint: state it as a fact, not as a warning.

---

## 2026-06-01: GEPA Seed Size — lib.rs Too Large

### Problem

Initial GEPA seed included full `lib.rs` (3244 lines). The LLM output analysis prose instead of code ("Looking at the evaluation results, the current implementation achieves...") because it couldn't reliably regenerate 3244 lines of Rust.

### Fix

Removed `lib.rs` from the GEPA seed. Only `pipeline.rs` (413 lines) and `dma.rs` (742 lines) are included. The pipeline optimization opportunity is concentrated in these two files.

### Lesson

**GEPA's full-rewrite mode has a practical limit of ~500-800 lines per file.** Beyond that, the LLM outputs reasoning/commentary instead of complete source code. For larger files, either:
- Extract only the relevant functions into the seed
- Use diff-based generation (but this has its own failure modes with SEARCH/REPLACE matching)
- Use a framework that operates on the full repo with targeted edits (Nous, AutoScientists)

---

## 2026-06-01: Concatenated Seed Pollution

### Problem

`_ensure_concatenated_seed()` wrote `concatenated_seed.rs` into `initial_programs/`. GEPA's `load_seed()` picks up all `.rs` files from that directory, so GEPA tried to evolve a 4th file that was only meant for SkyDiscover.

### Fix

Renamed to `concatenated_seed.rs.skydiscover` so `load_seed()` ignores it.

### Lesson

**Keep framework-specific artifacts out of shared directories.** The seed directory should only contain files that all frameworks can meaningfully operate on.

---

## 2026-06-01: SkyDiscover Signal Handler Error

### Problem

SkyDiscover runs the evaluator in a thread pool (`asyncio.run_in_executor`). Our evaluator's `signal.signal()` calls fail with "signal only works in main thread of the main interpreter."

### Fix

Wrapped signal handler registration in `if threading.current_thread() is threading.main_thread()`. When running from a thread, signal handlers are skipped — the `try/finally` block still handles cleanup.

### Lesson

**Evaluators called by async frameworks must not assume they're on the main thread.** GEPA runs evaluators on the main thread (synchronous). SkyDiscover runs them in a thread pool. The evaluator must handle both.

---

## 2026-06-01: SkyDiscover Return Type Mismatch

### Problem

Our evaluator returned `(score, metrics_dict)`. SkyDiscover's `_normalize_result()` expects a `dict` and logs "Unexpected result type: tuple."

### Fix

Added `_skydiscover_mode` detection: when called with a file path (SkyDiscover's pattern), return `metrics_dict` with `combined_score` key. When called with content (GEPA), return `(score, metrics)` tuple.

### Lesson

**Different frameworks have different evaluator return conventions.** Build a universal evaluator that detects the calling convention and adapts. The key is `combined_score` in the dict for SkyDiscover, tuple first element for GEPA.

---

## 2026-06-01: OPENAI_API_KEY Not Passed to Subprocesses

### Problem

SkyDiscover frameworks failed immediately with "The api_key client option must be set." We set `LITELLM_API_KEY` but SkyDiscover's OpenAI client reads `OPENAI_API_KEY`.

### Fix

Orchestrator now maps: `LITELLM_API_KEY` → `OPENAI_API_KEY`, `LITELLM_API_BASE` → `OPENAI_BASE_URL` in the subprocess environment.

### Lesson

**Map all credential env vars to what each framework expects.** Don't assume frameworks share env var conventions.

---

## 2026-06-01: GEPA Callback — on_iteration_end vs on_evaluation_end

### Problem

We used `on_iteration_end` which only provides the state's best-so-far score. Dashboard showed "all successes at 0.21" when actually 14/15 evaluations were build failures scoring 0.0.

### Fix

Switched to `on_evaluation_end` which fires per evaluator call with `scores`, `outputs` (containing our metrics dict with error messages), and `candidate_idx`.

### Lesson

**Use the most granular callback available.** `on_iteration_end` is a summary event. `on_evaluation_end` gives per-candidate detail needed for failure analysis.

---

## 2026-06-01: Feature Gate — P2P Code Dead Behind #[cfg(feature = "p2p")]

### Problem

GEPA's best candidate included P2P/GDRCopy buffer pools in `dma.rs`, but all behind `#[cfg(feature = "p2p")]`. The build command didn't enable this feature, so the code compiled (cfg gates = dead code, no errors) but never ran. GEPA appeared to "discover" P2P but didn't — it just preserved existing gated code from the seed.

### Fix

Added `p2p` feature to `certus-server/Cargo.toml` (passes through to `gpu-services/p2p`). Build command now uses `--features p2p`. P2P functions are compiled and available for the pipeline to call.

### Lesson

**Feature gates create invisible barriers for LLM-guided evolution.** The code exists, the LLM sees it and elaborates on it, but it never runs. The LLM doesn't understand Cargo feature flags as an activation mechanism. Either:
1. Enable all relevant features in the build (preferred — removes artificial constraint)
2. Or instruct frameworks to inline code rather than use cfg gates (fragile)

This maps to the experiment's H4-A vs H4-B comparison: the framework "discovered" the architecture (it's in the seed) but couldn't activate it due to a build system constraint outside its knowledge.

---

## 2026-06-01: False P2P Discovery — cfg Gates as Untested Dead Code Zones

### Problem

With `--features p2p` disabled, GEPA appeared to "discover" a GPU-direct architecture: its best candidate included GDRCopy buffer pools, nvidia-peermem registration, and P2P DMA helpers in `dma.rs`. The architecture classification showed "path_change" signals. This looked like a breakthrough.

After enabling `--features p2p` (so the compiler actually type-checks the P2P code), GEPA produced **zero architectural changes**. All 30 candidates were functionally identical to the wild-type seed. Score range: 0.2038–0.2046 (noise).

### Root Cause

`#[cfg(feature = "p2p")]` blocks are **not compiled** when the feature is disabled. The Rust compiler completely skips them — no type checking, no borrow checking, no name resolution. The LLM could write arbitrary nonsense inside a cfg-gated block and it would "compile."

What actually happened in the first run:
1. The seed `dma.rs` already contained P2P functions behind `#[cfg(feature = "p2p")]`
2. GEPA's LLM saw these and elaborated (added buffer pools, new helpers) — all inside cfg gates
3. This code was **never type-checked** → compiled fine regardless of correctness
4. The non-gated pipeline code had a real type error (`u32` vs `i32`) → build failure
5. The "best" candidate that survived had: broken pipeline (scored via cached seed eval) + untested P2P code

Once we enabled the feature:
1. P2P code is now type-checked → writing it correctly is hard
2. The LLM can't get FFI types, lifetimes, and unsafe blocks right simultaneously
3. It falls back to reproducing the seed unchanged (safe, compiles, scores baseline)
4. No P2P attempt because doing it correctly under compiler scrutiny is beyond current capability

### The Illusion

The previous "P2P discovery" was **an illusion created by dead code zones**. The LLM wrote P2P-shaped text into a region where the compiler couldn't reject it. It's analogous to writing pseudocode in comments — it looks like an implementation but was never validated.

### Finding

**Disabled feature gates create false signals of architectural discovery.** Any code behind `#[cfg(feature = "disabled")]` is untested dead code that can contain arbitrary errors. Evaluating evolution quality based on the presence of dead code symbols (our architecture classifier looked for "spdk_mem_register", "nvidia_peermem", etc.) produces false positives.

### Implications for the Experiment

1. **Architecture classification must only count compiled, executed code.** Symbols in cfg-gated blocks are not evidence of discovery.
2. **All relevant features must be enabled during the experiment.** Otherwise the fitness landscape has artificial "easy zones" where the LLM can deposit incorrect code without penalty.
3. **The gap between "can generate P2P-shaped code" and "can implement a working P2P path" is massive.** The LLM has the conceptual knowledge (it knows what GDRCopy is, what nvidia-peermem does) but can't produce type-correct Rust that passes the compiler under real scrutiny.
4. **This is the most important finding so far:** LLM-guided evolution can discover architectures *in principle* (it generates the right structure) but fails at *implementation* (correct types, lifetimes, unsafe invariants). The bottleneck is not discovery — it's execution.

---

## 2026-06-01: Final Quantitative Results (No-Hint Control Run)

### Run Status

The `tmux evolve` session completed and terminated. The Streamlit dashboard remains live at http://localhost:8501.

### Framework Results Summary

| Framework | Iterations | Wall Time | Best Score | Status |
|-----------|-----------|-----------|-----------|--------|
| **gepa_native** | 5/10 | 190s | 0.2017 | Completed — no improvement over seed |
| **adaevolve** | 10 evals | 1200s (timeout) | 0.2100 | Best performer — marginal gain (+0.83% over seed) |
| **evox** | 0 | 1200s (timeout) | 0.0 | Timed out during discovery retries |
| **openevolve** | 0 | 1200s (timeout) | 0.0 | Benchmark returned empty data |
| **shinkaevolve** | 0 | 3.9s | 0.0 | Import error (`AsyncEvolutionRunner` not in shinka.core) |
| **ksearch** | 0 | 1.0s | 0.0 | Crash on startup |
| **random** | 10 | 269s | 0.0 | 0/10 candidates compiled or ran successfully |

### GEPA Native Detail

All 5 iterations produced candidates scoring 0.0 (build failures). The seed score of 0.2017 was never beaten. GEPA kept the seed as `selected_program_candidate: 0` every iteration. The LLM's proposals all failed to compile.

### AdaEvolve Detail (Best Result)

AdaEvolve managed successful evaluations with scores ranging 0.2023–0.2100. Best candidate metrics:
- Throughput: 2.44 GB/s (test: 2.36 GB/s)
- p99 latency: 1.82ms (test: 1.89ms)
- CPU utilization: 2.7%
- Data integrity: PASS

This is only a **+4.1% improvement** over the seed (0.2017 → 0.2100) and entirely within noise for pipeline parameter tuning — no architectural change.

### Random Baseline

0/10 random mutations compiled successfully. Failures included array size mismatches, index-out-of-bounds panics, and type errors. This confirms the fitness landscape is extremely narrow — random code changes almost never produce valid Rust.

### Key Numbers

- **Seed baseline score**: 0.2017 (combined metric)
- **Best score achieved by any framework**: 0.2100 (AdaEvolve, +4.1%)
- **Architectural changes discovered**: 0 (zero frameworks produced P2P or GPU-direct paths)
- **Build success rate (GEPA)**: 0/5 candidates compiled
- **Build success rate (random)**: 0/10 candidates compiled
- **Build success rate (AdaEvolve)**: ~10/12 evaluations passed (highest of any framework)

### Interpretation

The no-hint control run confirms that under real compilation constraints (features enabled, all code type-checked), current LLM-guided evolutionary frameworks cannot discover novel GPU-direct architectures in Rust. The best outcome (AdaEvolve +4.1%) represents minor parameter tuning, not structural innovation. The extremely low build success rates (0–83%) show that generating valid Rust modifications is the binding constraint, not the search strategy.

---

## 2026-06-01: Framework Failure Modes

### Frameworks That Never Produced Results

- **ShinkaEvolve**: Python import error — the `AsyncEvolutionRunner` class was removed or renamed in the installed version. The SkyDiscover backend references a stale API.
- **KSearch**: Python import error — `ImportError: cannot import name 'KernelGeneratorWorldModel'` from the installed K-Search version. The run wrapper references a class that doesn't exist in the installed package (exited in 1.0s).
- **EvoX**: Entered a retry loop. The benchmark returned "no data" for all lookup operations (server started but served no objects). The discovery controller retried 3 times per iteration then gave up.
- **OpenEvolve**: Similar to EvoX — benchmark timeout. The modified code compiled but the server couldn't serve requests, producing 0-score evaluations until the 1200s wall-time limit.

### Lesson

**Only 2 of 7 frameworks (GEPA, AdaEvolve) successfully evaluated candidates.** The random baseline also ran all 10 evaluations but scored 0 due to build failures. The high framework failure rate (5/7) suggests the evaluator integration is fragile — different frameworks trip on different API conventions, import paths, and subprocess management patterns. A robust experiment needs pre-flight checks per framework.

---

## 2026-06-01: K-Search and ShinkaEvolve — Fixing Cascading Integration Failures

### Background

Both `ksearch` and `shinkaevolve` crashed on import errors in the initial run. Fixing them revealed multiple layers of integration issues — each fix uncovered the next problem.

### K-Search Fixes (4 layers)

**File**: `evolution/evolve_p2p/run_ksearch_p2p.py`

1. **Import error**: Wrapper imported `KernelGeneratorWorldModel` — doesn't exist. Correct class: `WorldModelKernelGeneratorWithBaseline`.
2. **API mismatch**: Wrapper called `generator.run_one_round()` in a loop — method doesn't exist. K-Search's API is `generator.generate(task, max_opt_rounds=N)` which runs the full world-model-guided loop internally.
3. **Model name format**: `openai/aws/claude-opus-4-6` → `aws/claude-opus-4-6`. The `openai/` prefix is a LiteLLM routing convention not expected by the proxy's model access control.
4. **Language mismatch**: `language="cpp"` → `language="triton"`. K-Search only supports `cuda`/`triton`/`mlx`/`python` in its prompt templates. `cuda` mode additionally enforces XML-structured output (`kernel.h`, `kernel.cu`, `main.cpp`) which is incompatible with our `// --- FILE:` marker format. `triton` mode uses a plain text code cleaner (strips markdown fences, returns raw text) which works for any language.

**Additional**: Instrumented `task.run_benchmark` with a wrapper to capture per-evaluation scores to `scores.jsonl` (K-Search's `generate()` doesn't expose per-round callbacks).

### ShinkaEvolve Fixes (5 layers)

**File**: `skydiscover/extras/external/shinkaevolve_backend.py`

1. **Import error**: `AsyncEvolutionRunner` → `ShinkaEvolveRunner` (class was renamed in current ShinkaEvolve version).
2. **Module path**: `from shinka.core.runner import EvolutionConfig` → `from shinka.core import EvolutionConfig` (no `runner.py` module exists; it's `async_runner.py`, but `EvolutionConfig` is re-exported from `shinka.core.__init__`).
3. **Async mismatch**: `await runner.run()` → `await runner.run_async()`. The `.run()` method is sync (wraps `asyncio.run()` — fails inside an already-running event loop). The async entry point is `.run_async()`.
4. **Model name resolution**: ShinkaEvolve's model resolver rejects `aws/claude-opus-4-6` — requires `local/<model>@<url>/v1` format for custom OpenAI-compatible endpoints. Added automatic mapping in `_map_config()`.
5. **Missing dependency**: ShinkaEvolve wasn't in skydiscover's venv. Required Python ≥3.10 (system python is 3.9, but skydiscover venv uses 3.12). Added to `pyproject.toml` as editable path source + `uv pip install`.

### Orchestrator Fixes

**File**: `evolution/evolve_p2p/run_experiment.py`

- Changed K-Search python from `python3` (→ 3.9) to `/usr/bin/python3.12`
- Read `/tmp/.bakeoff_key` for API key when `LITELLM_API_KEY` is not in environment (was only done for ksearch's `--api-key` flag, not for subprocess env)
- Set `OPENAI_BASE_URL` default for all subprocess environments (ShinkaEvolve's embedding model requires it)

### How K-Search Worked Before (pipeline-bakeoff/hypothesis_3)

The previous "ksearch" run at `evolution/pipeline-bakeoff/hypothesis_3/results/ksearch/` was **not using K-Search's native API at all**. It used SkyDiscover's AdaEvolve backend with a system prompt that *emulates* K-Search's world-model reasoning methodology. The log shows `search=adaevolve`, `AdaEvolveDatabase initialized`. This worked because SkyDiscover handles code as plain text — no language-specific parsing.

The evolve_p2p experiment wrapper attempted to use K-Search's actual `WorldModelKernelGeneratorWithBaseline` class, which is designed for CUDA/Triton kernel optimization, not arbitrary Rust code. The multi-file Rust task required adapting: `CertusP2PTask` was already written correctly, but the generator's language/prompt layer needed `triton` mode to bypass CUDA-specific output parsing.

### Lesson

**Framework integration is a fractal of version mismatches.** Each framework has its own:
- Import paths (which change between versions)
- Model name conventions (provider-prefixed, local-prefixed, raw)
- Evaluator call conventions (sync vs async, return types)
- Language/output format assumptions
- Dependency requirements (Python version, packages)
- Authentication patterns (env vars differ)

A single "import fix" cascades into 4-5 follow-up fixes. Pre-flight smoke tests should verify not just `import X` but `X(args).run_one_step()` to catch these layers before a full experiment run.

### ShinkaEvolve Additional Issue: EVOLVE-BLOCK Markers Required

After fixing all import/auth/model issues (6 layers total), ShinkaEvolve finally ran — but immediately failed with: `"No EVOLVE-BLOCK regions found in original content"`. ShinkaEvolve's patch system expects `# EVOLVE-BLOCK-START` / `# EVOLVE-BLOCK-END` markers in the seed file to identify mutable regions. Our concatenated Rust seed doesn't have these.

Every generation attempt fails with `"Could not extract code from patch string"` because the framework can't figure out where to apply its patches. This is a fundamental integration mismatch — ShinkaEvolve's code modification strategy requires annotated source, unlike GEPA/AdaEvolve which do full-file rewrites.

**Fix applied**: Added EVOLVE-BLOCK markers around 3 regions in the concatenated seed: `pipeline_constants` (ring size, timeout), `pipeline_loop` (the hot transfer loop), and `dma_buffer_creation` (the full dma.rs section).

### ShinkaEvolve Issue #7: Evaluator CLI Incompatibility

After adding EVOLVE-BLOCK markers, ShinkaEvolve successfully generated and applied patches (metadata shows `"success": true`, `"num_applied": 1`). However, all evaluations returned score 0.

**Root cause**: ShinkaEvolve calls the evaluator as `python evaluate.py --program_path X --results_dir Y`. Our evaluator only handled `--test` — for `--program_path` it fell through to `print("Usage: ...")` and exited without writing `metrics.json`. ShinkaEvolve interpreted the missing metrics as score 0.

The backend's auto-append logic (`if "__main__" not in eval_str: append wrapper`) was defeated because our evaluator already has `if __name__ == "__main__":`.

**Fix**: Added `--program_path` / `--results_dir` handling to `evaluate_p2p.py`'s `__main__` block.

### ShinkaEvolve Issue #8: File Format Mismatch

After fixing the CLI, the evaluator runs but reports `"No recognized files in candidate"`. ShinkaEvolve passes the patched EVOLVE-BLOCK content as a single file without `// --- FILE:` markers. The evaluator's `evaluate(path)` function expects the concatenated format with file markers to split into multiple source files.

**Root cause found**: ShinkaEvolve DOES preserve the `// --- FILE:` markers in the patched output. The actual problem was that the evaluator uses `REPO_ROOT = Path(__file__).resolve().parents[3]` to find target files. ShinkaEvolve copies the evaluator to `results/shinkaevolve/output/evaluate.py` — from that deeper path, `parents[3]` resolves to `evolve_p2p/` instead of the actual repo root `ai-native-storage-certus/`. All target paths (`pipeline.rs`, `dma.rs`) don't exist → "No recognized files."

**Fix #9**: Changed `REPO_ROOT` to walk up parents until it finds a `Cargo.toml` (the actual repo root marker), making it location-independent.

### ShinkaEvolve Final Result

After 9 integration fixes, ShinkaEvolve ran 10 generations (476s, 14 total candidates):
- Gen 0 (5 candidates): scored **0.1976** — compiles, runs, slight regression from baseline
- Gen 1-4: build failures (score 0.0) — ambitious patches couldn't compile
- **Gen 5: scored 0.2073** — beat the baseline (+2.8% over 0.2017)
- Gen 6-9: build failures (score 0.0)

ShinkaEvolve is the third framework (alongside GEPA/AdaEvolve) to produce compiling candidates, and one of only two (with AdaEvolve) to beat the baseline. The improvement is minor knob tuning (+2.8%), not architectural — same as AdaEvolve's +4.1%. The pattern holds: conservative changes can marginally improve performance; ambitious structural changes break the build.

### Integration Fix Summary (ShinkaEvolve: 9 layers)

1. Import: `AsyncEvolutionRunner` → `ShinkaEvolveRunner`
2. Module path: `shinka.core.runner` → `shinka.core`
3. Async: `runner.run()` → `runner.run_async()`
4. Model resolver: added `local/<model>@<url>/v1` format
5. Missing dependency: installed into skydiscover venv
6. API key routing: added `?api_key_env=OPENAI_API_KEY` to model URL
7. Evaluator CLI: added `--program_path`/`--results_dir` handling
8. EVOLVE-BLOCK markers: added to concatenated seed
9. REPO_ROOT resolution: walk parents to find `Cargo.toml` instead of hardcoded `parents[3]`

### K-Search Final Result (Run 2 — no bottleneck hints, no "pipeline" framing)

K-Search ran 10 evaluations (0 passed, completed all rounds within 3000s timeout):
- Rounds 1-5: Attempted "GPUDirect Storage (cuFile) for NVMe→GPU direct DMA" — all build failures. World model marked it "too hard" and downrated from 8.0/10 to 3.0/10.
- Rounds 6-10: Switched to "Deep pipeline: increase streams to 8, ring to 32, maximize NVMe QD" — also all build failures. Marked "too hard" after round 10.

World model action tree (final state):
- `action_gds` (GDS/cuFile): rated 3.0/10 — too hard
- `action_spdk_p2p` (SPDK P2P via gdrdrv): rated 4.0/10 — never attempted (GDS + deep pipeline consumed all rounds)
- `action_deep_pipeline` (deep pipeline): rated 7.5/10 → too hard

K-Search consistently identifies P2P as the highest-value action regardless of prompt framing (tested with and without "Key Bottlenecks" section, with and without "pipeline" in task title). The world model adapts correctly after failures but cannot produce compiling Rust for any action.

### K-Search Root Cause: Full-File Rewrite Without Base Code

K-Search's `generate()` log shows `parent_is_root=yes base_code=no` — meaning it generates code from the spec+action prompt alone, without seeing any working base code. Even "easy" changes (increase ring size 8→32) require the LLM to reproduce 400+ lines of pipeline.rs perfectly from memory. If it misses a single `// --- FILE:` marker, drops an import, or garbles a type, the evaluator gets garbage.

**Fix applied**: Added `continue_from_solution="wild_type"` to the `generate()` call. This seeds the world model with the working wild-type code, so subsequent action cycles use the "base_code+action" prompt ("start from this working code and apply this change") instead of "spec+action" ("generate from scratch"). The LLM should now produce modifications rather than full rewrites.

This is the same distinction that makes Nous successful — it edits known-good code incrementally rather than regenerating files from scratch.

K-Search's world model correctly identified and prioritized the P2P architecture, adapted after failure (downrated difficulty, switched strategy), but couldn't produce compiling Rust for either action. Same bottleneck as all other frameworks: implementation, not discovery.

---

## 2026-06-01: Framework Context / Prompt Mapping (No-Hint Control)

How each framework receives its context in the no-hint control run. All get roughly the same information level: hardware specs (including nvidia-peermem, gdrdrv), scoring formula, file structure, and `--features p2p` in the build command. None get explicit FFI type hints or directed "use P2P" instructions.

### GEPA Native

- **Context source**: `BACKGROUND` string hardcoded in `run_gepa_p2p.py`
- **Content**: Hardware (nvidia-peermem, gdrdrv), scoring formula, build command (`--features p2p`), "gpu-services crate has DMA buffer creation functions for various memory types"
- **Seed files**: `pipeline.rs`, `dma.rs` (raw source, <800 lines each)
- **Evaluator**: Direct Python call to `evaluate_p2p.py`

### SkyDiscover Frameworks (AdaEvolve, EvoX, OpenEvolve, ShinkaEvolve)

- **Context source**: `configs/skydiscover/config.yaml` → `prompt.system_message`
- **Content**: "Hardware: NVMe Gen4 SSDs via SPDK, NVIDIA A30 GPU PCIe Gen4 x16. Kernel modules loaded: nvidia-peermem, gdrdrv. The gpu-services crate has DMA buffer creation functions for various memory types."
- **Seed**: Concatenated `pipeline.rs` + `dma.rs` with `// --- FILE:` markers
- **Evaluator**: `evaluate_p2p.py` called as CLI subprocess

### K-Search (Native WorldModel)

- **Context source**: `CertusP2PTask.DEFINITION_TEXT` hardcoded in `/home/nara/certus/evo_frameworks/K-Search/k_search/tasks/certus_p2p_task.py` (lines 35-79)
- **Content**: Hardware (nvidia-peermem, gdrdrv, 2048 hugepages), scoring formula, file structure, output format requirements. Also mentions "Key Bottlenecks" (ring depth, cudaMemcpy stage, H2D bandwidth limit).
- **Seed**: Wild-type files loaded by `CertusP2PTask._load_seed()`
- **Evaluator**: Instrumented `task.run_benchmark()` → calls `evaluate_p2p.py` internally
- **World model**: K-Search proposes action nodes (LLM-generated strategy tree), picks highest-rated, generates code, evaluates, marks "too hard" after 5 consecutive failures

### Nous

- **Context source**: `configs/nous/campaign.yaml` → `target_system.description`
- **Content**: Hardware (nvidia-peermem, gdrdrv), scoring formula, build command (`--features p2p`), benchmark commands, key files, "DMA buffer creation functions for various memory types"
- **Mode**: Full repo access — Nous drives Claude to edit files, build, and benchmark directly (no external evaluator)

### Context Equivalence

All frameworks receive the same implicit P2P signal: hardware mentions nvidia-peermem/gdrdrv, build enables `--features p2p`, and gpu-services is described as having "various memory types." No framework is told *to use* P2P — they must discover it from the hardware hints and codebase exploration. This is the no-hint control condition.

---

## 2026-06-02: Agentic Framework Results (Nous + AutoScientists)

### Nous (best score: 0.4808, 5.96 GB/s)

- **3 iterations × 3 arms each** = 9 evaluations in ~100 minutes
- **Strategy**: Hypothesis-driven. Final winning hypothesis: "increase batch_lookup per-thread NVMe queue depth from 8 to 32-64 and expand thread count from 2 to 4"
- **Best config**: 4 queues/drive, QD32/thread, 4 CUDA streams, periodic sync removed
- **Key insight**: Tested ablation arms that isolated individual changes — the ablation arm (QD64 + removed sync without extra threads) actually outperformed h-main (5.96 vs 5.63 GB/s), proving that the deeper queue was more important than extra threads
- **Limitation**: Did not discover P2P/GPUDirect path. Stayed within the host-bounce pipeline architecture.

### AutoScientists (best score: 0.3366, 3.94 GB/s)

- **10 iterations** in ~28 minutes (fastest agentic framework)
- **Strategy**: Greedy hill-climbing. One change per iteration, revert on regression.
- **Best config**: 2 queues/drive, QD32, flexible stream count, periodic sync removed, pre-allocated memory-tier slots
- **Key optimizations**: Removed periodic GPU sync (biggest single win); 2 NVMe queues with QD=64 initially then backed to QD=32; pre-allocated MT slots before I/O threads
- **Why lower than nous**: Stopped exploring after finding a local optimum (2 queues). Never tried 4 queues or deeper parallelism. Sequential hill-climbing can't escape local optima.

### AutoScientists Permission Fix

AutoScientists initially scored 0.0 (zero iterations) because the runner launched `claude -p` without `--dangerously-skip-permissions`. The Claude session couldn't run `cargo`, `python3`, or write to source files. Fixed by adding permission flags and `--add-dir` for the repo.

### Evaluator Does Not Protect Against Agentic Edits

The evaluator's backup/restore cycle only covers its own patch operations. When an agentic framework (Claude session) edits files directly and then calls the evaluator, the evaluator backs up the *already-modified* file, scores it, then restores to... the modified version. The original wild-type is lost.

**Fix**: Added backup/restore at the `run_experiment.py` level — all target source files are backed up before launching an agentic framework and restored in `finally` after it exits.

---

## 2026-06-02: Coding Agent Framework (In Progress)

### Design

A lean alternative to AutoScientists — same concept (Claude with full repo access, iterative optimization) but with:
- Minimal prompt (no architecture walkthrough, no bottleneck hints)
- `--output-format json` for accurate cost/token tracking
- Instruction to save per-iteration candidates to `results/coding_agent/candidates/gen_N/`
- Same context as other frameworks: scoring formula, hardware specs, file scope, build command

### Result: 0.3896 (4.59 GB/s, 1.00ms p99) — $11.65, 42 minutes, 9 iterations

Beat autoscientists (0.3366) but below nous (0.4808). Key optimizations found:
1. Multi-object interleaved pipeline (+5.6%): process all cold objects simultaneously
2. Sequential priming (+3.1%): maximize SSD read-ahead
3. Queue depth 36-38 (+3.6%): slight overlap at object boundaries
4. 4 CUDA streams (+2.6%): more GPU H2D parallelism
5. `memcpy_h2d_async` bypass (minor): avoid DmaBuffer mutex

### Critical Finding: Local Optimum Trap on Queue Depth

The coding agent **never tried more than 2 queues per drive** (`MAX_QUEUES_PER_DRIVE` stayed at 2 across all 9 iterations). It only tuned `queue_depth` (32→36→38), which gives diminishing returns with 2 queues:

- Coding agent: 2 queues × QD38 = 76 max in-flight → 4.59 GB/s
- Nous: 4 queues × QD32 = 128 max in-flight → 5.96 GB/s

The agent interpreted diminishing QD returns as "near the sweet spot" and pivoted to other optimizations (coalescing, streams). It never asked "what if I doubled the queues instead?" — a structural change vs. a parameter tweak.

**This is the exact failure mode Glia's supervisor intervention is designed to break.** After 3 iterations of diminishing gains on QD tuning, a supervisor would ask: "Is there a structural limitation? What would 2x improvement require architecturally?" That nudge toward `MAX_QUEUES_PER_DRIVE` is the key insight nous discovered through ablation.

### Hypothesis (Original)

Sequential iterate-with-feedback should match or beat AutoScientists' greedy approach because:
1. The prompt explicitly asks the agent to reason about what it learned after each iteration
2. No prescriptive architecture guidance — the agent must discover optimizations from the code itself
3. Fair comparison: same hardware context, same files in scope, same scoring

---

## 2026-06-02: Coding Agent SDK (CORAL + Glia Patterns)

### Design

Built on the Claude Agent SDK (`claude-agent-sdk` Python package). Uses a **continuous session** with programmatic supervisor intervention.

1. **Continuous session with resume**: Unlike the per-iteration approach (which lost context between turns), the SDK maintains one long session via `resume=session_id`. The agent keeps its full conversation history — everything it read, tried, and learned. The SDK orchestrator can inject new prompts into this session between turns.

2. **Glia's supervisor intervention**: After N iterations where score improves by less than 1% (default: 3 iterations × <0.01 absolute gain), the orchestrator injects a supervisor prompt into the session:
   - "Is there a structural limitation in your current approach?"
   - "What does the hardware ceiling suggest about where throughput is being left on the table?"
   - "Are there files in scope you haven't read or fully explored yet?"
   - "What would a 2x improvement require — not parameter tuning, but a fundamentally different approach?"
   
   The supervisor does NOT mention P2P, dma.rs, or any specific optimization. It's generic structural nudging.

3. **Marginal-gain stagnation detection**: The previous per-iteration SDK version failed because `score > best_score` reset the stagnation counter even on +0.0008 gains. The fix: require `score > best_score + min_improvement` (default 0.01 = ~0.12 GB/s real throughput gain). Tiny incremental wins no longer mask stagnation.

### Key Differences from Plain Coding Agent

| | coding_agent | coding_agent_sdk |
|---|---|---|
| **Launch** | `claude -p` (one long session, 125 turns) | SDK `query()` with `resume` (same long session, programmatic control between turns) |
| **Stagnation response** | None — keeps hill-climbing | Supervisor fires after 3 iters with <1% gain |
| **Stagnation definition** | N/A | `score > best + 0.01` required for "real improvement" |
| **Session continuity** | Full (one subprocess) | Full (SDK resume maintains conversation) |
| **P2P nudge** | Never explored dma.rs | Supervisor asks generic "files unexplored?" — no P2P hint |
| **Cost tracking** | Aggregate at end | Per-turn via SDK |
| **Max turns** | Unlimited (ran 125) | 50 per SDK call, unlimited overall (1hr timeout) |

### Hypothesis

The supervisor intervention should break the QD local optimum that trapped the plain coding_agent. The plain agent spent iterations 5-8 making <1% gains (QD 36→38, simplified coalescing). With the SDK's marginal-gain detection, these would trigger supervisor at iteration ~5, forcing the agent to think structurally while retaining full memory of what it already tried. The session continuity means the supervisor prompt lands in context where the agent already knows "QD tuning plateaued" — it just needs the nudge to try something bigger (more queues, or explore dma.rs).

### Context Parity Across All Frameworks

| Framework | Scoring formula | Hardware specs | File scope | Build cmd | Architecture hints |
|-----------|:-:|:-:|:-:|:-:|:-:|
| gepa_native | ✅ | ✅ | ✅ | ✅ | ❌ |
| skydiscover variants | ✅ | ✅ | ✅ | ✅ | ❌ |
| ksearch | ✅ | ✅ | ✅ | ✅ | ❌ |
| nous | ✅ | ✅ | ✅ | ✅ | ❌ |
| autoscientists | ✅ | ✅ | ✅ | ✅ | ❌ |
| coding_agent | ✅ | ✅ | ✅ | ✅ | ❌ |
| coding_agent_sdk | ✅ | ✅ | ✅ | ✅ | ❌ (supervisor hints on stagnation only) |

All receive nvidia-peermem/gdrdrv in hardware description and `--features p2p` in build command. None are told what the bottleneck is or what architectural change to make.

---

## 2026-06-02: Why No Framework Successfully Implemented P2P

### The Discovery Asymmetry

**K-Search and GEPA** received the concatenated source files (pipeline.rs + dma.rs) as their **seed program** — the LLM literally sees the P2P functions (`create_spdk_dma_buffer_from_gpu_bar`, `GpuDirectBuffer`, the `#[cfg(feature = "p2p")]` blocks) in its context window from the start. It's impossible to miss them.

**The agentic frameworks** (nous, autoscientists, coding_agent) are told "Files in scope: pipeline.rs, lib.rs, dma.rs" but they have to **actively choose to read** dma.rs. And when they do read it, the P2P functions are behind `#[cfg(feature = "p2p")]` — which means if you just skim the file looking for the hot path, you see the non-P2P DMA buffer functions and move on. The P2P code looks like dead/optional code.

### The Pattern (observed across all agentic runs)

1. Agent reads pipeline.rs (the hot path) → sees obvious tuning knobs (QD, streams, sync)
2. Agent reads lib.rs (the dispatcher) → sees queue allocation, thread spawning
3. Agent maybe reads dma.rs → sees buffer creation but the P2P functions are gated behind `cfg(feature = "p2p")` and have comments like "only used by the P2P (GDRCopy) path" — which signals "this is for a different code path, not the one I'm optimizing"

The agent doesn't realize that **enabling** the P2P path IS the optimization. It thinks P2P is a separate feature, not an alternative implementation of the same pipeline it's tuning.

### K-Search Got It Right (But Couldn't Compile)

K-Search's world model correctly identified the opportunity: "GPUDirect Storage P2P path — Use nvidia-peermem/gdrdrv to enable GPUDirect Storage, allocate GPU BAR1-mapped buffers, register them with SPDK, issue NVMe reads directly into GPU memory. Eliminates host DRAM bounce entirely."

It failed because it couldn't produce type-correct Rust FFI code — every attempt to call `create_gpu_dma_buffer` in dma.rs failed with `cannot find function` (the function name was wrong; the actual function is `create_spdk_dma_buffer_from_gpu_bar`).

### Implications

This reveals THREE distinct failure modes:

1. **Discovery failure** (agentic frameworks: nous, autoscientists, coding_agent): Never explored dma.rs deeply enough to identify P2P as an optimization opportunity. Lazy exploration driven by hot-path tracing — they read what's called, not what else exists.

2. **Coordination failure** (evolutionary frameworks: AdaEvolve, EvoX, OpenEvolve, ShinkaEvolve): These frameworks DO see P2P code (it's in their concatenated seed). But implementing P2P requires coherent multi-section changes (dma.rs: new buffer allocation + pipeline.rs: new transfer loop + lib.rs: new wiring). Evolutionary mutation changes one section per iteration — when it modifies pipeline.rs to call the P2P function, it doesn't simultaneously update lib.rs to pass the right arguments. The result doesn't compile, gets score 0, gets discarded, and the population reverts to safe baseline. They attempted P2P but can't coordinate the multi-file change in a single mutation step.

3. **Implementation failure** (K-Search, GEPA): Identified the correct strategy AND attempted coordinated multi-file changes, but couldn't produce type-correct Rust FFI code. K-Search used the wrong function name (`create_gpu_dma_buffer` instead of `create_spdk_dma_buffer_from_gpu_bar`). GEPA had u32-vs-c_int type mismatches on cuda_ffi calls. Both have compile-error feedback loops but couldn't overcome these specific errors within their iteration budget.

### Failure Mode Hierarchy

| Failure mode | Frameworks | Saw P2P? | Tried P2P? | Compiled P2P? |
|---|---|:-:|:-:|:-:|
| Discovery | nous, autoscientists, coding_agent | ❌ | ❌ | ❌ |
| Coordination | AdaEvolve, EvoX, OpenEvolve, ShinkaEvolve | ✅ | ✅ (partial) | ❌ |
| Implementation | K-Search, GEPA | ✅ | ✅ (full) | ❌ |

### What Would Fix Each

- **Discovery**: Include full dma.rs in initial context (forced exploration), OR supervisor asks "what haven't you explored?"
- **Coordination**: Multi-file atomic mutations (not single-section), OR agentic execution (can edit multiple files in sequence)
- **Implementation**: Correct function names + FFI type signatures in context (the hints run tests this)

---

## 2026-06-02: P2P Hints Run — Early Findings

### GEPA with Hints: Coordination Failure Persists

GEPA with explicit P2P direction scored 0.2072 (baseline) across all 10 iterations. The hints did NOT fix GEPA's fundamental limitation.

**What happened:**
- Iterations 1,3,5,7,9: Generated safe baseline code (score 0.2072) — no P2P attempted
- Iterations 2,4,6,8,10: Generated P2P code that compiled (with dead_code warnings) but scored 0.0 — evaluator reported `build_succeeded: false` due to score reporting bug, but the real issue is the P2P code was dead (unused)

**Root cause — coordination failure persists even with hints:**

GEPA regenerates entire files from scratch each iteration. Even when it knows about P2P (the hint says "implement GPUDirect Storage"), it can't coordinate changes across pipeline.rs + lib.rs + dma.rs in a single coherent rewrite. It adds P2P structs and functions to dma.rs but doesn't wire them into the actual `pipelined_ssd_to_gpu_zero_copy` call in pipeline.rs. The old host-bounce path remains active. Score stays at baseline because P2P code exists but is dead code.

**Why hints don't fix coordination:**
- Hints address the DISCOVERY problem ("what to build") 
- They do NOT address the COORDINATION problem ("how to wire 3 files together coherently")
- GEPA's per-file regeneration model fundamentally can't do multi-file coordination in one mutation
- Each iteration is independent — it can't "first fix dma.rs, then update pipeline.rs to call it"

**Implication for Stratum:**
- Phase 2 (architectural change) MUST use an agentic framework (coding_agent, nous) — not evolutionary
- Evolutionary frameworks are suitable for Phase 0 (single-knob sweep) and Phase 1 (single-file structural) only
- The hint experiment confirms: coordination failure > discovery failure as the binding constraint for architectural changes

### Evaluator Bug: Warnings Misreported as Build Failures

GEPA's score reporter interprets any stderr output as build failure. Rust `dead_code` warnings appear in stderr even when the build succeeds (exit code 0). This caused `build_succeeded: false` for iterations that actually compiled fine.

**Impact:** Dashboard shows higher build failure rate than reality. The code compiles but the evaluator wrapper misclassifies warnings.

**Fix needed:** Score reporter should check exit code, not stderr presence.

### Evo Frameworks with Hints: P2P Direction Made Them WORSE

**Scores with hints vs without:**
| Framework | No hints | With P2P hints |
|-----------|---------|----------------|
| adaevolve | 0.0 (timed out) | 0.0 (100% build failures) |
| evox | 0.0 (timed out) | 0.0 (100% build failures) |
| openevolve | 0.2158 (baseline) | 0.0 (100% build failures) |
| gepa_native | 0.2017 (baseline) | 0.2072 (baseline, P2P dead code) |

**What happened:** The P2P hint pushed every mutation attempt toward the architectural change. Without hints, these frameworks made safe incremental mutations (tweak constants, reorder loops) that at least compiled. With the hint saying "Implement GPUDirect Storage," every iteration aggressively tried P2P code — and failed to compile because they can't coordinate multi-section changes.

**Root cause:** These frameworks use a concatenated seed (`pipeline.rs + lib.rs + dma.rs` with `// --- FILE: ---` markers). They rewrite the entire concatenated file each iteration. The P2P hint makes them try to add `create_spdk_dma_buffer_from_gpu_bar` calls in the pipeline section, but the dma section's function isn't wired up correctly in the same rewrite. One section references something the other section doesn't define or export properly.

**Key insight: Explicit architectural direction can HURT evolutionary frameworks** by pushing them out of their competence zone. They're better at incremental refinement without a strong direction that exceeds their coordination ability. The hint forces ambitious attempts they can't execute, destroying their ability to produce ANY working code.

**Implication for Stratum:** Never give architectural hints to evolutionary frameworks. Use them for Phase 0 (knob sweep) only — where the changes are single-parameter and always compile. Architectural changes (Phase 2) must use agentic frameworks exclusively.

### CORRECTION: Evo Frameworks Did NOT Fail to Compile

Earlier analysis was wrong. Upon checking the actual logs:
- adaevolve: 30 evals, ALL compiled, best 0.2166 (baseline)
- evox: 29 evals, ALL compiled, best 0.2128 (baseline)
- openevolve: 29 evals, ALL compiled, best 0.2093 (baseline)

They didn't have "100% build failures." They compiled fine every time — they just **ignored the P2P hint** and made safe conservative mutations. The evolutionary mutation engine defaults to small parameter tweaks regardless of what the system prompt says.

**Corrected diagnosis:** The P2P hint didn't make them worse (as originally claimed). It was simply irrelevant — they can't execute multi-section architectural changes regardless of the prompt. They stayed at baseline because their mutations are too conservative to implement P2P, not because they tried and failed.

**Corrected insight:** Architectural hints are wasted on evolutionary frameworks. They don't "try and fail" — they don't try at all. The mutation engine generates variations of existing code patterns, not new architectures from descriptions.

### Feature Gate Propagation Bug — P2P Invisible to Dispatcher

**Problem:** `pipeline.rs` (in the `dispatcher` crate) couldn't call `gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar` even with `--features p2p`. The function compiled in gpu-services but was invisible to the dispatcher.

**Root cause:** Feature propagation was incomplete:
- `certus-server/Cargo.toml`: `p2p = ["gpu-services/p2p"]` — enabled P2P in gpu-services
- `dispatcher/Cargo.toml`: `gpu-services = { features = ["spdk", "gpu"] }` — NO p2p feature

When building `--features p2p`, certus-server enabled gpu-services/p2p directly, but dispatcher still compiled gpu-services without the p2p feature. The P2P functions were conditionally compiled only for certus-server's direct dependency, not for dispatcher's.

**Fix:** Added feature forwarding:
- `dispatcher/Cargo.toml`: `p2p = ["gpu-services/p2p"]`
- `certus-server/Cargo.toml`: `p2p = ["gpu-services/p2p", "dispatcher/p2p"]`

Now `--features p2p` propagates: certus-server → dispatcher → gpu-services. Pipeline.rs can call P2P functions directly.

**Impact on evolution experiment:** This means P2P CAN be implemented entirely within pipeline.rs + dma.rs — no lib.rs changes needed. The `PipelineRing::new()` function in pipeline.rs can allocate GPU BAR buffers by calling `gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar` directly.

**Lesson:** Feature gates in Rust workspace crates require explicit forwarding through every crate in the dependency chain. A feature enabled on a leaf crate doesn't automatically propagate to intermediate crates that also depend on it.

### K-Search with Hints: Correct Plan, Wrong Code (LLM Recall Failure)

**Result:** 0/10 P2P iterations compiled. All failed with `cannot find function create_gpu_dma_buffer in module dma`.

**What happened:**
- K-Search's world model correctly planned: "implement `create_spdk_dma_buffer_from_gpu_bar()` to allocate GPU BAR1 memory"
- The correct function name was in the hint AND in the dma.rs seed code
- But codegen LLM generated `create_gpu_dma_buffer` — an invented shorter name
- Error feedback ("cannot find function") was fed back each iteration but the LLM kept generating the same wrong name

**Root cause — LLM recall failure on long identifiers:**
When generating Rust code, the LLM draws on its training distribution for function names rather than faithfully reproducing a specific 37-character name from the system prompt. Shorter, more "natural" names (`create_gpu_dma_buffer`) get prioritized over the actual name (`create_spdk_dma_buffer_from_gpu_bar`) because the LLM's prior for "GPU DMA buffer creation function" is stronger than its ability to copy an exact token sequence from context.

This is NOT a context window issue (the name is right there). It's a code generation fidelity issue — the model "knows" the function should exist but reconstructs its name from semantic memory instead of copying from the prompt.

**K-Search specific factor:** The world model creates a high-level plan, then a SEPARATE codegen LLM call generates the actual code. The plan says the right name, but the codegen step doesn't have the plan's exact text — it has a summary. The name gets corrupted in the summarization/handoff between planning and code generation.

**What would fix it:**
- Include the exact function call as a code snippet in the codegen prompt (not just a description)
- Or: use an agentic approach where the LLM reads the actual source file and copies the function name directly (nous/coding_agent approach)

---

## Hardware Fundamentals (Reference)

### Queue Depth (QD) Explained

QD = how many NVMe read commands are in-flight simultaneously on a single I/O queue. The NVMe SSD has an internal command queue. When you submit a read, the drive starts processing it. You don't wait for completion before submitting the next — you can have QD=64 reads processing in parallel.

**Why QD matters:**
- NVMe drives have internal parallelism (multiple NAND channels, controller pipelines)
- At QD=1: drive processes one read at a time, most hardware sits idle between commands
- At QD=32-64: drive can pipeline reads across channels, achieves near-maximum throughput
- Our drive (Intel P5800X): peaks at QD=64 (~5.92 GB/s sequential read, 128KiB blocks)

**Total pipeline depth = num_queues × QD_per_queue:**
- Nous: 4 queues × QD32 = 128 in-flight → 5.96 GB/s (fully saturated drive)
- Coding agent: 2 queues × QD38 = 76 in-flight → 4.59 GB/s (drive partially idle)
- Baseline: 1 queue × QD16 = 16 in-flight → 2.4 GB/s (severely underutilized)

### Hardware Ceiling Clarification

The 5.92 GB/s "ceiling" is **NVMe SSD → host DRAM** (measured with spdk_nvme_perf). It does NOT involve the GPU.

For the full SSD-to-GPU path:
- **Host-bounce** (current): SSD → host DRAM (5.9 GB/s limit) → GPU via cudaMemcpy (16.8 GB/s). Bottleneck = first hop.
- **P2P/GPUDirect**: SSD → GPU BAR1 directly. Still limited by drive's PCIe bandwidth (~5.9 GB/s).

P2P does NOT make the drive faster. It eliminates the second copy and reduces latency:
- Latency drops (one PCIe hop instead of two)
- CPU is completely out of the data path
- Host DRAM bandwidth is freed
- Pipeline can stay fuller (no stalls waiting for H2D copy)

The score improvement from P2P comes from: (1) latency reduction (p99 drops → better latency component), and (2) better pipeline utilization (no cudaMemcpy stalls keeping the drive's queue fuller).

### Nous P2P: Compiled and Ran, But Slower (-19%) — Wrong Design

**Result:** P2P compiled, data integrity passed, but 1.98 GB/s vs 2.43 GB/s baseline (-19%).

**What nous implemented:**
```
NVMe → GPU BAR1 staging ring → D2D copy (cudaMemcpyDeviceToDevice) → final gpu_dst
```

**vs host-bounce baseline:**
```
NVMe → host DRAM ring → H2D copy (cudaMemcpyHostToDevice) → final gpu_dst
```

Both are 2 hops. Nous replaced "host DRAM" with "GPU BAR1" as the intermediate — same number of copies, different memory type. Not true P2P.

**Why nous added the D2D copy:**
The caller provides a `gpu_dst` pointer (the final destination where the inference engine expects data). The BAR1 ring buffer is at a different address than `gpu_dst`. Nous reasoned: "data lands in BAR1 ring, but the caller wants it at `gpu_dst`, so I need to copy it there."

This is a reasonable assumption IF you treat the BAR1 ring as internal staging. But it defeats the purpose of P2P — you've just moved the intermediate from host DRAM to GPU BAR1 without eliminating a copy.

**The correct designs (that nous didn't try):**
1. **Option A — register the destination**: Make `gpu_dst` itself be the BAR1-registered buffer. Register the caller's buffer with SPDK once, then NVMe DMAs directly to the final destination. Zero intermediate copies.
2. **Option B — return ring pointer**: Change the interface so the caller reads directly from the BAR1 ring slot. Don't copy to a separate `gpu_dst`. The compute kernel reads from BAR1 memory directly.

**Root cause:** Nous thought like a software engineer (preserve the existing API, add staging) not a systems engineer (redesign the interface for zero-copy). The hint said "eliminate the host bounce" — and it did. But it didn't say "eliminate ALL intermediate copies." Nous eliminated host DRAM as the intermediate and replaced it with GPU BAR1 — technically P2P (NVMe goes to GPU) but architecturally the same pattern (staging + copy).

**Why it was slower:** BAR1 memory access from NVMe likely has higher per-transfer latency than host-pinned DRAM on this PCIe topology (A30 GPU). The P2P path goes through the GPU's PCIe interface which may add latency compared to direct host DRAM DMA. Plus the D2D copy within the GPU isn't free for 128KiB chunks.

**Implication for Stratum:** An analysis agent would have caught this immediately: "Still 2 copies. Nothing eliminated. The design is wrong." Without analysis, the agent just sees "score went down" and doesn't understand why.

---

## 2026-06-03: P2P Hints Run — Final Results

### Summary Table (Hint Run)

| Framework | Best Score | Throughput | P2P Attempted | P2P Compiled | P2P Worked | Wall Time |
|-----------|-----------|-----------|:---:|:---:|:---:|-----------|
| **coding_agent** | 0.3431 | 3.97 GB/s | ✅ | ✅ | ✅ | 32m |
| **autoscientists** | 0.3945 | 4.65 GB/s | ✅ | ✅ (runtime fail) | ❌ | 39m |
| **nous** | 0.2094 | 2.43 GB/s | ✅ | ✅ | ❌ | 120m (timeout) |
| **gepa_native** | 0.3314 | 3.87 GB/s | ❌ | ❌ | ❌ | 8m |
| **ksearch** | 0.2108 | — | ❌ | ❌ | ❌ | 18m |
| **openevolve** | 0.2116 | — | ❌ | ❌ | ❌ | 20m |

### Coding Agent: Successfully Implemented P2P (0.3431, 3.97 GB/s)

**Architecture implemented:**
```
NVMe → GPU BAR1 staging ring (64 slots, GDRCopy-mapped) → cudaMemcpyAsync D2D → final gpu_dst
```

**How it works:**
1. Pre-allocates 64 GPU memory slots via `cudaMalloc`
2. Maps each slot into BAR1 via GDRCopy (`gdr_pin_buffer` + `gdr_map`)
3. Registers the BAR1 addresses with SPDK (`spdk_mem_register`)
4. NVMe controller DMAs directly into GPU BAR1 memory (PCIe posted write)
5. After each NVMe read completes, `cudaMemcpyAsync(DeviceToDevice)` copies from staging slot to final `gpu_dst`
6. D2D copy runs at GPU internal bandwidth (~600 GB/s for 128KiB) — effectively zero-cost

**Iterative optimization (8 iterations):**
| Iter | Change | Score | Throughput |
|------|--------|-------|-----------|
| 0 | P2P staging ring + D2D | 0.3076 | 3.66 GB/s |
| 1 | Async D2H backfill (don't block) | 0.3168 | 3.79 GB/s |
| 2 | Remove D2H backfill entirely | 0.3321 | 3.90 GB/s |
| 3 | Lazy sync (only on ring recycle) | 0.3407 | 4.00 GB/s |
| 4 | Remove final stream sync | 0.3393 | 3.97 GB/s |
| 5 | 64 ring slots + thread partitioning | 0.3309 | 3.97 GB/s |
| 6 | Skip evict_for_space in P2P path | 0.3328 | 3.92 GB/s |
| 7 | Reduce ring lock contention | 0.3431 | 3.97 GB/s |

**Why it succeeded where nous failed:** See section below.

### Nous: Same Architecture, Opposite Result (0.2094, all 3 iterations REFUTED)

Nous attempted P2P with the same GDRCopy BAR1 + D2D architecture across 3 iterations:

**Iter 1 — Batch P2P (REFUTED, 19% slower):**
- Implementation: Read ALL chunks into BAR1 ring, THEN issue D2D copies
- Problem: Lost the sliding-window pipeline overlap that the baseline enjoys
- Score: 1.98 GB/s vs 2.43 GB/s baseline

**Iter 2 — Sliding-window P2P (REFUTED, 140x slower):**
- Implementation: Sliding-window with D2D copy after each NVMe completion (same as coding_agent)
- Problem: 0.01 GB/s. Agent reported "GPU L2 cache coherence — external PCIe DMA doesn't invalidate L2, so D2D reads stale data" and "BAR1 VA falls back to CUDA pageable memory path (~10ms/128KiB)"
- Root cause unclear: coding_agent used the SAME pattern and got 3.97 GB/s (see below)

**Iter 3 — nvidia-peermem direct (REFUTED, no improvement):**
- Implementation: `spdk_mem_register` on the final `gpu_dst` pointer directly (skip staging ring entirely)
- Problem: No improvement over baseline (2.41 GB/s). Likely `spdk_mem_register` failed silently on the IPC pointer.

### The Nous vs Coding Agent Mystery

Both used: `cudaMalloc` → GDRCopy BAR1 map → SPDK register → NVMe DMA to BAR1 → `cudaMemcpyAsync(D2D)` to `gpu_dst`

**Coding agent got 3.97 GB/s. Nous got 0.01 GB/s.** Same architecture.

Possible explanations:
1. **Timing difference**: Coding agent's sliding-window has enough latency between NVMe write and D2D read that L2 is naturally invalidated (128KiB NVMe reads take ~22µs; L2 eviction timer may clear the line before the D2D read arrives). Nous's implementation may have issued D2D too soon.
2. **Buffer identity**: Coding agent copies from `p2p_ring.dev_ptrs[slot]` (the original `cudaMalloc` pointer). If nous used the GDRCopy-mapped VA instead of the original dev_ptr, CUDA would treat it differently.
3. **Stream scheduling**: Coding agent alternates between 2 streams and only syncs lazily. Nous may have had synchronization points that forced reads from stale cache.
4. **Executor implementation bug**: Nous drives a separate Claude session for each iteration (executor agent). The executor may have introduced a subtle bug that coding_agent (single continuous session maintaining state) avoided.

**This is the most significant finding of the hint experiment:** The P2P architecture IS viable on this hardware, but success depends on implementation details that are invisible at the design level. An agent that reasons about hardware correctly (nous) can still fail due to subtle code-level differences that a brute-force iterative optimizer (coding_agent) happens to get right.

### AutoScientists: P2P Attempted, Runtime Failure, Fell Back (0.3945, 4.65 GB/s)

- Iter 2: Attempted P2P, code compiled, but reported GDRCopy failure at runtime (rc=22 on IPC-opened memory). Reverted immediately.
- Remaining iterations: Optimized host-bounce path — QD64, removed periodic stream sync, multi-object interleaved pipeline with overlapped H2D copies
- **Outperformed coding_agent** on raw score (0.3945 vs 0.3431) because it optimized the host-bounce path more aggressively instead of fighting with P2P staging overhead
- Key insight: P2P eliminates the DRAM bounce but adds staging ring overhead. For a single NVMe drive (~5.9 GB/s), the DRAM path with deep pipelining can be competitive with P2P.

### GEPA with Hints: Host-Bounce Optimization Only (0.3314, 3.87 GB/s)

- P2P function definitions reproduced from seed dma.rs but never called from pipeline.rs
- Coordination failure persists: cannot wire multi-file changes in a single rewrite
- Gains came from QD increase and sync removal — same knob tuning as no-hint run
- Confirms: evolutionary frameworks cannot implement architectural changes regardless of hints

### Revised Failure Mode Table (Hint Run)

| Failure mode | Frameworks | P2P Attempted? | P2P Compiled? | Outcome |
|---|---|:---:|:---:|---|
| **Success** | coding_agent | ✅ | ✅ | 3.97 GB/s (+72%) via GDRCopy BAR1 + D2D |
| **Runtime failure** | autoscientists | ✅ | ✅ | GDRCopy rc=22 on IPC memory; fell back to host-bounce 4.65 GB/s |
| **Implementation failure** | nous | ✅ | ✅ | Same architecture as coding_agent but 140x slower; likely timing/implementation bug |
| **Coordination failure** | gepa_native | ❌ | ❌ | Reproduced P2P definitions, never wired into pipeline |
| **No attempt** | ksearch, openevolve | ❌ | ❌ | Stuck at baseline with conservative mutations |

### Key Takeaways from Hint Run

1. **Only 1 of 6 frameworks successfully implemented P2P** (coding_agent). The hint (explicit P2P direction + FFI signatures) fixed the discovery problem but NOT the coordination or implementation problems.

2. **Hints can hurt evolutionary frameworks**: GEPA/ksearch/openevolve stayed at baseline with or without hints. Hints didn't make them worse (corrected from earlier analysis) but were completely irrelevant — evolutionary mutation engines ignore system prompt direction.

3. **Agentic frameworks attempted P2P but only coding_agent succeeded**: The iterative hill-climbing approach (try, evaluate, adjust) was more effective than nous's hypothesis-driven approach for this task. Nous reasoned correctly about the architecture but couldn't get the implementation right.

4. **AutoScientists found the best overall score without P2P**: 4.65 GB/s via aggressive host-bounce optimization outperformed coding_agent's 3.97 GB/s P2P. This suggests that for single-drive workloads, the P2P staging overhead may not be worth it — the simpler path with deep pipelining wins on net.

5. **The real P2P advantage is latency, not throughput**: Coding_agent's P2P path achieved 1.11ms p99 vs autoscientists' ~1.2ms. At multi-drive scale where the DRAM bounce becomes the bottleneck (not the drive), P2P would pull ahead.
