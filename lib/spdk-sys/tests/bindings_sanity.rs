//! Sanity tests for spdk-sys FFI bindings.
//!
//! These tests verify that the bindgen-generated types have expected sizes,
//! alignments, and field accessibility. They do NOT call any SPDK functions
//! (which would require a running SPDK environment).

use std::mem;

#[test]
fn spdk_env_opts_is_nonzero_size() {
    assert!(mem::size_of::<spdk_sys::spdk_env_opts>() > 0);
}

#[test]
fn spdk_env_opts_has_name_field() {
    let opts: spdk_sys::spdk_env_opts = unsafe { mem::zeroed() };
    // The name field should be a pointer (null after zeroing).
    assert!(opts.name.is_null());
}

#[test]
fn spdk_env_opts_has_shm_id_field() {
    let mut opts: spdk_sys::spdk_env_opts = unsafe { mem::zeroed() };
    opts.shm_id = -1;
    assert_eq!(opts.shm_id, -1);
}

#[test]
fn spdk_env_opts_default_is_zeroed() {
    let opts = spdk_sys::spdk_env_opts::default();
    assert!(opts.name.is_null());
    assert_eq!(opts.shm_id, 0);
}

#[test]
fn spdk_pci_addr_size_and_fields() {
    let mut addr: spdk_sys::spdk_pci_addr = unsafe { mem::zeroed() };
    addr.domain = 0xabcd;
    addr.bus = 0xff;
    addr.dev = 0x1f;
    addr.func = 7;
    assert_eq!(addr.domain, 0xabcd);
    assert_eq!(addr.bus, 0xff);
    assert_eq!(addr.dev, 0x1f);
    assert_eq!(addr.func, 7);
}

#[test]
fn spdk_pci_id_size_and_fields() {
    let mut id: spdk_sys::spdk_pci_id = unsafe { mem::zeroed() };
    id.class_id = 0x010802;
    id.vendor_id = 0x8086;
    id.device_id = 0x0a54;
    id.subvendor_id = 0x1234;
    id.subdevice_id = 0x5678;
    assert_eq!(id.class_id, 0x010802);
    assert_eq!(id.vendor_id, 0x8086);
    assert_eq!(id.device_id, 0x0a54);
    assert_eq!(id.subvendor_id, 0x1234);
    assert_eq!(id.subdevice_id, 0x5678);
}

#[test]
fn spdk_pci_addr_is_nonzero_size() {
    assert!(mem::size_of::<spdk_sys::spdk_pci_addr>() > 0);
}

#[test]
fn spdk_pci_id_is_nonzero_size() {
    assert!(mem::size_of::<spdk_sys::spdk_pci_id>() > 0);
}

#[test]
fn spdk_pci_device_is_nonzero_size() {
    assert!(mem::size_of::<spdk_sys::spdk_pci_device>() > 0);
}

#[test]
fn function_pointers_exist() {
    // Verify that the function symbols are linked and accessible.
    // We take their address but do NOT call them.
    let _init = spdk_sys::spdk_env_opts_init as *const ();
    let _env_init = spdk_sys::spdk_env_init as *const ();
    let _env_fini = spdk_sys::spdk_env_fini as *const ();
    let _for_each = spdk_sys::spdk_pci_for_each_device as *const ();
    let _get_addr = spdk_sys::spdk_pci_device_get_addr as *const ();
    let _get_id = spdk_sys::spdk_pci_device_get_id as *const ();
    let _get_numa = spdk_sys::spdk_pci_device_get_numa_id as *const ();

    assert!(!_init.is_null());
    assert!(!_env_init.is_null());
    assert!(!_env_fini.is_null());
    assert!(!_for_each.is_null());
    assert!(!_get_addr.is_null());
    assert!(!_get_id.is_null());
    assert!(!_get_numa.is_null());
}
