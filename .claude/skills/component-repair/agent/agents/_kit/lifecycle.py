"""The T2 gated-session lifecycle: PREPARE -> SESSION -> VERIFY [-> APPROVE] -> PUBLISH.

The model works however it likes inside SESSION. Code owns everything around it: the isolated
worktree, the hooks, the authoritative VERIFY, retries with failure feedback, the budget and
no-progress breaker, operator approval, and publishing.
"""
from __future__ import annotations

import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, List, Optional, Tuple

from .approval import Approver, deny_all, request_approval
from .budget import Budget, NoProgress
from .ledger import Ledger
from .machine import StateMachine
from .policy import GatePolicy
from .session import SessionRequest, hook_settings, launch, write_settings
from .verify import ExtraCheck, VerifyReport, verify
from .workspace import Worktree

Launcher = Callable[[SessionRequest], int]
Publisher = Callable[[Worktree, VerifyReport], Optional[str]]


def gated_edges(approval: bool, on_pass: str = "PUBLISH", on_exhausted: str = "FAIL") -> List[Tuple[str, str, str]]:
    edges = [("PREPARE", "_default", "SESSION"), ("SESSION", "ended", "VERIFY")]
    if approval:
        edges += [("VERIFY", "pass", "APPROVE"), ("APPROVE", "approved", on_pass),
                  ("APPROVE", "rejected", "SESSION"), ("APPROVE", "rejected_exhausted", on_exhausted)]
    else:
        edges.append(("VERIFY", "pass", on_pass))
    edges += [("VERIFY", "fail_retry", "SESSION"), ("VERIFY", "fail_exhausted", on_exhausted)]
    if on_pass == "PUBLISH":
        edges.append(("PUBLISH", "_default", "DONE"))
    return edges


@dataclass
class GatedTask:
    name: str
    prompt: str
    policy: GatePolicy
    repo: Path
    base_ref: str = "HEAD"
    max_sessions: int = 3
    no_progress_limit: int = 2
    max_wall_seconds: Optional[float] = None
    approval_prompt: Optional[str] = None
    provider: str = "claude"
    model: Optional[str] = None
    max_budget_usd: Optional[float] = None
    keep_worktree: bool = True
    extra_checks: Optional[ExtraCheck] = None
    worktree_dir: Optional[Path] = None  # default: <run_dir>/worktree


@dataclass
class GatedResult:
    status: str
    reason: str
    worktree: Optional[Worktree]
    reports: List[VerifyReport] = field(default_factory=list)
    published: Optional[str] = None


@dataclass
class _Context:
    task: GatedTask
    run_dir: Path
    ledger: Ledger
    launcher: Launcher
    approver: Approver
    publisher: Optional[Publisher]
    budget: Budget
    breaker: NoProgress
    worktree: Optional[Worktree] = None
    settings: Optional[Path] = None
    feedback: str = ""
    reports: List[VerifyReport] = field(default_factory=list)
    reason: str = ""
    published: Optional[str] = None


def _prepare(ctx: _Context) -> str:
    task = ctx.task
    ctx.worktree = Worktree.create(task.repo, task.worktree_dir or ctx.run_dir / "worktree", task.base_ref,
                                   ignore=task.policy.ignore_paths)
    policy_path = task.policy.dump(ctx.run_dir / "gate_policy.json")
    if task.provider == "claude":
        ctx.settings = write_settings(ctx.run_dir / "session_settings.json", hook_settings(
            policy_path, ctx.run_dir / "stop_state.json", base=ctx.worktree.base,
            log_path=ctx.run_dir / "hook_events.jsonl",
            stop_timeout=(task.policy.oracle.timeout + 60) if task.policy.oracle else 120,
        ))
    ctx.ledger.append("prepared", worktree=str(ctx.worktree.path), base=ctx.worktree.base)
    return "_default"


def _session(ctx: _Context) -> str:
    ctx.budget.charge_attempt()
    # Each session gets its own Stop-hook budget; the count must not carry over.
    (ctx.run_dir / "stop_state.json").unlink(missing_ok=True)
    prompt = ctx.task.prompt
    if ctx.feedback:
        prompt += "\n\n## Previous attempt\n\n" + ctx.feedback
    request = SessionRequest(
        workdir=ctx.worktree.path, prompt=prompt, settings_path=ctx.settings,
        attempt=ctx.budget.attempts, model=ctx.task.model, max_budget_usd=ctx.task.max_budget_usd,
        log_path=ctx.run_dir / f"session-{ctx.budget.attempts}.log",
    )
    started = time.monotonic()
    code = ctx.launcher(request)
    ctx.ledger.append("session", attempt=ctx.budget.attempts, exit_code=code,
                      seconds=round(time.monotonic() - started, 1), log=str(request.log_path))
    return "ended"


def _verify(ctx: _Context) -> str:
    report = verify(ctx.task.policy, ctx.worktree.path, ctx.worktree, ctx.task.extra_checks)
    ctx.reports.append(report)
    ctx.ledger.append("verify", attempt=ctx.budget.attempts, passed=report.passed,
                      failures=report.failures, changed=report.changed_files)
    if report.passed:
        return "pass"
    stalled = ctx.breaker.observe(report.signature())
    spent = ctx.budget.exhausted()
    if stalled or spent:
        ctx.reason = "no progress: the same failure repeated" if stalled else spent
        return "fail_exhausted"
    ctx.feedback = report.feedback()
    return "fail_retry"


def _approve(ctx: _Context) -> str:
    prompt = ctx.task.approval_prompt or f"Accept the verified result of {ctx.task.name}?"
    changed = ", ".join(ctx.reports[-1].changed_files) or "no files"
    if request_approval(f"{prompt} (changed: {changed})", ctx.approver, ctx.ledger):
        return "approved"
    spent = ctx.budget.exhausted()
    if spent:
        ctx.reason = f"operator rejected the result and the {spent}"
        return "rejected_exhausted"
    ctx.feedback = "The operator rejected the verified result. Revisit the approach."
    return "rejected"


def _publish(ctx: _Context) -> str:
    if ctx.publisher is not None:
        ctx.published = ctx.publisher(ctx.worktree, ctx.reports[-1])
    ctx.ledger.append("published", ref=ctx.published)
    return "_default"


def run_gated(
    task: GatedTask,
    run_dir: Path,
    *,
    launcher: Optional[Launcher] = None,
    approver: Approver = deny_all,
    publisher: Optional[Publisher] = None,
) -> GatedResult:
    run_dir = Path(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    ledger = Ledger(run_dir / "ledger.jsonl")
    ctx = _Context(
        task=task, run_dir=run_dir, ledger=ledger,
        launcher=launcher or (lambda request: launch(request, task.provider)),
        approver=approver, publisher=publisher,
        budget=Budget(max_attempts=task.max_sessions, max_wall_seconds=task.max_wall_seconds),
        breaker=NoProgress(task.no_progress_limit),
    )
    machine = StateMachine.from_edges(
        gated_edges(approval=task.approval_prompt is not None), "PREPARE", ("DONE", "FAIL"),
    )
    handlers = {"PREPARE": _prepare, "SESSION": _session, "VERIFY": _verify,
                "APPROVE": _approve, "PUBLISH": _publish}
    ledger.append("started", task=task.name, max_sessions=task.max_sessions)
    final = machine.run(handlers, ctx, ledger=ledger)
    if final == "FAIL" and ctx.worktree is not None and not task.keep_worktree:
        ctx.worktree.remove()
    status = "passed" if final == "DONE" else "failed"
    return GatedResult(status, ctx.reason if status == "failed" else "verified", ctx.worktree,
                       ctx.reports, ctx.published)
