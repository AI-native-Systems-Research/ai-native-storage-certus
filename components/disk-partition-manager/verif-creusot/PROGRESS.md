# Creusot (Role 2) — disk-partition-manager

Pin `d14dfb9a`. Inventory `verif/unified_properties.yaml`, **M = 102 verifiable** (+25 NV),
consumed by `id` — not re-extracted.

Crate: `components/disk-partition-manager/verif-creusot/` (`disk-partition-manager-verif`),
modules emitted to `verif/disk_partition_manager_verif_rlib/*.coma`.
`creusot-std` patched **at manifest level** (never `components/<c>/.cargo/config.toml`).

## Why a mirror and not the product crate
`src/gpt.rs` + `src/lib.rs` cannot be translated by creusot-rustc: `Arc<dyn IBlockDevice + Send
+ Sync>`, `Mutex<Option<PartitionTable>>`, the crossbeam `command_tx`/`completion_rx` pair,
`DmaBuffer`, `crc32fast::hash`, `std::fs::File::open("/dev/urandom")`,
`String::encode_utf16`/`from_utf16_lossy`, and the `define_component!` expansion are all
outside the model. Every obligation is re-expressed over a faithful pure-core mirror with the
source line cited on each function.

## Model layer (proved before any driver)
| mirror | source |
|---|---|
| `cdiv`/`cdiv32`/`cdiv64` | `div_ceil` (no creusot-std contract — measured) |
| `entry_sectors` | gpt.rs:324-327 |
| `usable_window` | gpt.rs:157-158 |
| `count_rest` / `sum_fixed` / `layout_classify` | gpt.rs:251-277 |
| `layout_place` / `compute_layout` | gpt.rs:279-322 |
| `project` | gpt.rs:132-144 **and** 219-231 (identical) |
| `write_gpt_trace` / `try_read_gpt_trace` | gpt.rs:204-216 / 103-120 |
| `protective_mbr` | gpt.rs:329-355 |
| `encode_units` / `decode_units` | gpt.rs:576-595 |
| `generate_guid` | gpt.rs:564-574 |
| `try_read_gpt_at_m` / `read_gpt_m` | gpt.rs:98-150 / 66-96 |
| `initialize_m` / `format_m` / `partition_info_m` / `num_partitions_m` | lib.rs:68-119 |
| `initialize_or_format_m` / `get_ns_id_m` / `set_ns_id_m` | lib.rs:45-64 / 30-36 |

Induction lemmas: `lemma_cdiv_nonneg`, `lemma_placed_split`, `lemma_fixed_sum_{nonneg,mono}`,
`lemma_rest_count_{nonneg,mono}`, `lemma_placed_{nonneg,mono}`,
`lemma_occupied_{nonneg,mono}`, `lemma_crc_field_roundtrip`.

## Log
- model layer green: 34 `.coma` modules proved.

## Drivers
All 102 verifiable ids have a runnable proof unit:
- **91** `verify_<id>` + `verify_<id>__mutant` (anti-vacuity twin; `cargo creusot` collapses the
  double underscore in the emitted filename — expected, the scorer resolves both spellings).
- **11** `refute_<id>` — the NEGATION proves, so the code violates the obligation:
  `DPM-INIT-ERR-IO`, `DPM-FORMAT-PRE-MAX-128`, `DPM-FORMAT-POST-TABLE`,
  `DPM-FORMAT-POST-GUID-FRESH`, `DPM-PINFO-POST-INDEX-ECHO`, `DPM-INV-NO-OVERLAP`,
  `DPM-INV-GUID-DISTINCT`, `DPM-INV-SECTOR-SIZE-DOMAIN`,
  `DPM-INV-PARTITION-INDEX-SEQUENTIAL`, `DPM-INV-NUM-SECTORS-POSITIVE`,
  `DPM-INV-NSID-CONSISTENT`.
  No refuted id also ships a `verify_<id>` (the scorer would call that a contradiction), and none
  of the refutations rests on a stub: the GUID pair uses the source's own `if let Ok` failure
  path, everything else runs the real mirrors on satisfiable concrete inputs.

Advisory side-file: `verif/creusot_advisory.yaml` (fidelity / note / evidence.modules only).
`verif/unified_properties.yaml` is NOT touched — the Kani agent works concurrently and the
scorer owns every verdict.

## Proof-engineering notes from this run
- `#[logic]` recursive definitions needed `#[logic(open)]` to be transparent at every use site.
- A monotonicity lemma must come in the QUANTIFIED form (`forall j <= n`) for a loop invariant to
  cite it; the per-pair form is useless there (`lemma_occupied_mono`).
- Non-negativity needed its OWN lemma with `#[variant(n)]`; folding it into a `#[variant(k - j)]`
  monotonicity lemma leaves the base case unproved.
