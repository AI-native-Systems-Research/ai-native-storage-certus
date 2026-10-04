"""Anti-gaming checks that gate_policy.yaml cannot express. They can only add failures.

gate_policy.yaml already protects the bundle, the specifications, the interfaces and the proof
crate's build files, and forbids writing verdicts. These checks cover the rest, comparing the
worktree with the revision it started from (`worktree.base`):

1. Every `verify_` / `refute_` / `lemma_` function that existed at base is unchanged: same
   contract attributes, signature and body (comments and whitespace aside). New proof functions
   may be added; the mirror's model functions (`arena_*` etc.) may be edited.
2. Every obligation's `statement`, `source` and `traces` in each `unified_properties.yaml` hash
   the same as at base (defence in depth behind the protected path).
3. Changes stay inside one component's `src/` and `verif-creusot/src/` (plus the prover's
   regenerated `verif-creusot/verif/` output).
4. Added code does not move the failure instead of removing it (new panics, aborts), skip tests,
   compile differently under test, or assume a proof obligation away with `#[trusted]`.

`handlers.py` reuses `proof_functions` and `obligation_fields` to assemble the session context.
"""
from __future__ import annotations

import hashlib
import re
import subprocess
from pathlib import Path, PurePosixPath
from typing import Dict, List, Optional

import yaml

PROOF_FN = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?fn\s+((?:verify|refute|lemma)_[A-Za-z0-9_]*)\b")
LINE_COMMENT = re.compile(r"//.*$")
FROZEN_FIELDS = ("statement", "source", "traces")

COMPONENT_PATH = re.compile(r"^components/([^/]+)/(.+)$")
ALLOWED_IN_COMPONENT = ("src/", "verif-creusot/src/", "verif-creusot/verif/")

# Added lines in shipped source (not the repair's own test file) that move a crash instead of
# removing it, or make tests and proofs see different code.
SRC_FORBIDDEN = [
    (re.compile(r"\b(panic|unreachable|unimplemented|todo)!\s*\("), "adds a panic-family macro"),
    (re.compile(r"\bprocess::(abort|exit)\s*\("), "adds a process abort/exit"),
    (re.compile(r"#\[\s*ignore\b"), "marks a test #[ignore]"),
    (re.compile(r"cfg\s*\(\s*not\s*\(\s*test\s*\)"), "adds cfg(not(test)) code the tests never see"),
]
TEST_FORBIDDEN = [
    (re.compile(r"#\[\s*ignore\b"), "marks a test #[ignore]"),
]
PROOF_FORBIDDEN = [
    (re.compile(r"#\[\s*trusted\b"), "adds #[trusted], which assumes a contract instead of proving it"),
    (re.compile(r"\bassume!\s*\("), "adds assume!, which removes a proof obligation"),
]


def _git(repo: Path, *args: str) -> Optional[str]:
    proc = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True)
    return proc.stdout if proc.returncode == 0 else None


def _normalize(text: str) -> str:
    lines = [LINE_COMMENT.sub("", line) for line in text.splitlines()]
    return re.sub(r"\s+", " ", "\n".join(lines)).strip()


def proof_functions(text: str) -> Dict[str, List[str]]:
    """name -> source text of each verify_/refute_/lemma_ fn, with the attributes above it.

    The attribute block is the contiguous run of non-blank, non-comment lines directly above the
    `fn` line (this is where `#[requires]` / `#[ensures]` live). The body runs to the matching
    closing brace."""
    lines = text.splitlines()
    found: Dict[str, List[str]] = {}
    for index, line in enumerate(lines):
        match = PROOF_FN.match(line)
        if not match:
            continue
        start = index
        while start > 0:
            above = lines[start - 1].strip()
            if not above or above.startswith("//") or above == "}":
                break
            start -= 1
        depth, opened, end = 0, False, index
        for end in range(index, len(lines)):
            code = LINE_COMMENT.sub("", lines[end])
            depth += code.count("{") - code.count("}")
            opened = opened or "{" in code
            if opened and depth <= 0:
                break
            if not opened and code.rstrip().endswith(";"):
                break  # a declaration without a body
        found.setdefault(match.group(1), []).append("\n".join(lines[start:end + 1]))
    return found


def _digest(text: str) -> str:
    return hashlib.sha256(_normalize(text).encode()).hexdigest()


def _proof_hashes(texts: Dict[str, str]) -> Dict[str, List[str]]:
    hashes: Dict[str, List[str]] = {}
    for _, text in sorted(texts.items()):
        for name, bodies in proof_functions(text).items():
            hashes.setdefault(name, []).extend(_digest(body) for body in bodies)
    return {name: sorted(values) for name, values in hashes.items()}


def _base_files(repo: Path, base: str, pattern: re.Pattern) -> Dict[str, str]:
    listing = _git(repo, "ls-tree", "-r", "--name-only", base, "--", "components") or ""
    files = {}
    for rel in listing.splitlines():
        if pattern.match(rel):
            content = _git(repo, "show", f"{base}:{rel}")
            if content is not None:
                files[rel] = content
    return files


