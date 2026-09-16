//! Emit CUDA runtime link directives, but only for the `live` feature.
//!
//! The emit path must build and run on a machine with no accelerator — that is what
//! makes User Story 2 and quickstart Scenarios 1-3 runnable anywhere — so the link
//! directive is gated on `CARGO_FEATURE_LIVE`. A `--no-default-features` build therefore
//! never asks the linker for `cudart`, and a machine without CUDA can still produce
//! traces.
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
    if env::var_os("CARGO_FEATURE_LIVE").is_some() {
        link_cuda();
    }
}
