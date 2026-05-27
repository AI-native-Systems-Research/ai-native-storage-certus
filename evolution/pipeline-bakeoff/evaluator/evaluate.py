#!/usr/bin/env python3
"""Shared evaluator for the pipeline bakeoff.

Supports two modes:
  1. Single-file: Takes a candidate pipeline.rs
  2. Multi-file: Takes a directory containing one or more source files to patch

Interface (compatible with SkyDiscover/OpenEvolve):
    python evaluate.py <candidate_pipeline.rs> [--eval fixed|mixed|concurrent]
    python evaluate.py <candidate_dir/> [--eval fixed|mixed|concurrent]

Multi-file mode: the candidate directory may contain:
  - pipeline.rs → components/dispatcher/src/pipeline.rs
  - lib.rs → components/dispatcher/src/lib.rs
  - Cargo.toml → components/dispatcher/Cargo.toml
  - gpu_services_lib.rs → components/gpu-services/src/lib.rs

Output (stdout, last line):
    {"status": "success"|"error", "combined_score": <float>, "artifacts": {"feedback": "..."}}

Exit code 0 always (score=0 on failure, with error details in artifacts.feedback).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]  # ai-native-storage-certus/
PIPELINE_RS = REPO_ROOT / "components" / "dispatcher" / "src" / "pipeline.rs"
LIB_RS = REPO_ROOT / "components" / "dispatcher" / "src" / "lib.rs"
SERVICE_RS = REPO_ROOT / "apps" / "certus-server" / "src" / "service.rs"
SERVER_BIN = REPO_ROOT / "target" / "release" / "certus-server"
BENCH_SCRIPT = REPO_ROOT / "apps" / "python" / "certus-api-bench.py"
VERIFY_SCRIPT = Path(__file__).resolve().parent / "verify_integrity.py"
SYSTEM_PYTHON = "/usr/bin/python3"

# Multi-file mapping: filename in candidate dir → target path in repo
MULTI_FILE_MAP = {
    "pipeline.rs": REPO_ROOT / "components" / "dispatcher" / "src" / "pipeline.rs",
    "lib.rs": REPO_ROOT / "components" / "dispatcher" / "src" / "lib.rs",
    "service.rs": REPO_ROOT / "apps" / "certus-server" / "src" / "service.rs",
    "Cargo.toml": REPO_ROOT / "components" / "dispatcher" / "Cargo.toml",
    "gpu_services_lib.rs": REPO_ROOT / "components" / "gpu-services" / "src" / "lib.rs",
    "dma.rs": REPO_ROOT / "components" / "gpu-services" / "src" / "dma.rs",
}

# PCIe addresses (H8 machine)
METADATA_PCI = "0000:61:00.0"
DATA_PCI_SINGLE = "0000:62:00.0"  # Single drive for Evaluator A/B (H1/H2)
DATA_PCI_ALL = [  # All 7 drives for Evaluator C (H3)
    "0000:62:00.0",
    "0000:63:00.0",
    "0000:64:00.0",
    "0000:c1:00.0",
    "0000:c2:00.0",
    "0000:c3:00.0",
]

GRPC_PORT = 50051
SERVER_STARTUP_TIMEOUT = 15  # seconds to wait for gRPC ready
BUILD_TIMEOUT = 120  # seconds
BENCH_TIMEOUT = 90  # seconds per benchmark run


def kill_server():
    """Kill any running certus-server processes."""
    subprocess.run(
        ["pkill", "-x", "certus-server"],
        capture_output=True,
        timeout=5,
    )
    time.sleep(1)
    subprocess.run(
        ["pkill", "-9", "-x", "certus-server"],
        capture_output=True,
        timeout=5,
    )
    time.sleep(1)


def patch_candidate(candidate_path: Path) -> tuple[list[Path], str | None]:
    """Patch candidate file(s) into source tree.

    If candidate_path is a file with H3 section markers: concatenated multi-file mode.
    If candidate_path is a file without markers: single-file mode (pipeline.rs only).
    If candidate_path is a directory: multi-file mode (maps known filenames to targets).

    Returns (list_of_patched_targets, error_string_or_None).
    """
    patched = []
    try:
        if candidate_path.is_dir():
            # Multi-file mode
            for fname, target in MULTI_FILE_MAP.items():
                src = candidate_path / fname
                if src.exists():
                    target.write_text(src.read_text())
                    patched.append(target)
            if not patched:
                return [], f"No recognized files in candidate dir: {list(candidate_path.iterdir())}"
            # Must have at least pipeline.rs or service.rs (H3 may evolve service.rs alone)
            if PIPELINE_RS not in patched and SERVICE_RS not in patched:
                return patched, "Multi-file candidate missing both pipeline.rs and service.rs"
        else:
            # Single-file mode — check for H3 concatenated format
            content = candidate_path.read_text()

            if H3_SECTION_PATTERN.search(content):
                # H3 concatenated multi-file format
                return split_concatenated_h3(content)

            if "pipelined_ssd_to_gpu" not in content:
                return [], "Candidate does not contain pipelined_ssd_to_gpu function"
            PIPELINE_RS.write_text(content)
            patched.append(PIPELINE_RS)
        return patched, None
    except Exception as e:
        return patched, f"Failed to patch candidate: {e}"


def build_server() -> tuple[bool, str]:
    """Build certus-server. Returns (success, output)."""
    try:
        result = subprocess.run(
            ["cargo", "build", "-p", "certus-server", "--release"],
            capture_output=True,
            text=True,
            timeout=BUILD_TIMEOUT,
            cwd=REPO_ROOT,
        )
        if result.returncode != 0:
            stderr = result.stderr[-2000:] if len(result.stderr) > 2000 else result.stderr
            return False, f"Build failed:\n{stderr}"
        return True, "Build succeeded"
    except subprocess.TimeoutExpired:
        return False, f"Build timed out after {BUILD_TIMEOUT}s"
    except Exception as e:
        return False, f"Build error: {e}"


def start_server(multi_drive: bool = False) -> tuple[bool, str]:
    """Start certus-server and wait for gRPC readiness."""
    cmd = [
        str(SERVER_BIN),
        "--metadata-pci", METADATA_PCI,
    ]
    if multi_drive:
        for pci in DATA_PCI_ALL:
            cmd.extend(["--data-pci", pci])
    else:
        cmd.extend(["--data-pci", DATA_PCI_SINGLE])

    try:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            preexec_fn=os.setsid,
        )
    except Exception as e:
        return False, f"Failed to start server: {e}"

    # Wait for gRPC port to be ready
    deadline = time.time() + SERVER_STARTUP_TIMEOUT
    while time.time() < deadline:
        try:
            import socket
            s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            s.settimeout(1)
            s.connect(("localhost", GRPC_PORT))
            s.close()
            return True, f"Server started (PID {proc.pid})"
        except (ConnectionRefusedError, OSError):
            time.sleep(0.5)

    # Timeout — kill and report
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except ProcessLookupError:
        pass
    stderr_out = ""
    try:
        stderr_out = proc.stderr.read().decode()[-1000:]
    except Exception:
        pass
    return False, f"Server failed to start within {SERVER_STARTUP_TIMEOUT}s. stderr: {stderr_out}"


def run_benchmark(block_size: int = 4 * 1024 * 1024, clients: int = 1) -> tuple[float | None, dict, str]:
    """Run certus-api-bench.py and parse cold lookup throughput + latency.

    Returns (throughput_gbps, latency_dict, raw_output) or (None, {}, error_msg).
    """
    cmd = [
        SYSTEM_PYTHON,
        str(BENCH_SCRIPT),
        "--server", f"localhost:{GRPC_PORT}",
        "--clients", str(clients),
        "--num-objects", "16",
        "--iterations", "10",
        "--block-size", str(block_size),
    ]

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=BENCH_TIMEOUT,
            cwd=BENCH_SCRIPT.parent,
        )
    except subprocess.TimeoutExpired:
        return None, {}, f"Benchmark timed out after {BENCH_TIMEOUT}s"
    except Exception as e:
        return None, {}, f"Benchmark error: {e}"

    output = result.stdout + result.stderr

    cold_section = False
    throughput = None
    latency = {}
    for line in output.split("\n"):
        if "Lookup (cold)" in line:
            cold_section = True
            # Parse latency from this line: avg=X us  p50=X us  p99=X us
            for key in ("avg", "p50", "p99"):
                m = re.search(rf"{key}=\s*([\d.]+)\s*us", line)
                if m:
                    latency[f"{key}_us"] = float(m.group(1))
            continue
        if cold_section and "per-client=" in line:
            m = re.search(r"aggregate=\s*([\d.]+)\s*GB/s", line)
            if m:
                throughput = float(m.group(1))
            else:
                m = re.search(r"per-client=\s*([\d.]+)\s*GB/s", line)
                if m:
                    throughput = float(m.group(1))
            break

    if throughput is None:
        return None, {}, f"Could not parse cold throughput from output:\n{output[-1000:]}"

    return throughput, latency, output


def verify_data_integrity() -> tuple[bool, str]:
    """Run data integrity verification. Returns (passed, detail)."""
    try:
        result = subprocess.run(
            [SYSTEM_PYTHON, str(VERIFY_SCRIPT)],
            capture_output=True,
            text=True,
            timeout=60,
        )
        output = result.stdout.strip()
        if result.returncode == 0:
            try:
                data = json.loads(output)
                return True, data.get("detail", "pass")
            except (json.JSONDecodeError, ValueError):
                return True, "pass"
        else:
            try:
                data = json.loads(output)
                return False, data.get("detail", "integrity check failed")
            except (json.JSONDecodeError, ValueError):
                return False, f"integrity check failed (exit {result.returncode}): {output[-200:]}"
    except subprocess.TimeoutExpired:
        return False, "integrity check timed out"
    except Exception as e:
        return False, f"integrity check error: {e}"


# ---------------------------------------------------------------------------
# H3 concatenated file support
# ---------------------------------------------------------------------------

# H3 concatenated file section markers
H3_SECTION_PATTERN = re.compile(r'^// === FILE: (\S+\.rs)')

# Map from section header filename to target file in repo
H3_FILE_TARGETS = {
    "service.rs": SERVICE_RS,
    "lib.rs": LIB_RS,
    "pipeline.rs": PIPELINE_RS,
}


def split_concatenated_h3(candidate_content: str) -> tuple[list[Path], str | None]:
    """Split a concatenated H3 candidate into individual file sections and patch.

    The candidate contains sections delimited by:
        // === FILE: <filename> (optional line info) ===
        ... code ...
        // === FILE: <next_filename> ... ===

    For pipeline.rs: the section replaces the entire file.
    For lib.rs: the section replaces the EVOLVE-BLOCK regions in the existing file.
    For service.rs: the section replaces the DispatcherService struct and impl block.

    Returns (list_of_patched_targets, error_string_or_None).
    """
    # Parse sections
    sections: dict[str, str] = {}
    current_file = None
    current_lines: list[str] = []

    for line in candidate_content.split("\n"):
        m = H3_SECTION_PATTERN.match(line)
        if m:
            # Save previous section
            if current_file is not None:
                sections[current_file] = "\n".join(current_lines)
            current_file = m.group(1)
            current_lines = []
        else:
            current_lines.append(line)

    # Save last section
    if current_file is not None:
        sections[current_file] = "\n".join(current_lines)

    if not sections:
        return [], "No H3 section markers found in candidate"

    patched = []

    for filename, content in sections.items():
        target = H3_FILE_TARGETS.get(filename)
        if target is None:
            continue

        if not target.exists():
            return patched, f"Target file not found: {target}"

        try:
            if filename == "pipeline.rs":
                # Full replacement -- pipeline.rs is entirely evolved
                target.write_text(content)
                patched.append(target)

            elif filename == "lib.rs":
                # Replace EVOLVE-BLOCK regions in existing lib.rs
                existing = target.read_text()
                patched_content = _replace_evolve_blocks(existing, content)
                if patched_content is None:
                    return patched, "Failed to patch lib.rs EVOLVE-BLOCK regions"
                target.write_text(patched_content)
                patched.append(target)

            elif filename == "service.rs":
                # Replace DispatcherService struct/impl in service.rs
                existing = target.read_text()
                patched_content = _replace_service_section(existing, content)
                if patched_content is None:
                    return patched, "Failed to patch service.rs"
                target.write_text(patched_content)
                patched.append(target)

        except Exception as e:
            return patched, f"Failed to patch {filename}: {e}"

    if not patched:
        return [], f"No recognized files in H3 sections: {list(sections.keys())}"

    # pipeline.rs must be present for a valid H3 candidate
    if PIPELINE_RS not in patched:
        return patched, "H3 candidate missing pipeline.rs section"

    return patched, None


def _replace_evolve_blocks(existing: str, candidate_section: str) -> str | None:
    """Replace EVOLVE-BLOCK regions in `existing` with matching blocks from candidate.

    Looks for patterns like:
        // ===== EVOLVE-BLOCK: NAME =====
        ... content ...
        // ===== END EVOLVE-BLOCK: NAME =====

    Each named block in the candidate replaces the same-named block in existing.
    """
    block_pattern = re.compile(
        r'([ \t]*// ===== EVOLVE-BLOCK: (\w+) =====\n)(.*?)([ \t]*// ===== END EVOLVE-BLOCK: \2 =====)',
        re.DOTALL,
    )

    # Extract named blocks from candidate
    candidate_blocks: dict[str, str] = {}
    for m in block_pattern.finditer(candidate_section):
        name = m.group(2)
        candidate_blocks[name] = m.group(3)

    if not candidate_blocks:
        # Candidate doesn't use EVOLVE-BLOCK markers -- try full-block replacement.
        # Find the largest evolve block in existing and replace its content.
        blocks_in_existing = list(block_pattern.finditer(existing))
        if blocks_in_existing:
            largest = max(blocks_in_existing, key=lambda x: len(x.group(3)))
            header = largest.group(1)
            footer = largest.group(4)
            result = (
                existing[:largest.start()]
                + header
                + candidate_section
                + "\n"
                + footer
                + existing[largest.end():]
            )
            return result
        return None

    # Replace each named block in existing with candidate version
    result = existing
    for m in reversed(list(block_pattern.finditer(result))):
        name = m.group(2)
        if name in candidate_blocks:
            header = m.group(1)
            footer = m.group(4)
            result = (
                result[:m.start()]
                + header
                + candidate_blocks[name]
                + footer
                + result[m.end():]
            )

    return result


def _find_braced_block(text: str, start_pos: int) -> int | None:
    """Find the end position of a braced block starting at the '{' at start_pos.

    Returns the index AFTER the closing '}', or None if unbalanced.
    """
    if start_pos >= len(text) or text[start_pos] != '{':
        return None
    depth = 0
    for i in range(start_pos, len(text)):
        if text[i] == '{':
            depth += 1
        elif text[i] == '}':
            depth -= 1
            if depth == 0:
                return i + 1
    return None


def _find_struct_and_impl(text: str, struct_name: str) -> tuple[int, int] | None:
    """Find the span of 'pub struct Name { ... }' followed by 'impl Name { ... }'.

    Returns (start, end) covering both the struct and impl blocks, or None.
    """
    struct_re = re.compile(rf'pub struct {struct_name}\s*\{{')
    m = struct_re.search(text)
    if not m:
        return None
    struct_start = m.start()
    brace_start = m.end() - 1
    struct_end = _find_braced_block(text, brace_start)
    if struct_end is None:
        return None

    # Look for impl immediately after (with optional whitespace/attributes)
    remainder = text[struct_end:]
    impl_re = re.compile(rf'\s*impl {struct_name}\s*\{{')
    im = impl_re.match(remainder)
    if not im:
        return (struct_start, struct_end)
    impl_brace_start = struct_end + im.end() - 1
    impl_end = _find_braced_block(text, impl_brace_start)
    if impl_end is None:
        return (struct_start, struct_end)
    return (struct_start, impl_end)


def _replace_service_section(existing: str, candidate_section: str) -> str | None:
    """Replace the DispatcherService struct and impl in service.rs.

    Uses brace-counting to correctly find nested blocks.
    Falls back to returning existing unchanged (service.rs changes are optional).
    """
    existing_span = _find_struct_and_impl(existing, "DispatcherService")
    if not existing_span:
        return existing

    candidate_span = _find_struct_and_impl(candidate_section, "DispatcherService")
    if not candidate_span:
        return existing

    replacement = candidate_section[candidate_span[0]:candidate_span[1]]
    return existing[:existing_span[0]] + replacement + existing[existing_span[1]:]


def evaluate_fixed(block_size: int = 4 * 1024 * 1024, clients: int = 1) -> tuple[float, str]:
    """Single fixed-size evaluation. Returns (score, feedback)."""
    throughput, latency, output = run_benchmark(block_size=block_size, clients=clients)
    if throughput is None:
        return 0.0, output
    lat_str = ""
    if latency:
        lat_str = f" | p50={latency.get('p50_us', 0):.0f}µs p99={latency.get('p99_us', 0):.0f}µs"
    return throughput, f"Cold lookup throughput: {throughput:.3f} GB/s{lat_str}"


def evaluate_mixed() -> tuple[float, str]:
    """Mixed-size evaluation (H2): 1/2/4/16 MiB equally weighted."""
    sizes = [1 * 1024 * 1024, 2 * 1024 * 1024, 4 * 1024 * 1024, 16 * 1024 * 1024]
    labels = ["1 MiB", "2 MiB", "4 MiB", "16 MiB"]
    scores = []
    feedback_lines = []

    for size, label in zip(sizes, labels):
        throughput, latency, output = run_benchmark(block_size=size)
        if throughput is None:
            return 0.0, f"Failed at {label}: {output}"
        scores.append(throughput)
        lat_str = f" p50={latency.get('p50_us', 0):.0f}µs" if latency else ""
        feedback_lines.append(f"  {label}: {throughput:.3f} GB/s{lat_str}")

    composite = sum(scores) / len(scores)
    feedback = "Mixed-size cold throughput:\n" + "\n".join(feedback_lines) + f"\n  Composite: {composite:.3f} GB/s"
    return composite, feedback


def evaluate_concurrent() -> tuple[float, str]:
    """Concurrent multi-client evaluation (H3): 8 clients, 4 MiB objects, multi-drive.

    Measures aggregate cold lookup throughput across 8 concurrent clients.
    The server must already be started with multi_drive=True.
    """
    clients = 8
    block_size = 4 * 1024 * 1024
    throughput, latency, output = run_benchmark(block_size=block_size, clients=clients)
    if throughput is None:
        return 0.0, output
    lat_str = ""
    if latency:
        lat_str = f" | p50={latency.get('p50_us', 0):.0f}us p99={latency.get('p99_us', 0):.0f}us"
    return throughput, f"Concurrent cold lookup ({clients} clients): {throughput:.3f} GB/s aggregate{lat_str}"


def evaluate(program_path: str) -> dict:
    """SkyDiscover-compatible evaluate function.

    Called by SkyDiscover's Evaluator class with the path to a temp file
    containing the candidate program source code.

    Returns dict with 'combined_score' and 'artifacts'.
    """
    candidate_path = Path(program_path).resolve()
    if not candidate_path.exists():
        return {"combined_score": 0.0, "artifacts": {"feedback": f"Candidate not found: {candidate_path}"}}

    t_start = time.time()

    # Safety: restore any leftover .bak files from a previous crashed evaluation
    all_targets = set(MULTI_FILE_MAP.values()) | set(H3_FILE_TARGETS.values())
    for target in all_targets:
        bak = target.with_suffix(target.suffix + ".bak")
        if bak.exists():
            shutil.copy2(bak, target)
            bak.unlink()

    backups = {}
    # Back up all files that might be patched
    for target in all_targets:
        if target.exists():
            bak = target.with_suffix(target.suffix + ".bak")
            shutil.copy2(target, bak)
            backups[target] = bak

    try:
        patched, err = patch_candidate(candidate_path)
        if err:
            return {"combined_score": 0.0, "artifacts": {"feedback": err}}

        ok, msg = build_server()
        if not ok:
            return {"combined_score": 0.0, "artifacts": {"feedback": msg}}

        eval_mode = os.environ.get("BAKEOFF_EVAL_MODE", "fixed")
        kill_server()
        ok, msg = start_server(multi_drive=(eval_mode == "concurrent"))
        if not ok:
            return {"combined_score": 0.0, "artifacts": {"feedback": msg}}

        if eval_mode == "mixed":
            score, feedback = evaluate_mixed()
        elif eval_mode == "concurrent":
            score, feedback = evaluate_concurrent()
        else:
            score, feedback = evaluate_fixed()

        # Data integrity check — zero the score if data is corrupted
        if score > 0:
            integrity_ok, integrity_detail = verify_data_integrity()
            if not integrity_ok:
                feedback += f"\n\nINTEGRITY FAILED: {integrity_detail}"
                score = 0.0
            else:
                feedback += f"\nIntegrity: {integrity_detail}"

        elapsed = time.time() - t_start
        feedback += f"\n\nEval time: {elapsed:.1f}s | Files patched: {[p.name for p in patched]}"

        return {"combined_score": round(score, 4), "artifacts": {"feedback": feedback}}

    finally:
        for target, bak in backups.items():
            if bak.exists():
                shutil.copy2(bak, target)
                bak.unlink()
        kill_server()


def main():
    parser = argparse.ArgumentParser(description="Pipeline bakeoff evaluator")
    parser.add_argument("candidate", type=str, help="Path to candidate pipeline.rs or directory with multiple files")
    parser.add_argument(
        "--eval", choices=["fixed", "mixed", "concurrent"],
        default=os.environ.get("BAKEOFF_EVAL_MODE", "fixed"),
        help="Evaluation mode: fixed (4 MiB, 1 client), mixed (1/2/4/16 MiB), concurrent (8 clients, multi-drive)",
    )
    parser.add_argument(
        "--block-size", type=int, default=4 * 1024 * 1024,
        help="Block size for fixed eval (default: 4 MiB)",
    )
    parser.add_argument(
        "--clients", type=int, default=1,
        help="Client count for fixed eval (default: 1)",
    )
    args = parser.parse_args()

    candidate_path = Path(args.candidate).resolve()
    if not candidate_path.exists():
        _emit(0.0, f"Candidate file not found: {candidate_path}")
        return

    t_start = time.time()

    # Safety: restore any leftover .bak files from a previous crashed evaluation
    all_targets = set(MULTI_FILE_MAP.values()) | set(H3_FILE_TARGETS.values())
    for target in all_targets:
        bak = target.with_suffix(target.suffix + ".bak")
        if bak.exists():
            shutil.copy2(bak, target)
            bak.unlink()

    # Backup all files that might be patched
    backups = {}
    for target in all_targets:
        if target.exists():
            bak = target.with_suffix(target.suffix + ".bak")
            shutil.copy2(target, bak)
            backups[target] = bak

    try:
        # 1. Patch
        patched, err = patch_candidate(candidate_path)
        if err:
            _emit(0.0, err)
            return

        # 2. Build
        ok, msg = build_server()
        if not ok:
            _emit(0.0, msg)
            return

        # 3. Kill existing server, start fresh
        kill_server()
        multi_drive = (args.eval == "concurrent")
        ok, msg = start_server(multi_drive=multi_drive)
        if not ok:
            _emit(0.0, msg)
            return

        # 4. Run benchmark
        if args.eval == "fixed":
            score, feedback = evaluate_fixed(block_size=args.block_size, clients=args.clients)
        elif args.eval == "mixed":
            score, feedback = evaluate_mixed()
        elif args.eval == "concurrent":
            score, feedback = evaluate_concurrent()
        else:
            score, feedback = 0.0, f"Unknown eval mode: {args.eval}"

        elapsed = time.time() - t_start
        feedback += f"\n\nEval time: {elapsed:.1f}s | Files patched: {[p.name for p in patched]}"
        _emit(score, feedback)

    finally:
        # Restore all backed-up files
        for target, bak in backups.items():
            if bak.exists():
                shutil.copy2(bak, target)
                bak.unlink()
        # Kill server (leave clean state for next eval)
        kill_server()


def _emit(score: float, feedback: str):
    """Print JSON result to stdout (SkyDiscover/OpenEvolve compatible)."""
    result = {
        "status": "success" if score > 0 else "error",
        "combined_score": round(score, 4),
        "artifacts": {"feedback": feedback},
    }
    print(json.dumps(result))


if __name__ == "__main__":
    main()
