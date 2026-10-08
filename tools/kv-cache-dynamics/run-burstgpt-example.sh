#!/bin/bash
# run-burstgpt-example.sh — kv-cache-dynamics stock run with BurstGPT intervals.
#
# Mixed-source run: the turn-interval (think-time) distribution comes from
# BurstGPT v2.0 (BurstGPT_without_fails_3.csv — 55K ChatGPT/GPT-4 conversation
# sessions over 110 days; gaps p50 131s, p90 39min, p99 22h), while everything
# else — model, cache tiers, 131K window, turn counts, per-turn tokens, shared
# prefix — is the cc131k configuration from run-cc131k-example.sh. BurstGPT
# itself is short, shallow chat (input p50 525 tokens, 2 turns/session), so only
# its intervals are used.
#
# Every run-cc131k-example.sh variable is still env-overridable, e.g.:
#   ADMISSION=round-robin ./run-burstgpt-example.sh
#   HBM_GB=40 DRAM_GB=0 ./run-burstgpt-example.sh
#
# Regenerate the YAML (in benchmarks/synthetic-gen-by-llm/; .csv => BurstGPT):
#   ./extract_interval_distribution.py BurstGPT_without_fails_3.csv -o burstgpt-3-intervals.yaml
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

DIST="${DIST:-${SCRIPT_DIR}/burstgpt-3-intervals.yaml}" \
  exec "${SCRIPT_DIR}/run-cc131k-example.sh" "$@"
