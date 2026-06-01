"""Transfer path verification for the P2P evolution experiment.

Determines which data path was actually used during a benchmark run by parsing
the binary's output. The gpu-bb-vs-p2p benchmark explicitly labels its paths
(bounce-buf vs p2p-direct), so detection is straightforward.

For evolved code, we additionally check:
- cudaMemcpy direction (H2D = bounce, D2D = P2P staging)
- Whether GDRCopy BAR1 mapping functions are referenced in the binary
"""

from __future__ import annotations

import re
import subprocess
from pathlib import Path


def verify_path_from_output(output: str) -> dict:
    """Parse benchmark output to determine which transfer path was used.

    The gpu-bb-vs-p2p binary outputs lines like:
        bounce-buf   | mean   30987.9 us | ... | 3227.1 MB/s
        p2p-direct   | mean   29614.1 us | ... | 3376.8 MB/s

    Returns dict with path classification and metrics for each detected path.
    """
    paths_detected = []
    metrics = {}

    for line in output.splitlines():
        line_stripped = line.strip()

        if "bounce-buf" in line_stripped:
            paths_detected.append("bounce_pinned")
            m = re.search(r"([\d.]+)\s*MB/s", line_stripped)
            if m:
                metrics["bounce_throughput_mbs"] = float(m.group(1))
            m = re.search(r"mean\s+([\d.]+)\s*us", line_stripped)
            if m:
                metrics["bounce_mean_latency_us"] = float(m.group(1))

        elif "p2p-direct" in line_stripped:
            paths_detected.append("p2p_gdrcopy")
            m = re.search(r"([\d.]+)\s*MB/s", line_stripped)
            if m:
                metrics["p2p_throughput_mbs"] = float(m.group(1))
            m = re.search(r"mean\s+([\d.]+)\s*us", line_stripped)
            if m:
                metrics["p2p_mean_latency_us"] = float(m.group(1))

    # Parse detailed stats (p50, p99, min, max)
    for line in output.splitlines():
        for path_label, prefix in [("bounce-buf", "bounce"), ("p2p-direct", "p2p")]:
            if path_label in line:
                for stat in ["min", "p50", "p99", "max"]:
                    m = re.search(rf"{stat}\s+([\d.]+)\s*us", line)
                    if m:
                        metrics[f"{prefix}_{stat}_latency_us"] = float(m.group(1))

    # Determine primary path (what the evaluator should score on)
    if "p2p_gdrcopy" in paths_detected:
        primary_path = "p2p_gdrcopy"
    elif "bounce_pinned" in paths_detected:
        primary_path = "bounce_pinned"
    else:
        primary_path = "unknown"

    return {
        "primary_path": primary_path,
        "paths_detected": paths_detected,
        "metrics": metrics,
        "verified": len(paths_detected) > 0,
    }


def verify_binary_uses_p2p(binary_path: str) -> dict:
    """Check if a compiled binary contains P2P-related symbols.

    Uses `nm` or `strings` to detect whether GDRCopy/P2P functions are linked.
    """
    binary = Path(binary_path)
    if not binary.exists():
        return {"has_p2p_symbols": False, "error": "binary not found"}

    try:
        result = subprocess.run(
            ["nm", "-D", str(binary)],
            capture_output=True, text=True, timeout=10,
        )
        symbols = result.stdout

        p2p_indicators = [
            "gdr_open", "gdr_pin_buffer", "gdr_map",
            "create_spdk_dma_buffer_from_gpu_bar",
            "rte_extmem_register",
        ]
        bounce_indicators = [
            "cudaHostAlloc", "cudaHostRegister",
            "create_spdk_dma_buffer_from_cuda_host_alloc",
        ]

        found_p2p = [s for s in p2p_indicators if s in symbols]
        found_bounce = [s for s in bounce_indicators if s in symbols]

        return {
            "has_p2p_symbols": len(found_p2p) > 0,
            "has_bounce_symbols": len(found_bounce) > 0,
            "p2p_symbols": found_p2p,
            "bounce_symbols": found_bounce,
        }
    except Exception as e:
        return {"has_p2p_symbols": False, "error": str(e)}


def classify_memcpy_direction(output: str) -> str:
    """Infer transfer path from cudaMemcpy direction in debug output.

    cudaMemcpyHostToDevice (kind=1) → bounce buffer path
    cudaMemcpyDeviceToDevice (kind=3) → P2P staging (BAR1 ring → final GPU buffer)
    """
    h2d_count = output.count("cudaMemcpyHostToDevice") + output.count("kind=1")
    d2d_count = output.count("cudaMemcpyDeviceToDevice") + output.count("kind=3")

    if d2d_count > h2d_count:
        return "p2p_gdrcopy"
    elif h2d_count > 0:
        return "bounce_pinned"
    else:
        return "unknown"
