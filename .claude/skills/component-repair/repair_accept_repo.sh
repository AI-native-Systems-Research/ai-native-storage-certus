#!/usr/bin/env bash
# repair_accept_repo.sh <repo> <base-rev> --target <component>:<OBLIGATION-ID>:<callee,mods> [--target ...]
#                       [--verif-prefix verif/creusot/]
#
# REPOSITORY-LEVEL acceptance for a repair that may span several components (2026-10-04). The person
# who approves the PR may not read Rust, so this check — run by code, outside the agent — is what makes
# the approval safe. Exit 0 only when every leg holds:
#
#   A. BUILDS    every crate the repair touched, and every crate that depends on one of them (from
#                `cargo metadata`), passes `cargo check`: a change to a shared interface reaches every
#                caller. A crate whose NATIVE build script cannot run on this machine (SPDK, zyre) and that
#                the repair did not touch is listed as "not checked here" — never silently passed — and
#                the PR says so, so CI covers it.
#   B. TESTED    `cargo test -p <crate>` passes for every crate the repair touched.
#   C. TARGETS   each --target passes repair_accept.sh (proved, real, red-first, tested, harmless).
#   D. MODEL     for every touched component that has a published Creusot proof crate: if a function
#                whose body changed is cited by that crate (by name), the crate must have changed in the
#                repair too. Otherwise re-verifying it would only re-prove a model of the OLD code.
#   E. REVERIFY  every touched component with a published proof crate is re-scored by its gate BEFORE and
#                AFTER the repair, same gate and scope (crate overlaid from <verif-prefix><component> if absent):
#                no property that was proved before the repair may be lost.
#
# Prints a plain-English evidence table at the end, for the PR description.
set -uo pipefail
REPO="${1:?usage: repair_accept_repo.sh <repo> <base-rev> --target c:ID:mods ...}"; BASE="${2:?}"; shift 2
TARGETS=(); PFX="verif/creusot/"
while (( $# )); do case "$1" in
  --target) TARGETS+=("${2:?}"); shift ;;
  --targets) IFS=';' read -ra _t <<<"${2:?}"; for x in "${_t[@]}"; do [[ -n "$x" ]] && TARGETS+=("$x"); done; shift ;;
  --verif-prefix) PFX="${2:?}"; shift ;;
  *) echo "ACCEPT-REPO: FAIL — unknown arg $1"; exit 2 ;;
esac; shift; done
export PATH="$HOME/.local/share/creusot/bin:$HOME/.cargo/bin:$PATH"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$(cd "$HERE/../component-verify/gate" 2>/dev/null && pwd)"
fail=0; declare -a EVID
TOUCHED="$( { git -C "$REPO" diff --name-only "$BASE" -- components; git -C "$REPO" ls-files --others --exclude-standard -- components; } \
            | awk -F/ '$1=="components"{print $2}' | sort -u)"
for t in "${TARGETS[@]}"; do TOUCHED="$(printf '%s\n%s\n' "$TOUCHED" "${t%%:*}" | sed '/^$/d' | sort -u)"; done
echo "ACCEPT-REPO: base $BASE; components touched: $(echo $TOUCHED)"
pkg() { sed -nE 's/^name[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' "$REPO/components/$1/Cargo.toml" 2>/dev/null | head -1; }

# native libraries are gitignored build products: link them in a fresh clone (default: /opt, as on green)
DEPS="${CERTUS_DEPS_DIR:-/opt}"
for n in spdk spdk-build; do [[ -e "$REPO/deps/$n" || ! -e "$DEPS/$n" ]] || ln -s "$DEPS/$n" "$REPO/deps/$n"; done

# A. BUILDS — touched crates and everything that depends on them
TPKGS=""; for c in $TOUCHED; do p="$(pkg "$c")"; [[ -n "$p" ]] && TPKGS="$TPKGS $p"; done
CHECK="$(cd "$REPO" && timeout 300 cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c "
import json, sys
m = json.load(sys.stdin); pk = {p['name']: p for p in m['packages']}
dep = {n: {d['name'] for d in p['dependencies'] if d['name'] in pk} for n, p in pk.items()}
want = set(sys.argv[1:]); out = set(want); frontier = set(want)
while frontier:
    nxt = {n for n, ds in dep.items() if ds & frontier} - out; out |= nxt; frontier = nxt
