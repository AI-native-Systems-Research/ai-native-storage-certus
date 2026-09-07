# disk-partition-manager — verifiable-property inventory (Role 1, tool-independent)

**Repo:** `/home/cornel/ai-native-storage-certus` · **Branch:** `unstable-creusot` · **Commit:** `c87f1e25`
**Built:** 2026-09-07, by reconciling **two BLIND extractions** (spec-only + code-only, isolated from each other).
**Sources reconciled:** `dpm_props_from_spec.md` (37 props) + `dpm_props_from_code.md` (20 observable + 31 internal).
**No prover named. No proof status recorded.** That is Role 2 / Role 3.

---

## Header — the denominator

**N = 4 public methods of the `IPartitionTable` trait** (`components/interfaces/src/ipartition_table.rs`):
`initialize`, `format`, `partition_info`, `num_partitions`.

**Not in N (tracked separately):** `initialize_or_format` and `set_ns_id`/`get_ns_id` are **inherent `lib.rs` methods, not on the trait**. The spec devotes IR-003 + user-story US3 to `initialize_or_format`, so it is inventoried below as an *out-of-trait helper bundle* — reported apart from the N=4 scoreboard, never folded into it.

**M = 35 distinct verifiable properties** across the 4 trait methods (9 global + 26 method-local).
**Attachments (Σ|methods|) = 46.** Bundle sizes: **initialize 12 · format 24 · partition_info 6 · num_partitions 4.**
(+ 4 helper properties on `initialize_or_format`, out-of-N.)

**How M was produced (not an intersection, not a raw sum):** the two blind lists were merged by *obligation identity* (subject × kind × statement), keeping every spec-only/code-only divergence flagged; then the code extraction's **31 implementation-internal obligations were collapsed up** into the observable property each serves (recorded in the implementing-obligation map, §4), **not** counted as ids. So M (35) sits above the spec's 37-minus-duplicates observable count and far below the raw 51 code emissions — discovered by the reconciled walk, not chosen.

---

## §1 Global invariants — the prove-once worklist (sorted by attachments ↓)

Role 2 discharges these **once**, highest-attachment first; status propagates by `id` to every bundle that contains them.

| # | id | attach | methods | kind | statement | origin |
|---|----|:---:|---------|------|-----------|--------|
| G1 | `DPM-SECTOR-SIZE-512-4096` | 4 | initialize, format, partition_info, num_partitions | precondition | Device sector size is 512 or 4096 bytes. | spec-only ⚑ |
| G2 | `DPM-INIT-GATE` | 2 | partition_info, num_partitions | precondition | A query returns `NotInitialized` unless a prior `initialize`/`format` cached a table (`state = Some`). | spec+code |
| G3 | `DPM-STATE-CACHED` | 2 | initialize, format | postcondition | On `Ok(t)`, `state ← Some(t.clone())` and the returned value equals the cached table. | spec+code |
| G4 | `DPM-IO-PROPAGATE` | 2 | initialize, format | error-case | Any block-device send/completion failure surfaces as `IoError`. | code-only |
| G5 | `DPM-COUNT-INDEX-AGREE` | 2 | partition_info, num_partitions | invariant | `partition_info(i)` is `Ok` iff `i < num_partitions()` — one denominator. | spec+code |
| G6 | `DPM-ROUNDTRIP-OFFSETS` | 2 | format, initialize | invariant | **Read-back after format yields identical offsets / sector counts / type-GUIDs / names.** (crown jewel — SC-001) | spec+code |
| G7 | `DPM-ROUNDTRIP-NAME` | 2 | format, initialize | invariant | A name UTF-8 → UTF-16LE → UTF-8 preserves all ASCII chars (≤36 code units). (SC-003) | spec+code |
| G8 | `DPM-CRC-INTEGRITY` | 2 | initialize, format | invariant | CRC32 over the correct domain (header first 92 B with crc field zeroed / entry array); recompute == stored gates read-validity and is filled on write. | spec+code |
| G9 | `DPM-NAMESPACE-ID` | 2 | initialize, format | frame | Block I/O targets the configured NVMe namespace id. | spec+code(helper) ⚑ |

## §2 Method-local properties

