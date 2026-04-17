# Research: Extent Management

**Feature**: 001-extent-management
**Date**: 2026-04-16

## Technical Context Resolution

No NEEDS CLARIFICATION items remain in the Technical Context — all values are determined from the project constitution, CLAUDE.md, and the existing v0 reference implementation.

## Research Findings

### R-001: On-Disk Layout for 10M Slots

**Decision**: Use the three-region layout from CLAUDE.md (superblock → bitmap region → extent record region) with layout calculations that accommodate 10M slots per size class.

**Rationale**: At 10M slots per class, the bitmap for a single class requires 10M bits = 1.25 MiB = 320 blocks (4KiB each). The record region requires 10M blocks (one 4KiB block per slot). For 32 classes at maximum capacity, the total record region is 320M blocks (~1.22 TiB), which is within range for modern NVMe SSDs. The layout math must use u64 for block addressing.

**Alternatives considered**:
- Packing multiple records per block: rejected — simplicity of one record per block enables atomic per-extent updates without read-modify-write
- Multi-level bitmap: rejected — flat bitmap fits in 320 blocks per class which is manageable

### R-002: In-Memory Index Structure

**Decision**: Use `HashMap<u64, ExtentMetadata>` for the key→metadata index, plus per-size-class `AllocationBitmap` structs backed by `Vec<u64>` bitmasks.

**Rationale**: HashMap provides O(1) lookup by key. At 10M extents, the HashMap overhead is ~500MB with standard load factors. Bitmap operations (find-free, set, clear) are O(n/64) with u64 word scanning.

**Alternatives considered**:
- BTreeMap: rejected — O(log n) lookup not needed since keys are opaque u64, not ordered
- Roaring bitmaps: rejected — adds external dependency; flat bitmaps are simple and sufficient at 10M scale

### R-003: Crash Consistency Protocol

**Decision**: Two-phase write protocol as specified in CLAUDE.md:
1. Write extent record block atomically (4KiB with CRC-32 at bytes 4092–4096)
2. Flip bitmap bit and write bitmap block atomically
3. Update in-memory index

Recovery on open(): scan each slot's record vs bitmap bit. Records with no bitmap bit are orphans (zero them). Records with corrupt CRC are cleared from bitmap and zeroed.

**Rationale**: NVMe guarantees 4KiB atomic writes. The two-phase protocol ensures that at any crash point, the bitmap is the source of truth for allocation state. Records without bitmap bits are safely discardable (orphaned write phase 1). Bitmap bits without valid records indicate corruption.

**Alternatives considered**:
- Write-ahead log: rejected — adds complexity; atomic 4KiB writes make WAL unnecessary for single-block metadata
- Copy-on-write: rejected — doubles space overhead for metadata; unnecessary given hardware atomicity guarantee

### R-004: Thread Safety Model

**Decision**: `RwLock<ExtentManagerState>` for the main state. Read lock for lookups and iteration. Write lock for create, remove, and recovery. Iteration holds the read lock for its full duration (exclusive lock behavior as specified — since write lock is needed for create/remove, the read lock held during iteration naturally blocks those).

**Rationale**: The clarified spec requires iteration to block concurrent modifications (exclusive lock model). An RwLock naturally provides this: iteration takes a read lock, and create/remove take write locks. Multiple concurrent lookups can proceed in parallel via shared read locks.

**Alternatives considered**:
- Per-size-class locking: rejected — more complex, and iteration needs global consistency
- Lock-free concurrent map: rejected — adds complexity without clear benefit given the exclusive-iteration requirement

### R-005: MockBlockDevice Design

**Decision**: Actor-thread model with `HashMap<u64, [u8; 4096]>` storage, connected via channel pairs. Supports `FaultConfig` for fail-after-n-writes, fail-on-LBA-range, and fail-all-writes. `reboot_from()` method creates a new component instance sharing the same backing storage to simulate restart.

**Rationale**: Matches v0's proven testing pattern. Actor thread ensures that block device operations are serialized as they would be with real hardware. Channel-based design matches IBlockDevice's client/channel model.

**Alternatives considered**:
- Direct HashMap access (no actor): rejected — doesn't model real IBlockDevice channel semantics
- File-backed test device: rejected — adds filesystem dependency and is slower

### R-006: CRC-32 Implementation

**Decision**: Use `crc32fast` crate for CRC-32 checksums on superblock and extent records.

**Rationale**: `crc32fast` is a well-maintained, zero-dependency crate with hardware acceleration (SSE4.2). It's the standard choice for CRC-32 in the Rust ecosystem.

**Alternatives considered**:
- Manual CRC implementation: rejected — error-prone, slower without hardware acceleration
- CRC-64: rejected — 32-bit is sufficient for 4KiB block integrity; matches the 4-byte CRC field at block offset 4092

### R-007: Filename Storage Format

**Decision**: Store filename as a length-prefixed UTF-8 byte sequence within the extent record block. Maximum 255 bytes (per clarification). A length field of 0 indicates no filename.

**Rationale**: Length-prefix avoids NUL-terminator ambiguity and enables efficient parsing. UTF-8 is the standard Linux filename encoding. 255 bytes aligns with POSIX NAME_MAX.

**Alternatives considered**:
- Fixed 256-byte field with NUL termination: rejected — wastes space when no filename; NUL in filename is technically valid on some systems
- Separate filename table: rejected — adds complexity; filename fits within the 4KiB record alongside other metadata
