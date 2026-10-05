#!/usr/bin/env bash
# check_heldout.sh <repo> — HELD-OUT check for EPO-INV-STALE-HANDLE-NEVER-CRASHES.
# Injects tests the agent never saw into a SCRATCH COPY of the agent's result and runs them against
# the real LruList. Passes only if src/ is genuinely fixed: a repair confined to the proof model, or
# one that passes the visible checks some other way, still panics here.
# Measured 2026-10-02: FAILS on a133d013 (panic at lru_list.rs:109), PASSES on the fix.
set -uo pipefail
REPO="${1:?usage: check_heldout.sh <repo>}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCR="$(mktemp -d)"; trap 'rm -rf "$SCR"' EXIT
cp -a "$REPO" "$SCR/repo"
C="$SCR/repo/components/eviction-policy-optimized/src"
cp "$HERE/heldout_stale_handle.rs" "$C/heldout_stale_handle.rs"
printf '\n#[cfg(test)]\n#[path = "heldout_stale_handle.rs"]\nmod heldout_stale_handle;\n' >> "$C/lru_list.rs"
out="$(cd "$SCR/repo" && timeout 1800 cargo test -q -p eviction-policy-optimized heldout_ 2>&1)"
if grep -qE 'test result: ok\. 2 passed' <<<"$out"; then echo "HELDOUT: PASS"; exit 0; fi
grep -E 'panicked at|test result|error' <<<"$out" | head -4; echo "HELDOUT: FAIL"; exit 1