### initialize — bundle 12 (7 globals G2*→ no; contains G3,G4,G6,G7,G8,G9,G1 = 7 + 5 local)
*(globals in bundle: G1, G3, G4, G6, G7, G8, G9)*

| id | kind | statement | origin |
|----|------|-----------|--------|
| `DPM-INIT-REQUIRES-BLOCKDEV` | error-case | Unconnected block-device receptacle → `NotInitialized`, no I/O. | code-only |
| `DPM-INIT-BACKUP-FALLBACK` | postcondition | Primary GPT failing specifically with `CorruptTable` → the backup GPT at the last LBA is tried before giving up. | spec+code |
| `DPM-INIT-BOTH-CORRUPT-NOPTBL` | error-case | Neither primary nor backup valid → `NoPartitionTable`. | spec+code |
| `DPM-INIT-RETURNS-CORRECT-LAYOUT` | postcondition | On a valid GPT, the returned table's offsets/sectors/type-GUIDs/names match what is stored. | spec+code |
| `DPM-INIT-IO-2RT` | frame | Happy path performs ≤ 2 logical read round-trips (header, entries). (PR-002) | spec-only ⚑ |

### format — bundle 24
*(globals in bundle: G1, G3, G4-no, G6, G7, G8, G9; i.e. G1,G3,G6,G7,G8,G9 = 6 + IO-PROP G4? format does I/O → yes G4 → 7 globals + 17 local)*

| id | kind | statement | origin |
|----|------|-----------|--------|
| `DPM-FORMAT-REQUIRES-BLOCKDEV` | error-case | Unconnected receptacle → `NotInitialized`, nothing written. | code-only |
| `DPM-FORMAT-DEVICE-TOO-SMALL` | error-case | `first_usable_lba >= last_usable_lba` → `LayoutError` before any write. | code-only |
| `DPM-FORMAT-ERR-MULTI-REST` | error-case | More than one `size_bytes==0` spec → `LayoutError`. (FR-005) | spec+code |
| `DPM-FORMAT-ERR-OVERSUBSCRIBED` | error-case | Fixed partitions exceeding usable space → `LayoutError` (code enforces via two guards: aggregate fixed-fits + per-placement remaining-fits). (FR-006) | spec+code |
| `DPM-FORMAT-WRITES-5-STRUCTURES` | postcondition | Writes, in order: protective MBR @ LBA0 (type 0xEE), primary header @ LBA1, primary entries @ LBA2, backup entries @ last_usable+1, backup header @ last LBA. (FR-001/003/007) | spec+code |
| `DPM-FORMAT-ENTRY-ARRAY-128x128` | invariant | Entry array is exactly 128 slots × 128 B; unused slots all-zero. (FR-001) | spec+code |
| `DPM-FORMAT-HEADER-CONSTANTS` | postcondition | Both headers carry signature "EFI PART", revision 0x00010000, header_size 92, num_entries 128, entry_size 128. | code (corroborates FR-001) |
| `DPM-FORMAT-DISK-GUID-V4` | postcondition | Disk GUID has RFC-4122 v4 version nibble + variant bits. (FR-008) | spec+code |
| `DPM-FORMAT-PART-GUID-V4` | postcondition | Each partition unique GUID is RFC-4122 v4. (FR-008) | spec+code |
| `DPM-FORMAT-NAME-LEN-36` | precondition | Configured name ≤ 36 UTF-16 code units. (FR-009) | spec+code |
| `DPM-FORMAT-TYPEGUID-PRESERVED` | postcondition | Written entry type-GUID equals the input PartitionSpec's type-GUID. (SC-001) | spec+code |
| `DPM-FORMAT-PART-SIZE-CEIL` | postcondition | Fixed partition sector count = `ceil(size_bytes / sector_size)`. (FR-004) | spec+code |
| `DPM-FORMAT-RESTOFDISK-REMAINING` | postcondition | The single `size_bytes==0` partition consumes all remaining usable sectors. (FR-004) | spec+code |
| `DPM-FORMAT-PART-NONOVERLAP` | invariant | Placed partitions are contiguous & disjoint: each `start == prev.end + 1`, starts strictly increasing. (US1-AS1) | spec+code |
| `DPM-FORMAT-PART-WITHIN-USABLE` | invariant | Every partition lies in `[first_usable_lba, last_usable_lba]`. (US1-AS1/FR-006) | spec+code |
| `DPM-FORMAT-BACKUP-MIRRORS-PRIMARY` | invariant | Backup header's `alternate_lba`/`my_lba` mirror the primary's; both describe the same partition set. (FR-003) | spec+code |
| `DPM-FORMAT-IO-O1` | frame | Write count is O(1) in device size (only header/entry/MBR sectors). (PR-001) | spec-only ⚑ |

