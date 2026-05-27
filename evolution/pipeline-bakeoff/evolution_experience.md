# Evolution Bakeoff — Engineering Experience

How we built the infrastructure to run 6 evolutionary frameworks against a live storage system.

## What We Built

### The Evaluator (`evaluator/evaluate.py`)

The core infrastructure that makes the bakeoff possible. None of this existed before — we wrote it from scratch to bridge the gap between "LLM generates code" and "code runs against real hardware."

**What it does:** Takes a candidate `pipeline.rs` (or multi-file directory), patches it into the live source tree, builds the server, restarts it, runs the benchmark, measures throughput, verifies data integrity, and returns a score — all in ~45 seconds.

**Key design decisions:**

1. **Backup/restore with `finally` block** — Every evaluation backs up all target files before patching, then unconditionally restores them in a `finally` block. This guarantees the source tree is never left in a broken state, regardless of whether the candidate compiles, crashes the server, or times out. Without this, a single bad candidate could brick all subsequent evaluations.

2. **`patch_candidate()` with three modes:**
   - Single-file: candidate replaces `pipeline.rs` directly (H1/H2)
   - Multi-file directory: maps filenames to target paths via `MULTI_FILE_MAP` (Nous H3)
   - Concatenated H3 format: detects `// === FILE: name.rs` section markers, splits and patches each section into the correct file (SkyDiscover H3)

3. **`evaluate()` as the SkyDiscover entry point** — SkyDiscover's evaluator protocol requires a function `evaluate(program_path: str) -> dict`. We implemented this as the single entry point that handles mode selection (fixed/mixed/concurrent) via `BAKEOFF_EVAL_MODE` environment variable.

4. **Server lifecycle management** — `kill_server()` / `start_server()` with gRPC health-check polling. The server takes ~3s to initialize SPDK and enumerate NVMe devices, so we poll gRPC readiness before running the benchmark.

5. **Data integrity verification** — After measuring throughput, a separate integrity check confirms the GPU received correct data (pattern=42 verification). Frameworks that achieve high throughput by corrupting the data path score zero.

6. **Multi-mode evaluation:**
   - `evaluate_fixed()`: Single size (4 MiB), single client — H1 baseline
   - `evaluate_mixed()`: Four sizes (1/2/4/16 MiB), equally weighted composite — H2
   - `evaluate_concurrent()`: 8 clients, multi-drive — H3

### The Orchestrator (`run_bakeoff.py`)

Runs all frameworks sequentially with consistent configuration. Handles:
- Framework-specific command building (SkyDiscover CLI vs Nous campaign runner vs K-Search wrapper)
- Per-framework timeout management (Nous needs 7200s, others get `iterations × 180s`)
- Score parsing from heterogeneous output formats (SkyDiscover checkpoints vs Nous findings.json)
- Auto-generated analysis after each framework completes
- Environment variable propagation (`BAKEOFF_EVAL_MODE`)

### The Initial Program (`initial_program.rs`)

Not just a copy of `pipeline.rs` — it's annotated with:
- `EVOLVE-BLOCK` markers around mutable regions (constants, sync logic, pipeline loop)
- Comments explaining the performance model (why QD matters, what sync frequency does)
- Hints about the search space (suggested ranges, hardware constraints)

For H3, we created a concatenated multi-file version (`initial_program_h3.rs`) with section delimiters — because SkyDiscover can only evolve one file, but the real optimization requires changes across service.rs + lib.rs + pipeline.rs.

### Multi-File Evolution via Concatenation

SkyDiscover (and most LLM-evolution frameworks) operate on a **single file**: they take one source file as input, ask the LLM to produce an improved version, and evaluate the output. But H3 requires evolving three files simultaneously (service.rs + lib.rs + pipeline.rs) because the Mutex bottleneck spans module boundaries.

**The solution: concatenate with section markers.**

We create a single `initial_program_h3.rs` that contains relevant sections from all three files, delimited by:
```
// === FILE: service.rs (lines 1-34, 122-170, 176-240) ===
<service.rs code — struct DispatcherService, impl, gRPC handlers>

// === FILE: lib.rs (lines 60-96, 106-131, 202-316, 632-673, 837-963) ===
<lib.rs code — component fields, promote_and_serve, pipeline_init, lookup_async>

// === FILE: pipeline.rs (full file) ===
<full pipeline.rs>
```

