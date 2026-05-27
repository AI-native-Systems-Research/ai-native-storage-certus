#!/usr/bin/env python3
"""Autonomous pipeline bakeoff orchestrator.

Runs all frameworks sequentially against the same evaluator.
No human gates — each framework runs, results are logged, analysis is written,
then the next framework starts.

Usage:
    python run_bakeoff.py [--iterations 50] [--eval fixed] [--frameworks all]

Frameworks run via SkyDiscover (single unified CLI):
  adaevolve, evox, claude_code, gepa_native, openevolve_native, shinkaevolve

Standalone frameworks:
  ksearch  — K-Search (custom wrapper, uses generate_kernels_and_eval.py)
  nous     — Nous campaign runner (run_campaign.py --auto-approve)
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]  # ai-native-storage-certus/
BAKEOFF_DIR = Path(__file__).resolve().parent
EVALUATOR_DIR = BAKEOFF_DIR / "evaluator"
EVALUATOR_PY = EVALUATOR_DIR / "evaluate.py"

HYPOTHESIS_DIRS = {
    "fixed": BAKEOFF_DIR / "hypothesis_1",
    "mixed": BAKEOFF_DIR / "hypothesis_2",
    "concurrent": BAKEOFF_DIR / "hypothesis_3",
}


def _get_hypothesis_dir(eval_mode: str) -> Path:
    return HYPOTHESIS_DIRS.get(eval_mode, BAKEOFF_DIR / "hypothesis_1")


def _get_results_dir(eval_mode: str) -> Path:
    return _get_hypothesis_dir(eval_mode) / "results"


def _get_configs_dir(eval_mode: str) -> Path:
    return _get_hypothesis_dir(eval_mode) / "configs"


def _get_initial_program(eval_mode: str, framework: str = "") -> Path:
    """Get initial program path. Nous uses multi-file dir for H3; others use concatenated single file."""
    h_dir = _get_hypothesis_dir(eval_mode)
    if eval_mode == "concurrent" and framework == "nous":
        dir_path = h_dir / "initial_program_dir"
        if dir_path.exists():
            return dir_path
    return h_dir / "initial_program.rs"

EVO_FRAMEWORKS_ROOT = Path("/home/nara/certus/evo_frameworks")
SKYDISCOVER_ROOT = EVO_FRAMEWORKS_ROOT / "skydiscover"
NOUS_ROOT = EVO_FRAMEWORKS_ROOT / "agentic-strategy-evolution"

# Frameworks that run via SkyDiscover's --search flag
SKYDISCOVER_FRAMEWORKS = [
    "adaevolve",
    "evox",
    "claude_code",
    "gepa_native",
    "openevolve_native",
]

# All frameworks in run order
ALL_FRAMEWORKS = SKYDISCOVER_FRAMEWORKS + ["ksearch", "nous"]


def timestamp():
    return datetime.now().strftime("%Y-%m-%d_%H-%M-%S")


def run_framework(name: str, iterations: int, eval_mode: str) -> dict:
    """Run a single framework and return results summary."""
    run_dir = _get_results_dir(eval_mode) / name
    run_dir.mkdir(parents=True, exist_ok=True)
    log_file = run_dir / f"run-{timestamp()}.log"

    print(f"\n{'='*70}")
    print(f"  Starting: {name} | iterations={iterations} | eval={eval_mode}")
    print(f"  Output: {run_dir}")
    print(f"{'='*70}\n")

    t_start = time.time()

    if name in SKYDISCOVER_FRAMEWORKS:
        cmd = _build_skydiscover_cmd(name, iterations, eval_mode, run_dir)
    elif name == "ksearch":
        cmd = _build_ksearch_cmd(iterations, eval_mode, run_dir)
    elif name == "nous":
        cmd = _build_nous_cmd(iterations, eval_mode, run_dir)
    else:
        return {"framework": name, "error": f"Unknown framework: {name}"}

    print(f"  CMD: {' '.join(str(c) for c in cmd)}\n")

    # Run framework
    # Nous is deep (3 iters × 3 arms × build+bench each) — needs more time
    timeout_s = 7200 if name == "nous" else iterations * 180

    # Pass eval mode to evaluator via environment variable
    env = os.environ.copy()
    env["BAKEOFF_EVAL_MODE"] = eval_mode

    try:
        with open(log_file, "w") as log:
            result = subprocess.run(
                cmd,
                stdout=log,
                stderr=subprocess.STDOUT,
                timeout=timeout_s,
                cwd=_get_cwd(name),
                env=env,
            )
        returncode = result.returncode
    except subprocess.TimeoutExpired:
        returncode = -1
        with open(log_file, "a") as log:
            log.write(f"\nTIMEOUT after {timeout_s}s\n")
    except Exception as e:
        returncode = -2
        with open(log_file, "a") as log:
            log.write(f"\nERROR: {e}\n")

    t_end = time.time()
    wall_time = t_end - t_start

    # Parse scores from SkyDiscover output or scores.jsonl
    scores = _parse_scores(run_dir, name)

    summary = {
        "framework": name,
        "iterations_completed": len(scores),
        "iterations_requested": iterations,
        "wall_time_seconds": round(wall_time, 1),
        "best_score": round(max(scores), 4) if scores else 0.0,
        "mean_score": round(sum(scores) / len(scores), 4) if scores else 0.0,
        "scores": scores,
        "returncode": returncode,
        "eval_mode": eval_mode,
        "timestamp": timestamp(),
        "log_file": str(log_file),
    }

    # Write per-framework summary
    summary_file = run_dir / "summary.json"
    summary_file.write_text(json.dumps(summary, indent=2))

    print(f"\n  {name} complete: {len(scores)} iterations, "
          f"best={summary['best_score']:.4f} GB/s, wall={wall_time:.0f}s\n")
    return summary


def _build_skydiscover_cmd(search_algo: str, iterations: int, eval_mode: str,
                           run_dir: Path) -> list[str]:
    """Build SkyDiscover CLI command.

    All algorithms use the same CLI: skydiscover-run <program> <evaluator> --search <algo>
    """
    # Select config file from hypothesis configs dir
    configs_dir = _get_configs_dir(eval_mode)
    if eval_mode == "concurrent":
        config_file = configs_dir / search_algo / "config-concurrent.yaml"
        if not config_file.exists():
            config_file = configs_dir / search_algo / "config.yaml"
    elif eval_mode == "mixed":
        config_file = configs_dir / search_algo / "config-mixed.yaml"
        if not config_file.exists():
            config_file = configs_dir / search_algo / "config.yaml"
    else:
        config_file = configs_dir / search_algo / "config.yaml"

    # Select initial program from hypothesis dir
    initial_program = _get_initial_program(eval_mode)

    # claude_code mode is deeper per iteration (multi-file), cap at 10
    iters = min(iterations, 10) if search_algo == "claude_code" else iterations

    cmd = [
        "uv", "run", "--project", str(SKYDISCOVER_ROOT),
        "skydiscover-run",
        str(initial_program),
        str(EVALUATOR_PY),
        "--search", search_algo,
        "--config", str(config_file),
        "-i", str(iters),
        "-o", str(run_dir / "output"),
    ]

    return cmd


def _build_ksearch_cmd(iterations: int, eval_mode: str, run_dir: Path) -> list[str]:
    """Build K-Search command via SkyDiscover.

    K-Search doesn't have a SkyDiscover --search mode, so we run it as
    adaevolve with the K-Search-specific config (world-model reasoning prompt).
    The config's system_message includes K-Search's reasoning approach.
    """
    configs_dir = _get_configs_dir(eval_mode)
    if eval_mode == "concurrent":
        config_file = configs_dir / "ksearch" / "config-concurrent.yaml"
    else:
        config_file = configs_dir / "ksearch" / "config.yaml"

    initial_program = _get_initial_program(eval_mode)

    cmd = [
        "uv", "run", "--project", str(SKYDISCOVER_ROOT),
        "skydiscover-run",
        str(initial_program),
        str(EVALUATOR_PY),
        "--search", "adaevolve",
        "--config", str(config_file),
        "-i", str(iterations),
        "-o", str(run_dir / "output"),
    ]

    return cmd


def _build_nous_cmd(iterations: int, eval_mode: str, run_dir: Path) -> list[str]:
    """Build Nous campaign command."""
    configs_dir = _get_configs_dir(eval_mode)
    if eval_mode != "fixed":
        config_file = configs_dir / "nous" / f"config-{eval_mode}.yaml"
        if not config_file.exists():
            config_file = configs_dir / "nous" / "config.yaml"
    else:
        config_file = configs_dir / "nous" / "config.yaml"

    run_prefix_map = {"mixed": "h2-bakeoff", "concurrent": "h3-bakeoff", "fixed": "h1-bakeoff"}
    run_prefix = run_prefix_map.get(eval_mode, "h1-bakeoff")
    # H3 is deeper (multi-file, architectural) — needs more time
    timeout = "7200" if eval_mode == "concurrent" else ("3600" if eval_mode == "mixed" else "1800")

    cmd = [
        "/usr/bin/python3.12",
        str(NOUS_ROOT / "run_campaign.py"),
        str(config_file),
        "--max-iterations", str(min(iterations, 3)),  # Nous uses deep iterations (design->execute->analyze)
        "--auto-approve",
        "--run-id", f"{run_prefix}-{timestamp()}",
        "--timeout", timeout,
    ]

    return cmd


def _get_cwd(name: str):
    """Get working directory for framework. Most run from REPO_ROOT, Nous needs its own dir."""
    if name == "nous":
        return NOUS_ROOT
    return None


def _parse_scores(run_dir: Path, framework: str) -> list[float]:
    """Parse scores from framework output."""
    scores = []

    # Check SkyDiscover output (it writes results to output/best_programs.json etc)
    output_dir = run_dir / "output"
    if output_dir.exists():
        # SkyDiscover writes iteration results
        for jsonl_file in output_dir.rglob("*.jsonl"):
            for line in jsonl_file.read_text().splitlines():
                try:
                    entry = json.loads(line)
                    score = entry.get("combined_score") or entry.get("score", 0.0)
                    if score > 0:
                        scores.append(float(score))
                except (json.JSONDecodeError, TypeError, ValueError):
                    continue

        # Also check scores.json or history files
        for json_file in output_dir.rglob("*history*.json"):
            try:
                data = json.loads(json_file.read_text())
                if isinstance(data, list):
                    for entry in data:
                        score = entry.get("combined_score") or entry.get("score", 0.0)
                        if score > 0:
                            scores.append(float(score))
            except (json.JSONDecodeError, TypeError, ValueError):
                continue

    # Check scores.jsonl (our format)
    scores_file = run_dir / "scores.jsonl"
    if scores_file.exists():
        for line in scores_file.read_text().splitlines():
            try:
                entry = json.loads(line)
                score = entry.get("combined_score", 0.0)
                if score > 0:
                    scores.append(float(score))
            except (json.JSONDecodeError, TypeError, ValueError):
                continue

    # Nous: parse from iteration outputs
    if framework == "nous":
        for md_file in run_dir.rglob("*.md"):
            content = md_file.read_text()
            # Look for throughput numbers in Nous reports
            import re
            for m in re.finditer(r"(\d+\.?\d*)\s*GB/s", content):
                val = float(m.group(1))
                if 0.5 < val < 30:  # reasonable range
                    scores.append(val)

    return scores


def write_analysis(summaries: list[dict], eval_mode: str):
    """Write comparative analysis markdown."""
    analysis_file = _get_results_dir(eval_mode) / f"analysis-{eval_mode}-{timestamp()}.md"

    lines = [
        f"# Pipeline Bakeoff Results — {eval_mode} evaluator",
        f"",
        f"Date: {datetime.now().strftime('%Y-%m-%d %H:%M')}",
        f"Evaluator mode: {eval_mode}",
        f"Baseline (current dispatcher, single drive, 4 MiB): **3.59 GB/s**",
        f"P2P reference (gpu-bb-vs-p2p): **3.4 GB/s**",
        f"Raw NVMe ceiling (QD=32): **5.28 GB/s**",
        f"",
        f"## Summary Table",
        f"",
        f"| # | Framework | Iters | Best (GB/s) | Mean (GB/s) | vs Baseline | Wall Time | Status |",
        f"|---|-----------|-------|-------------|-------------|-------------|-----------|--------|",
    ]

    baseline = 3.59
    for i, s in enumerate(sorted(summaries, key=lambda x: x.get("best_score", 0), reverse=True), 1):
        status = "OK" if s.get("returncode", -1) == 0 else f"exit={s.get('returncode', '?')}"
        improvement = ((s['best_score'] / baseline) - 1) * 100 if s['best_score'] > 0 else 0
        sign = "+" if improvement >= 0 else ""
        lines.append(
            f"| {i} | {s['framework']:<16} | {s['iterations_completed']:>3}/{s['iterations_requested']:<3} "
            f"| {s['best_score']:>8.4f}    | {s['mean_score']:>8.4f}    "
            f"| {sign}{improvement:>5.1f}%     "
            f"| {s['wall_time_seconds']:>7.0f}s  | {status} |"
        )

    lines.extend([
        "",
        "## Sample Efficiency (iterations to reach 90% of best)",
        "",
    ])

    for s in summaries:
        scores = s.get("scores", [])
        if scores:
            best = max(scores)
            threshold = 0.9 * best
            first_90 = next((i for i, sc in enumerate(scores) if sc >= threshold), len(scores))
            lines.append(f"- **{s['framework']}**: best={best:.4f} GB/s at iter {scores.index(best)+1}, "
                         f"90%-of-best at iter {first_90+1}/{len(scores)}")

    lines.extend([
        "",
        "## Cost / Efficiency",
        "",
        "| Framework | Wall Time | Iters/hour | Seconds/iter |",
        "|-----------|-----------|------------|--------------|",
    ])

    for s in summaries:
        iters = max(s.get("iterations_completed", 0), 1)
        wall = max(s.get("wall_time_seconds", 1), 1)
        per_iter = wall / iters
        per_hour = 3600 / per_iter
        lines.append(f"| {s['framework']:<16} | {wall:>7.0f}s | {per_hour:>8.1f}   | {per_iter:>10.1f}   |")

    hypothesis_labels = {
        "fixed": "H1: 'There exists an optimal pipelining configuration that significantly outperforms "
                 "current dispatcher and matches or exceeds P2P'",
        "mixed": "H2: 'An evolved pipeline can adapt to mixed workload sizes (1/2/4/16 MiB) and "
                 "outperform the static configuration across all sizes'",
        "concurrent": "H3: 'Evolved pipeline + service-level changes can improve multi-client "
                      "concurrent throughput beyond current Mutex-bound architecture'",
    }
    hypothesis_text = hypothesis_labels.get(eval_mode, f"Hypothesis ({eval_mode})")

    lines.extend([
        "",
        "## Hypothesis Assessment",
        "",
        hypothesis_text,
        "",
    ])

    if summaries:
        winner = max(summaries, key=lambda x: x.get("best_score", 0))
        beats_baseline = winner['best_score'] > baseline * 1.1  # >10% improvement
        beats_p2p = winner['best_score'] > 3.4
        lines.append(f"- Winner: **{winner['framework']}** at {winner['best_score']:.4f} GB/s")
        lines.append(f"- Beats baseline (3.59 GB/s) by >10%: **{'YES' if beats_baseline else 'NO'}** "
                     f"({((winner['best_score']/baseline)-1)*100:+.1f}%)")
        lines.append(f"- Matches/exceeds P2P (3.4 GB/s): **{'YES' if beats_p2p else 'NO'}**")
        lines.append(f"- Approaches NVMe ceiling (5.28 GB/s): {winner['best_score']/5.28*100:.0f}%")

    # Correctness / integrity flags
    lines.extend([
        "",
        "## Correctness Flags (requires manual verification)",
        "",
        "**WARNING**: The evaluator measures throughput but does NOT verify data integrity.",
        "Frameworks may achieve high scores by corrupting the data path. Check each winner for:",
        "",
        "| Flag | What to look for |",
        "|------|-----------------|",
        "| Removed DMA copy | Skipping `cuda_memcpy_h2d_async` or `copy_to_gpu_async` calls |",
        "| Zeroed output | Writing zeros instead of actual NVMe data to GPU |",
        "| Skipped NVMe reads | Not issuing `submit_read` for all chunks |",
        "| Broken sync | Removing stream synchronization (data race → incomplete transfers) |",
        "| Truncated transfer | Only transferring first/last chunk, skipping middle |",
        "| Hardcoded return | Returning `Ok(())` without doing actual work |",
        "",
        "**Action required**: For each framework's best program, diff against initial_program.rs",
        "and verify the data path is intact. Run `certus-api-bench.py` with data verification",
        "to confirm correctness before declaring a winner.",
        "",
    ])

    analysis_file.write_text("\n".join(lines))
    print(f"\nAnalysis written to: {analysis_file}")
    return analysis_file


def analyze_framework_result(name: str, eval_mode: str, summary: dict):
    """LLM-powered qualitative analysis of a framework's best candidate.

    Reads the best program, diffs against initial, and asks an LLM to explain
    what strategy was used, what's novel, and whether it addressed the real bottleneck.
    """
    import openai

    results_dir = _get_results_dir(eval_mode)
    h_dir = _get_hypothesis_dir(eval_mode)
    run_dir = results_dir / name

    # Find best program
    best_program = None
    for candidate in [
        run_dir / "output" / "best" / "best_program.rs",
        run_dir / "output" / "best_program.rs",
    ]:
        if candidate.exists():
            best_program = candidate.read_text()
            break

    # Also check checkpoints for best_program.rs
    if best_program is None:
        ckpts = sorted((run_dir / "output" / "checkpoints").glob("checkpoint_*")) if (run_dir / "output" / "checkpoints").exists() else []
        if ckpts:
            bp = ckpts[-1] / "best_program.rs"
            if bp.exists():
                best_program = bp.read_text()

    # For Nous: read findings.json instead of best_program
    nous_findings = None
    if best_program is None and name == "nous":
        nous_dir = Path("/home/nara/certus/ai-native-storage-certus/.nous")
        prefix_map = {"fixed": "h1-bakeoff", "mixed": "h2-bakeoff", "concurrent": "h3-bakeoff"}
        prefix = prefix_map.get(eval_mode, "bakeoff")
        campaigns = sorted(nous_dir.glob(f"{prefix}-*")) if nous_dir.exists() else []
        if campaigns:
            latest = campaigns[-1]
            # Gather all findings.json from all iterations
            findings_files = sorted(latest.glob("runs/*/findings.json"))
            if findings_files:
                import json as _json
                findings_parts = []
                for ff in findings_files:
                    try:
                        data = _json.loads(ff.read_text())
                        findings_parts.append(f"=== {ff.parent.name} ===\n{_json.dumps(data, indent=2)[:2000]}")
                    except Exception:
                        pass
                nous_findings = "\n\n".join(findings_parts)

    if best_program is None and nous_findings is None:
        print(f"  [analysis] No best program or findings for {name}, skipping LLM analysis")
        return

    # Read initial program for comparison
    initial_program = (h_dir / "initial_program.rs").read_text()

    # Build analysis prompt
    hypothesis_context = {
        "fixed": "H1: Optimizing single-client cold lookup throughput (QD, sync frequency, pipeline constants)",
        "mixed": "H2: Adapting to mixed workload sizes (1/2/4/16 MiB) — did it find size-adaptive logic?",
        "concurrent": "H3: Removing Mutex bottleneck for 8 concurrent clients on 6 NVMe drives",
    }

    scores = summary.get("scores", [])
    score_trajectory = ", ".join(f"{s:.2f}" for s in scores[:15]) if scores else "N/A"

    if nous_findings:
        prompt = f"""Analyze this Nous (controlled experiment) framework's findings for a storage pipeline optimization bakeoff.

