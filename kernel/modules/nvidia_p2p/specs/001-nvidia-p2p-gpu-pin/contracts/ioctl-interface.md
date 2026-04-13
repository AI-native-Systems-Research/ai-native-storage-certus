# Ioctl Interface Contract: NVIDIA P2P GPU Memory Pinning

**Feature**: 001-nvidia-p2p-gpu-pin
**Date**: 2026-04-13

## Device

- **Path**: `/dev/nvidia_p2p`
- **Type**: Misc character device (major 10, dynamic minor)
- **Mode**: 0600 (owner root)
- **Access**: Requires `CAP_SYS_RAWIO`

## Ioctl Magic and Command Numbers

```c
#define NVP2P_IOC_MAGIC  'N'

#define NVP2P_IOCTL_PIN       _IOWR(NVP2P_IOC_MAGIC, 1, struct nvp2p_pin_args)
#define NVP2P_IOCTL_UNPIN     _IOW(NVP2P_IOC_MAGIC, 2, struct nvp2p_unpin_args)
#define NVP2P_IOCTL_GET_PAGES _IOWR(NVP2P_IOC_MAGIC, 3, struct nvp2p_get_pages_args)
```

## Command 1: NVP2P_IOCTL_PIN

Pin a GPU virtual address range and return a handle with metadata.

### Input/Output Structure

```c
struct nvp2p_pin_args {
    /* Input */
    __u64 virtual_address;  /* GPU VA, must be 64KB-aligned */
    __u64 length;           /* Bytes, must be multiple of 64KB, > 0 */

    /* Output (filled by kernel on success) */
    __u64 handle;           /* Opaque handle for this pinned region */
    __u32 page_count;       /* Number of pages in the pinned region */
    __u32 page_size;        /* nvidia_p2p_page_size_type enum value */
};
```

### Behavior

1. Validate `virtual_address` is 64KB-aligned; return `-EINVAL` if not.
2. Validate `length` is a positive multiple of 64KB; return `-EINVAL` if not.
3. Acquire per-fd mutex.
4. Scan existing regions for overlap with [virtual_address, virtual_address + length);
   return `-EEXIST` if overlap found.
5. Call `nvidia_p2p_get_pages_persistent(virtual_address, length, &page_table, 0)`.
6. On NVIDIA error, release mutex, return the NVIDIA error code.
7. Allocate `nvp2p_region`, assign handle from counter, link to fd region list.
8. Fill output fields: handle, page_count (from `page_table->entries`),
   page_size (from `page_table->page_size`).
9. Release mutex.
10. Return 0.

### Error Returns

| errno | Condition |
|-------|-----------|
| `-EINVAL` | VA not 64KB-aligned, or length not multiple of 64KB, or length is 0 |
| `-EEXIST` | Overlapping region already pinned on this fd |
| `-ENOMEM` | NVIDIA driver out of memory |
| `-EIO` | NVIDIA driver internal error |
| `-ENOTSUPP` | Operation not supported by NVIDIA driver |

## Command 2: NVP2P_IOCTL_UNPIN

Release a previously pinned GPU memory region.

### Input Structure

```c
struct nvp2p_unpin_args {
    __u64 handle;           /* Handle from NVP2P_IOCTL_PIN */
};
```

### Behavior

1. Acquire per-fd mutex.
2. Search region list for matching handle; return `-EINVAL` if not found.
3. Remove region from list.
4. Release mutex.
5. Call `nvidia_p2p_put_pages_persistent(region->virtual_address, region->page_table, 0)`.
6. Free region memory.
7. Return 0.

### Error Returns

| errno | Condition |
|-------|-----------|
| `-EINVAL` | Handle not found (never pinned or already unpinned) |

## Command 3: NVP2P_IOCTL_GET_PAGES

Retrieve the physical addresses and metadata for a pinned region.

### Input/Output Structure

```c
struct nvp2p_get_pages_args {
    /* Input */
    __u64 handle;           /* Handle from NVP2P_IOCTL_PIN */
    __u64 phys_addr_buf;    /* User-space pointer to __u64[] buffer */
    __u32 buf_count;        /* Max entries the buffer can hold */
    __u32 _pad;             /* Alignment padding, must be 0 */

    /* Output (filled by kernel on success) */
    __u32 entries_written;  /* Actual number of addresses copied */
    __u32 page_size;        /* nvidia_p2p_page_size_type enum value */
    __u8  gpu_uuid[16];     /* GPU UUID */
};
```

### Behavior

1. Acquire per-fd mutex.
2. Search region list for matching handle; return `-EINVAL` if not found.
3. Compute `n = min(region->page_table->entries, buf_count)`.
4. Copy `n` physical addresses from `page_table->pages[i]->physical_address`
   to user-space buffer at `phys_addr_buf` using `copy_to_user()`.
5. Fill `entries_written = n`, `page_size`, copy `gpu_uuid` (16 bytes).
6. Release mutex.
7. Return 0.

### Error Returns

| errno | Condition |
|-------|-----------|
| `-EINVAL` | Handle not found |
| `-EFAULT` | `phys_addr_buf` is an invalid user-space pointer |

## Rust Library API Mapping

The Rust library combines PIN + GET_PAGES into a single user-facing call:

```rust
impl NvP2pDevice {
    /// Pin GPU memory and retrieve physical addresses.
    pub fn pin_gpu_memory(&self, va: u64, len: u64)
        -> Result<PinnedMemory, Error>
    {
        // 1. IOCTL_PIN → get handle, page_count, page_size
        // 2. Allocate Vec<u64> with capacity page_count
        // 3. IOCTL_GET_PAGES → fill physical_addresses
        // 4. Construct PinnedMemory
    }
}

impl PinnedMemory {
    /// Explicitly unpin. Also called by Drop.
    pub fn unpin(mut self) -> Result<(), Error> {
        // IOCTL_UNPIN
    }

    pub fn physical_addresses(&self) -> &[u64] { ... }
    pub fn page_size(&self) -> PageSize { ... }
    pub fn page_count(&self) -> u32 { ... }
    pub fn physical_address(&self) -> u64 { ... }  // first/base address
}

impl Drop for PinnedMemory {
    fn drop(&mut self) {
        if !self.unpinned {
            // Best-effort IOCTL_UNPIN, log error on failure
        }
    }
}
```
