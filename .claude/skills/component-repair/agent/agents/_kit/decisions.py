"""One way to ask a person: code poses a question with fixed options; the terminal or the studio answers.

Every decision point (confirm the spec, promote, select items, choose a fix, approve an item,
escalate) calls `ask`. It writes the question to `<run_dir>/decisions/<id>.pending.json`. While
the Builder Studio is open (`./build_agent studio`, heartbeat under `agents/.studio/`), the
page shows it and may answer by writing `<id>.answer.json`. The terminal can answer at the same
time; the first valid answer wins. An answer must name an offered option (and carry text only
where the option asks for it), so the page can do nothing the terminal could not. The resolved
question, with its answer and source, stays on disk as `<id>.json`.

Unattended runs (no terminal and no studio, or the caller says so) get the default at once,
recorded as `source: default`.
"""
from __future__ import annotations

import itertools
import json
import os
import select
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Sequence

# A heartbeat older than this means the studio is gone.
STUDIO_STALE_SECONDS = 10.0
POLL_SECONDS = 0.4
Terminal = Callable[[str], str]
_counter = itertools.count(1)


@dataclass
class Answer:
    option: Optional[str]            # single-choice answer: the option id
    options: List[str] = field(default_factory=list)   # multiple-choice answer: option ids
    text: str = ""
    source: str = "default"          # terminal | studio | default


def workbench_of(path: Path) -> Optional[Path]:
    path = Path(path).resolve()
    return next((p for p in [path, *path.parents] if (p / "agents" / "_kit").is_dir()), None)


def studio_dir(workbench: Path) -> Path:
    """The studio's local state (heartbeats, jobs, action log); untracked."""
    return Path(workbench) / "agents" / ".studio"


def heartbeat_path(workbench: Path, studio_id: str) -> Path:
    """One heartbeat file per running studio. A shared file would let a studio that stops delete
    the heartbeat of another still running, and builds would briefly act as if nobody can answer."""
    return studio_dir(workbench) / "heartbeats" / f"{studio_id}.json"


def _fresh(path: Path) -> bool:
    try:
        beat = json.loads(path.read_text(encoding="utf-8"))
        return time.time() - float(beat["time"]) < STUDIO_STALE_SECONDS
    except (OSError, ValueError, KeyError, TypeError):
        return False


def studio_present(near: Path) -> bool:
    """True if any studio for the workbench containing `near` sent a heartbeat recently."""
    workbench = workbench_of(near)
    if workbench is None:
        return False
    beats = list((studio_dir(workbench) / "heartbeats").glob("*.json"))
    # `heartbeat` is where studios before per-studio files wrote theirs; one may still be running.
    return any(_fresh(path) for path in beats + [studio_dir(workbench) / "heartbeat"])


def parse_selection(answer: str, count: int) -> List[int]:
    """'all', '', 'none', or '1,3-5' -> zero-based indexes. Raises ValueError on anything else."""
    answer = answer.strip().lower()
    if answer in ("", "all", "a"):
        return list(range(count))
    if answer in ("none", "n"):
        return []
    chosen = set()
    for part in answer.replace(" ", "").split(","):
        low, _, high = part.partition("-")
        start, end = int(low), int(high or low)
        if not 1 <= start <= end <= count:
            raise ValueError(f"{part} is outside 1-{count}")
        chosen.update(range(start - 1, end))
    return sorted(chosen)


def _write_json(path: Path, data: Dict[str, Any]) -> None:
    tmp = path.with_name(path.name + f".{os.getpid()}.tmp")
    tmp.write_text(json.dumps(data, indent=2, default=str) + "\n", encoding="utf-8")
    os.replace(tmp, path)


def validate(pending: Dict[str, Any], answer: Dict[str, Any]) -> Optional[str]:
    """Why an answer is not acceptable for a pending question, or None if it is."""
    ids = [o["id"] for o in pending["options"]]
    if pending.get("multiple"):
        chosen = answer.get("options")
        if not isinstance(chosen, list) or any(c not in ids for c in chosen):
            return "options must be a list of offered option ids"
        return None
    option = answer.get("option")
    if option not in ids:
        return f"option must be one of {ids}"
    needs = next(o for o in pending["options"] if o["id"] == option).get("needs_text")
    text = answer.get("text") or ""
    if needs and not str(text).strip():
        return f"option {option!r} needs a note"
    if text and not needs:
        return f"option {option!r} takes no text"
    return None


def _terminal_prompt(pending: Dict[str, Any]) -> str:
    lines = [pending["question"]]
    if pending.get("multiple"):
        lines += [f"  {n}. {o['label']}" for n, o in enumerate(pending["options"], 1)]
        return "\n".join(lines) + "\nSelect (e.g. 1,3-5, all, none) [all]: "
    default = pending.get("default")
    choices = []
    for o in pending["options"]:
        key = o.get("key") or o["id"]
        choices.append(f"[{key}] {o['label']}")
    marker = next((o.get("key") or o["id"] for o in pending["options"] if o["id"] == default), "")
    return "\n".join(lines) + "\n  " + ", ".join(choices) + (f" [{marker}]" if marker else "") + ": "


