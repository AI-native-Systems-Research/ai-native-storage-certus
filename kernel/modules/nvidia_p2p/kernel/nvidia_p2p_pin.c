// SPDX-License-Identifier: GPL-2.0
/*
 * nvidia_p2p_pin.c - Kernel module wrapping the NVIDIA persistent P2P API
 *                    behind a misc character device with ioctl interface.
 *
 * Provides pin/unpin/get_pages operations for GPU memory via /dev/nvidia_p2p.
 * Designed for GPUDirect Storage workflows (DMA between SSD and GPU).
 *
 * Requires: NVIDIA proprietary driver with persistent P2P API support.
 */

#include <linux/module.h>
#include <linux/miscdevice.h>
#include <linux/fs.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/list.h>
#include <linux/uaccess.h>
#include <linux/capability.h>
#include <linux/compat.h>

#include <nv-p2p.h>
#include "nvidia_p2p_pin.h"

#define NVP2P_ALIGN_64KB 0x10000ULL
#define NVP2P_ALIGN_MASK (NVP2P_ALIGN_64KB - 1)

/* Per-pinned-region state */
struct nvp2p_region {
	struct list_head list;
	uint64_t handle;
	uint64_t virtual_address;
	uint64_t length;
	struct nvidia_p2p_page_table *page_table;
};

/* Per-fd state stored in file->private_data */
struct nvp2p_fd_state {
	struct list_head regions;  /* list of nvp2p_region */
	struct mutex lock;
	uint64_t next_handle;      /* monotonically increasing handle counter */
};

/*
 * Check whether [va, va+len) overlaps with any existing region on this fd.
 * Caller must hold fd_state->lock.
 */
static bool nvp2p_regions_overlap(struct nvp2p_fd_state *state,
				  uint64_t va, uint64_t len)
{
	struct nvp2p_region *region;
	uint64_t new_end = va + len;

	list_for_each_entry(region, &state->regions, list) {
		uint64_t existing_end = region->virtual_address + region->length;

		if (va < existing_end && new_end > region->virtual_address)
			return true;
	}
	return false;
}

/*
 * Find a region by handle. Caller must hold fd_state->lock.
 */
static struct nvp2p_region *nvp2p_find_region(struct nvp2p_fd_state *state,
					      uint64_t handle)
{
	struct nvp2p_region *region;

	list_for_each_entry(region, &state->regions, list) {
		if (region->handle == handle)
			return region;
	}
	return NULL;
}

/*
 * Release a single region: call nvidia_p2p_put_pages_persistent, free memory.
 * Region must already be removed from the list by the caller.
 */
static void nvp2p_release_region(struct nvp2p_region *region)
{
	int ret;

	pr_debug("nvidia_p2p_pin: releasing region handle=%llu va=0x%llx len=%llu\n",
		 region->handle, region->virtual_address, region->length);

	ret = nvidia_p2p_put_pages_persistent(region->virtual_address,
					      region->page_table, 0);
	if (ret)
		pr_err("nvidia_p2p_pin: put_pages_persistent failed: %d (handle=%llu)\n",
		       ret, region->handle);

	kfree(region);
}

/* ---- Ioctl Handlers ---- */

static int nvp2p_ioctl_pin(struct nvp2p_fd_state *state,
			   unsigned long arg)
{
	struct nvp2p_pin_args kargs;
	struct nvp2p_region *region;
	int ret;

	if (copy_from_user(&kargs, (void __user *)arg, sizeof(kargs)))
		return -EFAULT;

	/* Validate alignment */
	if (kargs.virtual_address & NVP2P_ALIGN_MASK) {
		pr_debug("nvidia_p2p_pin: PIN rejected: VA 0x%llx not 64KB-aligned\n",
			 kargs.virtual_address);
		return -EINVAL;
	}

	/* Validate length */
	if (kargs.length == 0 || (kargs.length & NVP2P_ALIGN_MASK)) {
		pr_debug("nvidia_p2p_pin: PIN rejected: length %llu not valid\n",
			 kargs.length);
		return -EINVAL;
	}

	region = kzalloc(sizeof(*region), GFP_KERNEL);
	if (!region)
		return -ENOMEM;

	mutex_lock(&state->lock);

	/* Check for overlapping regions */
	if (nvp2p_regions_overlap(state, kargs.virtual_address, kargs.length)) {
		mutex_unlock(&state->lock);
		kfree(region);
		pr_debug("nvidia_p2p_pin: PIN rejected: overlapping region exists\n");
		return -EEXIST;
	}

	/* Call NVIDIA persistent P2P API */
	ret = nvidia_p2p_get_pages_persistent(kargs.virtual_address,
					      kargs.length,
					      &region->page_table, 0);
	if (ret) {
		mutex_unlock(&state->lock);
		kfree(region);
		pr_err("nvidia_p2p_pin: get_pages_persistent failed: %d\n", ret);
		return ret;
	}

	/* Assign handle and add to list */
	state->next_handle++;
	region->handle = state->next_handle;
	region->virtual_address = kargs.virtual_address;
	region->length = kargs.length;
	list_add_tail(&region->list, &state->regions);

	/* Fill output fields */
	kargs.handle = region->handle;
	kargs.page_count = region->page_table->entries;
	kargs.page_size = region->page_table->page_size;

	mutex_unlock(&state->lock);

	pr_debug("nvidia_p2p_pin: PIN success: handle=%llu pages=%u page_size=%u\n",
		 kargs.handle, kargs.page_count, kargs.page_size);

	if (copy_to_user((void __user *)arg, &kargs, sizeof(kargs)))
		return -EFAULT;

	return 0;
}

