---
name: tools-codex-bridge-setup
description: Set up the claude-codex-bridge so Claude Code can talk to OpenAI Codex CLI (and vice versa) via MCP.
allowed-tools: Bash(*), Read, Write, Edit
argument-hint: "[--bidirectional]"
---

Set up the claude-codex-bridge MCP server so that Claude Code can call Codex for second opinions, code reviews, explanations, and performance analysis.

## Prerequisites Check

Verify and enforce these before proceeding:

```bash
# Node.js >= 18 (required)
NODE_VER=$(node --version 2>/dev/null | sed 's/v//' | cut -d. -f1)
if [[ -z "$NODE_VER" ]] || [[ "$NODE_VER" -lt 18 ]]; then
  echo "ERROR: Node.js >= 18 required. Install via: fnm install 22 && fnm use 22"
  exit 1
fi

# npx available
command -v npx >/dev/null || { echo "ERROR: npx not found"; exit 1; }

# Codex CLI
if ! command -v codex >/dev/null 2>&1; then
  echo "Codex CLI not found. Installing..."
  npm install -g @openai/codex
fi
codex --version
```

### Codex Authentication

Codex needs an API key persisted in the shell profile (not just exported for the current shell):

**If using OpenAI directly**, add to `~/.bashrc` or `~/.zshrc`:
```bash
export OPENAI_API_KEY="sk-..."
```

**If using a LiteLLM proxy or custom provider**, configure `~/.codex/config.toml`:
```toml
model = "<your-proxy-model-name>"    # e.g. "gpt-5.5" — must match what your proxy serves
model_provider = "litellm"

[model_providers.litellm]
name = "My Proxy"
base_url = "https://my-proxy.example.com"
env_key = "LITELLM_API_KEY"           # name of the env var holding your key
```

And persist the key in your shell profile:
```bash
export LITELLM_API_KEY="your-key-here"
```

## Setup

### Option A: Automatic (recommended)

The most common setup is Claude-to-Codex (let Claude ask Codex for help):

```bash
npx claude-codex-bridge setup claude
```

For bidirectional (Claude calls Codex AND Codex calls Claude):
```bash
npx claude-codex-bridge setup
```

The automatic setup registers the MCP server, installs the `/codex` skill, and adds a codex-teammate agent.

### Option B: Manual

#### 1. Register the Codex MCP server in Claude Code

```bash
claude mcp add codex -s user -- npx claude-codex-bridge serve codex
```

This adds the following to `~/.claude/settings.json` under `mcpServers`:

```json
{
  "codex": {
    "command": "npx",
    "args": ["claude-codex-bridge", "serve", "codex"],
    "timeout": 600
  }
}
```

#### 2. Install the /codex skill

Create `~/.claude/skills/codex/SKILL.md`:

```bash
mkdir -p ~/.claude/skills/codex
cat > ~/.claude/skills/codex/SKILL.md << 'SKILL_EOF'
---
name: codex
description: Ask OpenAI Codex for a second opinion — code reviews, explanations, plan critiques, performance analysis, or general questions
argument-hint: "<task or question>"
allowed-tools: "Read, Glob, Grep, Bash, mcp__codex__codex_query, mcp__codex__codex_review_code, mcp__codex__codex_review_plan, mcp__codex__codex_explain_code, mcp__codex__codex_plan_perf, mcp__codex__codex_implement"
---

You are invoking Codex to get a second opinion. Route the user's request to the most appropriate Codex MCP tool.

## Tool Selection

| Request Type                | Tool                             | Key Parameters                              |
| --------------------------- | -------------------------------- | ------------------------------------------- |
| Code review, diff review    | `mcp__codex__codex_review_code`  | `target` (diff range or file), `focusAreas` |
| Plan critique               | `mcp__codex__codex_review_plan`  | `plan`, `codebasePath`                      |
| Explain code                | `mcp__codex__codex_explain_code` | `target` (file/function), `depth`           |
| Performance analysis        | `mcp__codex__codex_plan_perf`    | `target`, `metrics`                         |
| Implement/fix (writes code) | `mcp__codex__codex_implement`    | `task`                                      |
| General question            | `mcp__codex__codex_query`        | `prompt`                                    |

## Instructions

1. Parse the user's argument to determine the task type
2. If the user references files, read them first for context
3. Call the most specific Codex tool — prefer specialized tools over `codex_query`
4. Always pass `workingDirectory` to every tool call
5. Synthesize the response: summarize key findings, highlight important points
6. Only use `codex_implement` if the user explicitly asks Codex to make changes
SKILL_EOF
```

#### 3. (Optional) Enable Codex-to-Claude (bidirectional)

Add to `~/.codex/config.toml`:

```toml
[mcp_servers.claude]
command = "npx"
args = ["claude-codex-bridge", "serve", "claude"]
tool_timeout_sec = 600
```

## Verification

After setup, restart Claude Code and verify:

**Check 1 — MCP server registered:**
```bash
claude mcp list | grep codex
```

**Check 2 — Tools available (from inside a Claude Code session):**
Ask Claude: "Can you see the codex MCP tools?" — it should list `mcp__codex__codex_query` etc.

**Check 3 — End-to-end test:**
```
/codex what is 2+2
```
or ask Claude: "Ask Codex what model it is."

## Available Tools

Once the bridge is active, these MCP tools are available to Claude:

| Tool | Purpose |
|------|---------|
| `mcp__codex__codex_query` | General questions, second opinions |
| `mcp__codex__codex_review_code` | Code review (diff ranges, files) |
| `mcp__codex__codex_review_plan` | Critique implementation plans |
| `mcp__codex__codex_explain_code` | Explain code, modules, functions |
| `mcp__codex__codex_plan_perf` | Performance analysis and optimization plans |
| `mcp__codex__codex_implement` | Ask Codex to make code changes (writes files) |

## Usage Examples

Once set up, use naturally in conversation:

- "Ask Codex to review my recent changes"
- "Get Codex's opinion on this approach"
- `/codex explain src/lib/channel.rs`
- `/codex review HEAD~3..HEAD`
- `/codex is this lock-free queue correct?`
- `/codex plan perf improvements for the hot path in extent-manager`

## Troubleshooting

| Problem | Fix |
|---------|-----|
| `npx: command not found` | Install Node.js >= 18 (`fnm install 22` or `nvm install 22`) |
| Codex tools not appearing | Restart Claude Code after adding MCP config |
| Timeout errors | Increase `"timeout": 600` in settings.json (seconds) |
| Auth failures from Codex | Ensure API key is in shell profile (not just current shell) and restart |
| `codex: command not found` | `npm install -g @openai/codex` |
| `/codex` skill not found | Ensure `~/.claude/skills/codex/SKILL.md` exists |

## Execution

When this skill is invoked:

1. Check prerequisites (node >= 18, npx, codex CLI)
2. If `--bidirectional` is passed, run `npx claude-codex-bridge setup`
3. Otherwise run `npx claude-codex-bridge setup claude`
4. Verify with `claude mcp list | grep codex`
5. Report success and remind user to restart Claude Code if this is a fresh install
