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

    // Emit link search path and rpath for shared SPDK/DPDK libraries.
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    // Force all SPDK/DPDK shared libs into DT_NEEDED regardless of symbol usage.
    // SPDK shared libs are underlinked (don't list all their deps in DT_NEEDED),
    // so the consuming binary/dylib must pull everything in.
    println!("cargo:rustc-link-arg=-Wl,--no-as-needed");

    // Link SPDK and DPDK as shared libraries.
    // This ensures a single copy of SPDK/DPDK process-global state exists in
    // the process, enabling multiple component dylibs to share NVMe controllers,
    // hugepage allocations, and the DPDK EAL.
    let spdk_libs = [
        "spdk_env_dpdk",
        "spdk_log",
        "spdk_util",
        "spdk_nvme",
        "spdk_trace",
        "spdk_dma",
        "spdk_keyring",
        "spdk_json",
        "spdk_jsonrpc",
        "spdk_rpc",
        "spdk_sock",
        "spdk_sock_posix",
        "spdk_thread",
    ];

    for lib in &spdk_libs {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }

    // Link DPDK libraries (shared).
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
        "rte_vhost",
        "rte_ethdev",
        "rte_cryptodev",
        "rte_dmadev",
        "rte_hash",
        "rte_net",
        "rte_mbuf",
        "rte_rcu",
        "rte_cmdline",
    ];

    for lib in &dpdk_libs {
        println!("cargo:rustc-link-lib=dylib={lib}");
    }

    // Intel ISA-L is only linked when the installed SPDK build enables it.
    if is_isal_enabled {
        println!("cargo:rustc-link-lib=dylib=isal");
    }

    // Link system libraries that SPDK/DPDK depend on.
    // These must appear as DT_NEEDED in consuming dylibs because SPDK's shared
    // libraries reference them without listing them in their own DT_NEEDED.
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=numa");
    println!("cargo:rustc-link-lib=dylib=uuid");
    println!("cargo:rustc-link-lib=dylib=ssl");
    println!("cargo:rustc-link-lib=dylib=crypto");
    println!("cargo:rustc-link-lib=dylib=m");
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
        .allowlist_function("spdk_nvme_ns_cmd_flush")
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
