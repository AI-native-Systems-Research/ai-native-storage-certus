---
name: save
description: Save current session transcript to markdown with per-turn token counts and cost breakdown.
allowed-tools: Bash(*)
argument-hint: [output_path]
---

Save the current Claude Code session transcript as a markdown file with token usage and cost stats.

The user may pass an optional output path as an argument. If provided, use it as OUT. Otherwise default to the current directory.

## Steps

1. Derive the project key from the current working directory and find the most recent session JSONL:

```bash
PROJECT_KEY=$(pwd | sed 's|/|-|g')
JSONL=$(ls -t ~/.claude/projects/${PROJECT_KEY}/*.jsonl 2>/dev/null | head -1)
if [[ -z "$JSONL" ]]; then
    echo "No session JSONL found for project key: $PROJECT_KEY" >&2
    exit 1
fi
```

2. Determine output path — use the argument if provided, otherwise save to current directory:

```bash
SESSION_ID=$(basename "$JSONL" .jsonl)
DATE=$(date +%Y-%m-%d)
if [[ -n "$ARGUMENTS" ]]; then
    OUT="$ARGUMENTS"
else
    OUT="$(pwd)/transcript_${SESSION_ID}_${DATE}.md"
fi
```

3. Find and run the save script relative to the repo root:

```bash
SAVE_SCRIPT="$(git rev-parse --show-toplevel 2>/dev/null)/.claude/skills/save/save.sh"
if [[ ! -f "$SAVE_SCRIPT" ]]; then
    echo "save.sh not found at: $SAVE_SCRIPT" >&2
    exit 1
fi
bash "$SAVE_SCRIPT" "$JSONL" "$OUT"
```

4. Report the output path and total estimated cost.
