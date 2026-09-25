//! Emit CUDA runtime link directives.
//!
//! This crate is the only one in the workload generator that touches a GPU: FR-079 left the
//! agent as the only thing that talks to a Certus mailbox, and the payload buffer lives beside
//! that. It is a workspace member but not a **default** member for exactly this reason, so a
//! plain `cargo build` never asks the linker for `cudart`.
//!
//! It used to live in `workload-gen`, gated behind that crate's `live` feature so the emit path
//! would still build on a machine with no accelerator. The gate is unnecessary here: everything
//! in this crate needs the mailbox and the device, so there is no configuration of it that does
//! not.
//!
//! The search paths mirror `components/gpu-services/build.rs` and
//! `apps/remote-lookup-bench/build.rs` so a node with CUDA in any of the usual places
//! links without extra configuration.

use std::env;
use std::path::PathBuf;

/// Emit CUDA runtime link directives.
fn link_cuda() {
    for dir in [
        "/usr/local/cuda/lib64",
        "/usr/local/cuda/targets/x86_64-linux/lib",
        "/usr/lib64",
    ] {
        println!("cargo:rustc-link-search=native={dir}");
    }
    // Explicit override, and the pip `nvidia-cuda-runtime-cu12` layout.
    println!("cargo:rerun-if-env-changed=CUDA_RUNTIME_LIB_PATH");
    if let Ok(path) = env::var("CUDA_RUNTIME_LIB_PATH") {
        println!("cargo:rustc-link-search=native={path}");
    }
    if let Ok(home) = env::var("HOME") {
        let pip = format!("{home}/.local/lib/python3.9/site-packages/nvidia/cuda_runtime/lib");
        if PathBuf::from(&pip).exists() {
            println!("cargo:rustc-link-search=native={pip}");
        }
    }
    println!("cargo:rustc-link-lib=dylib=cudart");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    link_cuda();
}
