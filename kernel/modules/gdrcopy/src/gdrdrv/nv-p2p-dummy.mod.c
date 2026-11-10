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

KSYMTAB_FUNC(nvidia_p2p_init_mapping, "", "");
KSYMTAB_FUNC(nvidia_p2p_destroy_mapping, "", "");
KSYMTAB_FUNC(nvidia_p2p_get_pages, "", "");
KSYMTAB_FUNC(nvidia_p2p_put_pages, "", "");
KSYMTAB_FUNC(nvidia_p2p_free_page_table, "", "");
KSYMTAB_FUNC(nvidia_p2p_get_pages_persistent, "", "");
KSYMTAB_FUNC(nvidia_p2p_put_pages_persistent, "", "");

SYMBOL_CRC(nvidia_p2p_init_mapping, 0x45bb0ad7, "");
SYMBOL_CRC(nvidia_p2p_destroy_mapping, 0x180f4b6a, "");
SYMBOL_CRC(nvidia_p2p_get_pages, 0x8561e0cf, "");
SYMBOL_CRC(nvidia_p2p_put_pages, 0xf128f5e3, "");
SYMBOL_CRC(nvidia_p2p_free_page_table, 0xf91e26e6, "");
SYMBOL_CRC(nvidia_p2p_get_pages_persistent, 0xfec75bb0, "");
SYMBOL_CRC(nvidia_p2p_put_pages_persistent, 0x50bb92cd, "");

static const struct modversion_info ____versions[]
__used __section("__versions") = {
	{ 0xbdfb6dbb, "__fentry__" },
	{ 0x5b8239ca, "__x86_return_thunk" },
	{ 0x33bd423, "module_layout" },
};

MODULE_INFO(depends, "");


MODULE_INFO(srcversion, "E6F20D3C2E383CEBEB438A2");
MODULE_INFO(rhelversion, "9.7");
