//! Emit CUDA runtime link directives.
//!
//! The bench declares its own CUDA externs rather than depending on
//! `gpu-services` (see the note in `Cargo.toml`), so the link directives live
//! here. The search paths mirror `components/gpu-services/build.rs` so a node
//! with CUDA in any of the usual places links without extra configuration.
//!
//! The control transport is the `/dev/shm` shmq mailbox (`shm-queue` +
//! `shmq-dispatcher`), which is a plain library dependency — no protobuf/tonic
//! codegen, so nothing else happens in this build script.

use std::env;
use std::path::PathBuf;

/// Emit CUDA runtime link directives, mirroring `gpu-services/build.rs`.
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
    link_cuda();
}
