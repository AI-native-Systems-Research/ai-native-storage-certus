# Verified properties — disk-partition-manager (Creusot)

Proven from the Role-1 inventory `disk-partition-manager_property_inventory.md`
(N=4 trait methods, M=35 properties) against code
`components/disk-partition-manager/src/{gpt.rs,lib.rs}` and
`components/interfaces/src/ipartition_table.rs`. Artifacts: this `verif/` crate.
Branch `verif/creusot/disk-partition-manager-rerun`, clean-slate re-run based on
baseline `verif/creusot/disk-partition-manager @ 2acb0dce`. The baseline branch is
left **untouched**; this re-run stripped the proof crate to bare signatures
(`92056f46`), re-authored every contract from the id-keyed inventory (`dea72b06`),
then closed the two obligations the baseline left authorable-but-unwritten
(`d42c364f`, `verif/src/loops.rs`).

**Method.** Each function in `src/*.rs` is a *faithful whole-function mirror* of
a shipped function: the disk-partition-manager crate cannot be built under
Creusot (it depends on `interfaces`, `component_macros`, `crc32fast`,
`std::sync::Mutex`, SPDK/DMA channels and `/dev/urandom`), so each mirror
reproduces the source body and attaches the inventory-derived contract. Every
mirror's doc comment cites the exact source line range it tracks.

**Honesty caveat.** A green proof here covers the **mirror**, not the shipped
function. The residual gap is the body-equality obligation between mirror and
source, discharged **by inspection** against the cited line ranges. There are
**no `#[trusted]` items inside this crate** — every VC is discharged by the
prover portfolio.

**Result.** `cargo creusot` → `Proved (48 files) ✔`.
**70 named VCs across 48 `.coma` files** (baseline: 57 VCs / 43 files; +13 VCs /
+5 files, all from the new `loops.rs` whole-`Vec` induction). Under the
`compute_specified` + `split_vc` tactics those 70 named VCs expand into **136 leaf
goals discharged by the SMT portfolio: alt-ergo 126, cvc5 7, z3 3** (cvc5 carries
the two `#[bitwise_proof]` GUID VCs; z3 carries three error-path booleans). Six of
the 48 files are auto-derived `Clone` impls (1 leaf each). `why3find.json`: fast
0.2, time 8, depth 6.

**Measurement** (`/usr/bin/time -v cargo creusot`, this re-run):
- **Cold / clean run:** wall-clock **24.91 s**, peak RSS **≈ 803 MB** (822 220 KB).
- **Cached replay:** wall-clock **0.80 s**, peak RSS **≈ 72 MB** (73 732 KB).
- Baseline `2acb0dce` for comparison: cold **24.33 s** / 821 956 KB.

**Anti-vacuity (fault injection).** The clean-slate **strip** step is itself the
first anti-vacuity witness: with contracts removed the tree was honestly RED
(11 unproved files — the layout/roundtrip arithmetic mirrors losing their
overflow/underflow `#[requires]`), proving those preconditions are load-bearing.
On top of that, **six representative proofs across all proof styles** were
fault-injected, each confirmed to turn its VC **red**, then reverted:
- **FI-1** count-fold — `count_rest` body `if x == 0` → `if x != 0` reddened `vc_count_rest`;
- **FI-2** whole-`Vec` placement — `place_all` cursor step `current + n` → `current + n + 1` (breaks the budget invariant) reddened `vc_place_all`;
- **FI-3** G6 arithmetic — dropping the `-1` in `ending_lba` reddened `vc_ending_lba`;
- **FI-4** GUID bit-level — `0x40 → 0x50` (would-be version 5, sets bit 4) reddened `vc_guid_version4`;
- **FI-5** state gate — disabling the `!present → NotInitialized` guard reddened `vc_partition_info`;
- **FI-6** multi-rest guard — `multi_rest_ok` `rest_count > 1` → `> 2` reddened `vc_multi_rest_ok`.
Re-running clean after all reverts restored `Proved (48 files) ✔`.