def _parse_terminal(pending: Dict[str, Any], line: str) -> Dict[str, Any]:
    """A terminal line as an answer dict (validated afterwards). Raises ValueError if unreadable."""
    line = line.strip()
    options = pending["options"]
    if pending.get("multiple"):
        return {"options": [options[i]["id"] for i in parse_selection(line, len(options))]}
    if not line:
        return {"option": pending.get("default")}
    lowered = line.lower()
    for n, o in enumerate(options, 1):
        if lowered in (str(o.get("key") or "").lower(), o["id"].lower(), str(n)):
            return {"option": o["id"]}
    raise ValueError(f"{line!r} is not one of the choices")


def _resolve(root: Path, pending: Dict[str, Any], answer: Answer) -> Answer:
    record = dict(pending, answer={"option": answer.option, "options": answer.options, "text": answer.text},
                  source=answer.source, answered=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
    _write_json(root / f"{pending['id']}.json", record)
    for suffix in (".pending.json", ".answer.json"):
        (root / f"{pending['id']}{suffix}").unlink(missing_ok=True)
    return answer


def _answer(data: Dict[str, Any], multiple: bool, source: str) -> Answer:
    if multiple:
        return Answer(None, list(data.get("options") or []), "", source)
    return Answer(data.get("option"), [], str(data.get("text") or ""), source)


def ask(
    run_dir: Path, kind: str, question: str, options: Sequence[Dict[str, Any]], *,
    default: Any, multiple: bool = False, context: Optional[Dict[str, Any]] = None,
    attended: bool = True, terminal: Optional[Terminal] = None, studio: Optional[bool] = None,
) -> Answer:
    """Ask a person and return a validated answer.

    options: [{"id", "label", "detail"?, "key"? (terminal shortcut), "needs_text"?, "text_prompt"?}]
    default: an option id (or a list of ids when `multiple`), used when nobody is there to answer.
    terminal: how to read the terminal (e.g. `input`), or None if there is no terminal.
    studio: whether the studio may answer; by default, whether one is open for this workbench.
    """
    root = Path(run_dir) / "decisions"
    root.mkdir(parents=True, exist_ok=True)
    qid = f"{time.strftime('%Y%m%d-%H%M%S')}-{next(_counter):03d}-{kind}"
    pending = {
        "id": qid, "kind": kind, "question": question, "multiple": multiple,
        "options": [dict(o) for o in options], "default": default, "context": context or {},
        "created": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "pid": os.getpid(),
    }
    studio = studio_present(run_dir) if studio is None else studio
    if not attended or (terminal is None and not studio):
        data = {"options": list(default)} if multiple else {"option": default}
        return _resolve(root, pending, _answer(data, multiple, "default"))
    _write_json(root / f"{qid}.pending.json", pending)

    def from_terminal(line: str) -> Optional[Answer]:
        try:
            data = _parse_terminal(pending, line)
        except ValueError as exc:
            print(f"  {exc}", file=sys.stderr)
            return None
        chosen = next((o for o in pending["options"] if o["id"] == data.get("option")), None)
        if chosen and chosen.get("needs_text"):
            data["text"] = terminal(chosen.get("text_prompt") or "Note: ").strip()
        problem = validate(pending, data)
        if problem:
            print(f"  {problem}", file=sys.stderr)
            return None
        return _answer(data, multiple, "terminal")

    if not studio:
        while True:
            answer = from_terminal(terminal(_terminal_prompt(pending)))
            if answer is not None:
                return _resolve(root, pending, answer)

    # The studio may answer; so may the terminal, if it is a real one we can poll.
    real_tty = terminal is input and sys.stdin.isatty()
    prompt = _terminal_prompt(pending)
    print(prompt if real_tty else f"{question}\n  (waiting for an answer in the Builder Studio)",
          end="" if real_tty else "\n", flush=True)
    answer_file = root / f"{qid}.answer.json"
    rejected = 0
    while True:
        if answer_file.is_file():
            try:
                data = json.loads(answer_file.read_text(encoding="utf-8"))
                problem = validate(pending, data)
            except (OSError, ValueError) as exc:
                data, problem = {}, f"unreadable answer: {exc}"
            if problem is None:
                answer = _answer(data, multiple, "studio")
                if real_tty:
                    print(f"\n  (answered in the Builder Studio: {answer.option or answer.options})")
                return _resolve(root, pending, answer)
            rejected += 1
            os.replace(answer_file, root / f"{qid}.rejected-{rejected}.json")
        if real_tty:
            ready, _, _ = select.select([sys.stdin], [], [], POLL_SECONDS)
            if ready:
                answer = from_terminal(sys.stdin.readline())
                if answer is not None:
                    return _resolve(root, pending, answer)
                print(prompt, end="", flush=True)
        else:
            time.sleep(POLL_SECONDS)


def board_approver(run_dir: Path, *, attended: bool) -> Callable[..., bool]:
    """An approver (prompt -> bool) that asks through the decision channel; default: deny."""
    def approve(prompt: str, context: Optional[Dict[str, Any]] = None) -> bool:
        answer = ask(run_dir, "approve", prompt, [
            {"id": "approve", "label": "Approve", "key": "y"},
            {"id": "reject", "label": "Reject", "key": "n"},
        ], default="reject", context=context, attended=attended,
            terminal=input if sys.stdin.isatty() else None)
        return answer.option == "approve"
    approve.accepts_context = True  # type: ignore[attr-defined]
    return approve
