"""T3 engine runtime: the architecture's state machine, with kit defaults for generic states.

The engine loads the lifecycle from `architecture.yaml`, so the confirmed design is exactly
what runs. Generic states have default handlers here; the agent's `handlers.py` supplies the
domain parts and may override any default:

    build_items(ctx) -> [Item | {"id": ..., "prompt": ..., "deps": [...]} | "id", ...]
                                         required for BUILD_*; `deps` only with BUILD_GRAPH
    needs_approval(ctx) -> bool          optional; default: approve every item when the spec
                                         names approval points
    fix_options_prompt(ctx) -> str       optional extra context for PROPOSE_OPTIONS
    HANDLERS = {"STATE": fn(ctx) -> condition}                             optional overrides

A queue (BUILD_QUEUE) is processed in the order `build_items` returns. A work graph
(BUILD_GRAPH) is validated and ordered by `graph.order_items`; an item starts only when its
dependencies are done, and descendants of a failed item are recorded as `blocked`. Code decides
order and readiness; the model never does. Each item runs as a gated session in its own worktree, started from the
previous item's commit, so accepted changes accumulate. Resume is at item granularity: rerun
with the same `--run-dir` and finished items are skipped.

Waves (fan-out): when the architecture declares `item_lifecycle`, NEXT_WAVE takes up to
`parallelism` ready items, RUN_WAVE runs each one's gated attempt in its own thread and
worktree from the same base commit, and MERGE cherry-picks their verified commits onto the base
in item order and re-runs the gate policy on the merged tree. A merge conflict requeues the item
once on the new base (or calls `handlers.merge(ctx, item, worktree, commit) -> bool` if the
agent defines it); a failing merged tree discards the wave's merges and drops to one item per
wave. Code decides what runs together and what is accepted.

Operator choice (`shape.operator_choice` in the spec): SELECT_ITEMS shows the built items and
skips those the operator leaves out (a skipped graph item blocks its descendants).
PROPOSE_OPTIONS asks the model for alternative fixes without implementing them, CHOOSE records
the operator's pick, and the session is told to implement only that approach. Unattended runs
take every item and the first option, recorded as automatic.
"""
from __future__ import annotations

import dataclasses
import json
import re
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Any, Callable, Dict, List, Optional

from .budget import Budget, NoProgress
from .decisions import Answer, ask, parse_selection  # noqa: F401 - parse_selection is re-exported
from .graph import newly_blocked, order_items, ready
from .ledger import Ledger
from .machine import StateMachine
from .policy import GatePolicy
from .session import SessionRequest, hook_settings, launch, write_settings
from .verify import VerifyReport, verify
from .workspace import Worktree, remove_scratch

if TYPE_CHECKING:
    from .agent import AgentHome

# Git worktree bookkeeping and operator prompts are serialized across parallel workers.
REPO_LOCK = threading.RLock()
OPERATOR_LOCK = threading.Lock()

FINISHED = ("done", "skipped", "blocked")
# In a graph an escalated item is final for the run: its descendants are already blocked and
# unattended runs continue past it. A queue stops at it, so a resumed queue retries it.
GRAPH_FINISHED = FINISHED + ("escalated",)
ITEM_KEYS = ("id", "prompt", "deps", "depends_on")


@dataclass
class Item:
    id: str
    prompt: str = ""
    data: Dict[str, Any] = field(default_factory=dict)
    deps: List[str] = field(default_factory=list)

    @classmethod
    def coerce(cls, value: Any) -> "Item":
        if isinstance(value, Item):
            return value
        if isinstance(value, str):
            return cls(value)
        deps = value.get("deps") or value.get("depends_on") or []
        return cls(str(value["id"]), value.get("prompt", ""),
                   {k: v for k, v in value.items() if k not in ITEM_KEYS}, [str(d) for d in deps])

    def record(self) -> Dict[str, Any]:
        return {"id": self.id, "prompt": self.prompt, "data": self.data, "deps": self.deps}


