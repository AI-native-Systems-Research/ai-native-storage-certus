# Feature Specification: GPT Partition Management

**Feature Branch**: `001-gpt-partition-management`
**Created**: 2026-07-01
**Status**: Backfilled
**Source**: Generated from existing implementation
**Last Synced**: 2026-09-03 — spec-sync re-verification pass. All 11 FR / 3 IR / 2 PR / 4 SC + 4 key entities independently re-confirmed aligned against the current `src/gpt.rs`, `src/lib.rs`, and `IPartitionTable` interface; **no spec change was needed** (the 2026-08-20 Phase B backfills for FR-003 and PR-002 remain accurate). The `.specify/sync/drift-report.md` was refreshed and stamped `clean` for the CI Spec-Sync Gate — the prior report was the unstamped 2026-08-07 artifact whose anchors had drifted ~10 lines and which still (incorrectly) marked FR-003/PR-002 as Drifted. (Prior: 2026-08-20 — Phase B spec-sync. FR-003 re-classified from ALIGN (2026-08-07) to **BACKFILL**: the backup-fallback-on-signature-corruption fix is present in `read_gpt` (`src/gpt.rs:66-96`), so the requirement is resolved by describing the implemented behavior rather than a pending code task. PR-002 remains backfilled to the actual per-sector read behavior. Read-path LBA-2 assumption, GUID zero-fallback, and any-sector-size behavior documented in Implementation Notes. Supersedes the 2026-08-07 drift sweep on branch `sync/spec-drift-sweep-20260807`.)

## Backfill Notice

> ⚠️ This spec was generated from existing code via `speckit.sync.backfill`.
> It documents current behavior, not original intent.
> Review carefully and update to reflect desired behavior.

## Overview

The `disk-partition-manager` component provides UEFI GPT partition table management for NVMe block devices within the Certus storage system. It creates, reads, and validates GPT partition tables to divide a physical NVMe drive into isolated regions (metadata, extended metadata, data) used by the extent-manager and other downstream components.

## User Scenarios & Testing

### User Story 1 - Format New Drive (Priority: P1)

As a Certus dispatcher, I want to partition a new NVMe drive into metadata and data regions so that the extent-manager can store its checkpoint structures separately from user data.

**Acceptance Scenarios**:

1. **Given** an unformatted NVMe device, **When** `format()` is called with a 3-partition config (metadata 128 MiB, extended-metadata 128 MiB, data rest-of-disk), **Then** a valid GPT is written with protective MBR, primary header, backup header, and all three partitions are correctly sized and positioned.

2. **Given** a partition config where fixed-size partitions exceed total device capacity, **When** `format()` is called, **Then** a `LayoutError` is returned.

3. **Given** a partition config with more than one `size_bytes=0` partition, **When** `format()` is called, **Then** a `LayoutError` is returned.

### User Story 2 - Read Existing Partition Table (Priority: P1)

As a Certus dispatcher, I want to read an existing GPT partition table from a previously formatted drive so that I can recover partition offsets without reformatting.

**Acceptance Scenarios**:

1. **Given** a drive with a valid primary GPT, **When** `initialize()` is called, **Then** the partition table is returned with correct LBA offsets, sizes, type GUIDs, and names.

2. **Given** a drive with a corrupt primary GPT but valid backup GPT, **When** `initialize()` is called, **Then** the backup GPT is used and the partition table is returned successfully.

3. **Given** a drive with both primary and backup GPT corrupt, **When** `initialize()` is called, **Then** a `NoPartitionTable` error is returned.

4. **Given** a drive whose primary GPT header *signature* is damaged or zeroed (e.g. a torn/partial write) but whose backup GPT is valid, **When** `initialize()` is called, **Then** the backup GPT is used, the partition table is returned successfully, and the disk is NOT reformatted.

### User Story 3 - Initialize-or-Format (Priority: P1)

As a Certus dispatcher, I want to automatically format a drive only when no valid GPT exists so that existing data is preserved across restarts.

**Acceptance Scenarios**:

1. **Given** `force_format=false` and a valid GPT on disk, **When** `initialize_or_format()` is called, **Then** the existing table is returned and `formatted=false`.

2. **Given** `force_format=false` and no valid GPT on disk, **When** `initialize_or_format()` is called, **Then** a new GPT is written and `formatted=true`.

3. **Given** `force_format=true`, **When** `initialize_or_format()` is called regardless of existing state, **Then** a fresh GPT is written and `formatted=true`.

## Requirements

### Functional Requirements

- **FR-001**: System MUST implement the UEFI GPT specification (revision 1.0) with protective MBR, primary header at LBA 1, backup header at last LBA, and 128 partition entries of 128 bytes each.

- **FR-002**: System MUST validate GPT header integrity via CRC32 (IEEE) on both the header and partition entry array.

