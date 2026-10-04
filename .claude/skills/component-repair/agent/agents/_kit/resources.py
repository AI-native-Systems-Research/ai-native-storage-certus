"""External things an agent works on, kept portable: the spec stores identity, each machine location.

A spec's `resources` list names what lives outside the workbench, such as a repository, a
folder, a dataset, or a service: `{name, kind, url?, commit?, description?}`. Committed agents
must work on every machine, so the spec never stores a local path. Evaluation cases refer to a
resource as `{resource:<name>}`, and each machine resolves the name to a local path, in order:

1. the environment variable `AGENT_RESOURCE_<NAME>` (upper case, `-` as `_`);
2. the untracked mapping `agents/.local/resources.yaml` (`name: /local/path`), which the builder
   fills in for the resources you give it, so the machine that built an agent works at once;
3. for a git `url`, a clone cached under the kit's scratch directory.

Anything else is an error that says how to map the resource.
"""
from __future__ import annotations

import os
import re
import subprocess
from pathlib import Path
from typing import Any, Dict, List, Optional

import yaml

from .workspace import scratch_path

RESOURCE_REF = re.compile(r"\{resource:([a-z0-9][a-z0-9_-]*)\}")
NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,62}$")


class ResourceError(RuntimeError):
    pass


def slug(text: str) -> str:
    value = re.sub(r"[^a-z0-9]+", "-", str(text).lower()).strip("-")
    return (value or "resource")[:63]


def local_map_path(workbench: Path) -> Path:
    return Path(workbench) / "agents" / ".local" / "resources.yaml"


def read_local_map(workbench: Optional[Path]) -> Dict[str, str]:
    if workbench is None:
        return {}
    try:
        data = yaml.safe_load(local_map_path(workbench).read_text(encoding="utf-8"))
    except (OSError, yaml.YAMLError):
        return {}
    return {str(k): str(v) for k, v in (data or {}).items()} if isinstance(data, dict) else {}


def remember_local(workbench: Path, name: str, path: Path) -> Path:
    """Record where a resource lives on this machine (untracked; never committed)."""
    target = local_map_path(workbench)
    target.parent.mkdir(parents=True, exist_ok=True)
    ignore = target.parent / ".gitignore"
    if not ignore.exists():
        ignore.write_text("*\n", encoding="utf-8")
    mapping = read_local_map(workbench)
    mapping[name] = str(Path(path).expanduser().resolve())
    target.write_text("# Where resources live on this machine (untracked). name: /local/path\n"
                      + yaml.safe_dump(mapping, sort_keys=True), encoding="utf-8")
    return target


def unclaimed_name(workbench: Path, resource: Dict[str, Any], path: Path) -> str:
    """A name for `resource` that does not take over another resource's mapping on this machine.

    Names come from directory names, so two different checkouts can both be `repo`. Remapping a
    name that already points at another existing checkout would silently repoint every agent
    that uses it, so a different resource gets a suffixed name instead. The same path, a stale
    mapping, or the same git url keeps the name."""
    mapping = read_local_map(workbench)
    path_text = str(Path(path).expanduser().resolve())
    base, number = resource["name"], 1
    name = base
    while name in mapping and mapping[name] != path_text and Path(mapping[name]).exists():
        url = resource.get("url")
        if url and describe(Path(mapping[name])).get("url") == url:
            break
        number += 1
        name = f"{base[:60]}-{number}"
    return name


def describe(path: Path) -> Dict[str, Any]:
    """A resource's portable identity, read from a local path (never the path itself)."""
    path = Path(path).expanduser().resolve()
    if not path.exists():
        raise ResourceError(f"{path} does not exist")
    resource: Dict[str, Any] = {"name": slug(path.name),
                                "kind": "folder" if path.is_dir() else "file"}

    def git(*args: str) -> str:
        proc = subprocess.run(["git", "-C", str(path if path.is_dir() else path.parent), *args],
                              capture_output=True, text=True)
        return proc.stdout.strip() if proc.returncode == 0 else ""

    if path.is_dir() and git("rev-parse", "--show-toplevel") == str(path):
        resource["kind"] = "repository"
        url = git("remote", "get-url", "origin")
        if url:
            resource["url"] = url
        commit = git("rev-parse", "HEAD")
        if commit:
            resource["commit"] = commit
    return resource


def _git_url(url: str) -> bool:
    return bool(re.match(r"^(https?://|ssh://|git@|git://|file://)", url)) or url.endswith(".git")


def resolve(name: str, resources: List[Dict[str, Any]], workbench: Optional[Path], *, fetch: bool = True) -> Path:
    declared = {r.get("name"): r for r in resources or []}
    if name not in declared:
        raise ResourceError(f"the spec declares no resource named {name!r}")
    env = os.environ.get("AGENT_RESOURCE_" + name.upper().replace("-", "_"))
    if env:
        return Path(env).expanduser()
    mapped = read_local_map(workbench).get(name)
    if mapped and Path(mapped).exists():
        return Path(mapped)
    url = str(declared[name].get("url") or "")
    if url and _git_url(url) and fetch:
        cache = scratch_path("resources", name)
        if (cache / ".git").exists():
            subprocess.run(["git", "-C", str(cache), "fetch", "--quiet", "--all"], capture_output=True)
        else:
            cache.parent.mkdir(parents=True, exist_ok=True)
            proc = subprocess.run(["git", "clone", "--quiet", url, str(cache)], capture_output=True, text=True)
            if proc.returncode != 0:
                raise ResourceError(f"cloning {name} from {url} failed: {proc.stderr.strip()[-300:]}")
        return cache
    hint = local_map_path(workbench) if workbench else "agents/.local/resources.yaml"
    raise ResourceError(
        f"resource {name!r} is not on this machine. Map it once: add `{name}: /path/to/checkout` to "
        f"{hint}, or set AGENT_RESOURCE_{name.upper().replace('-', '_')}=/path"
        + ("" if url else " (the spec has no url to clone it from)"))


def fill(text: str, resources: List[Dict[str, Any]], workbench: Optional[Path]) -> str:
    """Replace every `{resource:<name>}` in text with that resource's local path."""
    return RESOURCE_REF.sub(lambda m: str(resolve(m.group(1), resources, workbench)), text)


def refs(values: List[str]) -> List[str]:
    return sorted({name for value in values for name in RESOURCE_REF.findall(str(value))})