@dataclass
class EngineContext:
    home: "AgentHome"
    inputs: Dict[str, str]
    run_dir: Path
    ledger: Ledger
    provider: str
    model: Optional[str]
    approver: Callable[[str], bool]
    interactive: bool
    repo: Optional[Path] = None
    base: Optional[str] = None
    items: List[Item] = field(default_factory=list)
    item: Optional[Item] = None
    worktree: Optional[Worktree] = None
    policy: Optional[GatePolicy] = None
    settings: Optional[Path] = None
    report: Optional[VerifyReport] = None
    feedback: str = ""
    budget: Budget = field(default_factory=Budget)
    breaker: NoProgress = field(default_factory=NoProgress)
    state: Dict[str, Any] = field(default_factory=dict)

    def item_dir(self) -> Path:
        return self.run_dir / "items" / self.item.id.replace("/", "_")

    def worktree_dir(self) -> Path:
        """Outside the workbench, so the session cannot reach the agent's spec or evals."""
        return self.home.worktree_path(self.run_dir, "items", self.item.id)

    def mark(self, status: str, **data: Any) -> None:
        self.ledger.append("item", item=self.item.id, status=status, **data)


def _budgets(ctx: EngineContext) -> Dict[str, Any]:
    return ctx.home.contract.get("budgets") or {}


def _scheduled(ctx: EngineContext) -> bool:
    lifecycle = ctx.home.architecture.get("lifecycle") or {}
    return any(s["name"] in ("BUILD_QUEUE", "BUILD_GRAPH") for s in lifecycle.get("states", []))


def load_input(ctx: EngineContext) -> str:
    if ctx.home.runtime.get("workspace_input"):
        ctx.repo = ctx.home.workspace_repo(ctx.inputs)
        ctx.base = Worktree.attach(ctx.repo).base
    ctx.ledger.append("inputs", inputs=ctx.inputs, base=ctx.base)
    if not _scheduled(ctx) and ctx.item is None:
        # One unit of work (e.g. a long or resumable task): the whole run is a single item.
        ctx.items = [Item("task")]
        ctx.ledger.append("items", graph=False, items=[item.record() for item in ctx.items])
        ctx.item = ctx.items[0]
        ctx.budget = Budget(max_attempts=int(_budgets(ctx).get("max_iterations", 3)))
        ctx.breaker = NoProgress(2)
        ctx.mark("started")
    return "_default"


def _collect_items(ctx: EngineContext, *, graph: bool) -> str:
    domain = ctx.home.module("handlers")
    items = [Item.coerce(v) for v in domain.build_items(ctx)]
    ids = [item.id for item in items]
    if len(set(ids)) != len(ids):
        raise ValueError("build_items returned duplicate item ids")
    if graph:
        items = order_items(items)
    elif any(item.deps for item in items):
        raise ValueError("build_items returned deps, but this agent's architecture has a work queue, "
                         "not a work graph; dependencies would be ignored")
    ctx.items = items
    ctx.state["graph"] = graph
    ctx.ledger.append("items", graph=graph, items=[item.record() for item in items])
    return "_default"


def build_items(ctx: EngineContext) -> str:
    return _collect_items(ctx, graph=False)


def build_graph(ctx: EngineContext) -> str:
    return _collect_items(ctx, graph=True)


def record_blocked(ctx: EngineContext) -> Dict[str, str]:
    """Record every item that can no longer run because a dependency failed; return status."""
    status = ctx.ledger.item_status()
    while True:
        blocked = newly_blocked(ctx.items, status)
        if not blocked:
            return status
        for item, reason in blocked:
            ctx.ledger.append("item", item=item.id, status="blocked", reason=reason)
            status[item.id] = "blocked"


def next_item(ctx: EngineContext) -> str:
    status = record_blocked(ctx)
    candidates = ready(ctx.items, status, GRAPH_FINISHED if ctx.state.get("graph") else FINISHED)
    if not candidates:
        ctx.item = None
        return "empty"
    ctx.item = candidates[0]
    ctx.feedback = ""
    ctx.state.pop("chosen", None)
    ctx.state.pop("options", None)
    ctx.budget = Budget(max_attempts=int(_budgets(ctx).get("max_iterations", 3)))
    ctx.breaker = NoProgress(2)
    ctx.mark("started")
    return "has_item"


