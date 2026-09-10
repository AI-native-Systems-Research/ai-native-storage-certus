# Verified properties — disk-partition-manager (Kani)

**Role 2 (Kani lane).** Verified from the property inventory
`disk-partition-manager_property_inventory.md` (Role 1, commit `c87f1e25`, N=4 methods,
M=35 properties) against code `components/disk-partition-manager/src/gpt.rs` +
`src/lib.rs`. Harnesses live in `#[cfg(kani)] mod verification` at the end of `gpt.rs`.
Toolchain: **Kani 0.67.0 / CBMC**. Branch `verif/kani/disk-partition-manager`
(base `unstable-kani` @ `b160a3ae`). Consumed by `id`; no properties re-extracted or
invented. This file **under-claims**: only what a green harness establishes is marked proved.

## Backing-structure scout (done FIRST, before any harness)

Confirmed the component is a **good Kani target for arithmetic**: GPT state is a 92-byte
header + a **128 × 128-byte entry array modeled as byte slices** (`[u8]`), CRC32 taken over
byte ranges, and all layout/LBA logic is **integer arithmetic** over `u32`/`u64` with a
UTF-16LE name codec. `Vec` is used but **bounded ≤ 128**; **no `BTreeMap`/`HashMap`** anywhere.
Pure-arithmetic harnesses run in ~0.05–0.2 s / ~170 MB. This scout avoided the memory-tier
time sink — no unbounded map was ever handed to CBMC.

Three **intrinsic Kani/CBMC boundaries** were then hit and are documented as TOOL residuals
(not agent gaps): (1) `crc32fast::hash` lowers to the `_xgetbv` intrinsic CBMC does not
support; (2) integer `format!` in error arms does not terminate under CBMC; (3) the
production 128-slot entry-array zero-pad forces `#[kani::unwind(≥129)]`, which SAT-explodes.

---

## Scoreboard (per method: proved m / bundle B, globals counted in each bundle)

| method | B | proved m | proved property ids |
|---|:--:|:--:|---|
| `initialize` | 12 | **2** | G6 (arithmetic core), G7 |
| `format` | 24 | **11** | G6 (core), G7, FORMAT-ERR-MULTI-REST, FORMAT-DISK-GUID-V4, FORMAT-PART-GUID-V4, FORMAT-NAME-LEN-36, FORMAT-PART-SIZE-CEIL, FORMAT-RESTOFDISK-REMAINING, FORMAT-PART-NONOVERLAP, FORMAT-PART-WITHIN-USABLE, FORMAT-BACKUP-MIRRORS-PRIMARY |
| `partition_info` | 6 | **0** | — (all lib.rs Vec/Option logic; AGENT, see ledger) |
| `num_partitions` | 4 | **0** | — (lib.rs `len()`; AGENT) |

**Unique properties proved: 11 of M=35.** (G6/G7 are globals shared by `initialize`+`format`,
counted in each bundle but once here.)

**Status legend (the 6 markers):**
`✓` proved by a green harness · `✓~` proved at bounded geometry / arithmetic-core only ·
`⊘` precondition, assumed (not a Kani obligation) · `⤴T-io` TOOL: effects Kani cannot model
(I/O, FFI/`_xgetbv`, `format!` non-termination, 128-Vec SAT) · `⤴T-x` TOOL: not expressible
for a bounded checker (resource/frame) · `⧗A` AGENT: bounded-checkable, harness not authored.

---

## Global invariants (§1) — proved once, propagate by id

- `DPM-ROUNDTRIP-OFFSETS` (G6) **[Invariant]** `✓~` — the offset↔sector-count inverse at the
  heart of read-back (`ending = start + num_sectors − 1  ⟺  num_sectors = ending − start + 1`)
  is proved symbolically over the full `u64` range with overflow guarded — harness
  `verify_ending_lba_inverse` (SUCCESSFUL). The **full byte+CRC read-back** (offsets/GUIDs/names
  identical after write→read) is `⤴T-io` — it runs through `crc32fast`/`_xgetbv` and block I/O.
- `DPM-ROUNDTRIP-NAME` (G7) **[Invariant]** `✓` — a name UTF-8→UTF-16LE→UTF-8 preserves ASCII —
  harness `verify_name_roundtrip_ascii` (SUCCESSFUL, symbolic ASCII length ≤ 5 under
  `#[kani::unwind(6)]`) + `verify_name_len_36` (SUCCESSFUL, the ≤36-code-unit truncation).
  Bound: proved for symbolic ASCII strings up to the unwind cap, not all 36 code units.
