#!/usr/bin/env bash
# save.sh — save a Claude Code session transcript as markdown with token/cost stats
# Usage: save.sh <session.jsonl> [output.md]

set -euo pipefail

JSONL="${1:-}"
if [[ -z "$JSONL" || ! -f "$JSONL" ]]; then
    echo "Usage: save.sh <session.jsonl> [output.md]" >&2
    exit 1
fi

SESSION_ID="$(basename "$JSONL" .jsonl)"
DATE="$(date +%Y-%m-%d)"

if [[ -n "${2:-}" ]]; then
    OUT="$2"
else
    OUT="$(pwd)/transcript_${SESSION_ID}_${DATE}.md"
fi

# Pricing per million tokens (claude-sonnet-4-6 defaults — edit as needed)
P_INPUT=3.00
P_OUTPUT=15.00
P_CW=3.75
P_CR=0.30

python3 - "$JSONL" "$OUT" "$P_INPUT" "$P_OUTPUT" "$P_CW" "$P_CR" <<'PYEOF'
import json, sys
from datetime import datetime

jsonl, out, p_in, p_out, p_cw, p_cr = \
    sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4]), \
    float(sys.argv[5]), float(sys.argv[6])

def fmt_ts(ts):
    try:
        dt = datetime.fromisoformat(ts.replace('Z', '+00:00'))
        return dt.astimezone().strftime('%Y-%m-%d %H:%M:%S %Z')
    except Exception:
        return ts

def extract_text(content):
    if isinstance(content, str):
        return content
    parts = []
    for b in content:
        if not isinstance(b, dict):
            continue
        if b.get('type') == 'text':
            parts.append(b['text'])
        elif b.get('type') == 'tool_use':
            inp = b.get('input', {})
            s = ', '.join(f'{k}={repr(v)[:60]}' for k, v in list(inp.items())[:3])
            parts.append(f'[tool: {b.get("name","?")}({s})]')
        elif b.get('type') == 'tool_result':
            parts.append('[tool result]')
    return '\n'.join(parts)

records = [json.loads(l) for l in open(jsonl) if l.strip()]
turns = [r for r in records
         if r.get('type') in ('user', 'assistant')
         and not r.get('isSidechain')]

session_id = next((r.get('sessionId', '') for r in records if r.get('sessionId')), '')
model = next((r['message'].get('model', '')
              for r in turns
              if r.get('type') == 'assistant' and isinstance(r.get('message'), dict)), '')
start_ts = turns[0].get('timestamp', '') if turns else ''
end_ts   = turns[-1].get('timestamp', '') if turns else ''

ti = to = tcw = tcr = 0
for r in turns:
    if r.get('type') == 'assistant':
        u = r.get('message', {}).get('usage', {})
        ti  += u.get('input_tokens', 0)
        to  += u.get('output_tokens', 0)
        tcw += u.get('cache_creation_input_tokens', 0)
        tcr += u.get('cache_read_input_tokens', 0)

cost = ti*p_in/1e6 + to*p_out/1e6 + tcw*p_cw/1e6 + tcr*p_cr/1e6

lines = [
    f'# Transcript: {session_id}', '',
    '| Field | Value |', '|-------|-------|',
    f'| Model | {model} |',
    f'| Start | {fmt_ts(start_ts)} |',
    f'| End   | {fmt_ts(end_ts)} |',
    f'| Input tokens | {ti:,} |',
    f'| Output tokens | {to:,} |',
    f'| Cache write tokens | {tcw:,} |',
    f'| Cache read tokens | {tcr:,} |',
    f'| **Estimated cost** | **${cost:.4f}** |',
    '', '---', '',
]

turn_num = 0
for r in turns:
    role = r.get('type', '')
    ts   = fmt_ts(r.get('timestamp', ''))
    msg  = r.get('message', {})
    text = extract_text(msg.get('content', ''))
    if role == 'user':
        turn_num += 1
        lines.append(f'## Turn {turn_num} — User  `{ts}`')
    else:
        u = msg.get('usage', {})
        i  = u.get('input_tokens', 0)
        o  = u.get('output_tokens', 0)
        cw = u.get('cache_creation_input_tokens', 0)
        cr = u.get('cache_read_input_tokens', 0)
        c  = i*p_in/1e6 + o*p_out/1e6 + cw*p_cw/1e6 + cr*p_cr/1e6
        lines.append(
            f'## Turn {turn_num} — Assistant  `{ts}`  '
            f'_(in:{i:,} out:{o:,} cw:{cw:,} cr:{cr:,} cost:${c:.4f})_'
        )
    lines += ['', text, '', '---', '']

open(out, 'w').write('\n'.join(lines))
print(f'Saved: {out}')
print(f'Cost:  ${cost:.4f}')
print(f'Turns: {turn_num}')
PYEOF
