"""Schema-validated headless model call with a bounded repair retry (T1 and engine phases).

The runner is injected, so the same logic serves Claude, Codex, or a test double. Output that
fails the schema is never passed on: it is repaired once or twice with the exact errors, then
the call fails.
"""
from __future__ import annotations

import json
import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

Runner = Callable[[str], str]


class ModelOutputError(RuntimeError):
    def __init__(self, message: str, attempts: List[str]):
        super().__init__(message)
        self.attempts = attempts


@dataclass
class JsonResult:
    value: Any
    calls: int
    raw: List[str] = field(default_factory=list)


def extract_json(text: str) -> Any:
    fenced = re.findall(r"```(?:json)?\s*\n(.*?)```", text, re.DOTALL)
    for candidate in fenced + [text]:
        try:
            return json.loads(candidate.strip())
        except ValueError:
            continue
    start = min((i for i in (text.find("{"), text.find("[")) if i >= 0), default=-1)
    if start >= 0:
        end = max(text.rfind("}"), text.rfind("]"))
        if end > start:
            return json.loads(text[start:end + 1])
    raise ValueError("no JSON value found in model output")


def _errors(value: Any, schema: Dict[str, Any]) -> List[str]:
    import jsonschema

    validator = jsonschema.Draft202012Validator(schema)
    return [
        f"{'.'.join(str(p) for p in error.absolute_path) or '<root>'}: {error.message}"
        for error in sorted(validator.iter_errors(value), key=str)
    ]


def call_json(runner: Runner, prompt: str, schema: Dict[str, Any], *, max_repairs: int = 1) -> JsonResult:
    raw: List[str] = []
    request = (
        prompt + "\n\nRespond with a single JSON value that validates against this schema:\n"
        + json.dumps(schema, indent=2)
    )
    for _ in range(max_repairs + 1):
        output = runner(request)
        raw.append(output)
        try:
            value = extract_json(output)
            problems = _errors(value, schema)
        except ValueError as exc:
            problems = [str(exc)]
        if not problems:
            return JsonResult(value, len(raw), raw)
        request = (
            prompt + "\n\nYour previous answer was rejected by the validator:\n"
            + "\n".join(f"- {p}" for p in problems)
            + "\n\nRespond again with a single JSON value that validates against this schema:\n"
            + json.dumps(schema, indent=2)
        )
    raise ModelOutputError(f"model output failed validation after {len(raw)} call(s)", raw)


def claude_runner(cwd: Path, model: Optional[str] = None, timeout: int = 900) -> Runner:
    def run(prompt: str) -> str:
        command = ["claude", "-p", "--output-format", "json"]
        if model:
            command += ["--model", model]
        proc = subprocess.run(command + [prompt], cwd=str(cwd), capture_output=True, text=True,
                              timeout=timeout)
        try:
            envelope = json.loads(proc.stdout)
            if isinstance(envelope, dict) and "result" in envelope:
                return str(envelope["result"])
        except ValueError:
            pass
        return proc.stdout
    return run
