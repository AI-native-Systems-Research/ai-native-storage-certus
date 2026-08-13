Generated: 2026-08-07T15:30:30Z

# Drift Report: disk-partition-manager

Spec: `.specify/specs/001-gpt-partition-management/spec.md` (Status: Backfilled)
Implementation: `src/lib.rs`, `src/gpt.rs`
Interface: `components/interfaces/src/ipartition_table.rs`

## Summary

| Status          | Count |
|-----------------|-------|
| Aligned         | 18    |
| Drifted         | 2     |
| Not Implemented | 0     |
| Unspecced       | 2     |

Total requirements analyzed: 20 (11 FR + 3 IR + 2 PR + 4 SC).

## Per-Requirement Table

| ID     | Status   | Location | Notes |
|--------|----------|----------|-------|
| FR-001 | Aligned  | gpt.rs:9,11,12,197,206,319,344 | Rev 1.0, 128×128B entries, primary@LBA1, backup@last, protective MBR@LBA0. |
| FR-002 | Aligned  | gpt.rs:97-119,416 | CRC32 (crc32fast = IEEE) on header and entry array, both read and write. |
| FR-003 | Drifted (Medium) | gpt.rs:66-86,73,353-357 | Backup fallback fires only on `CorruptTable` (CRC mismatch). A corrupt/zeroed primary *signature* returns `NoPartitionTable` and is propagated at line 73 without trying the backup. |
| FR-004 | Aligned  | gpt.rs:240-267,270 | `size_bytes=0` → rest-of-disk. |
| FR-005 | Aligned  | gpt.rs:246-250 | >1 rest-of-disk rejected with LayoutError. |
| FR-006 | Aligned  | gpt.rs:260-265 | Fixed partitions exceeding usable space rejected. |
| FR-007 | Aligned  | gpt.rs:329,344 | Protective MBR type 0xEE at LBA 0. |
| FR-008 | Aligned  | gpt.rs:554-564 | RFC 4122 v4 GUIDs (version/variant bits set). See unspecced note on urandom-failure fallback. |
| FR-009 | Aligned  | gpt.rs:566-585 | UTF-16LE encode/decode capped at 36 chars. |
| FR-010 | Aligned  | lib.rs:30-36,91 | `set_ns_id()` for initialize path; `config.ns_id` for format path. |
| FR-011 | Aligned  | gpt.rs:314-317,437-543 | Sector size fully parameterized; no 512-only hardcoding. No explicit 512/4096 validation (any size accepted). |
| IR-001 | Aligned  | ipartition_table.rs:116-128; lib.rs:67-120 | All four methods provided. |
| IR-002 | Aligned  | lib.rs:17-19 | `block_device: IBlockDevice` receptacle. |
| IR-003 | Aligned  | lib.rs:45-64 | `initialize_or_format()` returns `(PartitionTable, bool)` formatted flag. |
| PR-001 | Aligned  | gpt.rs:194-206 | Format writes a fixed set of sectors (MBR + 2 headers + 2× entry arrays), independent of device size → O(1) vs device size. |
| PR-002 | Drifted (Medium) | gpt.rs:441-485,68,110 | Read is NOT ≤2 round-trips. `read_bytes` issues one `ReadSync`/`recv` per sector in a loop; the 16 KiB entry array = 32 round-trips @512B (4 @4096B), plus 1 header read. |
| SC-001 | Aligned (no test) | gpt.rs:142-227,88-140 | Write/read use identical LBA↔sector math; round-trip offsets identical. No test exists to verify. |
| SC-002 | Aligned (no test, caveat) | gpt.rs:66-86 | CRC-corrupt primary transparently falls back to backup. Caveat: signature corruption does NOT (see FR-003). No test exists. |
| SC-003 | Aligned (no test) | gpt.rs:566-585 | ASCII names round-trip UTF-8→UTF-16LE→UTF-8. No test exists. |
| SC-004 | Aligned  | dispatcher/src/lib.rs:1517,1670; dispatcher-p2p/src/lib.rs:1025,1147 | Dispatcher wires `config.format_on_init` → `initialize_or_format(force_format, ...)`; when false, non-Certus layout does not reformat. |