def prepare(ctx: EngineContext) -> str:
    item_dir = ctx.item_dir()
    if ctx.worktree is not None:
        ctx.worktree.remove()
    inputs = ctx.home.remap(ctx.inputs, ctx.repo, ctx.worktree_dir())
    inputs["item"] = ctx.item.id
    ctx.state["session_inputs"] = inputs
    ctx.policy = ctx.home.gate_policy(inputs)
    with REPO_LOCK:
        ctx.worktree = Worktree.create(ctx.repo, ctx.worktree_dir(), ctx.base or "HEAD",
                                       ignore=ctx.policy.ignore_paths)
    policy_path = ctx.policy.dump(item_dir / "gate_policy.json")
    ctx.settings = None
    if ctx.provider == "claude":
        ctx.settings = write_settings(item_dir / "session_settings.json", hook_settings(
            policy_path, item_dir / "stop_state.json", base=ctx.worktree.base,
            stop_timeout=(ctx.policy.oracle.timeout + 60) if ctx.policy.oracle else 120,
            log_path=ctx.run_dir / "hook_events.jsonl",
        ))
    return "_default"


def _terminal(ctx: EngineContext):
    """How to read the operator's terminal: an injected chooser (tests), stdin, or nothing."""
    import sys

    chooser = ctx.state.get("chooser")
    if chooser is not None:
        return chooser
    return input if sys.stdin.isatty() else None


def _decide(ctx: EngineContext, kind: str, question: str, options, *, default, multiple=False,
            context=None) -> Answer:
    """Ask through the decision channel; the studio may answer. Unattended runs get the default."""
    with OPERATOR_LOCK:
        return ask(ctx.run_dir, kind, question, options, default=default, multiple=multiple,
                   context=context, attended=ctx.interactive, terminal=_terminal(ctx),
                   studio=False if ctx.state.get("chooser") else None)


def select_items(ctx: EngineContext) -> str:
    """The operator chooses which built items to work on; the rest are recorded as skipped."""
    status = ctx.ledger.item_status()
    pending = [item for item in ctx.items if status.get(item.id) not in GRAPH_FINISHED]
    options = [{"id": item.id, "label": item.id + (f": {item.prompt.splitlines()[0][:100]}" if item.prompt else ""),
                "detail": item.prompt, "deps": item.deps} for item in pending]
    answer = _decide(ctx, "select_items", "Which work items should the agent work on?", options,
                     default=[item.id for item in pending], multiple=True,
                     context={"agent": ctx.home.name})
    chosen = set(answer.options)
    ctx.ledger.append("selection", selected=sorted(chosen), automatic=answer.source == "default",
                      source=answer.source)
    for item in pending:
        if item.id not in chosen:
            ctx.ledger.append("item", item=item.id, status="skipped", reason="not selected by the operator")
    return "_default"


OPTIONS_SCHEMA = {
    "type": "object",
    "required": ["options"],
    "properties": {"options": {"type": "array", "minItems": 1, "items": {
        "type": "object", "required": ["title", "approach"],
        "properties": {"title": {"type": "string"}, "approach": {"type": "string"},
                       "risk": {"type": "string"}, "files": {"type": "array", "items": {"type": "string"}}},
    }}},
}


def propose_options(ctx: EngineContext) -> str:
    """The model proposes alternative fixes for the current item without implementing any."""
    from .llm import ModelOutputError, call_json, claude_runner

    count = int((ctx.home.spec.get("shape", {}).get("operator_choice") or {}).get("options") or 3)
    domain = ctx.home.module("handlers")
    hints = getattr(domain, "fix_options_prompt", None)
    prompt = "\n".join([
        ctx.home.task_prompt({k: v for k, v in ctx.inputs.items()}).rstrip(),
        "", f"## Current item: {ctx.item.id}", "", ctx.item.prompt or "",
        "", f"## Propose up to {count} distinct ways to fix this item",
        "Do not change any file. For each option give a short title, the approach, its main risk, "
        "and the files it would touch. The operator picks one; only that one will be implemented.",
        *([str(hints(ctx))] if hints else []),
    ])
    runner = ctx.state.get("runner") or claude_runner(ctx.repo or ctx.run_dir, ctx.model)
    try:
        options = call_json(runner, prompt, OPTIONS_SCHEMA).value["options"][:count]
    except ModelOutputError:
        options = []
    ctx.item_dir().mkdir(parents=True, exist_ok=True)
    (ctx.item_dir() / "options.json").write_text(json.dumps(options, indent=2) + "\n", encoding="utf-8")
    ctx.ledger.append("options", item=ctx.item.id, options=options)
    if not options:
        ctx.state["reason"] = "the model proposed no usable fix options"
        return "no_options"
    ctx.state["options"] = options
    return "options"


