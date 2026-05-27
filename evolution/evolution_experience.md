# Evolution Experience: Lessons from the Certus Pipeline Bakeoff

## Overview

This document captures operational learnings from running 6 evolutionary/agentic frameworks against the Certus storage system's SSD→GPU transfer pipeline. The goal: discover which framework best optimizes real systems code under hardware constraints.

**Campaign:** H3 — Multi-client concurrent throughput optimization
**Date:** 2026-05-26
**Duration:** ~3.5 hours (5 search frameworks sequential + Nous parallel)
**Result:** 4.4× throughput improvement (5 GB/s → 23 GB/s) achieved by Nous; search frameworks achieved 1.3-2.1×

---

## What We Learned About Each Framework

### Nous (Agentic Strategy Evolution)

**Strengths:**
- Designs complete multi-file patches — can coordinate changes across service.rs, main.rs, lib.rs simultaneously
- Hypothesis-driven: creates treatment/ablation/control arms for rigorous comparison
- Full repo access means no artificial constraints on what it can modify
- Found the optimal solution (full Mutex removal + per-drive sharding) in 1 iteration

**Weaknesses:**
- Extremely slow: consumed the entire 7200s timeout for 1 iteration
- Experiment execution has isolation bugs: control arm ran against the patched server
- No iterative refinement — if the first design is wrong, there's no recovery mechanism within a single iteration
- Overkill for parameter tuning; best suited for architectural changes

**When to use:** Multi-file refactoring, architectural decisions, when you need to understand WHY something works (causal analysis). Not for knob-turning.

---

### K-Search (via AdaEvolve backend)

**Strengths:**
- Highest compile rate (94%) among search frameworks — backtracking abandons broken branches
- Found best search-framework score (10.93 GB/s) on the very first iteration
- Consistent: only 2/12 evaluations failed to compile
- Good at finding "safe" mutations that don't break compilation

**Weaknesses:**
- Couldn't achieve the full Mutex removal (limited by concatenated-file output constraint)
- Test re-evaluation much lower than peak (6.97 vs 10.93) — solutions may be fragile
- Uses AdaEvolve internally, so the "tree search" distinction is thin in practice

**When to use:** When compile rate matters (expensive builds), when you want reliable incremental improvement, when the search space has many viable solutions.

---

### AdaEvolve (SkyDiscover)

**Strengths:**
- Good diagnostic feedback loop: uses compiler errors to guide next iteration
- Second-best peak (9.82 GB/s) with moderate compile rate (69%)
- Parent selection from best programs means it builds on successes
- Reasonable iteration time (~65s average)

**Weaknesses:**
- 31% compile failure rate still significant
- Occasional regression: iterations 6-9 scored lower than iteration 5
- Doesn't learn from repeated borrow-checker failures (same error pattern recurs)

**When to use:** General-purpose code evolution. Good default choice for single-file targets under ~500 lines.

---

### GEPA (Reflective Pareto)

**Strengths:**
- Most consistent scores: low variance between iterations (5.54-8.50 range, no outliers)
- Good reflection: analyzes WHY programs fail and adjusts strategy
- 87% compile rate (second-best)
- Merge step combines successful patterns from multiple parents

**Weaknesses:**
- Slow iterations (~120s average due to reflection + merge overhead)
- Peak performance (8.50) lower than K-Search or AdaEvolve
- Conservative: reflection may over-avoid risky changes that could yield breakthroughs

**When to use:** When reliability/consistency matters more than peak performance. Multi-objective optimization (Pareto front). When you want to understand trade-offs.

---

### OpenEvolve Native

**Strengths:**
- Fastest wall-clock time (660s for 10 iterations)
- Eventually found a good solution (8.34 GB/s at iteration 10)
- Simple population-based approach with low overhead

**Weaknesses:**
- Slow convergence: best score not found until final iteration
- 34% compile failure rate
- No strong recovery mechanism from failed mutations

**When to use:** Quick exploratory runs where you want many iterations cheaply. Parameter sweeps.

---

### EvoX (Self-Evolving Search)

**Strengths:**
- Interesting meta-evolution concept: evolves the search strategy itself
- Late-stage recovery: found 7.85-8.46 GB/s after many failed iterations

**Weaknesses:**
- Worst compile rate (23%) — catastrophically low
- Spent 80% of time evolving search strategies instead of evaluating programs
- 8 strategy evolutions in 10 iterations = almost no actual program improvement
- Fundamental mismatch: meta-evolution overhead doesn't pay off when the bottleneck is LLM code generation quality, not search strategy

**When to use:** Smooth optimization landscapes (ML hyperparameters, prompt tuning). NOT for compiled code with strict type systems.

---

## Operational Learnings

### 1. Concatenated Multi-File is the Wrong Abstraction

