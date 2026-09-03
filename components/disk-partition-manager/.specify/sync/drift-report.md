---
spec_sync_component: disk-partition-manager
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T18:07:26Z
spec_sync_git_commit: 2c7864a2
spec_sync_inputs_sha256: 0404aab9898ca897385ea0121820c841d3255396863ec73bc3067e80d29eaff9
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Drift Report: disk-partition-manager

**Generated**: 2026-09-03
**Component**: `components/disk-partition-manager`
**Scope**: `.specify/specs/001-gpt-partition-management/spec.md` (+ plan/tasks
skim) vs implementation in `src/gpt.rs`, `src/lib.rs`, and the interface
`components/interfaces/src/ipartition_table.rs`. Cross-component integration
(SC-004) verified against `components/dispatcher/src/lib.rs` and
`components/dispatcher-p2p/src/lib.rs`. No `src/`, `spec.md`, or interface source
was changed this sweep — the specification was already accurate and every anchor
independently re-verified.

> **Correction of the prior artifact.** The previous report was dated
> **2026-08-07T15:30:30Z**, carried **no** freshness stamp, and classified
> **FR-003** and **PR-002** as *Drifted (Medium)*:
> - FR-003 described the pre-fix `read_gpt`, where the backup header was tried
>   only on `CorruptTable` (CRC mismatch) and a damaged/zeroed primary
>   *signature* propagated `NoPartitionTable` without a backup attempt.
> - PR-002 flagged the per-sector read loop as exceeding the "at most 2 I/O
>   round-trips" the requirement then stated.
>
> Both were **resolved by the 2026-08-20 Phase B sync** and are stale in that
> report: FR-003's fix is now present in the code (`read_gpt` falls through to
> the backup on **both** `CorruptTable` **and** `NoPartitionTable`), and PR-002
> was backfilled to describe the actual per-sector behavior. In addition, every
> `file:line` anchor in the prior report had drifted ~10 lines as the code moved
> (e.g. `read_gpt` `66-86`→`66-96`; `generate_guid` `554-564`→`564-574`;
> `read_bytes` `441-485`→`451-495`; `parse_header` NoPartitionTable
> `353-357`→`364`; dispatcher `1517,1670`→`1611`). This sweep re-verifies the
> current spec against the current code, finds it fully aligned, and refreshes
> all anchors.

> **Note on the CI input hash.** `scripts/spec-sync-hash.sh
> components/disk-partition-manager` hashes `<dir>/src/**` + `<dir>/specs/**` +
> the `components/interfaces` tree. This component keeps its spec under
> **`.specify/specs/001-gpt-partition-management/`**, not `<dir>/specs/`, so the
> committed digest covers `src/gpt.rs`, `src/lib.rs`, and the interface tree, but
> **not** `spec.md` itself. The digest still changes on any code or interface
> edit; the spec-only backfills recorded here do not move it. Spec↔code
> alignment below was verified by hand regardless.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked (FR + IR + PR + SC + Key Entities) | 11 FR + 3 IR + 2 PR + 4 SC + 4 entities |
| Aligned (behavior) | all |
| Drifted this sweep | 0 (FR-003, PR-002 were resolved in the 2026-08-20 Phase B sync; re-confirmed aligned) |
| Not Implemented | 0 |
| Parked / tracked (not spec↔impl behavioral drift) | 3 (no automated tests; hardcoded LBA-2 read; `generate_guid` zero-fallback) |

---

## Per-Requirement Verification

All anchors below re-checked against the current `src/gpt.rs` / `src/lib.rs` /
`ipartition_table.rs`.

