# /save skill

Saves the current Claude Code session transcript as a markdown file with per-turn token counts and estimated cost.

## Usage

From within any Claude Code session:
```
/save
```
or with a custom output path:
```
/save path/to/output.md
```

Output defaults to `transcript_<session_id>_<date>.md` in the current working directory.

## Installation

This skill is picked up automatically by Claude Code when you open a session in this repo — no setup needed.

To also enable auto-save every 15 minutes, add to your crontab (`crontab -e`):
```
*/15 * * * * bash /path/to/repo/.claude/skills/save/autosave.sh
```

## Output format

```
# Transcript: <session-id>

| Field         | Value                  |
|---------------|------------------------|
| Model         | claude-sonnet-4-6      |
| Start         | 2026-04-21 10:27 PDT   |
| End           | 2026-04-21 15:32 PDT   |
| Input tokens  | 412                    |
| Output tokens | 89,779                 |
| Cache write   | 773,518                |
| Cache read    | 21,709,450             |
| Estimated cost| $10.76                 |

## Turn 1 — User  `2026-04-21 10:27:32 PDT`
...

## Turn 1 — Assistant  `...`  _(in:3 out:11 cw:30,129 cr:0 cost:$0.11)_
...
```

## Compaction warning

Claude Code compacts long sessions when they approach the context limit — earlier turns are replaced by a summary and the raw messages are lost. Run `/save` **before** compaction hits, or use the auto-save cron job to snapshot every 15 minutes automatically.

## Pricing defaults (claude-sonnet-4-6)

| Token type  | Rate (USD/million) |
|-------------|-------------------|
| Input       | $3.00             |
| Output      | $15.00            |
| Cache write | $3.75             |
| Cache read  | $0.30             |

Edit the `P_INPUT/P_OUTPUT/P_CW/P_CR` variables at the top of `save.sh` for other models.

## Files

| File          | Purpose                                      |
|---------------|----------------------------------------------|
| `SKILL.md`    | Claude Code skill definition                 |
| `save.sh`     | Core transcript parser and markdown writer   |
| `autosave.sh` | Cron-friendly wrapper for periodic snapshots |
| `README.md`   | This file                                    |
