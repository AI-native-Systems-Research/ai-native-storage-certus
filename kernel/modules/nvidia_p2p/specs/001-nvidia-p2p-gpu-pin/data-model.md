# Data Model: NVIDIA P2P GPU Memory Pinning

**Feature**: 001-nvidia-p2p-gpu-pin
**Date**: 2026-04-13

## Kernel-Space Entities

### nvp2p_device_state (singleton, module-level)

Module-level state for the misc character device.

| Field | Type | Description |
|-------|------|-------------|
| misc | `struct miscdevice` | Registered misc device |

Lifetime: Created at `module_init`, destroyed at `module_exit`.

### nvp2p_fd_state (per file descriptor)

Per-open-fd state stored in `file->private_data`.

| Field | Type | Description |
|-------|------|-------------|
| regions | `struct list_head` | Head of linked list of `nvp2p_region` |
| lock | `struct mutex` | Protects `regions` list |
| next_handle | `uint64_t` | Monotonically increasing handle counter |

Lifetime: Allocated in `.open`, freed in `.release` after releasing all regions.

### nvp2p_region (per pinned region)

Represents one active P2P pinned memory region.

| Field | Type | Description |
|-------|------|-------------|
| list | `struct list_head` | Linked list node in fd_state.regions |
| handle | `uint64_t` | Opaque handle returned to user space (>0) |
| virtual_address | `uint64_t` | GPU virtual address (64KB-aligned) |
| length | `uint64_t` | Length in bytes (multiple of 64KB) |
| page_table | `struct nvidia_p2p_page_table *` | NVIDIA page table from get_pages_persistent |

Lifetime: Allocated on successful pin, freed on unpin or fd release.

### Relationships

```text
nvp2p_device_state (1)
    │
    └──> file.open() creates nvp2p_fd_state (1 per fd)
              │
              └──> nvp2p_region (0..N per fd, linked list)
                       │
                       └──> nvidia_p2p_page_table_t (1:1, owned by NVIDIA driver)
                                │
                                └──> nvidia_p2p_page_t[] (1:N, physical addresses)
```

### State Transitions

```text
nvp2p_region lifecycle:

  [not exists]
      │
      ├─ IOCTL_PIN (success) ──> [ACTIVE]
      │                              │
      │                              ├─ IOCTL_UNPIN ──> nvidia_p2p_put_pages_persistent()
      │                              │                      ──> kfree(region) ──> [freed]
      │                              │
      │                              └─ fd close (release) ──> nvidia_p2p_put_pages_persistent()
      │                                                           ──> kfree(region) ──> [freed]
      │
      └─ IOCTL_PIN (failure) ──> [not exists] (no region created)
```

## User-Space Entities (Rust)

### NvP2pDevice

| Field | Type | Description |
|-------|------|-------------|
| fd | `OwnedFd` | Open file descriptor to `/dev/nvidia_p2p` |

Lifetime: Opened on construction, closed on Drop.

### PinnedMemory

| Field | Type | Description |
|-------|------|-------------|
| device | `Arc<NvP2pDevice>` | Shared reference to the device |
| handle | `u64` | Kernel-assigned opaque handle |
| virtual_address | `u64` | Original GPU VA that was pinned |
| length | `u64` | Original length |
| page_size | `PageSize` | Page size enum (4KB, 64KB, 128KB) |
| page_count | `u32` | Number of pages |
| physical_addresses | `Vec<u64>` | Physical addresses, one per page |
| unpinned | `bool` | Guard against double-unpin in Drop |

Lifetime: Created by `NvP2pDevice::pin_gpu_memory()`, unpinned on explicit
`unpin()` or on Drop.

### PageSize (enum)

| Variant | Value | Bytes |
|---------|-------|-------|
| Size4KB | 0 | 4,096 |
| Size64KB | 1 | 65,536 |
| Size128KB | 2 | 131,072 |

### Error (enum)

| Variant | Kernel errno | Description |
|---------|-------------|-------------|
| DeviceNotFound | N/A (open fails) | `/dev/nvidia_p2p` does not exist |
| PermissionDenied | EPERM | Caller lacks CAP_SYS_RAWIO |
| InvalidAlignment | EINVAL | VA not 64KB-aligned |
| InvalidLength | EINVAL | Length not multiple of 64KB or zero |
| InvalidHandle | EINVAL | Handle not found or already freed |
| AlreadyPinned | EEXIST | Overlapping VA range already pinned |
| OutOfMemory | ENOMEM | NVIDIA driver memory exhaustion |
| DriverError(i32) | other | Unexpected NVIDIA driver error code |
| IoError(io::Error) | varies | System-level I/O error |

## Shared Ioctl Structures

These structures are defined identically in the kernel header
(`nvidia_p2p_pin.h`) and Rust (`ioctl.rs`).

### nvp2p_pin_request

| Field | Type (C) | Type (Rust) | Description |
|-------|----------|-------------|-------------|
| virtual_address | `__u64` | `u64` | GPU VA to pin (64KB-aligned) |
| length | `__u64` | `u64` | Length in bytes (multiple of 64KB) |

### nvp2p_pin_response

| Field | Type (C) | Type (Rust) | Description |
|-------|----------|-------------|-------------|
| handle | `__u64` | `u64` | Opaque handle for this region |
| page_count | `__u32` | `u32` | Number of pages pinned |
| page_size | `__u32` | `u32` | Page size enum value |

### nvp2p_unpin_request

| Field | Type (C) | Type (Rust) | Description |
|-------|----------|-------------|-------------|
| handle | `__u64` | `u64` | Handle to unpin |

### nvp2p_get_pages_request

| Field | Type (C) | Type (Rust) | Description |
|-------|----------|-------------|-------------|
| handle | `__u64` | `u64` | Handle to query |
| buf_ptr | `__u64` | `u64` | User-space pointer to u64 array |
| buf_count | `__u32` | `u32` | Max entries in buffer |
| _pad | `__u32` | `u32` | Padding for alignment |

### nvp2p_get_pages_response

| Field | Type (C) | Type (Rust) | Description |
|-------|----------|-------------|-------------|
| entries_written | `__u32` | `u32` | Actual entries copied |
| page_size | `__u32` | `u32` | Page size enum value |
| gpu_uuid | `__u8[16]` | `[u8; 16]` | GPU UUID |
