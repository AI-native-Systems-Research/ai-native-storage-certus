---
name: tools-aggregate-coverage-by-interface
description: Render the two fixed presentation views (bundle sizes + per-method scoreboard, certus_fv slides 8 and 9) for a component by overlaying tool proof-status onto the per-method property bundles taken from its property inventory. Consumes the Role-1 `build-property-inventory` output and the Role-2 `verify_*_with_properties` status files, attaching status by `id`. Does NOT extract, bundle, or prove — the inventory owns the property set and the bundles.
argument-hint: "[component-name]"
---

## Purpose

Given a component whose verifiable properties are already extracted and agreed, produce a
**consistent, presentation-ready coverage rollup** whose aggregation unit is fixed:
the **public methods of the `I<component>` trait** (the `define_interface!` block).

The deck already fixes this format — do not reinvent it:
- **Slide 8** ("We verify a public method by verifying its whole bundle of properties"):
  one line per public method with its **bundle size**. Message: *to verify a method, we verify every
  property in its bundle.*
- **Slide 9** ("How many public methods did we verify?"): the **scoreboard** — one row per public
  method, `Creusot | Kani`, **two states `✓ / –`**, and the headline
  *"Out of N public methods, Creusot proved X and Kani proved Y."*

This skill **does not extract properties and does not run a prover.** It consumes an agreed property
set and emits the rollup. If the inputs disagree, it stops and asks for reconciliation.

## The model (match the deck exactly)

- **Unit of coverage = public method.** The denominator on the scoreboard is the **number of public
  methods** (e.g. 16/20, 12/20), NOT a property count.
- **A method is `✓` for a tool** when every property in that method's bundle is **covered in its
  assigned lane**, and that tool is the one carrying the method's proof-lane properties. It is `–`
  when the tool has nothing to prove for it, or has provable properties it did not (yet) prove.