### partition_info — bundle 6
*(globals in bundle: G1, G2, G5)*

| id | kind | statement | origin |
|----|------|-----------|--------|
| `DPM-PINFO-INDEX-RANGE` | error-case | `index >= partitions.len()` → `InvalidPartition`; valid indices exactly `0..num_partitions()`. | **code-only — fills a spec gap** ⚑ |
| `DPM-PINFO-RETURNS-ENTRY` | postcondition | In-range index → a clone of the cached `partitions[index]`. (IR-001/SC-001) | spec+code |
| `DPM-PINFO-READONLY` | frame | No mutation of `state`, no block I/O. | code-only |

### num_partitions — bundle 4
*(globals in bundle: G1, G2, G5)*

| id | kind | statement | origin |
|----|------|-----------|--------|
| `DPM-NUMP-RETURNS-LEN` | postcondition | Returns `partitions.len()` of the cached table as `u32`. (IR-001) | spec+code |

## §3 Out-of-trait helper — `initialize_or_format` (tracked, NOT in N)

| id | kind | statement | origin |
|----|------|-----------|--------|
| `DPM-IOF-VALID-NOFORMAT` | postcondition | `force_format=false` + valid GPT → returns existing table, `formatted=false`, no write. (US3-AS1/IR-003/SC-004) | spec+code |
| `DPM-IOF-NOVALID-FORMATS` | postcondition | `force_format=false` + no valid GPT → writes GPT, `formatted=true`. (US3-AS2) | spec+code |
| `DPM-IOF-FORCE-FORMATS` | postcondition | `force_format=true` → fresh GPT, `formatted=true` regardless of on-disk state. (US3-AS3) | spec+code |
| `DPM-IOF-CORRUPT-BRANCH-DEAD` | invariant | The `initialize_or_format` fallback branch on `CorruptTable` is **unreachable**: `read_gpt` remaps every terminal failure to `NoPartitionTable` (see reconciliation flag R1). | code-only ⚑ |

---

## §4 Implementing-obligation map (code internals → observable they serve)

The code extraction's 31 internal obligations are **not** ids; each collapses up into an observable property. This map preserves the evidence trail without inflating M.

| observable property | implementing internal obligations (gpt.rs) |
|---|---|
| `DPM-CRC-INTEGRITY` (G8) | HDR-CRC-DOMAIN, HDR-CRC-VALIDATE, ENTRY-CRC-VALIDATE, ENTRY-CRC-WRITE, CRC-WRITE-READ-ROUNDTRIP |
| `DPM-ROUNDTRIP-OFFSETS` (G6) | HDR-ROUNDTRIP, ENTRY-ROUNDTRIP, ENDING-LBA, PINFO-NUMSECTORS, FILTER-NONZERO-TYPE, PINFO-INDEX-IS-SLOT |
| `DPM-ROUNDTRIP-NAME` (G7) | UTF16-ROUNDTRIP |
| `DPM-FORMAT-WRITES-5-STRUCTURES` | FIRST-USABLE, LAST-USABLE, WRITE-HEADER-CONSTANTS, PRIMARY-BACKUP-LBA-MIRROR, MBR-TYPE-EE, MBR-BOOTSIG, MBR-START-AND-SIZE, WRITE-BLOCK-COUNT |
| `DPM-FORMAT-PART-SIZE-CEIL` / `-RESTOFDISK` | PART-SIZE-CEIL, ENTRY-SECTORS |
| `DPM-FORMAT-PART-NONOVERLAP` / `-WITHIN-USABLE` | CONTIGUOUS-NONOVERLAP, WITHIN-USABLE, ENTRY-ARRAY-128 |
| `DPM-INIT-BACKUP-FALLBACK` | READ-PLACEMENT, PARSE-HDR-TOO-SHORT, ENTRY-SECTORS |
| `DPM-INIT-BOTH-CORRUPT-NOPTBL` | PARSE-SIGNATURE (sole `NoPartitionTable` origin) |
| `DPM-INIT-RETURNS-CORRECT-LAYOUT` | READ-BLOCK-COUNT, PARSE-ENTRY-BOUNDS, HDR-ROUNDTRIP, ENTRY-ROUNDTRIP |
| `DPM-FORMAT-*-GUID-V4` | GUID-VERSION4, GUID-VARIANT |