- **FR-003**: System MUST support primary/backup GPT redundancy — if the primary header is corrupt, the backup header at the last LBA MUST be attempted. "Corrupt" includes **both** a header/entry-array CRC mismatch **and** a damaged or zeroed primary header *signature* (a torn write can damage either). *(Sync 2026-08-20 — implemented and backfilled to reality: `read_gpt` (`src/gpt.rs:66-96`) falls through to the backup-header attempt on **both** `CorruptTable` (CRC mismatch) and `NoPartitionTable` (signature damage from `parse_header`). A genuinely unformatted disk still returns `NoPartitionTable` — "neither primary nor backup GPT header is valid" — after both reads fail, so `initialize_or_format`'s reformat path for truly-blank disks is preserved; only the damaged-primary/valid-backup case is affected, and it now recovers instead of being destructively reformatted. This supersedes the 2026-08-07 sweep, which had tracked the fix as "drafted on branch" via an ALIGN task; the fix is present in the current code, so FR-003 is resolved by spec backfill and no longer carries an ALIGN task.)*

- **FR-004**: System MUST support a "rest of disk" partition (indicated by `size_bytes=0`) that consumes all remaining usable LBAs after fixed-size partitions are allocated.

- **FR-005**: System MUST reject configurations with more than one `size_bytes=0` partition.

- **FR-006**: System MUST reject configurations where total fixed-size partitions exceed the device's usable space.

- **FR-007**: System MUST write a protective MBR (type 0xEE) at LBA 0 to prevent legacy tools from misinterpreting the disk.

- **FR-008**: System MUST generate RFC 4122 version-4 (random) GUIDs for disk GUID and partition unique GUIDs.

- **FR-009**: System MUST encode/decode partition names as UTF-16LE (up to 36 characters per UEFI spec).

- **FR-010**: System MUST support configurable NVMe namespace ID for multi-namespace devices.

- **FR-011**: System MUST support both 512-byte and 4096-byte sector sizes.

### Interface Requirements

- **IR-001**: System MUST provide the `IPartitionTable` interface with methods: `initialize()`, `format()`, `partition_info()`, `num_partitions()`.

- **IR-002**: System MUST accept an `IBlockDevice` receptacle for underlying storage I/O.

- **IR-003**: System MUST expose `initialize_or_format()` as a convenience method combining read-or-create semantics with a `formatted` flag in the return value.

### Performance Requirements

- **PR-001**: Format operation MUST complete in O(1) I/O operations relative to device size (only header/entry sectors are written, not the full device).

- **PR-002**: Initialize (read) operation reads a fixed, device-size-independent set of sectors: 1 header sector plus the partition-entry array. *(Sync 2026-08-07 — backfilled to actual behavior: `read_bytes` issues one synchronous `Command::ReadSync` per sector, so the 128×128 B = 16 KiB entry array is read as 32 round-trips at 512 B sector size (4 at 4096 B), i.e. 33 (5) round-trips total for initialize — not the "at most 2" the requirement originally stated. This is O(1) in device size, satisfying the same intent as PR-001. Batching the entry-array read into a single multi-sector I/O to restore a 2-round-trip happy path is a deliberate optimization left for a future perf task, not tracked as drift.)*

## Key Entities

- **PartitionConfig**: Input configuration specifying sector size, total sectors, namespace ID, and an ordered list of `PartitionSpec` entries.
- **PartitionSpec**: Per-partition specification: type GUID, size in bytes (0 = rest of disk), and name.
- **PartitionTable**: Output containing resolved partition info (start LBA, sector count, GUIDs, names) and device sector size.
- **PartitionInfo**: Per-partition resolved state: index, start LBA, sector count, type GUID, unique GUID, name.

## Well-Known Type GUIDs

| Name | UUID | Usage |
|------|------|-------|
| CERTUS_METADATA | 7C3A8E01-1B4F-4A2D-9E6C-0D3F5A8B7C01 | Extent-manager checkpoint/superblock region |
| CERTUS_DATA | 7C3A8E02-1B4F-4A2D-9E6C-0D3F5A8B7C02 | Extent-manager data region |
| CERTUS_EXTERNAL_META | 7C3A8E03-1B4F-4A2D-9E6C-0D3F5A8B7C03 | Extended metadata (reserved for future use) |

## Dependencies

- **IBlockDevice interface** (`components/interfaces`): Provides NVMe I/O operations (read/write sectors).
- **Component Framework** (`components/component-framework`): `define_component!` macro, receptacle binding.
- **crc32fast**: CRC32 computation for GPT header and entry validation.

## Success Criteria

- **SC-001**: A formatted drive can be read back and produces identical partition offsets.
- **SC-002**: Corrupt primary GPT triggers transparent fallback to backup GPT.
- **SC-003**: Round-trip name encoding (UTF-8 → UTF-16LE → UTF-8) preserves all ASCII partition names.
- **SC-004**: Integration with dispatcher: drives initialize without reformatting on restart when `format_on_init=false`.

## Implementation Notes

> These notes capture current implementation details that may or may not
> belong in the spec long-term.

- I/O is performed via synchronous `Command::ReadSync`/`Command::WriteSync` messages over the block device's client channel (not async).
- GUIDs are generated from `/dev/urandom`. *(Sync 2026-08-07: if `/dev/urandom` cannot be opened, `generate_guid` silently returns an all-zero GUID except for the version/variant bits — non-random and collision-prone. This contradicts FR-008's unconditional "random GUIDs" assertion. Documented here as a known behavior rather than an FR-level guarantee; erroring out on `/dev/urandom` failure would be a future code change.)*
- The component uses `define_component!` with a single `block_device: IBlockDevice` receptacle and internal `Mutex<Option<PartitionTable>>` state.
- Partition entries are always padded to 128 (GPT_MAX_ENTRIES) regardless of how many partitions are configured.
- **Read path assumes Certus-written layout (Sync 2026-08-07):** on read, the primary partition-entry array is read from a hardcoded LBA 2 rather than honoring the parsed `header.partition_entry_lba`. Every Certus-written GPT uses LBA 2, so this is correct for round-tripping our own tables, but a GPT written by an external tool with a different entry offset would be read incorrectly. Low severity; honoring `partition_entry_lba` is a robustness improvement left for a future change. (US2's "read an existing partition table" is therefore scoped in practice to Certus-written and standard-layout tables.)
- **Sector size (Sync 2026-08-07):** FR-011 requires supporting 512 B and 4096 B sectors. The implementation is fully sector-size-parameterized and does not hardcode 512; it also performs no explicit validation that `sector_size ∈ {512, 4096}` — any sector size passed in is accepted. Adding explicit validation (or broadening FR-011 to "any sector size") is a future decision.
