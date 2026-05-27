# Evolutionary & Discovery Frameworks for Certus Phase 2

Background research on AI-driven optimization frameworks evaluated for evolving Certus storage system components — from single-file intra-component targets (eviction policy, allocator, transfer pipeline) to inter-component behaviors (eviction + flush coordination) to meta-level control systems (workload classification, evaluator tuning).

---

## 1. How LLM Code Evolution Works

All tools in this document solve the same problem: given source code + an evaluator that scores it, use an LLM to produce better code. They differ in how they structure the search (what feedback they extract from the evaluator, how they preserve progress, whether they can pursue multi-step transformations where performance gets worse before it gets better) but the underlying optimization problem is identical: maximize evaluator score.

The frameworks differ along two independent dimensions:

**Axis A: Information channel** (what feedback does the optimizer receive from each evaluation?)

| Level | What flows back | Tools |
|---|---|---|
| Scalar | Just a number (score, pass/fail) | ShinkaEvolve, AdaEvolve (baseline) |
| Scalar + periodic diagnosis | Scores normally; LLM analyzes failure patterns on stagnation | AdaEvolve (Meta-Guidance) |
| Scalar + population-level meta-signals | Scores from solution window feed back into search strategy scoring; population descriptor (diversity, frontier shape, stagnation depth) conditions strategy generation | EvoX |
| Eval results → world model update | Full eval results (status, metrics, logs) update the persistent world model; the world model (not raw traces) guides next action selection | K-Search |
| Full traces per candidate | LLM reads execution output, errors, profiler data for every eval | GEPA |
| Full traces + controlled comparison | Full traces plus explicit experimental arms isolating causal effects | Nous |

**Axis B: Search-state topology** (how does the framework organize and preserve progress?)

| Topology | What it maintains | Tools |
|---|---|---|
| Parallel population | Many candidate programs evolving independently | ShinkaEvolve, AdaEvolve |
| Parallel population + co-evolving strategy population | Solution population + a separate search-strategy population; strategies are scored, selected, and mutated using the same evolutionary machinery as solutions | EvoX |
| Pareto frontier | Diverse non-dominated candidates (per-instance or multi-objective) | GEPA, AdaEvolve (Pareto mode) |
| Action tree / world model | High-level strategies (plans) rated and preserved independently of their code implementations — a bad implementation doesn't kill a good plan | K-Search |
| Experiment sequence | Hypotheses, arms, and extracted principles | Nous |

### Framework Map: Information Channel × Search-State Topology

| Search-state topology | Scalar score | Scalar + diagnosis | Population meta-signals | Structured eval/log feedback | Full traces | Controlled experiments |
|---|---|---|---|---|---|---|
| **Parallel population** | ShinkaEvolve | AdaEvolve | — | — | — | — |
| **Co-evolving populations** | — | — | EvoX | — | — | — |
| **Pareto frontier** | AdaEvolve Pareto | — | — | — | GEPA | — |
| **Action tree / world model** | — | — | — | K-Search | — | — |
| **Experiment sequence** | — | — | — | — | — | Nous |

Tools are placed by their dominant operating mode. AdaEvolve and ShinkaEvolve primarily optimize populations using scalar feedback; EvoX uses scalar feedback for solutions but population-level meta-signals (improvement rate, diversity, stagnation depth) to evolve the search strategy itself; GEPA maintains Pareto diversity but uses richer per-candidate trace feedback; K-Search preserves progress in an explicit action tree/world model; Nous uses full benchmark evidence inside controlled experimental arms to produce causal or regime-level conclusions.

**What this means for optimization paths:** Population tools (ShinkaEvolve, AdaEvolve) discard non-improving mutations — they cannot pursue multi-step changes where performance dips before improving. K-Search decouples planning from instantiation: its world model preserves promising strategies even when their current implementation is defective, enabling multi-step structural transformations that population tools abandon. The world model co-evolves with the code — each eval result refines ratings, adjusts confidence, and triggers tree edits (insert new approaches, prune dead ends), so round 50's plans are grounded in accumulated experimental evidence, not just LLM priors. GEPA's acceptance gating similarly filters non-improving candidates but compensates via reflective mutation (diagnoses WHY and proposes targeted fixes). Nous doesn't search a code space at all — it runs controlled experiments to establish causal facts.

| Tool | What the LLM reasons about | How structured |
|---|---|---|
| **ShinkaEvolve** | "How to improve this code" (per-mutation) + "What's the population trend" (meta-recommendations) | Loosely — suggestions, not enforced |
| **AdaEvolve** | Same + "Why is progress stalling, what algorithmic shift is needed" (Meta-Guidance Level 3) | Moderately — tactics are mandatory but generated ad-hoc |
| **EvoX** | Two levels: (1) inner LLM generates solution candidates given parent+context; (2) outer LLM reasons about "What search strategy would work better given this population state — which parents to select, what variation operator to apply, how to balance explore vs exploit" | Highly — search strategy is executable Python code (EvolvedProgramDatabase class) validated before deployment; scored by improvement-over-window formula |
| **GEPA** | "WHY did this candidate fail on these specific instances" (full execution trace reflection) + accumulated lessons from ancestry | Structured — reflections are per-rollout, lessons compound through ancestry chain, Pareto-based selection ensures diversity |
| **K-Search** | "What are the bottlenecks, which approach should I try next, why did this fail" (persistent world model) | Highly — decision tree with ratings, confidence, explicit backtracking |
| **Nous** | "Is this hypothesis true, what's the causal mechanism, at what regime does it break" (controlled experiments) | Most structured — formal hypothesis arms, control-negatives, principles extraction |

**How to pick:**

| You need... | Pick... | Why |
|---|---|---|
| Maximum variants fast (single file) | ShinkaEvolve | Async pipeline, UCB model selection, fastest throughput |
| Broad adaptive search, no tuning (single file) | AdaEvolve | 3-level hierarchy adapts automatically, Pareto mode |
| Self-adapting search that breaks through plateaus without manual intervention | EvoX | Co-evolves search strategy alongside solutions; outperforms all fixed-strategy methods on 96% of 196 tasks; especially strong when optimal explore/exploit balance shifts mid-run |
| Diagnostic-guided Pareto search, few evaluations (single or multi-param) | GEPA | Reflection reads full traces, 35× fewer rollouts than RL, Pareto front avoids local optima, native multi-param support |
| Deep optimization requiring multi-step restructuring (single file or CUDA multi-file) | K-Search | World model preserves strategy across bad implementations, structured backtracking, 14.3× on complex problems |
| Multi-file edits, causal evidence, regime boundaries | Nous | Control-negative arms, principles extraction, full repo git worktree |

### Capability Comparison