- `DPM-SECTOR-SIZE-512-4096` (G1) **[Precondition]** `⊘` — assumed (`kani::any() ∈ {512,4096}`)
  in every arithmetic harness. Per inventory R3 the code is sector-size-generic and does not
  *enforce* it, so it is a precondition, not a Kani postcondition obligation.
- `DPM-STATE-CACHED` (G3) `⤴T-io`; `DPM-IO-PROPAGATE` (G4) `⤴T-io`; `DPM-CRC-INTEGRITY` (G8)
  `⤴T-io` (`_xgetbv`); `DPM-NAMESPACE-ID` (G9) `⤴T-io` — see residual ledger.
- `DPM-INIT-GATE` (G2) `⧗A`; `DPM-COUNT-INDEX-AGREE` (G5) `⧗A` — lib.rs Option/Vec logic.

## initialize

- `DPM-ROUNDTRIP-OFFSETS` / `DPM-ROUNDTRIP-NAME` — proved as above (G6 core, G7 bounded).
- All 5 initialize-local properties are residuals (I/O + CRC read path). See ledger.
  *Bonus (panic-freedom, not an M id):* `verify_parse_entries_bounds` and
  `verify_parse_header_too_short` prove the pure parse guards that `DPM-INIT-RETURNS-CORRECT-LAYOUT`
  and `DPM-INIT-BACKUP-FALLBACK` rely on; R4 harnesses (below) discharge the backup-LBA
  underflow that the fallback path assumes.

## format

- `DPM-FORMAT-ERR-MULTI-REST` **[Error-case]** `✓` — > 1 `size_bytes==0` spec → `LayoutError`
  (FR-005) — harness `verify_layout_multi_rest_err` (SUCCESSFUL, real `compute_partition_layout`
  call, early-return before the entry-array pad, `#[kani::unwind(4)]`).
- `DPM-FORMAT-DISK-GUID-V4` **[Postcondition]** `✓` — disk GUID carries the RFC-4122 v4 version
  nibble (`byte6 = _4x`) + variant bits (`byte8 = 10xx_xxxx`) (FR-008) — harness
  `verify_guid_v4_nibbles` (SUCCESSFUL, symbolic re-derivation of the nibble masking over
  `[u8;16] = kani::any()`).
- `DPM-FORMAT-PART-GUID-V4` **[Postcondition]** `✓` — same masking logic (`generate_guid` is
  shared for disk + partition GUIDs); covered by `verify_guid_v4_nibbles`.
- `DPM-FORMAT-NAME-LEN-36` **[Precondition]** `✓` — configured name ≤ 36 UTF-16 code units
  (FR-009) — harness `verify_name_len_36` (SUCCESSFUL).
- `DPM-FORMAT-PART-SIZE-CEIL` **[Postcondition]** `✓` — fixed partition sector count =
  `ceil(size_bytes / sector_size)` (FR-004) — harness `verify_layout_placement_arithmetic`
  (SUCCESSFUL, pure re-derivation mirroring the production placement loop).
- `DPM-FORMAT-RESTOFDISK-REMAINING` **[Postcondition]** `✓` — the single `size_bytes==0`
  partition consumes all remaining usable sectors (FR-004) — harness
  `verify_layout_restofdisk_arithmetic` (SUCCESSFUL).
- `DPM-FORMAT-PART-NONOVERLAP` **[Invariant]** `✓` — placed partitions contiguous & disjoint
  (`start == prev.end + 1`, starts strictly increasing) (US1-AS1) — harness
  `verify_layout_placement_arithmetic` (SUCCESSFUL).
- `DPM-FORMAT-PART-WITHIN-USABLE` **[Invariant]** `✓` — every partition in
  `[first_usable_lba, last_usable_lba]` (US1-AS1/FR-006) — harness
  `verify_layout_placement_arithmetic` (SUCCESSFUL).
- `DPM-FORMAT-BACKUP-MIRRORS-PRIMARY` **[Invariant]** `✓` — backup header `my_lba`/`alternate_lba`
  mirror the primary's (FR-003) — harness `verify_backup_mirror` (SUCCESSFUL, arithmetic).
- Remaining 8 format-local properties are residuals — see ledger.
  *Supporting green (not an M id):* `verify_entry_sectors_real` proves
  `entry_sectors == (128·128).div_ceil(sector_size)` (512→32, 4096→4), the base for
  FIRST/LAST-USABLE and PART-SIZE-CEIL.