---

## §5 Reconciliation flags (surfaced, never smoothed)

- **R1 — `CorruptTable` is never a terminal error (code-only finding).** `read_gpt` falls a primary `CorruptTable` through to the backup path, which remaps *all* errors to `NoPartitionTable`. So `initialize` surfaces only `NotInitialized | IoError | NoPartitionTable`. Consequence: `initialize_or_format`'s `CorruptTable` fallback branch is dead code (→ `DPM-IOF-CORRUPT-BRANCH-DEAD`). The spec (US2-AS3) only ever promised `NoPartitionTable` for "both corrupt," so **behaviour matches spec** — but the spec's separate notion of surfacing corruption is not realized. Worth a code note.
- **R2 — spec gap filled by code.** Spec pins no out-of-range obligation for `partition_info`; the code returns `InvalidPartition` (`DPM-PINFO-INDEX-RANGE`). Kept as code-only, flagged.
- **R3 — sector-size divergence.** Spec (FR-011) restricts to {512, 4096}; the code is sector-size-generic (`entry_sectors`, layout arithmetic work for any size). `DPM-SECTOR-SIZE-512-4096` is therefore a **spec-only precondition the code does not enforce** — the code is more general than the spec. Not a bug; a documented latitude.
- **R4 — three latent unguarded-underflow preconditions (code-only assumptions, not properties).** `last_usable_lba` computed before the too-small guard (gpt.rs:148); backup-LBA arithmetic on read (gpt.rs:77-79); `ending_lba - starting_lba + 1` on the read path for a CRC-valid-but-hostile entry (gpt.rs:129). Implicit preconditions `num_sectors >= entry_sectors + 2` and `ending_lba >= starting_lba`. These are real proof obligations for a panic-freedom pass — recorded so a prover can target them, not asserted as satisfied.
- **R5 — backfilled spec.** `spec.md` is generated from the implementation ("documents current behavior, not original intent"). So spec↔code agreement is partly circular; the corroboration signal is weaker here than for a spec authored before code. Stated for honesty.
- **R6 — GUID uniqueness not established.** `/dev/urandom` fallback to zeros (gpt.rs) can violate FR-008; only structural v4 version/variant nibbles are checkable. Uniqueness is an environmental assumption.

## §6 Coverage ledgers (nothing unaccounted)

- **Method ledger:** all 4 trait methods have non-empty bundles (12/24/6/4). No empty bundle.
- **Spec ledger:** all 11 FR, 3 IR, 2 PR, 3 user stories / 9 acceptance scenarios, 4 SC mapped to ≥1 property or listed Not-verifiable (see `dpm_props_from_spec.md` §ledger; IR-002 IBlockDevice binding = external, not a property).
- **Code ledger:** every public fn, error-return branch, and state transition mapped to a property or a reason (see `dpm_props_from_code.md` §3).

## §7 Not-verifiable / trusted boundary (the TCB for this component)

IBlockDevice sector durability & read-after-write · `crc32fast` algorithm correctness · `/dev/urandom` entropy · Mutex non-poisoning / cross-thread ordering (→ Loom) · Component-framework receptacle binding · physical hardware. Every format/initialize postcondition is conditional on faithful block I/O.

---

*Tool-independent. `lane` and `status` are added downstream by `tools-verify-{creusot,kani}-with-properties` (Role 2) and rendered by `tools-aggregate-coverage-by-interface` (Role 3).*
