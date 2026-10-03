#!/usr/bin/env bash
# repair_oracle.sh <component-dir> <OBLIGATION-ID>
#
# The acceptance test for a repair of a verification-proved defect. Exits 0 only when the repair is
# BOTH real and proved. This is the piece the agent cannot be trusted to judge for itself, so it is
# code, it is run outside the agent's session, and its verdict is the only one that counts.
#
# THREE PARTS, and each exists because of a measured failure:
#
#   0. EVERY MODULE THE OBLIGATION RESTS ON must prove — pass them with `--also`.
#      This is the part I got wrong first, and it is the one that matters most. Creusot is MODULAR: a
#      driver `verify_<id>` discharges against its callees' CONTRACTS, while the verification condition
#      that a call cannot panic lives in the CALLEE's own .coma. Measured on
#      EPO-INV-STALE-HANDLE-NEVER-CRASHES: the driver Proves against the UNFIXED body, while
#      `arena_move_to_back` was `✘ (24/35)` and `arena_remove` `✘ (14/25)` before the guard and Proved
#      after it. So an oracle checking only the driver would have certified the repair with the defect
#      still in the tree — exactly the "proved lemma crediting a failing property" defect this project
#      already hit once in the gate itself.
#      RULE: for any obligation of the form "this call is total / cannot panic", the callee module is
#      where the evidence lives. Name it. The same modules belong in the bundle's
#      `creusot.evidence.modules`, which is agent-supplied, so the repair should add them there too.
#
#   1. verify_<id> must PROVE.
#      For a refuted obligation there is NO verify_ module — measured on eviction-policy-optimized,
#      zero `verify_epo_inv_stale_handle*` definitions exist; by convention the proving agent writes
#      only a refutation for a divergent id. So a repair must AUTHOR the proof that it works, and
#      "the refutation is gone" is not evidence of anything.
#
#   2. verify_<id>__mutant must FAIL.
#      A deliberately wrong twin of the new proof. If it passes, the new proof has no content and the
#      repair is unvalidated. Not ceremony: on one component 106 harnesses passed while asserting
#      nothing, and one such empty proof had already been published.
#
#   3. refute_<id> is REPORTED, and gates nothing by default.
#      Refutations come in two shapes and only one dies when the code is fixed:
#        * VIOLATION-shaped — asserts the bad thing happens (e.g.
#          refute_epo_inv_stale_handle_no_cross_entry_effect: same slot, different key, still active).
#          A correct fix should make this stop proving.
#        * PREMISE-shaped — asserts only that the dangerous STATE is reachable (e.g.
#          refute_epo_inv_stale_handle_never_crashes: after arena_clear the arena is empty, so every
#          issued handle is out of range). The panic followed from that state plus unchecked indexing.
#          After a correct fix the state is STILL reachable, so this STILL PROVES.
#      Demanding that every refutation stop proving would therefore reject good repairs. The shape is
#      a judgement about the obligation, not something to infer from a proof run, so this script
#      reports the refutation's status and requires `--refutation-must-die` to gate on it.
#
# Deliberately NOT done here: writing any status into the bundle. Only scorer_creusot.py /
# scorer_kani.py may do that, and re-scoring the whole component is the gate's job, not the oracle's.
# This script answers one question about one obligation, fast, so it can sit in a repair loop.
set -uo pipefail

COMPONENT_DIR="${1:?usage: repair_oracle.sh <component-dir> <OBLIGATION-ID> [--refutation-must-die]}"
OBLIGATION="${2:?usage: repair_oracle.sh <component-dir> <OBLIGATION-ID> [--refutation-must-die]}"
REFUTATION_MUST_DIE=0
ALSO=""
shift 2
while (( $# )); do
  case "$1" in
    --refutation-must-die) REFUTATION_MUST_DIE=1 ;;
    --also) ALSO="${2:?--also needs a comma-separated module list}"; shift ;;
    *) echo "ORACLE: FAIL — unknown argument '$1'"; exit 2 ;;
  esac
  shift
done

export PATH="$HOME/.local/share/creusot/bin:$HOME/.cargo/bin:$PATH"

# The scorers derive artifact names this way; match them exactly or the oracle checks nothing.
slug="$(printf '%s' "$OBLIGATION" | tr '[:upper:]-' '[:lower:]_')"
VERIFY="verify_${slug}"
MUTANT="verify_${slug}__mutant"
REFUTE="refute_${slug}"

CRATE="$COMPONENT_DIR/verif-creusot"
[[ -d "$CRATE" ]] || { echo "ORACLE: FAIL — no Creusot crate at $CRATE"; exit 2; }

