#!/usr/bin/env python3
"""K-Search runner for Certus P2P evolution.

Uses K-Search's world-model-guided kernel generator loop with our custom
CertusP2PTask adapter.
"""
import argparse
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path("/home/nara/certus/evo_frameworks/K-Search")))

from k_search.tasks.certus_p2p_task import CertusP2PTask
from k_search.kernel_generators.kernel_generator_world_model import WorldModelKernelGeneratorWithBaseline


def main():
    parser = argparse.ArgumentParser(description="K-Search P2P evolution runner")
    parser.add_argument("--iterations", type=int, default=10)
    parser.add_argument("--run-dir", type=str, required=True)
    parser.add_argument("--model", type=str, default="openai/aws/claude-opus-4-6")
    parser.add_argument("--api-base", type=str, default="")
    parser.add_argument("--api-key", type=str, default="")
    args = parser.parse_args()

    run_dir = Path(args.run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    scores_file = run_dir / "scores.jsonl"

    # Set up OpenAI-compatible env for K-Search
    if args.api_base:
        os.environ["OPENAI_BASE_URL"] = args.api_base
    if args.api_key:
        os.environ["OPENAI_API_KEY"] = args.api_key
        os.environ["LLM_API_KEY"] = args.api_key

    task = CertusP2PTask(
        timeout_seconds=120,
        artifacts_dir=str(run_dir / "artifacts"),
    )

    # Wrap task.run_benchmark to capture per-evaluation scores and save candidate source
    original_run_benchmark = task.run_benchmark
    eval_count = [0]
    best_score = [0.0]
    candidates_dir = run_dir / "candidates"
    candidates_dir.mkdir(parents=True, exist_ok=True)

    def instrumented_run_benchmark(**kwargs):
        solution = kwargs.get("solution")

        # Save candidate source files regardless of outcome
        if solution and hasattr(solution, "sources") and solution.sources:
            eval_idx = eval_count[0]
            gen_dir = candidates_dir / f"gen_{eval_idx}"
            gen_dir.mkdir(parents=True, exist_ok=True)
            parts = []
            for sf in solution.sources:
                (gen_dir / sf.path).write_text(sf.content or "")
                parts.append(f"// --- FILE: {sf.path} ---\n{sf.content or ''}")
            (gen_dir / "main.rs").write_text("\n\n".join(parts))

        result = original_run_benchmark(**kwargs)
        score = result.metrics.get("score", 0.0) if result.metrics else 0.0
        is_passed = getattr(result, "status", "") == "passed"

        entry = {
            "combined_score": round(float(score), 4),
            "iteration": eval_count[0],
            "throughput_gbps": result.metrics.get("throughput_gbps") if result.metrics else None,
            "p99_latency_ms": result.metrics.get("p99_latency_ms") if result.metrics else None,
            "p50_latency_ms": result.metrics.get("p50_latency_ms") if result.metrics else None,
            "mean_latency_ms": result.metrics.get("mean_latency_ms") if result.metrics else None,
            "cpu_util_fraction": result.metrics.get("cpu_util_fraction") if result.metrics else None,
            "build_succeeded": is_passed or (result.metrics.get("build_succeeded", False) if result.metrics else False),
            "data_integrity": result.metrics.get("data_integrity", True) if result.metrics else True,
            "failure_type": "success" if is_passed else "build_failure",
            "error": (result.log_excerpt or "")[:200] if not is_passed else "",
        }
        with open(scores_file, "a") as f:
            f.write(json.dumps(entry) + "\n")

        if score > best_score[0]:
            best_score[0] = score

        eval_count[0] += 1
        print(f"  [K-Search eval {eval_count[0]}] "
              f"score={score:.4f} best={best_score[0]:.4f} "
              f"status={result.status}")
        return result

    task.run_benchmark = instrumented_run_benchmark

    generator = WorldModelKernelGeneratorWithBaseline(
        model_name=args.model,
        language="triton",
        target_gpu="A30",
        artifacts_dir=str(run_dir / "artifacts"),
    )

    print(f"Starting K-Search world-model evolution ({args.iterations} rounds)...")
    try:
        best_solution = generator.generate(
            task=task,
            max_opt_rounds=args.iterations,
            continue_from_solution="wild_type",
        )
        print(f"\nK-Search complete. Best score: {best_score[0]:.4f} "
              f"({eval_count[0]} evaluations)")
    except Exception as e:
        print(f"\nK-Search failed: {e}")
        if eval_count[0] == 0:
            entry = {
                "combined_score": 0.0,
                "iteration": 0,
                "build_succeeded": False,
                "failure_type": "other_failure",
                "error": str(e)[:200],
            }
            with open(scores_file, "a") as f:
                f.write(json.dumps(entry) + "\n")


if __name__ == "__main__":
    main()