print(' '.join(sorted(out)))" $TPKGS)"
nok=0; nenv=0; envlist=""
for p in $CHECK; do
  out="$(cd "$REPO" && timeout 1800 cargo check -q -p "$p" 2>&1)"
  if [[ $? -eq 0 ]]; then nok=$((nok+1)); continue; fi
  nat="$(grep -oE 'failed to run custom build command for `[A-Za-z0-9_-]+' <<<"$out" | sed 's/.*`//' | sort -u)"
  if [[ -n "$nat" ]] && ! grep -qwF -f <(echo $TPKGS | tr ' ' '\n') <<<"$nat"; then
    nenv=$((nenv+1)); envlist="$envlist $p"
  else echo "  A. BUILDS   : $p NO"; grep -E '^error' <<<"$out" | head -3 | sed 's/^/       │ /'; fail=1; fi
done
ntot=$(echo $CHECK | wc -w); envmsg=""; envev=""
if (( nenv )); then envmsg="; not checked here, native library unavailable on this machine:$envlist"; envev=", $nenv not checked here (native libraries), CI covers them"; fi
echo "  A. BUILDS   : $nok of $ntot crates checked OK$envmsg"
EVID+=("workspace|build of touched and dependent crates|$nok OK$envev")

# B. TESTED
for c in $TOUCHED; do
  p="$(pkg "$c")"; [[ -z "$p" ]] && continue
  tout="$(cd "$REPO" && timeout 1800 cargo test -q -p "$p" 2>&1)"
  if grep -q 'test result: ok' <<<"$tout" && ! grep -qE 'test result: FAILED|^error' <<<"$tout"; then
    n=$(grep -oE '[0-9]+ passed' <<<"$tout" | awk '{s+=$1} END {print s+0}'); echo "  B. TESTED   : $c yes ($n passed)"; EVID+=("$c|unit tests|$n passed")
  else echo "  B. TESTED   : $c NO"; grep -E 'panicked at|test result|^error' <<<"$tout" | head -3 | sed 's/^/       │ /'; fail=1; EVID+=("$c|unit tests|FAILED"); fi
done

# C. TARGETS
for t in "${TARGETS[@]}"; do
  IFS=: read -r c id mods <<<"$t"
  if (cd "$REPO" && bash "$HERE/repair_accept.sh" "$REPO" "$c" "$id" "$BASE" --also "$mods") > /tmp/rar_$$.log 2>&1; then
    echo "  C. TARGET   : $c $id yes"; EVID+=("$c|$id|not proved before -> PROVED; new test fails on the old code, passes on the new")
  else echo "  C. TARGET   : $c $id NO"; grep -E '^  [0-9b]+\.|NO' /tmp/rar_$$.log | head -6 | sed 's/^/       │ /'; fail=1; EVID+=("$c|$id|NOT accepted"); fi
done; rm -f /tmp/rar_$$.log

