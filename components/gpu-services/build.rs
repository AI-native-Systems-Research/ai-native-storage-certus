fn main() {
    if cfg!(feature = "gpu") {
        println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
        println!("cargo:rustc-link-search=native=/usr/lib64");
        if let Ok(path) = std::env::var("CUDA_RUNTIME_LIB_PATH") {
            println!("cargo:rustc-link-search=native={path}");
        }
        // Python nvidia-cuda-runtime-cu12 package location (pip install).
        if let Ok(site) = std::env::var("HOME") {
            let pip_path =
                format!("{site}/.local/lib/python3.9/site-packages/nvidia/cuda_runtime/lib");
            if std::path::Path::new(&pip_path).exists() {
                println!("cargo:rustc-link-search=native={pip_path}");
            }
        }
        println!("cargo:rustc-link-lib=dylib=cudart");

        // GDRCopy library (required for GPU BAR1 mapping with SPDK).
        if cfg!(feature = "spdk") {
            if let Ok(path) = std::env::var("GDRCOPY_LIB_PATH") {
                println!("cargo:rustc-link-search=native={path}");
            } else {
                // Project-local GDRCopy build (kernel/modules/gdrcopy/src/).
                #[expect(
                    clippy::expect_used,
                    reason = "build script: cargo always sets CARGO_MANIFEST_DIR; failing the build is the only sensible outcome"
                )]
                let manifest_dir = std::path::PathBuf::from(
                    std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"),
                );
                let gdrcopy_lib = manifest_dir.join("../../kernel/modules/gdrcopy/src");
                if gdrcopy_lib.exists() {
                    #[expect(
                        clippy::expect_used,
                        reason = "build script: the path was just checked to exist, and failing the build is the only sensible outcome if it cannot be resolved"
                    )]
                    let gdrcopy_lib = gdrcopy_lib
                        .canonicalize()
                        .expect("failed to canonicalize GDRCopy lib path");
                    println!("cargo:rustc-link-search=native={}", gdrcopy_lib.display());
                } else {
                    println!("cargo:rustc-link-search=native=/usr/local/lib");
                    println!("cargo:rustc-link-search=native=/usr/local/gdrcopy/lib");
                    println!("cargo:rustc-link-search=native=/usr/lib64");
                }
            }
            println!("cargo:rustc-link-lib=dylib=gdrapi");
        }
    }
}