| Capability | SkyDiscover (AdaEvolve) | SkyDiscover (EvoX) | GEPA | K-Search | ShinkaEvolve | Nous | What it means |
|---|---|---|---|---|---|---|---|
| **Causal reasoning** | No | No | Partial / diagnostic (reflection diagnoses WHY per-instance) | Partial / inferential (world model forms causal hypotheses from eval evidence, but doesn't prove them with controlled arms) | No | **Yes** / controlled (control-negative arms isolate causal effect experimentally) | Can explain WHY a change helped — from informal inference to controlled isolation |
| **Compounding knowledge** | Partial (Meta-Guidance tactics) | **Yes** (search strategy database H accumulates scored strategies + population states; new strategies are conditioned on prior strategy performance under similar population descriptors) | **Yes** (ancestry chain — lessons from all ancestors injected into mutation) | **Yes** (persistent world model + decision tree) | Partial (meta-recommendations) | **Yes** (principles.json) | Lessons from iteration N are injected into N+1's prompt — iteration 50 builds on everything before it |
| **Adaptive search** | **Yes** (3-level hierarchy: local intensity, global UCB, paradigm breakthrough) | **Most adaptive** (the search strategy itself is LLM-generated code that gets replaced when stagnation detected — can switch from random sampling to greedy to multi-objective to UCB+structural-variation within one run) | Partial (Pareto-based candidate selection provides diversity; no explicit adaptive strategy mechanism — no temperature scheduling, no mutation rate adaptation, no bandit-based parameter selection) | **Yes** (stagnation-aware backtracking, action re-ranking per round, difficulty relaxation) | **Yes** (UCB bandit model selection) | No (fixed hypothesis structure) | Adjusts exploration vs exploitation based on progress |
| **Backtracking** | No (population diversity substitutes) | Partial (restores fallback database if new strategy causes runtime errors; solution population is never reset) | No (Pareto diversity substitutes) | **Yes** (rates branches, backtracks on stagnation, prunes dead ends) | No | No | Can abandon a bad direction and try a different branch without losing history |
| **Pareto optimization** | **Yes** (multi-objective Pareto front) | No (single `combined_score`; evolved strategies could implement multi-objective selection internally) | **Yes** (instance-level Pareto front — best candidate PER task) | No (single objective with prediction) | No (single `combined_score`) | No | Optimizes multiple objectives simultaneously without collapsing to one weighted score |
| **Reflection on traces** | Partial (error messages + tracebacks injected into retry prompts; evaluator `artifacts["feedback"]` passed as context — but no dedicated reflection step) | Partial (inner solution loop retries include error messages + tracebacks in prompt context; outer search-strategy loop does not reflect on errors) | **Yes** (LLM reads full execution traces, error messages, profiler output via ASI system — core innovation) | Partial (eval results inform world model; structured reflection on PASSED rounds via CURRENT/FOLLOW_THROUGH/UPDATE_BELIEF/PERF_GAP blocks; failures get coarser treatment) | Partial (error messages fed back in patch-retry loops; fix-mode passes stdout/stderr to LLM; `text_feedback` option available but off by default) | **Yes** (full bench output in analysis phase; discrepancy analysis required) | Evaluator output is read in natural language, not just as a scalar |
| **Cross-program merge** | No | Possible (evolved strategy can implement merge in its `sample()` method) | **Yes** (system-aware merge — combines strengths of two Pareto-optimal candidates) | No | **Yes** (10% crossover patches) | No | Combines code from two candidates to create a child — merges good ideas from independent lineages |
| **EVOLVE-BLOCK markers** | Prompt guidance (core engines evolve whole file) | Same as AdaEvolve (solution-level) + search strategy is entire class file (EVOLVE-BLOCK wraps `EvolvedProgramDatabase`) | **Enforced** (optimize_anything scopes mutation to candidate text) | N/A (task backends provide code format) | **Enforced** (`apply_full.py` preserves immutable regions) | N/A (full repo access) | Tags marking which region the LLM can change |
| **Multi-file edits** | No (default) / Yes (claude_code mode, 10-50x slower) | No (solutions are single-file; search strategies are single-class) | **Yes** (native `dict[str, str]` multi-param candidates — each key is a named text parameter the LLM can mutate independently; not full repo access but multiple text artifacts per iteration) | Partial (CUDA targets: generates kernel.h + kernel.cu + main.cpp as 3 coordinated files; Triton/Python/MLX: single file only) | No | **Yes** (git worktree, full repo access) | Can change multiple files in one iteration |
| **Async pipeline** | No (sequential) | No (sequential) | No (sequential) | No (sequential debug-and-improve cycles) | **Yes** (proposal + eval in parallel pools) | No | Proposals and evaluations overlap — 5-10x throughput when eval is slow |
| **Dynamic island management** | **Yes** (auto-spawns new islands on global stagnation) | No (single population, but the entire search algorithm can be swapped — equivalent to replacing all island logic with a new paradigm) | N/A (Pareto diversity substitutes for islands) | N/A (single candidate) | Optional (dynamic spawning on stagnation exists via `enable_dynamic_islands`, disabled by default) | N/A | Creates new subpopulations when all existing ones stagnate |
| **Self-evolving search** | No (fixed algorithm with adaptive parameters) | **Yes** (search strategy is an evolvable Python program; co-evolved alongside solutions; scored by improvement × log-weight / √horizon) | No | No | No | No | The search algorithm itself is treated as a candidate to be evolved |
| **Sample efficiency** | Low (100-200+ evals typical) | Medium (100 evals typical; meta-evolution overhead is ~10% extra LLM calls for strategy generation + validation; beats fixed strategies on 96% of 196 benchmarks within same eval budget) | **Highest** (35× fewer rollouts than RL; 100-500 evals typical) | High (120 evals in paper; world model avoids wasting evals on dead-end approaches) | Low (volume-based) | High (few iterations but expensive per iteration) | How many evaluator calls needed to reach good performance |

---

## 2. What Each Tool Requires and Produces

All tools below accept the same fundamental inputs: source code to optimize + an evaluator that returns a score. They differ in API shape, search strategy, and what additional signals they extract from the evaluator. Any tool can be pointed at any Certus target — the question is which one reaches good performance in fewer evaluator calls.

### SkyDiscover (Platform) with AdaEvolve (Default Algorithm)

SkyDiscover is a **platform** that hosts multiple search algorithms. AdaEvolve is the default and best-performing algorithm within it.

| | |
|---|---|
| **You provide** | One source file (or multi-file workspace in claude_code mode), a Python evaluator script (returns score dict), initial program. No hyperparameter tuning needed — same settings work across all problem types |
| **It produces** | Ranked population of solutions; optionally a Pareto front. All solutions stored in archive |
| **Codebase requirement** | Default: evolvable logic in a single self-contained file with stable function signatures. claude_code mode: full workspace (10-50x slower) |
| **Iteration speed** | 1 LLM call + 1 evaluator run per iteration. With cargo bench: ~30-60s |
| **Key capabilities** | (1) Three-level adaptive hierarchy eliminates tuning. (2) Multi-objective Pareto. (3) Dynamic island spawning on stagnation. (4) Meta-Guidance generates algorithmic tactics when refinement saturates |
| **Track record** | Outperforms OpenEvolve, ShinkaEvolve, GEPA across 185 problems. ADRS systems benchmarks (transaction scheduling, load balancing, TCP congestion) directly validate for storage targets |

**Architecture:**

```
MainController → AdaEvolveManager → {AdaEvolveDatabase, LLM, Evaluator, ContextBuilder}

AdaEvolveDatabase (sole owner of global search state):
  - Islands (logical indices, each with its own archive)
  - Per-island adaptive state (G(k), R(k), V(k), n(k))
  - Global UCB adapter (selects next island)
  - Migration controller (ring topology, every M iterations)
  - Island spawner (triggers on global stagnation)
  - Tactics tracker (monitors stagnation, injects tactics)
```

**Key insight:** AdaEvolve's 3-level hierarchy (local exploration control → global island allocation → Meta-Guidance tactics) uses identical hyperparameters across all 185 tested problems — no per-campaign tuning. When all islands stagnate, Meta-Guidance generates mandatory algorithmic tactics (not "explore more" — specific implementable strategies like "switch to dynamic programming").

**Algorithms within SkyDiscover:** AdaEvolve (default), Pareto mode (multi-objective), EvoX mode (co-evolves the search itself), claude_code mode (multi-file).

### SkyDiscover (EvoX Mode) — Meta-Evolution of Search Strategies

Paper: [EvoX: Meta-Evolution for Automated Discovery](https://arxiv.org/abs/2602.23413) (UC Berkeley / Bespoke Labs, 2026)

EvoX treats the **search strategy itself as an evolvable program**, jointly optimizing candidate solutions and the algorithms used to generate them. Rather than hand-tuning explore/exploit ratios or selection heuristics, EvoX starts from a trivial random-sampling strategy and evolves it based on observed progress.

| | |
|---|---|
| **You provide** | Same as AdaEvolve: one source file, a Python evaluator script (returns score dict), initial program. No strategy tuning needed — starts from random sampling and evolves its own search |
| **It produces** | Best solution found (single `combined_score`). Additionally: a search strategy database H of scored strategies with population state descriptors |
| **Codebase requirement** | Same as AdaEvolve default: evolvable logic in a single self-contained file with stable function signatures |
| **Iteration speed** | 1 LLM call + 1 evaluator run per solution iteration (same as AdaEvolve). Strategy evolution triggered on stagnation adds 1 extra LLM call per window (~10% of iterations) + validation |
| **Key capabilities** | (1) Self-evolving search: adapts parent selection, variation operators, and explore/exploit balance by generating new `EvolvedProgramDatabase` classes as Python code. (2) Problem-specific variation operators: auto-generates diverge/refine labels via LLM analysis of problem + evaluator. (3) Demand-driven strategy switching: only evolves search when stagnation detected (not periodic). (4) Fallback safety: validates new strategies before deployment; restores previous strategy + preserves new solutions on failure |
| **Track record** | Outperforms AdaEvolve, GEPA, ShinkaEvolve, OpenEvolve on 96% of 196 benchmarks (math, systems, algorithms). 34.1% higher final score on signal processing. Matches/exceeds AlphaEvolve on 5/7 math tasks. Systems benchmarks directly relevant to storage: transaction scheduling (4347.8 vs human 2724.8), cloud transfer cost optimization |

**Architecture:**

```
CoEvolutionController (orchestrates two-level evolution)
  ├── Solution Evolution (inner loop)
  │   ├── EvolvedProgramDatabase (hot-swappable search strategy — Python class)
  │   │   ├── add(program) — controls how programs enter population
  │   │   └── sample(num_context_programs) → (parent_dict, context_programs_dict)
  │   │       ├── Parent selection: what to mutate next
  │   │       ├── Context selection: what else the LLM sees as inspiration
  │   │       └── Label selection: "" (default) / DIVERGE_LABEL / REFINE_LABEL
  │   ├── EvoxContextBuilder (parallel LLM calls for stats insight + problem summary + batch summaries)
  │   ├── Variation Operators (auto-generated per-problem)
  │   │   ├── Diverge operator: "try fundamentally different approaches" (structural variation)
  │   │   └── Refine operator: "intensify within current approach" (local refinement)
  │   └── Evaluator (cargo bench / custom scorer)
  │
  ├── Meta-Evolution (outer loop — triggered on stagnation)
  │   ├── LogWindowScorer: score = improvement × (1 + log(1 + start_score)) / √horizon
  │   ├── SearchStrategyDatabase H: stores (strategy_code, population_descriptor ϕ, score J)
  │   ├── Strategy Generator (LLM): produces new EvolvedProgramDatabase class
  │   │   ├── Conditioned on: H (prior strategies + scores), ϕ(Dt) (current population state)
  │   │   └── Mutations modify: parent selection rules, inspiration set construction, variation operator preferences
  │   ├── SearchStrategyEvaluator: validates structure, inheritance, add/sample signatures, metric preservation
  │   └── Hot-swap: migrate all programs to new database, preserve fallback for rollback
  │
  └── Stagnation Detection
      ├── switch_interval = 10% of total iterations (default)
      ├── improvement_threshold = 0.01 (absolute or 1% relative)
      └── Triggers strategy evolution only when consecutive iterations show no meaningful improvement
```

**Key insight — two-level co-evolution:** The critical difference from AdaEvolve's 3-level hierarchy is that EvoX's adaptation mechanism is itself an LLM-generated program, not a fixed algorithm with tunable parameters. When AdaEvolve stagnates, Meta-Guidance generates a new *tactic* (a text instruction like "try dynamic programming"). When EvoX stagnates, it generates a new *search algorithm* (executable Python implementing different parent selection, context construction, and variation operator preferences). This is a qualitative difference: AdaEvolve adapts within a fixed algorithmic framework; EvoX can abandon the framework entirely.

**Algorithm (from paper):**

```
Input: Budget T, window W (10% of T), stagnation threshold τ, evaluator E
Initialize: solution database D₀, strategy S₀ (random sampling), strategy history H = ∅

while t < T:
    Phase I — Solution evolution under Sₜ (one window):
        Record ϕₜ = population_descriptor(Dₜ), s_start = best_score(Dₜ)
        for i = 1 to W:
            (x_par, π, I) ~ C_Sₜ(Dₜ)          # Strategy selects parent, variation op, inspiration
            x' ~ G_sol(· | x_par, π, I)         # LLM generates candidate
            (s', a') = E(x')                     # Evaluator scores it
            Dₜ₊₁ = Dₜ ∪ {(x', s', a')}; t += 1

    Phase II — Progress monitoring:
        s_end = best_score(Dₜ), Δ = s_end - s_start
        Jₜ = Δ × log(1 + s_start) / √W          # Score the strategy
        H = H ∪ {(Sₜ, ϕₜ, Jₜ)}

    Phase III — Strategy evolution (on stagnation):
        if Δ < τ:
            S' ~ G_str(· | H, ϕ(Dₜ))            # LLM generates new strategy
            if VALID(S'):                          # Structural + functional checks
                Sₜ = S'                           # Hot-swap (programs preserved)

return argmax_{(x,s,a) ∈ D_T} s
```

**Variation operators (problem-specific, auto-generated):**

At campaign start, EvoX uses a guide LLM to analyze the problem description + evaluator code and generates two variation operator prompts:

1. **Diverge operator** — structural/algorithmic diversity: "Try fundamentally different approaches." Categorizes alternative techniques (libraries, algorithm families, construction methods, constraint handling). Used when progress stalls and the population lacks diversity.

2. **Refine operator** — local intensification: "Squeeze more from the current approach." Suggests hyperparameter tuning, initialization quality, post-processing, solver tolerances. Used when a promising candidate needs polishing.

The search strategy's `sample()` method decides which operator to apply via `parent_info` labels: empty string (default free-form), `DIVERGE_LABEL`, or `REFINE_LABEL`.

**Search strategy scoring formula:**

```
J(S | D) = (s_end - s_start) × log(1 + s_start) / √W
```

- `log(1 + s_start)` upweights strategies that improve already-strong solutions (harder gains are worth more)
- `√W` normalizes for window length
- Strategies scoring higher in H are preferentially selected as parents for the next strategy generation

**What evolved strategies look like (from signal processing case study):**

| Phase | Iterations | Strategy evolved | Effect |
|---|---|---|---|
| 1 | 0–20 | Random sampling (seed) | Modest early improvement (0.499 → 0.530), then stagnation |
| 2 | 20–40 | Greedy (refine single best) | No breakthrough — underlying structure too limited |
| 3 | 40–60 | Stratified multi-objective sampling | Largest jump (+0.119): blends complementary strengths from different score tiers |
| 4 | 60–90 | UCB + structural variation | Explores bold structural changes (+0.056): discovers advanced SciPy filtering pipelines |
| 5 | 90–100 | UCB + local refinement | Final polishing (+0.022): precise adjustments lock in the high score |

**When to use EvoX vs AdaEvolve:**

| Condition | EvoX wins | AdaEvolve wins |
|---|---|---|
| Optimal strategy shifts mid-run | **Yes** — adapts by generating entirely new search algorithms | No — 3-level hierarchy adapts parameters but not the algorithm shape |
| Unknown which selection/variation works best for this target | **Yes** — discovers it empirically | Needs correct initial configuration of island count, exploration params |
| Need multi-objective Pareto front | No (single `combined_score`) | **Yes** (native Pareto mode) |
| Budget > 50 iterations on single target | **Yes** — meta-evolution has enough window to evolve strategies | Both work; AdaEvolve's island spawning provides similar adaptation |
| Budget < 30 iterations | AdaEvolve — not enough iterations for EvoX to detect stagnation and evolve | **Yes** — 3-level hierarchy adapts faster from small signals |
| Complex non-convex landscape (solution quality jumps require fundamentally different approach families) | **Yes** — can switch from greedy to multi-objective to UCB exploration | Partial — Meta-Guidance generates tactics but cannot restructure selection logic |

**Usage:**

```bash
uv run skydiscover-run initial_program.py evaluator.py \
  --config config.yaml --search evox --iterations 100
```

```yaml
# config.yaml for EvoX mode
search:
  type: "evox"
  database:
    auto_generate_variation_operators: true  # LLM-generates diverge/refine operators
```

### GEPA (Reflective Pareto Evolution)

| | |
|---|---|
| **You provide** | A seed candidate (code, prompt, or config), an evaluator returning score + optional diagnostic text (Actionable Side Information). Optionally: dataset of task instances, validation set |
| **It produces** | Optimized candidate(s) from Pareto frontier. Ancestry tree. Per-instance best candidates |
| **Codebase requirement** | Single text artifact (`str`) or multi-param (`dict[str, str]`) per candidate. Evaluator callable from Python. Supports `optimize_anything` API or adapter-based (DSPy, RAG, MCP) |
| **Iteration speed** | 1 LLM call + 1 minibatch eval + optional merge. ~30-60s with cargo bench. 100-500 total evaluations typical |
| **Key capabilities** | (1) Reflective reasoning on full execution traces. (2) 35× more sample-efficient than RL. (3) Instance-level Pareto avoids local optima. (4) System-aware merge combines complementary candidates |
| **Track record** | 6% avg over GRPO with 35× fewer rollouts. 30.52% NPU utilization (vs 4.25% baseline). 50+ production deployments (Shopify, Databricks, OpenAI) |

**Architecture:**

```
GEPAEngine (orchestrates optimization loop)
  ├── ReflectiveMutationProposer
  │   ├── Execute candidate on minibatch → capture traces
  │   ├── Extract ASI (Actionable Side Information) via feedback function μf
  │   └── Reflect: LLM reads traces + ancestry lessons → proposes targeted fix
  ├── MergeProposer (system-aware crossover)
  │   ├── Select two Pareto-optimal candidates excelling on different tasks
  │   └── LLM identifies which module improved in each → takes best of both
  ├── GEPAState (Pareto pool + scores matrix + ancestry)
  │   ├── Candidates: list of (program, parent_idx) pairs
  │   ├── Scores matrix: S[candidate][task_instance] → score
  │   ├── Pareto frontier: non-dominated set based on instance-level scores
  │   └── EvaluationCache: avoids redundant evaluations
  └── AcceptanceCriterion (StrictImprovement or ImprovementOrEqual)
```

**Key insight — Actionable Side Information (ASI):** Traditional optimizers know THAT a candidate failed but not WHY. GEPA's evaluator returns textual diagnostics (compiler errors, profiler output, Criterion JSON) alongside the score. The reflection LLM reads these traces to diagnose failures and propose targeted fixes — the text-optimization analogue of a gradient. For Certus: `cargo bench` timing, `cargo test` assertions, and `cargo build` errors all become ASI.

**Three optimization modes:**

| Mode | When to use | Certus example |
|---|---|---|
| **Single-Task** | One artifact, one benchmark | Optimize `lru.rs` against `cargo bench -p memory-tier` |
| **Multi-Task** | Multiple workloads, cross-task transfer | Optimize eviction across 5 access traces |
| **Generalization** | Transfer to unseen workloads | Train on 10 traces, validate on 5 held-out |

### K-Search (Tree-Structured World Model Search)

| | |
|---|---|
| **You provide** | A task backend: `get_definition_text()`, `run_benchmark()` → EvalResult, `code_for_world_model_from_raw()`. Initial program optional |
| **It produces** | Best optimized solution. Persistent world model JSON (decision tree with hypotheses, ratings, confidence). Solution database |
| **Codebase requirement** | Task backend wraps evaluation; target code flows through world model's action→codegen→eval loop |
| **Iteration speed** | 1 eval per round; action cycles are ~5-7 rounds (stagnation window). Multiple LLM calls per round (codegen + debug-and-improve). With cargo bench: ~2-5min per action cycle |
| **Key capabilities** | (1) Structured backtracking — abandons bad branches. (2) Persistent reasoning about WHY approaches fail. (3) Deep single-candidate optimization with debug-and-improve. (4) Stagnation-aware action selection |
| **Track record** | 2.10× avg over OpenEvolve on FlashInfer-Bench. 14.3× on complex MoE kernels. SOTA on GPUMode TriMul |

**Architecture:**

```
WorldModelKernelGeneratorWithBaseline
  ├── WorldModelManager
  │   ├── World model JSON (persistent decision tree)
  │   ├── ensure_initialized() → LLM creates initial tree with hypotheses
  │   ├── propose_action_nodes() → LLM proposes + ranks actions via decision tree edits
  │   ├── choose_next_action_node_id() → selects highest-rated open node (respects max_difficulty)
  │   ├── refine() → LLM updates tree based on eval evidence
  │   └── note_action_too_hard() → marks failed actions, downgrades ratings
  ├── SolutionDB (JSONL persistence of all attempts + eval results)
  ├── Task backend (evaluator)
  └── LLM client (OpenAI-compatible API)
```

**Key insight — persistent world model:** K-Search maintains a decision tree where each node represents a tried approach with ratings (0-10), confidence (0-1), and predicted impacts. When an approach fails, it backtracks to a different branch rather than wasting iterations on a dead end. The tree accumulates structured knowledge about what works and why — unlike population tools that encode knowledge implicitly in the gene pool.

**Action cycle state machine:**

```
                    ┌─────────────────────────────────────────┐
                    │ CYCLE START: Choose open action node    │
                    │ (highest rated, respects max_difficulty)│
                    └───────────────┬─────────────────────────┘
                                    │
                    ┌───────────────▼─────────────────────────┐
                    │ ATTEMPT 1: Generate from spec+action    │
                    │ (or base_code+action if parent has sol) │
                    └───────────────┬─────────────────────────┘
                                    │
                    ┌───────────────▼─────────────────────────┐
                    │ EVALUATE: Run benchmark → EvalResult    │
                    └───────────────┬─────────────────────────┘
                                    │
                         ┌──────────┴──────────┐
                         │                     │
                    PASSED                 FAILED
                         │                     │
                ┌────────▼────────┐    ┌───────▼──────────┐
                │ Update best     │    │ DEBUG-AND-IMPROVE │
                │ Reset streak    │    │ (up to N rounds)  │
                └────────┬────────┘    └───────┬──────────┘
                         │                     │
                         ├─────────────────────┤
                         │                     │
              stagnation < window    stagnation >= window
                         │                     │
                ┌────────▼────────┐    ┌───────▼──────────┐
                │ CONTINUE:       │    │ CYCLE END:        │
                │ next attempt    │    │ Attach best OR    │
                │ same action     │    │ mark "too hard"   │
                └─────────────────┘    └───────┬──────────┘
                                               │
                                    ┌──────────▼──────────┐
                                    │ REFINE: Update tree  │
                                    │ ratings, insert      │
                                    │ child actions, prune │
                                    └──────────┬──────────┘
                                               │
                                    ┌──────────▼──────────┐
                                    │ NEXT CYCLE           │
                                    │ (choose new action)  │
                                    └─────────────────────┘
```

### ShinkaEvolve (Population-Based, Async, High-Throughput)

| | |
|---|---|
| **You provide** | One source file with EVOLVE-BLOCK markers + evaluator. Or run `shinka-convert` on existing code (auto-generates everything) |
| **It produces** | Ranked population, single best. No Pareto front — single `combined_score` |
| **Codebase requirement** | Same as SkyDiscover default: one file, EVOLVE-BLOCK markers, stable interface |
| **Iteration speed** | 5-10x faster than SkyDiscover when evaluator is slow (async pipeline overlaps proposal + eval) |
| **Key capabilities** | Async pipeline (ideal for 10-30s cargo bench). Cross-program crossover. `shinka-convert` auto-setup. UCB model selection across LLMs |
| **Track record** | Won ICFP 2025 programming contest |

**Architecture:**

```
AsyncRunner
  ProposalWorkers (LLM calls) ─┐
                                ├─ decoupled queues
  EvalWorkers (subprocess)    ─┘
  Database (island-based, embedding-deduped)
  LLM Ensemble (UCB bandit picks model per iteration)
```

**How it works:**
- **Async pipeline**: Proposal and evaluation run in separate process pools. 5-10x throughput when eval is slow (10-30s)
- **Patch generation**: Three types — diff (0.6), full rewrite (0.3), crossover (0.1). Scoped to EVOLVE-BLOCK markers
- **UCB model selection**: Bandit picks which LLM per iteration, with cost-aware penalty
- **Novelty filter**: Embedding-based similarity check prevents population collapse

**Key insight:** ShinkaEvolve's advantage is purely throughput — it overlaps proposal generation with evaluation in async pools. Adaptive control is limited to model selection (UCB picks which LLM). No exploration intensity control, no meta-guidance. Choose it when your evaluator is slow (10-30s) and wall-clock time matters more than sample efficiency.

### Nous (Controlled Experiments, Single-Candidate)

| | |
|---|---|
| **You provide** | Campaign config (hypothesis template, iteration count). Full repo access — no file extraction |
| **It produces** | Code changes per iteration (git worktree). Accumulated `principles.json`. Results per hypothesis arm (confirmed/refuted) |
| **Codebase requirement** | Working build + test + bench. No single-file constraint — edits any file |
| **Iteration speed** | Multi-phase per iteration (framing → design → plan → execute → analyze → extract). `claude -p` phases + LLM API calls + benchmark execution. Human gates optional (`skip_reviews: true`, `auto_approve`). Heavier than single-file tools but needs far fewer iterations (5-10 vs 100-500) |
| **Key capabilities** | Causal reasoning (control-negative arms). Multi-file edits. Explains WHY. Identifies regime boundaries ("frequency wins above 70% capacity") |
| **Track record** | 30 iterations on BLIS simulator → 73.7% TTFT P99 reduction |

**Architecture:**

```
Deterministic State Machine (engine.py)
  Phase loop: INIT → DESIGN → HUMAN_DESIGN_GATE → EXECUTE_ANALYZE → HUMAN_FINDINGS_GATE → DONE
  Atomic checkpoint via fsync + rename. Crash-safe.

Two Dispatch Paths:
  CLIDispatcher: claude -p subprocess with repo access (design, execute_analyze)
  LLMDispatcher: API calls (structured output phases)
```

**How it works:**
- **Hypothesis bundles**: Design phase produces YAML with h-main + h-control-negative arms
- **Execution**: Runs in isolated git worktree. Shell commands per arm (build, bench, test). `git checkout -- .` resets between conditions
- **Fast-fail** (prompt-level guidance): If h-main REFUTED early → agent may skip remaining arms. If control-negative REFUTED AND h-main not confirmed → redesign (confounded)
- **Principles**: JSON store injected into every subsequent prompt. Extractor can INSERT, UPDATE, or PRUNE
- **Campaigns**: Multi-iteration loops. Human continue-gate between iterations

**Key insight:** Nous is the only tool that answers "why does this work?" and "at what load does it stop working?" via controlled experiments with control-negative arms. Knowledge compounds through `principles.json` (INSERT/UPDATE/PRUNE). Use for inter-component targets or when you need causal understanding before committing to an approach.

For algorithm pseudocode, JSON schemas, and implementation details for all frameworks, see §6 (Reference).

### When Each Framework Wins

| Factor | SkyDiscover (AdaEvolve) | SkyDiscover (EvoX) | GEPA | K-Search | ShinkaEvolve | Nous |
|---|---|---|---|---|---|---|
| Many simple variants to explore | **Best** (population parallelism, islands) | Good (single population but adapts selection) | Overkill | Overkill | Good (async throughput) | Too slow |
| One complex target, deep optimization | Wastes iterations on dead ends | Good (evolves strategy to focus on promising regions) | Good (reflective mutation avoids dead ends) | **Best** (backtracking + world model reasoning) | Wastes iterations on dead ends | Too slow per iteration |
| Need Pareto front (multiple non-dominated solutions) | **Yes** (multi-objective Pareto mode) | No (single `combined_score`; would need manual Pareto in evaluator) | **Yes** (instance-level Pareto) | No (single best) | No (single `combined_score`) | No |
| High failure rate (most mutations don't compile) | Handles via volume | Better (evolved strategy can learn to avoid failure patterns through parent selection) | Better (reflection reads compiler errors) | **Best** (reasons about WHY, backtracks from dead ends) | Handles via volume | N/A (manual implementation) |
| Need to ship multiple variants | **Yes** (population = multiple solutions) | Yes (population preserved across strategy switches) | **Yes** (Pareto pool) | No (one optimized solution) | Yes (ranked population) | No (one implementation per arm) |
| Limited eval budget (<500 calls) | Wasteful (needs 100-200+) | Good at 100 iterations (paper default); poor below 30 | **Best** (35× more sample-efficient than RL) | Good (world model avoids wasting evals; paper uses 120 total) | Wasteful (volume-based) | Best per-iteration but expensive |
| Evaluator is slow (10-30s) | Sequential (slow wall-clock) | Sequential (slow wall-clock) | Sequential (slow wall-clock) | Sequential (slow wall-clock) | **Best** (async pipeline overlaps proposal + eval) | N/A |
| Need causal explanation (WHY does it win?) | No | No (adapts empirically, no causal model) | Partial (diagnostic reflection) | Partial (inferential from world model) | No | **Best** (control-negative arms, regime sweeps) |
| Multi-file target (spans crates) | Yes (claude_code mode, 10-50x slower) | No (single-file solutions) | Yes (native `dict[str,str]` multi-param — not full repo but coordinates multiple text artifacts) | Partial (CUDA: 3-file output; other targets: single file) | No | **Best** (full repo, git worktree) |
| Need adaptive search (auto-tunes itself) | Good (3-level hierarchy + paradigm breakthrough) | **Best** (search strategy itself is evolved — can restructure selection logic, not just tune parameters) | Limited (Pareto selection provides diversity but no explicit adaptive mechanism) | Yes (stagnation-aware backtracking + difficulty relaxation) | Limited (UCB model selection only) | No (fixed hypothesis structure) |
| Optimization landscape shifts mid-run (explore→exploit phase transition) | Partial (Meta-Guidance generates tactics on stagnation; island spawning helps; but cannot restructure the selection/variation logic itself) | **Best** (observes phase transition via population descriptor; evolves new strategy matched to current phase) | No (fixed selection logic) | No (fixed backtracking rules) | No | No |

---

## 3. Decision Guide

```
Do you know WHAT to optimize, or are you choosing BETWEEN approaches?
├── Choosing between approaches →
│   ├── Need to know WHY one wins, or at what load it stops winning? → Nous
│   └── Just need the winner on your benchmark? → SkyDiscover (seed one island per approach)
└── Know what to optimize →
    ├── Multiple competing objectives (can't collapse to one score)? → SkyDiscover (Pareto) or GEPA
    └── Single score or weighted composite? →
        ├── Target is complex with structured search space (multiple algorithmic families,
        │   high failure rate, need to reason about bottlenecks)? → K-Search
        ├── Limited eval budget (<500 calls) and evaluator gives diagnostics? → GEPA
        ├── Evaluator is slow (>10s) and large budget? → ShinkaEvolve (async pipeline)
        ├── Budget ≥ 50 iterations, unknown which search strategy works best,
        │   landscape may shift mid-run? → SkyDiscover (EvoX)
        └── Fast evaluator, large budget, need adaptive allocation
            with multi-objective Pareto? → SkyDiscover (AdaEvolve)
```

**EvoX vs AdaEvolve decision shortcut:** If you would normally pick AdaEvolve and (a) you have ≥50 iteration budget, (b) you don't need explicit Pareto front output, and (c) you suspect the optimal search strategy may shift as solutions improve — use EvoX instead. It subsumes AdaEvolve's role and outperforms it on 96% of benchmarks tested (196 problems, same eval budget).

**Pipeline pattern** (for high-stakes targets only): Nous → GEPA/EvoX → K-Search. Most targets use one or two stages — the full pipeline is for targets needing both explainability and peak performance.

---

## 4. Certus Evolution Plan

Production flows through `NativeCertusOffloadingManager` → Rust `CertusEngine` → dispatcher → dispatch-map. All evolution must ultimately land in the Rust storage engine, but exploration can happen in Python first.

### 4.1 Target Constraints

Anything with an evaluator (fast feedback signal) can be evolved. Constraints:

1. **Fast-enough evaluator** — Population tools need 100-500 evals, so eval speed directly constrains campaign duration. Nous needs only 5-20 iterations but each iteration is multi-phase (heavier). Either way, the evaluator (cargo bench, trace replay) must complete reliably within a timeout.
2. **Scalar (or multi-scalar) fitness signal** — Pass/fail alone isn't enough. A composite `0.4 * hit_ratio + 0.3 * (1/p99) + 0.3 * ops_per_sec` works. For Pareto mode, return separate objectives.
3. **Isolatable** — Clear boundary where you swap one implementation for another. Single-file targets are 10-50x faster than multi-file.
4. **Safe to evaluate** — LLM-generated code that panics/deadlocks must not take down the test harness. Timeout + sandboxing required.

### 4.2 Target Landscape

**Scope definitions:**
- **Intra-component** — self-contained within one crate; evaluator is `cargo bench -p <crate>` (~15-30s). All single-file tools work directly.
- **Inter-component** — behavior spans multiple crates (e.g., eviction policy in memory-tier + trigger logic in dispatcher + flush coordination in background writer). Evaluator is `cargo bench -p dispatcher` (full pipeline, ~30-60s). Requires either Nous (full repo), GEPA (multi-param dict can coordinate multiple code artifacts), or SkyDiscover (AdaEvolve) claude_code mode (multi-file, 10-50x slower).
- **Whole-system** — validates composition of independently-evolved components. Evaluator is full pipeline + end-to-end latency under production-like load. Only Nous (H7-style factorial) or manual benchmarking.

**Framework selection for intra-component targets:**

All frameworks work on intra-component targets. Single-file tools (SkyDiscover, GEPA, K-Search, ShinkaEvolve) run one eval per iteration (cargo bench). Nous runs multiple phases per iteration (framing → design → execute → analyze → extract) with optional human gates — heavier per iteration but needs far fewer iterations (5-10 vs 100-500). Use Nous when you need causal understanding before optimizing:

| Tool | When to pick it | Tradeoff |
|---|---|---|
| **SkyDiscover (EvoX)** | Best default for ≥50 iteration budget. Self-adapting search that co-evolves its own strategy; outperforms all fixed-strategy methods on 96% of benchmarks. Especially strong when the optimal balance between exploration and exploitation shifts as solutions improve. | ~10% LLM overhead for strategy evolution; no native Pareto front (use AdaEvolve Pareto mode for multi-objective) |
| **SkyDiscover (AdaEvolve)** | Need explicit multi-objective Pareto front, or budget < 50 iterations where EvoX cannot trigger strategy evolution. Adaptive 3-level hierarchy, Meta-Guidance on stagnation. | Most capable fixed-algorithm population search. Highest per-iteration overhead (island management, UCB, context building) |
| **GEPA** | Limited eval budget (<500 calls), evaluator gives rich diagnostics (compiler errors, bench traces). | Most sample-efficient — reflective mutation extracts maximum signal per eval. Runs standalone with its own `optimize_anything` API. |
| **K-Search** | Complex target with multiple valid algorithmic approaches, high failure rate, need to reason about bottlenecks. | Deepest single-candidate optimization — structured backtracking avoids wasting evals on dead ends. Slower per cycle (multiple LLM calls). |
| **ShinkaEvolve** | Evaluator is slow (10-30s) and you want maximum wall-clock throughput. | Async pipeline overlaps proposal generation with evaluation — 5-10x throughput. Less adaptive (no Meta-Guidance, no dynamic islands). |
| **Nous** | Need to know WHY one algorithmic family beats another before committing, or need causal regime boundaries (e.g., "frequency wins above 70% capacity"). Also useful as a scouting phase before handing off to population tools. | Heavier per iteration (multi-phase + optional human gates) but needs only 5-20 iterations total. Gives causal evidence via control-negative arms. Use as Phase 1 for targets where the winning approach is unknown (H1, H2, H3). |

When the table below says "SkyDiscover (AdaEvolve)" it means the default algorithm. "SkyDiscover (EvoX)" means EvoX mode. "GEPA" means running GEPA standalone (its own `optimize_anything` API, not inside SkyDiscover).

**Tier 1: Component targets**

| # | Target | File | LOC | Scope | Current | Evaluator | Framework | Hypothesis |
|---|---|---|---|---|---|---|---|---|
| 1 | **Transfer path** | `dispatcher/v1/src/pipeline.rs` + `gpu-services/v0/src/dma.rs` | 135+742 | Inter | Fixed 4-buffer ring, SSD→DRAM→GPU | End-to-end transfer latency × bandwidth | **Nous** | H8 |
| 2 | **Eviction policy** | `memory-tier/v0/src/lru.rs` | 222 | Intra (family selection) → Inter (integration) | Pure LRU doubly-linked list | Trace replay → hit ratio + p99 | **Nous** → GEPA / K-Search | H1, H2 |
| 3 | **Memory allocator** | `memory-tier/v0/src/allocator.rs` | 177 | Intra | First-fit free-list, BTreeMap, coalescing | Fragmentation × latency × utilization | SkyDiscover (EvoX) or ShinkaEvolve | — |
| 4 | **PIPELINE_RING_SIZE** | `pipeline.rs:16` | 1 | Intra | Hardcoded `4` | Transfer bandwidth saturation | SkyDiscover (EvoX) or ShinkaEvolve | Part of H8 |
| 5 | **Background writer** | `dispatcher/v1/src/background.rs` | 244 | Intra | FIFO queue, sequential writes | Write-amp × flush p99 × stall rate | SkyDiscover (EvoX) or ShinkaEvolve | H5 |
| 6 | **DRAM demotion trigger** | `dispatcher/v1/src/lib.rs:271-291` | ~20 | Inter (calls memory-tier + dispatch-map) | `evict_for_space()` — DRAM demotion (moves entry to BlockDevice, data stays on NVMe) | Demotion batch overhead × tail latency | Nous or GEPA | H3 (partial) |
| 7 | **Dispatch-map concurrency** | `dispatch-map/v0/src/{state,lib}.rs` | 68+lib | Intra | HashMap + Mutex (single global lock) | Concurrent ops/s under contention | K-Search or SkyDiscover (AdaEvolve, Pareto mode) | — |

**Tier 2: Meta-level control systems (Phase 3-4)**

| Target | Scope | Interface | Evaluator | Framework |
|---|---|---|---|---|
| Workload classifier | Intra | `fn classify(access_stream) -> WorkloadType` | Classification accuracy vs ground-truth | SkyDiscover (EvoX) |
| Adaptive selector (bandit params) | Intra | UCB constants, observation window | Convergence speed × total regret | SkyDiscover (EvoX) |
| Coordinator thresholds | Whole-system | τ_stagnation, redistribution weights | Campaign efficiency (iterations to target) | SkyDiscover (EvoX) — meta-evolution is native |
| Evaluator weights | Whole-system | hit-ratio / p99 / ops_s balance | Discrimination between good/bad policies | Nous |

**Tier 3: Non-code targets**

| Target | Scope | What's evolved | Evaluator | Framework |
|---|---|---|---|---|
| Domain context prompt | Intra | Constraints injected into evolution prompts | Compile success rate + score/iteration | GEPA or SkyDiscover (EvoX) |
| Trace replay curriculum | Whole-system | Which traces to evaluate against | Generalization to held-out traces | Nous |
| Rulebase governance predicates | Whole-system | `relax_when` conditions | Prediction accuracy | SkyDiscover (EvoX) |

**How scope constrains framework choice:**
- **Intra-component** — all single-file tools work. Pick based on target complexity: SkyDiscover (EvoX) as default (self-adapts, outperforms fixed strategies), K-Search for complex targets needing backtracking, GEPA when eval budget is limited, ShinkaEvolve when async throughput matters, AdaEvolve when explicit Pareto front needed
- **Inter-component** — spans multiple crates, so requires either Nous (full repo, multi-file edits per arm), GEPA (multi-param `dict[str, str]` can coordinate changes across multiple code artifacts with shared evaluator), or factored evaluators that test the interaction through the downstream crate's benchmarks (allowing single-file tools to optimize one side)
- **Whole-system** — evaluator runs full pipeline (minutes per eval), too slow for automated iteration tools — use Nous or manual benchmarking only

### 4.3 Execution Order

| Phase | Target(s) | Framework | Rationale |
|---|---|---|---|
| **1** | Transfer path (H8) | Nous | Binary architectural decision (bounce vs P2P) that shapes the entire read path. Both paths exist — can benchmark immediately. |
| **2** | Eviction policy (H1, H2) | Nous → EvoX / GEPA | Need to know which family wins before optimizing within it. Nous establishes the answer in 8-10 iterations, then EvoX (preferred for ≥50 iter budget, self-adapts to the winning family's landscape) or GEPA (if eval budget is tight) optimizes within it. |
| **3a** | Allocator | SkyDiscover (EvoX) or ShinkaEvolve | Independent of eviction. Simple target, fast eval. EvoX preferred — will adapt from broad exploration to refinement as allocator improves. ShinkaEvolve if wall-clock speed is priority (async pipeline). |
| **3b** | Background writer (H5) | SkyDiscover (EvoX) or ShinkaEvolve | Independent of eviction. Can run in parallel with 3a. Same rationale as 3a. |
| **4** | Dispatch-map concurrency | K-Search | Complex target — multiple valid approaches (sharded, lock-free, RCU), high failure rate, needs backtracking. |
| **5** | Composition validation (H7) | Nous | After subsystem optimization plateaus, validate that evolved components compose well. |

Phases 3a and 3b can overlap. Phase 4 can start as soon as Phase 2 Nous concludes (independent target).

### 4.4 Hypotheses

**H1: "Frequency-based eviction outperforms LRU under prefix-sharing workloads"**
- Arms: Frequency vs LRU (current `lru.rs`) vs Random (control-negative)
- Metric: Hit-ratio at 80% capacity, p99 eviction latency
- Iterations: 8-10
- Code target: Replace `LruList` in `memory-tier/v0/src/lru.rs` with frequency tracker

**H2: "Position-aware eviction (never evict depth < N) adds value beyond frequency alone"**
- Arms: Frequency-only vs Frequency+depth-guard vs Depth-only
- Metric: Hit-ratio on agentic trace, wasted cache (% holding dead leaves)
- Iterations: 10-12
- Code target: Extend `IMemoryTier::evict_lru()` to accept scoring context

**H3: "Proactive TTL-based eviction reduces pressure spikes vs reactive evict-on-demand"**
- Arms: Current watermark (demand-driven eviction in `prepare_store` via `dispatcher.remove()`) vs TTL-based (background reaper removes stale entries) vs Aggressive watermark (evict to 70% when hitting 80%)
- Metric: Eviction burst size × tail latency under pressure × steady-state hit ratio
- Iterations: 8
- Code target: `certus-connector/src/engine.rs` `prepare_store()` (capacity eviction — removes entries entirely) + `dispatcher/v1/src/lib.rs:271-291` `evict_for_space()` (DRAM demotion — moves entries to NVMe, NOT removal)
- Note: The "Watermark" arm is already implemented — engine.rs `prepare_store` uses `eviction_watermark` and proactively calls `dispatcher.remove()` for LRU entries when `entry_count + to_store > watermark`. The remaining question is whether background/TTL-based eviction on top of this demand-driven approach reduces tail latency spikes further.
- Clarification: Two distinct eviction layers exist: (1) **Capacity eviction** in engine.rs `prepare_store` — calls `dispatcher.remove()`, entry is gone entirely, triggered by vLLM contract. (2) **DRAM demotion** in `evict_for_space()` — calls `mt.evict_lru()` + `dm.convert_memory_tier_to_block()`, entry still accessible from NVMe, triggered internally by `populate()` when DRAM staging pool is full.

**H4: "Re-promotion threshold should be >1 (not promote-on-first-miss from SSD)"**
- Arms: Promote-on-1st-miss (current) vs Promote-on-3rd-miss vs Never-re-promote
- Metric: DRAM hit ratio, unnecessary promotion rate, NVMe read amplification
- Iterations: 6-8
- Code target: `promote_and_serve()` in `dispatcher/v1/src/lib.rs:189-265`

**H5: "Coalescing background writes by spatial locality reduces NVMe write amplification"**
- Arms: FIFO (current `background.rs`) vs Spatial-coalesce vs Deadline-ordered
- Metric: Write amplification factor, flush latency p99, eviction stall rate
- Iterations: 10
- Code target: `dispatcher/v1/src/background.rs` WriteJob scheduling

**H6: "Priority hints from context layer eliminate >50% of the eviction decision space"**
- Arms: No-hints (frequency only) vs Hard-hint (obey strictly) vs Soft-hint (scoring weight)
- Metric: Decision quality — % of evicted blocks re-requested within 5min
- Iterations: 8-10

**H7: "Cross-component: aggressive eviction + conservative batching > conservative eviction + aggressive batching"**
- Arms: 2×2 factorial (eviction aggressiveness × batching aggressiveness)
- Metric: End-to-end throughput under sustained 90% capacity pressure
- Iterations: 5-8

**H8: "Bounce-buffer SSD→CPU→GPU with pipelined transfers is faster than direct SSD→GPU P2P for 4MiB as 32×128KiB stream"**
- Arms:
  - A (bounce): `pipelined_ssd_to_gpu()` in `pipeline.rs` — SSD→DRAM ring→GPU, pipelined
  - B (P2P): `create_spdk_dma_buffer_from_gpu_bar()` in `dma.rs` — GDRCopy BAR1 → NVMe direct to GPU
  - C (control-negative): Single synchronous read → single DMA copy
- Metric: End-to-end latency for 4 MiB, throughput under streaming, CPU utilization
- Iterations: 5-8
- Code target: Both paths exist. Determines default data path and whether pipelining should be added to P2P.

### 4.5 Python Prototyping

The Python `CertusOffloadingManager` (322 LOC) evaluates in <1s vs 30-60s for cargo bench. Use SkyDiscover (EvoX)/ShinkaEvolve on Python first as a scouting phase — discover promising algorithmic shapes in minutes, then implement winners as Rust `IEvictionPolicy` impls for micro-optimization. 100-1000x more iterations per hour during exploration. EvoX is particularly well-suited here: the fast evaluator means strategy evolution triggers quickly and the system can explore many different search strategies within a single run.

---

## 5. Implementation: How To Set Up Evolution

### How Each Framework Mutates Code (Critical Constraints)

| Framework | Mutation Model | File Scope | What It Sees |
|---|---|---|---|
| **ShinkaEvolve** | LLM rewrites content within EVOLVE-BLOCK markers in a single file | **ONE file** (`initial.rs`) | The full file content; only markers region is mutable |
| **SkyDiscover (AdaEvolve)** | LLM generates complete solution string, written to single temp file, evaluated | **ONE file** (generated solutions max 60K chars) | Parent solution + inspiration programs from archive + optional tactic. Context mode: LLM can `read_file`/`search` codebase (read-only, no shell) |
| **SkyDiscover (EvoX)** | Same as AdaEvolve for solutions. Additionally: meta-LLM generates complete `EvolvedProgramDatabase` Python class (the search strategy), validated and hot-swapped | **ONE file** for solutions; search strategy is ONE Python class | Solution LLM: parent + context programs (selected by the evolved strategy's `sample()`) + variation operator (diverge/refine/free-form). Strategy LLM: prior strategies from H + their scores + population descriptor ϕ(Dₜ) |
| **SkyDiscover (claude_code)** | Claude Code CLI in Docker workspace with full tool access; can edit multiple files | **Multi-file** during run, but checkpoints track ONE solution file | Full workspace (repo map, shell access) |
| **GEPA (optimize_anything)** | LLM reflects on execution traces + ancestry lessons → proposes full candidate rewrite | **Multi-param** (`dict[str, str]` — multiple named text parameters mutated independently per iteration; not filesystem-aware) | Full execution traces (compiler errors, bench output, ASI diagnostics) + ancestry chain lessons + evaluator feedback |
| **K-Search** | LLM generates code from spec+action or base+action; world model guides what to generate | **ONE task target** (Triton/Python/MLX) or **THREE files** (CUDA: kernel.h + kernel.cu + main.cpp parsed from XML output) | Problem spec + world model JSON + chosen action + previous code + eval results |
| **Nous** | Claude Code CLI in isolated git worktree; runs shell commands per arm | **Multi-file, full repo** | Entire repo, shell access, build/test/bench commands |

### File Extraction (Required for ShinkaEvolve / SkyDiscover default / K-Search non-CUDA)

The evolvable logic must be extracted into **one file per evolution target** that:
1. Is self-contained enough that rewriting it doesn't break compilation
2. Has a stable interface boundary (function signatures, struct layouts used by other files)
3. Is small enough for the LLM to reason about effectively (~200-500 LOC sweet spot, <1000 LOC max)

**Actual Certus codebase structure (what exists today):**

```
components/memory-tier/v0/src/
  lib.rs              ← IMemoryTier impl: insert/get/evict_lru/remove (ORCHESTRATION)
  lru.rs              ← EVOLVE TARGET: LruList (222 LOC) — pure doubly-linked list, no frequency/scoring
  allocator.rs        ← EVOLVE TARGET: FreeList first-fit allocator (177 LOC)

components/dispatcher/v1/src/
  lib.rs              ← IDispatcher impl: populate/lookup/evict_for_space (ORCHESTRATION)
  pipeline.rs         ← EVOLVE TARGET: pipelined_ssd_to_gpu (135 LOC) — ring-buffer SSD→DRAM→GPU
  background.rs       ← EVOLVE TARGET: BackgroundWriter FIFO queue (244 LOC)
  io_segmenter.rs     ← STABLE (hardware-dictated MDTS math)

components/dispatch-map/v0/src/
  lib.rs              ← IDispatchMap impl + concurrency logic (EVOLVE TARGET for concurrency)
  entry.rs            ← EVOLVE TARGET: DispatchEntry struct + state transitions (56 LOC)
  state.rs            ← CacheState wrapper (68 LOC) — HashMap+Mutex, actual lock scope in lib.rs

components/gpu-services/v0/src/
  dma.rs              ← EVOLVE TARGET (path selection): P2P via GDRCopy BAR1 vs bounce-buffer
  memory.rs           ← STABLE (memory attribute checks)
```

**Note**: Two distinct eviction mechanisms exist:
1. **DRAM demotion** (`evict_for_space()` at `dispatcher/v1/lib.rs:271-291`): calls `mt.evict_lru()` to free DRAM slots, transitions entries to BlockDevice (still accessible from NVMe). Uses `memory-tier/lru.rs` for LRU ordering.
2. **Capacity eviction** (`prepare_store()` in `certus-connector/src/engine.rs`): calls `dispatcher.remove()` to permanently delete entries when cache is over watermark. Uses `dispatch_map.oldest_keys()` for LRU ordering.

File extraction for population tools would need to:
1. For DRAM demotion: factor `evict_for_space()` + `lru.rs` into a standalone `eviction.rs` with a trait interface
2. For capacity eviction: the logic is already isolated in `engine.rs` `prepare_store()` — extract as a standalone policy function
3. The current `IMemoryTier::evict_lru()` returns the LRU key — evolving this requires changing the interface to accept scoring context

### Evaluator

All frameworks need the same evaluator. The existing Criterion benchmarks ARE the evaluators:

```python
import subprocess, json

def evaluate(program_path: str) -> dict:
    patch_component(program_path, target="components/dispatcher/src/eviction.rs")

    # Stage 1: Build (fast-fail)
    result = subprocess.run(["cargo", "build", "-p", "dispatcher"],
                           capture_output=True, timeout=60)
    if result.returncode != 0:
        return {"combined_score": 0.0,
                "artifacts": {"build_error": result.stderr[-2000:]}}

    # Stage 2: Correctness (fast-fail before expensive benchmarks)
    result = subprocess.run(["cargo", "test", "-p", "dispatcher"],
                           capture_output=True, timeout=90)
    if result.returncode != 0:
        return {"combined_score": 0.0,
                "artifacts": {"test_error": result.stderr[-2000:]}}

    # Stage 3: Performance (only reached by correct mutations)
    result = subprocess.run(
        ["cargo", "bench", "-p", "dispatcher", "--bench", "dispatcher_benchmark"],
        capture_output=True, timeout=120)

    metrics = parse_criterion_json(result.stdout)
    return {"combined_score": metrics["ops_per_sec"],
            "pareto": {"ops_per_sec": metrics["ops_per_sec"],
                       "p99_ns": metrics["p99_latency_ns"],
                       "hit_ratio": metrics["hit_ratio"]}}
```

**Error-feedback loop**: Build and test failures are returned as `artifacts` — SkyDiscover and ShinkaEvolve inject these into the next mutation prompt. This teaches the LLM which patterns fail (borrow checker violations, deadlocks under concurrent access, panics on empty maps). Error feedback consistently brings compile success from ~30-50% to >90% across multiple code-evolution systems.

**For K-Search**: The evaluator wraps as a task backend:

```python
class CertusBenchTask:
    def get_definition_text(self, language: str) -> str:
        """Return the 'kernel spec' — trait interface + constraints + domain context"""
        return EVICTION_TRAIT_SPEC + DISPATCHER_CONSTRAINTS + SPDK_SAFETY_RULES
    
    def run_benchmark(self, solution, dump_traces=False, round_num=0) -> EvalResult:
        """Build + test + bench, return EvalResult"""
        # ... same cargo build/test/bench as above ...
        return EvalResult(status="passed", metrics={"score": combined_score, ...})
    
    def code_for_world_model_from_raw(self, raw, language) -> str:
        """Extract the relevant code excerpt for world model prompts"""
        return raw  # the full eviction.rs content
```

### Domain Context Injection

All frameworks benefit from architectural context. Without it, the LLM proposes mutations that compile but violate system invariants.

| Document | What it teaches | Prevents |
|---|---|---|
| `docs/dsfs_constitution.md` | Block immutability, content-addressable, write-once-read-many | Mutations that assume mutable blocks |
| `knowledge/components/dispatcher-v1.md` | Actor model, SPDK reactor constraints, DMA lifecycle | Mutations that allocate/block in reactor context |
| `specs/001-dispatch-map/data-model.md` | `DispatchEntry` fields, `write_ref`/`read_ref` semantics | Mutations that evict entries with active DMA refs |
| Evolved experiment results | Which algorithmic shapes already worked | Redundant rediscovery |

**Format for SkyDiscover/AdaEvolve**: Add as `context_prompt` field in campaign config — prepended to every mutation request. Also available to Meta-Guidance's tactic generator.

**Format for SkyDiscover/EvoX**: Same as AdaEvolve for solution evolution. Additionally, domain context is injected into the variation operator generation prompt (the guide LLM reads it when generating diverge/refine operators). The `system_message` in config is passed to both the solution LLM and the strategy-generation LLM. The strategy LLM also receives a population state summary (statistics, score distribution, trajectory) alongside the strategy history H.

**Format for ShinkaEvolve**: Add as comment block above EVOLVE-BLOCK markers.

**Format for K-Search**: Include in `get_definition_text()` return value — becomes part of the world model's problem spec.

Without domain context, ~40% of syntactically valid mutations violate SPDK threading or DMA safety invariants not enforced by the type system alone.

### Evolution Scope

See §4.2 for scope definitions and per-target scope assignments. The general principle: start intra-component for independent targets (fastest iteration), use inter-component scope for behaviors that span crate boundaries (eviction trigger, transfer path), and reserve whole-system scope for composition validation after subsystem optimization plateaus.

---

## 6. Framework Implementation Details (Reference)

Pseudocode, JSON schemas, and design decisions for anyone setting up or debugging campaigns. Architecture diagrams and operational overviews are in §2.

### 6.1 SkyDiscover / AdaEvolve Core Loop

```python
for t in range(T):
    # Level 2: Global Adaptation — select island via UCB
    k = select_island_ucb(R, V, n, C=sqrt(2))
    
    # Level 1: Local Adaptation — compute exploration intensity
    I_k = I_min + (I_max - I_min) / (1 + sqrt(G[k] + eps))
    
    # Sample parent and inspirations (adaptive)
    if random() < I_k:  # exploration
        parent = sample_uniform(archive[k])
        inspires = most_diverse(archive[k])
    else:  # exploitation
        parent = sample_top_quartile(archive[k])
        inspires = highest_fitness(archive[k])
    
    # Build context, optionally inject tactic
    prompt = context_builder(parent, inspires, current_tactic)
    
    # Mutate and evaluate
    child = llm(prompt)
    score = evaluator(child)
    archive[k].add(child)
    
    # Update adaptive state
    if score > f_star[k]:
        delta = (score - f_star[k]) / (abs(f_star[k]) + eps)  # local normalized
        G[k] = rho * G[k] + (1 - rho) * delta**2
        reward = (score - f_star[k]) / (abs(f_star_global) + eps)  # global normalized
        f_star[k] = score
        if score > f_star_global:
            f_star_global = score
    else:
        G[k] = rho * G[k]  # decays during stagnation
    
    # Level 3: Meta-Guidance — on global stagnation
    if all(G[k] <= tau_M for all k) and no_active_tactic:
        tactic = generate_solution_tactics(spec, evaluator_code, best_program, failures)
        inject_tactic(tactic)
    
    # Dynamic island spawning — on deeper stagnation
    if all(G[k] <= tau_S for all k):
        spawn_new_island(seed=random_from_archive())
    
    # Ring migration — every M iterations
    if t % M == 0:
        migrate_top_programs_ring()
```

**Critical design decisions:**
- **Global normalization for UCB**: Rewards normalized by f*_global (best across ALL islands), not f*_local. Prevents "poor island bias" where an island making trivial improvements at low fitness steals compute from a near-optimal island
- **Exponential decay on R(k) and V(k)**: UCB tracks RECENT productivity. Old improvements fade. An island that was productive 50 iterations ago doesn't continue receiving budget
- **Dynamic island spawning**: When ALL islands stagnate, spawn fresh island with random seed from archive. Not manual — triggered automatically by τS threshold
- **Tactics are mandatory**: When Meta-Guidance fires, the generated tactic is injected as "You MUST implement this breakthrough idea" — not a suggestion

### 6.2 EvoX Co-Evolution Core Loop

```python
class CoEvolutionController:
    """Two-level co-evolution: solutions + search strategies."""
    
    DEFAULT_SWITCH_RATIO = 0.10   # Evolve search after 10% of budget stagnates
    DEFAULT_IMPROVEMENT_THRESHOLD = 0.01

    async def run_discovery(self, start_iteration, max_iterations):
        switch_interval = max(1, int(max_iterations * self.DEFAULT_SWITCH_RATIO))
        
        # Generate problem-specific variation operators (diverge/refine)
        await self._generate_variation_operators()
        
        iteration = start_iteration
        while iteration < start_iteration + max_iterations:
            # === Phase I: Solution evolution under current strategy ===
            result = await self._run_iteration(iteration)
            self._record_search_window_step()  # Track best score for strategy scoring
            iteration += 1
            
            # === Phase II+III: Check stagnation → evolve search strategy ===
            if self._should_evolve_search():
                await self._evolve_search(iteration)

    def _should_evolve_search(self) -> bool:
        """Stagnation-based trigger (demand-driven, not periodic)."""
        current = self._get_best_score()
        if (current - self._last_tracked_best_score) > IMPROVEMENT_THRESHOLD:
            self._stagnant_count = 0
        else:
            self._stagnant_count += 1
        
        if self._stagnant_count >= self._switch_interval:
            self._stagnant_count = 0
            return True
        return False

    async def _evolve_search(self, solution_iter):
        """Score previous strategy, generate and deploy new one."""
        # 1. Score the outgoing strategy
        #    J = improvement × (1 + log(1 + start_score)) / √horizon
        self._assign_search_score()
        
        # 2. Generate new strategy (LLM produces EvolvedProgramDatabase class)
        #    Conditioned on: strategy history H, population state summary (db stats formatted as text)
        result = await self.search_controller.run_discovery(max_iterations=1)
        
        # 3. Validate: structural checks + functional tests (metric preservation,
        #    sample() contract, migration compatibility)
        if not self._switch_to_new_search_algorithm(result):
            # Validation failed → keep current strategy, log failure
            return
        
        # 4. Hot-swap: migrate all programs to new database
        #    Previous database kept as fallback for rollback

    def _switch_to_new_search_algorithm(self, result) -> bool:
        """Load, validate, and deploy new search strategy."""
        search_code = result.child_program_dict["solution"]
        
        # Load as Python module → instantiate EvolvedProgramDatabase
        new_db_class, prog_class = load_database_from_file(search_code)
        new_db = new_db_class("evolved", config)
        
        # Assign variation operator labels
        new_db.DIVERGE_LABEL = self._diverge_label
        new_db.REFINE_LABEL = self._refine_label
        
        # Migrate all programs from old database
        for program in self.database.programs.values():
            new_db.add(program, iteration=program.iteration_found)
        
        # Keep fallback for rollback on runtime errors
        self._fallback_database = self.database
        self.database = new_db
        return True
```

**Search strategy scoring (LogWindowScorer):**

```python
def compute_metrics(start_score, best_scores, horizon):
    """Score = improvement × log-weight / √horizon."""
    running_best = start_score
    for s in best_scores:
        running_best = max(running_best, s)
    
    improvement = running_best - start_score
    log_weight = 1.0 + math.log(1.0 + max(0.0, start_score))
    combined_score = improvement * log_weight / math.sqrt(horizon)
    return {"combined_score": combined_score, ...}
```

**Search strategy validation (SearchStrategyEvaluator):**

Validates LLM-generated `EvolvedProgramDatabase` before deployment:
1. **Structural**: Class name = `EvolvedProgramDatabase`, inherits `ProgramDatabase`, `sample()` has correct signature
2. **add() contract**: Metrics are never modified or deleted by `add()`
3. **sample() contract**: Returns `Tuple[Dict[str, Program], Dict[str, List[Program]]]`, exactly one parent, context programs ≤ requested count
4. **Edge cases**: Handles programs with error strings in metrics, handles `None` scores
5. **Migration compatibility**: Works with base `Program` instances (not just `EvolvedProgram`) — critical for hot-swap

**Variation operator generation:**

At campaign start, a guide LLM analyzes the problem description + evaluator code and generates two operators in a single call:

```python
async def generate_variation_operators(system_message, evaluator_code, problem_dir, llm_pool):
    """Single LLM call → (diverge_operator, refine_operator)."""
    # COMBINED_SYSTEM_PROMPT instructs LLM to:
    #   Section 1 (EXPLORATION): Identify problem type → enumerate standard toolkit →
    #     suggest 3-6 categories of different approaches (libraries, algorithm families,
    #     constraint handling, multi-phase pipelines)
    #   Section 2 (EXPLOITATION): Identify tunable knobs → suggest 3-5 categories of
    #     refinement strategies (hyperparameters, initialization quality, tolerances, polish)
    
    result = await llm_pool.generate(
        system_message=COMBINED_SYSTEM_PROMPT,
        messages=[{"role": "user", "content": build_operator_prompt(...)}],
    )
    return parse_combined_response(result.text)
```

The diverge operator is applied when `sample()` returns `DIVERGE_LABEL` as the parent label; the refine operator when it returns `REFINE_LABEL`. The evolved strategy decides when to use each based on population state.

### 6.3 K-Search World Model JSON Schema

```json
{
  "kernel_summary": "One paragraph: classification + bottleneck hypotheses + constraints",
  "decision_tree": {
    "root_id": "root",
    "active_leaf_id": "current_node",
    "nodes": [
      {
        "node_id": "...",
        "parent_id": "...",
        "decision": "algorithmic choice point",
        "choice": "what this branch chose",
        "solution_ref": {"solution_id": "...", "eval": {"status": "...", "score": ...}},
        "impacts": {
          "memory_bandwidth": {"rating_0_to_10": 7, "risk": "...", "notes": "..."},
          "register_pressure": {"rating_0_to_10": 5, "risk": "...", "notes": "..."},
          "compute_intensity_and_hw_fit": {"rating_0_to_10": 8, "risk": "...", "notes": "..."}
        },
        "overall_rating_0_to_10": 7,
        "confidence_0_to_1": 0.65,
        "action": {
          "title": "small implementable change",
          "description": "...",
          "difficulty_1_to_5": 3,
          "score_0_to_1": 0.7,
          "expected_vs_baseline_factor": 1.15
        }
      }
    ]
  },
  "open_questions": ["3-8 unknowns that affect correctness/performance"],
  "computed_signals": {"round_index": 5, "trace": {"status": "passed", "speedup_factor": 1.2}}
}
```

### 6.4 GEPA Core Loop

```python
# Initialize
P = [seed_candidate]  # candidate pool
S = evaluate_all(seed, Dpareto)  # scores matrix

while budget > 0:
    # 1. SELECT: Pareto-based candidate sampling
    #    Build instance-level Pareto: for each task i, find best candidate
    #    Remove dominated candidates → sample proportional to # tasks led
    k = select_candidate_pareto(P, S)
    
    # 2. EXECUTE: Run on minibatch, capture traces
    M = sample_minibatch(Dfeedback, size=b)
    traces, scores, asi = execute_with_feedback(P[k], M, μf)
    
    # 3. REFLECT + MUTATE: LLM reads traces → proposes improvement
    #    Prompt includes: current candidate, ancestry lessons,
    #    execution traces, ASI diagnostics, failure patterns
    P_new = reflective_mutation(P[k], traces, asi, ancestry[k])
    
    # 4. ACCEPT/REJECT: Minibatch improvement check
    σ_new = avg_score(P_new, M)
    if σ_new > avg_score(P[k], M):
        # Evaluate on full Dpareto, add to pool
        S[P_new] = evaluate_all(P_new, Dpareto)
        P.append(P_new)
        ancestry.append(k)
        
        # 5. MERGE (proactive): combine P_new with an ancestor-related candidate
        if use_merge:
            pair = find_common_ancestor_pair(P, P_new)
            P_merged = ancestor_relative_merge(pair, ancestry)
            # ... accept/reject merge as above

return best_aggregate(P, S, Dpareto)
```

**Key design decisions:**

- **Instance-level Pareto (not global Pareto)**: Standard multi-objective keeps candidates that trade off objectives. GEPA keeps candidates that are BEST ON SPECIFIC INSTANCES — a candidate that leads on 3 out of 20 tasks is Pareto-optimal even if its average score is low. This preserves diverse strategies that work in different regimes.
- **Ancestry tracking for merge**: Each candidate tracks its parent lineage. This ancestry is used by the merge mechanism to find common ancestors and identify structural changes, not directly fed into the reflective mutation prompt (which sees only the current candidate + current evaluation feedback/traces).
- **Acceptance gating prevents population pollution**: Unlike AdaEvolve (which adds all valid mutations to the archive), GEPA rejects non-improving mutations. This keeps the Pareto pool clean and focused.
- **Ancestor-relative merge**: When two Pareto-optimal candidates excel on different task subsets, merge finds their common ancestor and identifies which component (predictor) each descendant CHANGED. Unchanged components keep the ancestor version; changed components take the descendant version. When both changed the same component, the higher-scoring descendant's version wins. More principled than ShinkaEvolve's crossover (which blindly combines code regions).

**For Certus targets:**

| Mode | Certus use case | How it works |
|---|---|---|
| Single-Task | Optimize `lru.rs` against one trace | `seed_candidate="<lru.rs content>"`, evaluator = cargo bench → score. ASI = Criterion timing JSON |
| Multi-Task | Optimize across 5 access patterns | `dataset=[trace1, ..., trace5]`, evaluator called per-trace. Cross-task transfer: "LFU wins on zipf, LRU wins on sequential" → hybrid |

### 6.5 Nous Additional Details

**Phase sequence (engine.py state machine):**

```
INIT → DESIGN → HUMAN_DESIGN_GATE → EXECUTE_ANALYZE → HUMAN_FINDINGS_GATE → DONE
```

The EXECUTE_ANALYZE phase handles planning, execution, analysis, and extraction internally within a single agent session. Each transition is validated against an immutable `MappingProxyType` transition table — no ad-hoc jumps (though a `force_phase()` escape hatch exists for error recovery). State persisted via atomic fsync + rename to `state.json`.

**Hypothesis bundle schema (bundle.schema.yaml):**

```yaml
arms:
  - type: h-main | h-ablation | h-super-additivity | h-control-negative | h-robustness
    prediction: "Quantitative claim with measurable threshold"
    mechanism: "Causal explanation of how/why"
    diagnostic: "What to investigate if prediction is wrong"
    code_changes:  # optional
      - file: "path/relative/to/repo"
        intent: "What change (plain English, not a patch)"
        rationale: "Why this tests the hypothesis"
```

Five arm types: `h-main` (primary hypothesis), `h-control-negative` (null condition proving mechanism is real), `h-ablation` (isolates one component's contribution), `h-super-additivity` (tests interaction effects), `h-robustness` (boundary conditions).

**Experiment plan (experiment_plan.schema.yaml):**

The EXECUTE_ANALYZE phase translates the bundle into executable shell commands:

```yaml
setup:
  - cmd: "cargo build -p memory-tier --release"
arms:
  - arm_id: "h-main"
    conditions:
      - name: "baseline"
        cmd: "cargo bench -p memory-tier --bench eviction -- --output-format json"
      - name: "treatment"
        cmd: "cargo bench -p memory-tier --bench eviction -- --output-format json"
```

Each arm has multiple conditions (baseline + treatment). Executor runs in an isolated git worktree; `git checkout -- .` resets between conditions.

**Principles store (principles.schema.json):**

```json
{
  "id": "P-003",
  "statement": "Frequency-based eviction dominates recency above 70% capacity",
  "confidence": "high",
  "regime": "capacity > 70%, zipfian access",
  "evidence": ["iter-2/findings.json", "iter-5/findings.json"],
  "contradicts": ["P-001"],
  "extraction_iteration": 2,
  "mechanism": "At high fill, recency bias evicts warm entries that will be needed within 100ms",
  "applicability_bounds": "Only validated for single-tenant, zipfian α>1.0",
  "superseded_by": null,
  "category": "domain",
  "status": "active"
}
```

Operations: INSERT (new principle), UPDATE (strengthen/weaken confidence, refine bounds), PRUNE (mark superseded). `category` distinguishes domain principles (about the target system) from meta principles (about the investigation process).

**Dual dispatch architecture:**

- **CLIDispatcher**: Invokes `claude -p` as subprocess with full repo access. Used for DESIGN (needs to grep code, discover metrics) and EXECUTE_ANALYZE (needs to read files, run commands). Timeout 1800s, max_turns overridden per phase in defaults.yaml (80 for design, 300 for execute_analyze)
- **LLMDispatcher**: OpenAI-compatible API calls. Used for structured output phases. Parses structured YAML/JSON from code fences, validates against schemas

Both share the same prompt templates (in `prompts/methodology/`) and routing table.

**Fast-fail rules (prompt-level guidance, not code-enforced):**
- If h-main REFUTED early → agent may skip remaining arms
- If h-control-negative REFUTED AND h-main not confirmed → REDESIGN outcome (confounded experiment)
- If single component accounts for >80% of effect → simplify advisory

These are instructions in the execute_analyze prompt template — the executor agent decides whether to apply them. The orchestrator does not enforce them via code-level conditional transitions.

**Campaign lifecycle (`run_campaign.py` → `run_iteration.py`):**

- `run_campaign.py`: Multi-iteration loop. Each iteration returns `COMPLETED | CONTINUE | ABORTED | REDESIGN`
- `run_iteration.py`: Single iteration through the full phase sequence. Supports resume — if process crashes, re-run picks up from last committed phase. Human gates at DESIGN and FINDINGS (can reject → loop back, or abort)
- Per-phase model selection: `campaign.yaml` can override model per phase (e.g., opus for design, sonnet for execution); resolved via `_model_for()` with priority chain: campaign.models > defaults.yaml > CLI flag

**Prompt templates (`prompts/methodology/`):**

4 templates: `design.md`, `execute_analyze.md`, `report.md`, `summarize_gate.md`

### 6.6 ShinkaEvolve Additional Details

- **Adaptive oversubscription**: Adjusts proposal/eval worker ratio based on timing
- **Multiple temperatures + UCB model selection**: Temperatures sampled uniformly from a list (default [0.0, 0.5, 1.0]) per LLM call; UCB bandit (AsymmetricUCB) selects among configured models
- **Meta-recommendations**: Every N generations, LLM analyzes population progress and generates strategic suggestions injected into prompts
- **Prompt self-evolution**: System prompt itself treated as evolvable when enabled
- **Agent skills**: `shinka-convert` auto-generates evaluator + EVOLVE-BLOCK from existing code