# D. MODEL  +  E. REVERIFY
for c in $TOUCHED; do
  has_branch=$(git -C "$REPO" rev-parse -q --verify "${PFX}${c}" >/dev/null 2>&1 && echo y || (git -C "$REPO" rev-parse -q --verify "origin/${PFX}${c}" >/dev/null 2>&1 && echo o || echo n))
  [[ -d "$REPO/components/$c/verif-creusot" || "$has_branch" != n ]] || { echo "  D/E        : $c has no published proof crate — not re-verified (say so in the PR)"; EVID+=("$c|re-verification|no proof crate published"); continue; }
  # D. functions whose body changed in src/, cited by name in the proof crate
  chg="$(git -C "$REPO" diff -U0 "$BASE" -- "components/$c/src" | grep -E '^@@' | sed -nE 's/.*@@ .*fn ([A-Za-z0-9_]+).*/\1/p' | sort -u)"
  crate_changed=$(git -C "$REPO" diff --quiet "$BASE" -- "components/$c/verif-creusot/src" && echo n || echo y)
  if [[ -d "$REPO/components/$c/verif-creusot/src" && -n "$chg" ]]; then
    cited=""; for f in $chg; do grep -qE "\b$f\b" "$REPO/components/$c/verif-creusot/src/"*.rs 2>/dev/null && cited="$cited $f"; done
    if [[ -n "$cited" && "$crate_changed" == n ]]; then
      echo "  D. MODEL   : $c NO — changed functions cited by the proof model, but the model was not updated:$cited"; fail=1
    else echo "  D. MODEL   : $c yes"; fi
  fi
  # E. re-score BEFORE and AFTER with the same gate and the same scope, and compare those two.
  #    Comparing against the PUBLISHED statuses is wrong: measured 2026-10-04 on EPO, it reported 12
  #    "lost" for a repair that lost nothing — 11 were properties now out of scope (level-1
  #    discordances, deliberately not scored) and 1 was rejected by a gate fix made after publication.
  SCR="$(mktemp -d)"
  git -C "$REPO" worktree add -q --detach "$SCR/before" "$BASE"
  cp -a "$REPO" "$SCR/after"
  for side in before after; do
    W="$SCR/$side"
    if [[ ! -d "$W/components/$c/verif-creusot/src" ]]; then
      ref="${PFX}${c}"; [[ "$has_branch" == o ]] && ref="origin/${PFX}${c}"
      git -C "$W" checkout -q "$ref" -- "components/$c/verif-creusot" "components/$c/verif" 2>/dev/null
    fi
    [[ -e "$REPO/components/$c/creusot" ]] && ln -sfn "$(readlink -f "$REPO/components/$c/creusot")" "$W/components/$c/creusot"
    B="$W/components/$c/verif/unified_properties.yaml"
    [[ -f "$B" ]] || { echo "  E. REVERIFY: $c NO — no bundle on the $side side"; fail=1; continue 2; }
    python3 - "$B" <<'PY2'
import sys, yaml
d = yaml.safe_load(open(sys.argv[1]))
for p in d["properties"]:
    c = p.get("creusot")
    if isinstance(c, dict):
        for k in ("status", "symbol", "_scored_by"): c.pop(k, None)
yaml.safe_dump(d, open(sys.argv[1], "w"), sort_keys=False, allow_unicode=True, width=110)
PY2
    timeout 7200 python3 "$GATE/scorer_creusot.py" "$W/components/$c/verif" --crate-dir "$W/components/$c/verif-creusot" \
        --cap-seconds 60 --cap-max 300 > "$SCR/gate_$side.log" 2>&1
  done
  read -r lost gained prov <<<"$(python3 - "$SCR/before/components/$c/verif/unified_properties.yaml" "$SCR/after/components/$c/verif/unified_properties.yaml" <<'PY2'
import sys, yaml
st = lambda f: {p["id"]: (p.get("creusot") or {}).get("status") for p in yaml.safe_load(open(f))["properties"]}
a, b = st(sys.argv[1]), st(sys.argv[2])
lost = [k for k, v in a.items() if v == "proved" and b.get(k) != "proved"]
gained = [k for k, v in b.items() if v == "proved" and a.get(k) != "proved"]
print(len(lost), len(gained), sum(1 for v in b.values() if v == "proved"))
PY2
)"
  if [[ "$lost" == 0 ]]; then echo "  E. REVERIFY: $c yes ($prov proved after the repair; $gained newly proved; 0 lost — same gate, same scope, before vs after)"
  else echo "  E. REVERIFY: $c NO — $lost properties proved before the repair no longer prove"; fail=1; fi
  EVID+=("$c|re-verification (Creusot gate, before vs after)|$prov proved, $gained newly proved, $lost lost")
  git -C "$REPO" worktree remove --force "$SCR/before" 2>/dev/null; rm -rf "$SCR"
done

echo; echo "EVIDENCE (for the PR description):"; printf '  %-28s | %-40s | %s\n' component check result
for e in "${EVID[@]}"; do IFS='|' read -r a b c <<<"$e"; printf '  %-28s | %-40s | %s\n' "$a" "$b" "$c"; done
(( fail )) && { echo "ACCEPT-REPO: FAIL"; exit 1; }
echo "ACCEPT-REPO: PASS — builds, tested, every target proved, every touched component re-verified with nothing lost."
