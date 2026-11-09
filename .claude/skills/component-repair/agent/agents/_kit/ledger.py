"""Append-only JSONL ledger with a single locked writer. The ledger is the run's memory."""
from __future__ import annotations

import fcntl
import json
import time
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional


class Ledger:
    def __init__(self, path: Path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._lock_path = self.path.with_suffix(self.path.suffix + ".lock")

    def records(self) -> List[Dict[str, Any]]:
        if not self.path.is_file():
            return []
        return [json.loads(line) for line in self.path.read_text(encoding="utf-8").splitlines() if line.strip()]

    def __iter__(self) -> Iterator[Dict[str, Any]]:
        return iter(self.records())

    def append(self, event: str, **data: Any) -> Dict[str, Any]:
        with open(self._lock_path, "w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            existing = self.records()
            record = {
                "seq": (existing[-1]["seq"] + 1) if existing else 1,
                "time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "event": event,
                **data,
            }
            with self.path.open("a", encoding="utf-8") as handle:
                handle.write(json.dumps(record, sort_keys=True, default=str) + "\n")
            return record

    def last(self, event: Optional[str] = None, **match: Any) -> Optional[Dict[str, Any]]:
        for record in reversed(self.records()):
            if (event is None or record["event"] == event) and all(record.get(k) == v for k, v in match.items()):
                return record
        return None

    def item_status(self) -> Dict[str, str]:
        """Latest status per work item, from `item` events: resume skips finished items."""
        status: Dict[str, str] = {}
        for record in self.records():
            if record["event"] == "item" and "item" in record:
                status[str(record["item"])] = record.get("status", "")
        return status
