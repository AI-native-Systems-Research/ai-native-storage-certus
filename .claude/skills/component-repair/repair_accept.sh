#!/usr/bin/env bash
# repair_accept.sh <repo> <component> <OBLIGATION-ID> <base-rev> --also <callee,modules>
#
# Accepts a repair only when it is REAL, PROVED and HARMLESS. Exit 0 only if all four hold.
# Run by code, outside the agent's session; its verdict is the only one that counts.
#
#   1. PROVED   repair_oracle.sh: the callee modules and the new verify_<id> prove, its __mutant
#               twin fails BECAUSE THE PROVER RAN (not because the build broke).
#   2. REAL     components/<c>/src changed relative to <base-rev>.
#               The Creusot proofs check a MODEL of the code kept in verif-creusot/, not src/
#               itself — so a change confined to the model satisfies leg 1 while the real code stays
#               broken. This leg, and leg 3, are what tie the proof to the product.
#   3. TESTED   the component crate's own `cargo test` passes.
#   4. HARMLESS whole-crate proof regression: every module that proved at <base-rev> still proves.
#               Compared against a baseline, NOT "only mutants may fail": eviction-policy-optimized
#               has a pre-existing non-mutant failure (verify_epo_inv_list_empty_iff_no_ends,
#               ✘ 22/23 before and after the fix), and an absolute rule would reject a correct fix.
#
# Deliberately NOT here: writing any status. Only the scorers do that.
set -uo pipefail
REPO="${1:?usage: repair_accept.sh <repo> <component> <OBLIGATION-ID> <base-rev> --also <mods>}"
COMP="${2:?}"; OBL="${3:?}"; BASE="${4:?}"; shift 4
ALSO=""
while (( $# )); do case "$1" in --also) ALSO="${2:?}"; shift ;; *) echo "ACCEPT: FAIL — unknown arg $1"; exit 2 ;; esac; shift; done
[[ -n "$ALSO" ]] || { echo "ACCEPT: FAIL — --also is required: without the callee modules, a driver proves against callees' CONTRACTS and certifies unfixed code"; exit 2; }

export PATH="$HOME/.local/share/creusot/bin:$HOME/.cargo/bin:$PATH"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CDIR="$REPO/components/$COMP"
CRATE="$CDIR/verif-creusot"
PKG="$(sed -nE 's/^name[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "$CDIR/Cargo.toml" | head -1)"
fail=0
echo "ACCEPT: $OBL in $COMP (base $BASE)"

# 1. PROVED
( cd "$REPO" && bash "$HERE/repair_oracle.sh" "components/$COMP" "$OBL" --also "$ALSO" ) | sed 's/^/  │ /'
[[ ${PIPESTATUS[0]} -eq 0 ]] && echo "  1. PROVED   : yes" || { echo "  1. PROVED   : NO"; fail=1; }

# 2. REAL
if git -C "$REPO" diff --quiet "$BASE" -- "components/$COMP/src"; then
  echo "  2. REAL     : NO — src/ is unchanged. A fix confined to the proof model is not a fix."; fail=1
else
  echo "  2. REAL     : yes ($(git -C "$REPO" diff --shortstat "$BASE" -- "components/$COMP/src" | sed 's/^ *//'))"
fi

# 3. TESTED
tout="$(cd "$REPO" && timeout 1800 cargo test -q -p "$PKG" 2>&1)"
if grep -qE 'test result: FAILED|error(\[E[0-9]+\])?:' <<<"$tout" || ! grep -q 'test result: ok' <<<"$tout"; then
  echo "  3. TESTED   : NO"; grep -E 'panicked at|test result|error' <<<"$tout" | head -5 | sed 's/^/       │ /'; fail=1
else
  echo "  3. TESTED   : yes ($(grep -oE '[0-9]+ passed' <<<"$tout" | awk '{s+=$1} END {print s}') passed)"
fi

# 4. HARMLESS — whole crate now vs whole crate at base
unproved() {   # <crate-dir> -> sorted unproved module names; prints NORUN if the prover never ran
  local out; out="$(cd "$1" && timeout 3600 cargo creusot --why3find-arg=-f 2>&1)"
  if grep -qE '^Proved \(' <<<"$out"; then return 0; fi
  grep -qE 'unproved file' <<<"$out" || { echo NORUN; return 0; }
  grep -oE 'Goal Coma\.vc_[A-Za-z0-9_]+' <<<"$out" | sed 's/Goal Coma\.vc_//' | sed -E 's/_{2,}/_/g' | sort -u
}
TMPB="$(mktemp -d)"; git -C "$REPO" worktree add -q --detach "$TMPB/base" "$BASE"
[[ -e "$CDIR/creusot" ]] && ln -sfn "$(readlink -f "$CDIR/creusot")" "$TMPB/base/components/$COMP/creusot"
before="$(unproved "$TMPB/base/components/$COMP/verif-creusot")"
after="$(unproved "$CRATE")"
git -C "$REPO" worktree remove --force "$TMPB/base"; rm -rf "$TMPB"
if [[ "$before" == NORUN || "$after" == NORUN ]]; then
  echo "  4. HARMLESS : NO — the whole-crate prover did not run (setup error), so nothing is known"; fail=1
else
  regressed="$(comm -13 <(printf '%s\n' "$before" | sed '/^$/d') <(printf '%s\n' "$after" | grep -v '_mutant$' | sed '/^$/d'))"
  if [[ -n "$regressed" ]]; then
    echo "  4. HARMLESS : NO — proved at base, unproved now:"; printf '       │ %s\n' $regressed; fail=1
  else
    echo "  4. HARMLESS : yes (unproved at base: $(printf '%s\n' "$before" | sed '/^$/d' | wc -l), now: $(printf '%s\n' "$after" | sed '/^$/d' | wc -l), new non-mutant failures: 0)"
  fi
fi

(( fail )) && { echo "ACCEPT: FAIL"; exit 1; }
echo "ACCEPT: PASS — proved, real, tested, and no proof regressed."
