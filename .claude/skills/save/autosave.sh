#!/usr/bin/env bash
# autosave.sh — periodically snapshot the most active Claude Code session
# Run from cron every 15 minutes: */15 * * * * bash /path/to/autosave.sh
#
# Saves to <repo_root>/transcripts/, overwriting the previous snapshot
# for each session so disk usage stays bounded.

set -euo pipefail

REPO_ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
SAVE_SCRIPT="$REPO_ROOT/.claude/skills/save/save.sh"
TRANSCRIPTS_DIR="$REPO_ROOT/transcripts"
PROJECTS_DIR=~/.claude/projects

mkdir -p "$TRANSCRIPTS_DIR"

JSONL="$(find "$PROJECTS_DIR" -maxdepth 2 -name '*.jsonl' \
    | xargs ls -t 2>/dev/null | head -1)"

if [[ -z "$JSONL" ]]; then
    exit 0
fi

SESSION_ID="$(basename "$JSONL" .jsonl)"
OUT="$TRANSCRIPTS_DIR/transcript_${SESSION_ID}.md"

bash "$SAVE_SCRIPT" "$JSONL" "$OUT" >> "$TRANSCRIPTS_DIR/autosave.log" 2>&1