def _current_files(workdir: Path, pattern: re.Pattern) -> Dict[str, str]:
    files = {}
    components = workdir / "components"
    if not components.is_dir():
        return files
    for path in components.glob("*/verif-creusot/src/**/*.rs"):
        rel = path.relative_to(workdir).as_posix()
        if pattern.match(rel):
            files[rel] = path.read_text(encoding="utf-8", errors="replace")
    return files


PROOF_SRC = re.compile(r"^components/[^/]+/verif-creusot/src/.+\.rs$")
BUNDLE = re.compile(r"^components/[^/]+/verif/unified_properties\.yaml$")


def _crate(rel: str) -> str:
    return rel.split("/verif-creusot/", 1)[0]


def check_proof_functions(repo: Path, base: str, workdir: Path) -> List[str]:
    before_files = _base_files(repo, base, PROOF_SRC)
    after_files = _current_files(workdir, PROOF_SRC)
    failures = []
    for crate in sorted({_crate(rel) for rel in before_files}):
        before = _proof_hashes({r: t for r, t in before_files.items() if _crate(r) == crate})
        after = _proof_hashes({r: t for r, t in after_files.items() if _crate(r) == crate})
        for name, hashes in sorted(before.items()):
            if name not in after:
                failures.append(f"{crate}: proof function {name} existed at base and is gone (deleted or "
                                "renamed); existing verify_/refute_/lemma_ functions are frozen")
            elif after[name] != hashes:
                failures.append(f"{crate}: proof function {name} changed; existing verify_/refute_/lemma_ "
                                "functions are frozen — add a new function instead")
    return failures


def obligation_fields(text: str) -> Dict[str, Dict[str, object]]:
    """id -> {statement, source, traces} for every property in a unified_properties.yaml."""
    data = yaml.safe_load(text) or {}
    properties = data.get("properties") if isinstance(data, dict) else None
    out: Dict[str, Dict[str, object]] = {}
    for prop in properties or []:
        if isinstance(prop, dict) and prop.get("id"):
            out[str(prop["id"])] = {name: prop.get(name) for name in FROZEN_FIELDS}
    return out


def _field_hash(value: object) -> str:
    return hashlib.sha256(yaml.safe_dump(value, sort_keys=True).encode()).hexdigest()


def check_obligations(repo: Path, base: str, workdir: Path) -> List[str]:
    failures = []
    for rel, before_text in sorted(_base_files(repo, base, BUNDLE).items()):
        path = workdir / rel
        if not path.is_file():
            failures.append(f"{rel} was deleted; the bundle is frozen")
            continue
        after_text = path.read_text(encoding="utf-8", errors="replace")
        if after_text == before_text:
            continue
        try:
            before, after = obligation_fields(before_text), obligation_fields(after_text)
        except yaml.YAMLError as exc:
            failures.append(f"{rel} no longer parses ({exc}); the bundle is frozen")
            continue
        for oid, fields in sorted(before.items()):
            for name in FROZEN_FIELDS:
                if oid not in after or _field_hash(after[oid].get(name)) != _field_hash(fields.get(name)):
                    failures.append(f"{rel}: {oid}.{name} hash changed; obligation statements, sources "
                                    "and traces are frozen")
    return failures


def check_scope(changes: List[tuple]) -> List[str]:
    failures, components = [], set()
    for rel, _ in changes:
        match = COMPONENT_PATH.match(rel)
        if not match or match.group(1) == "interfaces":
            failures.append(f"{rel} is outside the component being repaired; only components/<component>/src/ "
                            "and verif-creusot/src/ may change")
            continue
        components.add(match.group(1))
        if not match.group(2).startswith(ALLOWED_IN_COMPONENT):
            failures.append(f"{rel} is not under src/ or verif-creusot/src/ of its component")
    if len(components) > 1:
        failures.append("the patch touches more than one component (" + ", ".join(sorted(components))
                        + "); a repair stays inside the component that was refuted")
    return failures


def check_added_text(changes: List[tuple]) -> List[str]:
    failures = []
    for rel, added in changes:
        match = COMPONENT_PATH.match(rel)
        if not match or not rel.endswith(".rs") or not added:
            continue
        inner = match.group(2)
        if inner.startswith("verif-creusot/src/"):
            rules = PROOF_FORBIDDEN
        elif PurePosixPath(rel).name.startswith("repair_test_"):
            rules = TEST_FORBIDDEN
        elif inner.startswith("src/"):
            rules = SRC_FORBIDDEN
        else:
            continue
        code = "\n".join(LINE_COMMENT.sub("", line) for line in added.splitlines())
        for pattern, why in rules:
            if pattern.search(code):
                failures.append(f"{rel} {why}; a repair removes the root cause instead")
    return failures


def extra_checks(workdir: Path, worktree: Optional[object]) -> List[str]:
    """Inspect the real diff against the base; without a worktree there is nothing to compare."""
    if worktree is None:
        return []
    workdir = Path(workdir)
    repo, base = Path(worktree.path), worktree.base
    changes = worktree.changes()
    failures: List[str] = []
    failures += check_scope(changes)
    failures += check_added_text(changes)
    failures += check_proof_functions(repo, base, workdir)
    failures += check_obligations(repo, base, workdir)
    return failures
