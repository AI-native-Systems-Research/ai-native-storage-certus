"""Claude Code hook entry points. Exit status 2 blocks; stderr becomes the reason shown to Claude.

    python -m agents._kit.hooks guard --policy P            # PreToolUse
    python -m agents._kit.hooks stop  --policy P --state S  # Stop

`--workdir` defaults to the hook input's `cwd`. `--base` enables the diff guard in the Stop
hook. Any internal error in the guard blocks rather than silently allowing (fail closed).
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any, Dict, Optional

from .guard import check_tool_call
from .policy import GatePolicy
from .verify import verify
from .workspace import Worktree, WorkspaceError

BLOCK = 2


def _log(args: argparse.Namespace, record: Dict[str, Any]) -> None:
    """Append a hook decision to the run's hook log, if one was given. Never raises."""
    if not getattr(args, "log", None):
        return
    try:
        record = {"time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), **record}
        with open(args.log, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(record, sort_keys=True, default=str) + "\n")
    except OSError:
        pass


def _read_event() -> Dict[str, Any]:
    raw = sys.stdin.read()
    return json.loads(raw) if raw.strip() else {}


def _workdir(args: argparse.Namespace, event: Dict[str, Any]) -> Path:
    return Path(args.workdir or event.get("cwd") or ".").resolve()


def guard_main(args: argparse.Namespace) -> int:
    try:
        event = _read_event()
        policy = GatePolicy.load(Path(args.policy))
        violations = check_tool_call(
            str(event.get("tool_name", "")), event.get("tool_input") or {}, policy, _workdir(args, event)
        )
    except Exception as exc:  # fail closed: a broken guard must not wave edits through
        print(f"guard hook error, blocking: {exc}", file=sys.stderr)
        _log(args, {"hook": "guard", "decision": "error", "error": str(exc)})
        return BLOCK
    if violations:
        _log(args, {"hook": "guard", "decision": "block", "tool": event.get("tool_name"),
                    "input": event.get("tool_input"), "violations": violations})
        print("Blocked by agent policy:\n" + "\n".join(f"- {v}" for v in violations), file=sys.stderr)
        return BLOCK
    return 0


def _load_state(path: Path) -> Dict[str, Any]:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def stop_main(args: argparse.Namespace) -> int:
    state_path = Path(args.state)
    state = _load_state(state_path)
    blocks = int(state.get("stop_blocks", 0))
    report, error, limit = None, "", 0
    try:
        policy = GatePolicy.load(Path(args.policy))
        limit = policy.max_stop_blocks
        event = _read_event()
        workdir = _workdir(args, event)
        worktree: Optional[Worktree] = (
            Worktree.attach(workdir, args.base, ignore=policy.ignore_paths) if args.base else None
        )
        report = verify(policy, workdir, worktree)
    except (WorkspaceError, OSError, ValueError) as exc:
        error = str(exc)

    def save(**update: Any) -> None:
        state.update(update)
        state_path.parent.mkdir(parents=True, exist_ok=True)
        state_path.write_text(json.dumps(state), encoding="utf-8")

    failures = report.failures if report is not None else [f"verification could not run: {error}"]
    if report is not None and report.passed:
        save(last="passed")
        _log(args, {"hook": "stop", "decision": "allow", "blocks": blocks})
        return 0
    if blocks >= limit:
        # Budget spent: let the session end; the lifecycle's VERIFY records the failure.
        save(last="gave_up")
        _log(args, {"hook": "stop", "decision": "gave_up", "blocks": blocks, "failures": failures})
        return 0
    save(stop_blocks=blocks + 1, last="blocked")
    _log(args, {"hook": "stop", "decision": "block", "blocks": blocks + 1, "failures": failures})
    message = report.feedback() if report is not None else f"Verification could not run: {error}"
    print(message + "\n\nKeep working until verification passes. Do not weaken the checks.",
          file=sys.stderr)
    return BLOCK


def main(argv: Optional[list] = None) -> int:
    parser = argparse.ArgumentParser(prog="agents._kit.hooks")
    sub = parser.add_subparsers(dest="hook", required=True)
    guard = sub.add_parser("guard")
    guard.add_argument("--policy", required=True)
    guard.add_argument("--workdir")
    guard.add_argument("--log")
    stop = sub.add_parser("stop")
    stop.add_argument("--policy", required=True)
    stop.add_argument("--state", required=True)
    stop.add_argument("--workdir")
    stop.add_argument("--base")
    stop.add_argument("--log")
    args = parser.parse_args(argv)
    return guard_main(args) if args.hook == "guard" else stop_main(args)


if __name__ == "__main__":
    raise SystemExit(main())
