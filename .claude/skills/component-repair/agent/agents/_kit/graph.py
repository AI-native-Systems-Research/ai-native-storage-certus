"""Dependency graph of work items: validated, ordered, and scheduled by code.

`build_items` may give each item `deps` (ids it depends on). Code checks the graph, orders it
topologically (ties keep `build_items` order, so the order is reproducible), starts an item
only when every dependency is done, and blocks the descendants of an item that failed. The
model never decides order or readiness.
"""
from __future__ import annotations

import heapq
from typing import Dict, List, Mapping, Sequence, Tuple, TypeVar

# An item whose dependency ended in one of these can never run.
BLOCKING = ("escalated", "skipped", "blocked")

T = TypeVar("T")


class GraphError(ValueError):
    pass


def _deps(item) -> List[str]:
    return list(getattr(item, "deps", None) or [])


def order_items(items: Sequence[T]) -> List[T]:
    """Validate the graph and return the items in dependency order.

    Raises GraphError on a duplicate id, an unknown or self dependency, or a cycle (the message
    names the cycle).
    """
    index = {}
    for position, item in enumerate(items):
        if item.id in index:
            raise GraphError(f"duplicate item id {item.id!r}")
        index[item.id] = position
    for item in items:
        for dep in _deps(item):
            if dep == item.id:
                raise GraphError(f"item {item.id!r} depends on itself")
            if dep not in index:
                raise GraphError(f"item {item.id!r} depends on unknown item {dep!r}")
    indegree = {item.id: len(set(_deps(item))) for item in items}
    dependents: Dict[str, List[str]] = {item.id: [] for item in items}
    for item in items:
        for dep in set(_deps(item)):
            dependents[dep].append(item.id)
    heap = [(index[i], i) for i, degree in indegree.items() if degree == 0]
    heapq.heapify(heap)
    ordered: List[str] = []
    while heap:
        _, current = heapq.heappop(heap)
        ordered.append(current)
        for child in dependents[current]:
            indegree[child] -= 1
            if indegree[child] == 0:
                heapq.heappush(heap, (index[child], child))
    if len(ordered) != len(items):
        raise GraphError("dependency cycle: " + " -> ".join(_find_cycle(items)))
    by_id = {item.id: item for item in items}
    return [by_id[i] for i in ordered]


def _find_cycle(items: Sequence) -> List[str]:
    graph = {item.id: _deps(item) for item in items}
    state: Dict[str, int] = {}
    stack: List[str] = []

    def visit(node: str) -> List[str]:
        state[node] = 1
        stack.append(node)
        for dep in graph[node]:
            if state.get(dep) == 1:
                return stack[stack.index(dep):] + [dep]
            if dep not in state:
                found = visit(dep)
                if found:
                    return found
        stack.pop()
        state[node] = 2
        return []

    for item in items:
        if item.id not in state:
            found = visit(item.id)
            if found:
                return found
    return []


def ready(items: Sequence[T], status: Mapping[str, str], finished: Sequence[str]) -> List[T]:
    """Items not yet finished whose dependencies are all done, in the given order."""
    return [
        item for item in items
        if status.get(item.id) not in finished and all(status.get(d) == "done" for d in _deps(item))
    ]


def newly_blocked(items: Sequence[T], status: Mapping[str, str]) -> List[Tuple[T, str]]:
    """Unfinished items with a dependency that can never finish, with the reason.

    Called repeatedly, this propagates blocking down the graph: once an item is recorded as
    blocked, its own dependents are returned on the next call.
    """
    blocked = []
    for item in items:
        if status.get(item.id) in ("done", *BLOCKING):
            continue
        failed = [d for d in _deps(item) if status.get(d) in BLOCKING]
        if failed:
            blocked.append((item, f"dependency {failed[0]} is {status[failed[0]]}"))
    return blocked
