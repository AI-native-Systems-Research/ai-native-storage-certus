#!/usr/bin/env python3.12
"""Coding Agent SDK — continuous session with supervisor intervention.

Uses the Claude Agent SDK with session resume for:
- One continuous session (agent maintains full context)
- Programmatic stagnation detection between turns
- Supervisor intervention when gains are marginal (<1% improvement for N iterations)
- Exact cost/token tracking per turn

Usage:
    python3.12 run_coding_agent_sdk.py --iterations 12 --stagnation-threshold 3
    python3.12 run_coding_agent_sdk.py --iterations 12 --min-improvement 0.01
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import sys
import time
from pathlib import Path

from claude_agent_sdk import query, ClaudeAgentOptions, HookMatcher

EXPERIMENT_DIR = Path(__file__).resolve().parent
REPO_ROOT = EXPERIMENT_DIR.parents[1]
EVALUATOR_PY = EXPERIMENT_DIR / "evaluator" / "evaluate_p2p.py"
RESULTS_DIR = EXPERIMENT_DIR / "results"

# Minimum score improvement to count as "real progress" (not marginal noise)
DEFAULT_MIN_IMPROVEMENT = 0.01


def build_initial_prompt(repo_root: Path, run_dir: Path) -> str:
    """Build the optimization prompt (same context as other frameworks)."""
    evaluator_path = repo_root / "evolution" / "evolve_p2p" / "evaluator" / "evaluate_p2p.py"

    return (
        "You are a performance optimization agent. Maximize the cold-lookup "
        "SSD-to-GPU data transfer score for certus-server.\n\n"
        "## Scoring\n"
        "score = 0.60 * (throughput_gbps / 12.0) + 0.40 * (0.4 / p99_ms)\n"
        "Baseline: ~0.20 (2.4 GB/s, 1.9ms p99).\n\n"
        "## Hardware\n"
        "- NVMe Gen4 SSD via SPDK userspace driver\n"
        "- NVIDIA A30 GPU, PCIe Gen4 x16\n"
        "- Kernel modules: nvidia-peermem, gdrdrv\n"
        "- 2048 hugepages, memlock unlimited, VFIO-bound NVMe\n\n"
        "## Evaluation\n"
        f"- Build: cargo build -p certus-server --release --features p2p\n"
        f"- Evaluate: python3 {evaluator_path} --test\n"
        "  (this builds, starts server, benchmarks, checks integrity, prints score)\n"
        "- The evaluator scores whatever is currently in the source files\n\n"
        "## Files in scope\n"
        "- components/dispatcher/src/pipeline.rs\n"
        "- components/dispatcher/src/lib.rs\n"
        "- components/gpu-services/src/dma.rs\n\n"
        "## Your approach\n"
        "Work iteratively. Each cycle:\n"
        "1. Read source files, identify a bottleneck\n"
        "2. Make ONE focused change\n"
        f"3. Save candidate to {run_dir}/candidates/gen_N/ (N = iteration number)\n"
        "4. Run the evaluator\n"
        "5. Report result as JSON on its own line:\n"
        '   {"iteration": N, "score": X, "throughput_gbps": Y, "p99_ms": Z, '
        '"change": "description", "reasoning": "why"}\n\n'
        "Do as many iterations as you can. After each evaluation, immediately "
        "start the next iteration. Do not stop to ask — keep optimizing.\n\n"
        "## Constraints\n"
        "- Code must compile with --features p2p\n"
        "- Data integrity must pass (no ERRORS in benchmark output)\n"
        "- Do not modify gRPC service, benchmark client, or interfaces\n"
    )


def build_supervisor_prompt(
    scores: list[float],
    changes: list[str],
    stagnation_count: int,
    best_score: float,
) -> str:
    """Supervisor intervention — injected into the session when stagnating."""
    recent = scores[-5:] if len(scores) >= 5 else scores

    return (
        f"## Supervisor Intervention\n\n"
        f"Your last {stagnation_count} iterations produced less than 1% improvement each. "
        f"You're in a local optimum.\n\n"
        f"**Current best**: {best_score:.4f}\n"
        f"**Recent scores**: {[round(s, 4) for s in recent]}\n"
        f"**Recent changes**: {'; '.join(changes[-stagnation_count:])}\n\n"
        f"**Step back and think structurally:**\n"
        f"1. Is there a structural limitation in your current approach? "
        f"Are you only tuning parameters when the architecture itself is the constraint?\n"
        f"2. What does the hardware ceiling (~5.9 GB/s per drive at QD64) suggest "
        f"about where throughput is being left on the table?\n"
        f"3. Are there files in scope you haven't read or fully explored yet?\n"
        f"4. What would a 2x improvement require — not parameter tuning, "
        f"but a fundamentally different approach to moving data?\n\n"
        f"Make your next change based on this reflection. Think bigger.\n"
        f"Report result as JSON as before.\n"
    )


def parse_scores_from_text(text: str) -> list[dict]:
    """Extract all iteration result JSON objects from agent output."""
    results = []
    pattern = r'\{[^{}]*"iteration"\s*:\s*\d+[^{}]*"score"\s*:\s*[\d.]+[^{}]*\}'
    for match in re.finditer(pattern, text):
        try:
            data = json.loads(match.group())
            if "score" in data and "iteration" in data:
                results.append(data)
        except json.JSONDecodeError:
            continue
    return results


def make_eval_capture_hook(scores_file: Path, candidates_dir: Path, run_dir: Path):
    """Create a PostToolUse hook that captures evaluator scores and edits in real-time."""
    eval_count = [0]  # mutable counter in closure
    recent_edits = []  # track file edits between evaluations
    iteration_log = run_dir / "iterations.jsonl"

    async def capture_eval_output(input_data, tool_use_id, context):
        """Capture evaluator output from Bash tool results, and track edits."""
        tool_input = input_data.get("tool_input", {})
        tool_result = input_data.get("tool_result", "")
        tool_name = input_data.get("tool_name", "")

        # Track file edits between evaluations
        if tool_name == "Edit" and isinstance(tool_input, dict):
            file_path = tool_input.get("file_path", "")
            if file_path and any(f in file_path for f in ["pipeline.rs", "lib.rs", "dma.rs"]):
                old_str = tool_input.get("old_string", "")[:80]
                new_str = tool_input.get("new_string", "")[:80]
                recent_edits.append(f"{Path(file_path).name}: {old_str[:30]}→{new_str[:30]}")

        if not isinstance(tool_result, str):
            return {}

        # Look for evaluator output pattern
        fitness_match = re.search(r"Fitness:\s*([\d.]+)", tool_result)
        score_match = re.search(r"Score:\s*([\d.]+)\s*\|\s*Throughput:\s*([\d.]+)\s*GB/s\s*\|\s*p99:\s*(\d+)us", tool_result)

        score = None
        throughput = None
        p99_ms = None

        if score_match:
            score = float(score_match.group(1))
            throughput = float(score_match.group(2))
            p99_ms = float(score_match.group(3)) / 1000.0
        elif fitness_match:
            score = float(fitness_match.group(1))

        if score is not None:
            eval_count[0] += 1
            change_desc = "; ".join(recent_edits[-5:]) if recent_edits else "unknown"

            entry = {
                "combined_score": round(score, 4),
                "iteration": eval_count[0] - 1,
                "throughput_gbps": throughput,
                "p99_latency_ms": p99_ms,
                "build_succeeded": True,
                "failure_type": "success",
                "error": "",
                "change": change_desc,
            }

            with open(scores_file, "a") as f:
                f.write(json.dumps(entry) + "\n")

            # Also write detailed iteration log
            with open(iteration_log, "a") as f:
                f.write(json.dumps({
                    "eval_num": eval_count[0],
                    "score": round(score, 4),
                    "throughput_gbps": throughput,
                    "p99_ms": p99_ms,
                    "edits_since_last_eval": list(recent_edits),
                    "timestamp": time.strftime("%H:%M:%S"),
                }) + "\n")

            if throughput:
                print(f"    [EVAL #{eval_count[0]}] Score: {score:.4f} | "
                      f"{throughput:.2f} GB/s | p99: {p99_ms:.2f}ms | "
                      f"changes: {len(recent_edits)}", flush=True)
            else:
                print(f"    [EVAL #{eval_count[0]}] Fitness: {score:.4f} | "
                      f"changes: {len(recent_edits)}", flush=True)

            recent_edits.clear()

        return {}

    return capture_eval_output, eval_count


async def run_session(
    prompt: str,
    cwd: str,
    session_id: str | None = None,
    max_turns: int = 40,
    hooks: dict | None = None,
) -> tuple[str, str | None, dict]:
    """Run a session turn. Returns (result_text, session_id, usage)."""
    kwargs = {
        "allowed_tools": ["Read", "Edit", "Write", "Bash", "Glob", "Grep"],
        "cwd": cwd,
        "permission_mode": "bypassPermissions",
        "max_turns": max_turns,
    }
    if session_id:
        kwargs["resume"] = session_id
    if hooks:
        kwargs["hooks"] = hooks

    options = ClaudeAgentOptions(**kwargs)

    result_text = ""
    new_session_id = None
    usage = {}

    try:
        async for message in query(prompt=prompt, options=options):
            if hasattr(message, "session_id") and message.session_id:
                new_session_id = message.session_id
            if hasattr(message, "type") and message.type == "system":
                if hasattr(message, "data") and isinstance(message.data, dict):
                    sid = message.data.get("session_id")
                    if sid:
                        new_session_id = sid
            if hasattr(message, "result"):
                result_text = message.result
            if hasattr(message, "total_cost_usd"):
                usage["total_cost_usd"] = message.total_cost_usd
            if hasattr(message, "usage") and message.usage:
                try:
                    usage["input_tokens"] = getattr(message.usage, "input_tokens", 0)
                    usage["output_tokens"] = getattr(message.usage, "output_tokens", 0)
                except Exception:
                    pass
    except Exception as e:
        err_msg = str(e)
        # "max turns" is expected — agent was working, just hit the limit
        if "maximum number of turns" in err_msg.lower() or "max" in err_msg.lower():
            print(f"  [MAX TURNS] Agent hit turn limit — extracting partial results")
        else:
            raise

    return result_text, new_session_id, usage


async def main():
    parser = argparse.ArgumentParser(description="Coding Agent SDK — continuous session optimization")
    parser.add_argument("--iterations", type=int, default=12,
                        help="Max optimization iterations (agent may do multiple per turn)")
    parser.add_argument("--run-dir", type=str, default=None)
    parser.add_argument("--stagnation-threshold", type=int, default=3,
                        help="Iterations with <min-improvement before supervisor intervenes")
    parser.add_argument("--min-improvement", type=float, default=DEFAULT_MIN_IMPROVEMENT,
                        help="Minimum score improvement to count as real progress (default: 0.01)")
    parser.add_argument("--timeout", type=int, default=3600,
                        help="Overall wall-time timeout in seconds (default: 3600 = 1 hour)")
    parser.add_argument("--repo-root", type=str, default=None)
    parser.add_argument("--max-turns-per-session", type=int, default=40,
                        help="Max tool-use turns per SDK query call (lower = more supervisor checks)")
    args = parser.parse_args()

    repo_root = Path(args.repo_root) if args.repo_root else REPO_ROOT
    run_dir = Path(args.run_dir) if args.run_dir else RESULTS_DIR / "coding_agent_sdk"
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "candidates").mkdir(parents=True, exist_ok=True)

    cwd = str(repo_root)

    # State
    all_results: list[dict] = []
    session_id: str | None = None
    best_score = 0.0
    stagnation_count = 0
    total_cost = 0.0
    supervisor_interventions = 0

    scores_file = run_dir / "scores.jsonl"

    t_start = time.time()
    print(f"{'='*60}")
    print(f"  Coding Agent SDK — Continuous Session")
    print(f"  Repo: {repo_root}")
    print(f"  Results: {run_dir}")
    print(f"  Stagnation: {args.stagnation_threshold} iters × <{args.min_improvement} improvement")
    print(f"  Timeout: {args.timeout/60:.0f}m")
    print(f"  Max turns per call: {args.max_turns_per_session}")
    print(f"{'='*60}\n")

    # Set up real-time eval capture hook
    capture_hook, eval_counter = make_eval_capture_hook(scores_file, run_dir / "candidates", run_dir)
    sdk_hooks = {
        "PostToolUse": [
            HookMatcher(matcher="Bash|Edit", hooks=[capture_hook])
        ]
    }

    # Phase 1: Initial session — let agent run freely
    print("--- Phase: Initial optimization ---")
    prompt = build_initial_prompt(repo_root, run_dir)

    result_text, session_id, usage = await run_session(
        prompt=prompt,
        cwd=cwd,
        session_id=None,
        max_turns=args.max_turns_per_session,
        hooks=sdk_hooks,
    )

    iter_cost = usage.get("total_cost_usd", 0) or 0
    total_cost += iter_cost

    # Parse all iteration results from this turn
    new_results = parse_scores_from_text(result_text)
    for r in new_results:
        all_results.append(r)
        score = r.get("score", 0)
        change = r.get("change", "unknown")

        # Track stagnation (with minimum improvement threshold)
        if score > best_score + args.min_improvement:
            best_score = score
            stagnation_count = 0
        elif score > 0:
            stagnation_count += 1

        # Write to scores.jsonl
        with open(scores_file, "a") as f:
            f.write(json.dumps({
                "combined_score": round(score, 4),
                "iteration": r.get("iteration", len(all_results) - 1),
                "throughput_gbps": r.get("throughput_gbps"),
                "p99_latency_ms": r.get("p99_ms"),
                "build_succeeded": score > 0,
                "failure_type": "success" if score > 0 else "build_failure",
                "error": "",
                "change": change,
            }) + "\n")

    print(f"  Initial phase: {len(new_results)} iterations, "
          f"best={best_score:.4f}, cost=${iter_cost:.2f}")
    for r in new_results:
        print(f"    iter {r.get('iteration')}: {r.get('score', 0):.4f} — {r.get('change', '?')}")

    # Phase 2: Supervisor intervention loop
    while len(all_results) < args.iterations:
        elapsed = time.time() - t_start
        if elapsed > args.timeout:
            print(f"\n  [TIMEOUT] {elapsed/60:.1f}m elapsed")
            break

        # Check if supervisor should intervene
        if stagnation_count >= args.stagnation_threshold:
            supervisor_interventions += 1
            changes = [r.get("change", "?") for r in all_results]
            scores = [r.get("score", 0) for r in all_results]
            print(f"\n--- Supervisor Intervention #{supervisor_interventions} "
                  f"(stagnation={stagnation_count}, best={best_score:.4f}) ---")

            prompt = build_supervisor_prompt(
                scores=scores,
                changes=changes,
                stagnation_count=stagnation_count,
                best_score=best_score,
            )
            stagnation_count = 0
        else:
            # Continue — tell agent to keep going
            prompt = (
                "Continue optimizing. You're making progress. "
                "Try a different approach or tune further. "
                "Report each result as JSON.\n"
            )

        result_text, session_id, usage = await run_session(
            prompt=prompt,
            cwd=cwd,
            session_id=session_id,
            max_turns=args.max_turns_per_session,
            hooks=sdk_hooks,
        )

        iter_cost = usage.get("total_cost_usd", 0) or 0
        total_cost += iter_cost

        new_results = parse_scores_from_text(result_text)
        if not new_results:
            print(f"  [WARN] No parseable results from this turn")
            (run_dir / f"raw_turn_{len(all_results)}.txt").write_text(result_text[:5000])
            stagnation_count += 1
            if stagnation_count > args.stagnation_threshold * 2:
                print("  [STOP] Giving up after repeated failures")
                break
            continue

        for r in new_results:
            all_results.append(r)
            score = r.get("score", 0)
            change = r.get("change", "unknown")

            if score > best_score + args.min_improvement:
                best_score = score
                stagnation_count = 0
            elif score > 0:
                stagnation_count += 1

            with open(scores_file, "a") as f:
                f.write(json.dumps({
                    "combined_score": round(score, 4),
                    "iteration": r.get("iteration", len(all_results) - 1),
                    "throughput_gbps": r.get("throughput_gbps"),
                    "p99_latency_ms": r.get("p99_ms"),
                    "build_succeeded": score > 0,
                    "failure_type": "success" if score > 0 else "build_failure",
                    "error": "",
                    "change": change,
                }) + "\n")

        print(f"  Turn: {len(new_results)} iterations, best={best_score:.4f}, "
              f"stagnation={stagnation_count}, cost=${iter_cost:.2f}")
        for r in new_results:
            print(f"    iter {r.get('iteration')}: {r.get('score', 0):.4f} — {r.get('change', '?')}")

    # Final summary
    wall_time = time.time() - t_start
    scores_list = [r.get("score", 0) for r in all_results]

    summary = {
        "framework": "coding_agent_sdk",
        "iterations_completed": len(all_results),
        "iterations_requested": args.iterations,
        "wall_time_seconds": round(wall_time, 1),
        "best_score": round(best_score, 4),
        "mean_score": round(sum(scores_list) / len(scores_list), 4) if scores_list else 0.0,
        "scores": [round(s, 4) for s in scores_list],
        "supervisor_interventions": supervisor_interventions,
        "returncode": 0,
        "timestamp": time.strftime("%Y-%m-%d_%H-%M-%S"),
    }
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2))
    (run_dir / "cost.json").write_text(json.dumps({
        "total_cost_usd": round(total_cost, 4),
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "supervisor_interventions": supervisor_interventions,
    }))
    (run_dir / "knowledge.json").write_text(json.dumps({
        "all_results": all_results,
        "supervisor_interventions": supervisor_interventions,
        "best_score": best_score,
    }, indent=2))

    print(f"\n{'='*60}")
    print(f"  COMPLETE")
    print(f"  Best: {best_score:.4f} | Iterations: {len(all_results)} | "
          f"Wall: {wall_time/60:.1f}m | Cost: ${total_cost:.2f}")
    print(f"  Supervisor interventions: {supervisor_interventions}")
    print(f"{'='*60}")


if __name__ == "__main__":
    asyncio.run(main())