## Detailed Findings

### FR-003 — Backup fallback only covers CRC corruption (Drifted, Medium)
`read_gpt` (gpt.rs:66-86) matches on the result of `try_read_gpt_at`. Only the
`CorruptTable` variant triggers the backup attempt; every other error, including
the `NoPartitionTable` returned by `parse_header` (gpt.rs:353-357) when the
primary header signature is invalid/zeroed, is returned immediately at line 73.
The spec (FR-003, US2 scenario 2) says "if the primary header is corrupt, the
backup header ... MUST be attempted." A torn/partial write that damages the
primary signature bytes is a plausible corruption mode that this path does not
recover from. Consequence is amplified by `initialize_or_format`
(lib.rs:57-61): a `NoPartitionTable` from a signature-damaged primary is treated
as "no table present" and triggers a (destructive) reformat rather than a backup
recovery.

Recommended fix: on `NoPartitionTable` from the primary read, also attempt the
backup header before giving up.

### PR-002 — Read exceeds the stated 2 I/O round-trips (Drifted, Medium)
The spec caps the initialize happy path at "at most 2 I/O round-trips (header +
entries)". The implementation's `read_bytes` (gpt.rs:441-485) loops per sector,
sending a single-sector `Command::ReadSync` and blocking on `completion_rx.recv()`
each iteration. The partition entry array is `128 × 128 = 16 KiB`, i.e. 32 sectors
at 512 B (4 sectors at 4096 B). Initialize therefore performs 1 (header) + 32
(entries) = 33 synchronous round-trips at 512 B sector size, far above the
specified 2. Same per-sector pattern applies to writes but PR-001 only constrains
scaling vs device size (which holds), so PR-001 is unaffected.

Recommended fix: either issue a single multi-sector read for the entry array, or
relax PR-002 to reflect per-sector I/O.

## Unspecced Code

| Item | Location | Notes |
|------|----------|-------|
| Hardcoded primary entry LBA on read | gpt.rs:68 | `try_read_gpt_at(1, 2)` assumes primary entries always begin at LBA 2 instead of honoring the parsed `header.partition_entry_lba` (which IS parsed at gpt.rs:370). A Certus-written GPT always uses LBA 2, but a GPT written by an external tool with a different entry offset would be read incorrectly, conflicting with US2's "read an existing partition table" intent. Low severity. |
| `generate_guid` silent zero-fallback | gpt.rs:556-559 | If `/dev/urandom` cannot be opened, the GUID stays all-zero except the version/variant bits — non-random and collision-prone. Called out in spec Implementation Notes but not covered by any FR; FR-008 asserts random GUIDs unconditionally. Low severity. |

## Referenced-but-Nonexistent Files / Proofs / Tests

- `plan.md:84` and all of `tasks.md` state no dedicated test files exist; confirmed — no `#[test]`/`#[cfg(test)]` anywhere under the component and `[dev-dependencies]` is empty (Cargo.toml:15). SC-001, SC-002, SC-003 are functionally aligned but have zero automated verification.
- No spec statement references a concrete file/dir/proof that is missing; the plan's disk-layout and module-structure descriptions match the code.

## Recommendations

1. FR-003: extend backup fallback to also cover `NoPartitionTable` (signature) failures on the primary header, before `initialize_or_format` can misclassify a damaged primary as "unformatted" and reformat.
2. PR-002: batch the entry-array read into one multi-sector I/O, or amend the requirement to describe per-sector round-trips.
3. Add the unit tests already enumerated in `tasks.md` (round-trip, backup fallback, layout errors, name encoding, MBR) so SC-001..SC-003 are verifiable.
4. FR-011: consider validating `sector_size ∈ {512, 4096}` explicitly, or broaden the requirement to "any sector size".
5. Read path: honor `header.partition_entry_lba` rather than the hardcoded LBA 2 for robustness against externally-created GPTs.