Build (creusot-std is not vendored on this branch; `Cargo.toml` patches it to
`/home/cornel/ai-native-storage-certus/tools/creusot/creusot/creusot-std` — a
read-only path reference that makes the crate **non-portable as-committed**, the
same known gotcha as the em/mt verif branches):
```
cd components/disk-partition-manager/verif && cargo creusot
```

---

## Per-method scoreboard (proved m of bundle B)

Counts follow the inventory's bundle sizes. A property is counted **proved** when
its Creusot-expressible core is discharged green — for a *mirror-with-trusted-
boundary* (★) the arithmetic/logic shape is proved and the residual is a
disclosed TCB effect (see the ledger). Not-counted rows are `G1` (spec-only
precondition the code does not enforce — R3), the two I/O-effect frames
(not expressible), and the one authorable Vec-count.

| method | bundle B | proved m | not counted |
|--------|:---:|:---:|-------------|
| `initialize` | 12 | **10** | G1 (R3); `DPM-INIT-IO-2RT` (TOOL ⊘) |
| `format` | 24 | **22** | G1 (R3); `DPM-FORMAT-IO-O1` (TOOL ⊘) |
| `partition_info` | 6 | **5** | G1 (R3) |
| `num_partitions` | 4 | **3** | G1 (R3) |
| *(helper, out-of-N)* `initialize_or_format` | 4 | **4** | — |

`format` rose **21 → 22** this re-run: `DPM-FORMAT-ERR-MULTI-REST` was the sole
baseline AGENT ⧗ item and is now **Proved** (`loops/multi_rest_layout_ok`, see
below). Only `DPM-FORMAT-IO-O1` (a TOOL ⊘ I/O-count frame) and the G1 R3-note
remain non-native. There are **no remaining AGENT ⧗ items** in this component.

The `G1` deduction is the *same* global appearing in all four bundles (attach 4);
it is not four separate misses (see G1 note below).

---

## Global invariants (§1 worklist, discharged once, by attachment order)

| id | attach | status | evidence (`.coma`) |
|----|:---:|:---:|--------------------|
| **G1** `DPM-SECTOR-SIZE-512-4096` | 4 | R3 note | *not enforced by code* — code is sector-size-generic. The arithmetic G1 would gate is proved **generically** for any `sector_size > 0` (`layout/entry_sectors`, `layout/part_size_ceil`), which is *stronger* than the spec's {512,4096} restriction. |
| **G6** `DPM-ROUNDTRIP-OFFSETS` (crown jewel) | 2 | ✓ | `roundtrip/sector_count_roundtrip` (3 VC), `ending_lba`, `read_num_sectors`, `reported_index`, `typeguid_roundtrip` — write→read is the identity on sector counts/start-LBA/index/type-GUID. Byte serialization layout ★. |
| **G2** `DPM-INIT-GATE` | 2 | ✓ | `state/partition_info`, `state/num_partitions` — `state=None ⇒ NotInitialized`. |
| **G3** `DPM-STATE-CACHED` | 2 | ✓ | `state/cache_state` — `Ok(t) ⇒ state=Some(t)` and returned == cached. Mutex ★. |
| **G4** `DPM-IO-PROPAGATE` | 2 | ★ | `errors/map_io_err` — any boundary failure surfaces as `IoError`. The I/O effect itself is TCB. |
| **G5** `DPM-COUNT-INDEX-AGREE` | 2 | ✓ | `state/count_index_agree` (3 VC) — `partition_info(i)` Ok iff `i < num_partitions()`, one denominator. |
| **G7** `DPM-ROUNDTRIP-NAME` | 2 | ★ | `name/utf16le_unit_roundtrip` (3 VC), `name/ascii_unit_roundtrip` (3 VC), `encode_unit_le`, `decode_unit_le` — per-code-unit LE round-trip + ASCII single-unit survival. String/`char` iteration (`encode_utf16`/`from_utf16_lossy`) is TCB. |
| **G8** `DPM-CRC-INTEGRITY` | 2 | ★ | `errors/crc_gate` — mismatch ⇒ `CorruptTable`, match ⇒ accept. The `crc32fast` algorithm and byte-domain layout are TCB. |
| **G9** `DPM-NAMESPACE-ID` | 2 | ★ | `errors/get_ns_id` (2 VC) — configured ns id used, else default `1`. That block I/O *targets* it is an I/O frame (TCB). |