- `rest * rest_count` had to be case-split by hand (`rest_count in {0, 1}`); no prover did it.
- `#[bitwise_proof]` was required for the GUID version/variant nibbles.
- `div_ceil` has NO creusot-std contract ("impossible precondition" warning) — mirrored by hand.
- `vec!` is ambiguous under `use creusot_std::prelude::*` — import it explicitly.
- `#![recursion_limit]` had to be raised (the default blew up at ~18 `#[ensures]`, well below the
  ~31 the skill records).
- A `result == f_logic(args)` clause is needed whenever a driver compares two calls of the same
  mirror; the partial postcondition alone does not make the result a function of the inputs.

## Reproduction gate — SELF-CHECK RUN (the orchestrator's Step 2.5 is authoritative)
```
scorer_creusot.py <sandbox> --crate-dir components/disk-partition-manager/verif-creusot \
                  --cap-seconds 90 --cap-max 300
scorer_creusot: touched 1 src/**.rs to force from-source .coma regeneration (anti-tamper)
scorer_creusot: 258 generated .coma modules
SUMMARY: {'proved': 91, 'refuted': 11, 'tool-boundary': 0, 'delegated': 0, 'UNRESOLVED': 0}
CREUSOT GATE: PASSED — every verifiable property is proved / tool-boundary / delegated,
                       each scorer-reproduced
```
Measured over the 102 scored modules: 63.4 s total wall clock, median 0.51 s, max 4.80 s;
peak RSS median 53 MB, max 247 MB. Toolchain: creusot v0.11.0-169-g9cf662ce6,
Why3 1.8.2+git, provers Alt-Ergo 2.6.2 / CVC4 1.8 / CVC5 1.3.1 / Z3 4.16.0.

The scorer was pointed at a THROWAWAY COPY of `unified_properties.yaml`, never at the shared
file: the Kani agent reads it concurrently, and an agent-run status block must not rival the
one the orchestrator's gate writes. That copy has been deleted; nothing in this tree carries a
`status` / `symbol` / `_scored_by` field.

Whole-crate `cargo creusot`: 91 unproved files, and that set is EXACTLY the 91 `__mutant`
twins — every base proof and every refutation discharges, every anti-vacuity twin goes red.

Artifacts: 274 `.coma` files (258 distinct module basenames) in
`verif-creusot/verif/disk_partition_manager_verif_rlib/`.

## J4 (level-2 exact-range re-proof, 2026-10-08)
The 17 proof-carries-range drivers of `verif/.run/t3_dependents.yaml` now assume only the
declared `assume_rust` (per-path conjunct, pearlite-faithful) or the obligation's own words;
derived premises (`ns >= 1`, `>= 2E+4`, `first/last` bounds, `total_usable >= 1`,
`nbytes/len >= 1`, `lba + n <= u64::MAX`, `pre.len() >= 92`, literal 128/128, implicit
config==device / config-ns==read-ns) are gone. Model-layer changes are weakenings only:
`project` requires the entry ranges over OCCUPIED slots; `layout_classify` drops
`total_usable >= 1` and needs the fixed-sum bound only when `rest_count <= 1`. New mirror
`parse_entries_m` (gpt.rs:387-405). Every changed driver's `__mutant` is now the driver
verbatim with its first ensures negated. Forced whole-crate run: 91 unproved == the 91
`__mutant` twins; 0 `#[trusted]`. Advisory: `verif/.run/j4_advisory.yaml`.

## J13 (level-2 classifier-A fresh re-proof, 2026-10-08)
The 10 ids of `verif/.run/j13_A_ids.txt` re-audited and re-proved; all 10 credited. New mirrors
(additions only, no existing contract changed): `write_gpt_m` (gpt.rs:152-216 incl. the
too-small check and the post-layout write trace), `place_type_guids` (the `type_guid` slice of
gpt.rs:279-319), `parse_entry_offsets` (gpt.rs:387-405 offsets), `lemma_u32_product`.
Removed premises: the six `layout_place` success premises (TYPE-GUID-ECHO), `ns >= 1`
(BACKUP-LOCATION-UNDERFLOW), `entries*size <= u64::MAX` (STRIDE-MISMATCH). Free-boolean /
identity drivers replaced by computed mirrors (FRAME-NO-WRITE-ON-REJECT,
IGNORES-DEVICE-GEOMETRY); STRIDE-MISMATCH's old `bytes/128` count was wrong (ignored the
`0..count` bound) and is replaced. Widened: INIT-ERR-NO-TABLE (blank drive), IOF-POST-MISSING-
FORMATTED (CorruptTable too), INIT-POST-BACKUP-FALLBACK (header-too-short excluded via SS).
All 10 mutants = driver verbatim, first ensures negated. Forced whole-crate run: 91 unproved ==
the 91 `__mutant` twins; 0 `#[trusted]`. Advisory: `verif/.run/j13_advisory.yaml`.