CONTEXT: {hypothesis_context.get(eval_mode, eval_mode)}
Framework: {name} (Nous runs controlled A/B experiments with hypothesis arms, not search)
Iterations completed: {summary.get('iterations_completed', 0)}/{summary.get('iterations_requested', 0)}

NOUS FINDINGS (experimental results with arm analysis):
{nous_findings[:5000]}

Analyze in 150-200 words:
1. KEY INSIGHT: What causal claim did Nous validate or refute?
2. STRONGEST ARM: Which experimental arm performed best and why?
3. MECHANISM: What underlying hardware/software mechanism explains the result?
4. ACTIONABLE: What concrete code change should be made based on these findings?
5. VERDICT: One sentence — what did Nous prove that search frameworks cannot?"""
    else:
        prompt = f"""Analyze this evolutionary framework's output for a storage pipeline optimization bakeoff.

CONTEXT: {hypothesis_context.get(eval_mode, eval_mode)}
Framework: {name}
Best score: {summary.get('best_score', 0):.4f} GB/s
Score trajectory (first 15): [{score_trajectory}]
Iterations completed: {summary.get('iterations_completed', 0)}/{summary.get('iterations_requested', 0)}

INITIAL PROGRAM (what the framework started with):
```rust
{initial_program[:3000]}
```
{"... (truncated)" if len(initial_program) > 3000 else ""}

