#!/usr/bin/env python3
"""Random search baseline: random constant perturbations in pipeline.rs."""
import argparse
import json
import os
import random
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "evaluator"))
from evaluate_p2p import evaluate

INITIAL = Path(__file__).resolve().parent / "initial_programs" / "pipeline.rs"


def random_mutate(code: str) -> str:
    """Randomly perturb numeric constants in the code."""
    def replace_const(match):
        val = int(match.group(0))
        if random.random() < 0.3:  # 30% chance to mutate each constant
            factor = random.choice([0.5, 0.75, 1.5, 2.0, 4.0])
            new_val = max(1, int(val * factor))
            return str(new_val)
        return match.group(0)

    return re.sub(r"\b([1-9]\d{0,5})\b", replace_const, code)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--run-dir", type=str, required=True)
    args = parser.parse_args()

    run_dir = Path(args.run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    scores_file = run_dir / "scores.jsonl"

    seed = INITIAL.read_text()
    best_score = 0.0

    for i in range(args.iterations):
        candidate = random_mutate(seed)
        score, metrics = evaluate(candidate)
        with open(scores_file, "a") as f:
            f.write(json.dumps({"combined_score": round(score, 4), "iteration": i}) + "\n")
        if score > best_score:
            best_score = score
            (run_dir / "best_pipeline.rs").write_text(candidate)
        print(f"  [{i+1}/{args.iterations}] score={score:.4f} best={best_score:.4f}")

    print(f"\nRandom search complete. Best: {best_score:.4f}")


if __name__ == "__main__":
    main()