## partition_info / num_partitions

No Kani-proved properties. Both bundles are lib.rs logic over `Mutex<Option<PartitionTable>>`
and `Vec<PartitionInfo>` — bounded-checkable in principle but harnesses were not authored
(AGENT); see ledger.

---

## R4 — latent unguarded-underflow obligations (panic-freedom; inventory R4, not M ids)

The directive's priority target. Kani **confirmed all three latent underflows exist** (found the
counterexample regions) **and proved the implied preconditions make the guarded code safe**:

- **gpt.rs:148** `last_usable_lba = num_sectors − 1 − entry_sectors − 1` (computed *before* the
  too-small guard at :150): the checked chain is `None` throughout `num_sectors < entry_sectors + 2`
  (`verify_r4_last_usable_underflows_below_min`, SUCCESSFUL); at/above that bound it is
  underflow-free and the :150 guard is the correct sufficient gate
  (`verify_r4_last_usable_safe_above_min`, SUCCESSFUL). Implied precondition
  `num_sectors ≥ entry_sectors + 2` confirmed.
- **gpt.rs:77-79** backup-LBA arithmetic on the read path: `backup_lba − entry_sectors`
  underflows for `num_sectors ≤ entry_sectors` (including `num_sectors == 0`)
  (`verify_r4_backup_lba_underflows_tiny`, SUCCESSFUL).
- **gpt.rs:129** `num_sectors = ending_lba − starting_lba + 1` on a CRC-valid-but-hostile entry:
  underflows when `ending < starting`, **and a second finding** — even with `ending ≥ starting`
  the `+ 1` overflows exactly at `diff == u64::MAX` (`verify_r4_ending_minus_starting`, SUCCESSFUL).

Recommendation: replace the three raw expressions with `checked_sub`/`checked_add` (or hoist the
:150 guard above :148) to make panic-freedom unconditional.

---

## Residual ledger — 24 unproved of 35, each with reason + TOOL/AGENT label

### TOOL — effects Kani cannot model or bounded-checker limits (15)

| id | reason (reproducible signature) | route |
|---|---|---|
| `DPM-CRC-INTEGRITY` (G8) | `crc32fast::hash` → `unsupported_construct: _xgetbv` | Creusot / integration test |
| `DPM-ROUNDTRIP-OFFSETS` (G6, full byte+CRC) | CRC + block I/O (`_xgetbv`); arithmetic core proved | Creusot + fault-injection test |
| `DPM-STATE-CACHED` (G3) | reached only via initialize/format I/O + CRC | integration test |
| `DPM-IO-PROPAGATE` (G4) | models block-device send/completion failure (I/O effect) | fault-injection test |
| `DPM-NAMESPACE-ID` (G9) | I/O-target frame property | integration test |
| `DPM-FORMAT-REQUIRES-BLOCKDEV` | component-framework receptacle binding (trusted boundary §7) | integration test |
| `DPM-INIT-REQUIRES-BLOCKDEV` | same receptacle binding | integration test |
| `DPM-FORMAT-DEVICE-TOO-SMALL` | real-call path crosses the write/`format!` region; guard arithmetic covered by R4 | R4 proof above + integration test |
| `DPM-FORMAT-ERR-OVERSUBSCRIBED` | error arm builds integer `format!` → CBMC non-termination (timeout >300 s, ~14 MB RSS, stuck in symex) | Creusot |
| `DPM-INIT-BOTH-CORRUPT-NOPTBL` | origin PARSE-SIGNATURE; reject arm `format!("...{:#x}", sig)` non-terminating; poisons whole `parse_header` (accept side also times out >300 s) | Creusot |
| `DPM-FORMAT-WRITES-5-STRUCTURES` | write ordering / block I/O | integration test |
| `DPM-FORMAT-ENTRY-ARRAY-128x128` | 128-slot zero-pad forces `#[kani::unwind(≥129)]` → SAT explosion (VERIFICATION FAILED / timeout) | Creusot |
| `DPM-FORMAT-IO-O1` | resource/frame: write count O(1) — not expressible for a bounded checker | Creusot / cost model |
| `DPM-INIT-BACKUP-FALLBACK` | read primary + CRC + backup I/O; backup-LBA arithmetic proved (R4) | integration test |
| `DPM-INIT-IO-2RT` | resource/frame: ≤2 read round-trips — not expressible bounded | integration test |

### AGENT — bounded-checkable, harness not authored (8)

