"""Run an agent's success check. Exit status decides; optional output patterns refine it."""
from __future__ import annotations

import os
import re
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional

from .policy import OracleSpec


@dataclass
class OracleResult:
    passed: bool
    exit_code: Optional[int]
    output: str
    duration_s: float
    reasons: List[str]

    def tail(self, lines: int = 30) -> str:
        return "\n".join(self.output.strip().splitlines()[-lines:])


def run_oracle(spec: OracleSpec, workdir: Path, env: Optional[Dict[str, str]] = None) -> OracleResult:
    started = time.monotonic()
    # Bytecode caches key on mtime+size, so a same-size edit within one second can run stale
    # code; never let the check itself write them.
    run_env = dict(os.environ if env is None else env, PYTHONDONTWRITEBYTECODE="1")
    try:
        proc = subprocess.run(
            spec.command, cwd=str(workdir), capture_output=True, text=True,
            timeout=spec.timeout, env=run_env,
        )
    except subprocess.TimeoutExpired as exc:
        output = (exc.stdout or "") if isinstance(exc.stdout, str) else ""
        return OracleResult(False, None, output, time.monotonic() - started,
                            [f"oracle timed out after {spec.timeout}s"])
    except OSError as exc:
        return OracleResult(False, None, str(exc), time.monotonic() - started,
                            [f"oracle could not start: {exc}"])
    output = proc.stdout + proc.stderr
    reasons = []
    if proc.returncode != 0:
        reasons.append(f"oracle exited with status {proc.returncode}")
    for pattern in spec.must_contain:
        if not re.search(pattern, output, re.MULTILINE):
            reasons.append(f"oracle output lacks required pattern: {pattern}")
    for pattern in spec.must_not_contain:
        if re.search(pattern, output, re.MULTILINE):
            reasons.append(f"oracle output contains forbidden pattern: {pattern}")
    return OracleResult(not reasons, proc.returncode, output, time.monotonic() - started, reasons)