| ID | Status | Location (current) | Notes |
|----|--------|--------------------|-------|
| FR-001 | Aligned | `gpt.rs:9,11-12`; `write_gpt:152-237` (`partition_entry_lba: 2` `:185`); `write_protective_mbr:329` | Rev 1.0 (`0x0001_0000`), 128×128 B entries, primary@LBA1, backup@last LBA, protective MBR@LBA0. |
| FR-002 | Aligned | `try_read_gpt_at:98-150` (header CRC `:111`, entry CRC `:125`); `serialize_header_with_crc:407` | CRC32 (crc32fast = IEEE) over both header and entry array, on read and write. |
| FR-003 | Aligned | `read_gpt:66-96` (fallback arm `:79-80`, backup read `:87-95`); `parse_header:357` (signature→`NoPartitionTable` `:364`) | **Resolved (2026-08-20 backfill).** Backup fallback fires on **both** `CorruptTable` (CRC) and `NoPartitionTable` (signature). A genuinely blank disk still returns `NoPartitionTable` after both reads fail, preserving `initialize_or_format`'s reformat path for truly-unformatted disks. |
| FR-004 | Aligned | `compute_partition_layout:239-322` (`size_bytes == 0` `:254,:280`) | `size_bytes=0` → rest-of-disk consumes remaining usable LBAs. |
| FR-005 | Aligned | `gpt.rs:256-257` | `rest_count > 1` → `LayoutError`. |
| FR-006 | Aligned | `gpt.rs:245` (`total_usable`), `:271` (`LayoutError`) | Fixed partitions exceeding usable space rejected. |
| FR-007 | Aligned | `write_protective_mbr:329`, `0xEE` at `:339` | Protective MBR type 0xEE at LBA 0. |
| FR-008 | Aligned | `generate_guid:564-574` (version `:571`, variant `:572`) | RFC 4122 v4 GUIDs. See parked note on `/dev/urandom` zero-fallback. |
| FR-009 | Aligned | `encode_utf16le_name:576`, `decode_utf16le_name:589` | UTF-16LE encode/decode capped at 36 chars. |
| FR-010 | Aligned | `lib.rs:30-36` (`set_ns_id`/`get_ns_id`); `format` uses `config.ns_id` | Configurable NVMe namespace ID; default 1. |
| FR-011 | Aligned | `read_bytes:451`, `write_bytes:501` (sector-size parameterized) | 512 B and 4096 B both supported; no explicit `∈{512,4096}` validation (any size accepted — documented in spec Implementation Notes). |
| IR-001 | Aligned | `ipartition_table.rs:118-127`; `lib.rs:68-119` | `initialize()` `:118`, `format()` `:121`, `partition_info()` `:124`, `num_partitions()` `:127`. |
| IR-002 | Aligned | `lib.rs:13-25` (define_component! receptacle) | `block_device: IBlockDevice` receptacle. |
| IR-003 | Aligned | `lib.rs:45-64` | `initialize_or_format()` returns `(PartitionTable, bool)` with the `formatted` flag; reformat arm on `NoPartitionTable`\|`CorruptTable` `:57-58`. |
| PR-001 | Aligned | `write_gpt:152-237` | Format writes a fixed sector set (MBR + 2 headers + 2× entry arrays), independent of device size → O(1) vs device size. |
| PR-002 | Aligned | `read_bytes:451-495` (per-sector `ReadSync`) | **Resolved (2026-08-07 backfill).** Spec now describes the actual per-sector read (33 round-trips @512 B / 5 @4096 B), O(1) in device size. Batching into one multi-sector read is a deliberate future perf task, not drift. |
| SC-001 | Aligned (no test) | `write_gpt:152-237` ↔ `try_read_gpt_at:98-150` | Write/read share identical LBA↔sector math; round-trip offsets identical. No automated test — see parked #1. |
| SC-002 | Aligned (no test) | `read_gpt:66-96` | CRC-corrupt **and** signature-damaged primary both fall back to backup transparently. No automated test — see parked #1. |
| SC-003 | Aligned (no test) | `encode/decode_utf16le_name:576-595` | ASCII names round-trip UTF-8→UTF-16LE→UTF-8. No automated test — see parked #1. |
| SC-004 | Aligned | `dispatcher/src/lib.rs:1611`; `dispatcher-p2p/src/lib.rs:1025,1147` | Dispatchers wire `config.format_on_init` → `initialize_or_format(format_on_init, partition_config)`; when false, a non-Certus layout is not reformatted. |

**Key Entities — all 4 Aligned:** `PartitionConfig`, `PartitionSpec`,
`PartitionTable`, `PartitionInfo` map to the interface types in
`ipartition_table.rs` and the resolved output built by `write_gpt` /
`try_read_gpt_at`.

