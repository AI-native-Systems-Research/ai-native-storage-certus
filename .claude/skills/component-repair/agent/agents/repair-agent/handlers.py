"""Domain handlers for the repair agent (tier T3). See agents/_kit/engine.py.

The architecture runs one unit of work (one refuted obligation, plus any extra targets the
operator names) through LOAD_INPUT -> PREPARE -> SESSION -> VERIFY -> APPROVE -> RECORD -> REPORT.
The kit's handlers do the generic work; these wrap them with the domain parts code must own:

LOAD_INPUT  validate the operator's inputs, derive `targets` from component/obligation_id/
            callee_modules when it is not given, pin the worktree to `base_rev`, and refuse an
            `oracle_dir` inside the repository (the agent must not be able to edit its own check).
PREPARE     recreate the untracked `components/<c>/creusot` toolchain link in the worktree, so the
            proof crates resolve creusot-std there as they do in the operator's checkout.
VERIFY      record the target obligations' statement/source/traces hashes (base vs now) in the
            ledger; a report that classifies the finding as not code-wrong goes straight to the
            operator instead of looping; otherwise the kit's VERIFY (diff guard, checks.py, oracle).
APPROVE     the operator sees the classification, the PR text and the oracle's before/after
            evidence before anything irreversible happens.
RECORD      keep the hand-off report out of the commit, commit, pin the result with a ref, and
            (attended runs only, after approval) open the pull request through actions.py.
ESCALATE    keep the session's report and diff for the operator.
REPORT      add the classification, hashes, evidence and PR to result.json.
"""
from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path
from typing import Any, Dict, List, Optional

from agents._kit import engine

REPORT_DIR = ".repair"
REPORT_FILE = f"{REPORT_DIR}/report.md"
CLASSIFICATIONS = ("code-wrong", "spec-wrong", "both", "spec-unimplementable", "withdrawn")
# Classifications that mean "do not change the code": the operator decides what happens next.
NO_CODE_FIX = ("spec-wrong", "spec-unimplementable", "withdrawn")
CLASSIFICATION_LINE = re.compile(r"^\s*\**\s*classification\s*\**\s*:\s*\**\s*`?([a-z-]+)`?", re.I | re.M)
TARGET = re.compile(r"^[A-Za-z0-9._-]+:[A-Z0-9][A-Z0-9_-]*:[A-Za-z0-9_]+(,[A-Za-z0-9_]+)*$")


# ---- inputs ------------------------------------------------------------------------------------

def parse_targets(text: str) -> List[Dict[str, Any]]:
    targets = []
    for part in (p.strip().replace(" ", "") for p in str(text).split(";")):
        if not part:
            continue
        if not TARGET.match(part):
            raise ValueError(f"target {part!r} is not component:OBLIGATION-ID:callee,modules")
        component, oid, modules = part.split(":", 2)
        targets.append({"component": component, "obligation_id": oid, "callee_modules": modules.split(",")})
    if not targets:
        raise ValueError("no targets: give targets=component:OBLIGATION-ID:callee,modules, or "
                         "component, obligation_id and callee_modules")
    return targets


def derive_inputs(inputs: Dict[str, str], repo: Path) -> Dict[str, str]:
    """The run inputs with defaults filled in; raises ValueError on inputs code cannot accept."""
    out = dict(inputs)
    out.setdefault("mode", "bug")
    if out["mode"] not in ("bug", "discordance"):
        raise ValueError(f"mode must be bug or discordance, not {out['mode']!r}")
    if out["mode"] == "discordance" and not out.get("discordance_id"):
        raise ValueError("mode=discordance needs discordance_id")
    if not out.get("targets") or "{" in out["targets"]:
        missing = [k for k in ("component", "obligation_id", "callee_modules") if not out.get(k)]
        if missing:
            raise ValueError("targets is not given and cannot be derived: missing " + ", ".join(missing))
        out["targets"] = f"{out['component']}:{out['obligation_id']}:{out['callee_modules'].replace(' ', '')}"
    targets = parse_targets(out["targets"])
    out.setdefault("component", targets[0]["component"])
    out.setdefault("obligation_id", targets[0]["obligation_id"])
    out.setdefault("callee_modules", ",".join(targets[0]["callee_modules"]))
    out.setdefault("bundle", f"components/{out['component']}/verif/unified_properties.yaml")
    for target in targets:
        if not (repo / "components" / target["component"]).is_dir():
            raise ValueError(f"no component directory components/{target['component']} in {repo}")
    if not out.get("base_rev"):
        raise ValueError("base_rev is required: the regression legs compare against it")
    oracle_dir = Path(out.get("oracle_dir") or "").expanduser()
    if not out.get("oracle_dir") or not (oracle_dir / "repair_accept_repo.sh").is_file():
        raise ValueError(f"oracle_dir {out.get('oracle_dir')!r} does not hold repair_accept_repo.sh")
    oracle_dir = oracle_dir.resolve()
    if oracle_dir == repo or repo in oracle_dir.parents:
        raise ValueError("oracle_dir is inside the repository; the check the agent is judged by must "
                         "live outside its worktree")
    out["oracle_dir"] = str(oracle_dir)
    return out


