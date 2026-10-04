"""After-approval action: open the pull request. Code performs it; the session never can.

Called by handlers.record only for attended runs, after the operator approved the verified
repair (the approval question says that approving opens a pull request). Evaluation cases never
reach it. Set REPAIR_AGENT_NO_PR=1 to keep the verified commit local instead.

The branch is pushed from the engine's verified commit, and the PR body is the review the
operator approved (classification, root cause, chosen approach, before/after gate runs).
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
from pathlib import Path
from typing import Any, Dict


def _run(argv, cwd: Path) -> subprocess.CompletedProcess:
    return subprocess.run(argv, cwd=str(cwd), capture_output=True, text=True, timeout=300)


def after_publish(home: Any, result: Dict[str, Any], inputs: Dict[str, str]) -> Dict[str, Any]:
    if os.environ.get("REPAIR_AGENT_NO_PR"):
        return {"pr": None, "reason": "REPAIR_AGENT_NO_PR is set"}
    commit, repo = result.get("commit"), Path(inputs["repository"])
    if not commit:
        return {"pr": None, "reason": "no verified commit"}
    if shutil.which("gh") is None:
        return {"pr": None, "reason": "gh is not installed", "commit": commit}
    base = _run(["git", "rev-parse", "--abbrev-ref", "HEAD"], repo).stdout.strip()
    if not base or base == "HEAD":
        return {"pr": None, "reason": "the repository is on a detached HEAD; no base branch for the PR",
                "commit": commit}
    oid = inputs["obligation_id"]
    branch = "repair/" + re.sub(r"[^a-z0-9._-]", "-", oid.lower()) + "-" + commit[:8]
    push = _run(["git", "push", "origin", f"{commit}:refs/heads/{branch}"], repo)
    if push.returncode != 0:
        return {"pr": None, "reason": f"git push failed: {push.stderr.strip()[-300:]}", "commit": commit}
    verdict = result.get("verdict") or {}
    title = f"repair({inputs['component']}): {oid} ({verdict.get('classification', 'code-wrong')})"
    body = Path(result["review"]).read_text(encoding="utf-8") if result.get("review") else title
    for label, name in (("gate before", "gate_before"), ("gate after", "gate_after")):
        path = Path(result[name]) if result.get(name) else None
        if path is not None and path.is_file():
            body += (f"\n\n<details><summary>{label}</summary>\n\n```\n"
                     f"{path.read_text(encoding='utf-8')[-6000:]}\n```\n</details>\n")
    pr = _run(["gh", "pr", "create", "--base", base, "--head", branch, "--title", title, "--body", body], repo)
    if pr.returncode != 0:
        return {"pr": None, "branch": branch, "reason": f"gh pr create failed: {pr.stderr.strip()[-300:]}"}
    return {"pr": pr.stdout.strip(), "branch": branch, "commit": commit}
