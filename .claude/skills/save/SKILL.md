---
name: save
description: Save current session transcript to markdown with per-turn token counts and cost breakdown.
allowed-tools: Bash(*)
argument-hint: [output_path]
---

Save the current Claude Code session transcript as a markdown file with token usage and cost stats.

## Steps

1. Derive the project key from the current working directory and find the most recent session JSONL:

```bash
PROJECT_KEY=$(pwd | sed 's|/|-|g' | sed 's|^-||')
JSONL=$(ls -t ~/.claude/projects/${PROJECT_KEY}/*.jsonl 2>/dev/null | head -1)
echo "JSONL: $JSONL"
```

2. Determine output path — save into the current working directory unless an argument was passed:

```bash
SESSION_ID=$(basename "$JSONL" .jsonl)
DATE=$(date +%Y-%m-%d)
if [[ -n "$ARGUMENTS" ]]; then
    OUT="$ARGUMENTS"
else
    OUT="$(pwd)/transcript_${SESSION_ID}_${DATE}.md"
fi
```

3. Run the save script (path is relative to the repo root so it works for anyone who clones this repo):

```bash
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
bash "$(git rev-parse --show-toplevel)/.claude/skills/save/save.sh" "$JSONL" "$OUT"
```

4. Report the output path and total estimated cost.
