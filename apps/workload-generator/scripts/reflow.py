#!/usr/bin/env python3
"""Rewrap this feature's markdown artifacts to 79 columns, in place.

Usage:
    scripts/reflow.py [--check] FILE...

`--check` reports files that would change and exits non-zero, without writing.

Why this exists
---------------

Three separate attempts to reflow these documents by hand or with a throwaway
script have corrupted them, each in a different way. The three traps are encoded
below so a fourth attempt does not rediscover them:

1. **A run of non-blank lines is not a paragraph.** A bulleted or numbered list
   has one item per line with no blank line between, so joining every run of
   non-blank lines merges adjacent items into one. A block must also break at
   each new list marker. (This mangled T013-T015 in tasks.md and the D5 bullets
   in research.md.)

2. **A list marker is both the indent and part of the text.** If the marker is
   passed as `initial_indent` while the first line is joined into the text
   unchanged, the marker appears twice — "1. 1. **An absent...". Strip the marker
   from the text and let `initial_indent` re-add it.

3. **Some lines are verbatim and must never be rewrapped or joined**: table rows
   (`|`), headings (`#`), fenced-code contents, horizontal rules, and metadata
   headers matching `^\\*\\*Label\\*\\*:` — the contracts' `**Version**:` /
   `**Status**:` lines, which an earlier pass joined into one and had to be
   repaired by hand.

Tables and fenced code are exempt from the width limit; everything else is not.

The safety check
----------------

After rewrapping, the whitespace-collapsed word stream must be byte-identical to
the input's. If it is not, the script has changed content rather than line
breaks, and it refuses to write. That check is what turns "it looked right" into
"it is right", and it is the reason to use this rather than an editor macro.
"""

from __future__ import annotations

import re
import sys
import textwrap

WIDTH = 79
LIST = re.compile(r"^(\s*)([-*+]\s+|\d+\.\s+)")
LABEL = re.compile(r"^\*\*[^*]+\*\*:")
RULE = re.compile(r"^-{3,}$")


def verbatim(line: str) -> bool:
    """Whether a line must be passed through untouched (trap 3)."""
    s = line.strip()
    return (
        s.startswith("|")
        or s.startswith("#")
        or s.startswith("```")
        or LABEL.match(s) is not None
        or RULE.match(s) is not None
    )


def words(text: str) -> list[str]:
    return [w for w in re.split(r"\s+", text) if w]


def reflow(source: str) -> str:
    lines = source.split("\n")
    out: list[str] = []
    i = 0
    fence = False
    while i < len(lines):
        line = lines[i]
        if line.lstrip().startswith("```"):
            fence = not fence
            out.append(line)
            i += 1
            continue
        if fence or not line.strip() or verbatim(line):
            out.append(line)
            i += 1
            continue

        # Collect a block: this line plus its continuation lines. A new list
        # marker ends the block (trap 1).
        block = [line]
        j = i + 1
        while j < len(lines):
            nxt = lines[j]
            if (
                not nxt.strip()
                or verbatim(nxt)
                or LIST.match(nxt)
                or nxt.lstrip().startswith("```")
            ):
                break
            block.append(nxt)
            j += 1

        if any(len(b) > WIDTH for b in block):
            m = LIST.match(block[0])
            if m:
                first_indent = m.group(1) + m.group(2)
                cont_indent = m.group(1) + " " * len(m.group(2))
                head = block[0][m.end() :].strip()  # trap 2
            else:
                lead = re.match(r"^(\s*)", block[0]).group(1)
                first_indent = lead
                cont_indent = (
                    re.match(r"^(\s*)", block[1]).group(1) if len(block) > 1 else lead
                )
                head = block[0].strip()
            text = " ".join([head] + [b.strip() for b in block[1:]])
            out.extend(
                textwrap.wrap(
                    text,
                    width=WIDTH,
                    initial_indent=first_indent,
                    subsequent_indent=cont_indent,
                    break_long_words=False,
                    break_on_hyphens=False,
                )
            )
        else:
            out.extend(block)
        i = j
    return "\n".join(out)


def main(argv: list[str]) -> int:
    check = "--check" in argv
    paths = [a for a in argv if not a.startswith("--")]
    if not paths:
        print(__doc__.strip().split("\n\n")[1], file=sys.stderr)
        return 2

    failed = False
    for path in paths:
        with open(path) as fh:
            before = fh.read()
        after = reflow(before)

        # The safety check: only line breaks may have moved.
        if words(before) != words(after):
            print(f"{path}: REFUSED — reflow changed content, not just line breaks",
                  file=sys.stderr)
            failed = True
            continue

        over = [
            (n, len(l))
            for n, l in enumerate(after.split("\n"), 1)
            if len(l) > WIDTH and not l.strip().startswith("|")
        ]
        if before == after:
            print(f"{path}: already wrapped")
            continue
        if check:
            print(f"{path}: would rewrap")
            failed = True
            continue
        with open(path, "w") as fh:
            fh.write(after)
        note = f" ({len(over)} line(s) still long — check they are inside code fences)" if over else ""
        print(f"{path}: rewrapped{note}")

    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