def choose(ctx: EngineContext) -> str:
    """The operator picks one proposed fix, or skips the item."""
    options = ctx.state["options"]
    choices = [{"id": str(n), "key": str(n), "label": o["title"],
                "detail": o["approach"] + (f"\nrisk: {o['risk']}" if o.get("risk") else ""),
                "files": o.get("files") or []} for n, o in enumerate(options, 1)]
    choices.append({"id": "skip", "key": "s", "label": "Skip this item"})
    answer = _decide(ctx, "choose_fix", f"Which fix should the agent implement for {ctx.item.id}?",
                     choices, default="1", context={"item": ctx.item.id, "prompt": ctx.item.prompt})
    automatic = answer.source == "default"
    if answer.option == "skip":
        ctx.ledger.append("choice", item=ctx.item.id, chosen=None, automatic=automatic, source=answer.source)
        ctx.mark("skipped", reason="skipped by the operator at fix choice")
        ctx.state["result"] = ("failed", "skip_item")
        return "skip"
    index = int(answer.option) - 1
    ctx.state["chosen"] = options[index]
    ctx.ledger.append("choice", item=ctx.item.id, chosen=index + 1, title=options[index]["title"],
                      automatic=automatic, source=answer.source)
    return "chosen"


def session(ctx: EngineContext) -> str:
    ctx.budget.charge_attempt()
    (ctx.item_dir() / "stop_state.json").unlink(missing_ok=True)
    extra = f"## Current item: {ctx.item.id}\n\n{ctx.item.prompt}".rstrip()
    chosen = ctx.state.get("chosen")
    if chosen:
        extra += (f"\n\n## Approach chosen by the operator\n\n{chosen['title']}: {chosen['approach']}\n\n"
                  "Implement this approach. Do not switch to a different one; if it cannot work, say so and stop.")
    if ctx.feedback:
        extra += "\n\n## Previous attempt\n\n" + ctx.feedback
    request = SessionRequest(
        workdir=ctx.worktree.path, prompt=ctx.home.task_prompt(ctx.state["session_inputs"], extra),
        settings_path=ctx.settings, attempt=ctx.budget.attempts, model=ctx.model,
        log_path=ctx.item_dir() / f"session-{ctx.budget.attempts}.log",
    )
    code = ctx.state.get("launcher", lambda r: launch(r, ctx.provider))(request)
    ctx.ledger.append("session", item=ctx.item.id, attempt=ctx.budget.attempts, exit_code=code,
                      log=str(request.log_path))
    return "ended"


def verify_item(ctx: EngineContext) -> str:
    ctx.report = verify(ctx.policy, ctx.worktree.path, ctx.worktree, ctx.home.extra_checks())
    ctx.ledger.append("verify", item=ctx.item.id, attempt=ctx.budget.attempts, passed=ctx.report.passed,
                      failures=ctx.report.failures, changed=ctx.report.changed_files)
    if ctx.report.passed:
        return "pass"
    stalled = ctx.breaker.observe(ctx.report.signature())
    spent = ctx.budget.exhausted()
    if stalled or spent:
        ctx.state["reason"] = "no progress: the same failure repeated" if stalled else spent
        return "fail_exhausted"
    ctx.feedback = ctx.report.feedback()
    return "fail_retry"


def approve(ctx: EngineContext) -> str:
    domain = ctx.home.module("handlers")
    ask = getattr(domain, "needs_approval", None)
    if ask is not None and not ask(ctx):
        ctx.ledger.append("approval", item=ctx.item.id, approved=True, automatic=True)
        return "approved"
    changed = ", ".join(ctx.report.changed_files) or "no files"
    diff_path = ctx.item_dir() / "pending.diff"
    diff_path.write_text(ctx.worktree.diff() if ctx.worktree is not None else "", encoding="utf-8")
    question = f"Accept item {ctx.item.id}? (changed: {changed})"
    context = {"item": ctx.item.id, "changed": ctx.report.changed_files, "diff": str(diff_path)}
    with OPERATOR_LOCK:
        if getattr(ctx.approver, "accepts_context", False):
            approved = bool(ctx.approver(question, context))
        else:
            approved = bool(ctx.approver(question))
    ctx.ledger.append("approval", item=ctx.item.id, approved=approved)
    if approved:
        return "approved"
    if ctx.budget.exhausted():
        ctx.state["reason"] = "operator rejected the result and the budget is spent"
        return "rejected_exhausted"
    ctx.feedback = "The operator rejected the verified result. Revisit the approach."
    return "rejected"