def load_input(ctx) -> str:
    condition = engine.load_input(ctx)
    derived = derive_inputs(ctx.inputs, ctx.repo)
    proc = subprocess.run(["git", "rev-parse", "--verify", f"{derived['base_rev']}^{{commit}}"],
                          cwd=str(ctx.repo), capture_output=True, text=True)
    if proc.returncode != 0:
        raise ValueError(f"base_rev {derived['base_rev']!r} is not a commit in {ctx.repo}")
    base = proc.stdout.strip()
    # Resume keeps the base of the last finished item; a fresh run starts at base_rev.
    if not ctx.ledger.last("item", status="done"):
        ctx.base = base
    added = {k: v for k, v in derived.items() if ctx.inputs.get(k) != v}
    ctx.inputs.update(derived)
    ctx.state["targets"] = parse_targets(derived["targets"])
    ctx.ledger.append("derived_inputs", inputs=added, base=ctx.base, targets=ctx.state["targets"])
    return condition


def build_items(ctx) -> List[Dict[str, Any]]:
    """One item per operator-named target, in the order the operator listed them. The
    architecture runs the whole repair as one unit, so this is used only by a queue variant."""
    targets = ctx.state.get("targets") or parse_targets(derive_inputs(ctx.inputs, ctx.repo)["targets"])
    return [{"id": f"{t['component']}:{t['obligation_id']}",
             "prompt": f"Repair {t['obligation_id']} in components/{t['component']} "
                       f"(callee modules: {', '.join(t['callee_modules'])}).", **t} for t in targets]


def needs_approval(ctx) -> bool:
    """Every result is approved: the next step opens a pull request against a shared repository."""
    return True


# ---- worktree setup ----------------------------------------------------------------------------

def _toolchain_target(repo: Path, component: str) -> Optional[Path]:
    link = repo / "components" / component / "creusot"
    if link.exists():
        return link.resolve()
    shared = repo / "tools" / "creusot" / "creusot"
    return shared.resolve() if shared.exists() else None


def link_toolchains(repo: Path, worktree: Path) -> List[str]:
    """Give every proof crate in the worktree the `creusot` link the checkout has (untracked, so
    a fresh worktree lacks it); returns the links created."""
    created = []
    for crate in sorted((worktree / "components").glob("*/verif-creusot")):
        component = crate.parent.name
        link = crate.parent / "creusot"
        target = _toolchain_target(repo, component)
        if target is not None and not link.exists() and not link.is_symlink():
            link.symlink_to(target)
            created.append(f"components/{component}/creusot")
    return created


def prepare(ctx) -> str:
    condition = engine.prepare(ctx)
    created = link_toolchains(ctx.repo, ctx.worktree.path)
    ctx.state["toolchain_links"] = created
    ctx.ledger.append("toolchain_links", item=ctx.item.id, created=created)
    return condition


# ---- the session's hand-off report -------------------------------------------------------------

def read_report(worktree_path: Path) -> Dict[str, Any]:
    path = Path(worktree_path) / REPORT_FILE
    if not path.is_file():
        return {"present": False, "classification": None, "text": ""}
    text = path.read_text(encoding="utf-8", errors="replace")
    m = CLASSIFICATION_LINE.search(text)
    value = m.group(1).lower() if m else None
    return {"present": True, "classification": value if value in CLASSIFICATIONS else None, "text": text}


