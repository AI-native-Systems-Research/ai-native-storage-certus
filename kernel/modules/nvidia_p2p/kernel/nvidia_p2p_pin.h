/* SPDX-License-Identifier: GPL-2.0 */
/*
 * nvidia_p2p_pin.h - Shared ioctl definitions for the nvidia_p2p_pin module.
 *
 * These structures are shared between the kernel module and user-space.
 * The Rust library duplicates these definitions in ioctl.rs.
 */

#ifndef _NVIDIA_P2P_PIN_H
#define _NVIDIA_P2P_PIN_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define NVP2P_IOC_MAGIC 'N'

/*
 * NVP2P_IOCTL_PIN - Pin a GPU virtual address range.
 *
 * Input:  virtual_address (64KB-aligned), length (multiple of 64KB, > 0)
 * Output: handle, page_count, page_size
 */
struct nvp2p_pin_args {
	__u64 virtual_address;  /* GPU VA, must be 64KB-aligned */
	__u64 length;           /* Bytes, must be multiple of 64KB, > 0 */
	__u64 handle;           /* Output: opaque handle for this pinned region */
	__u32 page_count;       /* Output: number of pages */
	__u32 page_size;        /* Output: nvidia_p2p_page_size_type enum */
};

/*
 * NVP2P_IOCTL_UNPIN - Release a previously pinned GPU memory region.
 *
 * Input: handle from NVP2P_IOCTL_PIN
 */
struct nvp2p_unpin_args {
	__u64 handle;
};

/*
 * NVP2P_IOCTL_GET_PAGES - Retrieve physical addresses for a pinned region.
 *
 * Input:  handle, phys_addr_buf (user pointer to __u64[]), buf_count
 * Output: entries_written, page_size, gpu_uuid
 */
struct nvp2p_get_pages_args {
	__u64 handle;           /* Input: handle from PIN */
	__u64 phys_addr_buf;    /* Input: user-space pointer to __u64[] buffer */
	__u32 buf_count;        /* Input: max entries the buffer can hold */
	__u32 _pad;             /* Alignment padding, must be 0 */
	__u32 entries_written;  /* Output: actual addresses copied */
	__u32 page_size;        /* Output: nvidia_p2p_page_size_type enum */
	__u8  gpu_uuid[16];     /* Output: GPU UUID */
};

#define NVP2P_IOCTL_PIN       _IOWR(NVP2P_IOC_MAGIC, 1, struct nvp2p_pin_args)
#define NVP2P_IOCTL_UNPIN     _IOW(NVP2P_IOC_MAGIC, 2, struct nvp2p_unpin_args)
#define NVP2P_IOCTL_GET_PAGES _IOWR(NVP2P_IOC_MAGIC, 3, struct nvp2p_get_pages_args)

#endif /* _NVIDIA_P2P_PIN_H */