def _ref_name(ctx: EngineContext, item_id: str) -> str:
    run = re.sub(r"[^A-Za-z0-9._-]", "_", ctx.run_dir.name)
    return f"{ctx.home.name}/{run}/" + re.sub(r"[^A-Za-z0-9._-]", "_", item_id)


def record(ctx: EngineContext) -> str:
    """Commit the verified item. Sequentially the commit becomes the next item's base; in a
    wave it is pinned by a ref and handed to MERGE, which decides whether it becomes `done`."""
    commit = ctx.base
    if ctx.worktree is not None:
        (ctx.item_dir() / "patch.diff").write_text(ctx.worktree.diff(), encoding="utf-8")
        with REPO_LOCK:
            commit = ctx.worktree.commit(f"{ctx.home.name}: {ctx.item.id}")
            if ctx.state.get("waves"):
                ctx.worktree.keep_ref(_ref_name(ctx, ctx.item.id), commit)
            ctx.worktree.remove()
        remove_scratch(ctx.worktree_dir().parent)
        ctx.worktree = None
    if ctx.state.get("waves"):
        ctx.mark("verified", commit=commit)
        ctx.state["result"] = ("verified", commit)
    else:
        ctx.base = commit
        ctx.mark("done", commit=commit)
    return "_default"


def escalate(ctx: EngineContext) -> str:
    """A failed item: the operator skips it or stops the run. Unattended, a queue stops (later
    items build on this one), while a graph blocks only the failed item's descendants and
    continues with independent branches."""
    reason = ctx.state.get("reason", "no viable attempt")
    graph = bool(ctx.state.get("graph"))
    choice, status = ("skip_item", "escalated") if graph else ("stop", "escalated")
    if ctx.interactive and ctx.item is not None:
        answer = _decide(ctx, "escalate", f"Item {ctx.item.id} failed ({reason}). What now?", [
            {"id": "skip_item", "key": "s", "label": "Skip this item and continue"},
            {"id": "stop", "key": "t", "label": "Stop the run"},
        ], default=choice, context={"item": ctx.item.id, "reason": reason})
        if answer.source != "default":
            choice = answer.option
            status = "skipped" if choice == "skip_item" else "escalated"
    if ctx.item is not None:
        ctx.mark(status, reason=reason)
        if ctx.worktree is not None:
            (ctx.item_dir() / "patch.diff").write_text(ctx.worktree.diff(), encoding="utf-8")
            with REPO_LOCK:
                ctx.worktree.remove()
            remove_scratch(ctx.worktree_dir().parent)
            ctx.worktree = None
    ctx.state["result"] = ("failed", choice)
    return choice


def _parallelism(ctx: EngineContext) -> int:
    return int(ctx.state.get("parallel") or ctx.home.architecture.get("parallelism") or 1)


def next_wave(ctx: EngineContext) -> str:
    """Up to `parallelism` ready items that run together from the current base."""
    if ctx.state.get("stop_requested"):
        return "empty"
    limit = _budgets(ctx).get("max_wall_seconds")
    if limit and time.monotonic() - ctx.state.get("started", time.monotonic()) >= float(limit):
        ctx.ledger.append("budget", reason=f"wall-clock budget spent ({limit}s)")
        return "empty"
    status = record_blocked(ctx)
    wave = ready(ctx.items, status, GRAPH_FINISHED)[:_parallelism(ctx)]
    if not wave:
        return "empty"
    ctx.state["wave"] = wave
    ctx.ledger.append("wave", items=[item.id for item in wave], base=ctx.base, width=_parallelism(ctx))
    return "has_wave"


def _run_item(ctx: EngineContext, item: Item) -> Any:
    """One worker: the item lifecycle on a private context that shares only the ledger."""
    child = dataclasses.replace(
        ctx, item=item, worktree=None, policy=None, settings=None, report=None, feedback="",
        budget=Budget(max_attempts=int(_budgets(ctx).get("max_iterations", 3))), breaker=NoProgress(2),
        state={k: v for k, v in ctx.state.items() if k in ("launcher", "graph", "waves", "runner", "chooser")},
    )
    child.mark("started")
    machine = StateMachine.from_architecture({"lifecycle": ctx.home.architecture["item_lifecycle"]})
    # Worker transitions stay out of the ledger: resume follows the outer machine only.
    machine.run(handlers_for(ctx.home), child)
    return item, child.state.get("result", ("failed", "stop"))


