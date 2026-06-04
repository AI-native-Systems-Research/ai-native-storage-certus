"""GEPA-compatible evaluator for the pipeline bakeoff.

Wraps the existing evaluate.py infrastructure (backup/restore, build, bench,
integrity check) into the signature GEPA's optimize_anything expects:

    evaluate(candidate: dict[str, str]) -> tuple[float, dict]

The candidate dict maps filenames (e.g. "pipeline.rs", "lib.rs") to source code.
Files are written to a temp directory and passed to the existing evaluator.
"""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path

import gepa.optimize_anything as oa

EVALUATOR_DIR = Path(__file__).resolve().parent
EVALUATE_MODULE = EVALUATOR_DIR / "evaluate.py"

# Import the existing evaluator's evaluate() function
import importlib.util

_spec = importlib.util.spec_from_file_location("bakeoff_evaluator", EVALUATE_MODULE)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)
_evaluate_skydiscover = _mod.evaluate


def evaluate(candidate: dict[str, str]) -> tuple[float, dict]:
    """Evaluate a multi-file candidate via the existing bakeoff evaluator.

    Args:
        candidate: Mapping of filename -> source code content.
                   Keys should match MULTI_FILE_MAP in evaluate.py
                   (e.g. "pipeline.rs", "lib.rs", "service.rs").

    Returns:
        (score, side_info) where score is throughput in GB/s and
        side_info contains feedback and diagnostics.
    """
    with tempfile.TemporaryDirectory(prefix="gepa_candidate_") as tmpdir:
        tmppath = Path(tmpdir)

        for filename, content in candidate.items():
            (tmppath / filename).write_text(content)

        result = _evaluate_skydiscover(str(tmppath))

    score = result.get("combined_score", 0.0)
    feedback = result.get("artifacts", {}).get("feedback", "")

    oa.log(feedback)

    return score, {"feedback": feedback}
