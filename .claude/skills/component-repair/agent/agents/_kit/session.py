"""Session commands with gates attached.

Claude sessions get the Stop and PreToolUse hooks through `--settings`. Codex has no hook
mechanism here, so Codex sessions rely on the lifecycle's VERIFY step alone.
"""
from __future__ import annotations

import json
import shlex
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Sequence

WORKBENCH = Path(__file__).resolve().parents[2]
GUARDED_TOOLS = "Write|Edit|MultiEdit|NotebookEdit|Bash|Read|Grep|Glob"


def _hook_command(*args: str) -> str:
    return " ".join(
        [f"PYTHONPATH={shlex.quote(str(WORKBENCH))}", shlex.quote(sys.executable),
         "-m", "agents._kit.hooks", *[shlex.quote(a) for a in args]]
    )


def hook_settings(
    policy_path: Path, state_path: Path, *, base: Optional[str] = None,
    guard: bool = True, stop: bool = True, stop_timeout: int = 900,
    log_path: Optional[Path] = None, workdir: Optional[Path] = None,
) -> Dict:
    """`workdir` pins the guard's root; otherwise it follows the session's reported cwd."""
    hooks: Dict[str, List] = {}
    log = ["--log", str(log_path)] if log_path else []
    pinned = ["--workdir", str(workdir)] if workdir else []
    if guard:
        hooks["PreToolUse"] = [{
            "matcher": GUARDED_TOOLS,
            "hooks": [{"type": "command", "timeout": 30,
                       "command": _hook_command("guard", "--policy", str(policy_path), *pinned, *log)}],
        }]
    if stop:
        args = ["stop", "--policy", str(policy_path), "--state", str(state_path), *log]
        if base:
            args += ["--base", base]
        hooks["Stop"] = [{
            "hooks": [{"type": "command", "timeout": stop_timeout, "command": _hook_command(*args)}],
        }]
    return {"hooks": hooks}


def write_settings(path: Path, settings: Dict) -> Path:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(settings, indent=2) + "\n", encoding="utf-8")
    return path


@dataclass
class SessionRequest:
    workdir: Path
    prompt: str
    settings_path: Optional[Path] = None
    attempt: int = 1
    interactive: bool = False
    model: Optional[str] = None
    max_budget_usd: Optional[float] = None
    add_dirs: Sequence[Path] = field(default_factory=tuple)
    allowed_tools: str = "Read,Glob,Grep,Edit,Write,MultiEdit,Bash"
    log_path: Optional[Path] = None


def claude_command(request: SessionRequest) -> List[str]:
    command = ["claude", "--allowedTools", request.allowed_tools]
    if request.settings_path:
        command += ["--settings", str(request.settings_path)]
    if request.max_budget_usd:
        command += ["--max-budget-usd", str(request.max_budget_usd)]
    for directory in request.add_dirs:
        command += ["--add-dir", str(directory)]
    if request.model:
        command += ["--model", request.model]
    if not request.interactive:
        command.append("-p")
    # --allowedTools and --add-dir are variadic; "--" stops them from swallowing the prompt.
    command += ["--", request.prompt]
    return command


def codex_command(request: SessionRequest) -> List[str]:
    command = ["codex"]
    if not request.interactive:
        command += ["exec", "--approve-for-me"]
    command += ["-s", "workspace-write", "-C", str(request.workdir)]
    for directory in request.add_dirs:
        command += ["--add-dir", str(directory)]
    if request.model:
        command += ["-m", request.model]
    command.append(request.prompt)
    return command


def session_env() -> Dict[str, str]:
    """No bytecode: a same-size edit within one second can otherwise run stale code, and the
    caches would pollute the diff."""
    import os

    return dict(os.environ, PYTHONDONTWRITEBYTECODE="1")


def launch(request: SessionRequest, provider: str = "claude") -> int:
    command = claude_command(request) if provider == "claude" else codex_command(request)
    if request.interactive or request.log_path is None:
        return subprocess.call(command, cwd=str(request.workdir), env=session_env())
    # Headless: stream the session's output to the terminal and keep a copy for the record.
    request.log_path.parent.mkdir(parents=True, exist_ok=True)
    with open(request.log_path, "w", encoding="utf-8") as log:
        proc = subprocess.Popen(command, cwd=str(request.workdir), env=session_env(),
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        for line in proc.stdout:
            sys.stdout.write(line)
            log.write(line)
        return proc.wait()