def _save_handoff(ctx) -> Dict[str, Any]:
    """Copy the report out of the worktree into the item's run directory."""
    report = read_report(ctx.worktree.path) if ctx.worktree is not None else {"present": False, "text": ""}
    if report.get("present"):
        ctx.item_dir().mkdir(parents=True, exist_ok=True)
        (ctx.item_dir() / "report.md").write_text(report["text"], encoding="utf-8")
    ctx.state["handoff"] = report
    return report


def _remove_scaffolding(ctx) -> None:
    """What the engine or the hand-off put in the worktree that must not be committed."""
    root = Path(ctx.worktree.path)
    for relative in ctx.state.get("toolchain_links") or []:
        link = root / relative
        if link.is_symlink():
            link.unlink()
    handoff = root / REPORT_DIR
    if handoff.is_dir():
        for path in sorted(handoff.rglob("*"), reverse=True):
            path.unlink() if not path.is_dir() else path.rmdir()
        handoff.rmdir()


# ---- VERIFY ------------------------------------------------------------------------------------

def _checks():
    from agents._kit.agent import _import  # the agent's checks.py, loaded the way VERIFY loads it

    return _import(Path(__file__).resolve().parent, "checks")


def record_obligation_hashes(ctx) -> Dict[str, Any]:
    ids = [t["obligation_id"] for t in ctx.state.get("targets") or []]
    hashes = _checks().obligation_hashes(ctx.worktree, ids)
    flat = {oid: {**pair, "unchanged": pair["base"] == pair["now"], "bundle": bundle}
            for bundle, entries in hashes.items() for oid, pair in entries.items()}
    for oid in ids:
        flat.setdefault(oid, {"base": None, "now": None, "unchanged": None, "bundle": None})
    ctx.ledger.append("obligation_hashes", item=ctx.item.id, attempt=ctx.budget.attempts, hashes=flat)
    ctx.state["obligation_hashes"] = flat
    return flat


def verify(ctx) -> str:
    record_obligation_hashes(ctx)
    report = _save_handoff(ctx)
    classification = report.get("classification")
    if classification in NO_CODE_FIX:
        ctx.state["reason"] = (f"classified {classification}: the code is not what must change; "
                               "the report goes to the operator")
        ctx.ledger.append("classification", item=ctx.item.id, classification=classification, escalated=True)
        return "fail_exhausted"
    condition = engine.verify_item(ctx)
    if condition == "pass" and not report.get("present"):
        # Code verified the fix, but the reviewer has no PR text: send the session back for it.
        ctx.ledger.append("verify_note", item=ctx.item.id, note=f"{REPORT_FILE} missing")
        if ctx.budget.exhausted():
            ctx.state["reason"] = f"verified, but no {REPORT_FILE} for the reviewer and the budget is spent"
            return "fail_exhausted"
        ctx.feedback = (f"The checks passed, but {REPORT_FILE} is missing. Write it (classification "
                        "line and the four parts) without changing anything else, then stop.")
        return "fail_retry"
    return condition


# ---- APPROVE -----------------------------------------------------------------------------------

def approval_packet(ctx) -> str:
    report = ctx.state.get("handoff") or {}
    oracle = ctx.report.oracle.output if ctx.report is not None and ctx.report.oracle else ""
    hashes = ctx.state.get("obligation_hashes") or {}
    lines = [f"# Repair of {ctx.inputs.get('obligation_id')} — approval packet", "",
             f"Classification: {report.get('classification') or 'not stated'}",
             f"Changed files: {', '.join(ctx.report.changed_files) if ctx.report else 'unknown'}", "",
             "## Obligation hashes (statement, source, traces)", ""]
    lines += [f"- {oid}: {'unchanged' if v.get('unchanged') else 'CHANGED'}" for oid, v in hashes.items()]
    lines += ["", "## Pull request text (written by the session)", "", report.get("text") or "(missing)", "",
              "## Acceptance check, re-run by code after the session (before/after)", "", "```",
              oracle.strip(), "```", ""]
    return "\n".join(lines)