**Problem:** Search frameworks (SkyDiscover) only support single-file in/out. We concatenated 3 files (1019 lines total) with section markers. This forced the LLM to output the ENTIRE concatenated file on every iteration.

**Impact:** 
- At 1019 lines / 38K chars, even Claude Opus frequently truncates
- The critical fix (removing `.lock()` in 5 handler methods at lines 800+) lives in the LAST section — first to get truncated
- Partial modifications create inconsistent state (struct changed, methods not)

**Fix for future:** Either (a) use a framework that supports multi-file natively (Nous), or (b) split the evolution target into truly independent single-file problems where possible.

### 2. Compile Rate is the Dominant Factor

With only 10 iterations and ~40% compile failure rate, frameworks get 6 actual evaluations. Of those, system variance means 2-3 are on the "low" end. You're selecting from 3-4 real data points.

**Rule of thumb:** If your target requires coordinated changes across >500 lines of Rust, expect 30-50% compile failures. Budget iterations accordingly (30+ to get 15-20 real evaluations).

### 3. Direction Doesn't Compensate for Output Constraints

We gave explicit hints: "remove Mutex in service.rs, it's safe because IDispatcher takes &self." Every framework understood the instruction. The issue was never comprehension — it was reliably producing 1000+ lines of coordinated output without dropping changes.

### 4. System Variance Requires Multiple Evaluations

Peak scores vs test re-evaluation showed 15-45% variance. This means:
- A single evaluation is unreliable for ranking solutions
- Best practice: evaluate top-3 candidates 3× each at the end
- The evaluator should restart the server fresh for each measurement

### 5. Nous Experiment Isolation Needs Work

Nous designed proper 3-arm experiments but the execution didn't properly:
- Rebuild between arms (`git checkout -- .` + `cargo build`)
- Verify the server was actually running the intended code
- Separate the worktree state between treatment and control

**Fix:** Add build-hash verification to the benchmark (embed git hash or binary checksum in server output).

### 6. Model Selection Matters Less Than Architecture

All frameworks used the same model (`aws/claude-opus-4-6`). The 4× gap between Nous (22 GB/s) and the best search framework (10.93 GB/s) is entirely due to the framework architecture (multi-file vs concatenated-file), not model capability.

---

## Setup Checklist (For Future Bakeoffs)

1. **Evaluator requirements:**
   - Kill server before each eval (clean state)
   - Wait for port ready (10s timeout)
   - Verify data integrity after each benchmark
   - Restore source files in `finally:` block

2. **Initial program prep:**
   - Keep under 500 lines if possible (for search frameworks)
   - Include ALL code that needs coordinated changes (don't omit handler methods!)
   - Add EVOLVE-BLOCK markers around mutable regions
   - Test that initial program compiles and produces baseline score

3. **Config common settings:**
   - Same model for all frameworks (fair comparison)
   - Same `max_tokens` (16384 for ~1000 lines of Rust)
   - Same evaluator timeout (120s)
   - Same number of iterations (10 minimum)

4. **Running the bakeoff:**
   - Sequential execution (frameworks share the server hardware)
   - tmux session for resilience
   - Kill orphaned server processes between frameworks
   - Save per-iteration JSONL stats (the orchestrator's summary.json is unreliable)

5. **Analysis:**
   - Parse scores from actual log files, not summary.json
   - Re-evaluate winner 3× for confidence
   - Check compile error patterns (common failure modes)
   - Verify winning solution's data integrity

---

## Performance Reference Points

| Configuration | Cold Lookup (8 clients) | Notes |
|---|---|---|
| Baseline (triple Mutex) | ~5-7 GB/s | System state dependent |
| K-Search best (partial opt) | 10.93 GB/s | lib.rs changes only |
| AdaEvolve best | 9.82 GB/s | Partial Mutex removal |
| Nous h-main (full fix) | 22-23 GB/s | Full Mutex removal + drive sharding |
| PCIe Gen4 x16 ceiling | ~25-28 GB/s | Hardware limit for GPU DMA |
| 7× NVMe raw bandwidth | ~37 GB/s | Theoretical (not PCIe-limited) |

---

## Recommendations for Next Steps

1. **Merge Nous's h-main patch** — it's verified correct, passes integrity, and achieves 4.4× improvement. The change is safe (removes unnecessary locking on a `&self` trait).

2. **For future bakeoffs, use Nous for architectural changes** — search frameworks are best for parameter tuning within a single file.

3. **Investigate remaining 2 GB/s gap** (23 vs 25 GB/s) — likely pipeline ring contention when multiple clients hit the same drive (key % 7 collision). Could add more pipeline rings per drive.

4. **Fix the bakeoff orchestrator** — the `summary.json` score parsing is broken (reports 0 when frameworks actually succeeded). Parse from `[runner] Discovery completed` log lines instead.
