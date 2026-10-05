"""Anti-gaming and isolation rules, checked per tool call (fast fail) and per diff (authoritative).

The PreToolUse hook uses `check_tool_call` to stop a violation before it happens. It cannot see
everything (a shell command can edit any file), so VERIFY always re-runs `check_changes` over
the actual diff; that result is the one that counts.
"""
from __future__ import annotations

import functools
import re
import shlex
from pathlib import Path, PurePosixPath
from typing import Any, Dict, Iterable, List, Optional, Tuple

from .policy import GatePolicy

WRITE_TOOLS = {"Write", "Edit", "MultiEdit", "NotebookEdit"}
READ_TOOLS = {"Read", "Grep", "Glob", "NotebookRead"}
# Shell commands that walk a directory tree; searching an ancestor of a hidden path reaches it.
RECURSIVE_SEARCH = re.compile(r"\b(grep\s+(-\w*\s+)*-\w*[rR]\w*|rg|ag|ack|find|git\s+grep|tree|du|ls\s+(-\w*\s+)*-\w*R\w*)\b")


@functools.lru_cache(maxsize=512)
def _glob_regex(pattern: str) -> "re.Pattern[str]":
    """`*` and `?` stay inside one path component; `**` spans components; `dir/` means `dir/**`."""
    if pattern.endswith("/"):
        pattern += "**"
    out, i = "", 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out, i = out + "(?:.*/)?", i + 3
        elif pattern.startswith("/**", i) and i + 3 == len(pattern):
            out, i = out + "(?:/.*)?", i + 3
        elif pattern.startswith("**", i):
            out, i = out + ".*", i + 2
        elif pattern[i] == "*":
            out, i = out + "[^/]*", i + 1
        elif pattern[i] == "?":
            out, i = out + "[^/]", i + 1
        else:
            out, i = out + re.escape(pattern[i]), i + 1
    return re.compile(out + r"\Z")


def path_matches(relative: str, pattern: str) -> bool:
    """Glob match on a workdir-relative posix path. A pattern without `/` matches any basename."""
    relative = PurePosixPath(relative).as_posix()
    if _glob_regex(pattern).match(relative):
        return True
    return "/" not in pattern and bool(_glob_regex(pattern).match(PurePosixPath(relative).name))


def is_ignored(relative: str, policy: GatePolicy) -> bool:
    return any(path_matches(relative, pattern) for pattern in policy.ignore_paths)


def _relative(path: str, workdir: Path) -> Optional[str]:
    candidate = Path(path)
    if not candidate.is_absolute():
        candidate = workdir / candidate
    try:
        return candidate.resolve().relative_to(workdir.resolve()).as_posix()
    except ValueError:
        return None


def check_path(path: str, policy: GatePolicy, workdir: Path) -> List[str]:
    relative = _relative(path, workdir)
    if relative is None:
        return [f"write outside the workdir is not allowed: {path}"] if policy.confine_writes else []
    return _path_violations(relative, policy)


def _path_violations(relative: str, policy: GatePolicy) -> List[str]:
    violations = [
        f"{relative} is protected (rule: {pattern})"
        for pattern in policy.protected_paths if path_matches(relative, pattern)
    ]
    if policy.writable_paths and not any(path_matches(relative, p) for p in policy.writable_paths):
        violations.append(f"{relative} is not writable (allowed: {', '.join(policy.writable_paths)})")
    return violations


def _literal_prefix(pattern: str) -> str:
    """The directory part of a glob before its first wildcard."""
    head = re.split(r"[*?\[]", pattern, maxsplit=1)[0]
    return head.rstrip("/") if head.endswith("/") or head == pattern else str(PurePosixPath(head).parent)


def _absolute(path: str, workdir: Path) -> str:
    candidate = Path(path).expanduser()
    if not candidate.is_absolute():
        candidate = workdir / candidate
    return Path(candidate).resolve().as_posix()


def _is_within(path: str, ancestor: str) -> bool:
    return path == ancestor or path.startswith(ancestor.rstrip("/") + "/")