BEST CANDIDATE (what the framework produced):
```rust
{best_program[:4000]}
```
{"... (truncated)" if len(best_program) > 4000 else ""}

Analyze in 150-200 words:
1. STRATEGY: What architectural change did it attempt? (e.g., removed Mutex, sharded by drive, increased QD, added per-client streams)
2. NOVELTY: Did it discover something non-obvious that a human might not try first?
3. CORRECTNESS: Any red flags? (removed DMA copies, broken sync, compile hacks)
4. EFFECTIVENESS: Did the strategy actually address the stated bottleneck?
5. VERDICT: One sentence — is this a real improvement or noise?"""

    api_key = os.environ.get("OPENAI_API_KEY", "")
    if not api_key:
        try:
            import json as _json
            settings = _json.load(open(Path.home() / ".claude" / "settings.json"))
            api_key = settings.get("env", {}).get("ANTHROPIC_AUTH_TOKEN", "")
        except Exception:
            pass

    if not api_key:
        print(f"  [analysis] No API key available, skipping LLM analysis for {name}")
        return

    try:
        client = openai.OpenAI(
            api_key=api_key,
            base_url="https://ete-litellm.ai-models.vpc-int.res.ibm.com",
        )
        response = client.chat.completions.create(
            model="aws/claude-sonnet-4-6",
            messages=[{"role": "user", "content": prompt}],
            max_tokens=500,
            timeout=60,
        )
        analysis_text = response.choices[0].message.content

        # Write analysis
        analysis_file = results_dir / f"llm-analysis-{name}-{timestamp()}.md"
        content = f"# {name} — LLM Analysis ({eval_mode})\n\n"
        content += f"**Score**: {summary.get('best_score', 0):.4f} GB/s | "
        content += f"**Iterations**: {summary.get('iterations_completed', 0)}\n\n"
        content += analysis_text
        analysis_file.write_text(content)
        print(f"  [analysis] LLM analysis written: {analysis_file.name}")

    except Exception as e:
        print(f"  [analysis] LLM analysis failed for {name}: {e}")


def main():
    parser = argparse.ArgumentParser(description="Autonomous pipeline bakeoff orchestrator")
    parser.add_argument(
        "--iterations", type=int, default=30,
        help="Iterations per framework (default: 30)",
    )
    parser.add_argument(
        "--eval", choices=["fixed", "mixed", "concurrent"], default="fixed",
        help="Evaluation mode (default: fixed)",
    )
    parser.add_argument(
        "--frameworks", type=str, default="all",
        help="Comma-separated list of frameworks, or 'all' (default: all)",
    )
    args = parser.parse_args()

    if args.frameworks == "all":
        frameworks = ALL_FRAMEWORKS
    else:
        frameworks = [f.strip() for f in args.frameworks.split(",")]
        for f in frameworks:
            if f not in ALL_FRAMEWORKS:
                print(f"ERROR: Unknown framework '{f}'. Available: {ALL_FRAMEWORKS}")
                sys.exit(1)

    results_dir = _get_results_dir(args.eval)
    results_dir.mkdir(parents=True, exist_ok=True)

    print(f"{'='*70}")
    print(f"  Pipeline Bakeoff Orchestrator — Autonomous Mode")
    print(f"{'='*70}")
    print(f"  Frameworks: {frameworks}")
    print(f"  Iterations per framework: {args.iterations}")
    print(f"  Eval mode: {args.eval}")
    print(f"  Results dir: {results_dir}")
    print(f"  Baseline: 3.59 GB/s (current dispatcher, single drive)")
    print(f"  Target: beat P2P (3.4 GB/s) and approach NVMe ceiling (5.28 GB/s)")
    print()

    summaries = []
    for name in frameworks:
        summary = run_framework(name, args.iterations, args.eval)
        summaries.append(summary)
        # Write intermediate analysis after each framework
        write_analysis(summaries, args.eval)
        # LLM-powered qualitative analysis of this framework's best candidate
        if summary.get("best_score", 0) > 0 or name == "nous":
            analyze_framework_result(name, args.eval, summary)

    # Final analysis
    final_analysis = write_analysis(summaries, args.eval)

    print(f"\n{'='*70}")
    print(f"  BAKEOFF COMPLETE")
    print(f"{'='*70}")
    print(f"  Frameworks run: {len(summaries)}")
    if summaries:
        winner = max(summaries, key=lambda x: x.get("best_score", 0))
        print(f"  Winner: {winner['framework']} ({winner['best_score']:.4f} GB/s)")
        print(f"  Analysis: {final_analysis}")
    print()


if __name__ == "__main__":
    main()
