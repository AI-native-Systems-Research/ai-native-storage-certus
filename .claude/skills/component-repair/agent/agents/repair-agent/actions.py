"""Actions code performs after the operator approves; the session never holds these tools.

`open_pull_request` pushes the verified commit to a new branch and opens a pull request whose
body is the session's report (the four plain-English parts) followed by the acceptance check's
before/after evidence. handlers.RECORD calls it only in attended runs, after APPROVE.
Evaluation runs never reach it. Set REPAIR_AGENT_NO_PR=1 to keep the result local.
"""
from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path
from typing import Any, Dict


def _run(repo: Path, *argv: str) -> subprocess.CompletedProcess:
    return subprocess.run(list(argv), cwd=str(repo), capture_output=True, text=True, timeout=300)


def _pr_base(repo: Path, commit: str) -> str:
    """The branch the repair was cut from, when the checkout is on it; else the remote default."""
    branch = _run(repo, "git", "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
    if branch and branch != "HEAD":
        parent = _run(repo, "git", "rev-parse", f"{commit}^").stdout.strip()
        tip = _run(repo, "git", "rev-parse", branch).stdout.strip()
        if parent and parent == tip:
            return branch
    return ""


def open_pull_request(repo: Path, commit: str, inputs: Dict[str, str], item_dir: Path) -> Dict[str, Any]:
    if os.environ.get("REPAIR_AGENT_NO_PR"):
        return {"opened": False, "reason": "REPAIR_AGENT_NO_PR is set"}
    oid = inputs.get("obligation_id") or "repair"
    branch = "repair/" + re.sub(r"[^a-z0-9._-]+", "-", oid.lower()) + "-" + commit[:8]
    report = Path(item_dir) / "report.md"
    packet = Path(item_dir) / "approval.md"
    body = report.read_text(encoding="utf-8") if report.is_file() else f"Repair of {oid}."
    if packet.is_file():
        evidence = packet.read_text(encoding="utf-8").split("## Acceptance check", 1)
        if len(evidence) == 2:
            body += "\n\n## Acceptance check" + evidence[1]
    body_file = Path(item_dir) / "pr_body.md"
    body_file.write_text(body, encoding="utf-8")
    push = _run(repo, "git", "push", "origin", f"{commit}:refs/heads/{branch}")
    if push.returncode != 0:
        return {"opened": False, "branch": branch, "error": "git push failed: " + push.stderr.strip()[-500:]}
    argv = ["gh", "pr", "create", "--head", branch, "--title", f"Repair {oid} ({inputs.get('component')})",
            "--body-file", str(body_file)]
    base = _pr_base(repo, commit)
    if base:
        argv += ["--base", base]
    pr = _run(repo, *argv)
    if pr.returncode != 0:
        return {"opened": False, "branch": branch, "error": "gh pr create failed: " + pr.stderr.strip()[-500:]}
    return {"opened": True, "branch": branch, "url": pr.stdout.strip().splitlines()[-1] if pr.stdout.strip() else ""}