static int nvp2p_ioctl_unpin(struct nvp2p_fd_state *state,
			     unsigned long arg)
{
	struct nvp2p_unpin_args kargs;
	struct nvp2p_region *region;

	if (copy_from_user(&kargs, (void __user *)arg, sizeof(kargs)))
		return -EFAULT;

	mutex_lock(&state->lock);

	region = nvp2p_find_region(state, kargs.handle);
	if (!region) {
		mutex_unlock(&state->lock);
		pr_debug("nvidia_p2p_pin: UNPIN rejected: handle %llu not found\n",
			 kargs.handle);
		return -EINVAL;
	}

	list_del(&region->list);
	mutex_unlock(&state->lock);

	nvp2p_release_region(region);

	pr_debug("nvidia_p2p_pin: UNPIN success: handle=%llu\n", kargs.handle);
	return 0;
}

static int nvp2p_ioctl_get_pages(struct nvp2p_fd_state *state,
				 unsigned long arg)
{
	struct nvp2p_get_pages_args kargs;
	struct nvp2p_region *region;
	uint32_t n;
	uint32_t i;
	uint64_t *buf;

	if (copy_from_user(&kargs, (void __user *)arg, sizeof(kargs)))
		return -EFAULT;

	mutex_lock(&state->lock);

	region = nvp2p_find_region(state, kargs.handle);
	if (!region) {
		mutex_unlock(&state->lock);
		pr_debug("nvidia_p2p_pin: GET_PAGES rejected: handle %llu not found\n",
			 kargs.handle);
		return -EINVAL;
	}

	n = min_t(uint32_t, region->page_table->entries, kargs.buf_count);

	/* Allocate temporary kernel buffer for physical addresses */
	buf = kmalloc_array(n, sizeof(uint64_t), GFP_KERNEL);
	if (!buf) {
		mutex_unlock(&state->lock);
		return -ENOMEM;
	}

	for (i = 0; i < n; i++)
		buf[i] = region->page_table->pages[i]->physical_address;

	kargs.entries_written = n;
	kargs.page_size = region->page_table->page_size;
	memset(kargs.gpu_uuid, 0, sizeof(kargs.gpu_uuid));
	/* gpu_uuid is available in page_table on some driver versions */

	mutex_unlock(&state->lock);

	/* Copy physical addresses to user space */
	if (copy_to_user((void __user *)kargs.phys_addr_buf, buf,
			 n * sizeof(uint64_t))) {
		kfree(buf);
		return -EFAULT;
	}

	kfree(buf);

	/* Copy output args back */
	if (copy_to_user((void __user *)arg, &kargs, sizeof(kargs)))
		return -EFAULT;

	pr_debug("nvidia_p2p_pin: GET_PAGES success: handle=%llu entries=%u\n",
		 kargs.handle, n);
	return 0;
}

/* ---- File Operations ---- */

static int nvp2p_open(struct inode *inode, struct file *file)
{
	struct nvp2p_fd_state *state;

	state = kzalloc(sizeof(*state), GFP_KERNEL);
	if (!state)
		return -ENOMEM;

	INIT_LIST_HEAD(&state->regions);
	mutex_init(&state->lock);
	state->next_handle = 0;

	file->private_data = state;

	pr_debug("nvidia_p2p_pin: device opened\n");
	return 0;
}

static int nvp2p_release(struct inode *inode, struct file *file)
{
	struct nvp2p_fd_state *state = file->private_data;
	struct nvp2p_region *region, *tmp;

	if (!state)
		return 0;

	mutex_lock(&state->lock);
	list_for_each_entry_safe(region, tmp, &state->regions, list) {
		list_del(&region->list);
		pr_debug("nvidia_p2p_pin: auto-releasing region handle=%llu on fd close\n",
			 region->handle);
		nvp2p_release_region(region);
	}
	mutex_unlock(&state->lock);

	kfree(state);
	file->private_data = NULL;

	pr_debug("nvidia_p2p_pin: device closed\n");
	return 0;
}

static long nvp2p_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	struct nvp2p_fd_state *state = file->private_data;

	if (!state)
		return -EBADF;

	pr_debug("nvidia_p2p_pin: ioctl cmd=0x%x\n", cmd);

	switch (cmd) {
	case NVP2P_IOCTL_PIN:
		return nvp2p_ioctl_pin(state, arg);
	case NVP2P_IOCTL_UNPIN:
		return nvp2p_ioctl_unpin(state, arg);
	case NVP2P_IOCTL_GET_PAGES:
		return nvp2p_ioctl_get_pages(state, arg);
	default:
		return -ENOTTY;
	}
}

static const struct file_operations nvp2p_fops = {
	.owner          = THIS_MODULE,
	.open           = nvp2p_open,
	.release        = nvp2p_release,
	.unlocked_ioctl = nvp2p_ioctl,
	.compat_ioctl   = compat_ptr_ioctl,
};

static struct miscdevice nvp2p_misc = {
	.minor = MISC_DYNAMIC_MINOR,
	.name  = "nvidia_p2p",
	.fops  = &nvp2p_fops,
	.mode  = 0666,
};

static int __init nvp2p_init(void)
{
	int ret;

	ret = misc_register(&nvp2p_misc);
	if (ret) {
		pr_err("nvidia_p2p_pin: failed to register misc device: %d\n", ret);
		return ret;
	}

	pr_info("nvidia_p2p_pin: loaded (device: /dev/nvidia_p2p)\n");
	return 0;
}

static void __exit nvp2p_exit(void)
{
	misc_deregister(&nvp2p_misc);
	pr_info("nvidia_p2p_pin: unloaded\n");
}

module_init(nvp2p_init);
module_exit(nvp2p_exit);

MODULE_LICENSE("GPL");
MODULE_AUTHOR("AI Native Systems Research");
MODULE_DESCRIPTION("NVIDIA P2P GPU memory pinning via persistent API");