def run_wave(ctx: EngineContext) -> str:
    wave = ctx.state.pop("wave")
    with ThreadPoolExecutor(max_workers=len(wave), thread_name_prefix="item") as pool:
        results = list(pool.map(lambda item: _run_item(ctx, item), wave))
    ctx.state["wave_results"] = results
    if any(outcome == ("failed", "stop") for _, outcome in results):
        ctx.state["stop_requested"] = True
    return "_default"


def _conflicts(ctx: EngineContext, item_id: str) -> int:
    return sum(1 for r in ctx.ledger.records()
               if r["event"] == "item" and r.get("item") == item_id and r.get("status") == "conflict")


def merge(ctx: EngineContext) -> str:
    """Apply the wave's verified commits onto the base in item order, then re-verify the result."""
    results = ctx.state.pop("wave_results", [])
    verified = [(item, outcome[1]) for item, outcome in results if outcome[0] == "verified"]
    if not verified:
        return "_default"
    custom = getattr(ctx.home.module("handlers"), "merge", None)
    path = ctx.home.worktree_path(ctx.run_dir, "merge")
    with REPO_LOCK:
        tree = Worktree.create(ctx.repo, path, ctx.base or "HEAD",
                               ignore=ctx.home.gate_policy(ctx.inputs).ignore_paths)
    merged: List[Item] = []
    try:
        for item, commit in verified:
            how = "cherry-pick" if tree.cherry_pick(commit) else None
            if how is None and custom is not None and custom(ctx, item, tree, commit):
                tree.commit(f"{ctx.home.name}: {item.id} (merged by handlers.merge)")
                how = "handlers.merge"
            ctx.ledger.append("merge", item=item.id, commit=commit, merged=bool(how), how=how)
            if how:
                merged.append(item)
            elif _conflicts(ctx, item.id) == 0:
                ctx.ledger.append("item", item=item.id, status="conflict",
                                  reason="merge conflict; requeued to run on the merged base")
            else:
                ctx.ledger.append("item", item=item.id, status="escalated",
                                  reason="merge conflict again after running on the merged base")
        if merged:
            inputs = ctx.home.remap(ctx.inputs, ctx.repo, tree.path)
            report = verify(ctx.home.gate_policy(inputs), tree.path, tree, ctx.home.extra_checks())
            ctx.ledger.append("merge_verify", items=[i.id for i in merged], passed=report.passed,
                              failures=report.failures)
            if report.passed:
                ctx.base = tree.head()
                for item in merged:
                    ctx.ledger.append("item", item=item.id, status="done", commit=ctx.base)
            elif _parallelism(ctx) > 1:
                ctx.state["parallel"] = 1
                ctx.ledger.append("parallelism", width=1, reason="merged wave failed verification")
                for item in merged:
                    ctx.ledger.append("item", item=item.id, status="requeued",
                                      reason="the merged wave failed verification; rerunning one at a time")
            else:
                for item in merged:
                    ctx.ledger.append("item", item=item.id, status="escalated",
                                      reason="verified alone but failed after merging")
    finally:
        with REPO_LOCK:
            tree.remove()
            for item, _ in verified:
                tree.drop_ref(f"refs/agent-runs/{_ref_name(ctx, item.id)}")
        remove_scratch(path.parent)
    return "_default"


def _item_summary(ctx: EngineContext, item: Item, status: Dict[str, str]) -> Dict[str, Any]:
    summary: Dict[str, Any] = {"id": item.id, "status": status.get(item.id, "pending")}
    if item.deps:
        summary["deps"] = item.deps
    last = ctx.ledger.last("item", item=item.id)
    if last and last.get("reason"):
        summary["reason"] = last["reason"]
    return summary


