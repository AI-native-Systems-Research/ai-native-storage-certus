#include <linux/module.h>
#define INCLUDE_VERMAGIC
#include <linux/build-salt.h>
#include <linux/elfnote-lto.h>
#include <linux/export-internal.h>
#include <linux/vermagic.h>
#include <linux/compiler.h>

BUILD_SALT;
BUILD_LTO_INFO;

MODULE_INFO(vermagic, VERMAGIC_STRING);
MODULE_INFO(name, KBUILD_MODNAME);

__visible struct module __this_module
__section(".gnu.linkonce.this_module") = {
	.name = KBUILD_MODNAME,
	.init = init_module,
#ifdef CONFIG_MODULE_UNLOAD
	.exit = cleanup_module,
#endif
	.arch = MODULE_ARCH_INIT,
};

#ifdef CONFIG_MITIGATION_RETPOLINE
MODULE_INFO(retpoline, "Y");
#endif



static const struct modversion_info ____versions[]
__used __section("__versions") = {
	{ 0xfec75bb0, "nvidia_p2p_get_pages_persistent" },
	{ 0xf0fdf6cb, "__stack_chk_fail" },
	{ 0x87767b8d, "single_release" },
	{ 0x19874e97, "single_open" },
	{ 0x3c93bd35, "seq_printf" },
	{ 0xa7946130, "proc_remove" },
	{ 0x6bc3fbc0, "__unregister_chrdev" },
	{ 0xcefb0c9f, "__mutex_init" },
	{ 0x7a8006ac, "pcpu_hot" },
	{ 0x7a4c9de9, "address_space_init_once" },
	{ 0xf978b631, "__register_chrdev" },
	{ 0x656e4a6e, "snprintf" },
	{ 0x7ab61b1a, "proc_mkdir_mode" },
	{ 0x7d232838, "proc_create" },
	{ 0x8a35b432, "sme_me_mask" },
	{ 0x8de7163a, "remap_pfn_range" },
	{ 0xe1537255, "__list_del_entry_valid" },
	{ 0x13c49cc2, "_copy_from_user" },
	{ 0x6b10bee1, "_copy_to_user" },
	{ 0x668b19a1, "down_read" },
	{ 0x53b954a2, "up_read" },
	{ 0xd96534ad, "seq_read" },
	{ 0xfd0f73e1, "seq_lseek" },
	{ 0xb8cf473f, "param_ops_int" },
	{ 0xbdfb6dbb, "__fentry__" },
	{ 0x5b8239ca, "__x86_return_thunk" },
	{ 0x92997ed8, "_printk" },
	{ 0xce807a25, "up_write" },
	{ 0xf128f5e3, "nvidia_p2p_put_pages" },
	{ 0x37a0cba, "kfree" },
	{ 0x50bb92cd, "nvidia_p2p_put_pages_persistent" },
	{ 0x57bc19d2, "down_write" },
	{ 0xf91e26e6, "nvidia_p2p_free_page_table" },
	{ 0xf074b122, "unmap_mapping_range" },
	{ 0xf9636917, "kmalloc_caches" },
	{ 0xcdb16d0f, "kmalloc_trace" },
	{ 0x7b4da6ff, "__init_rwsem" },
	{ 0x8561e0cf, "nvidia_p2p_get_pages" },
	{ 0xd6b33026, "cpu_khz" },
	{ 0x4dfa8d4b, "mutex_lock" },
	{ 0x364c23ad, "mutex_is_locked" },
	{ 0x3213f038, "mutex_unlock" },
	{ 0x68f31cbd, "__list_add_valid" },
	{ 0x33bd423, "module_layout" },
};

MODULE_INFO(depends, "nv-p2p-dummy");


MODULE_INFO(srcversion, "0569F3F2055D013BAAB59FD");
MODULE_INFO(rhelversion, "9.7");