**Key design decisions:**

1. **Not the full files** — service.rs is 300+ lines and lib.rs is 1900+ lines. Including everything would blow the LLM's output budget on unchanged boilerplate. We include only the sections that matter for the optimization (Mutex patterns, lock scoping, pipeline constants).

2. **EVOLVE-BLOCK markers within sections** — Inside the lib.rs section, we use `// ===== EVOLVE-BLOCK: NAME =====` markers to identify replaceable regions. The evaluator patches only these regions into the full file, preserving surrounding code.

3. **pipeline.rs is full replacement** — Since pipeline.rs is entirely about the transfer loop (no unrelated code), the evaluator replaces it wholesale.

4. **service.rs uses struct/impl matching** — The evaluator finds `pub struct DispatcherService` and its `impl` block using brace-counting (not regex, because nested braces break regex), and replaces that span.

5. **The LLM must preserve the markers** — The system prompt explicitly tells the LLM to keep the `// === FILE: ...` delimiters in its output. If they're missing, the evaluator can't split the sections and the candidate scores zero.

**The evaluator flow for H3 concatenated candidates:**
```
1. Read candidate file
2. Detect "// === FILE:" markers → route to split_concatenated_h3()
3. Parse sections by marker (service.rs, lib.rs, pipeline.rs)
4. For pipeline.rs: write full content to target
5. For lib.rs: find matching EVOLVE-BLOCK regions in existing file, replace content
6. For service.rs: find DispatcherService struct+impl via brace-counting, replace
7. Build, benchmark, score
8. Restore all files from backup
```

**Why not just use `--agentic` mode?** SkyDiscover's agentic mode lets the LLM *read* other files for context, but it still only *outputs* one file. The concatenation trick lets us get multi-file output through a single-file interface.

**Alternative for Nous:** Since Nous controls Claude directly (not through SkyDiscover), it uses the actual `initial_program_h3/` directory with separate files. The evaluator's directory mode handles this natively via `MULTI_FILE_MAP`.

### EVOLVE-BLOCK Markers in Source

Added `// ===== EVOLVE-BLOCK: NAME =====` markers to the actual dispatcher source (`lib.rs`) around:
- `COMPONENT_FIELDS` — the Mutex definitions in `define_component!`
- `PROMOTE_AND_SERVE` — the cold lookup path with pipeline_ring + data_drives locks
- `PIPELINE_INIT` — where pipeline rings and CUDA streams are allocated
- `LOOKUP_ASYNC` — the hot dispatch path (warm vs cold routing)

These markers serve dual purpose: they guide the evaluator's patching logic (replace only marked regions when evolving lib.rs), and they signal to the LLM which code is fair game for modification.

### Framework Configs

Per-framework YAML configs for each hypothesis:
- `config.yaml` — H1 (fixed 4 MiB)
- `config-mixed.yaml` — H2 (mixed sizes)
- `config-concurrent.yaml` — H3 (multi-client, multi-file)

Each config contains a domain-specific system prompt explaining the hardware, bottlenecks, and evolution strategy. The prompts evolve across hypotheses — H3 configs include findings from H1/H2 (e.g., "QD=32 is already optimal, the real bottleneck is the outer Mutex").

## Lessons Learned

### What Worked

- **The backup/restore pattern is essential.** Frameworks produce broken code constantly (compile errors, runtime crashes, git merge markers in output). The `finally`-block restore is the only thing that keeps the pipeline running.

- **Data integrity checks catch cheaters.** Without verification, a framework could "optimize" by removing the DMA copy entirely and score infinite throughput on an empty buffer.

- **Environment variable for eval mode** (`BAKEOFF_EVAL_MODE`) was the right abstraction. It lets us run the same framework binary against different evaluation criteria without changing configs.