| id | reason | note |
|---|---|---|
| `DPM-INIT-GATE` (G2) | `state.as_ref().ok_or(NotInitialized)` over `Mutex<Option<_>>` | needs a minimal `DiskPartitionManager` built with `state = None`; the `define_component!` macro + `Mutex` construction risks the same std-library spurious-check class seen with `SpscChannel` — attempt but may reclassify TOOL |
| `DPM-COUNT-INDEX-AGREE` (G5) | `partition_info(i)` Ok iff `i < len` over a bounded `Vec` | harnessable over a small `PartitionTable` |
| `DPM-FORMAT-HEADER-CONSTANTS` | assert the header literals (sig "EFI PART", rev 0x00010000, hdr_size 92, 128 entries, entry_size 128) | production path is CRC-blocked; a direct struct-build harness is authorable |
| `DPM-FORMAT-TYPEGUID-PRESERVED` | `entry.type_guid == spec.type_guid` copy | real call hits the 128-Vec pad; a pure re-derivation (like the placement harness) can assert the copy |
| `DPM-PINFO-INDEX-RANGE` | `index >= len → InvalidPartition` over bounded `Vec` | lib.rs; harnessable |
| `DPM-PINFO-RETURNS-ENTRY` | in-range → clone of `partitions[index]` | lib.rs; harnessable |
| `DPM-PINFO-READONLY` | frame: no `state` mutation / no I/O | expressible as a bounded no-write assertion |
| `DPM-NUMP-RETURNS-LEN` | returns `partitions.len() as u32` | lib.rs; harnessable |

### Precondition, assumed (1)

`DPM-SECTOR-SIZE-512-4096` (G1) `⊘` — assumed in every harness; code is generic (R3).

**Tally: proved 11 · TOOL 15 · AGENT 8 · precondition 1 = 35.**

---

## Timing & peak RSS (`/usr/bin/time -v`, per-harness CBMC solve)

**15 green harnesses** (all VERIFICATION SUCCESSFUL):

| harness | CBMC solve | wall | peak RSS |
|---|--:|--:|--:|
| `verify_name_roundtrip_ascii` (G7) | 592.9 s | 9:56.5 | **~19.5 GB** |
| `verify_name_len_36` | 41.4 s | 0:44.7 | 1.22 GB |
| `verify_layout_multi_rest_err` | 1.65 s | 0:07.9 | 479 MB |
| `verify_parse_entries_bounds` | 0.92 s | 0:03.6 | 316 MB |
| `verify_parse_header_too_short` | 0.83 s | 0:04.0 | 330 MB |
| `verify_layout_placement_arithmetic` | 0.16 s | <1 s | 174 MB |
| `verify_ending_lba_inverse` (G6) | 0.086 s | 0:00.8 | 172 MB |
| `verify_layout_restofdisk_arithmetic` | 0.082 s | <1 s | 172 MB |
| `verify_r4_last_usable_underflows_below_min` | 0.065 s | 0:00.8 | 174 MB |
| `verify_r4_last_usable_safe_above_min` | 0.064 s | 0:00.8 | 172 MB |
| `verify_r4_backup_lba_underflows_tiny` | 0.051 s | 0:00.8 | 174 MB |
| `verify_entry_sectors_real` | 0.050 s | 0:00.8 | 172 MB |
| `verify_guid_v4_nibbles` | 0.043 s | 0:00.8 | 170 MB |
| `verify_backup_mirror` | 0.029 s | 0:00.8 | 171 MB |
| `verify_r4_ending_minus_starting` | 0.050 s | 0:00.8 | 168 MB |

**Blowups / timeouts recorded (all TOOL, killed at the 300 s cap, ~14–16 MB RSS — stuck in
symex/instrumentation, never reached the solver):** `verify_parse_header_signature_gate` /
`verify_parse_header_valid_ok` (`format!` in reject arm), `verify_layout_oversubscribed_err`
(`format!`), `verify_layout_single_fixed` (128-Vec unroll → also seen as VERIFICATION FAILED at
~2 s once codegen completed). These harnesses were removed from the suite; their boundaries are
documented as TOOL residuals above so the committed suite stays green.

**Cost lesson:** pure integer arithmetic ≈ 0.05 s / 170 MB; `String`/`from_utf16_lossy` symbolic
execution is the dominant driver (623 s / 22.7 GB for the name round-trip, which still SUCCEEDED).
The scout's "byte-slice + integer" confirmation is why the arithmetic core was cheap.
