//! Compile the dispatcher gRPC contract into a *client* stub.
//!
//! Points at `certus-server-yaml`'s copy of the proto rather than adding a fourth
//! one to the tree (`certus-server`, `certus-server-yaml` and
//! `baseline-generalized-fs` each keep their own). The bench must speak exactly
//! what the server it targets speaks, so sharing that file is the point.
//!
//! `protoc` discovery mirrors `apps/certus-server-yaml/build.rs` and
//! `apps/baseline-generalized-fs/build.rs`: honour `$PROTOC`, else take one from
//! `PATH`, else fetch a pinned release into `OUT_DIR`.
//!
//! CUDA linking is emitted here because the bench declares its own CUDA externs
//! rather than depending on `gpu-services` (see the note in `Cargo.toml`). The
//! search paths mirror `components/gpu-services/build.rs` so a node with CUDA in
//! any of the usual places links without extra configuration.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const PROTOC_VERSION: &str = "25.1";
const PROTO: &str = "../certus-server-yaml/proto/dispatcher.proto";
const PROTO_DIR: &str = "../certus-server-yaml/proto";

fn find_protoc() -> Option<PathBuf> {
    if let Ok(p) = env::var("PROTOC") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(output) = Command::new("which").arg("protoc").output() {
        if output.status.success() {
            let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

fn download_protoc() -> PathBuf {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let protoc_dir = out_dir.join("protoc");
    let protoc_bin = protoc_dir.join("bin").join("protoc");

    if protoc_bin.exists() {
        return protoc_bin;
    }

    let url = format!(
        "https://github.com/protocolbuffers/protobuf/releases/download/v{}/protoc-{}-linux-x86_64.zip",
        PROTOC_VERSION, PROTOC_VERSION
    );

    let zip_path = out_dir.join("protoc.zip");

    let status = Command::new("curl")
        .args(["-sL", "-o"])
        .arg(&zip_path)
        .arg(&url)
        .status()
        .expect("failed to run curl");
    assert!(status.success(), "failed to download protoc from {url}");

    fs::create_dir_all(&protoc_dir).unwrap();
    let status = Command::new("unzip")
        .args(["-q", "-o"])
        .arg(&zip_path)
        .arg("-d")
        .arg(&protoc_dir)
        .status()
        .expect("failed to run unzip");
    assert!(status.success(), "failed to unzip protoc");

    fs::remove_file(&zip_path).ok();
    protoc_bin
}

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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed={PROTO}");
    link_cuda();

    let protoc = find_protoc().unwrap_or_else(|| {
        eprintln!("cargo:warning=protoc not found, downloading v{PROTOC_VERSION}...");
        download_protoc()
    });
    env::set_var("PROTOC", &protoc);

    // Client only: the bench never serves the dispatcher API.
    tonic_build::configure()
        .build_server(false)
        .compile_protos(&[PROTO], &[PROTO_DIR])?;

    Ok(())
}