- **Test re-evaluation** (SkyDiscover's built-in "evaluate best candidate again at the end") is critical. It exposes whether the "best" score was a real improvement or a lucky system state. AdaEvolve's 7.49 → 3.77 re-eval proved its winning candidate was noise.

### What Didn't Work

- **SkyDiscover writes to the same output directory regardless of hypothesis.** H2 overwrote H1 checkpoints in the same `results/adaevolve/output/` path. We lost checkpoint_5 and checkpoint_10 from H1. Fix: per-hypothesis directories (implemented for H3 onwards).

- **Score parsing across frameworks is fragile.** SkyDiscover stores scores in checkpoint JSON, Nous stores them in findings.json with a different schema. The orchestrator's `_parse_scores()` frequently returns 0 for Nous because it can't find the expected format.

- **Baseline variance of ±50% makes absolute comparison meaningless.** A framework scoring 4.7 GB/s vs another at 4.2 GB/s tells you nothing — they could swap positions on the next run. Only controlled A/B within the same thermal/system state is meaningful.

- **16 MiB scores are inflated by memory-tier caching.** Objects that fit in the warm cache get served from RAM (~20 GB/s) rather than cold SSD (~5 GB/s). The composite score becomes dominated by a caching artifact. Fix for future: ensure all lookups are truly cold (evict between measurements).

- **Server restart failures cascade.** If one evaluation leaves the server in a broken state (SPDK doesn't release NVMe devices cleanly), subsequent evaluations fail with "Server failed to start within 15s." The evaluator kills the server in the `finally` block, but SPDK device cleanup is non-deterministic.

### What We'd Do Differently

- **Use the micro-benchmark (`gpu-bb-vs-p2p`) as evaluator instead of the full server.** It's 10× faster (~3s vs ~45s per eval), more deterministic, and eliminates server restart failures. The tradeoff is it doesn't test the full code path — but for pipeline constants, it's sufficient.

- **Run more Nous iterations.** Nous is expensive ($8-16 per campaign) but it's the only framework that isolates individual factors. The search frameworks all bundle changes and can't tell you which one mattered.

- **Lock the system state.** Pin CPU frequency, disable turbo boost, pre-warm the memory-tier to a known state before each eval. The current variance makes 10-iteration runs statistically weak.

## Cost Summary

| Framework | H1 (30 iter) | H2 (10 iter) | Estimated $/iter |
|-----------|-------------|-------------|-----------------|
| AdaEvolve | ~$3 | ~$1.5 | ~$0.15 |
| EvoX | ~$3 | ~$1.5 | ~$0.15 |
| GEPA | ~$3 | ~$1.5 | ~$0.15 |
| OpenEvolve | ~$3 | ~$1.5 | ~$0.15 |
| K-Search | ~$3 | ~$1.5 | ~$0.15 |
| Nous | ~$16 | (running) | ~$8/campaign |

Total H1+H2 estimated: ~$45-55 in LLM API costs.

## File Inventory

| File | Purpose |
|------|---------|
| `evaluator/evaluate.py` | Core evaluator — patch, build, bench, score, restore |
| `evaluator/evaluate.sh` | Shell wrapper for standalone testing |
| `evaluator/verify_integrity.py` | Data integrity verification |
| `run_bakeoff.py` | Orchestrator — runs all frameworks sequentially |
| `run_bakeoff_h2.sh` | H2 launch script (sets BAKEOFF_EVAL_MODE=mixed) |
| `run_bakeoff_h3.sh` | H3 launch script (sets BAKEOFF_EVAL_MODE=concurrent) |
| `initial_program.rs` | Annotated pipeline.rs for H1/H2 (single file) |
| `initial_program_h3.rs` | Concatenated service.rs + lib.rs + pipeline.rs for H3 |
| `initial_program_h3/` | Directory with actual files (for Nous multi-file mode) |
| `restructure_results.sh` | Post-run: organize results into per-hypothesis folders |
| `frameworks/*/config.yaml` | H1 configs per framework |
| `frameworks/*/config-mixed.yaml` | H2 configs per framework |
| `frameworks/*/config-concurrent.yaml` | H3 configs per framework |
| `frameworks/h3-system-prompt.md` | Shared domain context for H3 |
| `results/h1_analysis.md` | Comprehensive H1 analysis |
| `results/hypothesis_1/` | H1 results (to be populated by restructure script) |
| `results/hypothesis_2/` | H2 results (to be populated by restructure script) |
| `results/hypothesis_3/` | H3 results (populated when H3 runs) |