def approve(ctx) -> str:
    packet = ctx.item_dir() / "approval.md"
    packet.write_text(approval_packet(ctx), encoding="utf-8")
    diff_path = ctx.item_dir() / "pending.diff"
    diff_path.write_text(ctx.worktree.diff(), encoding="utf-8")
    report = ctx.state.get("handoff") or {}
    question = (f"Open a pull request for the repair of {ctx.inputs.get('obligation_id')}? "
                f"Classification: {report.get('classification') or 'not stated'}. "
                f"Read {packet} (PR text and before/after gate runs) and {diff_path}.")
    context = {"item": ctx.item.id, "classification": report.get("classification"),
               "changed": ctx.report.changed_files, "approval_packet": str(packet), "diff": str(diff_path)}
    with engine.OPERATOR_LOCK:
        if getattr(ctx.approver, "accepts_context", False):
            approved = bool(ctx.approver(question, context))
        else:
            approved = bool(ctx.approver(question))
    ctx.ledger.append("approval", item=ctx.item.id, approved=approved, packet=str(packet))
    if approved:
        return "approved"
    if ctx.budget.exhausted():
        ctx.state["reason"] = "operator rejected the result and the budget is spent"
        return "rejected_exhausted"
    ctx.feedback = ("The operator rejected the verified result. Revisit the approach, and make the "
                    "reasoning in the report clearer for a reviewer who does not read Rust.")
    return "rejected"


# ---- RECORD / ESCALATE / REPORT ----------------------------------------------------------------

def record(ctx) -> str:
    _save_handoff(ctx)
    _remove_scaffolding(ctx)
    condition = engine.record(ctx)
    commit = ctx.base
    run = re.sub(r"[^A-Za-z0-9._-]", "_", ctx.run_dir.name)
    ref = f"refs/agent-runs/{ctx.home.name}/{run}"
    subprocess.run(["git", "update-ref", ref, commit], cwd=str(ctx.repo), capture_output=True, text=True)
    ctx.ledger.append("result_ref", item=ctx.item.id, ref=ref, commit=commit)
    ctx.state["result_ref"] = ref
    # The pull request is the irreversible step: only an attended run, after the operator approved.
    if ctx.interactive:
        actions = ctx.home.module("actions")
        if actions is not None and hasattr(actions, "open_pull_request"):
            outcome = actions.open_pull_request(ctx.repo, commit, ctx.inputs, ctx.item_dir())
            ctx.ledger.append("publish", item=ctx.item.id, **outcome)
            ctx.state["publish"] = outcome
    return condition


def escalate(ctx) -> str:
    if ctx.worktree is not None:
        _save_handoff(ctx)
    return engine.escalate(ctx)


def report(ctx) -> str:
    condition = engine.report(ctx)
    summary = ctx.state.get("summary") or {}
    handoff = ctx.state.get("handoff") or {}
    hashes = ctx.state.get("obligation_hashes") or {}
    summary.update({
        "obligation_id": ctx.inputs.get("obligation_id"),
        "targets": ctx.inputs.get("targets"),
        "base_rev": ctx.inputs.get("base_rev"),
        "classification": handoff.get("classification"),
        "obligation_hashes": hashes or None,
        # None when no VERIFY ran (nothing was measured), else whether every target is unchanged.
        "obligation_statement_unchanged": all(v.get("unchanged") is True for v in hashes.values()) if hashes else None,
        "report": str(ctx.item_dir() / "report.md") if ctx.item and handoff.get("present") else None,
        "result_ref": ctx.state.get("result_ref"),
        "pull_request": ctx.state.get("publish"),
    })
    if ctx.report is not None and ctx.report.oracle is not None:
        summary["evidence"] = ctx.report.oracle.tail(40)
    (ctx.run_dir / "result.json").write_text(json.dumps(summary, indent=2, default=str) + "\n", encoding="utf-8")
    ctx.state["summary"] = summary
    return condition


HANDLERS: Dict[str, Any] = {
    "LOAD_INPUT": load_input,
    "PREPARE": prepare,
    "VERIFY": verify,
    "APPROVE": approve,
    "RECORD": record,
    "ESCALATE": escalate,
    "REPORT": report,
}
