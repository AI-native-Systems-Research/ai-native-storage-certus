# Research: NVIDIA P2P GPU Memory Pinning

**Feature**: 001-nvidia-p2p-gpu-pin
**Date**: 2026-04-13

## R1: NVIDIA Persistent P2P API

**Decision**: Use `nvidia_p2p_get_pages_persistent()` and
`nvidia_p2p_put_pages_persistent()` exclusively.

**Rationale**: The persistent API (available since NVIDIA driver ~520+) avoids
the complexity of managing `p2p_token`, `va_space`, and async `free_callback`
required by the non-persistent API. Since the target driver is 580.126.20 and
MIG is out of scope, the persistent API is fully supported and simpler.

**API Signatures** (from `/usr/src/nvidia-580.126.20/nvidia/nv-p2p.h`):

```c
int nvidia_p2p_get_pages_persistent(
    uint64_t virtual_address,   /* 64KB-aligned GPU VA */
    uint64_t length,            /* multiple of 64KB */
    struct nvidia_p2p_page_table **page_table,  /* output */
    uint32_t flags              /* NVIDIA_P2P_FLAGS_DEFAULT = 0 */
);

int nvidia_p2p_put_pages_persistent(
    uint64_t virtual_address,
    struct nvidia_p2p_page_table *page_table,
    uint32_t flags              /* must be 0 */
);
```

**Page Table Structure**:
```c
struct nvidia_p2p_page_table {
    uint32_t version;
    uint32_t page_size;         /* enum nvidia_p2p_page_size_type */
    struct nvidia_p2p_page **pages;
    uint32_t entries;
    uint8_t *gpu_uuid;          /* 16 bytes */
    uint32_t flags;
};

struct nvidia_p2p_page {
    uint64_t physical_address;
    /* ... registers (not needed for our use case) */
};
```

**Page Size Enum**:
- `NVIDIA_P2P_PAGE_SIZE_4KB = 0`
- `NVIDIA_P2P_PAGE_SIZE_64KB = 1`
- `NVIDIA_P2P_PAGE_SIZE_128KB = 2`

**Alternatives Considered**:
- Non-persistent API (`nvidia_p2p_get_pages`): Requires p2p_token, va_space,
  and free_callback management. Significantly more complex with async
  invalidation handling. Rejected per clarification decision.

## R2: Ioctl Interface Design

**Decision**: Three-ioctl design with two-step physical address retrieval.

**Rationale**: The pin operation returns a variable number of physical addresses
(dependent on region size and page size chosen by the NVIDIA driver). Rather
than imposing a fixed maximum array in the ioctl struct, a two-step approach
is used: `IOCTL_PIN` returns {handle, page_count, page_size}, then
`IOCTL_GET_PAGES` copies physical addresses into a user-supplied buffer. This
keeps the ioctl structs fixed-size and avoids arbitrary limits.

**Ioctl Commands**:
1. `NVP2P_IOCTL_PIN`: Pin a GPU VA range → returns handle + metadata
2. `NVP2P_IOCTL_UNPIN`: Unpin by handle
3. `NVP2P_IOCTL_GET_PAGES`: Retrieve physical addresses for a pinned handle

**Alternatives Considered**:
- Single ioctl with large fixed buffer (e.g., 4096 entries): Wastes stack/copy
  space for small regions, limits large regions. Rejected.
- `mmap` the page table from kernel: Over-engineered for this use case,
  introduces additional memory management complexity. Rejected.
- `read()`/`write()` file operations: Less structured than ioctl for
  command-response patterns. Rejected.

## R3: Kernel Module Character Device Pattern

**Decision**: Use `misc_register()` for character device registration.

**Rationale**: `misc_register()` is the standard pattern for simple character
devices that need a single device node. It auto-allocates a minor number under
major 10, handles `/dev` node creation via udev automatically, and requires
minimal boilerplate compared to `alloc_chrdev_region()` + `cdev_add()` +
`class_create()` + `device_create()`.

**Key Implementation Details**:
- `struct miscdevice` with `.minor = MISC_DYNAMIC_MINOR`, `.name = "nvidia_p2p"`,
  `.mode = 0600`
