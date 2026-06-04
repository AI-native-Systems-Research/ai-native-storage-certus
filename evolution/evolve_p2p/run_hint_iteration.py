#!/usr/bin/env python3
"""Run all search frameworks with implementation hints."""
import json
import shutil
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from run_experiment import run_framework, RESULTS_DIR, EXPERIMENT_DIR

HINT_RESULTS_DIR = EXPERIMENT_DIR / "results_hint"
HINT_CONFIGS_DIR = EXPERIMENT_DIR / "configs_hint"

# Frameworks to run with hints (all search-based, not agentic)
HINT_FRAMEWORKS = ["gepa_native", "adaevolve", "evox", "openevolve", "shinkaevolve", "ksearch"]

ITERATIONS = {
    "ksearch": 15,  # needs more rounds (5 per action cycle)
    "default": 10,
}


def main():
    # Swap configs to hint versions
    original_skydiscover_config = EXPERIMENT_DIR / "configs" / "skydiscover" / "config.yaml"
    hint_skydiscover_config = HINT_CONFIGS_DIR / "skydiscover" / "config.yaml"

    # Backup original config
    backup = original_skydiscover_config.with_suffix(".yaml.nohint")
    if not backup.exists():
        shutil.copy2(original_skydiscover_config, backup)

    # Use hint config
    shutil.copy2(hint_skydiscover_config, original_skydiscover_config)

    # For K-Search: swap DEFINITION_TEXT (done via env var read in the task)
    os.environ["KSEARCH_DEFINITION_FILE"] = str(HINT_CONFIGS_DIR / "ksearch_definition.txt")

    # For GEPA: swap background (done via env var)
    os.environ["GEPA_BACKGROUND_FILE"] = str(HINT_CONFIGS_DIR / "gepa_background.txt")

    # Override RESULTS_DIR to results_hint/
    import run_experiment
    run_experiment.RESULTS_DIR = HINT_RESULTS_DIR
    HINT_RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    print(f"{'='*70}")
    print(f"  P2P Evolution — HINT ITERATION")
    print(f"{'='*70}")
    print(f"  Frameworks: {HINT_FRAMEWORKS}")
    print(f"  Results: {HINT_RESULTS_DIR}")
    print(f"  Hints: {HINT_CONFIGS_DIR / 'HINTS.md'}")
    print()

    summaries = []
    for fw in HINT_FRAMEWORKS:
        iters = ITERATIONS.get(fw, ITERATIONS["default"])
        print(f"\n=== Running {fw} (hint, {iters} iterations) ===")

        # Clear previous hint results for this framework
        fw_dir = HINT_RESULTS_DIR / fw
        if fw_dir.exists():
            shutil.rmtree(fw_dir)
        fw_dir.mkdir(parents=True, exist_ok=True)

        r = run_framework(fw, iters)
        summaries.append(r)
        print(json.dumps(r, indent=2))

    # Restore original config
    if backup.exists():
        shutil.copy2(backup, original_skydiscover_config)

    # Save summary
    (HINT_RESULTS_DIR / "experiment_summary.json").write_text(json.dumps(summaries, indent=2))

    print(f"\n{'='*70}")
    print("  HINT ITERATION COMPLETE")
    print(f"{'='*70}")
    for s in summaries:
        print(f"  {s['framework']:<16} best={s['best_score']:.4f} wall={s['wall_time_seconds']:.0f}s")


if __name__ == "__main__":
    main()
