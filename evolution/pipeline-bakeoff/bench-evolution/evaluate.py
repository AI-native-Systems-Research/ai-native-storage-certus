"""Evaluator for GEPA benchmark script evolution.

Runs the evolved benchmark script against the live Certus server and
parses aggregate cold-lookup throughput as the optimization score.
"""

import os
import re
import subprocess
import tempfile

BENCH_ARGS = [
    "--clients", "8",
    "--num-objects", "16",
    "--iterations", "5",
    "--block-size", "4194304",
]

SERVER = os.environ.get("CERTUS_SERVER", "localhost:50051")
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
TIMEOUT_SECONDS = 120


def evaluate(candidate: str) -> tuple[float, dict]:
    """Run evolved benchmark script, return (throughput_gbps, side_info)."""

    with tempfile.NamedTemporaryFile(
        mode="w", suffix=".py", dir=SCRIPT_DIR, delete=False
    ) as f:
        f.write(candidate)
        script_path = f.name

    try:
        env = os.environ.copy()
        env["PYTHONPATH"] = os.path.join(
            os.path.dirname(SCRIPT_DIR), "..", "..", "apps", "python"
        ) + ":" + env.get("PYTHONPATH", "")

        result = subprocess.run(
            ["python3", script_path, "--server", SERVER] + BENCH_ARGS,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_SECONDS,
            env=env,
            cwd=os.path.join(SCRIPT_DIR, "..", "..", "..", "apps", "python"),
        )

        stdout = result.stdout
        stderr = result.stderr

        if result.returncode != 0:
            return 0.0, {
                "Error": f"Script exited with code {result.returncode}",
                "stdout": stdout[-3000:] if stdout else "",
                "stderr": stderr[-3000:] if stderr else "",
            }

        throughput = _parse_cold_throughput(stdout)
        if throughput is None:
            return 0.0, {
                "Error": "Could not parse cold lookup throughput from output",
                "stdout": stdout[-3000:] if stdout else "",
                "stderr": stderr[-3000:] if stderr else "",
            }

        errors_match = re.search(r"ERRORS \((\d+)\)", stdout)
        if errors_match and int(errors_match.group(1)) > 0:
            return 0.0, {
                "Error": "Benchmark reported errors (possible data corruption)",
                "stdout": stdout[-3000:],
                "stderr": stderr[-3000:] if stderr else "",
            }

        return throughput, {
            "throughput_gbps": throughput,
            "stdout": stdout[-3000:],
            "stderr": stderr[-1000:] if stderr else "",
        }

    except subprocess.TimeoutExpired:
        return 0.0, {"Error": f"Script timed out after {TIMEOUT_SECONDS}s"}
    except Exception as e:
        return 0.0, {"Error": f"Evaluator exception: {type(e).__name__}: {e}"}
    finally:
        os.unlink(script_path)


def _parse_cold_throughput(stdout: str) -> float | None:
    """Extract aggregate GB/s from the 'Lookup (cold)' stats block.

    The output format is:
        Lookup (cold)        avg=... p50=... p99=... min=... max=...
                             per-client=X.XX GB/s  aggregate=X.XX GB/s
    We want the last aggregate= value (cold comes after hot in output).
    """
    lines = stdout.splitlines()
    found_cold = False
    for line in lines:
        if "Lookup (cold)" in line:
            found_cold = True
        if found_cold and "aggregate=" in line:
            match = re.search(r"aggregate=\s*([\d.]+)\s*GB/s", line)
            if match:
                return float(match.group(1))

    all_aggregates = re.findall(r"aggregate=\s*([\d.]+)\s*GB/s", stdout)
    if all_aggregates:
        return float(all_aggregates[-1])
    return None
