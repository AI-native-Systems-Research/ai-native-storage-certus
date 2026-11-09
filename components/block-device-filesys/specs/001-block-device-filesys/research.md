# Research: Block Device Filesys Component

**Date**: 2026-06-04

## R1: io_uring for File IO in Rust

**Decision**: Use the `io-uring` crate (crates.io: io-uring, repo: tokio-rs/io-uring).

**Rationale**:
- Thin, safe wrapper around the io_uring syscall interface
- No async runtime dependency (operates on raw file descriptors)
- Supports all needed operations: read, write, fsync, nop (for cancel)
- Well-maintained (part of tokio-rs ecosystem)
- Compatible with our actor model — the actor thread owns the ring and polls completions

**Alternatives considered**:
- Raw libc syscalls (`io_uring_setup`, `io_uring_enter`): More control but significantly more boilerplate and unsafe code. Rejected for maintainability.
- `glommio`/`nuclei`: Full async runtimes built on io_uring. Too opinionated — conflict with our actor thread model. Rejected for architectural incompatibility.

**Key API surface needed**:
- `IoUring::new(entries)` — create the ring with submission queue depth
- `opcode::Read::new(fd, buf, len).offset(off)` — async read at offset
- `opcode::Write::new(fd, buf, len).offset(off)` — async write at offset
- `opcode::Fsync::new(fd).flags(FsyncFlags::DATASYNC)` — async fdatasync
- `submitter().submit_and_wait(n)` — submit and wait for completions
- `completion()` — iterate completion queue entries

**Minimum kernel**: 5.6 (for `IORING_OP_READ`/`IORING_OP_WRITE` with fixed offsets). RHEL 9 kernel 5.14 satisfies this.

## R2: fdatasync Durability Strategy

**Decision**: Call fdatasync after every write operation (sync and async).

**Rationale**:
- Simulates real NVMe write completion semantics where data is persisted before the completion is signaled
- Critical for correctness testing of extent-manager crash consistency
- For sync ops: pwrite + fdatasync (two syscalls)
- For async ops: io_uring write SQE followed by linked fsync SQE (io_uring link chains ensure ordering)

**Alternatives considered**:
- O_DSYNC open flag: Would make every write durable but prevents batching fsync with io_uring link chains. Also affects reads (unnecessary overhead).
- Buffered without sync: Fastest but doesn't simulate block device semantics. Rejected per clarification decision.
- Periodic fsync: Reduces fsync overhead but doesn't guarantee per-write durability. Rejected.

**Implementation note**: io_uring supports `IOSQE_IO_LINK` flag to chain write→fsync atomically in the submission queue. This ensures the fsync executes only after the write completes, without requiring a round-trip to userspace between them.

## R3: DmaBuffer Integration Without SPDK

**Decision**: Access DmaBuffer byte slices directly via `as_slice()`/`as_mut_slice()` and pass the underlying pointer to IO syscalls.

**Rationale**:
- DmaBuffer already provides `as_ptr() -> *mut c_void`, `as_slice() -> &[u8]`, `as_mut_slice() -> &mut [u8]`
- For pread/pwrite: use `as_mut_slice().as_ptr()` as the buffer argument
- For io_uring: use `as_ptr()` cast to `*mut u8` for the buffer address in SQEs
- No SPDK allocator needed — callers in tests can use `DmaBuffer::from_raw` with standard heap memory

**Alternatives considered**:
- Intermediate copy to Vec<u8>: Wasteful — doubles memory and adds copy latency. Rejected.
- Custom allocator producing DmaBuffer-compatible objects: Over-engineered for a file-backed device. Rejected.

**Test buffer allocation**: For unit tests, DmaBuffer::from_raw can wrap a heap allocation with a libc::free deallocator, avoiding any SPDK dependency.

## R4: Actor Model with io_uring Event Loop

**Decision**: The actor thread runs a combined event loop that polls both the client command channel and io_uring completions.

**Rationale**:
- Matches block-device-spdk-nvme pattern: actor polls ingress channels per handle() call
- io_uring completions are harvested in the same loop iteration
- No context switching between command processing and IO completion
- Back-pressure: if SQ is full, actor defers new submissions until CQ has entries

**Design**:
1. Actor `handle()` is called when a ControlMessage arrives (connect/disconnect client)
2. Actor's periodic poll: drain all client ingress channels for Commands
3. For each Command, either:
   - Sync ops (ReadSync/WriteSync): execute inline with pread/pwrite + fdatasync, send Completion immediately
   - Async ops (ReadAsync/WriteAsync): submit to io_uring SQ with linked fsync, track OpHandle→client mapping
   - WriteZeros: pwrite zeros + fdatasync (inline, sync)
   - NsProbe: return static namespace info
   - BatchSubmit: process sequentially
   - AbortOp: attempt io_uring cancel via `opcode::AsyncCancel`
4. After processing commands, harvest io_uring CQ entries and dispatch Completions to appropriate clients
5. Timeout handling: maintain a deadline map; on each iteration check for expired ops

## R5: Backing File Management

**Decision**: Use fallocate for pre-allocation, open with O_RDWR, and error on size mismatch.

**Rationale**:
- fallocate(FALLOC_FL_ZERO_RANGE) pre-allocates without physical writes
- Avoids sparse file holes that cause inconsistent read performance
- Deterministic device geometry: file size = block_size × num_blocks

**Implementation**:
1. On initialize():
   - If file doesn't exist: create with O_RDWR|O_CREAT, fallocate to full size
   - If file exists: open O_RDWR, check size == block_size × num_blocks, error if mismatch
2. File descriptor is owned by the actor thread (passed at actor creation)
3. On shutdown: close fd (implicit on Drop)

## R6: Component Framework Conformance

**Decision**: Follow block-device-spdk-nvme pattern exactly for define_component! usage.

**Key differences from SPDK variant**:
- No `spdk_env` receptacle (not needed)
- No PCI address configuration (replaced by file path + block_size + num_blocks)
- Admin interface: `set_file_path()`, `set_block_size()`, `set_num_blocks()`, `initialize()`, `shutdown()`
- IBlockDeviceAdmin methods: `set_pci_address` and `set_actor_cpu` → return NotSupported (or we define a separate IBlockDeviceFilesysAdmin)

**Resolution**: Since the spec says "interfaces defined in components/interfaces crate" and we implement IBlockDevice, we should NOT define a new admin interface. Instead, configuration is done via the component's own methods (not through a trait interface) — the same pattern block-device-spdk-nvme uses for `set_pci_address` before wrapping it in `IBlockDeviceAdmin`. We will provide configuration methods on the component struct directly (pub(crate)) and call them in tests/apps.
