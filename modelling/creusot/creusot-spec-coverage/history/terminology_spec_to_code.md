# Spec-to-Code Traceability Terminology (Plain English)

This note explains the terms used in our automated traceability outputs.

## What we are doing

We are connecting three things:

1. **Specification properties** (`P1..Pn` in `first_properties.md`)
2. **Verification artifacts** (coverage matrix + verification plan + assumptions)
3. **Implementation-facing evidence** (functions/tests/proofs that should satisfy each property)

Goal: make it easy to answer, for each property, whether it is covered, how it is covered, and what assumptions remain.

## Starting Steps (Process Front-End)

Before writing verifier contracts, we use two explicit front-end steps:

1. **Extract truths from spec**  
   Create `extracted_thruths_from_spec.md` with `Tx` lines (`Tx: abbreviation: description`) that capture non-negotiable semantic truths.
2. **Derive properties from truths**  
   Create `properties_based_on_truths.md` with rows `Pxx -> based on Tx` so each formal property has clear semantic provenance.

This improves reviewer understanding and makes blog/paper narratives much clearer.

## Terms

### Coverage

The status of a property in the coverage matrix:

- `Covered`: represented and claimed covered in the current model/matrix.
- `Partial`: some intent is captured, but important parts are simplified or missing.
- `Not covered` / `Missing`: no meaningful coverage entry yet.

### Coverage row

A single row in the coverage matrix table (for one property).

### Properties missing coverage row

Properties that exist in `first_properties.md` but have no mapped row in the coverage matrix.

Interpretation: this is a direct gap to fix next.

### Coverage rows not in property list

Rows present in the coverage matrix that do not map to the current property list.

Interpretation: either stale rows, naming drift, or list mismatch.

### Properties with weak verif_plan link

Properties where our lightweight keyword heuristic did not find a strong textual match in `verif_plan.md`.

Important: this is a **review signal**, not a formal proof failure.
It usually means we should add clearer property IDs/tags in the plan.

### Covered (bounded)

The property is covered under a bounded abstraction, such as:

- finite number of keys (`Cache2`-style model),
- finite number of steps (`N`-step eventuality).

Interpretation: useful evidence, but weaker than an unbounded/general proof.

### Assumption link

A mapping from property ID (`Pxx`) to assumption ID (`Axx`) in the assumption ledger.

Interpretation: this property currently depends on stated assumptions; removing the assumption is future proof work.

### Traceability matrix

A generated table that shows, per property:

- spec references (FR/US/AS),
- statement text,
- coverage status,
- mapped artifacts/functions,
- linked assumptions,
- whether it is visible in verification plan text.

## Why this matters

Without explicit traceability, it is easy to claim coverage that is hard to audit.
With explicit traceability, reviewers can quickly see:

1. what is proven,
2. what is only partial/bounded,
3. what assumptions are still carrying proof strength,
4. what remains unverified.