---

## Resolved in prior syncs (re-confirmed aligned this sweep)

- **FR-003 (BACKFILL, 2026-08-20).** The backup-fallback-on-signature-corruption
  fix is present in `read_gpt` (`gpt.rs:66-96`): the match arm at `:79-80`
  covers `CorruptTable(_) | NoPartitionTable(_)`, so a torn/partial write that
  damages the primary *signature* recovers via the backup header instead of
  being destructively reformatted by `initialize_or_format`. Re-verified: the
  arm, the backup read (`:87-95`), and the "neither primary nor backup" terminal
  `NoPartitionTable` (`:92`) are all present and correct.
- **PR-002 (BACKFILL, 2026-08-07).** The requirement text was aligned to the
  real per-sector `Command::ReadSync` loop in `read_bytes` (`:451-495`); the
  16 KiB entry array is 32 sectors @512 B / 4 @4096 B, so initialize is
  1+32=33 round-trips @512 B (5 @4096 B). Still O(1) in device size (same intent
  as PR-001). Re-verified the loop is unchanged.

## Parked / tracked (not spec↔impl behavioral drift)

1. **No automated tests** (medium, tracked). No `#[test]` / `#[cfg(test)]`
   anywhere under the component and `[dev-dependencies]` is empty
   (`Cargo.toml:15`). SC-001, SC-002, SC-003 are behaviorally aligned but have
   **zero automated verification**. This is a **code-level test-coverage gap,
   not a spec-text contradiction** — the Success Criteria remain the target and
   must not be weakened. The unit tests are already enumerated in the
   component's `tasks.md` (round-trip, backup fallback, layout errors, name
   encoding, MBR); adding them requires editing `src/`/`Cargo.toml`, out of
   scope for a spec-sync apply.
2. **Hardcoded primary entry LBA on read** (low, documented). `read_gpt` calls
   `try_read_gpt_at(1, 2)` (`gpt.rs:68`), assuming the primary entry array always
   begins at LBA 2 rather than honoring the parsed `header.partition_entry_lba`
   (which *is* parsed, `:380`). Every Certus-written GPT uses LBA 2 (`write_gpt`
   sets `partition_entry_lba: 2` `:185`), so this round-trips our own tables
   correctly; an externally-authored GPT with a different entry offset would be
   read incorrectly. Documented in the spec's Implementation Notes; honoring
   `partition_entry_lba` is a future robustness change, not actionable drift.
3. **`generate_guid` silent zero-fallback** (low, documented). If `/dev/urandom`
   cannot be opened, `generate_guid` (`gpt.rs:564-574`) returns an all-zero GUID
   except the version/variant bits — non-random and collision-prone. Called out
   in the spec's Implementation Notes as a known behavior rather than an
   FR-level guarantee; erroring out on `/dev/urandom` failure would be a future
   code change. Not a spec↔impl contradiction as written.

## Stamp rationale

`drift_status: clean`. Every FR (1-11), IR (1-3), PR (1-2), Success-Criteria
statement, and Key-Entity row is behaviorally aligned with the shipped
`src/gpt.rs` / `src/lib.rs` and the `IPartitionTable` interface, independently
re-verified this sweep against current line anchors (not carried over from the
stale, unstamped 2026-08-07 report). The two items that report had marked
*Drifted* — FR-003 (signature-corruption backup fallback) and PR-002 (per-sector
read count) — were both resolved by prior syncs (FR-003 by a code fix now present
at `gpt.rs:66-96`; PR-002 by a spec backfill to the real behavior) and are
confirmed aligned here. **No `src/`, `spec.md`, or interface source was changed
this sweep**, so no build/test/clippy/doc state changed and the committed digest
is stable. The three remaining items — the absent unit tests, the hardcoded
LBA-2 read, and the `generate_guid` zero-fallback — are a tracked
code-completeness follow-up and two low-severity behaviors already documented in
the spec's Implementation Notes; **none is a spec↔implementation behavioral
contradiction**. This is not a clean stamp over an unacknowledged mismatch:
every remaining gap is documented here and in the spec.
