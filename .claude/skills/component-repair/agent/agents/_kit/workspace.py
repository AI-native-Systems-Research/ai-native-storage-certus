"""Git worktree isolation: every session works on a throwaway checkout of a base revision."""
from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Sequence, Tuple


class WorkspaceError(RuntimeError):
    pass


def scratch_path(*parts: str) -> Path:
    """A directory outside the workbench for worktrees and evaluation fixtures.

    Sessions run here, not under `agents/<name>/runs/`, so neither relative paths nor a search
    from the working directory reaches the agent's spec or evaluation cases.
    """
    root = Path(os.environ.get("AGENT_KIT_SCRATCH") or Path(tempfile.gettempdir()) / "agent-kit")
    return root.joinpath(*[part.replace("/", "_") for part in parts])


def remove_scratch(path: Path) -> None:
    """Delete a scratch directory, refusing anything outside the scratch root."""
    path, root = Path(path).resolve(), scratch_path().resolve()
    if path != root and root in path.parents:
        shutil.rmtree(path, ignore_errors=True)


def uncommitted_changes(repo: Path) -> list:
    """Tracked changes in a checkout (untracked files are ignored); the agent cannot see these."""
    out = _git(Path(repo), "status", "--porcelain", "--untracked-files=no", check=False)
    return [line for line in out.splitlines() if line.strip()]


def _git(cwd: Path, *args: str, check: bool = True) -> str:
    proc = subprocess.run(["git", *args], cwd=str(cwd), capture_output=True, text=True)
    if check and proc.returncode != 0:
        raise WorkspaceError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
    return proc.stdout


@dataclass
class Worktree:
    repo: Path
    path: Path
    base: str
    # Tool output (bytecode, caches) never enters diffs, patches, or result commits.
    ignore: Sequence[str] = field(default_factory=tuple)

    @classmethod
    def create(cls, repo: Path, dest: Path, ref: str = "HEAD", ignore: Sequence[str] = ()) -> "Worktree":
        repo, dest = Path(repo).resolve(), Path(dest).resolve()
        base = _git(repo, "rev-parse", "--verify", f"{ref}^{{commit}}").strip()
        if dest.exists() and scratch_path().resolve() in dest.parents:
            # Left over from a crashed or resumed run; scratch worktrees are always disposable.
            _git(repo, "worktree", "remove", "--force", str(dest), check=False)
            shutil.rmtree(dest, ignore_errors=True)
            _git(repo, "worktree", "prune", check=False)
        dest.parent.mkdir(parents=True, exist_ok=True)
        _git(repo, "worktree", "add", "--detach", str(dest), base)
        return cls(repo, dest, base, tuple(ignore))

    @classmethod
    def attach(cls, path: Path, base: str = "HEAD", ignore: Sequence[str] = ()) -> "Worktree":
        """Treat an existing checkout as the workspace, diffing against base."""
        path = Path(path).resolve()
        resolved = _git(path, "rev-parse", "--verify", f"{base}^{{commit}}").strip()
        top = Path(_git(path, "rev-parse", "--show-toplevel").strip())
        return cls(top, path, resolved, tuple(ignore))

    def _stage(self) -> None:
        excludes = [f":(exclude,glob){pattern}" for pattern in self.ignore]
        _git(self.path, "add", "-A", "--", ".", *excludes)
        if excludes:
            # Return ignored paths to how base has them: drops ones staged earlier (e.g. by a tool
            # before the ignore list applied), and keeps ones the repository commits, such as
            # bytecode, instead of staging their deletion.
            _git(self.path, "reset", "-q", self.base, "--",
                 *[f":(glob){pattern}" for pattern in self.ignore], check=False)

    def changed_files(self) -> List[str]:
        self._stage()
        return [line for line in _git(self.path, "diff", "--cached", "--name-only", self.base).splitlines() if line]

    def added_text(self) -> Dict[str, str]:
        """Added lines per changed file; deleted and renamed-away paths map to ''."""
        self._stage()
        diff = _git(self.path, "diff", "--cached", "--no-color", "-U0", self.base)
        added: Dict[str, List[str]] = {}
        current = None
        for line in diff.splitlines():
            if line.startswith("diff --git "):
                current = line.split(" b/", 1)[1] if " b/" in line else None
                if current is not None:
                    added.setdefault(current, [])
            elif line.startswith("+++ ") or line.startswith("--- "):
                continue
            elif line.startswith("+") and current is not None:
                added[current].append(line[1:])
        return {path: "\n".join(lines) for path, lines in added.items()}

    def changes(self) -> List[Tuple[str, str]]:
        return sorted(self.added_text().items())

    def diff(self) -> str:
        self._stage()
        return _git(self.path, "diff", "--cached", "--no-color", self.base)

    def commit(self, message: str) -> str:
        self._stage()
        _git(self.path, "-c", "user.name=agent", "-c", "user.email=agent@localhost",
             "commit", "--allow-empty", "-m", message)
        return _git(self.path, "rev-parse", "HEAD").strip()

    def fast_forward(self, commit: str) -> None:
        """Move the source repository's checkout to a verified commit, only as a fast-forward."""
        _git(self.repo, "merge", "--ff-only", "--quiet", commit)

    def cherry_pick(self, commit: str) -> bool:
        """Apply a commit onto this worktree's HEAD; on conflict, abort and return False."""
        proc = subprocess.run(
            ["git", "-c", "user.name=agent", "-c", "user.email=agent@localhost", "cherry-pick",
             "--allow-empty", "--keep-redundant-commits", commit],
            cwd=str(self.path), capture_output=True, text=True,
        )
        if proc.returncode == 0:
            return True
        _git(self.path, "cherry-pick", "--abort", check=False)
        _git(self.path, "reset", "--hard", "--quiet", "HEAD", check=False)
        return False

    def head(self) -> str:
        return _git(self.path, "rev-parse", "HEAD").strip()

    def drop_ref(self, ref: str) -> None:
        _git(self.repo, "update-ref", "-d", ref, check=False)

    def keep_ref(self, name: str, commit: str) -> str:
        """Pin a result commit with a ref in the source repo so removing the worktree keeps it."""
        ref = f"refs/agent-runs/{name}"
        _git(self.repo, "update-ref", ref, commit)
        return ref

    def remove(self) -> None:
        _git(self.repo, "worktree", "remove", "--force", str(self.path), check=False)