def check_read(path: str, policy: GatePolicy, workdir: Path, *, recursive: bool) -> List[str]:
    """A read of a hidden path, or a recursive search rooted at one of its ancestors."""
    if not policy.hidden_paths:
        return []
    target = _absolute(path, workdir)
    for pattern in policy.hidden_paths:
        prefix = _literal_prefix(pattern)
        if path_matches(target, pattern):
            return [f"{path} is off-limits to this session (rule: {pattern})"]
        if recursive and _is_within(prefix, target):
            return [f"searching {path} would reach {prefix}, which is off-limits; search a narrower path"]
    return []


def _check_shell_reads(command: str, policy: GatePolicy, workdir: Path) -> List[str]:
    if not policy.hidden_paths:
        return []
    try:
        words = shlex.split(command, posix=True)
    except ValueError:
        words = command.split()
    recursive = bool(RECURSIVE_SEARCH.search(command))
    violations: List[str] = []
    for word in words:
        if "/" not in word and word not in (".", ".."):
            continue
        for part in re.split(r"[=:]", word):
            if part and ("/" in part or part in (".", "..")):
                violations += check_read(part, policy, workdir, recursive=recursive)
    return sorted(set(violations))


def check_text(text: str, policy: GatePolicy, where: str = "") -> List[str]:
    suffix = f" in {where}" if where else ""
    return [
        f"forbidden pattern {pattern!r}{suffix}"
        for pattern in policy.forbidden_patterns if re.search(pattern, text, re.MULTILINE)
    ]


def _tool_writes(tool_input: Dict[str, Any]) -> Tuple[List[str], List[str]]:
    paths = [tool_input[key] for key in ("file_path", "notebook_path", "path")
             if isinstance(tool_input.get(key), str)]
    texts = [tool_input[key] for key in ("content", "new_string", "new_source")
             if isinstance(tool_input.get(key), str)]
    for edit in tool_input.get("edits") or []:
        if isinstance(edit, dict) and isinstance(edit.get("new_string"), str):
            texts.append(edit["new_string"])
    return paths, texts


def check_tool_call(tool_name: str, tool_input: Dict[str, Any], policy: GatePolicy, workdir: Path) -> List[str]:
    violations: List[str] = []
    if tool_name == "Bash":
        command = str(tool_input.get("command", ""))
        violations += [
            ("command is not on the allowed list of read-only commands" if pattern.startswith("^(?!")
             else f"command matches forbidden rule {pattern!r}")
            for pattern in policy.forbidden_commands if re.search(pattern, command)
        ]
        return violations + _check_shell_reads(command, policy, workdir)
    if tool_name in READ_TOOLS:
        paths = [tool_input[key] for key in ("file_path", "notebook_path", "path")
                 if isinstance(tool_input.get(key), str)]
        recursive = tool_name in ("Grep", "Glob")
        if recursive and not paths:
            paths = [str(workdir)]
        for path in paths:
            violations += check_read(path, policy, workdir, recursive=recursive)
        if tool_name == "Glob" and isinstance(tool_input.get("pattern"), str) and tool_input["pattern"].startswith("/"):
            violations += check_read(_literal_prefix(tool_input["pattern"]), policy, workdir, recursive=True)
        return violations
    if tool_name in WRITE_TOOLS or tool_name.startswith("mcp__"):
        paths, texts = _tool_writes(tool_input)
        for path in paths:
            violations += check_path(path, policy, workdir)
        where = paths[0] if paths else tool_name
        for text in texts:
            violations += check_text(text, policy, where)
    return violations


def check_changes(changes: Iterable[Tuple[str, str]], policy: GatePolicy) -> List[str]:
    """changes: (workdir-relative path, added text) pairs from the real diff."""
    violations: List[str] = []
    for relative, added in changes:
        if is_ignored(relative, policy):
            continue
        violations += _path_violations(relative, policy)
        violations += check_text(added, policy, relative)
    return violations