---

## initialize — 10/12

- `DPM-INIT-REQUIRES-BLOCKDEV` **[error]** unconnected receptacle → `NotInitialized`, no I/O. — ★ `errors/require_blockdev` (receptacle binding is TCB).
- `DPM-INIT-BACKUP-FALLBACK` **[post]** primary `CorruptTable` → backup tried. — ✓ `errors/read_gpt`.
- `DPM-INIT-BOTH-CORRUPT-NOPTBL` **[error]** neither valid → `NoPartitionTable`. — ✓ `errors/read_gpt`.
- `DPM-INIT-RETURNS-CORRECT-LAYOUT` **[post]** returned offsets/sectors/type-GUIDs match stored. — ✓ `roundtrip/*` (byte layout ★).
- `DPM-INIT-IO-2RT` **[frame]** ≤ 2 read round-trips. — **⊘ TOOL**: I/O round-trip counting is a resource/effect count, not expressible in Creusot's sequential logic. Route: instrumented integration test.
- + globals G1(R3), G3✓, G4★, G6✓, G7★, G8★, G9★.

## format — 21/24

- `DPM-FORMAT-REQUIRES-BLOCKDEV` **[error]** — ★ `errors/require_blockdev`.
- `DPM-FORMAT-DEVICE-TOO-SMALL` **[error]** `first_usable >= last_usable` → `LayoutError` before any write. — ✓ `layout/device_large_enough` + `layout/usable_lbas` (with the R4 panic-freedom precondition).
- `DPM-FORMAT-ERR-MULTI-REST` **[error]** >1 `size_bytes==0` → `LayoutError`. — ✓ **Proved this re-run** (`loops/multi_rest_layout_ok`, 3 VC; `loops/count_rest`, `loops/count_zeros`, `loops/multi_rest_ok`). The `size_bytes==0` tally is modelled by a right-recursive `#[logic]` `count_zeros` over `Seq<u64>` plus an index-free `for`-loop (`count_rest`) whose invariant `count@ == count_zeros(specs@, 0, produced.len())` ties the running counter to the fold; `multi_rest_ok` proves the `> 1` guard is `rest_count <= 1`, and `multi_rest_layout_ok` composes them so the observable decision is `count_zeros(whole Vec) <= 1`. (Was baseline's sole AGENT ⧗; now closed.)
- `DPM-FORMAT-ERR-OVERSUBSCRIBED` **[error]** fixed partitions exceed usable → `LayoutError` (two guards). — ✓ `layout/fixed_fits` + `layout/placement_fits`.
- `DPM-FORMAT-WRITES-5-STRUCTURES` **[post]** MBR@0, primary hdr@1, entries@2, backup entries@last_usable+1, backup hdr@last. — ✓ (LBA-target arithmetic) `layout/usable_lbas`, `header/backup_mirrors_primary`, `header/mbr_protective_type`. Write **ordering/effect** is TCB (★).
- `DPM-FORMAT-ENTRY-ARRAY-128x128` **[inv]** exactly 128×128 B, unused zero. — ✓ `layout/entry_array_128`.
- `DPM-FORMAT-HEADER-CONSTANTS` **[post]** signature/revision/92/128/128. — ✓ `header/header_constants`.
- `DPM-FORMAT-DISK-GUID-V4` **[post]** RFC-4122 v4 version+variant. — ✓ `guid/guid_version4` + `guid/guid_variant` (`#[bitwise_proof]`).
- `DPM-FORMAT-PART-GUID-V4` **[post]** — ✓ same functions (GUID generation is shared).
- `DPM-FORMAT-NAME-LEN-36` **[pre]** ≤ 36 UTF-16 code units. — ✓ `name/name_unit_offset` (every admitted unit fits the 72-byte buffer; `.take(36)` String truncation is TCB, ★).
- `DPM-FORMAT-TYPEGUID-PRESERVED` **[post]** written type-GUID == input spec's. — ✓ `roundtrip/typeguid_roundtrip`.
- `DPM-FORMAT-PART-SIZE-CEIL` **[post]** `ceil(size_bytes / sector_size)`. — ✓ `layout/part_size_ceil`.
- `DPM-FORMAT-RESTOFDISK-REMAINING` **[post]** rest partition consumes all remaining usable. — ✓ `layout/restofdisk_remaining`.
- `DPM-FORMAT-PART-NONOVERLAP` **[inv]** `start==prev.end+1`, starts strictly increasing. — ✓ inductive step `layout/place_step` + two-partition instance `layout/two_partitions_disjoint` (2 VC), **and now the full N-partition `Vec` loop** `loops/place_all`: the loop invariant `current@ >= first_usable@ + produced.len()` proves partition starts strictly increase across the *whole* sequence ⇒ pairwise disjoint. (Baseline's authorable-extension note is now closed.)
- `DPM-FORMAT-PART-WITHIN-USABLE` **[inv]** every partition in `[first_usable, last_usable]`. — ✓ step `layout/within_usable_step` **and** whole-`Vec` `loops/place_all`: the invariant `current@ + remaining@ == first_usable@ + total@` with `remaining@ >= 0` keeps the cursor `<= last_usable + 1`, so every placed partition ends `<= last_usable`.
- `DPM-FORMAT-BACKUP-MIRRORS-PRIMARY` **[inv]** backup `my_lba`/`alternate_lba` mirror primary. — ✓ `header/backup_mirrors_primary`.
- `DPM-FORMAT-IO-O1` **[frame]** write count O(1) in device size. — **⊘ TOOL**: I/O write-count is an effect count, not expressible in Creusot. Route: instrumented integration test.
- + globals G1(R3), G3✓, G4★, G6✓, G7★, G8★, G9★.

## partition_info — 5/6

- `DPM-PINFO-INDEX-RANGE` **[error]** `index >= len` → `InvalidPartition`; valid exactly `0..num`. — ✓ `state/partition_info` (3 VC).
- `DPM-PINFO-RETURNS-ENTRY` **[post]** in-range → clone of `partitions[index]`. — ✓ `state/partition_info`.
- `DPM-PINFO-READONLY` **[frame]** no mutation, no I/O. — ✓ `state/pinfo_readonly` (shared-ref signature, pure read).
- + globals G1(R3), G2✓, G5✓.

## num_partitions — 3/4

- `DPM-NUMP-RETURNS-LEN` **[post]** returns `partitions.len() as u32`. — ✓ `state/num_partitions` (2 VC).
- + globals G1(R3), G2✓, G5✓.

## initialize_or_format (helper, out-of-N) — 4/4

- `DPM-IOF-VALID-NOFORMAT` / `DPM-IOF-NOVALID-FORMATS` / `DPM-IOF-FORCE-FORMATS` **[post]** — ✓ `errors/initialize_or_format`.
- `DPM-IOF-CORRUPT-BRANCH-DEAD` **[inv]** the `CorruptTable` fallback arm is unreachable (R1). — ✓ `errors/read_gpt` proves `read_gpt` never returns `CorruptTable`; `errors/initialize_or_format` carries the `init != Corrupt` precondition, so the arm is dead.

---

## TOOL-vs-AGENT residual ledger (required deliverable)

**TOOL — Creusot genuinely cannot (2 properties, ⊘):**
- `DPM-INIT-IO-2RT`, `DPM-FORMAT-IO-O1` — I/O round-trip / write-count *frames*.
  These are resource/effect counts over the block-device channel; Creusot's
  sequential logic has no notion of "number of I/O operations." Route:
  instrumented integration test.

**AGENT — Creusot could, proof not yet authored (⧗): NONE.** Both baseline
authorable items were **closed this re-run** (`verif/src/loops.rs`):
- `DPM-FORMAT-ERR-MULTI-REST` — now Proved: `count_zeros` (`#[logic]` fold) +
  `count_rest` (loop-invariant tally) + `multi_rest_ok`/`multi_rest_layout_ok`.
- Full **N-partition loop induction** for `DPM-FORMAT-PART-NONOVERLAP` /
  `-WITHIN-USABLE` — now Proved over the whole `Vec` by `loops/place_all`, not
  just the step + two-partition instance.

**Trusted boundaries disclosed inside proved mirrors (★ — the TCB for this
component, §7 of the inventory):**
- Block-device I/O send/recv & read-after-write durability (G4, WRITES-5,
  BACKUP-FALLBACK actual reads, REQUIRES-BLOCKDEV receptacle binding).
- `crc32fast` algorithm correctness + CRC byte-domain layout (G8).
- `String`/`char` iteration `encode_utf16`/`from_utf16_lossy` and `.take(36)`
  (G7, NAME-LEN-36) — only the byte-level LE code-unit arithmetic is proved.
- Byte (de)serialization layout of headers/entries (G6/INIT-RETURNS cover the
  LBA/sector/index arithmetic, not the `&[u8]` field offsets).
- `Mutex` non-poisoning / cross-thread ordering (G3, all state ops) → Loom.
- `/dev/urandom` entropy and GUID **uniqueness** (R6) — only the structural v4
  version/variant nibbles are proved (bit-level), not uniqueness.

**Tally:** 2 TOOL ⊘ (not-expressible I/O-count frames) · **0 AGENT ⧗** (both
baseline authorable items closed this re-run) · the remainder of the M=35 set has
its Creusot-expressible core **proved green** (✓ pure, ★ mirror with disclosed
TCB). No property was punted on lane/effort grounds — every one was attempted and
its outcome recorded by id.

---

## Reconciliation-flag observations (§5)

- **R1** — *`CorruptTable` never terminal.* Proved: `errors/read_gpt` discharges
  `result != Err(CorruptTable)`. This makes `initialize_or_format`'s `CorruptTable`
  arm provably dead (`DPM-IOF-CORRUPT-BRANCH-DEAD`). Behaviour matches spec.
- **R3** — *sector-size divergence.* Confirmed at the proof level: the layout
  arithmetic (`entry_sectors`, `part_size_ceil`, usable-LBA math) is proved
  **generically for any `sector_size > 0`**, so the code is genuinely more general
  than the spec's {512,4096}. G1 is a spec-only precondition the code does not
  enforce — a documented latitude, not a bug and not a tool limit.
- **R4** — *latent unguarded underflows.* Creusot is well-suited here and the
  panic-freedom **preconditions were proved as explicit `#[requires]`**:
  - `layout/usable_lbas` requires `num_sectors >= entry_sectors + 2` (the
    `num_sectors - 1 - entry_sectors - 1` subtraction at gpt.rs:148, computed
    *before* the too-small guard);
  - `roundtrip/read_num_sectors` requires `ending_lba >= starting_lba` (the
    `ending_lba - starting_lba + 1` at gpt.rs:129 for a CRC-valid hostile entry);
  - `layout/within_usable_step` requires `last_usable < u64::MAX`.
  Each is the exact implicit assumption R4 flagged; a panic-freedom pass over the
  shipped code should hoist these as guards (backup-LBA read arithmetic at
  gpt.rs:77-79 is the same shape, covered by the `usable_lbas` precondition
  reasoning).
- **R6** — GUID uniqueness is environmental; only structural v4 nibbles proved.

## Did the strengthened skill discipline help here?

**Yes.** "Prove globals once, ordered by attachment count" paid off directly: G1
(attach 4) and the six attach-2 globals were each proved (or R3-dispositioned)
**once** and their status propagates by id into all four bundles — the per-method
scoreboard is almost entirely globals + a few locals, so one `state/*` gate proof
discharged G2/G5 across both query methods, and one `roundtrip/*` proof carried G6
into both `format` and `initialize`. "Attempt every property, label every miss
TOOL vs AGENT" forced the two I/O-count frames and the one Vec-count out into the
open as ⊘/⧗ with concrete routes, instead of being silently dropped as
"better for Kani" — and it surfaced that the crown-jewel G6 and the R4
panic-freedom preconditions are squarely in Creusot's sweet spot.