- `struct file_operations` with `.open`, `.release`, `.unlocked_ioctl`,
  `.compat_ioctl`
- Per-fd state allocated in `.open`, stored in `file->private_data`
- Cleanup in `.release`: iterate pinned regions list, call
  `nvidia_p2p_put_pages_persistent()` for each

**Alternatives Considered**:
- Full cdev registration: More boilerplate, no benefit for a single device node.
  Rejected.
- sysfs/procfs interface: Not suited for command-response operations with
  structured data. Rejected.

## R4: Kernel Region Tracking Data Structure

**Decision**: Per-fd linked list protected by a mutex.

**Rationale**: The expected number of concurrent pinned regions per fd is small
(tens to low hundreds). A linked list with `struct list_head` is the idiomatic
kernel data structure for this scale, provides O(1) insert, and O(n) lookup by
handle is acceptable given the expected cardinality. A mutex (not spinlock) is
appropriate because `nvidia_p2p_get_pages_persistent()` may sleep.

**Handle Generation**: Monotonically increasing `uint64_t` counter per fd. The
counter is incremented atomically. Handle 0 is reserved as invalid.

**Alternatives Considered**:
- `struct xarray` (radix tree): O(log n) lookup by handle, but adds complexity
  for a small-n use case. Could be adopted later if profiling shows lookup as
  a bottleneck. Rejected for now.
- RB-tree keyed by virtual address: Useful for overlap detection but overkill
  when duplicate detection is a simple linear scan. Rejected.

## R5: Rust Ioctl Crate Selection

**Decision**: Use the `nix` crate for ioctl definitions.

**Rationale**: The `nix` crate provides safe Rust wrappers for POSIX/Linux
system calls including `ioctl`. The `ioctl_readwrite!`, `ioctl_read!`, and
`ioctl_write_ptr!` macros generate type-safe ioctl wrappers that minimize
unsafe surface area. The `nix` crate is well-maintained, widely used, and
supports all target platforms (RHEL 9/10 glibc).

**Alternatives Considered**:
- Raw `libc::ioctl()`: More unsafe code, no type safety, manual error handling.
  Rejected.
- `ioctl-rs` crate: Less maintained, smaller community. Rejected.
- Direct syscall via `syscall()`: Maximum unsafe surface. Rejected.

## R6: Build System for nv-p2p.h Auto-Discovery

**Decision**: Makefile with shell glob and version sorting.

**Rationale**: The kernel module Makefile uses a shell command to glob
`/usr/src/nvidia-*/nvidia/nv-p2p.h`, sort by version number (using `sort -V`),
and select the newest. This is set as an `EXTRA_CFLAGS` include path.
If no match is found, the build fails with a descriptive error.

**Implementation**:
```makefile
NV_P2P_DIR := $(shell ls -d /usr/src/nvidia-*/nvidia 2>/dev/null | sort -V | tail -1)
ifeq ($(NV_P2P_DIR),)
  $(error NVIDIA driver source not found. Install nvidia-driver-devel or equivalent.)
endif
EXTRA_CFLAGS += -I$(NV_P2P_DIR)
```

**Alternatives Considered**:
- pkg-config: NVIDIA driver doesn't ship pkg-config files. Not available.
- CMake/Meson: Overkill for a kernel module that must use kbuild. Rejected.
- Hardcoded path: Breaks on driver updates. Explicitly rejected by spec FR-008.

## R7: Overlap Detection for Duplicate Pin Rejection

**Decision**: Linear scan of per-fd region list checking for VA range overlap.

**Rationale**: When a new pin request arrives, the kernel module scans the
per-fd linked list to check if any existing region overlaps with the requested
[virtual_address, virtual_address + length) range. This detects both exact
duplicates and partial overlaps. Per the clarification, overlapping pins are
rejected with `-EEXIST` (mapped to `AlreadyPinned` in Rust).

**Overlap Check**: Two ranges [a_start, a_end) and [b_start, b_end) overlap
if and only if `a_start < b_end && b_start < a_end`.