def report(ctx: EngineContext) -> str:
    status = ctx.ledger.item_status()
    summary = {
        "agent": ctx.home.name, "tier": "T3", "final_commit": ctx.base,
        "items": [_item_summary(ctx, item, status) for item in ctx.items],
        "status": "passed" if ctx.items and all(status.get(i.id) == "done" for i in ctx.items) else "failed",
    }
    (ctx.run_dir / "result.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    ctx.state["summary"] = summary
    return "_default"


DEFAULT_HANDLERS: Dict[str, Callable[[EngineContext], str]] = {
    "LOAD_INPUT": load_input, "BUILD_GRAPH": build_graph, "BUILD_QUEUE": build_items,
    "NEXT_ITEM": next_item, "PREPARE": prepare, "SESSION": session, "VERIFY": verify_item,
    "APPROVE": approve, "RECORD": record, "ESCALATE": escalate, "REPORT": report,
    "NEXT_WAVE": next_wave, "RUN_WAVE": run_wave, "MERGE": merge,
    "SELECT_ITEMS": select_items, "PROPOSE_OPTIONS": propose_options, "CHOOSE": choose,
}
DOMAIN_REQUIRED = {"BUILD_GRAPH": "build_items", "BUILD_QUEUE": "build_items"}


def handlers_for(home: "AgentHome") -> Dict[str, Callable[[EngineContext], str]]:
    handlers = dict(DEFAULT_HANDLERS)
    domain = home.module("handlers")
    handlers.update(getattr(domain, "HANDLERS", {}) if domain else {})
    return handlers


def engine_problems(home: "AgentHome") -> List[str]:
    """Every non-terminal state needs a handler, and BUILD_* states need build_items."""
    try:
        machines = [StateMachine.from_architecture(home.architecture)]
        if home.architecture.get("item_lifecycle"):
            machines.append(StateMachine.from_architecture({"lifecycle": home.architecture["item_lifecycle"]}))
    except (KeyError, TypeError, ValueError) as exc:
        return [f"architecture.yaml lifecycle is invalid: {exc}"]
    domain = home.module("handlers")
    overrides = getattr(domain, "HANDLERS", {}) if domain else {}
    problems = []
    states = {state: machine for machine in machines for state in machine.states - set(machine.terminal)}
    for state, machine in sorted(states.items()):
        if state in overrides:
            continue
        required = DOMAIN_REQUIRED.get(state)
        if required and not (domain and hasattr(domain, required)):
            problems.append(f"handlers.py must define {required}() for {state}")
        elif state not in DEFAULT_HANDLERS and machine.conditions(state) != ("_default",):
            problems.append(f"state {state} has no handler; add it to HANDLERS in handlers.py")
    return problems


def run_engine(
    home: "AgentHome", inputs: Dict[str, str], run_dir: Path, *, provider: str, model: Optional[str],
    approver: Callable[[str], bool], interactive: bool, launcher: Optional[Callable] = None,
    apply: bool = False, runner: Optional[Callable[[str], str]] = None,
    chooser: Optional[Callable[[str], str]] = None,
) -> int:
    run_dir = Path(run_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    ledger = Ledger(run_dir / "ledger.jsonl")
    machine = StateMachine.from_architecture(home.architecture)
    ctx = EngineContext(home, inputs, run_dir, ledger, provider, model, approver, interactive)
    if launcher is not None:
        ctx.state["launcher"] = launcher
    ctx.state["waves"] = bool(home.architecture.get("item_lifecycle"))
    if runner is not None:
        ctx.state["runner"] = runner
    if chooser is not None:
        ctx.state["chooser"] = chooser
    ctx.state["started"] = time.monotonic()
    start = None
    recorded = ledger.last("items")
    if recorded and StateMachine.resume_state(ledger) is not None:
        # Resume at item granularity: same items, same order, finished items skipped.
        ctx.items = [Item(i["id"], i.get("prompt", ""), i.get("data", {}), i.get("deps") or [])
                     for i in recorded["items"]]
        ctx.state["graph"] = bool(recorded.get("graph"))
        load_input(ctx)
        done = [r for r in ledger.records() if r["event"] == "item" and r.get("status") == "done"]
        if done:
            ctx.base = done[-1].get("commit") or ctx.base
        narrowed = ledger.last("parallelism")
        if narrowed:
            ctx.state["parallel"] = narrowed["width"]
        if ctx.state["waves"]:
            start = "NEXT_WAVE"
        elif _scheduled(ctx):
            start = "NEXT_ITEM"
        else:
            # A single task restarts from LOAD_INPUT unless it already finished.
            finished = ledger.item_status().get("task") in FINISHED
            ctx.items = [] if not finished else ctx.items
            start = "REPORT" if finished else None
        ledger.append("resumed", base=ctx.base)
    machine.run(handlers_for(home), ctx, ledger=ledger, start=start)
    summary = ctx.state.get("summary") or {"status": "failed"}
    if apply and summary["status"] == "passed" and ctx.repo is not None and ctx.base:
        Worktree.attach(ctx.repo).fast_forward(ctx.base)
    print(f"{home.name}: {summary['status']}")
    return 0 if summary["status"] == "passed" else 1
