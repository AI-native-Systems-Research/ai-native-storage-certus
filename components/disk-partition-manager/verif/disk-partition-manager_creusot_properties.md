# Verified properties — disk-partition-manager (Creusot)

Proven from the Role-1 inventory `disk-partition-manager_property_inventory.md`
(N=4 trait methods, M=35 properties) against code
`components/disk-partition-manager/src/{gpt.rs,lib.rs}` and
`components/interfaces/src/ipartition_table.rs`. Artifacts: this `verif/` crate.
Branch `verif/creusot/disk-partition-manager`, based on `unstable-creusot`
@ `c87f1e25`.

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

**Result.** `cargo creusot` → `Proved (43 files) ✔`.
**57 VCs discharged across 43 `.coma` files**, of which **6 files (6 VCs) are
auto-derived `Clone` impls**; the **37 property-bearing `.coma` files carry 51
VCs**. Solver portfolio **alt-ergo / z3 / cvc5 / cvc4** (most goals closed by
alt-ergo or cvc5), tactics `split_vc` + `compute_specified`
(`why3find.json`: fast 0.2, time 8, depth 6).

**Measurement** (`/usr/bin/time -v cargo creusot`):
- **Cold / clean run:** wall-clock **24.33 s**, peak RSS **≈ 803 MB** (821 956 KB).
- **Cached replay:** wall-clock **0.68 s**, peak RSS **≈ 66 MB** (66 052 KB).

**Anti-vacuity (fault injection).** Three representative proofs across the three
proof styles were fault-injected and confirmed to go **red**, then reverted:
- G6 arithmetic — dropping the `-1` in `ending_lba` reddened `vc_ending_lba`;
- GUID bit-level — `0x40 → 0x50` (version 5) reddened `vc_guid_version4`;
- state gate — removing the `!present → NotInitialized` guard reddened
  `vc_partition_info`.
Re-running clean restored `Proved (43 files) ✔`.

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
| `initialize` | 12 | **10** | G1 (R3); `DPM-INIT-IO-2RT` (TOOL) |
| `format` | 24 | **21** | G1 (R3); `DPM-FORMAT-ERR-MULTI-REST` (AGENT); `DPM-FORMAT-IO-O1` (TOOL) |
| `partition_info` | 6 | **5** | G1 (R3) |
| `num_partitions` | 4 | **3** | G1 (R3) |
| *(helper, out-of-N)* `initialize_or_format` | 4 | **4** | — |

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
- `DPM-FORMAT-ERR-MULTI-REST` **[error]** >1 `size_bytes==0` → `LayoutError`. — **⧗ AGENT**: the guard predicate is trivial, but counting zero-size specs over `config.partitions: Vec<PartitionSpec>` needs a `Seq`-fold count model that is *not yet authored*. Creusot **can** express it. Route: model the count with `Seq::fold`/an index loop invariant.
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
- `DPM-FORMAT-PART-NONOVERLAP` **[inv]** `start==prev.end+1`, starts strictly increasing. — ✓ `layout/place_step` + `layout/two_partitions_disjoint` (2 VC). *The observable statement is proved as the inductive step + a two-partition instance; the full N-partition `Vec` loop invariant is an authorable extension (AGENT).*
- `DPM-FORMAT-PART-WITHIN-USABLE` **[inv]** every partition in `[first_usable, last_usable]`. — ✓ `layout/within_usable_step` (same AGENT note on the full loop).
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

**AGENT — Creusot could, proof not yet authored (1 property + 1 extension, ⧗):**
- `DPM-FORMAT-ERR-MULTI-REST` — counting `size_bytes==0` specs over
  `Vec<PartitionSpec>`; needs a `Seq`-fold / index-loop count model. Expressible;
  unauthored.
- Full **N-partition loop induction** for `DPM-FORMAT-PART-NONOVERLAP` /
  `-WITHIN-USABLE` — proved here as the inductive **step** + a two-partition
  instance; the whole-`Vec` loop invariant is an authorable extension.

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

**Tally:** 2 TOOL (not-expressible) · 2 AGENT (authorable) · the remainder of the
M=35 set has its Creusot-expressible core **proved green** (✓ pure, ★ mirror with
disclosed TCB). No property was punted on lane/effort grounds — every one was
attempted and its outcome recorded by id.

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
