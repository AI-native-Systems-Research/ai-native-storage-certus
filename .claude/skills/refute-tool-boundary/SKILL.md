---
name: refute-tool-boundary
description: Adversarially attack a single formal-verification tool-boundary claim. Given a component, a tool (creusot|kani), and a property id the scorer scored `tool-boundary`, this skill runs as an INDEPENDENT agent (different from the one that produced the artifact) whose only job is to BREAK the wall — apply the full lever arsenal plus anything the battery has not yet encoded, and either land a reproducible proof (refuted) or exhaust every avenue (upheld). Separation of powers: the prover argues the wall is real; this skill argues it is not. Invoked by component-verify Step 2.6, once per surviving tool-boundary.
argument-hint: "<component> --tool creusot|kani --id <PROPERTY_ID> [--cap-seconds N]"
---

> **Frozen 2026-10-09.** This skill moved to [ai-native-storage-workbench](https://github.com/AI-native-Systems-Research/ai-native-storage-workbench) (`fv/skills/`). Edit it there; this copy is not updated. Install the current version with `fv/install.sh <this-repo>`.


## Purpose
A `tool-boundary` is the single most abusable verdict in the pipeline: it is the agent saying "the tool cannot do this," which ends the work. This skill exists so that claim is never taken on trust. You are a **fresh adversary** — you did not write the artifact under review and you do not get to agree with it. Your success condition is a **reproducible proof that the wall was false.** If you cannot produce one after exhausting the arsenal, the boundary stands — but only then.

Assume the boundary is wrong until the tool defeats you too. Every wall in `known_defeats.yaml` was once someone's confident tool-boundary.

## Inputs
- `<component>` — matches `components/<component>/`.
- `--tool creusot|kani` — which tool's boundary is under attack.
- `--id <PROPERTY_ID>` — the property the scorer scored `tool-boundary` (its `evidence.signature` is your starting intel).
- `--cap-seconds N` — per-attempt wall-clock cap (inherit the orchestrator's).

## Step 0 — Environment (mandatory, same as the verify skills)
- Kani: `command -v cargo-kani` or stop; run all `cargo kani` from `components/<component>/`.
- Creusot: `export PATH="$HOME/.local/share/creusot/bin:$PATH"`; `why3 config list-provers` must show alt-ergo/z3/cvc5/cvc4 (else `why3 config detect`, else stop — a missing prover is an environment fault, never a confirmed boundary).

## Step 1 — Read the claim and its intel
From `components/<component>/verif/unified_properties.yaml`, read the property's tool block: the captured `evidence.signature`, `note`, `fidelity`, and the artifact it ran. Read the failure class the signature falls under in `gate/lever_battery_<tool>.yaml`, and check `gate/known_defeats.yaml` — **if the signature matches a known defeat, the boundary is already refuted by precedent**: apply the mandated lever and you are done (report refuted, cite the KD).

## Step 1.5 — If the boundary is an INEXPRESSIBILITY verdict, attack the *expressibility claim*
A `tool-boundary` whose evidence carries `fidelity: not-expressible` (the property had
`creusot.claims_inexpressible: <IX-ID>`, confirmed by the scorer's isolated probe) is a different kind of
wall: the scorer already proved the *tool fact* is real on this toolchain, so **re-running provers will not
help**. Your job is the other half — prove the **property did not need that construct**.

1. **Read the construct** in `gate/inexpressible_creusot.yaml`: its `authority`, and especially its
   `implication` line, which states exactly what IS still expressible (for `IX-CREUSOT-STRING-CONTENT`:
   length, structure via `split_at`/`deref`, and view-to-view relations `s@ == t@`).
2. **Re-express the real obligation inside that envelope, faithfully.** Write a runnable proof module
   (`verify_<id>` / `lemma_<id>`) that captures the property using only expressible terms — e.g. a length
   relation (`out@.len() == …`), a structural relation via `split_at`, or an input/output view equality
   (`format(t,m)@` determined by `t@`,`m@`). If a faithful reformulation **proves**, the property was
   expressible after all → **refuted** (leave the module for the scorer to reproduce).
3. **Faithfulness is the whole game.** A reformulation that *weakens* the obligation (checks only length
   when the property is about which bytes appear, asserts a tautology, drops the tag-presence claim) is a
   **different, weaker property — not a refutation.** State plainly what your module proves and confirm it
   entails the original obligation; if it does not, you have not refuted the wall.
4. **Semantic-match audit.** Independent of proving, judge whether the property *genuinely requires* the
   inexpressible construct or was mislabelled to dodge work. A property that is really about a stateable
   length/structure fact, or is actually just *hard to prove* (a solver miss with no translate/ICE), does
   **not** belong to this construct → report `refuted` (mislabelled) and route it to the battery.

Verdict for this step: **refuted** if you produced a faithful, proving, expressible reformulation OR showed
the property does not need the construct; **upheld** only if every faithful attempt to state the obligation
still requires the concrete content the model cannot represent — in which case the covering tool named in
the construct's `covered_by` (here Kani, bounded bytes) must carry it, and you say so.

## Step 2 — Attack with the full arsenal (do not stop at the first miss)
Work through every lever the battery lists for the failure class, AND the general levers below that a battery may not yet encode. Produce each attempt as a **runnable artifact named for the scorer** (`verify_<id>__<lever>` / `lemma_<id>` / a strengthened `verify_<id>`), so a success is reproducible by the scorer, not just by you.

Kani arsenal: stub the opaque/expensive dependency (`kani::stub`, `-Z stubbing`); sweep `--unwind` across a range and at the cap; `--no-unwinding-checks` for the bounded-shallow prefix; swap solver (minisat ↔ cadical ↔ kissat); replace symbolic keys/values with concrete representatives; split one over-broad harness into per-effect harnesses; shrink the symbolic surface to the arithmetic/logical core while keeping the real type.

Creusot arsenal: run the full prover portfolio (`-P alt-ergo,z3,cvc5,cvc4`); raise budget (`--time`, `--depth`); apply tactics (`-T split_vc,compute_specified`); model std containers as a logic `FMap`/`Seq` ghost mirror; put an opaque type (raw ptr, `Arc`, `*mut`, FFI, trait object) behind a `#[trusted]` u64-handle boundary; strengthen loop invariants / add intermediate assertions; extract an inductive `#[logic]` proof lemma over the unbounded structure and cite it.

Cross-tool move: if one tool genuinely cannot express the obligation but the **other** can (Kani bounded-shallow ↔ Creusot FMap induction is the canonical pair), that is a *delegation with a resolvable referent*, not a tool-boundary — record it as `delegated` to the other tool/component with the concrete obligation.

## Step 3 — Verdict (reproducible, or it did not happen)
- **Refuted.** You produced an artifact that verifies. Leave it in the tree under the scorer's naming convention, name the lever that worked, and report `refuted` — the orchestrator will re-run the scorer (which must independently reproduce your proof) and append the beaten signature + lever to `known_defeats.yaml`. A refutation the scorer cannot reproduce is not a refutation.
- **Upheld.** You applied every lever for the class as runnable artifacts and each still failed, and the residual signature is genuinely novel (not in `known_defeats.yaml`). Report `upheld` with your own captured signature(s) and the exhaustive list of levers you ran. Two independent agents failing the same wall with captured evidence is what makes `⊘` legitimate.

Never report `upheld` on the basis of reasoning alone. "I believe the tool cannot do this" is not a result; a battery of failed runnable artifacts is. If you did not run a lever the battery requires, you are not done — run it.

## Anti-patterns
- ❌ Agreeing with the original claim because it sounds plausible. You are the adversary; make the tool prove you wrong.
- ❌ A refutation that only you can reproduce (uncommitted flags, hand-edited `.coma`). The scorer regenerates from source — so must your proof.
- ❌ Reporting `upheld` without having run every required lever as a named artifact.
- ❌ Treating a missing prover / unreachable toolchain as a confirmed boundary. That is an environment fault (Step 0).
- ❌ "Refuting" an inexpressibility verdict by proving a **weaker** property (only length, a tautology) that does not entail the original obligation. That is a different property, not a refutation (Step 1.5).
- ❌ Re-running provers against a `not-expressible` boundary and reporting `upheld` when they miss — the scorer already confirmed the tool fact; the only refutation is a faithful *expressible* reformulation that proves (Step 1.5).
