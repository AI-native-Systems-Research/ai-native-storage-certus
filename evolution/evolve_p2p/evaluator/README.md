# Evaluator Framework

Scores candidate code changes for the SSD-to-GPU cold lookup data path. Used by all evolution frameworks as a universal fitness function.

## How It Works

```
candidate source → patch files → cargo build → start server → benchmark → score → restore
```

### Flow

1. **Backup** all target source files (`.bak` copies)
2. **Patch** target files with candidate content
3. **Build** `cargo build -p certus-server --release --features p2p`
4. **Start** the certus-server with NVMe device
5. **Benchmark** via `certus-api-bench.py` (16 × 4MiB cold objects, 10 iterations)
6. **Score** using fitness formula
7. **Restore** original source files from backup (always, even on crash)

### Scoring Formula

```
score = 0.60 * min(1.0, throughput_gbps / 12.0) + 0.40 * min(1.0, 0.4 / p99_latency_ms)
```

Hard constraints:
- Build must succeed → score 0.0 on failure
- Data integrity must pass → score -1.0 on corruption

### Target Files

| File | Path | What it controls |
|------|------|-----------------|
| pipeline.rs | components/dispatcher/src/pipeline.rs | Transfer loop (NVMe read → GPU copy) |
| lib.rs | components/dispatcher/src/lib.rs | Queue allocation, pipeline wiring, batch_lookup |
| dma.rs | components/gpu-services/src/dma.rs | DMA buffer creation (host-pinned and GPU BAR) |
| service.rs | apps/certus-server/src/service.rs | gRPC service (rarely modified) |

### Usage

```bash
# Score current source files (wild-type test)
python3 evaluate_p2p.py --test

# Score a candidate (SkyDiscover mode — file path input)
python3 evaluate_p2p.py --program_path /path/to/candidate.rs --results_dir /path/to/output

# Called programmatically by GEPA
from evaluate_p2p import evaluate
score, metrics = evaluate({"pipeline.rs": code1, "dma.rs": code2})
```

### CERTUS_REPO_ROOT

The evaluator resolves the repository root via:
1. `CERTUS_REPO_ROOT` env var (if set)
2. Walk up from evaluator file until `Cargo.toml` is found

This allows the evaluator to work in git worktrees or alternative paths.

---

## Concatenated Seed (for Evolutionary Frameworks)

SkyDiscover-based frameworks (AdaEvolve, EvoX, OpenEvolve, ShinkaEvolve) operate on a **single concatenated file** containing all target files separated by markers.

### Format

```rust
// --- FILE: pipeline.rs ---
<full pipeline.rs content>

// --- FILE: dma.rs (buffer creation functions) ---
<full dma.rs content>

// --- CONTEXT (read-only, do not include in output): lib.rs relevant sections ---
// This shows how pipeline.rs and dma.rs are called from the dispatcher.
<lib.rs extract — 553 lines of relevant sections>
```

### Why This Format

- Evo frameworks rewrite the entire file each iteration
- They need to see all files to understand cross-file relationships
- lib.rs is READ-ONLY context (too large for rewrite at 3244 lines)
- Output must contain only `pipeline.rs` and `dma.rs` sections

### How It's Split Back

The evaluator's `split_concatenated()` function parses `// --- FILE: xxx ---` markers and extracts each file:

```python
def split_concatenated(text: str) -> dict[str, str]:
    marker_re = re.compile(r'^//\s*---\s*FILE:\s*(\S+?)(?:\s*\(.*?\))?\s*---\s*$', re.MULTILINE)
    # ... splits into {"pipeline.rs": content, "dma.rs": content}
```

### GEPA (Multi-File Native)

GEPA doesn't use the concatenated format. It receives a `dict[str, str]`:

```python
seed = {"pipeline.rs": pipeline_code, "dma.rs": dma_code}
# Returns: (score, metrics_dict)
```

GEPA can only handle ~800 lines per file reliably. Beyond that, the LLM outputs prose instead of code. lib.rs (3244 lines) is excluded from GEPA's seed.

---

## Post-Run Analysis

After each framework completes, `_analyze_run()` generates `analysis.json`:

```json
{
  "framework": "gepa_native",
  "total_evals": 10,
  "best_score": 0.2072,
  "p2p_attempted": true,
  "p2p_compiled": false,
  "stagnation_ceiling": "baseline",
  "primary_failure_mode": "coordination_failure",
  "build_errors": ["Cannot find: create_gpu_dma_buffer"],
  "diagnosis": "Hints did not fix coordination failure..."
}
```

### Fields

| Field | Description |
|-------|-------------|
| `p2p_attempted` | Did the framework generate code referencing P2P functions? |
| `p2p_compiled` | Did P2P code compile AND beat baseline? |
| `stagnation_ceiling` | Where it plateaued: `baseline` / `mid_range` / `hardware_limit` |
| `primary_failure_mode` | `build_failures` / `coordination_failure` / `local_optimum` / `near_ceiling` |
| `diagnosis` | One-sentence explanation of why it stagnated |

### Score Extraction

Scores are parsed from multiple sources (in priority order):

1. `scores.jsonl` — standard format written by framework extractors
2. SkyDiscover output JSONL files in `output/` directory
3. Run log — regex for `combined_score=X.XXXX` (evaluator output format)

If scores are found but `scores.jsonl` doesn't exist, it's auto-created.

---

## Dashboard

Streamlit app (`dashboard.py`) reads from `results/` or `results_hint/` directories.

### Tabs

1. **Overview** — summary table, top-line metrics, framework comparison
2. **Trajectories** — score vs evaluation number, best-so-far curves, stagnation analysis
3. **Pareto** — throughput vs p99 latency scatter plot
4. **Candidate Details** — per-evaluation source diff from wild-type
5. **Failure Analysis** — build error breakdown, diagnosis from analysis.json
6. **Architecture** — P2P discovery/attempt/compile matrix, key differentiator, detailed strategy

### Sidebar Toggle

Switches between two result sets:
- **Optimize data path (no hints)** → reads `results/`
- **Implement P2P (with hints)** → reads `results_hint/`

### Running

```bash
streamlit run dashboard.py --server.port 8501
```

---

## Feature Gate: `--features p2p`

Building with `--features p2p` enables P2P/GPUDirect code throughout the crate chain:

```
certus-server/p2p → dispatcher/p2p → gpu-services/p2p
```

This makes `gpu_services::dma::create_spdk_dma_buffer_from_gpu_bar` visible to `pipeline.rs` in the dispatcher crate. Without this feature propagation, P2P functions are invisible to the transfer loop.

---

## File Layout

```
evaluator/
  evaluate_p2p.py        — main evaluator (build, benchmark, score)
  path_verifier.py       — validates file paths
  README.md              — this file

initial_programs/
  pipeline.rs            — wild-type transfer loop (413 lines)
  dma.rs                 — wild-type DMA buffer functions (742 lines)
  lib.rs                 — wild-type dispatcher (3244 lines, full)
  lib_extract.rs         — relevant sections only (553 lines, for context)
  concatenated_seed.rs.skydiscover — pipeline + dma + lib_extract with markers

results/                 — no-hint experiment results
results_hint/            — P2P-directed experiment results

configs/                 — no-hint framework configs
configs_hint/            — P2P-directed framework configs
  HINTS.md              — shared hint text (direction + compile notes)
  gepa_background.txt   — GEPA-specific context
  ksearch_definition.txt — K-Search task definition
  skydiscover/config.yaml — SkyDiscover system prompt + settings
  nous/campaign.yaml    — Nous campaign config
  autoscientists/TASK.md — AutoScientists task description
```
