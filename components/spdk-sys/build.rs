//! Build script for spdk-sys: generates FFI bindings via bindgen and links SPDK/DPDK libraries.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Locate the SPDK source and build directories relative to the workspace root.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let deps_dir = manifest_dir.join("../../deps");

    let spdk_src = deps_dir.join("spdk");
    if !spdk_src.exists() {
        panic!(
            "\n\nerror: SPDK source not found at deps/spdk/.\n\
             Clone it first:  git submodule update --init deps/spdk\n\
             Then build it:   deps/build_spdk.sh\n\n"
        );
    }

    let spdk_build = deps_dir.join("spdk-build");
    let spdk_build = spdk_build.canonicalize().unwrap_or_else(|_| {
        panic!(
            "\n\nerror: SPDK build directory not found at deps/spdk-build/.\n\
             SPDK source exists at deps/spdk/ but has not been built yet.\n\
             Run:  deps/build_spdk.sh\n\n"
        );
    });

    let include_dir = spdk_build.join("include");
    let lib_dir = spdk_build.join("lib");
    let config_header = include_dir.join("spdk/config.h");
    let config_header = fs::read_to_string(&config_header).unwrap_or_else(|_| {
        panic!(
            "\n\nerror: SPDK config header not found at {}.\n\
             Rebuild SPDK with: deps/build_spdk.sh\n\n",
            config_header.display()
        );
    });
    let is_isal_enabled = config_header
        .lines()
        .any(|line| line.trim() == "#define SPDK_CONFIG_ISAL 1");

    // Emit link search path.
    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    // Link SPDK libraries we need (static, all +whole-archive).
    //
    // Rust only emits native library -l flags for symbols that Rust code
    // references directly.  SPDK is a C library stack where spdk_nvme and
    // spdk_sock_posix (both +whole-archive) pull in all their object files,
    // creating C-level references to spdk_log, spdk_util, spdk_env_dpdk, etc.
    // that no Rust code calls directly.  Rust therefore silently drops the
    // -l flags for those support libs, leaving the whole-archive objects with
    // unresolved symbols.  Using +whole-archive on every SPDK lib forces them
    // all into the link unconditionally, so the C-level cross-references are
    // always satisfied regardless of what Rust code calls.
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_env_dpdk");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_log");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_util");

    // NVMe driver — +whole-archive so the PCI driver constructor
    // (SPDK_PCI_DRIVER_REGISTER) is included by the linker.  Without this,
    // spdk_pci_get_driver("nvme") returns NULL and no NVMe devices are enumerated.
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_nvme");

    // Transitive dependencies of spdk_nvme (from spdk_nvme.pc / spdk_sock.pc):
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_trace");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_dma");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_keyring");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_json");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_jsonrpc");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_rpc");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_sock");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_sock_posix");
    println!("cargo:rustc-link-lib=static:+whole-archive=spdk_thread");

    // Link DPDK libraries (static).
    // These are the libraries that spdk_env_dpdk depends on, discovered from
    // the spdk_env_dpdk.pc and libdpdk.pc files. We link them manually because
    // DPDK's .pc files use -l:libfoo.a syntax which the Rust pkg-config crate
    // cannot handle.
    let dpdk_libs = [
        "rte_eal",
        "rte_kvargs",
        "rte_log",
        "rte_telemetry",
        "rte_argparse",
        "rte_mempool_ring",
        "rte_mempool",
        "rte_ring",
        "rte_bus_pci",
        "rte_bus_vdev",
        "rte_pci",
        "rte_power",
        "rte_timer",
        "rte_power_acpi",
        "rte_power_amd_pstate",
        "rte_power_cppc",
        "rte_power_intel_pstate",
        "rte_power_intel_uncore",
        "rte_power_kvm_vm",
        "rte_vhost",
        "rte_ethdev",
        "rte_meter",
        "rte_cryptodev",
        "rte_dmadev",
        "rte_hash",
        "rte_net",
        "rte_mbuf",
        "rte_rcu",
        "rte_cmdline",
        "rte_stack",
    ];

    for lib in &dpdk_libs {
        println!("cargo:rustc-link-lib=static:+whole-archive={lib}");
    }

    // Intel ISA-L is only linked when the installed SPDK build enables it.
    // Some repo setups build SPDK with CONFIG_ISAL=n, in which case libisal
    // is neither installed nor required by the generated SPDK archives.
    if is_isal_enabled {
        println!("cargo:rustc-link-lib=static:+whole-archive=isal");
    }

    // Link system libraries that SPDK/DPDK depend on.
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=numa");
    println!("cargo:rustc-link-lib=dylib=uuid");
    println!("cargo:rustc-link-lib=dylib=ssl");
    println!("cargo:rustc-link-lib=dylib=crypto");
    println!("cargo:rustc-link-lib=dylib=m");
    // Link the FUSE library (CUSE support) — required by spdk_nvme CUSE code.
    // Link libfuse3 (preferred). Do not link generic 'fuse' to avoid
    // requiring unversioned dev symlinks on the host.
    println!("cargo:rustc-link-lib=dylib=fuse3");

    // Detect the GCC internal include path so that clang (used by bindgen)
    // can resolve `#include_next <limits.h>` from the system headers.
    let gcc_include = std::process::Command::new("gcc")
        .args(["-print-file-name=include"])
        .output()
        .ok()
        .and_then(|o| {
            let p = String::from_utf8(o.stdout).ok()?.trim().to_string();
            if p.is_empty() || p == "include" {
                None
            } else {
                Some(p)
            }
        });

    // Generate bindings with bindgen.
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", include_dir.display()));

    if let Some(ref gcc_inc) = gcc_include {
        builder = builder.clang_arg(format!("-I{gcc_inc}"));
    }

    let bindings = builder
        .allowlist_function("spdk_env_opts_init")
        .allowlist_function("spdk_env_init")
        .allowlist_function("spdk_env_fini")
        .allowlist_function("spdk_pci_enumerate")
        .allowlist_function("spdk_pci_for_each_device")
        .allowlist_function("spdk_pci_get_driver")
        .allowlist_function("spdk_pci_device_get_addr")
        .allowlist_function("spdk_pci_device_get_id")
        .allowlist_function("spdk_pci_device_get_domain")
        .allowlist_function("spdk_pci_device_get_bus")
        .allowlist_function("spdk_pci_device_get_dev")
        .allowlist_function("spdk_pci_device_get_func")
        .allowlist_function("spdk_pci_device_get_vendor_id")
        .allowlist_function("spdk_pci_device_get_device_id")
        .allowlist_function("spdk_pci_device_get_subvendor_id")
        .allowlist_function("spdk_pci_device_get_subdevice_id")
        .allowlist_function("spdk_pci_device_get_numa_id")
        .allowlist_function("spdk_pci_device_get_serial_number")
        // NVMe driver: probe, attach, detach
        .allowlist_function("spdk_nvme_probe")
        .allowlist_function("spdk_nvme_detach")
        // NVMe controller
        .allowlist_function("spdk_nvme_ctrlr_get_num_ns")
        .allowlist_function("spdk_nvme_ctrlr_get_ns")
        .allowlist_function("spdk_nvme_ctrlr_alloc_io_qpair")
        .allowlist_function("spdk_nvme_ctrlr_free_io_qpair")
        .allowlist_function("spdk_nvme_ctrlr_process_admin_completions")
        .allowlist_function("spdk_nvme_ctrlr_get_default_ctrlr_opts")
        .allowlist_function("spdk_nvme_ctrlr_reset")
        .allowlist_function("spdk_nvme_ctrlr_get_data")
        // NVMe admin commands
        .allowlist_function("spdk_nvme_ctrlr_cmd_admin_raw")
        // NVMe namespace management
        .allowlist_function("spdk_nvme_ctrlr_create_ns")
        .allowlist_function("spdk_nvme_ctrlr_attach_ns")
        .allowlist_function("spdk_nvme_ctrlr_delete_ns")
        .allowlist_function("spdk_nvme_ctrlr_format")
        .allowlist_function("spdk_nvme_ctrlr_get_id")
        // NVMe namespace
        .allowlist_function("spdk_nvme_ns_is_active")
        .allowlist_function("spdk_nvme_ns_get_data")
        .allowlist_function("spdk_nvme_ns_get_sector_size")
        .allowlist_function("spdk_nvme_ns_get_num_sectors")
        .allowlist_function("spdk_nvme_ns_get_size")
        // NVMe I/O
        .allowlist_function("spdk_nvme_ns_cmd_read")
        .allowlist_function("spdk_nvme_ns_cmd_write")
        .allowlist_function("spdk_nvme_ns_cmd_write_zeroes")
        .allowlist_function("spdk_nvme_qpair_process_completions")
        // DMA memory allocation
        .allowlist_function("spdk_dma_zmalloc")
        .allowlist_function("spdk_dma_free")
        .allowlist_function("spdk_zmalloc")
        .allowlist_function("spdk_free")
        // Types
        .allowlist_type("spdk_env_opts")
        .allowlist_type("spdk_pci_addr")
        .allowlist_type("spdk_pci_id")
        .allowlist_type("spdk_pci_device")
        .allowlist_type("spdk_pci_driver")
        .allowlist_type("spdk_nvme_ctrlr")
        .allowlist_type("spdk_nvme_ctrlr_data")
        .opaque_type("spdk_nvme_ctrlr_data")
        .allowlist_type("spdk_nvme_ctrlr_opts")
        .allowlist_type("spdk_nvme_ns")
        .allowlist_type("spdk_nvme_qpair")
        .allowlist_type("spdk_nvme_transport_id")
        .allowlist_type("spdk_nvme_cpl")
        .allowlist_type("spdk_nvme_io_qpair_opts")
        .allowlist_type("spdk_nvme_cmd")
        .allowlist_type("spdk_nvme_ns_data")
        .allowlist_type("spdk_nvme_format")
        .allowlist_type("spdk_nvme_ctrlr_list")
        .allowlist_var("SPDK_PCI_.*")
        .allowlist_var("SPDK_NVME_TRANSPORT_.*")
        .derive_debug(true)
        .derive_default(true)
        // Disable layout tests — SPDK NVMe spec headers use C bitfields that
        // bindgen cannot always reproduce with correct size. The bindings are
        // still usable; only the compile-time size assertions fail.
        .layout_tests(false)
        .generate()
        .expect("Failed to generate SPDK bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Failed to write bindings.rs");

    // Tell cargo to re-run if the wrapper header changes.
    println!("cargo:rerun-if-changed=wrapper.h");
    println!(
        "cargo:rerun-if-changed={}",
        spdk_build.join("lib").display()
    );
}