# Forced replay. why3find caches by goal, so without -f a source change can be judged against the
# PREVIOUS run's result — which has produced wrong numbers on this project more than once.
run_module() {   # <module> -> prints "PROVED" | "FAILED" | "ABSENT" | "CRASH" | "ERROR"
  local m="$1"
  grep -qE "fn[[:space:]]+${m}\b" "$CRATE/src/"*.rs 2>/dev/null || { echo ABSENT; return; }
  # `cargo creusot` collapses a double underscore in the emitted module name (`__mutant` ->
  # `_mutant.coma`), and the module filter matches the EMITTED name. Asking for `x__mutant` gets
  # "Error: No files to prove" -- the prover never runs. Measured 2026-10-02.
  local filt; filt="$(printf '%s' "$m" | sed -E 's/_{2,}/_/g')"
  local out
  out="$(cd "$CRATE" && timeout 1800 cargo creusot "$filt" --why3find-arg=-f 2>&1)"
  # Gate on the PRINTED verdict line, never on proof.json: it is written incrementally and a mid-run
  # read once reported 0 unproved where the finished run reported 2.
  # BOTH print forms must match, and getting this wrong reported a PROVING refutation as FAILED:
  #   whole crate / several modules -> `Proved (3 files) ✔`
  #   ONE named module              -> `Proved (verif/<crate>_rlib/<module>.coma) ✔`
  if grep -qE '^Proved \(.+\)' <<<"$out"; then echo PROVED
  elif grep -qE 'CBMC failed|Invariant check failed|panicked at' <<<"$out"; then echo CRASH
  # FAILED needs EVIDENCE THE PROVER RAN: a goal line `✘ (k/n)` or "N unproved file".
  # Anything else is ERROR. This distinction is the anti-vacuity check's correctness: the mutant is
  # REQUIRED to fail, so when a build error or "No files to prove" was read as FAILED, a twin that
  # never ran was credited as proof the base has content. Measured 2026-10-02: both a missing
  # creusot-std (0.37s) and the `__mutant` name collapse produced exactly that.
  elif grep -qE '✘ \([0-9]+/[0-9]+\)|[0-9]+ unproved file' <<<"$out"; then echo FAILED
  else
    echo ERROR
    printf '%s\n' "$out" | grep -E 'Error|error' | head -3 | sed 's/^/       │ /' >&2
  fi
}

echo "ORACLE: obligation $OBLIGATION in $COMPONENT_DIR"
extra_fail=0
if [[ -n "$ALSO" ]]; then
  IFS=',' read -ra mods <<<"$ALSO"
  for m in "${mods[@]}"; do
    m="$(printf '%s' "$m" | tr -d '[:space:]')"; [[ -z "$m" ]] && continue
    s="$(run_module "$m")"
    echo "  0. $m : $s   (required PROVED — this is where a panic VC actually lives)"
    [[ "$s" == PROVED ]] || extra_fail=1
  done
else
  echo "  0. (no --also modules given)"
  echo "     ⚠ WARNING: a driver alone can PROVE against an UNFIXED body, because it discharges"
  echo "     against its callees' contracts. For a totality obligation, pass the callee modules with"
  echo "     --also or this oracle can certify a repair that did not happen."
fi
V="$(run_module "$VERIFY")";  echo "  1. $VERIFY : $V   (required PROVED)"
M="$(run_module "$MUTANT")";  echo "  2. $MUTANT : $M   (required FAILED)"
R="$(run_module "$REFUTE")";  echo "  3. $REFUTE : $R   (reported; gated only with --refutation-must-die)"

fail=$extra_fail
(( extra_fail )) && echo "  ✗ a module the obligation rests on does NOT prove — the defect is still present."
[[ "$V" == ERROR ]] && echo "  ✗ verify_ did not run (build/setup error, shown above) — this is the environment, not the proof."
[[ "$V" == PROVED ]] || { echo "  ✗ the obligation is NOT proved. A repair without a proof is unvalidated."; fail=1; }
case "$M" in
  FAILED) : ;;
  ERROR)  echo "  ✗ INCONCLUSIVE: the anti-vacuity twin did not run (build/setup error, shown above)."
          echo "    A twin that never ran is not a failure. Fix the environment and re-run."; fail=1 ;;
  ABSENT) echo "  ✗ no anti-vacuity twin. Author $MUTANT — an unfalsifiable proof is not evidence."; fail=1 ;;
  PROVED) echo "  ✗ VACUOUS: the mutant also proves, so $VERIFY has no content."; fail=1 ;;
  CRASH)  echo "  ✗ INCONCLUSIVE: the mutant CRASHED rather than failing on its assertion. A crash is"
          echo "    not a refutation — Kani/CBMC render one as a failure, which is how a crashed twin"
          echo "    gets miscredited. Shrink the harness or raise the budget, then re-run."; fail=1 ;;
esac
if (( REFUTATION_MUST_DIE )); then
  [[ "$R" == FAILED || "$R" == ABSENT ]] || { echo "  ✗ the violation-shaped refutation STILL proves: the defect is still reachable."; fail=1; }
else
  [[ "$R" == PROVED ]] && echo "     note: still proves — expected for a PREMISE-shaped refutation; confirm the shape by reading it."
fi

if (( fail )); then echo "ORACLE: FAIL"; exit 1; fi
echo "ORACLE: PASS — obligation proved, twin correctly fails, refutation left intact."