- **Every property is assigned exactly one lane** (from slide 7's four lanes):
  `Kani` · `Creusot` · `Loom/Spin` (concurrency) · `Targeted` (fault-injection / hardware / perf).
  A property in a non-proof lane (Loom/Targeted) does **not** block a tool's `✓` **iff that lane
  actually covers it** — exactly as `create_memory_tier_entry` stays `✓` for Creusot while its
  SSD-atomicity property is covered by a targeted test.
- **A property in the Creusot/Kani lane that is not yet proved DOES block that method's `✓`.** That is
  an unfinished bundle, reported honestly as `–` — never relabel it covered.
- **Footnotes (keep them):** `*` = this tool is the better-fit prover for the method; `**` = the
  method's logic lives in other components (those rows are `– / –`).
- **Scoreboard has no `◐`.** Partial coverage, assumptions, and trusted boundaries go on a **separate**
  view ("What our proofs cover — and what they assume", slide 11), never in the count.
- **Display grouping by capability area is allowed** (slide 9 groups Reference-counting, Tier-transitions,
  …) — it is *visual* only. The counting unit stays the public method.

## Inputs (consume; do NOT re-extract, do NOT re-bundle)

1. **The property inventory** `verif/<component>_property_inventory.md` (Role 1,
   `build-property-inventory`) — the single source of truth: id-keyed properties, their `methods`
   bundles, the bundle sizes, and the counts (distinct M + attachments). This skill **reads** those
   bundles; it does **not** assign owners or compute bundle sizes.
2. **The Role-2 status files** — `verify_creusot_with_properties` / `verify_kani_with_properties`
   outputs `<component>_properties.md`, each line keyed to an inventory `id` with its lane/status.

Attach statuses to inventory properties **by `id`**. If a status file cites an `id` absent from the
inventory, or the two disagree on an obligation — **STOP and flag it**; never paper over divergence,
invent a property, or re-bundle. Reconciliation of the property *set* already happened in Role 1.

## The property record (schema owned by Role 1; this skill overlays only lane + status)

`id`, `methods` (the bundle), `object_fn`, `kind`, `statement`, `source`, and `origin` all come **from
the inventory** — read them, never re-derive. This skill adds exactly two fields per property, `lane`
and `status`, sourced from the Role-2 status files and attached by `id`:

```
  # (from inventory) id, methods, object_fn, kind, statement, source, origin
  lane:      Kani | Creusot | Loom | Targeted     # the ONE lane responsible
  status:    proved | not_yet | covered_by_lane   # proved (proof lane) / not proved / covered by Loom|Targeted
```

### Bundle ownership (read from the inventory — do not recompute)

- The `methods` bundle of each property, the per-method bundle sizes, and both counts (*distinct
  properties* M, *method→property attachments*) are **already computed in the inventory**. Read them.
- A shared-helper property (e.g. `align_up`) is already attached to every dependent method there — carry
  that through unchanged; it is expected for one property to sit in many bundles.
- The counting unit stays the public method. If you feel a need to reassign an owner, that is an
  inventory fix (Role 1), not something this skill does silently.

## Method verdict (per tool)

For public method `m` and tool `T` (Creusot or Kani):
- `✓` if **every** property in `m`'s bundle is `proved` (by any proof tool) or `covered_by_lane`
  (Loom/Targeted), **and** `T` proved at least one of `m`'s proof-lane properties.
- `–` otherwise (T proved nothing for `m`, or a proof-lane property of `m` is still `not_yet`).

Component score: `Creusot X / N`, `Kani Y / N`, where **N = number of public methods**. Also report
the net: **methods whose whole bundle is covered in-lane (done) vs incomplete**.

## Required outputs

1. **Coverage rollup** `<component>_properties_by_interface.md` — the inventory's bundles with `lane`+`status`
   overlaid by `id`, the per-method bundle rollup, both counts, per-method `✓/–` per tool, and the score.
   Renders from inventory + status files; it is **not** a re-reconciliation (the inventory is the property
   source of truth). The HTML views render from this rollup.
2. **Bundle view** (slide 8) `<component>_property_bundles.html` — one line/card per public method with
   its bundle size and the properties (each with lane + status).
3. **Scoreboard** (slide 9) `<component>_interface_api_verification.html` — per-method rows grouped by
   area, `Creusot | Kani` `✓/–`, headline "Out of N methods, Creusot X, Kani Y", `*`/`**` footnotes.
4. **Assumptions view** (slide 11) — what is proved vs assumed/trusted, and which lane covers each gap.
   This is where `not_yet` and `covered_by_lane` detail lives — NOT the scoreboard.

HTML must be **self-contained and theme-aware**; deliverables are scp'd and cut onto plain white slides
(keep backgrounds light, avoid heavy colored bands). Reference implementations: the per-component
`<component>_interface_api_verification.html` / `<component>_property_bundles.html` deliverables.

## Honesty rules (non-negotiable)

- **A method is `✓` only when its whole bundle is covered in-lane.** An unproved proof-lane property
  makes the method `–`. Do not inflate the method count by ignoring unfinished properties.
- **N = number of public methods**, fixed by the trait. Never shrink N to improve the ratio.
- Every `proved` claim traces to a real artifact (VC/`.coma` or harness) via the property `id`.
- `covered_by_lane` must name a real Loom model or targeted test — otherwise it is `not_yet`.
- Under-claim, never over-claim.

## Procedure

1. Read the public methods, N, and the `methods` bundles **from the inventory** (do not recompute them).
2. Attach each Role-2 status to its inventory property **by `id`**; flag any id present in a status file
   but absent from the inventory (or a disagreement); stop if unresolved.
3. Set each property's `lane` + `status` from the status files (its owner/bundle is already fixed by Role 1).
4. Compute each method's `✓/–` per tool, the score X/N and Y/N, and the done/incomplete split.
5. Write the canonical `.md`; render the three HTML views from it.
6. Report: `<component> — Creusot X/N, Kani Y/N public methods proved; Z methods have complete bundles,
   (N−Z) incomplete (list the blocking not_yet properties)`.

## Anti-patterns (the rabbit holes)

- ❌ Counting **properties** as the scoreboard denominator. (Denominator = public methods.)
- ❌ Using `◐` on the scoreboard. (Two-state ✓/–; partial detail lives on the assumptions view.)
- ❌ Marking a method `✓` while a proof-lane property is still `not_yet`.
- ❌ Re-extracting or extracting differently per tool. (Extraction is upstream and shared.)
- ❌ Re-bundling or reassigning method owners here. (Bundles come from the inventory; a fix goes to Role 1.)
- ❌ Guessing a round property/method count to fit a table.

## Routing

General methodology → `unstable` (promote via PR; don't leave siloed on a verif branch).
Generated `.md`/`.html` reports → PR to `unstable`.
